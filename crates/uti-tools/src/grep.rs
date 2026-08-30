use std::fs;
use std::path::Path;
use anyhow::Result;
use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::json;

use crate::types::{Tool, ToolContext, ToolOutput};

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep_search"
    }

    fn description(&self) -> &'static str {
        "Searches for a regular expression pattern within file contents. [PREFERRED for searching text in files — use this instead of run_shell_command with grep]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regular expression (regex) pattern to search for within file contents."
                },
                "dir_path": {
                    "type": "string",
                    "description": "Optional: Directory or file to search within. Defaults to current workspace root if omitted."
                },
                "include_pattern": {
                    "type": "string",
                    "description": "Optional: Glob pattern to filter files (e.g. '*.rs', 'src/**')."
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Optional: If true, search is case-sensitive. Defaults to false."
                },
                "context": {
                    "type": "integer",
                    "description": "Optional: Lines of context around each match (default 1)."
                },
                "total_max_matches": {
                    "type": "integer",
                    "description": "Optional: Maximum total matching lines to return (default 50)."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let pattern_str = args
            .get("pattern")
            .or_else(|| args.get("query"))
            .and_then(|v| v.as_str());

        let pattern = match pattern_str {
            Some(p) => p,
            None => return Ok(ToolOutput::error("Missing 'pattern' argument.")),
        };

        let case_sensitive = args.get("case_sensitive").and_then(|v| v.as_bool()).unwrap_or(false);
        let context_lines = args.get("context").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let max_matches = args
            .get("total_max_matches")
            .or_else(|| args.get("max_matches"))
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;

        let root_dir = args
            .get("dir_path")
            .and_then(|v| v.as_str())
            .map(|p| {
                let path = Path::new(p);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    context.workspace_dir.join(path)
                }
            })
            .unwrap_or_else(|| context.workspace_dir.clone());

        let include_filter = args
            .get("include_pattern")
            .and_then(|v| v.as_str())
            .and_then(|pat| glob::Pattern::new(pat).ok());

        let regex = match RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::error(format!("Invalid regex pattern: {}", e))),
        };

        let mut output = Vec::new();
        let mut total_matches = 0;

        let walker = WalkBuilder::new(&root_dir)
            .hidden(false)
            .git_ignore(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let rel_display = path
                .strip_prefix(&context.workspace_dir)
                .unwrap_or(path)
                .to_string_lossy();

            if let Some(ref filter) = include_filter {
                if !filter.matches(&rel_display) {
                    continue;
                }
            }

            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let lines: Vec<&str> = content.lines().collect();
            for (idx, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    total_matches += 1;
                    let start = idx.saturating_sub(context_lines);
                    let end = (idx + context_lines + 1).min(lines.len());

                    output.push(format!("--- {} ---", rel_display));
                    for (i, l) in lines[start..end].iter().enumerate() {
                        let line_num = start + i + 1;
                        let marker = if line_num == idx + 1 { ">" } else { " " };
                        output.push(format!("{}{:4} | {}", marker, line_num, l));
                    }
                    output.push(String::new());

                    if total_matches >= max_matches {
                        output.push(format!("(Max matches limit {} reached)", max_matches));
                        break;
                    }
                }
            }

            if total_matches >= max_matches {
                break;
            }
        }

        if output.is_empty() {
            Ok(ToolOutput::success(format!("No matches found for '{}'.", pattern)))
        } else {
            Ok(ToolOutput::success(output.join("\n")))
        }
    }
}
