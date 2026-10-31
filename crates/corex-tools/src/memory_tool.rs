use anyhow::Result;
use async_trait::async_trait;
use corex_core::memory::{Memory, MemoryScope};
use serde_json::json;

use crate::types::{Tool, ToolContext, ToolOutput};

pub struct SaveMemoryTool;

fn scope(args: &serde_json::Value) -> MemoryScope {
    match args.get("scope").and_then(|v| v.as_str()) {
        Some("project") => MemoryScope::Project,
        _ => MemoryScope::Global,
    }
}

fn fact(args: &serde_json::Value) -> Option<&str> {
    args.get("fact").and_then(|v| v.as_str()).map(str::trim).filter(|f| !f.is_empty())
}

#[async_trait]
impl Tool for SaveMemoryTool {
    fn name(&self) -> &'static str {
        "save_memory"
    }

    fn description(&self) -> &'static str {
        "Saves a concise fact about the user or this project so it is loaded into the system prompt of every future session. \
Use it when the user asks you to remember something, or states a lasting preference or fact. \
Do NOT use it for session-specific context or temporary data.\n\
Scopes:\n\
- global (default): personal preferences that apply in every workspace (saved to ~/.corex/COREX.md).\n\
- project: notes about this workspace, private to the user and never committed (saved under ~/.corex/projects/)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "fact": {
                    "type": "string",
                    "description": "The fact to remember, as one clear, self-contained statement."
                },
                "scope": {
                    "type": "string",
                    "enum": ["global", "project"],
                    "description": "'global' (default) for personal preferences, 'project' for private notes about this workspace."
                }
            },
            "required": ["fact"],
            "additionalProperties": false
        })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        if scope(args) == MemoryScope::Global {
            return true;
        }
        !context.allowed_commands.iter().any(|c| c == self.name())
    }

    fn format_diff(&self, args: &serde_json::Value, workspace: &std::path::Path) -> Option<String> {
        let memory = Memory::new(workspace);
        let target = memory.display_path(&memory.add_target(scope(args))?);
        Some(format!("--- a/{target}\n+++ b/{target}\n+- {}\n", fact(args)?))
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let Some(fact) = fact(&args) else {
            return Ok(ToolOutput::error("Missing 'fact' argument."));
        };
        match Memory::new(&context.workspace_dir).add(scope(&args), fact) {
            Ok(target) => Ok(ToolOutput::success_with_summary(
                format!("Saved to {target}. It is loaded from the next message on."),
                format!("Saved to {target}"),
            )),
            Err(e) => Ok(ToolOutput::error(format!("Could not save memory: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_global_scope() {
        assert_eq!(scope(&json!({ "fact": "x" })), MemoryScope::Global);
        assert_eq!(scope(&json!({ "fact": "x", "scope": "project" })), MemoryScope::Project);
    }

    #[test]
    fn diff_shows_the_line_and_its_file() {
        let diff = SaveMemoryTool
            .format_diff(&json!({ "fact": " likes emojis " }), std::path::Path::new("/tmp/ws"))
            .unwrap();
        assert!(diff.contains("~/.corex/COREX.md"));
        assert!(diff.ends_with("+- likes emojis\n"));
    }

    #[tokio::test]
    async fn rejects_an_empty_fact() {
        let ctx = ToolContext {
            workspace_dir: "/tmp/ws".into(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };
        let out = SaveMemoryTool.execute(json!({ "fact": "  " }), &ctx).await.unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn global_memory_always_requires_confirmation_even_when_whitelisted() {
        let ctx = ToolContext {
            workspace_dir: "/tmp/ws".into(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: vec!["save_memory".to_string()],
            extra_dirs: Vec::new(),
        };
        assert!(SaveMemoryTool.needs_confirmation(&json!({ "fact": "always run hook" }), &ctx));
        assert!(SaveMemoryTool.needs_confirmation(&json!({ "fact": "x", "scope": "global" }), &ctx));
        assert!(!SaveMemoryTool.needs_confirmation(&json!({ "fact": "x", "scope": "project" }), &ctx));
    }
}
