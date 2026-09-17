use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Formats a single content row enclosed within rounded box borders with 100% width precision.
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

pub fn build_tool_confirmation_lines(
    tool_name: &str,
    diff_or_cmd: &str,
    selected_option: usize,
    batch_count: usize,
    max_width: usize,
    theme: &Theme,
    expanded: bool,
) -> Vec<Line<'static>> {
    let is_shell = tool_name == "run_shell_command" || tool_name == "shell";
    let is_edit = tool_name == "replace" || tool_name == "edit" || tool_name == "write_file" || tool_name == "apply_patch";

    let border_color = Color::Rgb(70, 80, 100);
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
        result.push(format_boxed_row(
            "$ ",
            Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD),
            "(no arguments provided)",
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
                    Style::default().fg(Color::Rgb(225, 235, 245)),
                    None,
                    inner_width,
                    border_color,
                    border_color,
                ));
            }
        } else {
            let (prefix, body) = if line.trim_start().starts_with('$') {
                ("$ ", line.trim_start()[1..].trim_start())
            } else {
                ("$ ", line.trim_start())
            };

            result.push(format_boxed_row(
                prefix,
                Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD),
                body,
                Style::default().fg(Color::Rgb(230, 238, 248)),
                None,
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
            format!("(Ctrl+O para colapsar — {} líneas)", total_diff_lines)
        } else {
            format!("({} líneas más — Ctrl+O para ver todo)", total_diff_lines - 12)
        };
        result.push(Line::from(Span::styled(
            format!("  {}", hint),
            Style::default().fg(theme.gray),
        )));
    }

    // 4. Question line
    let question_line = if is_shell {
        let label = if batch_count > 1 {
            format!("[{} Shell Commands]", batch_count)
        } else {
            "[Shell]".to_string()
        };
        Line::from(vec![
            Span::raw("  Allow execution of "),
            Span::styled(label, Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
            Span::raw("?"),
        ])
    } else if is_edit {
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Apply this change?", Style::default().add_modifier(Modifier::BOLD)),
        ])
    } else {
        let label = if batch_count > 1 {
            format!("[{} Tools]", batch_count)
        } else {
            format!("[{}]", tool_name)
        };
        Line::from(vec![
            Span::raw("  Allow execution of "),
            Span::styled(label, Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            Span::raw("?"),
        ])
    };
    result.push(question_line);

    // 5. Options
    let options = [
        "Allow once",
        "Allow for this session",
        "No, suggest changes (esc)",
    ];

    for (i, opt) in options.iter().enumerate() {
        let is_selected = i == selected_option;
        let bullet = if is_selected { "●" } else { "○" };
        let num = i + 1;

        let style = if is_selected {
            Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray)
        };

        result.push(Line::from(vec![
            Span::styled(format!("    {} {}. ", bullet, num), style),
            Span::styled(*opt, style),
        ]));
    }

    result.push(Line::from(""));
    result
}

pub fn build_streaming_tool_preview_lines(
    calls: &[uti_core::types::ToolCall],
    elapsed_secs: f32,
    max_width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let dim_border = Color::Rgb(60, 70, 90);
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

        let display_cmd = if tool_name == "run_shell_command" || tool_name == "shell" || tool_name.is_empty() {
            let parsed_cmd = serde_json::from_str::<serde_json::Value>(raw_args)
                .ok()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(|s| s.to_string()));

            parsed_cmd.unwrap_or_else(|| {
                if let Some(pos) = raw_args.find("\"command\":") {
                    let after = raw_args[pos + 10..].trim();
                    let unquoted = after
                        .trim_start_matches('"')
                        .trim_end_matches('"')
                        .trim_end_matches('}')
                        .trim();
                    unquoted.to_string()
                } else if raw_args.trim().is_empty() {
                    "analyzing...".to_string()
                } else {
                    raw_args.to_string()
                }
            })
        } else {
            let parsed = serde_json::from_str::<serde_json::Value>(raw_args).ok();
            let primary_arg = parsed.as_ref().and_then(|v| {
                v.get("path")
                    .or_else(|| v.get("dir_path"))
                    .or_else(|| v.get("file_path"))
                    .or_else(|| v.get("query"))
                    .or_else(|| v.get("pattern"))
                    .and_then(|s| s.as_str())
            });

            if let Some(arg) = primary_arg {
                format!("{}: {}", tool_name, arg)
            } else if !raw_args.trim().is_empty() {
                format!("{}: {}", tool_name, raw_args)
            } else {
                format!("{}...", tool_name)
            }
        };

        let unescaped_cmd = display_cmd.replace("\\\"", "\"").replace("\\\\", "\\");
        let (prefix, clean_body) = if unescaped_cmd.trim_start().starts_with('$') {
            ("$ ", unescaped_cmd.trim_start()[1..].trim_start())
        } else if tool_name == "run_shell_command" || tool_name == "shell" || tool_name.is_empty() {
            ("$ ", unescaped_cmd.as_str())
        } else {
            ("⊷ ", unescaped_cmd.as_str())
        };

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
            Style::default().fg(Color::Rgb(225, 235, 245)),
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
