use std::fs;
use std::path::Path;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use diffy::Patch;

use crate::fs_tools::{atomic_write, resolve_path};
use crate::types::{Tool, ToolContext, ToolOutput};

pub struct ApplyPatchTool;

impl ApplyPatchTool {
    fn extract_file_path(hunk_header: &str) -> Option<String> {
        // Priority 1: +++ b/path (post-image target)
        for line in hunk_header.lines() {
            if line.starts_with("+++ ") {
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
        // Priority 2: --- a/path (pre-image fallback)
        for line in hunk_header.lines() {
            if line.starts_with("--- ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let mut file = parts[1].trim();
                    if file.starts_with("a/") || file.starts_with("b/") {
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
                },
                "file_path": {
                    "type": "string",
                    "description": "Optional: Path of the file to patch if not specified in diff headers (supports ~/)."
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
            .or_else(|| args.get("input"))
            .or_else(|| args.get("diff"))
            .and_then(|p| p.as_str())
            .or_else(|| args.as_str())
            .map(|s| s.to_string())
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let patch_str = match args.get("patch")
            .or_else(|| args.get("input"))
            .or_else(|| args.get("diff"))
            .and_then(|v| v.as_str())
            .or_else(|| args.as_str())
        {
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

        let target_file = match Self::extract_file_path(patch_str) {
            Some(f) => f,
            None => match args.get("file_path")
                .or_else(|| args.get("path"))
                .or_else(|| args.get("file"))
                .and_then(|v| v.as_str())
            {
                Some(f) => f.to_string(),
                None => {
                    return Ok(ToolOutput::error(
                        "Error: Could not determine target file from diff headers (e.g. '+++ b/path/to/file') and 'file_path' was not provided."
                    ));
                }
            },
        };

        let target_path = resolve_path(&context.workspace_dir, &target_file);

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
                    target_file, e
                )));
            }
        };

        if let Err(e) = atomic_write(&target_path, &patched_content) {
            return Ok(ToolOutput::error(format!("Failed to write patched content to {}: {}", target_path.display(), e)));
        }

        let action = if original_content.is_empty() { "Created" } else { "Patched" };
        Ok(ToolOutput::success_with_summary(
            format!("{} {} successfully via apply_patch.", action, target_file),
            format!("{} {}", action, target_file),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_diff_flexible_keys() {
        let tool = ApplyPatchTool;
        let ws = Path::new("/tmp");

        let args_patch = json!({"patch": "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-a\n+b\n"});
        assert!(tool.format_diff(&args_patch, ws).is_some());

        let args_input = json!({"input": "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-a\n+b\n"});
        assert!(tool.format_diff(&args_input, ws).is_some());

        let args_diff = json!({"diff": "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-a\n+b\n"});
        assert!(tool.format_diff(&args_diff, ws).is_some());
    }
}

