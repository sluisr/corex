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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum HybridMode {
    #[default]
    AutoTriage,
    LocalScout,
    DraftAndReview,
    CompressionOnly,
}

impl HybridMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            HybridMode::AutoTriage => "auto_triage",
            HybridMode::LocalScout => "local_scout",
            HybridMode::DraftAndReview => "draft_and_review",
            HybridMode::CompressionOnly => "compression_only",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            HybridMode::AutoTriage => "Auto-Triage",
            HybridMode::LocalScout => "Local Scout",
            HybridMode::DraftAndReview => "Draft & Review",
            HybridMode::CompressionOnly => "Compression Only",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            HybridMode::AutoTriage => "Chat & quick local queries routed locally; heavy coding to DeepSeek.",
            HybridMode::LocalScout => "Local model explores files & greps at $0.00; DeepSeek writes final patch.",
            HybridMode::DraftAndReview => "Local model drafts solutions/tests; DeepSeek audits & refines.",
            HybridMode::CompressionOnly => "All queries to Cloud; Local model compresses bulky tool outputs.",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Self> {
        let clean = s.trim().to_lowercase().replace('-', "_");
        match clean.as_str() {
            "auto_triage" | "triage" | "auto" | "1" => Some(HybridMode::AutoTriage),
            "local_scout" | "scout" | "read_local" | "2" => Some(HybridMode::LocalScout),
            "draft_and_review" | "draft" | "review" | "speculative" | "3" => Some(HybridMode::DraftAndReview),
            "compression_only" | "compression" | "compress" | "4" => Some(HybridMode::CompressionOnly),
            _ => None,
        }
    }
}


fn default_hybrid_mode() -> HybridMode {
    HybridMode::CompressionOnly
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSettings {
    #[serde(default = "default_hybrid_primary_model")]
    pub primary_model: String,
    #[serde(default = "default_local_llm_url")]
    pub local_url: String,
    #[serde(default = "default_local_llm_model")]
    pub secondary_local_model: String,
    #[serde(default = "default_hybrid_compression")]
    pub auto_compression: bool,
    #[serde(default = "default_hybrid_mode")]
    pub mode: HybridMode,
}

fn default_hybrid_primary_model() -> String {
    "deepseek-flash".to_string()
}

impl Default for HybridSettings {
    fn default() -> Self {
        Self {
            primary_model: default_hybrid_primary_model(),
            local_url: default_local_llm_url(),
            secondary_local_model: default_local_llm_model(),
            auto_compression: true,
            mode: HybridMode::CompressionOnly,
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
    pub hybrid_settings: HybridSettings,
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
    #[serde(default = "default_hybrid_compression")]
    pub hybrid_compression: bool,
    #[serde(default = "default_auto_compact")]
    pub auto_compact: bool,
    #[serde(default = "default_compact_threshold_tokens")]
    pub compact_threshold_tokens: usize,
    /// Extra shell commands (binary names or `cmd subcommand` prefixes) that
    /// are treated as safe without requiring user confirmation.
    #[serde(default)]
    pub allowed_commands: Vec<String>,
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
    std::env::var("UTI_LOCAL_LLM_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8080/v1".to_string())
}

fn default_local_llm_model() -> String {
    std::env::var("UTI_LOCAL_LLM_MODEL")
        .unwrap_or_else(|_| "local-model".to_string())
}

fn default_hybrid_compression() -> bool {
    false
}

fn default_api_key() -> String {
    std::env::var("UTI_API_KEY")
        .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .unwrap_or_default()
}

fn default_base_url() -> String {
    std::env::var("UTI_BASE_URL")
        .or_else(|_| std::env::var("DEEPSEEK_BASE_URL"))
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string())
}

fn default_model() -> String {
    std::env::var("UTI_MODEL")
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
            hybrid_settings: HybridSettings::default(),
            mcp_servers: HashMap::new(),
            sudo_password: None,
            yolo_mode: false,
            target_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            local_llm_enabled: default_local_llm_enabled(),
            local_llm_url: default_local_llm_url(),
            local_llm_model: default_local_llm_model(),
            hybrid_compression: default_hybrid_compression(),
            auto_compact: default_auto_compact(),
            compact_threshold_tokens: default_compact_threshold_tokens(),
            allowed_commands: Vec::new(),
        }
    }
}

impl Config {
    pub fn load_flash_settings() -> FlashSettings {
        Self::load_flash_settings_with_workspace(None)
    }

    pub fn load_flash_settings_with_workspace(workspace: Option<&Path>) -> FlashSettings {
        if let Some(ws) = workspace {
            let local_file = ws.join(".uti").join("flash_settings.json");
            if local_file.exists() {
                if let Ok(c) = fs::read_to_string(&local_file) {
                    if let Ok(parsed) = serde_json::from_str::<FlashSettings>(&c) {
                        return parsed;
                    }
                }
            }
        }
        if let Some(dirs) = BaseDirs::new() {
            let uti_file = dirs.home_dir().join(".uti").join("flash_settings.json");
            let legacy_file = dirs.home_dir().join(".deepseek").join("flash_settings.json");
            if uti_file.exists() {
                if let Ok(c) = fs::read_to_string(&uti_file) {
                    if let Ok(parsed) = serde_json::from_str::<FlashSettings>(&c) {
                        return parsed;
                    }
                }
            } else if legacy_file.exists() {
                if let Ok(c) = fs::read_to_string(&legacy_file) {
                    if let Ok(parsed) = serde_json::from_str::<FlashSettings>(&c) {
                        return parsed;
                    }
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
        let is_temp = workspace.map(|w| w.starts_with(std::env::temp_dir())).unwrap_or(false);
        if let Some(ws) = workspace {
            let dir = ws.join(".uti");
            if dir.exists() {
                let _ = fs::write(dir.join("flash_settings.json"), &content);
            }
        }
        if !is_temp {
            if let Some(dirs) = BaseDirs::new() {
                let dir = dirs.home_dir().join(".uti");
                fs::create_dir_all(&dir)?;
                let file = dir.join("flash_settings.json");
                fs::write(file, content)?;
            }
        }
        Ok(())
    }

    pub fn load_pro_settings() -> ProSettings {
        Self::load_pro_settings_with_workspace(None)
    }

    pub fn load_pro_settings_with_workspace(workspace: Option<&Path>) -> ProSettings {
        if let Some(ws) = workspace {
            let local_file = ws.join(".uti").join("pro_settings.json");
            if local_file.exists() {
                if let Ok(c) = fs::read_to_string(&local_file) {
                    if let Ok(parsed) = serde_json::from_str::<ProSettings>(&c) {
                        return parsed;
                    }
                }
            }
        }
        if let Some(dirs) = BaseDirs::new() {
            let uti_file = dirs.home_dir().join(".uti").join("pro_settings.json");
            let legacy_file = dirs.home_dir().join(".deepseek").join("pro_settings.json");
            if uti_file.exists() {
                if let Ok(c) = fs::read_to_string(&uti_file) {
                    if let Ok(parsed) = serde_json::from_str::<ProSettings>(&c) {
                        return parsed;
                    }
                }
            } else if legacy_file.exists() {
                if let Ok(c) = fs::read_to_string(&legacy_file) {
                    if let Ok(parsed) = serde_json::from_str::<ProSettings>(&c) {
                        return parsed;
                    }
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
        let is_temp = workspace.map(|w| w.starts_with(std::env::temp_dir())).unwrap_or(false);
        if let Some(ws) = workspace {
            let dir = ws.join(".uti");
            if dir.exists() {
                let _ = fs::write(dir.join("pro_settings.json"), &content);
            }
        }
        if !is_temp {
            if let Some(dirs) = BaseDirs::new() {
                let dir = dirs.home_dir().join(".uti");
                fs::create_dir_all(&dir)?;
                let file = dir.join("pro_settings.json");
                fs::write(file, content)?;
            }
        }
        Ok(())
    }

    pub fn load_hybrid_settings() -> HybridSettings {
        Self::load_hybrid_settings_with_workspace(None)
    }

    pub fn load_hybrid_settings_with_workspace(workspace: Option<&Path>) -> HybridSettings {
        if let Some(ws) = workspace {
            let local_file = ws.join(".uti").join("hybrid_settings.json");
            if local_file.exists() {
                if let Ok(c) = fs::read_to_string(&local_file) {
                    if let Ok(parsed) = serde_json::from_str::<HybridSettings>(&c) {
                        return parsed;
                    }
                }
            }
        }
        if let Some(dirs) = BaseDirs::new() {
            let uti_file = dirs.home_dir().join(".uti").join("hybrid_settings.json");
            if uti_file.exists() {
                if let Ok(c) = fs::read_to_string(&uti_file) {
                    if let Ok(parsed) = serde_json::from_str::<HybridSettings>(&c) {
                        return parsed;
                    }
                }
            }
        }
        HybridSettings::default()
    }

    pub fn save_hybrid_settings(settings: &HybridSettings) -> anyhow::Result<()> {
        Self::save_hybrid_settings_with_workspace(settings, None)
    }

    pub fn save_hybrid_settings_with_workspace(settings: &HybridSettings, workspace: Option<&Path>) -> anyhow::Result<()> {
        let content = serde_json::to_string_pretty(settings)?;
        let is_temp = workspace.map(|w| w.starts_with(std::env::temp_dir())).unwrap_or(false);
        if let Some(ws) = workspace {
            let dir = ws.join(".uti");
            if dir.exists() {
                let _ = fs::write(dir.join("hybrid_settings.json"), &content);
            }
        }
        if !is_temp {
            if let Some(dirs) = BaseDirs::new() {
                let dir = dirs.home_dir().join(".uti");
                fs::create_dir_all(&dir)?;
                let file = dir.join("hybrid_settings.json");
                fs::write(file, content)?;
            }
        }
        Ok(())
    }

    pub fn load() -> Self {
        Self::load_with_workspace(None)
    }

    pub fn load_with_workspace(workspace: Option<&Path>) -> Self {
        let mut config = Config::default();

        let global_config_path = BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".uti").join("settings.json"));

        if let Some(path) = global_config_path {
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(parsed) = serde_json::from_str::<Config>(&content) {
                        config = parsed;
                    }
                }
            } else {
                if let Some(dirs) = BaseDirs::new() {
                    let legacy_path = dirs.home_dir().join(".deepseek").join("settings.json");
                    if legacy_path.exists() {
                        if let Ok(content) = fs::read_to_string(&legacy_path) {
                            if let Ok(parsed) = serde_json::from_str::<Config>(&content) {
                                config = parsed;
                            }
                        }
                    }
                }
            }
        }

        // Local project config ./.uti/settings.json override (safe merge: protect sensitive credentials & security settings)
        let local_path = workspace
            .map(|ws| ws.join(".uti").join("settings.json"))
            .unwrap_or_else(|| Path::new(".uti").join("settings.json"));

        if local_path.exists() {
            if let Ok(content) = fs::read_to_string(&local_path) {
                if let Ok(parsed) = serde_json::from_str::<Config>(&content) {
                    if !parsed.model.is_empty() {
                        config.model = parsed.model;
                    }
                    config.temperature = parsed.temperature;
                    config.reasoning_effort = parsed.reasoning_effort;
                    config.yolo_mode = parsed.yolo_mode;
                    config.auto_compact = parsed.auto_compact;
                    config.compact_threshold_tokens = parsed.compact_threshold_tokens;
                    config.local_llm_enabled = parsed.local_llm_enabled;

                    // SECURITY: Do not let untrusted repository configs hijack endpoints,
                    // inject malicious MCP commands, or bypass safe command lists.
                    if config.api_key.is_empty() && !parsed.api_key.is_empty() {
                        config.api_key = parsed.api_key;
                    }
                }
            }
        }

        // Load disk-persisted flash, pro and hybrid settings
        config.flash_settings = Self::load_flash_settings_with_workspace(workspace);
        config.pro_settings = Self::load_pro_settings_with_workspace(workspace);
        config.hybrid_settings = Self::load_hybrid_settings_with_workspace(workspace);

        // Apply active model's reasoning/temp defaults from settings
        if config.model.contains("reasoner") || config.model.contains("pro") {
            config.reasoning_effort = config.pro_settings.reasoning_effort.clone();
        } else {
            config.temperature = config.flash_settings.temperature;
            config.reasoning_effort = config.flash_settings.reasoning_effort.clone();
        }

        if let Ok(k) = std::env::var("UTI_API_KEY")
            .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
        {
            if !k.is_empty() {
                config.api_key = k;
            }
        }
        if let Ok(u) = std::env::var("UTI_BASE_URL").or_else(|_| std::env::var("DEEPSEEK_BASE_URL")) {
            if !u.is_empty() {
                config.base_url = u;
            }
        }
        if let Ok(m) = std::env::var("UTI_MODEL").or_else(|_| std::env::var("DEEPSEEK_MODEL")) {
            if !m.is_empty() {
                config.model = m;
            }
        }
        if let Ok(l_url) = std::env::var("UTI_LOCAL_LLM_URL") {
            if !l_url.is_empty() {
                config.local_llm_url = l_url;
            }
        }
        if let Ok(l_model) = std::env::var("UTI_LOCAL_LLM_MODEL") {
            if !l_model.is_empty() {
                config.local_llm_model = l_model;
            }
        }
        if let Ok(l_en) = std::env::var("UTI_LOCAL_LLM_ENABLED") {
            config.local_llm_enabled = l_en != "0" && l_en.to_lowercase() != "false";
        }
        if let Ok(h_comp) = std::env::var("UTI_HYBRID_COMPRESSION") {
            config.hybrid_compression = h_comp != "0" && h_comp.to_lowercase() != "false";
        }
        if let Ok(h_mode) = std::env::var("UTI_HYBRID_MODE") {
            if let Some(m) = HybridMode::from_str_loose(&h_mode) {
                config.hybrid_settings.mode = m;
            }
        }

        config
    }

    fn load_existing_api_key(workspace: Option<&Path>) -> Option<String> {
        if let Some(ws) = workspace {
            let local_path = ws.join(".uti").join("settings.json");
            if local_path.exists() {
                if let Ok(c) = fs::read_to_string(&local_path) {
                    if let Ok(parsed) = serde_json::from_str::<Config>(&c) {
                        if !parsed.api_key.trim().is_empty() {
                            return Some(parsed.api_key);
                        }
                    }
                }
            }
        }
        if let Some(dirs) = BaseDirs::new() {
            let path = dirs.home_dir().join(".uti").join("settings.json");
            if path.exists() {
                if let Ok(c) = fs::read_to_string(&path) {
                    if let Ok(parsed) = serde_json::from_str::<Config>(&c) {
                        if !parsed.api_key.trim().is_empty() {
                            return Some(parsed.api_key);
                        }
                    }
                }
            }
        }
        if let Ok(k) = std::env::var("UTI_API_KEY")
            .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
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
        let is_temp = workspace.map(|w| w.starts_with(std::env::temp_dir())).unwrap_or(false);

        if let Some(ws) = workspace {
            let uti_dir = ws.join(".uti");
            if uti_dir.exists() {
                let file = uti_dir.join("settings.json");
                let _ = fs::write(file, &content);
            }
        }

        if !is_temp {
            if let Some(dirs) = BaseDirs::new() {
                let dir = dirs.home_dir().join(".uti");
                fs::create_dir_all(&dir)?;
                let file = dir.join("settings.json");
                fs::write(file, content)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hybrid_mode_parsing() {
        assert_eq!(HybridMode::from_str_loose("triage"), Some(HybridMode::AutoTriage));
        assert_eq!(HybridMode::from_str_loose("auto_triage"), Some(HybridMode::AutoTriage));
        assert_eq!(HybridMode::from_str_loose("scout"), Some(HybridMode::LocalScout));
        assert_eq!(HybridMode::from_str_loose("local-scout"), Some(HybridMode::LocalScout));
        assert_eq!(HybridMode::from_str_loose("review"), Some(HybridMode::DraftAndReview));
        assert_eq!(HybridMode::from_str_loose("compression"), Some(HybridMode::CompressionOnly));
        assert_eq!(HybridMode::from_str_loose("invalid"), None);
    }

    #[test]
    fn test_hybrid_settings_serialization() {
        let settings = HybridSettings {
            primary_model: "deepseek-flash".to_string(),
            local_url: "http://127.0.0.1:8080/v1".to_string(),
            secondary_local_model: "Llama-3.2-3B-Instruct".to_string(),
            auto_compression: true,
            mode: HybridMode::LocalScout,
        };

        let json = serde_json::to_string(&settings).expect("serialize");
        let deserialized: HybridSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.mode, HybridMode::LocalScout);
        assert_eq!(deserialized.auto_compression, true);
    }

    #[test]
    fn test_workspace_config_override() {
        let unique = format!("uti_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let temp_dir = std::env::temp_dir().join(unique);
        let uti_dir = temp_dir.join(".uti");
        fs::create_dir_all(&uti_dir).expect("create dir");

        let mut config = Config::default();
        config.model = "deepseek-v4-pro".to_string();
        config.local_llm_enabled = false;
        config.save_with_workspace(Some(&temp_dir)).expect("save");

        let loaded = Config::load_with_workspace(Some(&temp_dir));
        assert_eq!(loaded.model, "deepseek-v4-pro");
        assert_eq!(loaded.local_llm_enabled, false);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}

