use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::types::{Tool, ToolContext, ToolOutput};

pub const TRACKER_ACTIONS: [(&str, &str); 6] = [
    ("create", "tracker_create_task"),
    ("update", "tracker_update_task"),
    ("get", "tracker_get_task"),
    ("list", "tracker_list_tasks"),
    ("add_dependency", "tracker_add_dependency"),
    ("visualize", "tracker_visualize"),
];

pub const FLEET_ACTIONS: [(&str, &str); 4] = [
    ("list_nodes", "fleet_list_nodes"),
    ("telemetry", "fleet_get_telemetry"),
    ("run_command", "fleet_run_command"),
    ("broadcast_command", "fleet_broadcast_command"),
];

// Lets code keyed on member names (`fleet_run_command`) recognise the grouped call.
pub fn canonical_name(name: &str, args: &Value) -> String {
    let action = args.get("action").or_else(|| args.get("Action")).and_then(Value::as_str).unwrap_or("");
    if name == "subagent" {
        return if matches!(action.to_ascii_lowercase().as_str(), "delegate" | "spawn" | "run")
            || (action.is_empty() && args.get("prompt").is_some())
        {
            "delegate_subagent".to_string()
        } else if !action.is_empty() {
            "manage_subagents".to_string()
        } else {
            "subagent".to_string()
        };
    }
    let table: &[(&str, &str)] = match name {
        "tracker" => &TRACKER_ACTIONS,
        "fleet" => &FLEET_ACTIONS,
        _ => return name.to_string(),
    };
    table
        .iter()
        .find(|(a, _)| a.eq_ignore_ascii_case(action))
        .map_or(name, |(_, member)| member)
        .to_string()
}

pub fn canonical_name_raw(name: &str, raw_args: &str) -> String {
    match name {
        "tracker" | "fleet" | "subagent" => {
            canonical_name(name, &serde_json::from_str(raw_args).unwrap_or(Value::Null))
        }
        _ => name.to_string(),
    }
}

pub struct ActionGroupTool {
    name: &'static str,
    description: String,
    actions: Vec<(&'static str, Arc<dyn Tool>)>,
}

impl ActionGroupTool {
    pub fn new(name: &'static str, summary: &str, actions: Vec<(&'static str, Arc<dyn Tool>)>) -> Self {
        let listed: Vec<String> = actions
            .iter()
            .map(|(action, tool)| format!("'{action}': {}", tool.description()))
            .collect();
        let description = format!("{summary} Actions: {}", listed.join(" | "));
        Self { name, description, actions }
    }

    pub fn actions(&self) -> impl Iterator<Item = (&'static str, &Arc<dyn Tool>)> {
        self.actions.iter().map(|(action, tool)| (*action, tool))
    }

    fn resolve(&self, args: &Value) -> Option<(&Arc<dyn Tool>, Value)> {
        let action = args.get("action").or_else(|| args.get("Action"))?.as_str()?.to_ascii_lowercase();
        let tool = self.actions.iter().find(|(name, _)| *name == action).map(|(_, tool)| tool)?;
        let mut rest = args.clone();
        if let Some(map) = rest.as_object_mut() {
            map.remove("action");
            map.remove("Action");
        }
        Some((tool, rest))
    }
}

#[async_trait]
impl Tool for ActionGroupTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        let mut properties = Map::new();
        let names: Vec<&str> = self.actions.iter().map(|(name, _)| *name).collect();
        properties.insert("action".into(), json!({ "type": "string", "enum": names }));
        for (action, tool) in &self.actions {
            let schema = tool.parameters();
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let Some(props) = schema.get("properties").and_then(Value::as_object) else { continue };
            for (key, prop) in props {
                let entry = properties.entry(key.clone()).or_insert_with(|| prop.clone());
                if let Some(obj) = entry.as_object_mut() {
                    let base = obj.get("description").and_then(Value::as_str).unwrap_or("").to_string();
                    let tag = if required.contains(&key.as_str()) { "required" } else { "optional" };
                    let note = format!("[{action}: {tag}]");
                    let merged = if base.is_empty() { note } else { format!("{base} {note}") };
                    obj.insert("description".into(), Value::String(merged));
                }
            }
        }
        json!({ "type": "object", "properties": properties, "required": ["action"] })
    }

    fn needs_confirmation(&self, args: &Value, context: &ToolContext) -> bool {
        self.resolve(args).is_some_and(|(tool, rest)| tool.needs_confirmation(&rest, context))
    }

    fn format_diff(&self, args: &Value, workspace: &std::path::Path) -> Option<String> {
        self.resolve(args).and_then(|(tool, rest)| tool.format_diff(&rest, workspace))
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolOutput> {
        match self.resolve(&args) {
            Some((tool, rest)) => tool.execute(rest, context).await,
            None => {
                let names: Vec<&str> = self.actions.iter().map(|(name, _)| *name).collect();
                Ok(ToolOutput::error(format!(
                    "{} needs an 'action': one of {}.",
                    self.name,
                    names.join(", ")
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo(&'static str, bool);

    #[async_trait]
    impl Tool for Echo {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "echoes"
        }
        fn parameters(&self) -> Value {
            json!({"type": "object", "properties": {"id": {"type": "string", "description": "Task id."}}, "required": ["id"]})
        }
        fn needs_confirmation(&self, _args: &Value, _context: &ToolContext) -> bool {
            self.1
        }
        async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolOutput> {
            Ok(ToolOutput::success(format!("{} {}", self.0, args)))
        }
    }

    fn group() -> ActionGroupTool {
        ActionGroupTool::new(
            "tracker",
            "Track tasks.",
            vec![("get", Arc::new(Echo("get", false))), ("delete", Arc::new(Echo("delete", true)))],
        )
    }

    fn ctx() -> ToolContext {
        ToolContext {
            workspace_dir: std::env::temp_dir(),
            yolo_mode: false,
            yolo_all: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        }
    }

    #[tokio::test]
    async fn dispatches_by_action_without_the_action_key() {
        let out = group().execute(json!({"action": "get", "id": "7"}), &ctx()).await.unwrap();
        assert_eq!(out.output, r#"get {"id":"7"}"#);
        assert!(group().execute(json!({"id": "7"}), &ctx()).await.unwrap().is_error);
    }

    #[test]
    fn canonical_name_maps_grouped_calls_to_members() {
        assert_eq!(canonical_name("fleet", &json!({"action": "run_command"})), "fleet_run_command");
        assert_eq!(canonical_name_raw("tracker", r#"{"action":"create"}"#), "tracker_create_task");
        assert_eq!(canonical_name("subagent", &json!({"action": "delegate", "role": "scout", "prompt": "x"})), "delegate_subagent");
        assert_eq!(canonical_name("subagent", &json!({"role": "scout", "prompt": "x"})), "delegate_subagent");
        assert_eq!(canonical_name("subagent", &json!({"action": "status", "task_id": "s1"})), "manage_subagents");
        assert_eq!(canonical_name("fleet", &json!({})), "fleet");
        assert_eq!(canonical_name("read_file", &json!({"action": "x"})), "read_file");
    }

    #[test]
    fn confirmation_follows_the_chosen_action() {
        assert!(group().needs_confirmation(&json!({"action": "delete", "id": "1"}), &ctx()));
        assert!(!group().needs_confirmation(&json!({"action": "get", "id": "1"}), &ctx()));
    }

    #[test]
    fn schema_merges_properties_and_marks_where_they_are_required() {
        let schema = group().parameters();
        assert_eq!(schema["properties"]["action"]["enum"], json!(["get", "delete"]));
        let id = schema["properties"]["id"]["description"].as_str().unwrap();
        assert!(id.contains("[get: required]") && id.contains("[delete: required]"), "{id}");
        assert_eq!(schema["required"], json!(["action"]));
    }
}
