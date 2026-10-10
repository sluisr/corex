//! Provider / model catalog.
//!
//! The model picker is driven by this data instead of hard-coded per-model tabs. Built-in
//! providers (DeepSeek, OpenAI, OpenRouter, Local) can be extended or overridden by the
//! `providers` list in `settings.json`.

use serde::{Deserialize, Serialize};

pub const DEEPSEEK_ID: &str = "deepseek";
pub const OPENAI_ID: &str = "openai";
pub const ANTHROPIC_ID: &str = "anthropic";
pub const GOOGLE_ID: &str = "google";
pub const GITHUB_ID: &str = "github";
pub const GROQ_ID: &str = "groq";
pub const OPENROUTER_ID: &str = "openrouter";
pub const MISTRAL_ID: &str = "mistral";
pub const LOCAL_ID: &str = "local";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    DeepSeek,
    OpenAi,
    Anthropic,
    Google,
    GitHub,
    Groq,
    OpenRouter,
    Mistral,
    Local,
    GenericOpenAi,
}

/// Which group of tunable settings a model exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModelFamily {
    /// DeepSeek flash: temperature + dynamic CoT (general/command/code/search).
    Flash,
    /// DeepSeek pro: reasoning depth + search CoT.
    Pro,
    /// Anything else: temperature and (optionally) a reasoning level.
    #[default]
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelProfile {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Allowed reasoning levels; empty means the model has no reasoning control.
    #[serde(default)]
    pub reasoning: Vec<String>,
    #[serde(default)]
    pub family: ModelFamily,
}

impl ModelProfile {
    pub fn display_name(&self) -> &str {
        if self.label.is_empty() { &self.id } else { &self.label }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderConfig {
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub base_url: String,
    /// Environment variable holding the API key (keys are never stored in the provider list).
    #[serde(default)]
    pub api_key_env: String,
    #[serde(default)]
    pub models: Vec<ModelProfile>,
}

impl ProviderConfig {
    pub fn display_name(&self) -> &str {
        if self.label.is_empty() { &self.name } else { &self.label }
    }

    pub fn is_deepseek(&self) -> bool {
        self.name == DEEPSEEK_ID
    }

    pub fn is_openai(&self) -> bool {
        self.name == OPENAI_ID
    }

    pub fn is_anthropic(&self) -> bool {
        self.name == ANTHROPIC_ID
    }

    pub fn is_google(&self) -> bool {
        self.name == GOOGLE_ID
    }

    pub fn is_github(&self) -> bool {
        self.name == GITHUB_ID
    }

    pub fn is_groq(&self) -> bool {
        self.name == GROQ_ID
    }

    pub fn is_openrouter(&self) -> bool {
        self.name == OPENROUTER_ID
    }

    pub fn is_mistral(&self) -> bool {
        self.name == MISTRAL_ID
    }

    pub fn is_local(&self) -> bool {
        self.name == LOCAL_ID
    }

    pub fn kind(&self) -> ProviderKind {
        match self.name.as_str() {
            DEEPSEEK_ID => ProviderKind::DeepSeek,
            OPENAI_ID => ProviderKind::OpenAi,
            ANTHROPIC_ID => ProviderKind::Anthropic,
            GOOGLE_ID => ProviderKind::Google,
            GITHUB_ID => ProviderKind::GitHub,
            GROQ_ID => ProviderKind::Groq,
            OPENROUTER_ID => ProviderKind::OpenRouter,
            MISTRAL_ID => ProviderKind::Mistral,
            LOCAL_ID => ProviderKind::Local,
            _ => ProviderKind::GenericOpenAi,
        }
    }

    pub fn portal_url(&self) -> &str {
        match self.name.as_str() {
            OPENAI_ID => "https://platform.openai.com/api-keys",
            ANTHROPIC_ID => "https://console.anthropic.com",
            GOOGLE_ID => "https://aistudio.google.com (Free Tier)",
            GITHUB_ID => "https://github.com/settings/tokens (Personal Access Token)",
            GROQ_ID => "https://console.groq.com/keys (Free Tier)",
            OPENROUTER_ID => "https://openrouter.ai/keys",
            MISTRAL_ID => "https://console.mistral.ai/api-keys",
            DEEPSEEK_ID => "https://platform.deepseek.com",
            _ => "https://platform.deepseek.com",
        }
    }

    /// API key resolved from the environment (empty when unset).
    pub fn api_key(&self) -> String {
        if self.is_deepseek() {
            let key = std::env::var("COREX_API_KEY")
                .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
                .unwrap_or_default();
            if !key.trim().is_empty() {
                return key;
            }
        }
        if self.is_google() {
            let key = std::env::var("GEMINI_API_KEY")
                .or_else(|_| std::env::var("GOOGLE_API_KEY"))
                .or_else(|_| std::env::var("COREX_GEMINI_API_KEY"))
                .unwrap_or_default();
            if !key.trim().is_empty() {
                return key;
            }
        }
        if self.api_key_env.is_empty() {
            return String::new();
        }
        std::env::var(&self.api_key_env)
            .or_else(|_| std::env::var(format!("COREX_{}", self.api_key_env)))
            .unwrap_or_default()
    }

    /// Local providers need no key; remote ones need a non-empty env value.
    pub fn is_ready(&self) -> bool {
        self.is_local() || self.is_deepseek() || !self.api_key().trim().is_empty()
    }
}

fn model(id: &str, label: &str, tags: &[&str], reasoning: &[&str], family: ModelFamily) -> ModelProfile {
    ModelProfile {
        id: id.to_string(),
        label: label.to_string(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        reasoning: reasoning.iter().map(|s| s.to_string()).collect(),
        family,
    }
}

pub fn builtin_providers() -> Vec<ProviderConfig> {
    vec![
        ProviderConfig {
            name: DEEPSEEK_ID.into(),
            label: "DeepSeek".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            models: vec![
                model("deepseek-flash", "DeepSeek-V4.1-Flash", &["fast", "tools", "recommended"], &["dynamic", "low", "medium", "high"], ModelFamily::Flash),
                model("deepseek-pro", "DeepSeek-V4-Pro", &["thinking"], &["max", "high", "medium", "low"], ModelFamily::Pro),
            ],
        },
        ProviderConfig {
            name: OPENAI_ID.into(),
            label: "OpenAI".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            models: vec![
                model("gpt-4o", "GPT-4o (Omni)", &["flagship", "multimodal", "tools"], &[], ModelFamily::Generic),
                model("gpt-4o-mini", "GPT-4o Mini", &["fast", "cheap", "tools"], &[], ModelFamily::Generic),
                model("o3-mini", "o3-mini", &["reasoning", "coding", "fast"], &["low", "medium", "high"], ModelFamily::Generic),
                model("o1", "o1", &["reasoning", "deep", "complex"], &["low", "medium", "high"], ModelFamily::Generic),
            ],
        },
        ProviderConfig {
            name: ANTHROPIC_ID.into(),
            label: "Anthropic (Claude)".into(),
            base_url: "https://api.anthropic.com/v1".into(),
            api_key_env: "ANTHROPIC_API_KEY".into(),
            models: vec![
                model("claude-3-7-sonnet-20250219", "Claude 3.7 Sonnet", &["hybrid", "thinking", "tools", "recommended"], &["dynamic", "low", "medium", "high"], ModelFamily::Generic),
                model("claude-3-5-sonnet-20241022", "Claude 3.5 Sonnet", &["coding", "agentic", "tools"], &[], ModelFamily::Generic),
                model("claude-3-5-haiku-20241022", "Claude 3.5 Haiku", &["fast", "cheap", "tools"], &[], ModelFamily::Generic),
            ],
        },
        ProviderConfig {
            name: GOOGLE_ID.into(),
            label: "Google (Gemini)".into(),
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai".into(),
            api_key_env: "GEMINI_API_KEY".into(),
            models: vec![
                model("gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite", &["recommended", "free-tier", "fastest", "tools"], &[], ModelFamily::Generic),
                model("gemini-3.5-flash", "Gemini 3.5 Flash", &["free-tier", "multimodal", "tools"], &[], ModelFamily::Generic),
                model("gemini-3.8-flash", "Gemini 3.8 Flash", &["latest", "flagship", "tools"], &[], ModelFamily::Generic),
                model("gemini-flash-latest", "Gemini Flash (Latest)", &["auto-updating"], &[], ModelFamily::Generic),
            ],
        },
        ProviderConfig {
            name: GITHUB_ID.into(),
            label: "GitHub Models (Free)".into(),
            base_url: "https://models.inference.ai.azure.com".into(),
            api_key_env: "GITHUB_TOKEN".into(),
            models: vec![
                model("gpt-4o", "GitHub - GPT-4o", &["free", "flagship", "tools"], &[], ModelFamily::Generic),
                model("gpt-4o-mini", "GitHub - GPT-4o Mini", &["free", "fast", "tools"], &[], ModelFamily::Generic),
                model("o3-mini", "GitHub - o3-mini", &["free", "reasoning", "coding"], &["low", "medium", "high"], ModelFamily::Generic),
            ],
        },
        ProviderConfig {
            name: GROQ_ID.into(),
            label: "Groq (Free / Ultra-Fast)".into(),
            base_url: "https://api.groq.com/openai/v1".into(),
            api_key_env: "GROQ_API_KEY".into(),
            models: vec![
                model("llama-3.3-70b-versatile", "Groq - Llama 3.3 70B", &["free-tier", "ultra-fast", "tools"], &[], ModelFamily::Generic),
                model("llama-3.1-8b-instant", "Groq - Llama 3.1 8B", &["free-tier", "fastest", "tools"], &[], ModelFamily::Generic),
                model("deepseek-r1-distill-llama-70b", "Groq - DeepSeek R1 70B", &["free-tier", "reasoning"], &[], ModelFamily::Generic),
            ],
        },
        ProviderConfig {
            name: OPENROUTER_ID.into(),
            label: "OpenRouter".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key_env: "OPENROUTER_API_KEY".into(),
            models: vec![
                model("anthropic/claude-3.7-sonnet", "Claude 3.7 Sonnet (Router)", &["flagship", "tools"], &[], ModelFamily::Generic),
                model("openai/gpt-4o", "GPT-4o (Router)", &["flagship", "tools"], &[], ModelFamily::Generic),
                model("deepseek/deepseek-chat", "DeepSeek-V3 (Router)", &["cheap", "fast", "tools"], &[], ModelFamily::Generic),
                model("meta-llama/llama-3.3-70b-instruct", "Llama 3.3 70B (Router)", &["open-weights", "tools"], &[], ModelFamily::Generic),
            ],
        },
        ProviderConfig {
            name: MISTRAL_ID.into(),
            label: "Mistral AI".into(),
            base_url: "https://api.mistral.ai/v1".into(),
            api_key_env: "MISTRAL_API_KEY".into(),
            models: vec![
                model("codestral-latest", "Codestral (Coding)", &["coding", "tools", "recommended"], &[], ModelFamily::Generic),
                model("mistral-large-latest", "Mistral Large", &["flagship", "reasoning", "tools"], &[], ModelFamily::Generic),
            ],
        },
        ProviderConfig {
            name: LOCAL_ID.into(),
            label: "Local".into(),
            base_url: String::new(),
            api_key_env: String::new(),
            models: vec![model("local-assistant", "Local Offline Assistant", &["private", "$0.00"], &[], ModelFamily::Generic)],
        },
    ]
}

/// Built-ins merged with user-defined providers. A user provider with the same `name`
/// replaces the built-in one; new names are appended before `local`.
pub fn merge_providers(custom: &[ProviderConfig]) -> Vec<ProviderConfig> {
    let mut all = builtin_providers();
    for c in custom {
        if let Some(slot) = all.iter_mut().find(|p| p.name == c.name) {
            *slot = c.clone();
        } else {
            let at = all.iter().position(|p| p.is_local()).unwrap_or(all.len());
            all.insert(at, c.clone());
        }
    }
    all
}

/// Finds which provider/model pair corresponds to the current configuration.
pub fn locate_active(
    providers: &[ProviderConfig],
    active_provider: Option<&str>,
    model: &str,
    local_enabled: bool,
) -> (usize, usize) {
    let pi = if local_enabled {
        providers.iter().position(|p| p.is_local())
    } else {
        active_provider
            .and_then(|n| providers.iter().position(|p| p.name == n))
            .or_else(|| {
                let m = model.to_ascii_lowercase();
                if m.starts_with("gpt-") || m.starts_with("o1") || m.starts_with("o3") {
                    providers.iter().position(|p| p.is_openai() || p.is_github())
                } else if m.starts_with("claude") {
                    providers.iter().position(|p| p.is_anthropic())
                } else if m.starts_with("gemini") {
                    providers.iter().position(|p| p.is_google())
                } else {
                    None
                }
            })
            .or_else(|| providers.iter().position(|p| p.is_deepseek()))
    }
    .unwrap_or(0);

    let models = &providers[pi].models;
    let mi = models
        .iter()
        .position(|m| m.id == model)
        .or_else(|| {
            if providers[pi].is_deepseek() {
                let want = if crate::config::is_pro_model(model) { ModelFamily::Pro } else { ModelFamily::Flash };
                models.iter().position(|m| m.family == want)
            } else {
                None
            }
        })
        .unwrap_or(0);
    (pi, mi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_provider_overrides_and_inserts_before_local() {
        let custom = vec![
            ProviderConfig {
                name: "together".into(),
                label: "Together AI".into(),
                base_url: "https://api.together.xyz/v1".into(),
                api_key_env: "TOGETHER_API_KEY".into(),
                models: vec![],
            },
            ProviderConfig {
                name: "groq".into(),
                label: "Custom Groq".into(),
                base_url: "https://custom.groq.internal/v1".into(),
                api_key_env: "GROQ_API_KEY".into(),
                models: vec![],
            },
        ];
        let all = merge_providers(&custom);
        assert_eq!(all.last().unwrap().name, LOCAL_ID);
        assert!(all.iter().any(|p| p.name == "together"));
        let groq_entry = all.iter().find(|p| p.name == "groq").unwrap();
        assert_eq!(groq_entry.label, "Custom Groq");
    }

    #[test]
    fn locate_active_maps_legacy_deepseek_names() {
        let all = builtin_providers();
        let (p, m) = locate_active(&all, None, "deepseek-reasoner", false);
        assert!(all[p].is_deepseek());
        assert_eq!(all[p].models[m].family, ModelFamily::Pro);
        let (p, _) = locate_active(&all, None, "anything", true);
        assert!(all[p].is_local());
    }
}
