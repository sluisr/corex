use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};

fn default_flash_temperature() -> f32 { 1.0 }
fn default_flash_reasoning() -> String { "dynamic".to_string() }
fn default_command_reasoning() -> String { "low".to_string() }
fn default_code_reasoning() -> String { "high".to_string() }
fn default_search_reasoning() -> String { "low".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashSettings {
    #[serde(default = "default_flash_temperature")]
    pub temperature: f32,
    #[serde(rename = "reasoningEffort", default = "default_flash_reasoning")]
    pub reasoning_effort: String,
    #[serde(rename = "commandReasoningEffort", default = "default_command_reasoning")]
    pub command_reasoning_effort: String,
    #[serde(rename = "codeReasoningEffort", default = "default_code_reasoning")]
    pub code_reasoning_effort: String,
    #[serde(rename = "searchReasoningEffort", default = "default_search_reasoning")]
    pub search_reasoning_effort: String,
}

impl Default for FlashSettings {
    fn default() -> Self {
        Self {
            temperature: default_flash_temperature(),
            reasoning_effort: default_flash_reasoning(),
            command_reasoning_effort: default_command_reasoning(),
            code_reasoning_effort: default_code_reasoning(),
            search_reasoning_effort: default_search_reasoning(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProSettings {
    #[serde(rename = "reasoningEffort", default = "default_pro_reasoning")]
    pub reasoning_effort: String,
    #[serde(rename = "searchReasoningEffort", default = "default_search_reasoning")]
    pub search_reasoning_effort: String,
}

fn default_pro_reasoning() -> String { "high".to_string() }

impl Default for ProSettings {
    fn default() -> Self {
        Self {
            reasoning_effort: default_pro_reasoning(),
            search_reasoning_effort: default_search_reasoning(),
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
    local_prompt_lite: Option<bool>,
    yolo_mode: Option<bool>,
    local_llm_enabled: Option<bool>,
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
    #[serde(default = "default_compact_threshold_tokens")]
    pub compact_threshold_tokens: usize,
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

fn default_compact_threshold_tokens() -> usize {
    95_000
}

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
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") || host.starts_with("127.")
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
        || m.split(|c: char| c == '-' || c == '_' || c == ':' || c == '/' || c == '.')
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
            compact_threshold_tokens: default_compact_threshold_tokens(),
            local_prompt_lite: false,
            allowed_commands: Vec::new(),
            providers: Vec::new(),
            active_provider: None,
            provider_api_keys: HashMap::new(),
        }
    }
}

impl Config {
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
        // Fallback 1: Match by model ID
        if let Some(p) = all.iter().find(|p| !p.is_deepseek() && !p.is_local() && p.models.iter().any(|m| m.id == self.model)) {
            return Some(p.clone());
        }
        // Fallback 2: Match by base_url
        let host = url_host(&self.base_url).unwrap_or_default();
        if !is_deepseek_base_url(&self.base_url) && !is_loopback_host(&host) {
            if let Some(p) = all.iter().find(|p| !p.is_deepseek() && !p.is_local() && !p.base_url.is_empty() && self.base_url.starts_with(&p.base_url)) {
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
                let key = self.provider_api_keys.get(&p.name).cloned()
                    .filter(|k| !k.trim().is_empty())
                    .unwrap_or_else(|| p.api_key());
                let effective_key = if !key.trim().is_empty() {
                    key
                } else if !self.api_key.trim().is_empty() {
                    self.api_key.clone()
                } else {
                    String::new()
                };
                (p.base_url.clone(), effective_key)
            }
            None => (self.base_url.clone(), self.api_key.clone()),
        }
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
                let sanitized = Self::strip_secrets(content);
                if let Err(e) = crate::secure_fs::write_private_atomic(&dir.join(filename), sanitized) {
                    tracing::warn!("Failed to save workspace settings {}: {}", dir.join(filename).display(), e);
                    if is_temp {
                        return Err(e.into());
                    }
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

    /// Removes `api_key` from a serialized settings document.
    fn strip_secrets(content: &str) -> String {
        match serde_json::from_str::<serde_json::Value>(content) {
            Ok(mut v) => {
                if let Some(obj) = v.as_object_mut() {
                    obj.remove("api_key");
                }
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| content.to_string())
            }
            Err(_) => content.to_string(),
        }
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

        // 1. Global config (~/.corex/settings.json, fallback ~/.deepseek/settings.json)
        if let Some(path) = Self::resolve_settings_path(None, "settings.json") {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(parsed) = serde_json::from_str::<Config>(&content) {
                    config = parsed;
                }
            }
        }

        // 2. Local project config (.corex/settings.json) override (safe merge).
        //
        // SECURITY: the workspace is untrusted input (a cloned repository). Its config may only
        // tune cosmetic/performance knobs. It can NEVER relax security or touch credentials and
        // endpoints: `yolo_mode`, `local_llm_enabled`, `api_key`, `base_url`, `allowed_commands`
        // and `mcp_servers` are ignored here. Fields are `Option` so that a key absent from the
        // file does not silently reset the user's global value to a serde default.
        if let Some(ws) = workspace {
            let local_path = ws.join(".corex").join("settings.json");
            if local_path.exists() {
                match fs::read_to_string(&local_path)
                    .map_err(|e| e.to_string())
                    .and_then(|c| serde_json::from_str::<LocalOverrides>(&c).map_err(|e| e.to_string()))
                {
                    Ok(o) => {
                        if let Some(m) = o.model.filter(|m| !m.trim().is_empty()) {
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
                            config.compact_threshold_tokens = t;
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

        // Export persisted provider API keys to environment if not already set
        for (prov, key) in &config.provider_api_keys {
            if !key.trim().is_empty() {
                if let Some(p) = config.all_providers().iter().find(|p| p.name == *prov) {
                    if !p.api_key_env.is_empty() && std::env::var(&p.api_key_env).unwrap_or_default().is_empty() {
                        std::env::set_var(&p.api_key_env, key);
                    }
                }
            }
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

    fn load_existing_api_key(workspace: Option<&Path>) -> Option<String> {
        // Credentials are only ever read from the user's global config, never from a workspace.
        let _ = workspace;
        if let Some(path) = Self::resolve_settings_path(None, "settings.json") {
            if let Ok(c) = fs::read_to_string(&path) {
                if let Ok(parsed) = serde_json::from_str::<Config>(&c) {
                    if !parsed.api_key.trim().is_empty() {
                        return Some(parsed.api_key);
                    }
                }
            }
        }
        if let Ok(k) = std::env::var("COREX_API_KEY")
            .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
        {
            if !k.trim().is_empty() {
                return Some(k);
            }
        }
        None
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_with_workspace(None)
    }

    pub fn save_with_workspace(&self, workspace: Option<&Path>) -> anyhow::Result<()> {
        let mut to_save = self.clone();
        if to_save.api_key.trim().is_empty() {
            if let Some(existing) = Self::load_existing_api_key(workspace) {
                to_save.api_key = existing;
            }
        }

        let content = serde_json::to_string_pretty(&to_save)?;
        Self::save_to_destinations(&content, "settings.json", workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;


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

