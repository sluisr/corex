use std::fs;
use anyhow::Result;
use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::json;

use crate::fs_tools::{is_valid_walk_file, resolve_path};
use crate::types::{Tool, ToolContext, ToolOutput};
use corex_core::safe_truncate_str;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep_search"
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        crate::fs_tools::read_path_needs_confirmation(
            self.name(),
            args.get("dir_path").and_then(|v| v.as_str()),
            context,
        )
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
            .map(|p| resolve_path(&context.workspace_dir, p))
            .unwrap_or_else(|| context.workspace_dir.clone());

        if crate::fs_tools::is_sensitive_system_path(&root_dir) {
            return Ok(ToolOutput::error(format!(
                "Access denied: searching sensitive credential directory '{}' is forbidden.",
                root_dir.display()
            )));
        }

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

        let pattern_owned = pattern.to_string();
        let workspace_dir = context.workspace_dir.clone();
        let output = tokio::task::spawn_blocking(move || {
            let num_threads = std::thread::available_parallelism()
                .map(|n| n.get().min(12))
                .unwrap_or(4);
            let total_matches_atomic = std::sync::atomic::AtomicUsize::new(0);
            let file_results: std::sync::Mutex<Vec<(String, Vec<(usize, Vec<String>)>)>> =
                std::sync::Mutex::new(Vec::new());

            let walker = WalkBuilder::new(&root_dir)
                .hidden(false)
                .git_ignore(true)
                .threads(num_threads)
                .filter_entry(|entry| {
                    let name = entry.file_name().to_string_lossy();
                    name != ".git"
                })
                .build_parallel();

            walker.run(|| {
                let regex = &regex;
                let include_filter = &include_filter;
                let root_dir = &root_dir;
                let workspace_dir = &workspace_dir;
                let total_matches_atomic = &total_matches_atomic;
                let file_results = &file_results;
                Box::new(move |entry_res| {
                    if total_matches_atomic.load(std::sync::atomic::Ordering::Relaxed) >= max_matches {
                        return ignore::WalkState::Quit;
                    }
                    let Ok(entry) = entry_res else {
                        return ignore::WalkState::Continue;
                    };
                    if !is_valid_walk_file(&entry, root_dir) {
                        return ignore::WalkState::Continue;
                    }
                    let path = entry.path();
                    let rel_display = path
                        .strip_prefix(workspace_dir)
                        .unwrap_or(path)
                        .to_string_lossy();

                    if let Some(ref filter) = include_filter {
                        if !filter.matches(&rel_display) {
                            return ignore::WalkState::Continue;
                        }
                    }

                    // Skip files larger than 10 MB to prevent memory exhaustion / OOM crashes
                    let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    if file_size > 10 * 1024 * 1024 {
                        return ignore::WalkState::Continue;
                    }

                    let Ok(bytes) = fs::read(path) else {
                        return ignore::WalkState::Continue;
                    };
                    if bytes[..bytes.len().min(1024)].contains(&0) {
                        return ignore::WalkState::Continue;
                    }
                    let Ok(content) = String::from_utf8(bytes) else {
                        return ignore::WalkState::Continue;
                    };
                    if !regex.is_match(&content) {
                        return ignore::WalkState::Continue;
                    }

                    let lines: Vec<&str> = content.lines().collect();
                    let mut snippets: Vec<(usize, Vec<String>)> = Vec::new();
                    for (idx, line) in lines.iter().enumerate() {
                        if regex.is_match(line) {
                            let start = idx.saturating_sub(context_lines);
                            let end = (idx + context_lines + 1).min(lines.len());
                            let mut block = Vec::with_capacity(end - start + 2);
                            block.push(format!("--- {} ---", rel_display));
                            for (i, l) in lines[start..end].iter().enumerate() {
                                let line_num = start + i + 1;
                                let marker = if line_num == idx + 1 { ">" } else { " " };
                                let display_line = if l.len() > 350 {
                                    format!("{}... [truncated]", safe_truncate_str(l, 350))
                                } else {
                                    l.to_string()
                                };
                                block.push(format!("{}{:4} | {}", marker, line_num, display_line));
                            }
                            block.push(String::new());
                            snippets.push((idx, block));

                            let prev = total_matches_atomic.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if prev + 1 >= max_matches {
                                break;
                            }
                        }
                    }

                    if !snippets.is_empty() {
                        if let Ok(mut guard) = file_results.lock() {
                            guard.push((rel_display.into_owned(), snippets));
                        }
                    }

                    if total_matches_atomic.load(std::sync::atomic::Ordering::Relaxed) >= max_matches {
                        ignore::WalkState::Quit
                    } else {
                        ignore::WalkState::Continue
                    }
                })
            });

            let mut collected = file_results.into_inner().unwrap_or_default();
            collected.sort_by(|a, b| a.0.cmp(&b.0));

            let mut output = Vec::new();
            let mut total_matches = 0;
            'outer: for (_, snippets) in collected {
                for (_, block) in snippets {
                    total_matches += 1;
                    output.extend(block);
                    if total_matches >= max_matches {
                        output.push(format!("(Max matches limit {} reached)", max_matches));
                        break 'outer;
                    }
                }
            }
            output
        })
        .await?;

        if output.is_empty() {
            Ok(ToolOutput::success(format!("No matches found for '{}'.", pattern_owned)))
        } else {
            Ok(ToolOutput::success(output.join("\n")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn grep_searches_directory_and_single_file_and_skips_binaries() {
        let ws = std::env::temp_dir().join(format!("corex_grep_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(ws.join("src")).unwrap();
        fs::write(ws.join("src/main.rs"), "fn alpha() {}\nfn needle_here() {}\n").unwrap();
        fs::write(ws.join("src/ignore.txt"), "nothing\n").unwrap();
        fs::write(ws.join("src/bin.dat"), b"\x00\x01needle_here\x00").unwrap();

        let ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            yolo_all: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };

        let res = GrepTool
            .execute(json!({ "pattern": "needle_here", "include_pattern": "*.rs" }), &ctx)
            .await
            .unwrap();
        assert!(!res.is_error);
        assert!(res.output.contains("src/main.rs"), "{}", res.output);
        assert!(res.output.contains(">   2 | fn needle_here() {}"), "{}", res.output);
        assert!(!res.output.contains("bin.dat"), "{}", res.output);

        // Searching a single file directly via dir_path (depth == 0) must also work.
        let single = GrepTool
            .execute(json!({ "pattern": "needle_here", "dir_path": "src/main.rs" }), &ctx)
            .await
            .unwrap();
        assert!(!single.is_error);
        assert!(single.output.contains("fn needle_here()"), "{}", single.output);

        let _ = fs::remove_dir_all(&ws);
    }
}
