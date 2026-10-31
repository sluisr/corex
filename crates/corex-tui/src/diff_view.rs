use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// The bar that marks a block (a command, a diff, a running tool) instead of boxing it in.
const RAIL: &str = "  ▎ ";

/// One row of a railed block; content that does not fit is cut with "...".
fn format_rail_row(
    prefix: &str,
    prefix_style: Style,
    content: &str,
    content_style: Style,
    cursor: Option<Span<'static>>,
    inner_width: usize,
    rail: Color,
) -> Line<'static> {
    let cursor_len = usize::from(cursor.is_some());
    let prefix_len = unicode_width::UnicodeWidthStr::width(prefix);
    let avail = inner_width.saturating_sub(prefix_len + cursor_len);

    let display = if unicode_width::UnicodeWidthStr::width(content) > avail {
        let mut cut = String::new();
        let mut width = 0;
        for c in content.chars() {
            let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if width + w > avail.saturating_sub(3) {
                break;
            }
            cut.push(c);
            width += w;
        }
        cut.push_str("...");
        cut
    } else {
        content.to_string()
    };

    let mut spans = vec![
        Span::styled(RAIL, Style::default().fg(rail)),
        Span::styled(prefix.to_string(), prefix_style),
        Span::styled(display, content_style),
    ];
    if let Some(c) = cursor {
        spans.push(c);
    }
    Line::from(spans)
}

/// A railed row that wraps onto more rows instead of cutting, so a command is always shown whole.
fn format_rail_wrapped(
    prefix: &str,
    prefix_style: Style,
    content: &str,
    content_style: Style,
    inner_width: usize,
    rail: Color,
) -> Vec<Line<'static>> {
    let prefix_w = unicode_width::UnicodeWidthStr::width(prefix);
    if inner_width <= prefix_w || unicode_width::UnicodeWidthStr::width(content) <= inner_width - prefix_w {
        return vec![format_rail_row(prefix, prefix_style, content, content_style, None, inner_width, rail)];
    }

    let continuation = " ".repeat(prefix_w);
    let avail = inner_width - prefix_w;
    let mut lines = Vec::new();
    let mut remaining = content;
    while !remaining.is_empty() {
        let first = lines.is_empty();
        let mut take = 0;
        let mut width = 0;
        let mut last_space = None;
        for (idx, ch) in remaining.char_indices() {
            let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + w > avail {
                break;
            }
            width += w;
            take = idx + ch.len_utf8();
            if ch.is_whitespace() {
                last_space = Some(take);
            }
        }
        if take == remaining.len() {
            last_space = None;
        }
        // Prefer breaking at a space, unless that would leave a very short row.
        let split = last_space.filter(|&sp| sp > avail / 3).unwrap_or(take);
        if split == 0 {
            break;
        }
        let (chunk, rest) = remaining.split_at(split);
        lines.push(Line::from(vec![
            Span::styled(RAIL, Style::default().fg(rail)),
            Span::styled(if first { prefix.to_string() } else { continuation.clone() }, if first { prefix_style } else { Style::default() }),
            Span::styled(chunk.trim_end().to_string(), content_style),
        ]));
        remaining = rest.trim_start();
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

    // The rail's colour says what kind of action waits: a command, an edit, a delegation.
    let rail = if is_shell {
        theme.accent_yellow
    } else if is_edit {
        theme.accent_blue
    } else if is_subagent {
        theme.accent_cyan
    } else {
        theme.border_dim
    };
    let box_width = max_width.clamp(30, 105);
    let inner_width = box_width.saturating_sub(6);

    let mut result = Vec::new();

    // Body lines: 12 at most unless the user expands them with Ctrl+O.
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
            ("· ", "(no arguments provided)", theme.accent_purple)
        };
        result.push(format_rail_row(
            prefix,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
            body,
            Style::default().fg(theme.gray),
            None,
            inner_width,
            rail,
        ));
    } else {
        for line in shown_diff_lines {
        if is_edit {
            if line.starts_with('+') && !line.starts_with("+++") {
                result.push(format_rail_row(
                    "+ ",
                    Style::default().fg(theme.diff_added_fg).add_modifier(Modifier::BOLD),
                    &line[1..],
                    Style::default().fg(theme.diff_added_fg),
                    None,
                    inner_width,
                    rail,
                ));
            } else if line.starts_with('-') && !line.starts_with("---") {
                result.push(format_rail_row(
                    "- ",
                    Style::default().fg(theme.diff_removed_fg).add_modifier(Modifier::BOLD),
                    &line[1..],
                    Style::default().fg(theme.diff_removed_fg),
                    None,
                    inner_width,
                    rail,
                ));
            } else if line.starts_with("@@") {
                result.push(format_rail_row(
                    "",
                    Style::default(),
                    line,
                    Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD),
                    None,
                    inner_width,
                    rail,
                ));
            } else if line.starts_with("---") || line.starts_with("+++") {
                result.push(format_rail_row(
                    "",
                    Style::default(),
                    line,
                    Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD),
                    None,
                    inner_width,
                    rail,
                ));
            } else {
                result.push(format_rail_row(
                    "  ",
                    Style::default(),
                    line,
                    Style::default().fg(theme.foreground),
                    None,
                    inner_width,
                    rail,
                ));
            }
        } else {
            let is_sub = is_subagent || line.starts_with("Subagent");
            let is_command = !is_sub && (is_shell || line.trim_start().starts_with('$'));
            let (prefix, prefix_style, body) = if is_sub {
                ("◆ ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD), line.trim_start().to_string())
            } else if is_command {
                let b = line.trim_start().strip_prefix('$').unwrap_or(line.trim_start()).trim_start();
                // The command sits on a shaded chip, padded so the shade frames it.
                (" $ ", Style::default().fg(theme.accent_yellow).bg(theme.code_bg).add_modifier(Modifier::BOLD), format!("{b} "))
            } else {
                ("› ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD), line.trim_start().to_string())
            };
            let body_style = if is_command {
                Style::default().fg(theme.text_strong).bg(theme.code_bg)
            } else {
                Style::default().fg(theme.text_strong)
            };

            result.extend(format_rail_wrapped(prefix, prefix_style, &body, body_style, inner_width, rail));
        }
    }
    }

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
    // Without a box around it, a blank line keeps the block apart from the question.
    result.push(Line::from(""));

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

    // Free-text instructions typed after choosing "Deny and provide instructions".
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
    let inner_width = max_width.clamp(30, 105).saturating_sub(6);
    // A soft pulse runs down the rail while the model is still writing the calls.
    let rail_at = |row: usize| -> Color {
        let phase = (elapsed_secs * 1.4 - row as f32 * 0.25).rem_euclid(1.0);
        if phase < 0.35 {
            theme.accent_cyan
        } else if phase < 0.6 {
            Color::Rgb(70, 110, 160)
        } else {
            theme.border_dim
        }
    };

    let mut result = Vec::new();
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

        let rail = rail_at(idx);

        result.push(format_rail_row(
            prefix,
            Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD),
            clean_body,
            Style::default().fg(theme.foreground),
            cursor_span,
            inner_width,
            rail,
        ));
    }

    result.push(Line::from(""));

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_commands_wrap_instead_of_being_cut() {
        let long_cmd = "uptime && echo \"---\" && free -h && echo \"---\" && df -h / /home 2>/dev/null";
        let inner_width = 40;
        let lines = format_rail_wrapped(
            "$ ",
            Style::default(),
            long_cmd,
            Style::default(),
            inner_width,
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
    fn short_commands_stay_on_one_row() {
        let short_cmd = "whoami";
        let inner_width = 40;
        let lines = format_rail_wrapped(
            "$ ",
            Style::default(),
            short_cmd,
            Style::default(),
            inner_width,
            Color::White,
        );

        assert_eq!(lines.len(), 1);
        let text = lines[0].spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert!(text.contains("whoami"));
        assert!(!text.contains("..."));
    }

    #[test]
    fn blocks_are_marked_by_a_rail_not_boxed_in() {
        let theme = Theme::default();
        let lines = build_tool_confirmation_lines("run_shell_command", "$ uname -a", 0, 1, 1, 1, 80, &theme, false, false, "");
        let text: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert!(text.iter().any(|t| t.starts_with("  ▎  $ uname -a ")), "{text:#?}");
        let command = lines.iter().flat_map(|l| &l.spans).find(|s| s.content.contains("uname")).unwrap();
        assert_eq!(command.style.bg, Some(theme.code_bg), "the command sits on a shaded chip");
        assert!(!text.iter().any(|t| t.contains(['╭', '╮', '╰', '╯', '│'])), "{text:#?}");
        let rail = lines.iter().flat_map(|l| &l.spans).find(|s| s.content.as_ref() == RAIL).unwrap();
        assert_eq!(rail.style.fg, Some(theme.accent_yellow), "a shell command gets the shell colour");
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
