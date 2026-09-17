use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use anyhow::Result;
use async_trait::async_trait;
use ignore::WalkBuilder;
use serde_json::json;
use similar::{ChangeTag, TextDiff};

use crate::types::{Tool, ToolContext, ToolOutput};

/// Resolves a file path: expands `~/` to the user home directory,
/// preserves absolute paths, and joins relative paths with the workspace directory.
pub fn resolve_path(workspace: &Path, rel: &str) -> PathBuf {
    if let Some(stripped) = rel.strip_prefix("~/") {
        if let Some(base) = directories::BaseDirs::new() {
            return base.home_dir().join(stripped);
        }
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace.join(p)
    }
}

/// Detects if a file is binary by inspecting the first 1024 bytes for null bytes.
pub fn is_binary_file(path: &Path) -> bool {
    if let Ok(mut file) = fs::File::open(path) {
        let mut buffer = [0u8; 1024];
        if let Ok(bytes_read) = file.read(&mut buffer) {
            return buffer[..bytes_read].contains(&0);
        }
    }
    false
}

/// Writes content to a file atomically via a temporary file in the same directory,
/// preventing corruption if the process is interrupted.
pub fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let pid = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = path.with_extension(format!("tmp.{}.{}", pid, ts));
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, path)
}

/// Formats byte counts into clean human-readable representations.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ─── 1. ReadFileTool ────────────────────────────────────────────────────────

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Reads the contents of a file with line numbers and optional start/end ranges. Includes binary file protection and automatic pagination. [PREFERRED instead of shell cat]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path of the file to read (relative or absolute, supports ~/)."
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

        if path.is_dir() {
            return Ok(ToolOutput::error(format!(
                "Path '{}' is a directory, not a file. Use 'list_directory' to inspect directory contents.",
                rel_path
            )));
        }

        if is_binary_file(&path) {
            let metadata = fs::metadata(&path).ok();
            let size = metadata.map(|m| m.len()).unwrap_or(0);
            return Ok(ToolOutput::success(format!(
                "[Binary file: {} ({}). Cannot display content as plain text]",
                rel_path,
                format_size(size)
            )));
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to read {}: {}", rel_path, e))),
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start_line = args.get("start_line").and_then(|v| v.as_u64()).map(|v| v as usize);
        let end_line = args.get("end_line").and_then(|v| v.as_u64()).map(|v| v as usize);

        let default_limit = 1000;
        let s = start_line.unwrap_or(1).saturating_sub(1);
        let e = match (start_line, end_line) {
            (_, Some(end)) => end.min(total_lines),
            (Some(start), None) => (start.saturating_sub(1) + default_limit).min(total_lines),
            (None, None) => default_limit.min(total_lines),
        };

        if s >= total_lines && total_lines > 0 {
            return Ok(ToolOutput::error(format!(
                "Start line {} exceeds total lines {} in {}",
                s + 1,
                total_lines,
                rel_path
            )));
        }

        let mut output = String::new();
        for (i, line) in lines[s..e].iter().enumerate() {
            output.push_str(&format!("{:4} | {}\n", s + i + 1, line));
        }

        if e < total_lines {
            output.push_str(&format!(
                "\n... [Showing lines {}-{} of {}. Specify start_line={} to continue reading]\n",
                s + 1,
                e,
                total_lines,
                e + 1
            ));
        }

        if output.is_empty() {
            output = "(Empty file)".to_string();
        }

        Ok(ToolOutput::success(output))
    }
}

// ─── 2. WriteFileTool ───────────────────────────────────────────────────────

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Writes complete content to a file atomically, creating any missing parent directories. [PREFERRED for new files]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to write (relative or absolute, supports ~/)."
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
        let existed = path.exists();
        let old_size = if existed {
            fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        if let Err(e) = atomic_write(&path, content) {
            return Ok(ToolOutput::error(format!("Failed to write to {}: {}", rel_path, e)));
        }

        let line_count = content.lines().count();
        let byte_count = content.len();
        let summary_msg = if existed {
            format!(
                "Successfully updated {} ({} lines, {}, previous size: {}).",
                rel_path,
                line_count,
                format_size(byte_count as u64),
                format_size(old_size)
            )
        } else {
            format!(
                "Successfully created {} ({} lines, {}).",
                rel_path,
                line_count,
                format_size(byte_count as u64)
            )
        };

        Ok(ToolOutput::success_with_summary(
            summary_msg,
            format!("Wrote {}", rel_path),
        ))
    }
}

// ─── 3. EditTool ────────────────────────────────────────────────────────────

pub struct EditTool;

fn format_edit_preview(content: &str, start_line: usize, end_line: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total == 0 {
        return String::new();
    }
    let display_start = start_line.saturating_sub(3).max(1);
    let display_end = (end_line + 3).min(total);

    let mut preview = String::new();
    for idx in display_start..=display_end {
        let line_num = idx;
        let line_content = lines[idx - 1];
        let is_modified = line_num >= start_line && line_num <= end_line;
        let marker = if is_modified { ">" } else { " " };
        preview.push_str(&format!("{}{:4} | {}\n", marker, line_num, line_content));
    }
    preview
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Replaces exact text within a file (old_string -> new_string). Includes CRLF/LF resilience, whitespace-tolerant fallback matching, intelligent diagnostics, and immediate edit preview. [PREFERRED for surgical code edits]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative or absolute, supports ~/)."
                },
                "instruction": {
                    "type": "string",
                    "description": "A clear description of what and why the change is being made."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact literal string segment to replace (including indentation and surrounding context lines)."
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
        let formatted = diff
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{}", rel_path), &format!("b/{}", rel_path))
            .to_string();
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

        // 1. Fast exact matching
        let count = content.matches(old_string).count();
        let (modified, start_line, end_line) = if count == 1 {
            let offset = content.find(old_string).unwrap();
            let start_line = content[..offset].chars().filter(|&c| c == '\n').count() + 1;
            let replaced_lines = new_string.lines().count().max(1);
            let end_line = start_line + replaced_lines - 1;
            let res = content.replacen(old_string, new_string, 1);
            (res, start_line, end_line)
        } else if count > 1 {
            if !allow_multiple {
                return Ok(ToolOutput::error(format!(
                    "old_string occurs {} times in {}. Specify allow_multiple: true or provide more surrounding context lines.",
                    count, rel_path
                )));
            }
            let res = content.replace(old_string, new_string);
            (res, 1, 1)
        } else {
            // 2. CRLF/LF newline normalization match
            let content_norm = content.replace("\r\n", "\n");
            let old_norm = old_string.replace("\r\n", "\n");
            let new_norm = new_string.replace("\r\n", "\n");
            let norm_count = content_norm.matches(&old_norm).count();

            if norm_count == 1 {
                let offset = content_norm.find(&old_norm).unwrap();
                let start_line = content_norm[..offset].chars().filter(|&c| c == '\n').count() + 1;
                let replaced_lines = new_norm.lines().count().max(1);
                let end_line = start_line + replaced_lines - 1;
                let mut res = content_norm.replacen(&old_norm, &new_norm, 1);
                if content.contains("\r\n") {
                    res = res.replace('\n', "\r\n");
                }
                (res, start_line, end_line)
            } else if norm_count > 1 && !allow_multiple {
                return Ok(ToolOutput::error(format!(
                    "old_string occurs {} times in {} (with normalized line endings). Specify allow_multiple: true or provide more surrounding context lines.",
                    norm_count, rel_path
                )));
            } else if norm_count > 1 && allow_multiple {
                let mut res = content_norm.replace(&old_norm, &new_norm);
                if content.contains("\r\n") {
                    res = res.replace('\n', "\r\n");
                }
                (res, 1, 1)
            } else {
                // 3. Flexible line-by-line trimmed matching fallback
                let content_lines: Vec<&str> = content.lines().collect();
                let old_lines: Vec<&str> = old_string.lines().collect();
                let old_trimmed: Vec<&str> = old_lines.iter().map(|l| l.trim()).collect();

                let mut matching_indices = Vec::new();
                if !old_trimmed.is_empty() && content_lines.len() >= old_trimmed.len() {
                    for i in 0..=content_lines.len() - old_trimmed.len() {
                        let matches_all = (0..old_trimmed.len())
                            .all(|j| content_lines[i + j].trim() == old_trimmed[j]);
                        if matches_all {
                            matching_indices.push(i);
                        }
                    }
                }

                if matching_indices.len() == 1 {
                    let start_idx = matching_indices[0];
                    let end_idx = start_idx + old_trimmed.len();
                    let mut new_lines = Vec::new();
                    new_lines.extend_from_slice(&content_lines[..start_idx]);
                    for nl in new_string.lines() {
                        new_lines.push(nl);
                    }
                    new_lines.extend_from_slice(&content_lines[end_idx..]);
                    let line_ending = if content.contains("\r\n") { "\r\n" } else { "\n" };
                    let mut res = new_lines.join(line_ending);
                    if content.ends_with('\n') || content.ends_with("\r\n") {
                        res.push_str(line_ending);
                    }
                    let start_line = start_idx + 1;
                    let replaced_lines = new_string.lines().count().max(1);
                    let end_line = start_line + replaced_lines - 1;
                    (res, start_line, end_line)
                } else if matching_indices.len() > 1 && !allow_multiple {
                    return Ok(ToolOutput::error(format!(
                        "old_string matches {} different locations with trimmed whitespace. Provide more surrounding context lines.",
                        matching_indices.len()
                    )));
                } else {
                    // Intelligent diagnostic hint
                    let first_non_empty = old_lines.iter().find(|l| !l.trim().is_empty()).map(|l| l.trim());
                    let hint = if let Some(target) = first_non_empty {
                        let candidates: Vec<usize> = content_lines
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| l.trim() == target)
                            .map(|(i, _)| i + 1)
                            .take(3)
                            .collect();
                        if !candidates.is_empty() {
                            format!(
                                " Hint: Line(s) {:?} match the start of old_string, but surrounding lines or indentation differed.",
                                candidates
                            )
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    return Ok(ToolOutput::error(format!(
                        "Target old_string was not found in {}. Ensure exact whitespace and context match.{}",
                        rel_path, hint
                    )));
                }
            }
        };

        if let Err(e) = atomic_write(&path, &modified) {
            return Ok(ToolOutput::error(format!("Failed to write edited content to {}: {}", rel_path, e)));
        }

        let preview = format_edit_preview(&modified, start_line, end_line);
        let lines_str = if start_line == end_line {
            format!("line {}", start_line)
        } else {
            format!("lines {}-{}", start_line, end_line)
        };
        let output_msg = if !preview.is_empty() {
            format!(
                "Successfully edited {} ({}):\n{}",
                rel_path, lines_str, preview
            )
        } else {
            format!("Successfully edited {}.", rel_path)
        };

        Ok(ToolOutput::success_with_summary(
            output_msg,
            format!("Edited {}", rel_path),
        ))
    }
}

// ─── 4. GlobTool ────────────────────────────────────────────────────────────

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "Efficiently finds files matching glob patterns (e.g. `src/**/*.rs`, `**/*.md`, `*.json`), skipping .git and respecting .gitignore. [PREFERRED instead of shell find]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g. '**/*.rs', 'src/**/*.ts', '*.toml')."
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
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                name != ".git"
            })
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if let Ok(rel) = path.strip_prefix(&context.workspace_dir) {
                let rel_str = rel.to_string_lossy();
                let file_name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                if let Ok(ref g) = glob_pattern {
                    // Match relative path, full path, or file basename (for patterns like "*.rs")
                    if g.matches(&rel_str) || g.matches(path.to_str().unwrap_or("")) || g.matches(&file_name) {
                        matches.push(rel_str.to_string());
                        if matches.len() >= 150 {
                            break;
                        }
                    }
                }
            }
        }

        matches.sort();
        if matches.is_empty() {
            Ok(ToolOutput::success("No matching files found."))
        } else {
            let count = matches.len();
            let header = if count >= 150 {
                "Found 150+ matching files (capped at 150):\n".to_string()
            } else {
                format!("Found {} matching file(s):\n", count)
            };
            Ok(ToolOutput::success(format!("{}{}", header, matches.join("\n"))))
        }
    }
}

// ─── 5. LsTool ──────────────────────────────────────────────────────────────

pub struct LsTool;

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }

    fn description(&self) -> &'static str {
        "Lists names and sizes of files and subdirectories directly within a specified directory path. [PREFERRED instead of shell ls]"
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

        if !path.is_dir() {
            return Ok(ToolOutput::error(format!("Path '{}' is a file, not a directory. Use 'read_file' instead.", rel_path)));
        }

        let mut entries = Vec::new();
        match fs::read_dir(&path) {
            Ok(read_dir) => {
                for item in read_dir.flatten() {
                    let file_name = item.file_name().to_string_lossy().to_string();
                    if file_name == ".git" {
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

        let total_count = entries.len();
        if entries.len() > 150 {
            entries.truncate(150);
        }

        let mut formatted_entries = Vec::new();
        for (is_dir, name, size) in entries {
            if is_dir {
                formatted_entries.push(format!("[DIR]  {}", name));
            } else {
                formatted_entries.push(format!("{:>10}  {}", format_size(size), name));
            }
        }

        let cap_notice = if total_count > 150 {
            format!("\n... [Showing 150 of {} items]", total_count)
        } else {
            String::new()
        };

        let result = format!(
            "Directory listing for {}:\n{}{}",
            path.display(),
            if formatted_entries.is_empty() {
                "(Directory is empty)".to_string()
            } else {
                formatted_entries.join("\n")
            },
            cap_notice
        );

        Ok(ToolOutput::success(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_file_and_edit() {
        let ws = std::env::temp_dir().join(format!("uti_fs_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&ws);
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        // Write a test file
        let writer = WriteFileTool;
        let res = writer
            .execute(
                json!({
                    "file_path": "hello.rs",
                    "content": "fn main() {\n    println!(\"hello\");\n}\n"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(!res.is_error);

        // Read the test file
        let reader = ReadFileTool;
        let read_res = reader
            .execute(
                json!({
                    "file_path": "hello.rs"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(read_res.output.contains("1 | fn main()"));

        // Edit the test file
        let editor = EditTool;
        let edit_res = editor
            .execute(
                json!({
                    "file_path": "hello.rs",
                    "instruction": "change print",
                    "old_string": "    println!(\"hello\");",
                    "new_string": "    println!(\"world\");"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(!edit_res.is_error);
        assert!(edit_res.output.contains("world"));

        // Verify edited content
        let read_again = reader
            .execute(
                json!({
                    "file_path": "hello.rs"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(read_again.output.contains("println!(\"world\");"));

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn test_edit_trimmed_matching_fallback() {
        let ws = std::env::temp_dir().join(format!("uti_fs_test_trimmed_{}", std::process::id()));
        let _ = fs::create_dir_all(&ws);
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let writer = WriteFileTool;
        writer
            .execute(
                json!({
                    "file_path": "sample.py",
                    "content": "def greet():\n    # greeting\n    print('hi')\n"
                }),
                &context,
            )
            .await
            .unwrap();

        let editor = EditTool;
        // Edit with slightly different leading whitespace in old_string
        let edit_res = editor
            .execute(
                json!({
                    "file_path": "sample.py",
                    "instruction": "update greeting",
                    "old_string": "  # greeting\n  print('hi')",
                    "new_string": "    print('hello there')"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(!edit_res.is_error);

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn test_edit_line_number_calculation() {
        let ws = std::env::temp_dir().join(format!("uti_fs_test_line_calc_{}", std::process::id()));
        let _ = fs::create_dir_all(&ws);
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let content = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10\n";
        let writer = WriteFileTool;
        writer
            .execute(
                json!({
                    "file_path": "test.txt",
                    "content": content
                }),
                &context,
            )
            .await
            .unwrap();

        let editor = EditTool;
        let res = editor
            .execute(
                json!({
                    "file_path": "test.txt",
                    "old_string": "line 8\n",
                    "new_string": "line 8 modified\n"
                }),
                &context,
            )
            .await
            .unwrap();

        assert!(!res.is_error);
        assert!(res.output.contains("(line 8):"), "Output should contain '(line 8):', got: {}", res.output);
        assert!(res.output.contains(">   8 | line 8 modified"), "Output should highlight line 8, got: {}", res.output);

        let diff = editor.format_diff(
            &json!({
                "file_path": "test.txt",
                "old_string": "line 8 modified\n",
                "new_string": "line 8 diff\n"
            }),
            &ws,
        ).unwrap();
        assert!(diff.contains("@@"), "Diff should contain hunk header @@, got: {}", diff);
        assert!(diff.contains("-line 8 modified"));
        assert!(diff.contains("+line 8 diff"));

        let _ = fs::remove_dir_all(&ws);
    }
}
