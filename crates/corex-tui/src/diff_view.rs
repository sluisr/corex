use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Formats a single content row enclosed within rounded box borders with 100% width precision.
#[allow(clippy::too_many_arguments)]
fn format_boxed_row(
    prefix: &str,
    prefix_style: Style,
    content: &str,
    content_style: Style,
    cursor: Option<Span<'static>>,
    inner_width: usize,
    left_border_color: Color,
    right_border_color: Color,
) -> Line<'static> {
    let cursor_len = if cursor.is_some() { 1 } else { 0 };
    let prefix_len = unicode_width::UnicodeWidthStr::width(prefix);
    let avail_for_content = inner_width.saturating_sub(prefix_len + cursor_len);

    let (display_content, content_len) = if unicode_width::UnicodeWidthStr::width(content) > avail_for_content {
        let mut truncated = String::new();
        let mut current_width = 0;
        let target_width = avail_for_content.saturating_sub(3);
        for c in content.chars() {
            let char_w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if current_width + char_w > target_width {
                break;
            }
            truncated.push(c);
            current_width += char_w;
        }
        truncated.push_str("...");
        let len = unicode_width::UnicodeWidthStr::width(truncated.as_str());
        (truncated, len)
    } else {
        let len = unicode_width::UnicodeWidthStr::width(content);
        (content.to_string(), len)
    };

    let total_used = prefix_len + content_len + cursor_len;
    let pad_len = inner_width.saturating_sub(total_used);
    let padding = " ".repeat(pad_len);

    let mut spans = vec![
        Span::styled("  │ ", Style::default().fg(left_border_color)),
        Span::styled(prefix.to_string(), prefix_style),
        Span::styled(display_content, content_style),
    ];
    if let Some(c) = cursor {
        spans.push(c);
    }
    spans.push(Span::raw(padding));
    spans.push(Span::styled(" │", Style::default().fg(right_border_color)));

    Line::from(spans)
}

/// Formats a content row, wrapping across multiple boxed lines if it exceeds the available width,
/// ensuring that commands and parameters are never truncated with '...'.
fn format_boxed_wrapped(
    prefix: &str,
    prefix_style: Style,
    content: &str,
    content_style: Style,
    inner_width: usize,
    left_border_color: Color,
    right_border_color: Color,
) -> Vec<Line<'static>> {
    let prefix_w = unicode_width::UnicodeWidthStr::width(prefix);
    let avail_first = inner_width.saturating_sub(prefix_w);
    if avail_first == 0 || content.is_empty() {
        return vec![format_boxed_row(
            prefix,
            prefix_style,
            content,
            content_style,
            None,
            inner_width,
            left_border_color,
            right_border_color,
        )];
    }

    if unicode_width::UnicodeWidthStr::width(content) <= avail_first {
        return vec![format_boxed_row(
            prefix,
            prefix_style,
            content,
            content_style,
            None,
            inner_width,
            left_border_color,
            right_border_color,
        )];
    }

    let continuation_prefix = " ".repeat(prefix_w);
    let mut lines = Vec::new();
    let mut remaining = content;
    let mut is_first = true;

    while !remaining.is_empty() {
        let current_prefix = if is_first { prefix } else { &continuation_prefix };
        let current_style = if is_first { prefix_style } else { Style::default() };
        let current_avail = inner_width.saturating_sub(unicode_width::UnicodeWidthStr::width(current_prefix));

        if current_avail == 0 {
            break;
        }

        if unicode_width::UnicodeWidthStr::width(remaining) <= current_avail {
            let chunk_w = unicode_width::UnicodeWidthStr::width(remaining);
            let pad = inner_width.saturating_sub(unicode_width::UnicodeWidthStr::width(current_prefix) + chunk_w);
            let spans = vec![
                Span::styled("  │ ", Style::default().fg(left_border_color)),
                Span::styled(current_prefix.to_string(), current_style),
                Span::styled(remaining.to_string(), content_style),
                Span::raw(" ".repeat(pad)),
                Span::styled(" │", Style::default().fg(right_border_color)),
            ];
            lines.push(Line::from(spans));
            break;
        }

        // Search for the best split point within `current_avail` width
        let mut take_bytes = 0;
        let mut cur_w = 0;
        let mut last_space_bytes = None;

        for (idx, ch) in remaining.char_indices() {
            let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if cur_w + ch_w > current_avail {
                break;
            }
            cur_w += ch_w;
            take_bytes = idx + ch.len_utf8();
            if ch.is_whitespace() {
                last_space_bytes = Some(take_bytes);
            }
        }

        let split_at = if let Some(sp) = last_space_bytes {
            if sp > current_avail / 3 {
                sp
            } else {
                take_bytes
            }
        } else {
            take_bytes
        };

        if split_at == 0 {
            break;
        }

        let chunk = remaining[..split_at].trim_end();
        remaining = remaining[split_at..].trim_start();
        let chunk_w = unicode_width::UnicodeWidthStr::width(chunk);
        let pad = inner_width.saturating_sub(unicode_width::UnicodeWidthStr::width(current_prefix) + chunk_w);
        let spans = vec![
            Span::styled("  │ ", Style::default().fg(left_border_color)),
            Span::styled(current_prefix.to_string(), current_style),
            Span::styled(chunk.to_string(), content_style),
            Span::raw(" ".repeat(pad)),
            Span::styled(" │", Style::default().fg(right_border_color)),
        ];
        lines.push(Line::from(spans));
        is_first = false;
    }

    if lines.is_empty() {
        lines.push(format_boxed_row(
            prefix,
            prefix_style,
            content,
            content_style,
            None,
            inner_width,
            left_border_color,
            right_border_color,
        ));
    }

    lines
}

#[allow(clippy::too_many_arguments)]
pub fn build_tool_confirmation_lines(
    tool_name: &str,
    diff_or_cmd: &str,
    selected_option: usize,
    batch_count: usize,
    current_idx: usize,
    total_count: usize,
    max_width: usize,
    theme: &Theme,
    expanded: bool,
    input_mode: bool,
    feedback_text: &str,
) -> Vec<Line<'static>> {
    let is_shell = tool_name == "run_shell_command" || tool_name == "shell";
    let is_edit = tool_name == "replace" || tool_name == "edit" || tool_name == "write_file" || tool_name == "apply_patch";
    let is_subagent = tool_name == "delegate_subagent";

    let border_color = theme.border_dim;
    let box_width = max_width.clamp(30, 105);
    let inner_width = box_width.saturating_sub(6);

    let mut result = Vec::new();

    // 1. Top border
    let top_border = format!("  ╭{}╮", "─".repeat(inner_width + 2));
    result.push(Line::from(Span::styled(top_border, Style::default().fg(border_color))));

    // 2. Body lines (truncated to 12 unless the user expands with Ctrl+O)
    let all_diff_lines: Vec<&str> = diff_or_cmd.lines().collect();
    let total_diff_lines = all_diff_lines.len();
    let shown_diff_lines: Vec<&str> = if expanded {
        all_diff_lines
    } else {
        all_diff_lines.into_iter().take(12).collect()
    };
    if shown_diff_lines.is_empty() {
        let (prefix, body, color) = if is_subagent {
            ("◆ ", "(subagent delegation)", theme.accent_cyan)
        } else if is_shell {
            ("$ ", "(no arguments provided)", theme.accent_yellow)
        } else {
            ("⚙ ", "(no arguments provided)", theme.accent_purple)
        };
        result.push(format_boxed_row(
            prefix,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
            body,
            Style::default().fg(theme.gray),
            None,
            inner_width,
            border_color,
            border_color,
        ));
    } else {
        for line in shown_diff_lines {
        if is_edit {
            if line.starts_with('+') && !line.starts_with("+++") {
                result.push(format_boxed_row(
                    "+ ",
                    Style::default().fg(theme.diff_added_fg).add_modifier(Modifier::BOLD),
                    &line[1..],
                    Style::default().fg(theme.diff_added_fg),
                    None,
                    inner_width,
                    border_color,
                    border_color,
                ));
            } else if line.starts_with('-') && !line.starts_with("---") {
                result.push(format_boxed_row(
                    "- ",
                    Style::default().fg(theme.diff_removed_fg).add_modifier(Modifier::BOLD),
                    &line[1..],
                    Style::default().fg(theme.diff_removed_fg),
                    None,
                    inner_width,
                    border_color,
                    border_color,
                ));
            } else if line.starts_with("@@") {
                result.push(format_boxed_row(
                    "",
                    Style::default(),
                    line,
                    Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD),
                    None,
                    inner_width,
                    border_color,
                    border_color,
                ));
            } else if line.starts_with("---") || line.starts_with("+++") {
                result.push(format_boxed_row(
                    "",
                    Style::default(),
                    line,
                    Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD),
                    None,
                    inner_width,
                    border_color,
                    border_color,
                ));
            } else {
                result.push(format_boxed_row(
                    "  ",
                    Style::default(),
                    line,
                    Style::default().fg(theme.foreground),
                    None,
                    inner_width,
                    border_color,
                    border_color,
                ));
            }
        } else {
            let is_sub = is_subagent || line.starts_with("Subagent");
            let (prefix, prefix_style, body) = if is_sub {
                ("◆ ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD), line.trim_start())
            } else if is_shell || line.trim_start().starts_with('$') {
                let b = if line.trim_start().starts_with('$') {
                    line.trim_start()[1..].trim_start()
                } else {
                    line.trim_start()
                };
                ("$ ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD), b)
            } else {
                ("› ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD), line.trim_start())
            };

            result.extend(format_boxed_wrapped(
                prefix,
                prefix_style,
                body,
                Style::default().fg(theme.text_strong),
                inner_width,
                border_color,
                border_color,
            ));
        }
    }
    }

    // 3. Bottom border
    let bottom_border = format!("  ╰{}╯", "─".repeat(inner_width + 2));
    result.push(Line::from(Span::styled(bottom_border, Style::default().fg(border_color))));

    // 3.5 Hint when the diff was truncated (or is fully expanded)
    if total_diff_lines > 12 {
        let hint = if expanded {
            format!("(Ctrl+O to collapse — {} lines)", total_diff_lines)
        } else {
            format!("({} more lines — Ctrl+O to view all)", total_diff_lines - 12)
        };
        result.push(Line::from(Span::styled(
            format!("  {}", hint),
            Style::default().fg(theme.gray),
        )));
    }

    // 4. Question line
    let step_label = if total_count > 1 {
        format!("[{} of {}] ", current_idx, total_count)
    } else {
        String::new()
    };

    let question_line = if is_subagent {
        let label = format!("{}Subagent", step_label);
        Line::from(vec![
            Span::raw("  Allow execution of "),
            Span::styled(label, Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            Span::raw("?"),
        ])
    } else if is_shell {
        let label = format!("{}Shell Command", step_label);
        Line::from(vec![
            Span::raw("  Allow execution of "),
            Span::styled(label, Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
            Span::raw("?"),
        ])
    } else if is_edit {
        let label = if total_count > 1 {
            format!("Apply change [{} of {}]?", current_idx, total_count)
        } else {
            "Apply this change?".to_string()
        };
        Line::from(vec![
            Span::raw("  "),
            Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
        ])
    } else {
        let label = format!("{}{}", step_label, tool_name);
        Line::from(vec![
            Span::raw("  Allow execution of "),
            Span::styled(label, Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            Span::raw("?"),
        ])
    };
    result.push(question_line);

    // 5. Options (Modern CLI style without emojis)
    let remaining_all_str = format!("Allow all remaining ({})", batch_count);
    let options: [(&str, &str); 5] = if total_count > 1 {
        [
            ("Allow this change", "[y]"),
            (&remaining_all_str, "[a]"),
            ("Allow for this session", "[s]"),
            ("Deny this change", "[n / esc]"),
            ("Deny and provide instructions...", "[w]"),
        ]
    } else {
        [
            ("Allow once", "[y]"),
            ("Allow for this session", "[s]"),
            ("Deny execution", "[n / esc]"),
            ("Always allow & save to config", "[p]"),
            ("Deny and provide instructions...", "[w]"),
        ]
    };

    for (i, (opt, shortcut)) in options.iter().enumerate() {
        let is_selected = i == selected_option;
        let bullet = if is_selected { "●" } else { "○" };
        let num = i + 1;

        let num_style = if is_selected {
            Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray)
        };

        let text_style = if is_selected {
            Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };

        let shortcut_style = if is_selected {
            Style::default().fg(theme.accent_cyan)
        } else {
            Style::default().fg(theme.dark_gray)
        };

        let label_width = max_width.saturating_sub(21).clamp(16, 36);
        result.push(Line::from(vec![
            Span::styled(format!("    {} {}. ", bullet, num), num_style),
            Span::styled(format!("{:<label_width$} ", corex_core::truncate_ellipsis(opt, label_width)), text_style),
            Span::styled(*shortcut, shortcut_style),
        ]));
    }

    // 6. Interactive Instructions Input for Option 5
    if input_mode {
        result.push(Line::from(""));
        result.push(Line::from(vec![
            Span::styled("    Instructions for Corex:", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
        ]));

        let cursor_char = crate::cursor::block();
        result.push(Line::from(vec![
            Span::styled("    › ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            Span::styled(feedback_text.to_string(), Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
            Span::styled(cursor_char, Style::default().fg(theme.accent_cyan)),
        ]));

        result.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("[Enter]", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            Span::styled(" Send instructions to Corex    ", Style::default().fg(theme.gray)),
            Span::styled("[Esc]", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" Cancel", Style::default().fg(theme.gray)),
        ]));
    }

    result.push(Line::from(""));
    result
}

pub fn build_streaming_tool_preview_lines(
    calls: &[corex_core::types::ToolCall],
    elapsed_secs: f32,
    max_width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let dim_border = theme.border_dim;
    let beam_color = theme.accent_cyan;
    let box_width = max_width.clamp(30, 105);
    let inner_width = box_width.saturating_sub(6);
    let bar_len = inner_width + 2;
    let num_rows = calls.len().max(1);

    // Full 360-degree perimeter mapping (clockwise: Top -> Right -> Bottom -> Left)
    let total_p = ((bar_len + num_rows + 2) * 2) as f32;
    let cycle_time = 2.2; // 2.2 seconds for full perimeter circuit
    let head_pos = ((elapsed_secs % cycle_time) / cycle_time) * total_p;
    let beam_len = (total_p / 4.0).clamp(6.0, 20.0);

    let get_color_for_offset = |offset: f32| -> Color {
        let diff = (offset - head_pos).abs();
        let dist = diff.min(total_p - diff);
        if dist < beam_len * 0.45 {
            beam_color
        } else if dist < beam_len {
            Color::Rgb(70, 110, 160)
        } else {
            dim_border
        }
    };

    let mut result = Vec::new();

    // 1. Top border with traveling light (left-to-right)
    let mut top_spans = vec![
        Span::styled("  ╭", Style::default().fg(get_color_for_offset(0.0))),
    ];
    for i in 0..bar_len {
        let c = get_color_for_offset(i as f32);
        let style = if c == beam_color {
            Style::default().fg(c).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c)
        };
        top_spans.push(Span::styled("─", style));
    }
    top_spans.push(Span::styled("╮", Style::default().fg(get_color_for_offset(bar_len as f32))));
    result.push(Line::from(top_spans));

    // 2. Format currently streaming calls with dynamic left and right vertical border colors
    for (idx, call) in calls.iter().enumerate() {
        let is_last = idx == calls.len().saturating_sub(1);
        let tool_name = &call.function.name;
        let raw_args = &call.function.arguments;

        let (action_label, display_arg) = match tool_name.as_str() {
            "run_shell_command" | "shell" | "run_command" | "" => {
                let parsed_cmd = serde_json::from_str::<serde_json::Value>(raw_args)
                    .ok()
                    .and_then(|v| v.get("command").or_else(|| v.get("CommandLine")).and_then(|c| c.as_str()).map(|s| s.to_string()));

                let cmd = parsed_cmd.unwrap_or_else(|| {
                    if let Some(pos) = raw_args.find("\"command\":") {
                        let after = raw_args[pos + 10..].trim();
                        after.trim_start_matches('"').trim_end_matches('"').trim_end_matches('}').trim().to_string()
                    } else if raw_args.trim().is_empty() {
                        "analyzing...".to_string()
                    } else {
                        raw_args.to_string()
                    }
                });
                ("$ ", cmd)
            }
            "read_file" => {
                let path = serde_json::from_str::<serde_json::Value>(raw_args).ok()
                    .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|s| s.as_str()).map(|s| s.to_string()))
                    .unwrap_or_else(|| raw_args.to_string());
                ("read ", crate::app::formatting::format_relative_path(&path))
            }
            "write_file" => {
                let path = serde_json::from_str::<serde_json::Value>(raw_args).ok()
                    .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|s| s.as_str()).map(|s| s.to_string()))
                    .unwrap_or_else(|| raw_args.to_string());
                ("write ", crate::app::formatting::format_relative_path(&path))
            }
            "edit" | "apply_patch" => {
                let path = serde_json::from_str::<serde_json::Value>(raw_args).ok()
                    .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|s| s.as_str()).map(|s| s.to_string()))
                    .or_else(|| crate::app::formatting::patch_target(raw_args))
                    .unwrap_or_else(|| "patch".to_string());
                ("edit ", crate::app::formatting::format_relative_path(&path))
            }
            "list_directory" | "ls" => {
                let path = serde_json::from_str::<serde_json::Value>(raw_args).ok()
                    .and_then(|v| v.get("path").or_else(|| v.get("dir_path")).and_then(|s| s.as_str()).map(|s| s.to_string()))
                    .unwrap_or_else(|| ".".to_string());
                let mut rel = crate::app::formatting::format_relative_path(&path);
                if !rel.ends_with('/') && rel != "." {
                    rel.push('/');
                }
                ("ls ", rel)
            }
            "web_search" => {
                let q = serde_json::from_str::<serde_json::Value>(raw_args).ok()
                    .and_then(|v| v.get("query").and_then(|s| s.as_str()).map(|s| s.to_string()))
                    .unwrap_or_else(|| raw_args.to_string());
                ("web ", format!("\"{}\"", q))
            }
            "web_fetch" => {
                let url = serde_json::from_str::<serde_json::Value>(raw_args).ok()
                    .and_then(|v| v.get("url").and_then(|s| s.as_str()).map(|s| s.to_string()))
                    .unwrap_or_else(|| raw_args.to_string());
                ("fetch ", url)
            }
            "grep_search" | "grep" => {
                let parsed = serde_json::from_str::<serde_json::Value>(raw_args).ok();
                let q = parsed.as_ref().and_then(|v| v.get("pattern").or_else(|| v.get("query")).and_then(|s| s.as_str())).unwrap_or("");
                let path = parsed.as_ref().and_then(|v| v.get("path").or_else(|| v.get("dir_path")).and_then(|s| s.as_str())).unwrap_or("");
                let text = if !path.is_empty() && path != "." {
                    let rel = crate::app::formatting::format_relative_path(path);
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
                ("grep ", text)
            }
            "activate_skill" => {
                let name = serde_json::from_str::<serde_json::Value>(raw_args).ok()
                    .and_then(|v| v.get("name").and_then(|s| s.as_str()).map(|s| s.to_string()))
                    .unwrap_or_else(|| raw_args.to_string());
                ("skill ", name)
            }
            "delegate_subagent" => {
                let parsed = serde_json::from_str::<serde_json::Value>(raw_args).ok();
                let role = parsed.as_ref().and_then(|v| v.get("role").and_then(|s| s.as_str())).unwrap_or("worker");
                let prompt = parsed.as_ref()
                    .and_then(|v| v.get("prompt").or_else(|| v.get("task")).and_then(|s| s.as_str()))
                    .unwrap_or("")
                    .trim();
                let first_line = prompt.lines().next().unwrap_or(prompt);
                let display = if first_line.is_empty() {
                    role.to_string()
                } else {
                    format!("{} {}", role, corex_core::truncate_ellipsis(first_line, 50))
                };
                ("agent ", display)
            }
            other => {
                let parsed = serde_json::from_str::<serde_json::Value>(raw_args).ok();
                let primary_arg = parsed.as_ref().and_then(|v| {
                    v.get("path")
                        .or_else(|| v.get("dir_path"))
                        .or_else(|| v.get("file_path"))
                        .or_else(|| v.get("query"))
                        .or_else(|| v.get("pattern"))
                        .and_then(|s| s.as_str())
                });
                let arg = if let Some(arg) = primary_arg {
                    crate::app::formatting::format_relative_path(arg)
                } else if !raw_args.trim().is_empty() {
                    raw_args.to_string()
                } else {
                    "...".to_string()
                };
                (other, arg)
            }
        };

        let formatted = if action_label.ends_with(' ') {
            format!("{}{}", action_label, display_arg)
        } else {
            format!("{} {}", action_label, display_arg)
        };
        let unescaped_cmd = formatted.replace("\\\"", "\"").replace("\\\\", "\\");
        let (prefix, clean_body) = ("", unescaped_cmd.as_str());

        let cursor_span = if is_last {
            Some(Span::styled("█", Style::default().fg(theme.accent_cyan)))
        } else {
            None
        };

        // Calculate offsets along perimeter for right side (going down) and left side (going up)
        let right_offset = (bar_len + 1 + idx) as f32;
        let left_offset = (bar_len * 2 + num_rows + 3 + (num_rows.saturating_sub(1 + idx))) as f32;

        let right_border_color = get_color_for_offset(right_offset);
        let left_border_color = get_color_for_offset(left_offset);

        result.push(format_boxed_row(
            prefix,
            Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD),
            clean_body,
            Style::default().fg(theme.foreground),
            cursor_span,
            inner_width,
            left_border_color,
            right_border_color,
        ));
    }

    // 3. Bottom border with traveling light (right-to-left)
    let mut bot_spans = vec![
        Span::styled("  ╰", Style::default().fg(get_color_for_offset((bar_len * 2 + num_rows + 2) as f32))),
    ];
    for i in 0..bar_len {
        let offset = (bar_len + num_rows + 2 + (bar_len.saturating_sub(1 + i))) as f32;
        let c = get_color_for_offset(offset);
        let style = if c == beam_color {
            Style::default().fg(c).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c)
        };
        bot_spans.push(Span::styled("─", style));
    }
    bot_spans.push(Span::styled("╯", Style::default().fg(get_color_for_offset((bar_len + num_rows + 1) as f32))));
    result.push(Line::from(bot_spans));
    result.push(Line::from(""));

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_boxed_wrapped_preserves_full_long_command() {
        let long_cmd = "uptime && echo \"---\" && free -h && echo \"---\" && df -h / /home 2>/dev/null";
        let inner_width = 40;
        let lines = format_boxed_wrapped(
            "$ ",
            Style::default(),
            long_cmd,
            Style::default(),
            inner_width,
            Color::White,
            Color::White,
        );

        assert!(lines.len() > 1, "Long command must wrap into multiple rows");
        let combined_text: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ");

        assert!(!combined_text.contains("..."), "Wrapped command must never be truncated with '...'");
        assert!(combined_text.contains("uptime"));
        assert!(combined_text.contains("free -h"));
        assert!(combined_text.contains("df -h / /home"));
        assert!(combined_text.contains("2>/dev/null"));
    }

    #[test]
    fn test_format_boxed_wrapped_short_command() {
        let short_cmd = "whoami";
        let inner_width = 40;
        let lines = format_boxed_wrapped(
            "$ ",
            Style::default(),
            short_cmd,
            Style::default(),
            inner_width,
            Color::White,
            Color::White,
        );

        assert_eq!(lines.len(), 1);
        let text = lines[0].spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert!(text.contains("whoami"));
        assert!(!text.contains("..."));
    }

    #[test]
    fn approval_options_fit_a_phone_screen() {
        let theme = Theme::default();
        for width in [40usize, 46, 50, 80, 120] {
            let lines = build_tool_confirmation_lines("edit", "+++ b/a.rs\n+x", 0, 1, 1, 1, width, &theme, false, false, "");
            for line in &lines {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                if text.trim_start().starts_with(['●', '○']) {
                    let w = unicode_width::UnicodeWidthStr::width(text.as_str());
                    assert!(w <= width.max(30), "width {width}: option row is {w} wide: {text:?}");
                    assert!(text.contains('['), "the shortcut must stay visible: {text:?}");
                }
            }
        }
    }
}
