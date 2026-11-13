use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};

fn default_flash_temperature() -> f32 { 1.0 }
fn default_flash_reasoning() -> String { "dynamic".to_string() }

/// Per-user state directory. `COREX_HOME` lets a session or installation keep its settings
/// and sessions isolated from the default `~/.corex` directory.
pub fn corex_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("COREX_HOME").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".corex"))
        .unwrap_or_else(|| PathBuf::from(".corex"))
}

/// Whether `COREX_HOME` keeps this process's state isolated from the shared `~/.corex` directory.
pub fn isolated_home() -> bool {
    std::env::var_os("COREX_HOME")
        .map(|dir| !dir.is_empty())
        .unwrap_or(false)
}

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


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Local servers are spawned as a subprocess and talked to over stdio.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Remote servers (Streamable HTTP transport) are reached at this URL instead of a command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Headers sent with every request to a remote server. Values may reference
    /// environment variables as `${NAME}` so keys never have to be written to disk.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

impl McpServerConfig {
    /// A local server launched as a subprocess and spoken to over stdio.
    pub fn stdio(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
        }
    }

    /// A remote server spoken to over Streamable HTTP.
    pub fn remote(url: impl Into<String>, headers: HashMap<String, String>) -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: Some(url.into()),
            headers,
        }
    }

    /// How the server is shown in `/mcp status`: its URL, or the command line it runs.
    pub fn display_target(&self) -> String {
        match &self.url {
            Some(url) if !url.is_empty() => url.clone(),
            _ => format!("{} {}", self.command, self.args.join(" ")).trim_end().to_string(),
        }
    }
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
    yolo_mode: Option<serde_json::Value>,
    local_llm_enabled: Option<bool>,
}

/// Order of the keys in `settings.json`: the model, how Corex behaves, you, the commands it may
/// run, the local model, then providers and MCP servers. Other keys follow alphabetically.
const SETTINGS_ORDER: &[&str] = &[
    "active_provider",
    "model",
    "base_url",
    "temperature",
    "reasoning_effort",
    "yolo_mode",
    "auto_compact",
    "compact_threshold_tokens",
    "history_token_limit",
    "context_window_tokens",
    "user_name",
    "user_color",
    "exports_dir",
    "allowed_commands",
    "local_llm_enabled",
    "local_llm_url",
    "local_llm_model",
    "local_prompt_lite",
    "providers",
    "mcp_servers",
];

/// Keys `settings.json` no longer carries; they are still read from files that have them.
const NOT_IN_SETTINGS: &[&str] = &["api_key", "provider_api_keys", "flash_settings", "pro_settings", "target_dir"];

/// Smallest token limit a workspace file may set: lower ones compact on every turn, at the user's cost.
const MIN_WORKSPACE_TOKEN_LIMIT: usize = 8_000;

/// The fixed auto-compact threshold of earlier versions, which counts as unset.
const LEGACY_COMPACT_THRESHOLD: u64 = 95_000;

/// The system keyring entry that holds every key, as one JSON document.
const KEYRING_SERVICE: &str = "corex";
const KEYRING_USER: &str = "credentials";

/// The keys, from the system keyring or `~/.corex/credentials.json`.
#[derive(Debug, Default, Deserialize)]
struct Credentials {
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    provider_api_keys: HashMap<String, String>,
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

/// Execution permission mode for tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YoloMode {
    /// Normal: asks before mutating or dangerous actions.
    #[default]
    Off,
    /// YOLO Normal: auto-approves mutating actions, but asks for sensitive paths (credentials,
    /// shell startup files, git hooks), global memory, and browser uploads.
    Normal,
    /// YOLO All: auto-approves ALL tool executions without asking for any confirmation.
    All,
}

impl YoloMode {
    pub fn is_active(self) -> bool {
        matches!(self, YoloMode::Normal | YoloMode::All)
    }

    pub fn is_all(self) -> bool {
        matches!(self, YoloMode::All)
    }
}

impl Serialize for YoloMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            YoloMode::Off => serializer.serialize_bool(false),
            YoloMode::Normal => serializer.serialize_str("normal"),
            YoloMode::All => serializer.serialize_str("all"),
        }
    }
}

impl<'de> Deserialize<'de> for YoloMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct YoloModeVisitor;

        impl<'de> serde::de::Visitor<'de> for YoloModeVisitor {
            type Value = YoloMode;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a boolean or string (\"normal\", \"all\", \"off\")")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(if v { YoloMode::Normal } else { YoloMode::Off })
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v.trim().to_ascii_lowercase().as_str() {
                    "all" | "yolo-all" | "yolo_all" => Ok(YoloMode::All),
                    "normal" | "true" | "on" | "yolo" | "yolo-normal" => Ok(YoloMode::Normal),
                    "off" | "false" => Ok(YoloMode::Off),
                    _ => Ok(YoloMode::Off),
                }
            }
        }

        deserializer.deserialize_any(YoloModeVisitor)
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
    pub yolo_mode: YoloMode,
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
    /// Name on the user's messages in the conversation. Unset: the login of the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    /// Colour of the user's messages, as `#rrggbb`. Unset: the theme's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_color: Option<String>,
    /// Where `/export` saves chats, one folder per project inside. Unset: `~/.corex/exports`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exports_dir: Option<String>,
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
/// True when both URLs name the same host, so a key meant for one is not handed to the other.
pub fn same_host(a: &str, b: &str) -> bool {
    matches!((url_host(a), url_host(b)), (Some(x), Some(y)) if x == y)
}

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
            yolo_mode: YoloMode::Off,
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
            user_name: None,
            user_color: None,
            exports_dir: None,
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

    /// Routes the session to the built-in provider that owns `model` when the model is chosen by
    /// name (`-m`, `/model`, a resumed session) instead of through the picker. Unknown models keep
    /// the legacy behavior of inferring the endpoint from `base_url`.
    pub fn adopt_provider_for_model(&mut self) {
        if self.model.starts_with("local") {
            return;
        }
        self.local_llm_enabled = false;
        self.active_provider = self
            .all_providers()
            .into_iter()
            .find(|p| !p.is_local() && p.models.iter().any(|m| m.id == self.model))
            .map(|p| p.name);
    }

    /// Restores the model and provider a session was using. The provider saved with the session
    /// wins when it still owns the model; sessions saved before that field existed infer it from
    /// the model, and a stale provider never sends a model to the wrong API.
    pub fn adopt_session_model(&mut self, model: &str, provider: Option<&str>) {
        self.model = model.to_string();
        if model.starts_with("local") {
            self.local_llm_enabled = true;
            self.active_provider = Some(crate::providers::LOCAL_ID.to_string());
            return;
        }
        if let Some(name) = provider.filter(|name| !name.trim().is_empty()) {
            let known = self.all_providers().into_iter().find(|p| p.name == name);
            let matches_model = known
                .as_ref()
                .map(|p| p.is_local() || p.models.is_empty() || p.models.iter().any(|m| m.id == self.model))
                .unwrap_or(false);
            if matches_model {
                self.local_llm_enabled = known.as_ref().map(|p| p.is_local()).unwrap_or(false);
                self.active_provider = Some(name.to_string());
                return;
            }
        }
        self.adopt_provider_for_model();
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
            None => {
                if validate_base_url(&self.base_url).is_err() {
                    (default_base_url_static().to_string(), self.api_key.clone())
                } else {
                    (self.base_url.clone(), self.api_key.clone())
                }
            }
        }
    }

    /// A key that belongs to DeepSeek, for its search and fetch endpoints. The key of any other
    /// provider is never returned: it would be sent to `api.deepseek.com`.
    pub fn deepseek_api_key(&self) -> Option<String> {
        let stored = self.provider_api_keys.get(crate::providers::DEEPSEEK_ID).cloned();
        let default = is_deepseek_base_url(&self.base_url).then(|| self.api_key.clone());
        std::env::var("DEEPSEEK_API_KEY")
            .ok()
            .into_iter()
            .chain(stored)
            .chain(default)
            .find(|key| !key.trim().is_empty())
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
        let corex_global = corex_home().join(filename);
        if isolated_home() && corex_global.exists() {
            return Some(corex_global);
        }
        if let Some(ws) = workspace {
            let corex_local = ws.join(".corex").join(filename);
            if corex_local.exists() {
                return Some(corex_local);
            }
        }
        if corex_global.exists() {
            return Some(corex_global);
        }
        None
    }

    fn save_to_destinations(content: &str, filename: &str, workspace: Option<&Path>) -> anyhow::Result<()> {
        let is_temp = workspace.map(|w| w.starts_with(std::env::temp_dir())).unwrap_or(false);
        if !isolated_home() {
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
        }
        if !is_temp {
            let dir = corex_home();
            crate::secure_fs::ensure_private_dir(&dir)?;
            crate::secure_fs::write_private_atomic(&dir.join(filename), content)?;
        }
        Ok(())
    }

    /// `credentials.json` for `cfg`: the API keys, apart from the settings so that `settings.json`
    /// can be read, shared or synced without them. `None` when there is no key to keep.
    fn credentials_document(cfg: &Config) -> Option<String> {
        let mut doc = serde_json::Map::new();
        if !cfg.api_key.trim().is_empty() {
            doc.insert("api_key".into(), cfg.api_key.clone().into());
        }
        let mut providers: Vec<(&String, &String)> = cfg.provider_api_keys.iter().filter(|(_, k)| !k.trim().is_empty()).collect();
        providers.sort();
        if !providers.is_empty() {
            let map = providers.into_iter().map(|(p, k)| (p.clone(), serde_json::Value::from(k.clone()))).collect();
            doc.insert("provider_api_keys".into(), serde_json::Value::Object(map));
        }
        (!doc.is_empty()).then(|| serde_json::to_string_pretty(&serde_json::Value::Object(doc)).ok()).flatten()
    }

    /// `settings.json` for `cfg`: only what differs from the defaults, grouped by [`SETTINGS_ORDER`],
    /// so the file shows what was changed. Credentials, the flash/pro settings (files of their own)
    /// and `target_dir` (read by nothing) are left out, and so is `compact_threshold_tokens: 95000`,
    /// the old default that earlier versions wrote.
    fn settings_document(cfg: &Config) -> anyhow::Result<String> {
        let value = serde_json::to_value(cfg)?;
        let mut defaults = Config::default();
        defaults.base_url = default_base_url_static().to_string();
        defaults.model = "deepseek-flash".to_string();
        let defaults = serde_json::to_value(defaults)?;
        let Some(fields) = value.as_object() else { anyhow::bail!("settings are not an object") };
        let kept = |key: &str, v: &serde_json::Value| {
            !NOT_IN_SETTINGS.contains(&key)
                && !v.is_null()
                && !(key == "compact_threshold_tokens" && v.as_u64() == Some(LEGACY_COMPACT_THRESHOLD))
                && (key == "model" || defaults.get(key) != Some(v))
        };
        let mut keys: Vec<&str> = SETTINGS_ORDER.iter().copied().filter(|k| fields.get(*k).is_some_and(|v| kept(k, v))).collect();
        let mut rest: Vec<&str> = fields
            .iter()
            .filter(|(k, v)| !SETTINGS_ORDER.contains(&k.as_str()) && kept(k, v))
            .map(|(k, _)| k.as_str())
            .collect();
        rest.sort_unstable();
        keys.extend(rest);

        let mut out = String::from("{\n");
        for (i, key) in keys.iter().enumerate() {
            let pretty = serde_json::to_string_pretty(&fields[*key])?.replace('\n', "\n  ");
            let comma = if i + 1 < keys.len() { "," } else { "" };
            out.push_str(&format!("  {}: {}{}\n", serde_json::to_string(key)?, pretty, comma));
        }
        out.push_str("}\n");
        Ok(out)
    }

    fn credentials_path() -> Option<PathBuf> {
        Some(corex_home().join("credentials.json"))
    }

    /// The system keyring entry holding the keys (Secret Service, Keychain or Credential
    /// Manager). `None` when there is none to reach (SSH, a server, Termux, a container), when
    /// `COREX_KEYRING` is `0`, `false` or `off`, and always in tests, which must never touch the
    /// keyring of whoever runs them.
    fn keyring_entry() -> Option<keyring::Entry> {
        if cfg!(test)
            || std::env::var("COREX_KEYRING").is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off"))
        {
            return None;
        }
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()
    }

    /// The keys from the system keyring, else from `credentials.json`.
    fn load_credentials() -> Option<Credentials> {
        if let Some(stored) = Self::keyring_entry().and_then(|e| e.get_password().ok()) {
            if let Ok(credentials) = serde_json::from_str(&stored) {
                return Some(credentials);
            }
        }
        let content = fs::read_to_string(Self::credentials_path()?).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Stores the keys in the system keyring and removes `credentials.json`; where no keyring
    /// answers, writes that file (mode 600) instead. Never a workspace copy, which lives in a
    /// repository.
    fn save_credentials(content: &str, workspace: Option<&Path>) -> anyhow::Result<()> {
        if workspace.is_some_and(|w| w.starts_with(std::env::temp_dir())) {
            return Ok(());
        }
        let path = Self::credentials_path();
        if Self::keyring_entry().is_some_and(|e| e.set_password(content).is_ok()) {
            if let Some(path) = path.filter(|p| p.exists()) {
                fs::remove_file(path)?;
            }
            return Ok(());
        }
        if let Some(path) = path {
            if let Some(dir) = path.parent() {
                crate::secure_fs::ensure_private_dir(dir)?;
            }
            crate::secure_fs::write_private_atomic(&path, content)?;
        }
        Ok(())
    }

    /// Where the keys are kept, for messages to the user.
    pub fn credentials_location() -> &'static str {
        if Self::keyring_entry().is_some_and(|e| e.get_password().is_ok()) {
            "the system keyring"
        } else {
            "~/.corex/credentials.json"
        }
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

        // Keys live in `credentials.json`; a `settings.json` from before keeps working until the
        // next save moves them.
        if let Some(credentials) = Self::load_credentials() {
            if !credentials.api_key.trim().is_empty() {
                config.api_key = credentials.api_key;
            }
            config.provider_api_keys.extend(credentials.provider_api_keys);
        }

        // Workspace overrides (`.corex/settings.json`) are restricted to non-privileged fields.
        // Credentials, endpoints, `yolo_mode`, `local_llm_enabled`, `allowed_commands`, and
        // `mcp_servers` are ignored from workspace files.
        if let Some(ws) = workspace.filter(|_| !isolated_home()) {
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
                            // A workspace may pick a model, never the provider (and key) that serves it.
                            let candidate = m;
                            let owner = config
                                .all_providers()
                                .into_iter()
                                .find(|p| p.models.iter().any(|model| model.id == candidate))
                                .map(|p| p.name);
                            let belongs = match (config.active_provider.as_deref(), owner) {
                                (Some(active), Some(owner)) => active == owner,
                                _ => true,
                            };
                            if belongs {
                                config.model = candidate;
                            } else {
                                tracing::warn!("Ignoring `model` in {}: it belongs to another provider", local_path.display());
                            }
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
                        let roomy = |tokens: &usize| *tokens >= MIN_WORKSPACE_TOKEN_LIMIT;
                        if let Some(t) = o.compact_threshold_tokens.filter(roomy) {
                            config.compact_threshold_tokens = Some(t);
                        }
                        if let Some(t) = o.history_token_limit.filter(roomy) {
                            config.history_token_limit = Some(t);
                        }
                        if let Some(t) = o.context_window_tokens.filter(roomy) {
                            config.context_window_tokens = Some(t);
                        }
                        if let Some(l) = o.local_prompt_lite {
                            config.local_prompt_lite = l;
                        }
                        if o.yolo_mode.is_some() || o.local_llm_enabled.is_some() {
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

    /// The key literally stored on disk (never the environment): `credentials.json`, else the
    /// `settings.json` of versions that kept it there.
    fn load_existing_api_key(workspace: Option<&Path>) -> Option<String> {
        // Credentials are only ever read from the user's global config, never from a workspace.
        let _ = workspace;
        if let Some(key) = Self::load_credentials().map(|c| c.api_key).filter(|k| !k.trim().is_empty()) {
            return Some(key);
        }
        let path = Self::resolve_settings_path(None, "settings.json")?;
        let content = fs::read_to_string(path).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
        parsed
            .get("api_key")
            .and_then(|k| k.as_str())
            .filter(|k| !k.trim().is_empty())
            .map(str::to_string)
    }

    /// The `mcp_servers` map `settings.json` holds right now. Only the global file carries MCP
    /// servers (a workspace copy is stripped of them), so this is what a reload and a save must
    /// use instead of the copy that was loaded at start-up.
    pub fn mcp_servers_from_disk() -> Option<HashMap<String, McpServerConfig>> {
        let path = Self::resolve_settings_path(None, "settings.json")?;
        let content = fs::read_to_string(path).ok()?;
        Self::mcp_servers_in(&content)
    }

    /// The `mcp_servers` map of a settings document: `Some` empty when the key is absent (the
    /// user removed every server), `None` when the document is not JSON.
    fn mcp_servers_in(content: &str) -> Option<HashMap<String, McpServerConfig>> {
        let value: serde_json::Value = serde_json::from_str(content).ok()?;
        match value.get("mcp_servers") {
            Some(servers) => serde_json::from_value(servers.clone()).ok(),
            None => Some(HashMap::new()),
        }
    }

    /// Replaces the map in memory with the one the file holds, when there is a file: a save must
    /// never bring back a server, or an old key, the user removed or fixed by hand meanwhile.
    fn keep_mcp_servers_from(&mut self, on_disk: Option<HashMap<String, McpServerConfig>>) {
        if let Some(servers) = on_disk {
            self.mcp_servers = servers;
        }
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
        self.save_settings(workspace, false)
    }

    /// `write_mcp_servers` is true only from `/mcp add` and `/mcp remove`, the two edits made to
    /// the map in memory that must reach the file. Every other save keeps the map the file holds,
    /// so a process that started before `settings.json` was edited cannot write the old copy
    /// — an outdated key included — back over it.
    fn save_settings(&self, workspace: Option<&Path>, write_mcp_servers: bool) -> anyhow::Result<()> {
        let mut to_save = self.clone();
        if !write_mcp_servers {
            to_save.keep_mcp_servers_from(Self::mcp_servers_from_disk());
        }
        if to_save.api_key.trim().is_empty() || Self::is_env_supplied_key(&to_save.api_key) {
            // Keep whatever key the file already had; an environment key stays in the environment.
            to_save.api_key = Self::load_existing_api_key(workspace).unwrap_or_default();
        }

        // Keys first: if they cannot be written, settings.json keeps the ones it may still hold.
        match Self::credentials_document(&to_save) {
            Some(credentials) => Self::save_credentials(&credentials, workspace)?,
            // Every key was removed: empty the store instead of leaving the old ones in it.
            None if Self::load_credentials().is_some() => Self::save_credentials("{}\n", workspace)?,
            None => {}
        }
        let content = Self::settings_document(&to_save)?;
        Self::save_to_destinations(&content, "settings.json", workspace)
    }

    pub fn add_mcp_server(
        &mut self,
        name: String,
        server: McpServerConfig,
        workspace: Option<&Path>,
    ) -> anyhow::Result<()> {
        // The file is the truth here too: another Corex, or a hand edit, may have changed a
        // server since this one loaded it, and only the added server must be new.
        let mut servers = Self::mcp_servers_from_disk().unwrap_or_else(|| self.mcp_servers.clone());
        servers.insert(name, server);
        self.mcp_servers = servers;
        self.save_settings(workspace, true)
    }

    pub fn remove_mcp_server(
        &mut self,
        name: &str,
        workspace: Option<&Path>,
    ) -> anyhow::Result<bool> {
        let mut servers = Self::mcp_servers_from_disk().unwrap_or_else(|| self.mcp_servers.clone());
        let existed = servers.remove(name).is_some();
        if existed {
            self.mcp_servers = servers;
            self.save_settings(workspace, true)?;
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
    #[ignore = "uses the real system keyring; run with --ignored on a desktop session"]
    fn system_keyring_round_trip_with_a_throwaway_entry() {
        let entry = keyring::Entry::new("corex-selftest", "round-trip").expect("a keyring answers");
        entry.set_password(r#"{"api_key":"sk-selftest"}"#).expect("stored");
        let back = entry.get_password().expect("read back");
        entry.delete_credential().expect("deleted");
        assert_eq!(back, r#"{"api_key":"sk-selftest"}"#);
        assert!(entry.get_password().is_err(), "gone after delete");
    }

    #[test]
    fn settings_keep_only_what_changed_and_no_keys() {
        let mut cfg = Config {
            api_key: "sk-main".into(),
            model: "deepseek-flash".into(),
            active_provider: Some("deepseek".into()),
            reasoning_effort: "high".into(),
            compact_threshold_tokens: Some(95_000),
            allowed_commands: vec!["cargo test".into()],
            target_dir: PathBuf::from("/home/someone"),
            local_llm_url: "http://10.0.0.2:8080/v1".into(),
            user_name: Some("Luis".into()),
            ..Config::default()
        };
        cfg.provider_api_keys.insert("openai".into(), "sk-openai".into());
        let doc = Config::settings_document(&cfg).unwrap();

        for gone in ["sk-", "api_key", "provider_api_keys", "flash_settings", "pro_settings", "target_dir", "95000", "local_llm_model", "yolo_mode"] {
            assert!(!doc.contains(gone), "{gone} must not be written:\n{doc}");
        }
        let at = |k: &str| doc.find(&format!("\"{k}\"")).unwrap_or_else(|| panic!("{k} missing:\n{doc}"));
        assert!(at("active_provider") < at("model") && at("model") < at("user_name"), "{doc}");
        assert!(at("user_name") < at("allowed_commands") && at("allowed_commands") < at("local_llm_url"), "{doc}");

        let back: Config = serde_json::from_str(&doc).expect("the file reads back");
        assert_eq!(back.model, "deepseek-flash");
        assert_eq!(back.active_provider.as_deref(), Some("deepseek"));
        assert_eq!(back.allowed_commands, ["cargo test"]);
        assert_eq!(back.local_llm_url, "http://10.0.0.2:8080/v1", "a changed value survives");
        assert_eq!(back.user_name.as_deref(), Some("Luis"));
        assert_eq!(back.compact_threshold_tokens, None, "the old default goes back to unset");

        let creds: serde_json::Value = serde_json::from_str(&Config::credentials_document(&cfg).unwrap()).unwrap();
        assert_eq!(creds["api_key"], "sk-main");
        assert_eq!(creds["provider_api_keys"]["openai"], "sk-openai");
        assert!(Config::credentials_document(&Config { api_key: String::new(), ..Config::default() }).is_none()
            || std::env::var("COREX_API_KEY").is_ok()
            || std::env::var("DEEPSEEK_API_KEY").is_ok());
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
    fn mcp_servers_read_as_local_or_remote() {
        let local: McpServerConfig =
            serde_json::from_str(r#"{"command":"npx","args":["-y","@modelcontextprotocol/server-github"]}"#)
                .expect("a local server needs only a command");
        assert_eq!(local.url, None);
        assert_eq!(local.display_target(), "npx -y @modelcontextprotocol/server-github");

        let remote: McpServerConfig = serde_json::from_str(
            r#"{"url":"https://stitch.googleapis.com/mcp","headers":{"X-Goog-Api-Key":"${STITCH_API_KEY}"}}"#,
        )
        .expect("a remote server needs no command");
        assert_eq!(remote.command, "");
        assert_eq!(remote.display_target(), "https://stitch.googleapis.com/mcp");
        assert_eq!(remote.headers["X-Goog-Api-Key"], "${STITCH_API_KEY}");

        // An empty command is not written back, and neither are empty headers.
        let written = serde_json::to_string(&remote).expect("serializes");
        assert!(!written.contains("command"), "{written}");
        assert!(!written.contains("headers") || written.contains("X-Goog-Api-Key"), "{written}");
    }

    #[test]
    fn the_mcp_map_is_read_from_the_file_not_only_at_startup() {
        let doc = r#"{"model":"m","mcp_servers":{"stitch":{"url":"https://stitch.googleapis.com/mcp","headers":{"X-Goog-Api-Key":"fresh"}}}}"#;
        let servers = Config::mcp_servers_in(doc).expect("a settings document has a map");
        assert_eq!(servers["stitch"].headers["X-Goog-Api-Key"], "fresh");

        // No key at all: every server was removed, which is an empty map, not a failure.
        assert_eq!(Config::mcp_servers_in(r#"{"model":"m"}"#), Some(HashMap::new()));
        // Content that is not JSON has no map to offer, so the caller keeps what it has.
        assert_eq!(Config::mcp_servers_in("{not json"), None);
    }

    #[test]
    fn a_save_keeps_the_mcp_map_the_file_holds() {
        // What the process loaded at start-up: the placeholder the catalog wrote.
        let mut memory = Config::default();
        memory.mcp_servers.insert(
            "stitch".to_string(),
            McpServerConfig::remote(
                "https://stitch.googleapis.com/mcp",
                HashMap::from([("X-Goog-Api-Key".to_string(), "${STITCH_API_KEY}".to_string())]),
            ),
        );
        // What the user pasted by hand afterwards.
        let mut on_disk = memory.mcp_servers.clone();
        on_disk.get_mut("stitch").unwrap().headers.insert("X-Goog-Api-Key".to_string(), "a-key-pasted-by-hand".to_string());

        let mut saved = memory.clone();
        saved.keep_mcp_servers_from(Some(on_disk.clone()));
        assert_eq!(saved.mcp_servers["stitch"].headers["X-Goog-Api-Key"], "a-key-pasted-by-hand");

        // With no file to read (first run), the map in memory is all there is.
        let mut first_run = memory.clone();
        first_run.keep_mcp_servers_from(None);
        assert_eq!(first_run.mcp_servers["stitch"].headers["X-Goog-Api-Key"], "${STITCH_API_KEY}");
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

    #[test]
    fn session_model_restores_its_provider() {
        let mut cfg = Config::default();

        cfg.adopt_session_model("gpt-4o", None);
        assert_eq!(cfg.model, "gpt-4o");
        assert_eq!(cfg.active_provider.as_deref(), Some("openai"));
        assert!(!cfg.local_llm_enabled);

        cfg.local_llm_enabled = true;
        cfg.adopt_session_model("deepseek-flash", Some("deepseek"));
        assert_eq!(cfg.active_provider.as_deref(), Some("deepseek"));
        assert!(!cfg.local_llm_enabled);

        // A stale provider must not send a model to the wrong API.
        cfg.adopt_session_model("gpt-4o", Some("deepseek"));
        assert_eq!(cfg.active_provider.as_deref(), Some("openai"));
    }

    #[test]
    fn same_host_compares_hosts_only() {
        assert!(same_host("https://proxy.example.com/v1", "https://proxy.example.com/other"));
        assert!(!same_host("https://proxy.example.com/v1", "https://evil.example.com/v1"));
        assert!(!same_host("not a url", "https://proxy.example.com"));
    }

    #[test]
    fn api_key_is_never_sent_over_remote_plain_http() {
        let cfg = Config {
            base_url: "http://evil.example.com/v1".to_string(),
            api_key: "sk-live-secret".to_string(),
            active_provider: None,
            ..Config::default()
        };
        let (url, key) = cfg.endpoint();
        assert!(
            url.starts_with("https://") || key.is_empty(),
            "API key must never be paired with a remote plaintext http:// base_url"
        );
    }

    #[test]
    fn test_yolo_mode_serialization_and_deserialization() {
        assert_eq!(serde_json::from_str::<YoloMode>("true").unwrap(), YoloMode::Normal);
        assert_eq!(serde_json::from_str::<YoloMode>("false").unwrap(), YoloMode::Off);
        assert_eq!(serde_json::from_str::<YoloMode>(r#""normal""#).unwrap(), YoloMode::Normal);
        assert_eq!(serde_json::from_str::<YoloMode>(r#""all""#).unwrap(), YoloMode::All);
        assert_eq!(serde_json::from_str::<YoloMode>(r#""off""#).unwrap(), YoloMode::Off);

        assert_eq!(serde_json::to_string(&YoloMode::Off).unwrap(), "false");
        assert_eq!(serde_json::to_string(&YoloMode::Normal).unwrap(), r#""normal""#);
        assert_eq!(serde_json::to_string(&YoloMode::All).unwrap(), r#""all""#);

        assert!(YoloMode::Normal.is_active());
        assert!(YoloMode::All.is_active());
        assert!(!YoloMode::Off.is_active());
        assert!(YoloMode::All.is_all());
        assert!(!YoloMode::Normal.is_all());
    }
}

