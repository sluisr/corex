use std::path::PathBuf;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uti_core::types::ToolDefinition;

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub workspace_dir: PathBuf,
    pub yolo_mode: bool,
    pub sudo_password: Option<String>,
    pub allowed_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub output: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_summary: Option<String>,
}

impl ToolOutput {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            display_summary: None,
        }
    }

    pub fn success_with_summary(output: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            display_summary: Some(summary.into()),
        }
    }

    pub fn error(err: impl Into<String>) -> Self {
        Self {
            output: err.into(),
            is_error: true,
            display_summary: None,
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;

    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition::function(self.name(), self.description(), self.parameters())
    }

    fn needs_confirmation(&self, _args: &serde_json::Value, _context: &ToolContext) -> bool {
        false
    }

    fn format_diff(&self, _args: &serde_json::Value, _workspace: &std::path::Path) -> Option<String> {
        None
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput>;
}
