use std::fs;
use std::path::{Path, PathBuf};
use anyhow::Result;
use async_trait::async_trait;
use ignore::WalkBuilder;
use serde_json::json;
use similar::{ChangeTag, TextDiff};

use crate::types::{Tool, ToolContext, ToolOutput};

fn resolve_path(workspace: &Path, rel: &str) -> PathBuf {
    let p = Path::new(rel);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace.join(p)
    }
}

// --- ReadFileTool ---
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Reads the contents of a file with optional start and end line ranges. [PREFERRED instead of shell cat]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path of the file to read."
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional 1-indexed start line number."
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional 1-indexed end line number."
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let rel_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("Missing 'file_path' argument.")),
        };

        let path = resolve_path(&context.workspace_dir, rel_path);
        if !path.exists() {
            return Ok(ToolOutput::error(format!("File does not exist: {}", rel_path)));
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to read {}: {}", rel_path, e))),
        };

        let start_line = args.get("start_line").and_then(|v| v.as_u64()).map(|v| v as usize);
        let end_line = args.get("end_line").and_then(|v| v.as_u64()).map(|v| v as usize);

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let s = start_line.unwrap_or(1).saturating_sub(1);
        let e = end_line.unwrap_or(total_lines).min(total_lines);

        if s >= total_lines && total_lines > 0 {
            return Ok(ToolOutput::error(format!("Start line {} exceeds total lines {}", s + 1, total_lines)));
        }

        let mut output = String::new();
        for (i, line) in lines[s..e].iter().enumerate() {
            output.push_str(&format!("{:4} | {}\n", s + i + 1, line));
        }

        if output.is_empty() {
            output = "(Empty file)".to_string();
        }

        Ok(ToolOutput::success(output))
    }
}

// --- WriteFileTool ---
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Writes complete content to a file, creating any missing parent directories. [PREFERRED for new files]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to write."
                },
                "content": {
                    "type": "string",
                    "description": "The exact full text content to write."
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn needs_confirmation(&self, _args: &serde_json::Value, _context: &ToolContext) -> bool {
        true
    }

    fn format_diff(&self, args: &serde_json::Value, workspace: &Path) -> Option<String> {
        let rel_path = args.get("file_path")?.as_str()?;
        let new_content = args.get("content")?.as_str()?;
        let path = resolve_path(workspace, rel_path);

        let old_content = fs::read_to_string(&path).unwrap_or_default();
        let diff = TextDiff::from_lines(old_content.as_str(), new_content);

        let mut formatted = format!("--- a/{}\n+++ b/{}\n", rel_path, rel_path);
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            formatted.push_str(&format!("{}{}", sign, change));
        }
        Some(formatted)
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let rel_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("Missing 'file_path' argument.")),
        };
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("Missing 'content' argument.")),
        };

        let path = resolve_path(&context.workspace_dir, rel_path);
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return Ok(ToolOutput::error(format!("Failed to create directories: {}", e)));
            }
        }

        if let Err(e) = fs::write(&path, content) {
            return Ok(ToolOutput::error(format!("Failed to write to {}: {}", rel_path, e)));
        }

        Ok(ToolOutput::success_with_summary(
            format!("Successfully wrote {} bytes to {}.", content.len(), rel_path),
            format!("Wrote {}", rel_path),
        ))
    }
}

// --- EditTool ---
pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Replaces text within a file (old_string -> new_string). Always provide sufficient surrounding context lines to ensure exact targeting."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to edit."
                },
                "instruction": {
                    "type": "string",
                    "description": "A clear description of what and why the change is being made."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact literal string segment to replace (including exact indentation and surrounding context)."
                },
                "new_string": {
                    "type": "string",
                    "description": "The exact literal replacement string."
                },
                "allow_multiple": {
                    "type": "boolean",
                    "description": "Whether to replace multiple occurrences if found (default false)."
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    fn needs_confirmation(&self, _args: &serde_json::Value, _context: &ToolContext) -> bool {
        true
    }

    fn format_diff(&self, args: &serde_json::Value, workspace: &Path) -> Option<String> {
        let rel_path = args.get("file_path")?.as_str()?;
        let old_str = args.get("old_string")?.as_str()?;
        let new_str = args.get("new_string")?.as_str()?;
        let allow_multiple = args.get("allow_multiple").and_then(|v| v.as_bool()).unwrap_or(false);

        let path = resolve_path(workspace, rel_path);
        let content = fs::read_to_string(&path).ok()?;

        let new_content = if allow_multiple {
            content.replace(old_str, new_str)
        } else {
            content.replacen(old_str, new_str, 1)
        };

        let diff = TextDiff::from_lines(content.as_str(), new_content.as_str());
        let mut formatted = format!("--- a/{}\n+++ b/{}\n", rel_path, rel_path);
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            formatted.push_str(&format!("{}{}", sign, change));
        }
        Some(formatted)
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let rel_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("Missing 'file_path' argument.")),
        };
        let old_string = match args.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolOutput::error("Missing 'old_string' argument.")),
        };
        let new_string = match args.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolOutput::error("Missing 'new_string' argument.")),
        };
        let allow_multiple = args.get("allow_multiple").and_then(|v| v.as_bool()).unwrap_or(false);

        let path = resolve_path(&context.workspace_dir, rel_path);
        if !path.exists() {
            return Ok(ToolOutput::error(format!("File does not exist: {}", rel_path)));
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to read {}: {}", rel_path, e))),
        };

        let count = content.matches(old_string).count();
        if count == 0 {
            return Ok(ToolOutput::error(format!(
                "Target old_string was not found in {}. Ensure exact whitespace and context match.",
                rel_path
            )));
        }

        if count > 1 && !allow_multiple {
            return Ok(ToolOutput::error(format!(
                "old_string occurs {} times in {}. Specify allow_multiple: true or provide more surrounding context lines.",
                count, rel_path
            )));
        }

        let modified = if allow_multiple {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        if let Err(e) = fs::write(&path, modified) {
            return Ok(ToolOutput::error(format!("Failed to write edited content to {}: {}", rel_path, e)));
        }

        Ok(ToolOutput::success_with_summary(
            format!("Replaced {} occurrence(s) in {}.", count, rel_path),
            format!("Edited {}", rel_path),
        ))
    }
}

// --- GlobTool ---
pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "Efficiently finds files matching specific glob patterns (e.g. `src/**/*.rs`, `**/*.md`), returning relative paths sorted by modification time. [PREFERRED instead of shell find]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g. '**/*.rs', 'src/**/*.ts')."
                },
                "dir_path": {
                    "type": "string",
                    "description": "Optional root directory to search in (defaults to workspace root)."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let pattern_str = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("Missing 'pattern' argument.")),
        };

        let root_dir = args
            .get("dir_path")
            .and_then(|v| v.as_str())
            .map(|p| resolve_path(&context.workspace_dir, p))
            .unwrap_or_else(|| context.workspace_dir.clone());

        let glob_pattern = glob::Pattern::new(pattern_str);

        let mut matches = Vec::new();
        let walker = WalkBuilder::new(&root_dir)
            .hidden(false)
            .git_ignore(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if let Ok(rel) = path.strip_prefix(&context.workspace_dir) {
                let rel_str = rel.to_string_lossy();
                if let Ok(ref g) = glob_pattern {
                    if g.matches(&rel_str) || g.matches(path.to_str().unwrap_or("")) {
                        matches.push(rel_str.to_string());
                        if matches.len() >= 100 {
                            break;
                        }
                    }
                }
            }
        }

        if matches.is_empty() {
            Ok(ToolOutput::success("No matching files found."))
        } else {
            Ok(ToolOutput::success(matches.join("\n")))
        }
    }
}

// --- LsTool ---
pub struct LsTool;

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }

    fn description(&self) -> &'static str {
        "Lists the names of files and subdirectories directly within a specified directory path with file sizes. [PREFERRED instead of shell ls]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "dir_path": {
                    "type": "string",
                    "description": "The path to the directory to list (defaults to current working directory)."
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let rel_path = args.get("dir_path").and_then(|v| v.as_str()).unwrap_or(".");
        let path = resolve_path(&context.workspace_dir, rel_path);

        if !path.exists() {
            return Ok(ToolOutput::error(format!("Directory does not exist: {}", rel_path)));
        }

        let mut entries = Vec::new();
        match fs::read_dir(&path) {
            Ok(read_dir) => {
                for item in read_dir.flatten() {
                    let file_name = item.file_name().to_string_lossy().to_string();
                    if file_name.starts_with(".git") && file_name == ".git" {
                        continue;
                    }
                    let metadata = item.metadata();
                    let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

                    entries.push((is_dir, file_name, size));
                }
            }
            Err(e) => return Ok(ToolOutput::error(format!("Failed to list {}: {}", rel_path, e))),
        }

        // Sort entries: directories first, then alphabetically
        entries.sort_by(|a, b| {
            if a.0 && !b.0 {
                std::cmp::Ordering::Less
            } else if !a.0 && b.0 {
                std::cmp::Ordering::Greater
            } else {
                a.1.to_lowercase().cmp(&b.1.to_lowercase())
            }
        });

        let mut formatted_entries = Vec::new();
        for (is_dir, name, size) in entries {
            if is_dir {
                formatted_entries.push(format!("[DIR] {}", name));
            } else {
                formatted_entries.push(format!("{} ({} bytes)", name, size));
            }
        }

        let result = format!(
            "Directory listing for {}:\n{}",
            path.display(),
            if formatted_entries.is_empty() {
                "(Directory is empty)".to_string()
            } else {
                formatted_entries.join("\n")
            }
        );

        Ok(ToolOutput::success(result))
    }
}
