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
        if !url.is_empty() {
            // Compute exact display columns (visual character cells) for this URL
            let mut col = 0usize;
            let mut start_col = None;
            let mut end_col = None;
            for (byte_idx, ch) in line_text.char_indices() {
                let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if byte_idx == abs_byte_start {
                    start_col = Some(col);
                }
                if byte_idx + ch.len_utf8() == abs_byte_end {
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
    if call.function.name == "run_shell_command" || call.function.name == "shell" || call.function.name == "run_command" {
        if let Ok(args_json) = serde_json::from_str::<serde_json::Value>(&call.function.arguments) {
            if let Some(cmd) = args_json.get("command").or_else(|| args_json.get("CommandLine")).and_then(|c| c.as_str()) {
                let trimmed = cmd.trim();
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 && !parts[1].starts_with('-') {
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
    } else {
        call.function.name.clone()
    }
}

pub fn format_tool_call_summary(name: &str, raw_args: &str) -> String {
    let args_val = serde_json::from_str::<serde_json::Value>(raw_args).ok();
    match name {
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
        "activate_skill" => {
            let skill_name = args_val.as_ref()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()))
                .unwrap_or(raw_args);
            format!("Loading skill: {}", skill_name)
        }
        _ => format!("Executing: {}", name),
    }
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

pub fn format_tool_call_spans(name: &str, raw_args: &str, theme: &Theme) -> Vec<Span<'static>> {
    let args_val = serde_json::from_str::<serde_json::Value>(raw_args).ok();
    match name {
        "run_shell_command" | "shell" => {
            let cmd = args_val.as_ref()
                .and_then(|v| v.get("command").or_else(|| v.get("CommandLine")).and_then(|c| c.as_str()))
                .unwrap_or(raw_args)
                .trim();
            vec![
                Span::styled("› ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(name.to_string(), Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({})", cmd), Style::default().fg(theme.dark_gray)),
            ]
        }
        "read_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("› ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(name.to_string(), Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({})", path), Style::default().fg(theme.dark_gray)),
            ]
        }
        "write_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("› ", Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
                Span::styled(name.to_string(), Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({})", path), Style::default().fg(theme.dark_gray)),
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
            vec![
                Span::styled("› ", Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
                Span::styled(name.to_string(), Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({})", path), Style::default().fg(theme.dark_gray)),
            ]
        }
        "activate_skill" => {
            let skill_name = args_val.as_ref()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("› ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(name.to_string(), Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({})", skill_name), Style::default().fg(theme.dark_gray)),
            ]
        }
        _ => {
            let display_args = corex_core::truncate_ellipsis(raw_args, 40);
            vec![
                Span::styled("› ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                Span::styled(name.to_string(), Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({})", display_args), Style::default().fg(theme.dark_gray)),
            ]
        }
    }
}

pub fn format_input_spans<'a>(text: &'a str, theme: &'a Theme) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(start_idx) = rest.find("[Pasted text #") {
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
        spans.push(Span::styled("█", Style::default().fg(theme.accent_blue)));
        return (spans, 0);
    }

    if clamped >= text.len() {
        spans.extend(format_input_spans(text, theme));
        let cursor_col: usize = spans.iter().map(|s| s.width()).sum();
        spans.push(Span::styled("█", Style::default().fg(theme.accent_blue)));
        return (spans, cursor_col);
    }

    let before = &text[..clamped];
    let mut before_spans = format_input_spans(before, theme);
    let cursor_col: usize = before_spans.iter().map(|s| s.width()).sum();

    if text[clamped..].starts_with("[Pasted text #") {
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

    for span in spans {
        let style = span.style;
        for ch in span.content.chars() {
            if ch == '\n' {
                lines.push(Line::from(std::mem::take(&mut current)));
                current_len = 0;
                if indent > 0 {
                    current.push(Span::raw(" ".repeat(indent)));
                    current_len = indent;
                }
                flat += 1;
                continue;
            }

            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if current_len + ch_width > width && current_len > 0 {
                lines.push(Line::from(std::mem::take(&mut current)));
                current_len = 0;
                if indent > 0 {
                    current.push(Span::raw(" ".repeat(indent)));
                    current_len = indent;
                }
            }

            if flat == cursor_flat {
                cursor_row = lines.len();
            }

            current.push(Span::styled(ch.to_string(), style));
            current_len += ch_width;
            flat += 1;
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
