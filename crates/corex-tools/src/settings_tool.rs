use anyhow::Result;
use async_trait::async_trait;
use corex_core::config::{Config, AGENT_SETTABLE};
use serde_json::json;

use crate::types::{Tool, ToolContext, ToolOutput};

pub const SAVED_PREFIX: &str = "Setting saved: ";

pub struct CorexSettingsTool;

fn is_set(args: &serde_json::Value) -> bool {
    args.get("action").and_then(|v| v.as_str()) == Some("set")
}

#[async_trait]
impl Tool for CorexSettingsTool {
    fn name(&self) -> &'static str {
        "corex_settings"
    }

    fn description(&self) -> &'static str {
        "Reads or changes Corex's own settings (~/.corex/settings.json), the only way to reach that file: \
other tools refuse ~/.corex. 'get' returns the settings with API keys, tokens and passwords hidden. \
'set' changes one setting after the user approves it, saves it and applies it to this session. \
Every setting can be set except the API keys, which only the user changes with /key. \
yolo_mode applies to this session only; providers takes the whole provider list (never stores API keys); \
mcp_servers takes the whole map and keeps any value left as [hidden]. \
yolo_mode, allowed_commands, providers and mcp_servers always wait for the user's approval, even in YOLO mode. \
Use it when the user asks you to configure Corex itself."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["get", "set"] },
                "key": { "type": "string", "enum": AGENT_SETTABLE, "description": "Setting to change (set only)." },
                "value": { "description": "New value with its JSON type (true, 256000, \"deepseek-flash\"); null resets it to the default." }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, _context: &ToolContext) -> bool {
        is_set(args)
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let mut cfg = Config::load_with_workspace(Some(&context.workspace_dir));
        if !is_set(&args) {
            let shown = serde_json::to_string_pretty(&cfg.redacted_json()).unwrap_or_default();
            return Ok(ToolOutput::success_with_summary(shown, "Read Corex settings"));
        }
        let Some(key) = args.get("key").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::error("Missing 'key'."));
        };
        let value = args.get("value").cloned().unwrap_or(serde_json::Value::Null);
        if let Err(e) = cfg.set_agent_setting(key, value.clone()) {
            return Ok(ToolOutput::error(e));
        }
        if let Err(e) = cfg.save_agent_setting(key, Some(&context.workspace_dir)) {
            return Ok(ToolOutput::error(format!("Could not save settings.json: {e}")));
        }
        Ok(ToolOutput::success_with_summary(format!("{SAVED_PREFIX}{key} = {value}"), format!("{key} = {value}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_leave_the_settings() {
        let mut cfg = Config::default();
        cfg.api_key = "sk-live-123".into();
        cfg.provider_api_keys.insert("openai".into(), "sk-oai".into());
        cfg.context_limit_tokens = Some(256_000);
        let redacted = cfg.redacted_json();
        let shown = serde_json::to_string(&redacted).unwrap();
        assert!(!shown.contains("sk-live-123") && !shown.contains("sk-oai"), "{shown}");
        assert!(shown.contains("auto_compact"));
        assert_eq!(redacted["context_limit_tokens"], json!(256_000));
    }

    #[test]
    fn only_safe_settings_can_be_set() {
        let mut cfg = Config::default();
        assert!(cfg.set_agent_setting("context_limit_tokens", json!(256000)).is_ok());
        assert_eq!(cfg.context_limit_tokens, Some(256_000));
        assert!(cfg.set_agent_setting("context_limit_tokens", json!(null)).is_ok());
        assert_eq!(cfg.context_limit_tokens, None);
        assert!(cfg.set_agent_setting("auto_compact", json!("yes")).is_err());
        for key in ["api_key", "provider_api_keys"] {
            assert!(cfg.set_agent_setting(key, json!("x")).is_err(), "{key}");
        }
        assert!(cfg.set_agent_setting("allowed_commands", json!(["cargo test"])).is_ok());
        assert_eq!(cfg.allowed_commands, ["cargo test"]);

        cfg.api_key = "sk-keep".into();
        cfg.provider_api_keys.insert("nvidia".into(), "nvapi-keep".into());
        assert!(cfg.set_agent_setting(
            "providers",
            json!([{
                "name": "nvidia",
                "models": [{"id": "moonshotai/kimi-k3.1-pro", "label": "Kimi K3.1 Pro", "reasoning": ["low", "medium", "high"]}]
            }])
        ).is_ok());
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(cfg.providers[0].name, "nvidia");
        assert_eq!(cfg.providers[0].models[0].id, "moonshotai/kimi-k3.1-pro");
        assert_eq!(cfg.api_key, "sk-keep");
        assert_eq!(cfg.provider_api_keys.get("nvidia").map(String::as_str), Some("nvapi-keep"));

        assert!(cfg.set_agent_setting("active_provider", json!("nvidia")).is_ok());
        assert_eq!(cfg.active_provider.as_deref(), Some("nvidia"));
        assert!(cfg.set_agent_setting("active_provider", json!(null)).is_ok());
        assert_eq!(cfg.active_provider, None);
    }

    #[test]
    fn hidden_mcp_secrets_survive_a_round_trip() {
        let mut cfg = Config::default();
        let mut server = corex_core::McpServerConfig::remote("https://mcp.example", Default::default());
        server.headers.insert("Authorization".into(), "Bearer real".into());
        cfg.mcp_servers.insert("stitch".into(), server);
        let shown = cfg.redacted_json()["mcp_servers"].clone();
        assert_eq!(shown["stitch"]["headers"]["Authorization"], "[hidden]");
        cfg.set_agent_setting("mcp_servers", shown).unwrap();
        assert_eq!(cfg.mcp_servers["stitch"].headers["Authorization"], "Bearer real");
    }

    #[test]
    fn only_set_asks_for_approval() {
        let ctx = ToolContext {
            workspace_dir: "/tmp/ws".into(),
            yolo_mode: false,
            yolo_all: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };
        assert!(!CorexSettingsTool.needs_confirmation(&json!({"action": "get"}), &ctx));
        assert!(CorexSettingsTool.needs_confirmation(&json!({"action": "set", "key": "auto_compact", "value": false}), &ctx));
    }

    #[test]
    fn yolo_mcp_and_commands_ask_even_in_yolo() {
        for key in ["yolo_mode", "mcp_servers", "allowed_commands", "providers"] {
            assert!(crate::policy::needs_approval_even_in_yolo("corex_settings", &json!({"action": "set", "key": key})), "{key}");
        }
        assert!(!crate::policy::needs_approval_even_in_yolo("corex_settings", &json!({"action": "set", "key": "auto_compact"})));
    }
}
