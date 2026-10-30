use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};

fn default_flash_temperature() -> f32 { 1.0 }
fn default_flash_reasoning() -> String { "dynamic".to_string() }

/// Depth used by the `dynamic` reasoning mode on turns that only answer a tool result (~200ms).
pub const DYNAMIC_TOOL_TURN_EFFORT: &str = "low";
/// Depth used by the `dynamic` reasoning mode on every other turn.
pub const DYNAMIC_DEFAULT_EFFORT: &str = "high";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashSettings {
    #[serde(default = "default_flash_temperature")]
    pub temperature: f32,
    #[serde(rename = "reasoningEffort", default = "default_flash_reasoning")]
    pub reasoning_effort: String,
}

impl Default for FlashSettings {
    fn default() -> Self {
        Self {
            temperature: default_flash_temperature(),
            reasoning_effort: default_flash_reasoning(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProSettings {
    #[serde(rename = "reasoningEffort", default = "default_pro_reasoning")]
    pub reasoning_effort: String,
}

fn default_pro_reasoning() -> String { "high".to_string() }

impl Default for ProSettings {
    fn default() -> Self {
        Self {
            reasoning_effort: default_pro_reasoning(),
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Subset of settings a workspace (`<repo>/.corex/settings.json`) may override.
/// Every field is optional so that absent keys keep the user's global values.
/// Security-sensitive keys are parsed only to warn that they are ignored.
#[derive(Debug, Default, Deserialize)]
struct LocalOverrides {
    model: Option<String>,
    temperature: Option<f32>,
    reasoning_effort: Option<String>,
    auto_compact: Option<bool>,
    compact_threshold_tokens: Option<usize>,
    history_token_limit: Option<usize>,
    context_window_tokens: Option<usize>,
    local_prompt_lite: Option<bool>,
    yolo_mode: Option<bool>,
    local_llm_enabled: Option<bool>,
}

/// Where `Config::context_window` got its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowSource {
    /// `context_window_tokens` in `settings.json`.
    Settings,
    /// The model's `context_window` in the provider list.
    Catalog,
    /// The provider's own models API.
    Provider,
    /// A guess per provider; the model may differ.
    Default,
}

impl WindowSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Settings => "settings.json",
            Self::Catalog => "provider list",
            Self::Provider => "reported by the provider",
            Self::Default => "default for this provider, not confirmed for this model",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_api_key")]
    pub api_key: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_reasoning_effort")]
    pub reasoning_effort: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub flash_settings: FlashSettings,
    #[serde(default)]
    pub pro_settings: ProSettings,
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
    #[serde(default, skip)]
    pub sudo_password: Option<String>,
    #[serde(default)]
    pub yolo_mode: bool,
    #[serde(default)]
    pub target_dir: PathBuf,
    #[serde(default = "default_local_llm_enabled")]
    pub local_llm_enabled: bool,
    #[serde(default = "default_local_llm_url")]
    pub local_llm_url: String,
    #[serde(default = "default_local_llm_model")]
    pub local_llm_model: String,
    #[serde(default = "default_auto_compact")]
    pub auto_compact: bool,
    /// Estimated prompt size that triggers auto-compaction. Unset: 80% of the context window.
    /// (`95000`, the old fixed default that earlier versions wrote to `settings.json`, also
    /// counts as unset.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_threshold_tokens: Option<usize>,
    /// Hard cap on the history sent per request; older turns are dropped above it.
    /// Unset: 90% of the context window, and never below the compaction threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_token_limit: Option<usize>,
    /// Context window of the active model. Unset: the provider's default
    /// (see `providers::default_context_window`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<usize>,
    /// Use a reduced system prompt for small local models (Gemma 2B, Llama 3B, etc.).
    /// Lite mode strips complex tool enforcement instructions that confuse SLMs.
    #[serde(default)]
    pub local_prompt_lite: bool,
    /// Extra shell commands (binary names or `cmd subcommand` prefixes) that
    /// are treated as safe without requiring user confirmation.
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    /// User-defined providers (merged over the built-in catalog, see `providers.rs`).
    #[serde(default)]
    pub providers: Vec<crate::providers::ProviderConfig>,
    /// Name of the provider currently in use. `None` keeps the legacy single-endpoint
    /// behavior (`base_url` / `api_key`, i.e. DeepSeek by default).
    #[serde(default)]
    pub active_provider: Option<String>,
    /// Persisted API keys for external providers (e.g. google, openai, anthropic).
    #[serde(default)]
    pub provider_api_keys: HashMap<String, String>,
}

fn default_auto_compact() -> bool {
    true
}

/// Compaction threshold that earlier versions applied to every model and saved to `settings.json`.
const LEGACY_COMPACT_THRESHOLD_TOKENS: usize = 95_000;

fn default_local_llm_enabled() -> bool {
    false
}

fn default_local_llm_url() -> String {
    std::env::var("COREX_LOCAL_LLM_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8080/v1".to_string())
}

fn default_local_llm_model() -> String {
    std::env::var("COREX_LOCAL_LLM_MODEL")
        .unwrap_or_else(|_| "local-model".to_string())
}


/// Default API key: only keys issued for the default provider (DeepSeek) or Corex itself.
/// `OPENAI_API_KEY` is deliberately NOT used here: sending an OpenAI key to the DeepSeek endpoint
/// leaks a credential to a third party. It is only honoured when `base_url` points to OpenAI
/// (see [`Config::load_with_workspace`]).
fn default_api_key() -> String {
    std::env::var("COREX_API_KEY")
        .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
        .unwrap_or_default()
}

/// Returns the host of `url`, lowercased, if it parses.
fn url_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

fn host_is_or_sub(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{}", domain))
}

fn is_loopback_host(host: &str) -> bool {
    if matches!(host, "localhost" | "::1" | "[::1]") {
        return true;
    }
    let clean = host.trim_start_matches('[').trim_end_matches(']');
    clean
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// A remote `base_url` receives the API key in every request, so it must use TLS.
/// Plain HTTP is only accepted for loopback (local LLM servers).
pub fn validate_base_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid base_url '{}': {}", url, e))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if is_loopback_host(&host) => Ok(()),
        other => Err(format!(
            "base_url '{}' uses '{}': remote endpoints must use https (the API key is sent with every request)",
            url, other
        )),
    }
}

/// True when `base_url` points to DeepSeek's official API.
pub fn is_deepseek_base_url(url: &str) -> bool {
    url_host(url).map(|h| host_is_or_sub(&h, "deepseek.com")).unwrap_or(false)
}

/// True when `base_url` points to OpenAI's official API.
pub fn is_openai_base_url(url: &str) -> bool {
    url_host(url).map(|h| host_is_or_sub(&h, "openai.com")).unwrap_or(false)
}

/// True when `base_url` points to Anthropic's official API.
pub fn is_anthropic_base_url(url: &str) -> bool {
    url_host(url).map(|h| host_is_or_sub(&h, "anthropic.com")).unwrap_or(false)
}

/// True when `base_url` points to Google Gemini's official API.
pub fn is_google_base_url(url: &str) -> bool {
    url_host(url).map(|h| host_is_or_sub(&h, "generativelanguage.googleapis.com") || host_is_or_sub(&h, "aiplatform.googleapis.com")).unwrap_or(false)
}

/// True for "pro"/reasoner model profiles. Matches whole name segments so that names such as
/// `improved-coder` or `approx-7b` are not mistaken for the pro profile.
pub fn is_pro_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("reasoner")
        || m.split(['-', '_', ':', '/', '.'])
            .any(|seg| seg == "pro")
}

fn default_base_url_static() -> &'static str {
    "https://api.deepseek.com"
}

fn default_base_url() -> String {
    std::env::var("COREX_BASE_URL")
        .or_else(|_| std::env::var("DEEPSEEK_BASE_URL"))
        .unwrap_or_else(|_| default_base_url_static().to_string())
}

fn default_model() -> String {
    std::env::var("COREX_MODEL")
        .or_else(|_| std::env::var("DEEPSEEK_MODEL"))
        .unwrap_or_else(|_| "deepseek-flash".to_string())
}

fn default_reasoning_effort() -> String {
    "high".to_string()
}

fn default_temperature() -> f32 {
    1.0
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: default_api_key(),
            base_url: default_base_url(),
            model: default_model(),
            reasoning_effort: default_reasoning_effort(),
            temperature: default_temperature(),
            flash_settings: FlashSettings::default(),
            pro_settings: ProSettings::default(),
            mcp_servers: HashMap::new(),
            sudo_password: None,
            yolo_mode: false,
            target_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            local_llm_enabled: default_local_llm_enabled(),
            local_llm_url: default_local_llm_url(),
            local_llm_model: default_local_llm_model(),
            auto_compact: default_auto_compact(),
            compact_threshold_tokens: None,
            history_token_limit: None,
            context_window_tokens: None,
            local_prompt_lite: false,
            allowed_commands: Vec::new(),
            providers: Vec::new(),
            active_provider: None,
            provider_api_keys: HashMap::new(),
        }
    }
}

impl Config {
    /// Context window of the active model, in tokens.
    pub fn context_window(&self) -> usize {
        self.context_window_with_source().0
    }

    /// The window and where it comes from, most specific first: `context_window_tokens`, the
    /// model's `context_window` in the provider list, what the provider reported
    /// (see `model_info`), and finally the provider's default.
    pub fn context_window_with_source(&self) -> (usize, WindowSource) {
        if let Some(tokens) = self.context_window_tokens.filter(|&t| t > 0) {
            return (tokens, WindowSource::Settings);
        }
        if let Some(tokens) = self.catalog_context_window() {
            return (tokens, WindowSource::Catalog);
        }
        if let Some(tokens) = crate::model_info::detected_window(&self.endpoint().0, &self.model) {
            return (tokens, WindowSource::Provider);
        }
        (crate::providers::default_context_window(self.provider_kind()), WindowSource::Default)
    }

    fn catalog_context_window(&self) -> Option<usize> {
        self.all_providers()
            .into_iter()
            .filter(|p| self.active_provider.as_ref().is_none_or(|name| *name == p.name))
            .flat_map(|p| p.models)
            .find(|m| m.id == self.model && m.context_window.is_some_and(|t| t > 0))
            .and_then(|m| m.context_window)
    }

    /// Prompt size, in tokens, above which auto-compaction runs.
    pub fn effective_compact_threshold(&self) -> usize {
        match self.compact_threshold_tokens {
            Some(t) if t > 0 && t != LEGACY_COMPACT_THRESHOLD_TOKENS => t,
            _ => self.context_window() / 10 * 8,
        }
    }

    /// Prompt size, in tokens, above which the oldest turns are dropped from a request.
    /// Never below the compaction threshold, so compaction gets to summarize first.
    pub fn effective_history_limit(&self) -> usize {
        self.history_token_limit
            .filter(|&t| t > 0)
            .unwrap_or_else(|| self.context_window() / 10 * 9)
            .max(self.effective_compact_threshold())
    }

    /// All providers: built-ins merged with user-defined ones.
    pub fn all_providers(&self) -> Vec<crate::providers::ProviderConfig> {
        crate::providers::merge_providers(&self.providers)
    }

    /// The remote (non-DeepSeek, non-local) provider currently selected, if any.
    pub fn external_provider(&self) -> Option<crate::providers::ProviderConfig> {
        let all = self.all_providers();
        if let Some(ref name) = self.active_provider {
            return all
                .into_iter()
                .find(|p| {
                    p.name == *name
                        && !p.is_deepseek()
                        && !p.is_local()
                        && validate_base_url(&p.base_url).is_ok()
                });
        }
        if let Some(p) = all.iter().find(|p| {
            !p.is_deepseek()
                && !p.is_local()
                && validate_base_url(&p.base_url).is_ok()
                && p.models.iter().any(|m| m.id == self.model)
        }) {
            return Some(p.clone());
        }
        // Match by model prefix so snapshots absent from the built-in catalog still route to their vendor.
        let model_lower = self.model.to_ascii_lowercase();
        let family_provider = if model_lower.starts_with("claude-") {
            Some(crate::providers::ANTHROPIC_ID)
        } else if model_lower.starts_with("gemini-") {
            Some(crate::providers::GOOGLE_ID)
        } else {
            None
        };
        if let Some(name) = family_provider {
            if let Some(p) = all
                .iter()
                .find(|p| p.name == name && validate_base_url(&p.base_url).is_ok())
            {
                return Some(p.clone());
            }
        }
        let host = url_host(&self.base_url).unwrap_or_default();
        if !is_deepseek_base_url(&self.base_url) && !is_loopback_host(&host) {
            if let Some(p) = all.iter().find(|p| {
                !p.is_deepseek()
                    && !p.is_local()
                    && !p.base_url.is_empty()
                    && validate_base_url(&p.base_url).is_ok()
                    && self.base_url.starts_with(&p.base_url)
            }) {
                return Some(p.clone());
            }
        }
        None
    }

    /// `(base_url, api_key)` that requests must use. Switching provider never overwrites the
    /// stored DeepSeek `base_url` / `api_key`; the other provider's key comes from its env var or persisted provider_api_keys.
    pub fn endpoint(&self) -> (String, String) {
        match self.external_provider() {
            Some(p) => {
                let key = self
                    .provider_api_keys
                    .get(&p.name)
                    .cloned()
                    .filter(|k| !k.trim().is_empty())
                    .unwrap_or_else(|| p.api_key());
                let effective_key = if !key.trim().is_empty() {
                    key
                } else if !self.api_key.trim().is_empty()
                    && !is_deepseek_base_url(&self.base_url)
                    && self.base_url.starts_with(&p.base_url)
                {
                    self.api_key.clone()
                } else {
                    String::new()
                };
                (p.base_url.clone(), effective_key)
            }
            None => (self.base_url.clone(), self.api_key.clone()),
        }
    }

    /// False for the models known to read only text, which reject image parts: DeepSeek Pro.
    /// DeepSeek Flash sees images, like the other vendors' models; one that turns out not to is
    /// handled when the request is rejected (see `LlmClient::stream_chat`).
    pub fn can_see_images(&self) -> bool {
        !(self.is_deepseek_endpoint() && is_pro_model(&self.model))
    }

    /// True when requests go to DeepSeek (enables DeepSeek-only request fields and model aliases).
    pub fn is_deepseek_endpoint(&self) -> bool {
        if self.external_provider().is_some() {
            return false;
        }
        if self.active_provider.as_deref() == Some(crate::providers::DEEPSEEK_ID) {
            return true;
        }
        if self.active_provider.is_none() {
            return is_deepseek_base_url(&self.base_url);
        }
        false
    }

    /// Resolves the provider classification to adapt protocols (OpenAI, Anthropic, Google, DeepSeek, Local).
    pub fn provider_kind(&self) -> crate::providers::ProviderKind {
        if self.local_llm_enabled || self.model.starts_with("local") {
            return crate::providers::ProviderKind::Local;
        }
        if let Some(ref name) = self.active_provider {
            match name.as_str() {
                crate::providers::DEEPSEEK_ID => return crate::providers::ProviderKind::DeepSeek,
                crate::providers::OPENAI_ID => return crate::providers::ProviderKind::OpenAi,
                crate::providers::ANTHROPIC_ID => return crate::providers::ProviderKind::Anthropic,
                crate::providers::GOOGLE_ID => return crate::providers::ProviderKind::Google,
                crate::providers::GITHUB_ID => return crate::providers::ProviderKind::GitHub,
                crate::providers::GROQ_ID => return crate::providers::ProviderKind::Groq,
                crate::providers::OPENROUTER_ID => return crate::providers::ProviderKind::OpenRouter,
                crate::providers::MISTRAL_ID => return crate::providers::ProviderKind::Mistral,
                crate::providers::LOCAL_ID => return crate::providers::ProviderKind::Local,
                _ => {}
            }
        }
        let (url, _) = self.endpoint();
        let url_lower = url.to_lowercase();
        if url_lower.contains("anthropic.com") {
            crate::providers::ProviderKind::Anthropic
        } else if url_lower.contains("openai.com") {
            crate::providers::ProviderKind::OpenAi
        } else if url_lower.contains("generativelanguage.googleapis.com") {
            crate::providers::ProviderKind::Google
        } else if url_lower.contains("models.inference.ai.azure.com") {
            crate::providers::ProviderKind::GitHub
        } else if url_lower.contains("groq.com") {
            crate::providers::ProviderKind::Groq
        } else if url_lower.contains("openrouter.ai") {
            crate::providers::ProviderKind::OpenRouter
        } else if url_lower.contains("mistral.ai") {
            crate::providers::ProviderKind::Mistral
        } else if url_lower.contains("deepseek.com") {
            crate::providers::ProviderKind::DeepSeek
        } else {
            crate::providers::ProviderKind::GenericOpenAi
        }
    }
}

impl Config {
    fn resolve_settings_path(workspace: Option<&Path>, filename: &str) -> Option<PathBuf> {
        if let Some(ws) = workspace {
            let corex_local = ws.join(".corex").join(filename);
            if corex_local.exists() {
                return Some(corex_local);
            }
        }
        if let Some(dirs) = BaseDirs::new() {
            let corex_global = dirs.home_dir().join(".corex").join(filename);
            if corex_global.exists() {
                return Some(corex_global);
            }
            let legacy = dirs.home_dir().join(".deepseek").join(filename);
            if legacy.exists() {
                return Some(legacy);
            }
        }
        None
    }

    fn save_to_destinations(content: &str, filename: &str, workspace: Option<&Path>) -> anyhow::Result<()> {
        let is_temp = workspace.map(|w| w.starts_with(std::env::temp_dir())).unwrap_or(false);
        if let Some(ws) = workspace {
            let dir = ws.join(".corex");
            if dir.exists() {
                // Workspace copies must never carry credentials: they live inside a repository
                // that may be committed or shared.
                match Self::strip_secrets(content) {
                    Some(sanitized) => {
                        if let Err(e) = crate::secure_fs::write_private_atomic(&dir.join(filename), sanitized) {
                            tracing::warn!("Failed to save workspace settings {}: {}", dir.join(filename).display(), e);
                            if is_temp {
                                return Err(e.into());
                            }
                        }
                    }
                    None => tracing::warn!(
                        "Not writing workspace settings {}: content could not be sanitized",
                        dir.join(filename).display()
                    ),
                }
            }
        }
        if !is_temp {
            if let Some(dirs) = BaseDirs::new() {
                let dir = dirs.home_dir().join(".corex");
                crate::secure_fs::ensure_private_dir(&dir)?;
                crate::secure_fs::write_private_atomic(&dir.join(filename), content)?;
            }
        }
        Ok(())
    }

    /// Removes credentials and secret-bearing maps from a serialized workspace settings document.
    /// Fails closed: content that cannot be parsed is never returned as-is.
    fn strip_secrets(content: &str) -> Option<String> {
        let mut v = serde_json::from_str::<serde_json::Value>(content).ok()?;
        if let Some(obj) = v.as_object_mut() {
            obj.remove("api_key");
            obj.remove("provider_api_keys");
            obj.remove("mcp_servers");
        }
        serde_json::to_string_pretty(&v).ok()
    }

    pub fn load_flash_settings() -> FlashSettings {
        Self::load_flash_settings_with_workspace(None)
    }

    pub fn load_flash_settings_with_workspace(workspace: Option<&Path>) -> FlashSettings {
        if let Some(path) = Self::resolve_settings_path(workspace, "flash_settings.json") {
            if let Ok(c) = fs::read_to_string(&path) {
                if let Ok(parsed) = serde_json::from_str::<FlashSettings>(&c) {
                    return parsed;
                }
            }
        }
        FlashSettings::default()
    }

    pub fn save_flash_settings(settings: &FlashSettings) -> anyhow::Result<()> {
        Self::save_flash_settings_with_workspace(settings, None)
    }

    pub fn save_flash_settings_with_workspace(settings: &FlashSettings, workspace: Option<&Path>) -> anyhow::Result<()> {
        let content = serde_json::to_string_pretty(settings)?;
        Self::save_to_destinations(&content, "flash_settings.json", workspace)
    }

    pub fn load_pro_settings() -> ProSettings {
        Self::load_pro_settings_with_workspace(None)
    }

    pub fn load_pro_settings_with_workspace(workspace: Option<&Path>) -> ProSettings {
        if let Some(path) = Self::resolve_settings_path(workspace, "pro_settings.json") {
            if let Ok(c) = fs::read_to_string(&path) {
                if let Ok(parsed) = serde_json::from_str::<ProSettings>(&c) {
                    return parsed;
                }
            }
        }
        ProSettings::default()
    }

    pub fn save_pro_settings(settings: &ProSettings) -> anyhow::Result<()> {
        Self::save_pro_settings_with_workspace(settings, None)
    }

    pub fn save_pro_settings_with_workspace(settings: &ProSettings, workspace: Option<&Path>) -> anyhow::Result<()> {
        let content = serde_json::to_string_pretty(settings)?;
        Self::save_to_destinations(&content, "pro_settings.json", workspace)
    }


    pub fn load() -> Self {
        Self::load_with_workspace(None)
    }

    pub fn load_with_workspace(workspace: Option<&Path>) -> Self {
        let _ = crate::engine_signature();
        let mut config = Config::default();

        if let Some(path) = Self::resolve_settings_path(None, "settings.json") {
            if let Ok(content) = fs::read_to_string(&path) {
                match serde_json::from_str::<Config>(&content) {
                    Ok(parsed) => config = parsed,
                    Err(e) => {
                        let backup = path.with_extension("json.invalid");
                        let _ = crate::secure_fs::write_private(&backup, &content);
                        tracing::warn!("Ignoring invalid {}: {}", path.display(), e);
                        eprintln!(
                            "[corex] warning: {} is not valid ({}); using defaults. A copy was saved to {}",
                            path.display(),
                            e,
                            backup.display()
                        );
                    }
                }
            }
        }

        // Workspace overrides (`.corex/settings.json`) are restricted to non-privileged fields.
        // Credentials, endpoints, `yolo_mode`, `local_llm_enabled`, `allowed_commands`, and
        // `mcp_servers` are ignored from workspace files.
        if let Some(ws) = workspace {
            let local_path = ws.join(".corex").join("settings.json");
            if local_path.exists() {
                match fs::read_to_string(&local_path)
                    .map_err(|e| e.to_string())
                    .and_then(|c| serde_json::from_str::<LocalOverrides>(&c).map_err(|e| e.to_string()))
                {
                    Ok(o) => {
                        if let Some(m) = o.model.filter(|m| {
                            let t = m.trim();
                            !t.is_empty() && !t.to_ascii_lowercase().starts_with("local")
                        }) {
                            config.model = m;
                        }
                        if let Some(t) = o.temperature {
                            config.temperature = t;
                        }
                        if let Some(r) = o.reasoning_effort.filter(|r| !r.trim().is_empty()) {
                            config.reasoning_effort = r;
                        }
                        if let Some(a) = o.auto_compact {
                            config.auto_compact = a;
                        }
                        if let Some(t) = o.compact_threshold_tokens {
                            config.compact_threshold_tokens = Some(t);
                        }
                        if let Some(t) = o.history_token_limit {
                            config.history_token_limit = Some(t);
                        }
                        if let Some(t) = o.context_window_tokens {
                            config.context_window_tokens = Some(t);
                        }
                        if let Some(l) = o.local_prompt_lite {
                            config.local_prompt_lite = l;
                        }
                        if o.yolo_mode == Some(true) || o.local_llm_enabled.is_some() {
                            tracing::warn!(
                                "Ignoring security-sensitive keys (yolo_mode/local_llm_enabled) in {}",
                                local_path.display()
                            );
                        }
                    }
                    Err(e) => tracing::warn!("Ignoring invalid {}: {}", local_path.display(), e),
                }
            }
        }

        // Load disk-persisted flash and pro settings
        config.flash_settings = Self::load_flash_settings_with_workspace(workspace);
        config.pro_settings = Self::load_pro_settings_with_workspace(workspace);

        // Apply active model's reasoning/temp defaults from settings (DeepSeek models only:
        // other providers keep the temperature / reasoning saved in settings.json).
        if config.is_deepseek_endpoint() && !config.local_llm_enabled {
            if is_pro_model(&config.model) {
                config.reasoning_effort = config.pro_settings.reasoning_effort.clone();
            } else {
                config.temperature = config.flash_settings.temperature;
                config.reasoning_effort = config.flash_settings.reasoning_effort.clone();
            }
        }

        if let Ok(k) = std::env::var("COREX_API_KEY")
            .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
        {
            if !k.is_empty() {
                config.api_key = k;
            }
        }
        if let Ok(u) = std::env::var("COREX_BASE_URL")
            .or_else(|_| std::env::var("DEEPSEEK_BASE_URL"))
        {
            if !u.is_empty() {
                config.base_url = u;
            }
        }
        if let Err(e) = validate_base_url(&config.base_url) {
            tracing::warn!("{}; falling back to {}", e, default_base_url_static());
            eprintln!("[corex] warning: {}; falling back to {}", e, default_base_url_static());
            config.base_url = default_base_url_static().to_string();
        }
        // Provider-specific keys are only sent to their designated endpoints:
        if config.api_key.trim().is_empty() && is_openai_base_url(&config.base_url) {
            if let Ok(k) = std::env::var("OPENAI_API_KEY") {
                config.api_key = k;
            }
        }
        if config.api_key.trim().is_empty() && is_anthropic_base_url(&config.base_url) {
            if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
                config.api_key = k;
            }
        }
        if config.api_key.trim().is_empty() && is_google_base_url(&config.base_url) {
            if let Ok(k) = std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY")) {
                config.api_key = k;
            }
        }
        // Auto-migrate retired/deprecated models from Google AI Studio (2.0-flash / 2.5-flash / 2.5-pro -> 3.5-flash-lite)
        if config.active_provider.as_deref() == Some("google")
            && (config.model == "gemini-2.0-flash" || config.model == "gemini-2.5-flash" || config.model == "gemini-1.5-flash" || config.model == "gemini-2.5-pro")
        {
            config.model = "gemini-3.5-flash-lite".to_string();
        }

        if let Ok(m) = std::env::var("COREX_MODEL")
            .or_else(|_| std::env::var("DEEPSEEK_MODEL"))
        {
            if !m.is_empty() {
                config.model = m;
            }
        }
        if let Ok(l_url) = std::env::var("COREX_LOCAL_LLM_URL") {
            if !l_url.is_empty() {
                config.local_llm_url = l_url;
            }
        }
        if let Ok(l_model) = std::env::var("COREX_LOCAL_LLM_MODEL") {
            if !l_model.is_empty() {
                config.local_llm_model = l_model;
            }
        }
        if let Ok(l_en) = std::env::var("COREX_LOCAL_LLM_ENABLED") {
            config.local_llm_enabled = l_en != "0" && l_en.to_lowercase() != "false";
        }

        config
    }

    /// The key literally stored in the global `settings.json` (never the environment).
    fn load_existing_api_key(workspace: Option<&Path>) -> Option<String> {
        // Credentials are only ever read from the user's global config, never from a workspace.
        let _ = workspace;
        let path = Self::resolve_settings_path(None, "settings.json")?;
        let content = fs::read_to_string(path).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
        parsed
            .get("api_key")
            .and_then(|k| k.as_str())
            .filter(|k| !k.trim().is_empty())
            .map(str::to_string)
    }

    /// True when `key` is the value of a provider API-key environment variable. Such keys are
    /// injected at load time (env > file) and must never be written into `settings.json`.
    fn is_env_supplied_key(key: &str) -> bool {
        const KEY_VARS: &[&str] = &[
            "COREX_API_KEY",
            "DEEPSEEK_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
        ];
        !key.trim().is_empty()
            && KEY_VARS
                .iter()
                .any(|v| std::env::var(v).is_ok_and(|val| val == key))
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_with_workspace(None)
    }

    pub fn save_with_workspace(&self, workspace: Option<&Path>) -> anyhow::Result<()> {
        let mut to_save = self.clone();
        if to_save.api_key.trim().is_empty() || Self::is_env_supplied_key(&to_save.api_key) {
            // Keep whatever key the file already had; an environment key stays in the environment.
            to_save.api_key = Self::load_existing_api_key(workspace).unwrap_or_default();
        }

        let content = serde_json::to_string_pretty(&to_save)?;
        Self::save_to_destinations(&content, "settings.json", workspace)
    }

    pub fn add_mcp_server(
        &mut self,
        name: String,
        server: McpServerConfig,
        workspace: Option<&Path>,
    ) -> anyhow::Result<()> {
        self.mcp_servers.insert(name, server);
        self.save_with_workspace(workspace)
    }

    pub fn remove_mcp_server(
        &mut self,
        name: &str,
        workspace: Option<&Path>,
    ) -> anyhow::Result<bool> {
        let existed = self.mcp_servers.remove(name).is_some();
        if existed {
            self.save_with_workspace(workspace)?;
        }
        Ok(existed)
    }
}

#[cfg(test)]
mod tests {
    fn config_for(provider: &str) -> Config {
        Config {
            model: "some-model".to_string(),
            active_provider: Some(provider.to_string()),
            local_llm_enabled: false,
            ..Config::default()
        }
    }

    #[test]
    fn context_limits_follow_the_provider_window() {
        let deepseek = config_for(crate::providers::DEEPSEEK_ID);
        assert_eq!(deepseek.context_window(), 1_000_000);
        assert_eq!(deepseek.effective_compact_threshold(), 800_000);
        assert_eq!(deepseek.effective_history_limit(), 900_000);

        let anthropic = config_for(crate::providers::ANTHROPIC_ID);
        assert_eq!(anthropic.effective_compact_threshold(), 160_000);
        assert_eq!(anthropic.effective_history_limit(), 180_000);
    }

    #[test]
    fn the_window_comes_from_the_most_specific_source() {
        use crate::providers::{ModelProfile, ProviderConfig};
        let mut cfg = Config {
            model: "acme-1".into(),
            active_provider: Some("acme".into()),
            providers: vec![ProviderConfig {
                name: "acme".into(),
                label: String::new(),
                base_url: "https://acme.test/v1".into(),
                api_key_env: String::new(),
                models: vec![ModelProfile {
                    id: "acme-1".into(),
                    label: String::new(),
                    tags: Vec::new(),
                    reasoning: Vec::new(),
                    family: Default::default(),
                    context_window: Some(64_000),
                }],
            }],
            local_llm_enabled: false,
            ..Config::default()
        };
        assert_eq!(cfg.context_window_with_source(), (64_000, WindowSource::Catalog));

        cfg.context_window_tokens = Some(32_000);
        assert_eq!(cfg.context_window_with_source(), (32_000, WindowSource::Settings));

        cfg.context_window_tokens = None;
        cfg.model = "acme-2".into();
        assert_eq!(cfg.context_window_with_source().1, WindowSource::Default);
    }

    #[test]
    fn explicit_context_settings_win() {
        let mut cfg = config_for(crate::providers::DEEPSEEK_ID);
        cfg.context_window_tokens = Some(128_000);
        assert_eq!(cfg.effective_compact_threshold(), 102_400);

        cfg.compact_threshold_tokens = Some(50_000);
        cfg.history_token_limit = Some(70_000);
        assert_eq!(cfg.effective_compact_threshold(), 50_000);
        assert_eq!(cfg.effective_history_limit(), 70_000);

        // The history cap never sits below the compaction threshold.
        cfg.history_token_limit = Some(10_000);
        assert_eq!(cfg.effective_history_limit(), 50_000);
    }

    #[test]
    fn the_old_fixed_threshold_counts_as_unset() {
        let cfg: Config = serde_json::from_str(r#"{"compact_threshold_tokens": 95000}"#).unwrap();
        let mut cfg = Config { active_provider: Some(crate::providers::GOOGLE_ID.into()), model: "m".into(), ..cfg };
        assert_eq!(cfg.effective_compact_threshold(), 800_000);
        cfg.compact_threshold_tokens = Some(95_001);
        assert_eq!(cfg.effective_compact_threshold(), 95_001);
    }

    #[test]
    fn unset_context_keys_are_not_written_back() {
        let json = serde_json::to_string(&Config::default()).unwrap();
        assert!(!json.contains("compact_threshold_tokens"));
        assert!(!json.contains("history_token_limit"));
        assert!(!json.contains("context_window_tokens"));
    }

    use super::*;

    #[test]
    fn only_known_text_models_are_told_apart_from_the_ones_that_see_images() {
        // DeepSeek Flash is multimodal; Pro is the one that reads text only.
        let mut cfg = Config::default();
        assert!(cfg.is_deepseek_endpoint() && cfg.can_see_images());
        cfg.model = "deepseek-v4-pro".to_string();
        assert!(!cfg.can_see_images());

        let mut claude = Config::default();
        claude.active_provider = Some(crate::providers::ANTHROPIC_ID.to_string());
        claude.model = "claude-opus-5-5".to_string();
        claude.provider_api_keys.insert(crate::providers::ANTHROPIC_ID.to_string(), "k".to_string());
        assert!(claude.can_see_images());
    }

    #[test]
    fn strip_secrets_removes_credentials_and_fails_closed() {
        let cleaned = Config::strip_secrets(
            r#"{"api_key":"sk-1","provider_api_keys":{"openai":"sk-2"},"mcp_servers":{},"model":"m"}"#,
        )
        .expect("valid JSON is sanitized");
        assert!(!cleaned.contains("sk-1") && !cleaned.contains("sk-2"));
        assert!(cleaned.contains("\"model\""));
        assert_eq!(Config::strip_secrets(r#"{"api_key":"sk-1", broken"#), None);
    }


    #[test]
    fn test_workspace_config_override() {
        let unique = format!("corex_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let temp_dir = std::env::temp_dir().join(unique);
        let corex_dir = temp_dir.join(".corex");
        fs::create_dir_all(&corex_dir).expect("create dir");

        let config = Config {
            model: "deepseek-v4-pro".to_string(),
            local_llm_enabled: false,
            ..Default::default()
        };
        config.save_with_workspace(Some(&temp_dir)).expect("save");

        let loaded = Config::load_with_workspace(Some(&temp_dir));
        assert_eq!(loaded.model, "deepseek-v4-pro");
        assert!(!loaded.local_llm_enabled);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}

