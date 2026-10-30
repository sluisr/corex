//! Context window of the active model, as its provider reports it.
//!
//! Providers disagree on where the number lives (Anthropic `max_input_tokens`, Google
//! `inputTokenLimit`, OpenRouter `context_length`, ...), and a window is a property of the model,
//! not of the provider. The lookup runs in the background when a model is selected and the answer
//! is kept in memory, so `Config::context_window` stays a plain synchronous read.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use reqwest::Url;
use serde_json::Value;

use crate::providers::ProviderKind;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// A lookup that found nothing (offline, unknown model, no such field) is not repeated sooner.
const RETRY_AFTER: Duration = Duration::from_secs(600);
/// Fields providers use for the input window, in the order tried.
const WINDOW_FIELDS: &[&str] = &[
    "max_input_tokens",
    "inputTokenLimit",
    "context_length",
    "context_window",
    "max_context_length",
    "max_model_len",
    "n_ctx",
];
/// Anthropic's models API reports the 1M window of the models that offer it, but that window
/// needs a beta header Corex does not send; past this a request is rejected.
const ANTHROPIC_USABLE_WINDOW: usize = 200_000;

struct Entry {
    window: Option<usize>,
    checked: Instant,
}

fn cache() -> &'static Mutex<HashMap<String, Entry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(base_url: &str, model: &str) -> String {
    format!("{}|{}", base_url.trim_end_matches('/').to_ascii_lowercase(), model)
}

/// Window the provider reported for `model` at `base_url`, once the lookup has finished.
pub fn detected_window(base_url: &str, model: &str) -> Option<usize> {
    let map = cache().lock().unwrap_or_else(|e| e.into_inner());
    map.get(&cache_key(base_url, model)).and_then(|e| e.window)
}

/// True when this caller should run the lookup: none ran yet, or the last one found nothing and
/// is old enough to try again. Marks the lookup as started, so callers never run it twice at once.
fn claim(key: &str) -> bool {
    let mut map = cache().lock().unwrap_or_else(|e| e.into_inner());
    match map.get(key) {
        Some(e) if e.window.is_some() || e.checked.elapsed() < RETRY_AFTER => false,
        _ => {
            map.insert(key.to_string(), Entry { window: None, checked: Instant::now() });
            true
        }
    }
}

fn record(key: &str, window: usize) {
    let mut map = cache().lock().unwrap_or_else(|e| e.into_inner());
    map.insert(key.to_string(), Entry { window: Some(window), checked: Instant::now() });
}

/// Starts the background lookup of `model`'s window; a no-op for providers that do not publish
/// it, when no runtime is running, or when the answer is already known.
pub fn spawn_lookup(http: reqwest::Client, kind: ProviderKind, base_url: String, api_key: String, model: String) {
    let needs_key = matches!(
        kind,
        ProviderKind::Anthropic | ProviderKind::Google | ProviderKind::Groq | ProviderKind::Mistral | ProviderKind::DeepSeek
    );
    if needs_key && api_key.trim().is_empty() {
        return;
    }
    let targets = lookup_targets(kind, &base_url, &model);
    if targets.is_empty() {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let key = cache_key(&base_url, &model);
    if !claim(&key) {
        return;
    }
    runtime.spawn(async move {
        for (url, shape) in targets {
            if let Some(window) = fetch(&http, kind, &url, &api_key, &model, shape).await {
                record(&key, window);
                return;
            }
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The response describes the model itself.
    Single,
    /// The response lists models; the entry whose `id` matches is the one.
    List,
}

/// Where to ask, in order. Empty for providers that do not report a window.
fn lookup_targets(kind: ProviderKind, base_url: &str, model: &str) -> Vec<(Url, Shape)> {
    let targets: Vec<(String, Shape)> = match kind {
        ProviderKind::Anthropic => {
            let base = base_url.trim_end_matches('/');
            let base = if base.ends_with("/v1") { base.to_string() } else { format!("{base}/v1") };
            vec![(join(&base, &["models", model]), Shape::Single)]
        }
        // The catalog URL is the OpenAI-compatible shim; the native API sits one level up.
        ProviderKind::Google => {
            let base = base_url.trim_end_matches('/');
            let base = base.strip_suffix("/openai").unwrap_or(base);
            vec![(join(base, &["models", model]), Shape::Single)]
        }
        ProviderKind::OpenRouter | ProviderKind::DeepSeek => vec![(join(base_url, &["models"]), Shape::List)],
        ProviderKind::Groq | ProviderKind::Mistral | ProviderKind::GenericOpenAi => vec![
            (join(base_url, &["models", model]), Shape::Single),
            (join(base_url, &["models"]), Shape::List),
        ],
        // These APIs return no window for a model (OpenAI, GitHub) or are not remote.
        ProviderKind::OpenAi | ProviderKind::GitHub | ProviderKind::Local => Vec::new(),
    };
    targets
        .into_iter()
        .filter_map(|(url, shape)| Url::parse(&url).ok().map(|u| (u, shape)))
        .collect()
}

/// `base` with `segments` appended, percent-encoded (model ids can contain `/` and `:`).
fn join(base: &str, segments: &[&str]) -> String {
    match Url::parse(base) {
        Ok(mut url) => {
            if let Ok(mut path) = url.path_segments_mut() {
                path.pop_if_empty().extend(segments);
            }
            url.to_string()
        }
        Err(_) => String::new(),
    }
}

async fn fetch(http: &reqwest::Client, kind: ProviderKind, url: &Url, api_key: &str, model: &str, shape: Shape) -> Option<usize> {
    let mut request = http.get(url.clone()).timeout(REQUEST_TIMEOUT);
    request = match kind {
        ProviderKind::Anthropic => request.header("x-api-key", api_key).header("anthropic-version", "2023-06-01"),
        ProviderKind::Google => request.header("x-goog-api-key", api_key),
        _ if !api_key.trim().is_empty() => request.bearer_auth(api_key),
        _ => request,
    };
    let body: Value = request.send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
    let window = match shape {
        Shape::Single => window_in(&body),
        Shape::List => find_in_list(&body, model).and_then(window_in),
    }?;
    Some(if kind == ProviderKind::Anthropic { window.min(ANTHROPIC_USABLE_WINDOW) } else { window })
}

/// The window a model description carries, if it has a plausible one.
fn window_in(model: &Value) -> Option<usize> {
    WINDOW_FIELDS
        .iter()
        .find_map(|field| model.get(*field).and_then(Value::as_u64))
        .filter(|&tokens| (1_000..=100_000_000).contains(&tokens))
        .map(|tokens| tokens as usize)
}

fn find_in_list<'a>(body: &'a Value, model: &str) -> Option<&'a Value> {
    let entries = body.get("data").or(Some(body))?.as_array()?;
    entries.iter().find(|entry| entry.get("id").and_then(Value::as_str) == Some(model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn urls(kind: ProviderKind, base: &str, model: &str) -> Vec<String> {
        lookup_targets(kind, base, model).into_iter().map(|(u, _)| u.to_string()).collect()
    }

    #[test]
    fn reads_the_field_each_provider_uses() {
        assert_eq!(window_in(&json!({"max_input_tokens": 200000})), Some(200_000));
        assert_eq!(window_in(&json!({"inputTokenLimit": 1048576})), Some(1_048_576));
        assert_eq!(window_in(&json!({"context_length": 131072})), Some(131_072));
        assert_eq!(window_in(&json!({"context_window": 32768})), Some(32_768));
        assert_eq!(window_in(&json!({"max_context_length": 262144})), Some(262_144));
        assert_eq!(window_in(&json!({"max_model_len": 8192})), Some(8_192));
    }

    #[test]
    fn ignores_missing_or_implausible_values() {
        assert_eq!(window_in(&json!({"id": "m"})), None);
        assert_eq!(window_in(&json!({"context_length": 0})), None);
        assert_eq!(window_in(&json!({"context_length": 12})), None);
        assert_eq!(window_in(&json!({"context_length": "128k"})), None);
    }

    #[test]
    fn reads_deepseeks_models_response() {
        let body = json!({"object": "list", "data": [
            {"id": "deepseek-flash", "context_window": 1048576, "max_output_tokens": 393216},
            {"id": "deepseek-v4-pro", "context_window": 1048576, "max_output_tokens": 393216},
        ]});
        assert_eq!(find_in_list(&body, "deepseek-v4-pro").and_then(window_in), Some(1_048_576));
    }

    #[test]
    fn finds_the_model_in_a_list() {
        let body = json!({"data": [
            {"id": "a/one", "context_length": 8000},
            {"id": "a/two", "context_length": 64000},
        ]});
        assert_eq!(find_in_list(&body, "a/two").and_then(window_in), Some(64_000));
        assert!(find_in_list(&body, "a/three").is_none());
        assert_eq!(find_in_list(&json!([{"id": "x", "context_window": 4096}]), "x").and_then(window_in), Some(4_096));
    }

    #[test]
    fn builds_each_providers_lookup_url() {
        assert_eq!(
            urls(ProviderKind::Anthropic, "https://api.anthropic.com/v1", "claude-opus-5-5"),
            ["https://api.anthropic.com/v1/models/claude-opus-5-5"]
        );
        assert_eq!(
            urls(ProviderKind::Anthropic, "https://api.anthropic.com", "m"),
            ["https://api.anthropic.com/v1/models/m"]
        );
        assert_eq!(
            urls(ProviderKind::Google, "https://generativelanguage.googleapis.com/v1beta/openai", "gemini-3.5-flash"),
            ["https://generativelanguage.googleapis.com/v1beta/models/gemini-3.5-flash"]
        );
        assert_eq!(
            urls(ProviderKind::OpenRouter, "https://openrouter.ai/api/v1/", "x/y"),
            ["https://openrouter.ai/api/v1/models"]
        );
        assert_eq!(
            urls(ProviderKind::DeepSeek, "https://api.deepseek.com", "deepseek-flash"),
            ["https://api.deepseek.com/models"]
        );
        assert_eq!(
            urls(ProviderKind::Groq, "https://api.groq.com/openai/v1", "llama-3.3-70b-versatile"),
            [
                "https://api.groq.com/openai/v1/models/llama-3.3-70b-versatile",
                "https://api.groq.com/openai/v1/models"
            ]
        );
    }

    #[test]
    fn model_ids_are_encoded_in_the_path() {
        let found = urls(ProviderKind::GenericOpenAi, "https://host/v1", "org/model?x#y");
        assert_eq!(found[0], "https://host/v1/models/org%2Fmodel%3Fx%23y");
    }

    #[test]
    fn providers_without_a_window_field_are_not_queried() {
        for kind in [ProviderKind::OpenAi, ProviderKind::GitHub, ProviderKind::Local] {
            assert!(lookup_targets(kind, "https://host/v1", "m").is_empty());
        }
    }

    #[test]
    fn a_lookup_runs_once_until_it_fails_and_the_retry_delay_passes() {
        let key = cache_key("https://claim.test/v1", "once");
        assert!(claim(&key));
        assert!(!claim(&key), "already in flight");
        record(&key, 50_000);
        assert!(!claim(&key), "answered");
        assert_eq!(detected_window("https://claim.test/v1/", "once"), Some(50_000));
    }
}
