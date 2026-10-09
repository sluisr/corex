//! Provider / model catalog.
//!
//! The model picker is driven by this data instead of hard-coded per-model tabs. Built-in
//! providers (DeepSeek, OpenAI, OpenRouter, Local) can be extended or overridden by the
//! `providers` list in `settings.json`.

use serde::{Deserialize, Serialize};

pub const DEEPSEEK_ID: &str = "deepseek";
pub const LOCAL_ID: &str = "local";

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

    pub fn is_local(&self) -> bool {
        self.name == LOCAL_ID
    }

    /// API key resolved from the environment (empty when unset).
    pub fn api_key(&self) -> String {
        if self.api_key_env.is_empty() {
            return String::new();
        }
        std::env::var(&self.api_key_env).unwrap_or_default()
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
        let custom = vec![ProviderConfig {
            name: "groq".into(),
            label: String::new(),
            base_url: "https://api.groq.com/openai/v1".into(),
            api_key_env: "GROQ_API_KEY".into(),
            models: vec![],
        }];
        let all = merge_providers(&custom);
        assert_eq!(all.last().unwrap().name, LOCAL_ID);
        assert!(all.iter().any(|p| p.name == "groq"));
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
