//! Text, span, and tool formatting utilities for the TUI.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use corex_core::types::ToolCall;
use crate::theme::Theme;

pub fn find_url_in_line(line_text: &str, click_col: Option<usize>) -> Option<String> {
    use unicode_width::UnicodeWidthChar;

    let mut urls = Vec::new();
    let mut start = 0;
    while start < line_text.len() {
        let remainder = &line_text[start..];
        let pos = remainder.find("https://").or_else(|| remainder.find("http://"));
        let p = match pos {
            Some(idx) => idx,
            None => break,
        };
        let abs_byte_start = start + p;
        let url_rem = &line_text[abs_byte_start..];
        let end = url_rem
            .find(|c: char| c.is_whitespace() || c == ')' || c == ']' || c == '>' || c == '"' || c == '\'' || c == '│' || c == '|' || c == '}' || c == '<')
            .unwrap_or(url_rem.len());
        let mut url = url_rem[..end].to_string();
        while url.ends_with('.') || url.ends_with(',') || url.ends_with(';') {
            url.pop();
        }
        let abs_byte_end = abs_byte_start + url.len();

        // Check if this URL is part of a markdown link: [label](url)
        let mut abs_token_start = abs_byte_start;
        let mut abs_token_end = abs_byte_end;
        if abs_byte_start >= 2 && line_text[..abs_byte_start].ends_with("](") {
            let before = &line_text[..abs_byte_start - 2];
            if let Some(bracket_idx) = before.rfind('[') {
                abs_token_start = bracket_idx;
                if line_text[abs_byte_end..].starts_with(')') {
                    abs_token_end = abs_byte_end + 1;
                }
            }
        }

        if !url.is_empty() {
            // Compute exact display columns (visual character cells) for this link token
            let mut col = 0usize;
            let mut start_col = None;
            let mut end_col = None;
            for (byte_idx, ch) in line_text.char_indices() {
                let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if byte_idx == abs_token_start {
                    start_col = Some(col);
                }
                if byte_idx + ch.len_utf8() == abs_token_end {
                    end_col = Some(col + w);
                }
                col += w;
            }
            if let (Some(sc), Some(ec)) = (start_col, end_col) {
                urls.push((sc, ec, url));
            }
        }
        let step = if end == 0 {
            url_rem.chars().next().map(|c| c.len_utf8()).unwrap_or(1)
        } else {
            end
        };
        start = abs_byte_start + step;
    }

    if urls.is_empty() {
        return None;
    }

    if let Some(col) = click_col {
        for (sc, ec, u) in &urls {
            // Only match if the click is strictly on the visual text of the URL
            if col >= *sc && col < *ec {
                return Some(u.clone());
            }
        }
        // Clicked elsewhere on the line -> never open any URL
        return None;
    }

    urls.first().map(|(_, _, u)| u.clone())
}

pub fn extract_tool_allow_rule(call: &ToolCall) -> String {
    if call.function.name == "run_shell_command"
        || call.function.name == "shell"
        || call.function.name == "run_command"
        || call.function.name == "fleet_run_command"
        || call.function.name == "fleet_broadcast_command"
    {
        if let Ok(args_json) = serde_json::from_str::<serde_json::Value>(&call.function.arguments) {
            if let Some(cmd) = args_json.get("command").or_else(|| args_json.get("CommandLine")).and_then(|c| c.as_str()) {
                let trimmed = cmd.trim();
                let has_operators = trimmed.chars().any(|c| matches!(c, ';' | '|' | '&' | '>' | '<' | '$' | '`' | '\n'));
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                let bin = parts
                    .first()
                    .and_then(|p| p.rsplit('/').next())
                    .unwrap_or("");
                let is_high_risk_bin = matches!(
                    bin,
                    "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish"
                        | "python" | "python3" | "node" | "perl" | "ruby" | "php" | "lua"
                        | "env" | "sudo" | "su" | "doas" | "eval" | "exec"
                        | "rm" | "find" | "xargs" | "curl" | "wget" | "ssh" | "scp"
                        | "dd" | "kill" | "pkill" | "chmod" | "chown"
                );
                if has_operators || is_high_risk_bin {
                    trimmed.to_string()
                } else if parts.len() >= 2 && !parts[1].starts_with('-') {
                    format!("{} {}", parts[0], parts[1])
                } else if !parts.is_empty() {
                    parts[0].to_string()
                } else {
                    trimmed.to_string()
                }
            } else {
                call.function.name.clone()
            }
        } else {
            call.function.name.clone()
        }
    } else if call.function.name == "web_fetch" {
        // Scope the approval to the host, never to every URL the model may come up with.
        serde_json::from_str::<serde_json::Value>(&call.function.arguments)
            .ok()
            .and_then(|args| corex_tools::web::web_fetch_allow_rule(&args))
            .unwrap_or_default()
    } else {
        call.function.name.clone()
    }
}

/// Extracts all applicable allow rules for a tool call.
/// For code modifications (`edit`, `apply_patch`, `write_file`, `replace`), this returns all
/// file-modification tool names so allowing one allows code modifications consistently for the session.
pub fn extract_tool_allow_rules(call: &ToolCall) -> Vec<String> {
    match call.function.name.as_str() {
        "apply_patch" | "edit" | "write_file" | "replace" => {
            vec![
                "apply_patch".to_string(),
                "edit".to_string(),
                "write_file".to_string(),
                "replace".to_string(),
                "edit_file".to_string(),
            ]
        }
        _ => {
            let single = extract_tool_allow_rule(call);
            if single.is_empty() {
                Vec::new()
            } else {
                vec![single]
            }
        }
    }
}

pub fn format_tool_call_summary(name: &str, raw_args: &str) -> String {
    let args_val = serde_json::from_str::<serde_json::Value>(raw_args).ok();
    match name {
        "fleet_run_command" => {
            let cmd = args_val.as_ref()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()))
                .unwrap_or(raw_args)
                .trim();
            let node = args_val.as_ref()
                .and_then(|v| v.get("node_id").and_then(|n| n.as_str()))
                .unwrap_or("");
            if !node.is_empty() {
                format!("[{}] $ {}", node, corex_core::truncate_ellipsis(cmd, 45))
            } else {
                format!("$ {}", corex_core::truncate_ellipsis(cmd, 50))
            }
        }
        "run_shell_command" | "shell" | "run_command" => {
            let cmd = args_val.as_ref()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()))
                .unwrap_or(raw_args)
                .trim();
            if cmd.is_empty() {
                "Running command...".to_string()
            } else {
                format!("Running command: {}", corex_core::truncate_ellipsis(cmd, 50))
            }
        }
        "read_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            format!("Reading file: {}", path)
        }
        "write_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            format!("Writing file: {}", path)
        }
        "edit" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            format!("Editing file: {}", path)
        }
        "apply_patch" => {
            // The diff names its own target, so a call without `file_path` still says which file it
            // touches instead of echoing the whole patch.
            let target = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .map(str::to_string)
                .or_else(|| {
                    args_val.as_ref()
                        .and_then(|v| v.get("patch").and_then(|p| p.as_str()))
                        .and_then(patch_target)
                })
                .unwrap_or_else(|| "patch".to_string());
            format!("Editing file: {}", target)
        }
        "list_directory" | "list_dir" | "glob" | "ls" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("dir_path").or_else(|| v.get("path")).or_else(|| v.get("dir")).and_then(|p| p.as_str()))
                .unwrap_or(".");
            format!("Exploring directory: {}", path)
        }
        "web_search" => {
            let q = args_val.as_ref()
                .and_then(|v| v.get("query").and_then(|c| c.as_str()))
                .unwrap_or(raw_args);
            format!("Searching web: {}", corex_core::truncate_ellipsis(q, 50))
        }
        "web_fetch" => {
            let url = args_val.as_ref()
                .and_then(|v| v.get("url").and_then(|c| c.as_str()))
                .unwrap_or(raw_args);
            format!("Fetching URL: {}", corex_core::truncate_ellipsis(url, 50))
        }
        "grep_search" | "grep" => {
            let q = args_val.as_ref()
                .and_then(|v| v.get("pattern").or_else(|| v.get("query")).and_then(|c| c.as_str()))
                .unwrap_or(raw_args);
            format!("Searching code: {}", corex_core::truncate_ellipsis(q, 50))
        }
        "activate_skill" => {
            let skill_name = args_val.as_ref()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()))
                .unwrap_or(raw_args);
            format!("Loading skill: {}", skill_name)
        }
        _ => format!("Executing: {}", name),
    }
}

pub fn clean_tool_args(name: &str, raw_args: &str) -> String {
    let args_val = serde_json::from_str::<serde_json::Value>(raw_args).ok();
    if let Some(val) = &args_val {
        if let Some(obj) = val.as_object() {
            match name {
                "web_search" => {
                    if let Some(q) = obj.get("query").and_then(|v| v.as_str()) {
                        return corex_core::truncate_ellipsis(q, 60).to_string();
                    }
                }
                "web_fetch" => {
                    if let Some(u) = obj.get("url").and_then(|v| v.as_str()) {
                        return corex_core::truncate_ellipsis(u, 60).to_string();
                    }
                }
                "grep_search" | "grep" => {
                    let q = obj.get("pattern").or_else(|| obj.get("query")).and_then(|v| v.as_str()).unwrap_or("");
                    let path = obj.get("path").or_else(|| obj.get("dir_path")).and_then(|v| v.as_str());
                    if let Some(p) = path {
                        if !p.is_empty() && p != "." {
                            let rel = format_relative_path(p);
                            if q.is_empty() {
                                return format!("in {}", rel);
                            } else {
                                return format!("{} in {}", q, rel);
                            }
                        }
                    }
                    if !q.is_empty() {
                        return q.to_string();
                    }
                }
                "glob" => {
                    if let Some(p) = obj.get("pattern").and_then(|v| v.as_str()) {
                        return p.to_string();
                    }
                }
                "delegate_subagent" => {
                    let role = obj.get("role").and_then(|v| v.as_str()).unwrap_or("worker");
                    let task = obj.get("prompt")
                        .or_else(|| obj.get("task"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    let first_line = task.lines().next().unwrap_or(task);
                    if first_line.is_empty() {
                        return role.to_string();
                    } else {
                        return format!("{}: {}", role, corex_core::truncate_ellipsis(first_line, 40));
                    }
                }
                _ => {}
            }

            for key in ["query", "url", "command", "pattern", "path", "file_path", "name", "task", "prompt"] {
                if let Some(v) = obj.get(key) {
                    if let Some(s) = v.as_str() {
                        return corex_core::truncate_ellipsis(s, 60).to_string();
                    }
                }
            }
            if obj.len() == 1 {
                if let Some((_, v)) = obj.iter().next() {
                    if let Some(s) = v.as_str() {
                        return corex_core::truncate_ellipsis(s, 60).to_string();
                    } else {
                        return corex_core::truncate_ellipsis(&v.to_string(), 60).to_string();
                    }
                }
            }
            let formatted_pairs: Vec<String> = obj.iter()
                .map(|(k, v)| {
                    if let Some(s) = v.as_str() {
                        format!("{}={}", k, corex_core::truncate_ellipsis(s, 30))
                    } else {
                        format!("{}={}", k, v)
                    }
                })
                .collect();
            if !formatted_pairs.is_empty() {
                return corex_core::truncate_ellipsis(&formatted_pairs.join(", "), 60).to_string();
            }
        }
    }
    corex_core::truncate_ellipsis(raw_args, 60).to_string()
}

/// Target file of an `apply_patch` call.
pub fn patch_target(patch: &str) -> Option<String> {
    patch
        .lines()
        .filter(|line| line.starts_with("+++ ") || line.starts_with("--- "))
        .filter_map(|line| {
            let path = line[4..].split('\t').next().unwrap_or("").trim();
            if path.is_empty() || path == "/dev/null" {
                return None;
            }
            let path = path.strip_prefix("b/").or_else(|| path.strip_prefix("a/")).unwrap_or(path);
            Some(path.to_string())
        })
        .next()
}

/// Helper to trim current working directory or home directory prefix from a path
pub fn format_relative_path(path_str: &str) -> String {
    let clean = path_str.trim();
    if clean.is_empty() {
        return ".".to_string();
    }
    let p = std::path::Path::new(clean);
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(rel) = p.strip_prefix(&cwd) {
            let rel_str = rel.to_string_lossy();
            if rel_str.is_empty() {
                return ".".to_string();
            }
            return rel_str.to_string();
        }
    }
    if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.to_string_lossy());
        }
    }
    clean.to_string()
}

pub fn format_tool_call_spans(name: &str, raw_args: &str, theme: &Theme) -> Vec<Span<'static>> {
    let args_val = serde_json::from_str::<serde_json::Value>(raw_args).ok();
    match name {
        "run_shell_command" | "shell" | "run_command" => {
            let cmd = args_val.as_ref()
                .and_then(|v| v.get("command").or_else(|| v.get("CommandLine")).and_then(|c| c.as_str()))
                .unwrap_or(raw_args)
                .trim();
            vec![
                Span::styled("$ ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(corex_core::truncate_ellipsis(cmd, 70).to_string(), Style::default().fg(theme.gray)),
            ]
        }
        "read_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            let rel = format_relative_path(path);
            vec![
                Span::styled("read ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(rel, Style::default().fg(theme.gray)),
            ]
        }
        "write_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            let rel = format_relative_path(path);
            vec![
                Span::styled("write ", Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
                Span::styled(rel, Style::default().fg(theme.gray)),
            ]
        }
        "edit" | "apply_patch" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .map(str::to_string)
                .or_else(|| {
                    args_val.as_ref()
                        .and_then(|v| v.get("patch").and_then(|p| p.as_str()))
                        .and_then(patch_target)
                })
                .unwrap_or_else(|| "patch".to_string());
            let rel = format_relative_path(&path);
            vec![
                Span::styled("edit ", Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
                Span::styled(rel, Style::default().fg(theme.gray)),
            ]
        }
        "list_directory" | "ls" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("path").or_else(|| v.get("dir_path")).and_then(|p| p.as_str()))
                .unwrap_or(".");
            let mut rel = format_relative_path(path);
            if !rel.ends_with('/') && rel != "." {
                rel.push('/');
            }
            vec![
                Span::styled("ls ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(rel, Style::default().fg(theme.gray)),
            ]
        }
        "glob" => {
            let pattern = args_val.as_ref()
                .and_then(|v| v.get("pattern").and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("glob ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(pattern.to_string(), Style::default().fg(theme.gray)),
            ]
        }
        "web_search" => {
            let q = args_val.as_ref()
                .and_then(|v| v.get("query").and_then(|c| c.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("web ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                Span::styled(format!("\"{}\"", corex_core::truncate_ellipsis(q, 60)), Style::default().fg(theme.gray)),
            ]
        }
        "web_fetch" => {
            let url = args_val.as_ref()
                .and_then(|v| v.get("url").and_then(|c| c.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("fetch ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                Span::styled(corex_core::truncate_ellipsis(url, 60).to_string(), Style::default().fg(theme.gray)),
            ]
        }
        "grep_search" | "grep" => {
            let q = args_val.as_ref()
                .and_then(|v| v.get("pattern").or_else(|| v.get("query")).and_then(|c| c.as_str()))
                .unwrap_or("");
            let path = args_val.as_ref()
                .and_then(|v| v.get("path").or_else(|| v.get("dir_path")).and_then(|c| c.as_str()))
                .filter(|p| !p.is_empty() && *p != ".");
            let text = if let Some(p) = path {
                let rel = format_relative_path(p);
                if q.is_empty() {
                    format!("in {}", rel)
                } else {
                    format!("\"{}\" in {}", q, rel)
                }
            } else if !q.is_empty() {
                format!("\"{}\"", q)
            } else {
                raw_args.to_string()
            };
            vec![
                Span::styled("grep ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(corex_core::truncate_ellipsis(&text, 60).to_string(), Style::default().fg(theme.gray)),
            ]
        }
        "activate_skill" => {
            let skill_name = args_val.as_ref()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("skill ", Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
                Span::styled(skill_name.to_string(), Style::default().fg(theme.gray)),
            ]
        }
        "delegate_subagent" => {
            let role = args_val.as_ref().and_then(|v| v.get("role").and_then(|r| r.as_str())).unwrap_or("worker");
            let prompt = args_val.as_ref()
                .and_then(|v| v.get("prompt").or_else(|| v.get("task")).and_then(|t| t.as_str()))
                .unwrap_or("")
                .trim();
            let first_line = prompt.lines().next().unwrap_or(prompt);
            let display_desc = if first_line.is_empty() {
                role.to_string()
            } else {
                format!("{} {}", role, corex_core::truncate_ellipsis(first_line, 50))
            };
            vec![
                Span::styled("agent ", Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
                Span::styled(display_desc, Style::default().fg(theme.gray)),
            ]
        }
        _ => {
            let clean = clean_tool_args(name, raw_args);
            vec![
                Span::styled(format!("{} ", name), Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                Span::styled(clean, Style::default().fg(theme.gray)),
            ]
        }
    }
}

/// Prefixes of the tags the composer shows in place of pasted content (text or an image).
const PASTE_TAG_PREFIXES: [&str; 2] = ["[Pasted text #", "[Image #"];

/// Start of the first paste tag in `text`.
pub fn paste_tag_start(text: &str) -> Option<usize> {
    PASTE_TAG_PREFIXES.iter().filter_map(|p| text.find(p)).min()
}

/// Start of the last paste tag in `text`.
pub fn paste_tag_last_start(text: &str) -> Option<usize> {
    PASTE_TAG_PREFIXES.iter().filter_map(|p| text.rfind(p)).max()
}

/// True when `text` begins with a paste tag.
pub fn starts_with_paste_tag(text: &str) -> bool {
    PASTE_TAG_PREFIXES.iter().any(|p| text.starts_with(p))
}

/// Ids of the pasted images `text` refers to (`[Image #3]`), in the order they appear.
pub fn image_tag_ids(text: &str) -> Vec<usize> {
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("[Image #") {
        let after = &rest[at + "[Image #".len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if after[digits.len()..].starts_with(']') {
            if let Ok(id) = digits.parse() {
                ids.push(id);
            }
        }
        rest = after;
    }
    ids
}

pub fn format_input_spans<'a>(text: &'a str, theme: &'a Theme) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(start_idx) = paste_tag_start(rest) {
        if start_idx > 0 {
            spans.push(Span::raw(&rest[..start_idx]));
        }
        let after_start = &rest[start_idx..];
        if let Some(end_bracket) = after_start.find(']') {
            let tag = &after_start[..=end_bracket];
            spans.push(Span::styled(
                tag,
                Style::default()
                    .fg(theme.accent_cyan)
                    .bg(Color::Rgb(25, 45, 65))
                    .add_modifier(Modifier::BOLD),
            ));
            rest = &after_start[end_bracket + 1..];
        } else {
            spans.push(Span::raw(after_start));
            rest = "";
            break;
        }
    }
    if !rest.is_empty() {
        spans.push(Span::raw(rest));
    }
    spans
}

pub fn format_input_with_cursor<'a>(
    text: &'a str,
    cursor_idx: usize,
    theme: &'a Theme,
) -> (Vec<Span<'a>>, usize) {
    let mut clamped = cursor_idx.min(text.len());
    while !text.is_char_boundary(clamped) {
        clamped = clamped.saturating_sub(1);
    }
    let mut spans = Vec::new();

    if text.is_empty() {
        spans.push(Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_blue)));
        return (spans, 0);
    }

    if clamped >= text.len() {
        spans.extend(format_input_spans(text, theme));
        let cursor_col: usize = spans.iter().map(|s| s.width()).sum();
        spans.push(Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_blue)));
        return (spans, cursor_col);
    }

    let before = &text[..clamped];
    let mut before_spans = format_input_spans(before, theme);
    let cursor_col: usize = before_spans.iter().map(|s| s.width()).sum();

    if starts_with_paste_tag(&text[clamped..]) {
        if let Some(end_bracket) = text[clamped..].find(']') {
            let tag = &text[clamped..=clamped + end_bracket];
            let after = &text[clamped + end_bracket + 1..];
            spans.append(&mut before_spans);
            spans.push(Span::styled(
                tag,
                Style::default()
                    .fg(Color::Rgb(20, 20, 20))
                    .bg(theme.accent_blue)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.extend(format_input_spans(after, theme));
            return (spans, cursor_col);
        }
    }

    let c = text[clamped..].chars().next().unwrap_or(' ');
    let c_len = c.len_utf8();
    let char_str = &text[clamped..clamped + c_len];
    let after = &text[clamped + c_len..];

    spans.append(&mut before_spans);
    spans.push(Span::styled(
        char_str,
        Style::default()
            .fg(Color::Rgb(20, 20, 20))
            .bg(theme.accent_blue)
            .add_modifier(Modifier::BOLD),
    ));
    spans.extend(format_input_spans(after, theme));

    (spans, cursor_col)
}

/// Hard-wraps the composer's styled spans to `width` columns.
pub fn wrap_composer_spans<'a>(
    spans: Vec<Span<'a>>,
    width: usize,
    indent: usize,
    cursor_flat: usize,
) -> (Vec<Line<'a>>, usize) {
    let width = width.max(1);
    let mut lines: Vec<Line<'a>> = Vec::new();
    let mut current: Vec<Span<'a>> = Vec::new();
    let mut current_len = 0usize;
    let mut flat = 0usize;
    let mut cursor_row = 0usize;
    let indent_str = if indent > 0 { " ".repeat(indent) } else { String::new() };

    for span in spans {
        let style = span.style;
        let content = span.content.as_ref();
        let mut seg_start = 0usize;

        for (byte_idx, ch) in content.char_indices() {
            if ch == '\n' {
                if byte_idx > seg_start {
                    current.push(Span::styled(content[seg_start..byte_idx].to_string(), style));
                }
                lines.push(Line::from(std::mem::take(&mut current)));
                current_len = 0;
                if indent > 0 {
                    current.push(Span::raw(indent_str.clone()));
                    current_len = indent;
                }
                flat += 1;
                seg_start = byte_idx + ch.len_utf8();
                continue;
            }

            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if current_len + ch_width > width && current_len > 0 {
                if byte_idx > seg_start {
                    current.push(Span::styled(content[seg_start..byte_idx].to_string(), style));
                }
                lines.push(Line::from(std::mem::take(&mut current)));
                current_len = 0;
                if indent > 0 {
                    current.push(Span::raw(indent_str.clone()));
                    current_len = indent;
                }
                seg_start = byte_idx;
            }

            if flat == cursor_flat {
                cursor_row = lines.len();
            }

            current_len += ch_width;
            flat += 1;
        }

        if seg_start < content.len() {
            current.push(Span::styled(content[seg_start..].to_string(), style));
        }
    }

    lines.push(Line::from(current));
    (lines, cursor_row)
}

/// Plain text of a rendered chat line, with no styling attached.
pub fn line_plain_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

/// Char range of `text` covered by the inclusive display-column interval `from..=to`.
pub fn char_range_for_display_columns(text: &str, from: u16, to: u16) -> (usize, usize) {
    use unicode_width::UnicodeWidthChar;

    let (from, to) = (from as u32, to as u32);
    let mut column = 0u32;
    let mut first: Option<usize> = None;
    let mut last: Option<usize> = None;

    for (idx, ch) in text.chars().enumerate() {
        let width = ch.width().unwrap_or(0) as u32;
        let cell_start = column;
        let cell_end = column + width;
        column = cell_end;

        if width == 0 {
            if first.is_some() && cell_start <= to {
                last = Some(idx);
            }
            continue;
        }

        if first.is_none() && cell_end > from {
            first = Some(idx);
        }
        if cell_start <= to {
            last = Some(idx);
        } else {
            break;
        }
    }

    let first = first.unwrap_or_else(|| text.chars().count());
    let last = last.map(|index| index + 1).unwrap_or(first).max(first);
    (first, last)
}

/// Cuts `text` to at most `max_cols` terminal columns, ending in `...` when something was dropped.
/// Counting columns rather than bytes keeps accented or wide text from being cut early or late.
pub fn truncate_to_width(text: &str, max_cols: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    if UnicodeWidthStr::width(text) <= max_cols {
        return text.to_string();
    }
    let budget = max_cols.saturating_sub(3);
    let mut used = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(ch);
    }
    out.push_str("...");
    out
}
