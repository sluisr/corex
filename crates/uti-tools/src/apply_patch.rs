use std::fs;
use std::path::Path;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use diffy::Patch;

use crate::types::{Tool, ToolContext, ToolOutput};

pub struct ApplyPatchTool;

impl ApplyPatchTool {
    fn extract_file_path(hunk_header: &str) -> Option<String> {
        for line in hunk_header.lines() {
            if line.starts_with("+++ ") || line.starts_with("--- ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let mut file = parts[1].trim();
                    if file.starts_with("b/") || file.starts_with("a/") {
                        file = &file[2..];
                    }
                    if file != "/dev/null" && !file.is_empty() {
                        return Some(file.to_string());
                    }
                }
            }
        }
        None
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn description(&self) -> &'static str {
        "Applies unified diff patches directly to files. Fast, atomic, and token-efficient for code editing. [PREFERRED for code edits]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "The unified diff patch content to apply."
                }
            },
            "required": ["patch"]
        })
    }

    fn needs_confirmation(&self, _args: &serde_json::Value, _context: &ToolContext) -> bool {
        true
    }

    fn format_diff(&self, args: &serde_json::Value, _workspace: &Path) -> Option<String> {
        args.get("patch")
            .and_then(|p| p.as_str())
            .map(|s| s.to_string())
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let patch_str = match args.get("patch").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Ok(ToolOutput::error("Error: 'patch' parameter cannot be empty.")),
        };

        let patch = match Patch::from_str(patch_str) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Error: Unable to parse unified diff patch: {}. Make sure it starts with '---' and '+++' or has valid '@@' hunk headers.",
                    e
                )));
            }
        };

        let target_file_rel = Self::extract_file_path(patch_str)
            .unwrap_or_else(|| "unknown".to_string());

        if target_file_rel == "unknown" {
            return Ok(ToolOutput::error("Could not determine target file from unified diff header. Ensure '--- a/file' and '+++ b/file' exist."));
        }

        let target_path = context.workspace_dir.join(&target_file_rel);

        let original_content = if target_path.exists() {
            match fs::read_to_string(&target_path) {
                Ok(c) => c,
                Err(e) => return Ok(ToolOutput::error(format!("Failed to read target file {}: {}", target_path.display(), e))),
            }
        } else {
            String::new()
        };

        let patched_content = match diffy::apply(&original_content, &patch) {
            Ok(res) => res,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Patch application failed for {}: {}. Context line mismatch.",
                    target_file_rel, e
                )));
            }
        };

        if let Some(parent) = target_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return Ok(ToolOutput::error(format!("Failed to create directories for {}: {}", target_path.display(), e)));
            }
        }

        if let Err(e) = fs::write(&target_path, patched_content) {
            return Ok(ToolOutput::error(format!("Failed to write patched content to {}: {}", target_path.display(), e)));
        }

        let action = if original_content.is_empty() { "Created" } else { "Patched" };
        Ok(ToolOutput::success_with_summary(
            format!("{} {} successfully via apply_patch.", action, target_file_rel),
            format!("{} {}", action, target_file_rel),
        ))
    }
}
