//! UI rendering widgets and layouts using Ratatui.

use std::sync::atomic::Ordering;
use std::time::Duration;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use corex_core::types::Message;
use crate::ascii::{render_gradient_logo, render_gradient_logo_adaptive};
use crate::auth_dialog::render_auth_dialog;
use crate::diff_view::{build_streaming_tool_preview_lines, build_tool_confirmation_lines};
use crate::markdown::render_markdown;
use crate::model_dialog::render_model_dialog;
use crate::session_dialog::render_session_dialog;
use crate::slash_commands::render_command_popup;
use crate::sudo_dialog::render_sudo_dialog;
use crate::theme::Theme;
use crate::user_dialog::render_user_dialog;

use super::formatting::*;
use super::mobile::is_mobile_portrait;
use super::state::{App, PendingToolBatch, Selection};

#[allow(clippy::too_many_arguments)]
pub fn render_about_card(
    version: &str,
    model: &str,
    base_url: &str,
    local_engine: &str,
    local_enabled: bool,
    session_id: &str,
    workspace: &str,
    branch: &str,
    theme: &Theme,
    content_max_width: usize,
) -> Vec<Line<'static>> {
    // Total inner width between left border and right border
    let inner_width = (content_max_width.saturating_sub(6)).clamp(44, 72);
    let label_w = 23;
    let usable_w = inner_width.saturating_sub(4); // 3 spaces left, 1 space right

    let make_row = |label: &str, val_spans: Vec<Span<'static>>| -> Line<'static> {
        let mut line_spans = vec![
            Span::styled("  │   ", Style::default().fg(theme.dark_gray)),
            Span::styled(format!("{:<width$}", label, width = label_w), Style::default().fg(Color::Rgb(160, 172, 190)).add_modifier(Modifier::BOLD)),
        ];
        let val_used: usize = val_spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
        line_spans.extend(val_spans);
        let rem = usable_w.saturating_sub(label_w + val_used);
        if rem > 0 {
            line_spans.push(Span::raw(" ".repeat(rem)));
        }
        line_spans.push(Span::styled(" │", Style::default().fg(theme.dark_gray)));
        Line::from(line_spans)
    };

    let empty_row = || -> Line<'static> {
        Line::from(vec![
            Span::styled("  │", Style::default().fg(theme.dark_gray)),
            Span::raw(" ".repeat(inner_width)),
            Span::styled("│", Style::default().fg(theme.dark_gray)),
        ])
    };

    // Top line: ╭─ Corex v0.2.0 ───────╮
    let title = format!(" Corex v{} ", version);
    let title_w = unicode_width::UnicodeWidthStr::width(title.as_str());
    let top_fill = inner_width.saturating_sub(1 + title_w);
    let top_line = Line::from(vec![
        Span::styled("  ╭─", Style::default().fg(theme.dark_gray)),
        Span::styled(title, Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}╮", "─".repeat(top_fill)), Style::default().fg(theme.dark_gray)),
    ]);

    // Section divider: ├─ Session Telemetry ──────┤
    let div_title = "─ Session Telemetry ";
    let div_w = unicode_width::UnicodeWidthStr::width(div_title);
    let div_fill = inner_width.saturating_sub(div_w);
    let div_line = Line::from(vec![
        Span::styled("  ├", Style::default().fg(theme.dark_gray)),
        Span::styled(div_title, Style::default().fg(Color::Rgb(95, 115, 140)).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}┤", "─".repeat(div_fill)), Style::default().fg(theme.dark_gray)),
    ]);

    // Bottom line: ╰────────────────────────╯
    let bot_line = Line::from(Span::styled(
        format!("  ╰{}╯", "─".repeat(inner_width)),
        Style::default().fg(theme.dark_gray),
    ));

    let local_status = if local_enabled { "(Active)" } else { "(Disabled)" };
    let local_color = if local_enabled { Color::Rgb(115, 185, 140) } else { Color::Rgb(110, 120, 135) };

    let val_max = usable_w.saturating_sub(label_w + 1);

    vec![
        top_line,
        empty_row(),
        make_row("Creator & Maintainer", vec![
            Span::styled("sluisr", Style::default().fg(Color::Rgb(225, 230, 242)).add_modifier(Modifier::BOLD)),
            Span::styled(" (https://sluisr.com)", Style::default().fg(Color::Rgb(115, 160, 220))),
        ]),
        make_row("Official Website", vec![
            Span::styled(corex_core::truncate_ellipsis("https://corex.sluisr.com", val_max), Style::default().fg(Color::Rgb(105, 185, 235))),
        ]),
        make_row("Discord Community", vec![
            Span::styled(corex_core::truncate_ellipsis("https://discord.gg/MGMt6M6Yx", val_max), Style::default().fg(Color::Rgb(105, 185, 235))),
        ]),
        make_row("Changelog & Releases", vec![
            Span::styled(corex_core::truncate_ellipsis("https://corex.sluisr.com/changelog", val_max), Style::default().fg(Color::Rgb(105, 185, 235))),
        ]),
        make_row("Report Issues & Bugs", vec![
            Span::styled(corex_core::truncate_ellipsis("https://github.com/sluisr/corex/issues", val_max), Style::default().fg(Color::Rgb(105, 185, 235))),
        ]),
        make_row("GitHub Repository", vec![
            Span::styled(corex_core::truncate_ellipsis("https://github.com/sluisr/corex", val_max), Style::default().fg(Color::Rgb(105, 185, 235))),
        ]),
        empty_row(),
        div_line,
        empty_row(),
        make_row("Active Model", vec![
            Span::styled(corex_core::truncate_ellipsis(model, val_max), Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
        ]),
        make_row("API Base URL", vec![
            Span::styled(corex_core::truncate_ellipsis(base_url, val_max), Style::default().fg(Color::Rgb(140, 150, 170))),
        ]),
        make_row("Local SLM Engine", vec![
            Span::styled(format!("{} ", corex_core::truncate_ellipsis(local_engine, val_max.saturating_sub(12))), Style::default().fg(Color::Rgb(140, 150, 170))),
            Span::styled(local_status, Style::default().fg(local_color)),
        ]),
        make_row("Session ID", vec![
            Span::styled(corex_core::truncate_ellipsis(session_id, val_max), Style::default().fg(Color::Rgb(125, 135, 150))),
        ]),
        make_row("Workspace", vec![
            Span::styled(corex_core::truncate_ellipsis(workspace, val_max), Style::default().fg(Color::Rgb(150, 180, 135))),
        ]),
        make_row("Git Branch", vec![
            Span::styled(corex_core::truncate_ellipsis(branch, val_max), Style::default().fg(Color::Rgb(210, 165, 105))),
        ]),
        empty_row(),
        bot_line,
    ]
}

pub fn format_system_message(text: &str, theme: &Theme, content_max_width: usize, logs_expanded: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let raw_lines: Vec<&str> = text.lines().collect();

    if raw_lines.is_empty() {
        return lines;
    }

    let first_line = raw_lines[0].trim();

    // 0. Dedicated Corex Info Card
    if first_line.starts_with("COREX_INFO_CARD|") {
        let parts: Vec<&str> = first_line.split('|').collect();
        if parts.len() >= 9 {
            return render_about_card(
                parts[1],
                parts[2],
                parts[3],
                parts[4],
                parts[5] == "true",
                parts[6],
                parts[7],
                parts[8],
                theme,
                content_max_width,
            );
        }
    }

    // Soft, muted color palette for discreet visual presence (opaco / disimulado)
    let muted_dim = Color::Rgb(70, 82, 98);
    let muted_label = Color::Rgb(105, 118, 135);
    let muted_text = Color::Rgb(155, 168, 185);
    let muted_tag = Color::Rgb(95, 115, 138);

    // 1. Task Finished / Background Task Done (One-Liner Badge + tail or full log)
    if first_line.starts_with("TASK_DONE|") || first_line.starts_with("[TASK FINISHED]") {
        let (pid, exit_code_str, duration) = if first_line.starts_with("TASK_DONE|") {
            let parts: Vec<&str> = first_line.split('|').collect();
            let pid = parts.get(1).copied().unwrap_or("?");
            let code = parts.get(2).copied().unwrap_or("0");
            let dur = parts.get(3).copied().unwrap_or("");
            (pid, code, dur)
        } else {
            ("task", "0", "")
        };

        let is_ok = exit_code_str == "0";
        let is_stopped = exit_code_str == "stopped";

        let (icon, icon_color, status_text, status_color) = if is_stopped {
            ("  ■ ", Color::Rgb(215, 175, 100), "stopped".to_string(), Color::Rgb(215, 175, 100))
        } else if is_ok {
            ("  ✓ ", Color::Rgb(105, 185, 135), "finished (exit 0)".to_string(), Color::Rgb(125, 160, 145))
        } else {
            let label = format!("finished (exit {})", exit_code_str);
            ("  ✕ ", Color::Rgb(220, 100, 100), label, Color::Rgb(220, 110, 110))
        };

        // Output lines
        let output_lines: Vec<&str> = raw_lines[1..]
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let total = output_lines.len();
        let max_lines = 5;

        let mut badge_spans = vec![
            Span::styled(icon, Style::default().fg(icon_color)),
            Span::styled(format!("[bg:{}] ", pid), Style::default().fg(muted_tag)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ];
        if !duration.trim().is_empty() {
            badge_spans.push(Span::styled(format!(" ({})", duration.trim()), Style::default().fg(muted_dim)));
        }
        if logs_expanded && total > max_lines {
            badge_spans.push(Span::styled(" (Ctrl+O to collapse)", Style::default().fg(muted_dim)));
        }
        lines.push(Line::from(badge_spans));

        if !output_lines.is_empty() {
            if logs_expanded {
                for line_str in &output_lines {
                    let clean = line_str.replace('\t', "    ");
                    let spans = vec![
                        Span::styled(clean, Style::default().fg(muted_text)),
                    ];
                    lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    │ "));
                }
            } else {
                if total > max_lines {
                    let omitted = total - max_lines;
                    let om_spans = vec![
                        Span::styled("    │ ", Style::default().fg(muted_dim)),
                        Span::styled(format!("… (+{} lines above, Ctrl+O to expand)", omitted), Style::default().fg(muted_dim)),
                    ];
                    lines.push(Line::from(om_spans));
                }

                let start_idx = total.saturating_sub(max_lines);
                for line_str in &output_lines[start_idx..] {
                    let clean = line_str.replace('\t', "    ");
                    let spans = vec![
                        Span::styled(clean, Style::default().fg(muted_text)),
                    ];
                    lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    │ "));
                }
            }
        }

        return lines;
    }

    // 2. Model Activation Message
    if first_line.starts_with("Activated ") || first_line.starts_with("Switched ") {
        let title = first_line.trim_start_matches("Activated ").trim_start_matches("Switched ");
        let header_spans = vec![
            Span::styled("  · ", Style::default().fg(muted_dim)),
            Span::styled("[model] ", Style::default().fg(muted_tag)),
            Span::styled(title.to_string(), Style::default().fg(muted_text)),
        ];
        lines.push(Line::from(header_spans));

        for sub_line in &raw_lines[1..] {
            let trimmed = sub_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(param) = trimmed.strip_prefix("- ") {
                let mut spans = vec![
                    Span::styled("    · ", Style::default().fg(muted_dim)),
                ];
                if let Some((k, v)) = param.split_once(':') {
                    spans.push(Span::styled(format!("{}: ", k.trim()), Style::default().fg(muted_label)));
                    let val = v.trim();
                    let val_color = match val.to_lowercase().as_str() {
                        "dynamic" | "low" | "enabled" => Color::Rgb(115, 150, 135),
                        "medium" => Color::Rgb(165, 150, 120),
                        "high" | "max" => Color::Rgb(150, 130, 165),
                        _ => Color::Rgb(140, 155, 175),
                    };
                    spans.push(Span::styled(val.to_string(), Style::default().fg(val_color)));
                } else {
                    spans.push(Span::styled(param.to_string(), Style::default().fg(muted_text)));
                }
                lines.push(Line::from(spans));
            } else {
                let spans = vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(trimmed.to_string(), Style::default().fg(muted_label)),
                ];
                lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    "));
            }
        }
        return lines;
    }

    // 2. Error / Failure Message
    if first_line.starts_with("Error") || first_line.starts_with("Failed") {
        let err_spans = vec![
            Span::styled("  ✕ ", Style::default().fg(Color::Rgb(190, 95, 95))),
            Span::styled("[error] ", Style::default().fg(Color::Rgb(190, 95, 95))),
            Span::styled(first_line.to_string(), Style::default().fg(muted_text)),
        ];
        lines.extend(crate::markdown::wrap_spans(err_spans, content_max_width, "    "));
        for sub_line in &raw_lines[1..] {
            let spans = vec![
                Span::styled("    ", Style::default()),
                Span::styled(sub_line.to_string(), Style::default().fg(muted_label)),
            ];
            lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    "));
        }
        return lines;
    }

    // 3. Checkpoint / Saved Message
    if first_line.contains("checkpoint saved") || first_line.contains("saved with tag") {
        let save_spans = vec![
            Span::styled("  ✓ ", Style::default().fg(Color::Rgb(105, 145, 125))),
            Span::styled("[checkpoint] ", Style::default().fg(Color::Rgb(105, 145, 125))),
            Span::styled(first_line.to_string(), Style::default().fg(muted_text)),
        ];
        lines.extend(crate::markdown::wrap_spans(save_spans, content_max_width, "    "));
        return lines;
    }

    // 4. Session Event
    if first_line.starts_with("Resumed session") || first_line.starts_with("Started new chat") {
        let session_spans = vec![
            Span::styled("  · ", Style::default().fg(muted_dim)),
            Span::styled("[session] ", Style::default().fg(muted_tag)),
            Span::styled(first_line.to_string(), Style::default().fg(muted_text)),
        ];
        lines.extend(crate::markdown::wrap_spans(session_spans, content_max_width, "    "));
        return lines;
    }

    // 5. Rich Markdown Formatted System Messages (e.g. headers, rules, code blocks)
    if text.contains('#') || text.contains("```") || text.contains("---") {
        return crate::markdown::render_markdown(text, theme, content_max_width);
    }

    // 6. Default General Multi-line or Single-line System Message
    let mut is_first = true;
    for line in raw_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let prefix = if is_first { "  · " } else { "    " };
        let sys_spans = vec![
            Span::styled(prefix, Style::default().fg(muted_dim)),
            Span::styled(trimmed.to_string(), Style::default().fg(muted_label)),
        ];
        lines.extend(crate::markdown::wrap_spans(sys_spans, content_max_width, "    "));
        is_first = false;
    }

    lines
}

pub fn render_single_message_with_pending(
    _msg_idx: usize,
    msg: &Message,
    next_msg: Option<&Message>,
    theme: &Theme,
    content_max_width: usize,
    pending_conf: Option<&PendingToolBatch>,
    logs_expanded: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match msg.role.as_str() {
        "user" => {
            let content = msg.text_content().unwrap_or("");
            for (idx, line) in content.split('\n').enumerate() {
                let (spans, first_indent) = if idx == 0 {
                    (
                        vec![
                            Span::styled("You: ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                            Span::styled(line.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                        ],
                        "  ",
                    )
                } else {
                    (
                        vec![
                            Span::styled(line.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                        ],
                        "       ",
                    )
                };
                let wrapped = crate::markdown::wrap_spans_with_indent(spans, content_max_width, first_indent, "       ");
                lines.extend(wrapped);
            }
            lines.push(Line::from(""));
        }
        "assistant" => {
            if let Some(text) = msg.text_content() {
                let mut md_lines = render_markdown(text, theme, content_max_width);
                md_lines.push(Line::from(""));
                lines.extend(md_lines);
            }

            if let Some(ref calls) = msg.tool_calls {
                for call in calls {
                    if let Some(pending) = pending_conf {
                        if pending.calls.iter().any(|c| c.id == call.id) {
                            continue;
                        }
                    }

                    let spans = format_tool_call_spans(&call.function.name, &call.function.arguments, theme);
                    let wrapped = crate::markdown::wrap_spans(spans, content_max_width, "  ");
                    lines.extend(wrapped);
                }
            }
        }
        "tool" => {
            let content = msg.text_content().unwrap_or("");
            let is_last_tool = next_msg.map(|m| m.role.as_str() != "tool").unwrap_or(true);

            // Skills are internal procedural knowledge loaded for the AI — suppress raw log dumping
            let is_skill = msg.name.as_deref() == Some("activate_skill")
                || content.starts_with("---\nname:")
                || content.starts_with("# Corex — ")
                || content.contains("Specialized procedural skill for");

            if is_skill {
                let is_err = content.starts_with("Error") || content.starts_with("Failed") || content.starts_with("Skill '");
                if is_err {
                    let line_spans = vec![
                        Span::styled("    ✕ ", Style::default().fg(Color::Red)),
                        Span::styled(content.lines().next().unwrap_or("Failed to load skill").to_string(), Style::default().fg(theme.gray)),
                    ];
                    lines.extend(crate::markdown::wrap_spans(line_spans, content_max_width, "    "));
                }
                return lines;
            }

            let check_slice = corex_core::safe_truncate_str(content, 2048);
            if check_slice.contains("denied by user") || check_slice.contains("declined") {
                let line_spans = vec![
                    Span::styled("    ✕ ", Style::default().fg(Color::Red)),
                    Span::styled("Execution declined by user", Style::default().fg(theme.gray)),
                ];
                let wrapped = crate::markdown::wrap_spans(line_spans, content_max_width, "    ");
                lines.extend(wrapped);
            } else if content.starts_with("Successfully edited") && content.contains('\n') {
                let mut content_lines = content.lines();
                let header = content_lines.next().unwrap_or("Successfully edited");
                let header_spans = vec![
                    Span::styled("    ✓ ", Style::default().fg(Color::Green)),
                    Span::styled(header.to_string(), Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
                ];
                let wrapped_header = crate::markdown::wrap_spans(header_spans, content_max_width, "    ");
                lines.extend(wrapped_header);

                for row in content_lines {
                    if row.is_empty() {
                        continue;
                    }
                    // The marker decides the colour, so what the edit dropped is painted as
                    // prominently as what it added instead of blending into the context.
                    let (marker, body) = match row.chars().next() {
                        Some(marker @ ('-' | '+' | ' ')) => (marker, &row[1..]),
                        _ => (' ', row),
                    };
                    let (marker_style, text_style, gutter_style) = match marker {
                        '-' => (
                            Style::default().fg(theme.diff_removed_fg).add_modifier(Modifier::BOLD),
                            Style::default().fg(theme.diff_removed_fg),
                            Style::default().fg(theme.diff_removed_fg),
                        ),
                        '+' => (
                            Style::default().fg(theme.diff_added_fg).add_modifier(Modifier::BOLD),
                            Style::default().fg(theme.diff_added_fg),
                            Style::default().fg(theme.diff_added_fg),
                        ),
                        _ => (
                            Style::default(),
                            Style::default().fg(theme.gray),
                            Style::default().fg(theme.dark_gray),
                        ),
                    };

                    let mut spans = vec![Span::styled(format!("      {} ", marker), marker_style)];
                    if let Some((num, code)) = body.split_once('|') {
                        spans.push(Span::styled(num.to_string(), gutter_style));
                        spans.push(Span::styled("│", gutter_style));
                        spans.push(Span::styled(code.to_string(), text_style));
                    } else {
                        spans.push(Span::styled(body.to_string(), text_style));
                    }
                    lines.push(Line::from(spans));
                }
            } else if content.starts_with("[COMMAND SENT TO BACKGROUND]") || content.starts_with("[BACKGROUND TASK LAUNCHED]") {
                let pid_str = content.lines().find(|l| l.contains("Task ID (PID):")).and_then(|l| l.split(':').nth(1)).map(|p| p.trim()).unwrap_or("?");
                let line_spans = vec![
                    Span::styled("    · ", Style::default().fg(theme.accent_cyan)),
                    Span::styled(format!("[bg:{}] ", pid_str), Style::default().fg(theme.accent_cyan)),
                    Span::styled("running in background...", Style::default().fg(theme.gray)),
                ];
                let wrapped = crate::markdown::wrap_spans(line_spans, content_max_width, "    ");
                lines.extend(wrapped);
            } else {
                let raw_output_lines: Vec<&str> = content
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .collect();

                let is_err = content.starts_with("Error")
                    || content.starts_with("Failed")
                    || content.starts_with("Command exited with status code");

                let icon = if is_err { "    ✕ " } else { "    ✓ " };
                let icon_color = if is_err { Color::Red } else { Color::Green };

                if raw_output_lines.len() <= 1 {
                    let first_line_raw = raw_output_lines.first().copied().unwrap_or("Done").replace('\t', "    ");
                    let display_first_line = if first_line_raw.len() > 140 {
                        let mut end = 137;
                        while end > 0 && !first_line_raw.is_char_boundary(end) {
                            end -= 1;
                        }
                        format!("{}...", &first_line_raw[..end])
                    } else {
                        first_line_raw
                    };

                    let line_spans = vec![
                        Span::styled(icon, Style::default().fg(icon_color)),
                        Span::styled(display_first_line, Style::default().fg(theme.gray)),
                    ];
                    let wrapped = crate::markdown::wrap_spans(line_spans, content_max_width, "    ");
                    lines.extend(wrapped);
                } else {
                    let total = raw_output_lines.len();
                    let header_label = if is_err {
                        format!("Command failed ({} lines)", total)
                    } else {
                        format!("Output ({} lines)", total)
                    };

                    let mut header_spans = vec![
                        Span::styled(icon, Style::default().fg(icon_color)),
                        Span::styled(header_label, Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
                    ];
                    if !logs_expanded {
                        header_spans.push(Span::styled(" (Ctrl+O to expand)", Style::default().fg(theme.dark_gray)));
                    } else {
                        header_spans.push(Span::styled(" (Ctrl+O to collapse)", Style::default().fg(theme.dark_gray)));
                    }
                    lines.push(Line::from(header_spans));

                    let max_lines = 5;
                    if logs_expanded {
                        for line_str in &raw_output_lines {
                            let clean = line_str.replace('\t', "    ");
                            let spans = vec![
                                Span::styled(clean, Style::default().fg(theme.gray)),
                            ];
                            lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "        │ "));
                        }
                    } else {
                        if total > max_lines {
                            let omitted = total - max_lines;
                            let om_spans = vec![
                                Span::styled("        │ ", Style::default().fg(theme.dark_gray)),
                                Span::styled(format!("… (+{} lines above)", omitted), Style::default().fg(theme.dark_gray)),
                            ];
                            lines.push(Line::from(om_spans));
                        }

                        let start_idx = total.saturating_sub(max_lines);
                        for line_str in &raw_output_lines[start_idx..] {
                            let clean = line_str.replace('\t', "    ");
                            let spans = vec![
                                Span::styled(clean, Style::default().fg(theme.gray)),
                            ];
                            lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "        │ "));
                        }
                    }
                }
            }

            if is_last_tool {
                lines.push(Line::from(""));
            }
        }
        "system" => {
            if let Some(text) = msg.text_content() {
                lines.extend(format_system_message(text, theme, content_max_width, logs_expanded));
                let is_task_done = text.starts_with("TASK_DONE|") || text.starts_with("[TASK FINISHED]");
                let next_is_task_done = next_msg
                    .and_then(|m| if m.role.as_str() == "system" { m.text_content() } else { None })
                    .map(|t| t.starts_with("TASK_DONE|") || t.starts_with("[TASK FINISHED]"))
                    .unwrap_or(false);

                if !is_task_done || !next_is_task_done {
                    lines.push(Line::from(""));
                }
            }
        }
        _ => {}
    }
    lines
}

pub fn render_ui(frame: &mut Frame, app: &mut App) {
    let size = frame.area();

    if app.input_buffer.starts_with('/') {
        app.last_slash_filter = app.input_buffer.clone();
    }

    let target_height = app.slash_popup_target_height();
    let speed = 0.35;
    let diff = target_height - app.slash_popup_height_current;
    if diff.abs() > 0.05 {
        app.slash_popup_height_current += diff * speed;
    } else {
        app.slash_popup_height_current = target_height;
    }

    // Bordered popup needs at least 3 lines (top border, 1 item, bottom border).
    // When closing, any height < 2.5 snaps to 0 so the single top-border line is never rendered.
    let popup_height = if app.slash_popup_height_current >= 2.5 {
        app.slash_popup_height_current.round() as u16
    } else {
        0
    };

    let show_activity = (app.is_streaming || !app.status_transition.is_empty())
        && app.pending_confirmation.is_none()
        && !app.user_dialog.is_open
        && !app.sudo_dialog.is_open;
    let activity_height = if show_activity { 1 } else { 0 };

    // The prompt prefix and the composer height are resolved before the layout because the
    // composer grows with the wrapped input: a long message must push the text onto extra rows
    // instead of running off the right edge of the terminal.
    let prompt_prefix = if app.always_allow_tools {
        Span::styled("* ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else if app.plan_mode {
        Span::styled("? ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("> ", Style::default().fg(app.theme.accent_blue).add_modifier(Modifier::BOLD))
    };
    let prefix_width = prompt_prefix.width();

    const MAX_COMPOSER_LINES: usize = 6;
    let composer_content_lines = {
        let cursor_at_end = app.cursor_idx >= app.input_buffer.len();
        let stream_width = unicode_width::UnicodeWidthStr::width(app.input_buffer.as_str())
            + usize::from(!app.input_buffer.is_empty() && cursor_at_end);
        let capacity = (size.width as usize).saturating_sub(prefix_width).max(1);
        if app.pending_confirmation.is_some() || app.input_buffer.is_empty() {
            1
        } else {
            stream_width.div_ceil(capacity).clamp(1, MAX_COMPOSER_LINES)
        }
    };
    let composer_height = if app.user_dialog.is_open {
        crate::user_dialog::calculate_dialog_height(&app.user_dialog, size.width, size.height)
    } else {
        composer_content_lines as u16 + 2
    };

    let chunks = if popup_height > 0 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),               // Chat Feed
                Constraint::Length(popup_height), // Autocomplete Popup Slot
                Constraint::Length(activity_height), // Fixed Activity / Status Bar
                Constraint::Length(composer_height), // Input Composer
                Constraint::Length(1),            // Status Footer Bar
            ])
            .split(size)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),               // Chat Feed
                Constraint::Length(activity_height), // Fixed Activity / Status Bar
                Constraint::Length(composer_height), // Input Composer
                Constraint::Length(1),            // Status Footer Bar
            ])
            .split(size)
    };

    let chat_chunk = chunks[0];
    let (popup_chunk, activity_chunk, composer_chunk, status_chunk) = if popup_height > 0 {
        (Some(chunks[1]), chunks[2], chunks[3], chunks[4])
    } else {
        (None, chunks[1], chunks[2], chunks[3])
    };

    let content_max_width = (chat_chunk.width as usize).saturating_sub(4).max(20);
    let mut all_lines = Vec::with_capacity(app.cached_message_lines.len() + 128);

    // 1. Header Banner (First item in scrollable feed!)
    let cfg = app.llm_client.get_config();
    let is_auth = !cfg.api_key.trim().is_empty();
    let is_local = cfg.local_llm_enabled;
    let update_notice = app.update_available.lock().ok().and_then(|l| l.clone());
    let elapsed = app.start_time.elapsed().as_secs_f32();
    let header_lines = if is_mobile_portrait(size.width, size.height) {
        render_gradient_logo_adaptive(
            env!("CARGO_PKG_VERSION"),
            &cfg.model,
            is_auth,
            is_local,
            update_notice.as_deref(),
            elapsed,
            chat_chunk.width as usize,
        )
    } else {
        render_gradient_logo(
            env!("CARGO_PKG_VERSION"),
            &cfg.model,
            is_auth,
            is_local,
            update_notice.as_deref(),
            elapsed,
        )
    };
    all_lines.extend(header_lines);

    // 2. Cached Messages & Tool Executions List
    if app.cached_render_width != content_max_width
        || app.cached_session_id != app.session.id
        || app.cached_logs_expanded != app.logs_expanded
    {
        app.cached_message_lines.clear();
        app.cached_message_count = 0;
        app.cached_render_width = content_max_width;
        app.cached_session_id = app.session.id.clone();
        app.cached_logs_expanded = app.logs_expanded;
    }

    let target_cache_count = if app.pending_confirmation.is_some() {
        app.session.messages.len().saturating_sub(1)
    } else {
        app.session.messages.len()
    };

    if app.cached_message_count > target_cache_count {
        app.cached_message_lines.clear();
        app.cached_message_count = 0;
    }

    if app.cached_message_count < target_cache_count {
        let start_idx = app.cached_message_count;
        for msg_idx in start_idx..target_cache_count {
            let msg = &app.session.messages[msg_idx];
            let next_msg = app.session.messages[..target_cache_count].get(msg_idx + 1);
            let rendered = render_single_message_with_pending(
                msg_idx,
                msg,
                next_msg,
                &app.theme,
                content_max_width,
                None,
                app.logs_expanded,
            );
            app.cached_message_lines.extend(rendered);
        }
        app.cached_message_count = target_cache_count;
    }

    all_lines.extend(app.cached_message_lines.iter().cloned());

    if target_cache_count < app.session.messages.len() {
        let msg_idx = target_cache_count;
        let msg = &app.session.messages[msg_idx];
        let next_msg = app.session.messages.get(msg_idx + 1);
        let rendered = render_single_message_with_pending(
            msg_idx,
            msg,
            next_msg,
            &app.theme,
            content_max_width,
            app.pending_confirmation.as_ref(),
            app.logs_expanded,
        );
        all_lines.extend(rendered);
    }

    // Render Tool Confirmation Dialog inline directly in the chat stream!
    if let Some(ref pending) = app.pending_confirmation {
        let diff_str = pending.diff_preview.as_deref().unwrap_or("");
        let tool_name = pending.calls.first().map(|c| c.function.name.as_str()).unwrap_or("tool");
        let conf_lines = build_tool_confirmation_lines(
            tool_name,
            diff_str,
            pending.selected_option,
            pending.calls.len(),
            content_max_width,
            &app.theme,
            pending.diff_expanded,
            pending.input_mode,
            &pending.feedback_text,
        );
        all_lines.extend(conf_lines);
    }

    if app.is_streaming {
        let elapsed = app.last_turn_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);

        if !app.streaming_text.is_empty() {
            let mut stream_lines = render_markdown(&app.streaming_text, &app.theme, content_max_width);
            stream_lines.push(Line::from(""));
            all_lines.extend(stream_lines);
        }

        if !app.streaming_tool_calls.is_empty() {
            let preview_lines = build_streaming_tool_preview_lines(
                &app.streaming_tool_calls,
                elapsed,
                content_max_width,
                &app.theme,
            );
            all_lines.extend(preview_lines);
        }
    }

    if !app.message_queue.is_empty() {
        all_lines.push(Line::from(""));
        for (i, queued_msg) in app.message_queue.iter().enumerate() {
            let label = if app.message_queue.len() == 1 {
                "[queued] ".to_string()
            } else {
                format!("[queued #{}] ", i + 1)
            };
            all_lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(label, Style::default().fg(app.theme.accent_yellow).add_modifier(Modifier::BOLD)),
                Span::styled(queued_msg.clone(), Style::default().fg(app.theme.foreground)),
                Span::styled(" (pending)", Style::default().fg(app.theme.dark_gray)),
            ]));
        }
        all_lines.push(Line::from(""));
    }

    let total_lines = all_lines.len();
    app.total_rendered_items = total_lines;

    let visible_height = (chat_chunk.height as usize).max(1);
    let max_scroll = total_lines.saturating_sub(visible_height) as u16;

    if app.auto_scroll {
        app.scroll_offset = max_scroll;
    } else {
        app.scroll_offset = app.scroll_offset.min(max_scroll);
    }

    app.last_chat_rect = Some(chat_chunk);

    // Resolving drag columns into characters needs the plain text of every rendered line.
    // Only paid for while a selection is live.
    app.selection_lines = if app.selection.is_some() {
        all_lines.iter().map(|line| line_plain_text(line)).collect()
    } else {
        Vec::new()
    };

    let message_paragraph = Paragraph::new(all_lines)
        .block(Block::default().borders(Borders::NONE))
        .scroll((app.scroll_offset, 0));
    frame.render_widget(message_paragraph, chat_chunk);

    // Painted straight onto the rendered cells: the feed is built from generated lines, so this
    // is the only place a selection can be highlighted without reworking the renderer.
    if let Some(selection) = app.selection {
        paint_selection(
            frame.buffer_mut(),
            chat_chunk,
            app.scroll_offset,
            &selection,
            &app.selection_lines,
            &app.theme,
        );
    }

    // 2.5. Fixed Activity / Status Bar (Directly above the Composer!)
    if show_activity {
        let spinner_frames = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
        let elapsed = app.last_turn_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
        let idx = ((elapsed * 10.0) as usize) % spinner_frames.len();
        let spinner_char = spinner_frames[idx];

        if app.status_transition.is_empty() {
            if app.thinking_state.is_streaming {
                app.set_status("Thinking...");
            } else if !app.streaming_text.is_empty() {
                app.set_status("Generating response...");
            } else {
                app.set_status("Generating...");
            }
        }

        let text_spans = app.status_transition.render_spans(&app.theme);

        let mut left_spans = vec![
            Span::raw(" "),
            Span::styled(spinner_char, Style::default().fg(app.theme.accent_purple)),
            Span::raw("  "),
        ];
        left_spans.extend(text_spans);

        let queue_count = app.message_queue.len();
        if queue_count > 0 {
            left_spans.push(Span::styled(
                format!(" · [{} queued]", queue_count),
                Style::default().fg(app.theme.accent_yellow).add_modifier(Modifier::BOLD),
            ));
        }

        let mut right_spans = Vec::new();
        if queue_count > 0 {
            right_spans.push(Span::styled("(Esc para descolar) ", Style::default().fg(app.theme.dark_gray)));
        } else {
            right_spans.push(Span::styled("(Esc para cancelar) ", Style::default().fg(app.theme.dark_gray)));
        }

        let left_len: usize = left_spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
        let right_len: usize = right_spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
        let bar_width = activity_chunk.width as usize;

        let mut spans = left_spans;
        if bar_width > left_len + right_len {
            let pad = bar_width - (left_len + right_len);
            spans.push(Span::raw(" ".repeat(pad)));
            spans.extend(right_spans);
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), activity_chunk);
    }

    // 3. Input Prompt Composer
    let show_esc_hint = app.last_esc_press
        .map(|i| i.elapsed() <= Duration::from_millis(500))
        .unwrap_or(false);

    // `cursor_row` is the wrapped row the caret sits on; it drives the vertical scroll so the
    // caret stays visible once the message occupies more rows than the composer can show.
    let (composer_lines, cursor_row) = if app.pending_confirmation.is_some() {
        (
            vec![Line::from(vec![
                prompt_prefix.clone(),
                Span::styled("(Press 1-3 or Enter to select, Esc to decline)", Style::default().fg(app.theme.dark_gray)),
            ])],
            0usize,
        )
    } else if app.input_buffer.is_empty() && !show_esc_hint {
        let is_mobile = is_mobile_portrait(size.width, size.height);
        let placeholder = if is_mobile {
            "Type message or /help"
        } else {
            "Type your message or @path/to/file"
        };
        (
            vec![Line::from(vec![
                prompt_prefix.clone(),
                Span::styled("█ ", Style::default().fg(app.theme.accent_blue)),
                Span::styled(placeholder, Style::default().fg(app.theme.dark_gray)),
            ])],
            0usize,
        )
    } else {
        let hint = show_esc_hint.then(|| {
            let msg = if app.input_buffer.is_empty() {
                "Press Esc again to rewind."
            } else {
                "Press Esc again to clear prompt."
            };
            Span::styled(format!(" ({})", msg), Style::default().fg(app.theme.gray))
        });

        let (input_spans, _) = format_input_with_cursor(&app.input_buffer, app.cursor_idx, &app.theme);

        let mut clamped = app.cursor_idx.min(app.input_buffer.len());
        while !app.input_buffer.is_char_boundary(clamped) {
            clamped = clamped.saturating_sub(1);
        }
        // Char offset of the caret inside the flattened stream (prefix chars + input chars).
        let cursor_flat =
            prompt_prefix.content.chars().count() + app.input_buffer[..clamped].chars().count();

        let mut spans: Vec<Span<'_>> = Vec::new();
        spans.push(prompt_prefix.clone());
        spans.extend(input_spans);
        if let Some(hint_span) = hint {
            spans.push(hint_span);
        }

        wrap_composer_spans(spans, size.width as usize, prefix_width, cursor_flat)
    };

    let visible_lines = composer_content_lines.max(1);
    let scroll_y = if cursor_row >= visible_lines {
        (cursor_row + 1 - visible_lines).min(composer_lines.len().saturating_sub(visible_lines))
    } else {
        0
    };

    let composer_border_color = if !app.auto_scroll {
        Color::Rgb(75, 110, 155)
    } else {
        app.theme.dark_gray
    };

    let composer_block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(composer_border_color));

    if app.user_dialog.is_open {
        render_user_dialog(frame, composer_chunk, &app.user_dialog, &app.theme);
    } else {
        frame.render_widget(
            Paragraph::new(composer_lines)
                .block(composer_block)
                .scroll((scroll_y as u16, 0)),
            composer_chunk,
        );
    }

    // Render slash command popup if active
    if let Some(p_chunk) = popup_chunk {
        let filter_text = if app.input_buffer.starts_with('/') {
            &app.input_buffer
        } else {
            &app.last_slash_filter
        };
        render_command_popup(frame, filter_text, app.slash_selected_idx, p_chunk, &app.theme);
    }

    // 4. Status Footer Bar (Left: Workspace Path | Right: Model Info)
    let cfg = app.llm_client.get_config();

    // Left spans: ONLY the workspace directory (and git branch)
    let mut left_spans = vec![
        Span::styled(format!(" {} ", app.shorten_path()), Style::default().fg(app.theme.foreground)),
    ];
    if !app.git_branch.is_empty() {
        left_spans.push(Span::styled(format!("({}) ", app.git_branch), Style::default().fg(app.theme.accent_cyan)));
    }
    if app.plan_mode {
        left_spans.push(Span::styled("[PLAN MODE] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    }

    let show_exit_warning = app.last_ctrl_c_press
        .map(|i| i.elapsed() <= std::time::Duration::from_secs(2))
        .unwrap_or(false);

    if show_exit_warning {
        left_spans.push(Span::styled("Press Ctrl+C again to exit. ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)));
    }

    // Right spans: Model info, temperature, reasoning
    let mut right_spans = Vec::new();
    let is_deepseek = cfg.is_deepseek_endpoint();
    let is_flash = is_deepseek && (cfg.model == "deepseek-flash" || cfg.model == "deepseek-v4.1-flash" || cfg.model == "deepseek-v4-flash" || cfg.model == "deepseek-chat" || cfg.model == "DeepSeek-V4.1-Flash" || cfg.model == "DeepSeek-V4-Flash");
    let is_pro = is_deepseek && (cfg.model == "deepseek-pro" || cfg.model == "deepseek-v4-pro" || cfg.model == "deepseek-reasoner" || cfg.model == "DeepSeek-V4-Pro");

    if is_flash {
        let temp = cfg.temperature;
        let temp_color = if temp <= 0.2 {
            Color::Rgb(79, 195, 247)
        } else if temp <= 0.5 {
            Color::Rgb(105, 240, 174)
        } else if temp <= 1.0 {
            Color::Rgb(255, 213, 79)
        } else if temp <= 1.5 {
            Color::Rgb(255, 152, 0)
        } else {
            Color::Rgb(244, 67, 54)
        };

        let effort = if (cfg.local_llm_enabled || cfg.reasoning_effort == "dynamic") && !cfg.flash_settings.code_reasoning_effort.is_empty() {
            &cfg.flash_settings.code_reasoning_effort
        } else {
            &cfg.reasoning_effort
        };

        let r_color = match effort.as_str() {
            "none" => Color::Rgb(158, 158, 158),
            "low" => Color::Rgb(79, 195, 247),
            "high" => Color::Rgb(255, 213, 79),
            "xhigh" => Color::Rgb(255, 110, 64),
            "max" => Color::Rgb(224, 64, 251),
            _ => app.theme.accent_cyan,
        };

        right_spans.push(Span::styled("DeepSeek-V4.1-Flash", Style::default().fg(app.theme.accent_purple).add_modifier(Modifier::BOLD)));
        right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
        right_spans.push(Span::styled(format!("{:.1}", temp), Style::default().fg(temp_color)));
        right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
        right_spans.push(Span::styled(format!("{} ", effort), Style::default().fg(r_color)));
    } else if is_pro {
        let effort = &cfg.pro_settings.reasoning_effort;
        let r_color = match effort.as_str() {
            "low" => Color::Rgb(105, 240, 174),
            "medium" => Color::Rgb(255, 152, 0),
            "high" => Color::Rgb(244, 67, 54),
            "max" => Color::Rgb(224, 64, 251),
            _ => app.theme.accent_cyan,
        };

        right_spans.push(Span::styled("DeepSeek-V4-Pro", Style::default().fg(app.theme.accent_purple).add_modifier(Modifier::BOLD)));
        right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
        right_spans.push(Span::styled(format!("{} ", effort), Style::default().fg(r_color)));
    } else if cfg.model.starts_with("local") {
        right_spans.push(Span::styled(format!("Local: {} ", cfg.local_llm_model), Style::default().fg(Color::Rgb(105, 240, 174)).add_modifier(Modifier::BOLD)));
        right_spans.push(Span::styled("· Offline @ $0.00 ", Style::default().fg(app.theme.gray)));
    } else {
        let display_name = {
            let all = cfg.all_providers();
            let prov = cfg.active_provider.as_deref().and_then(|p_name| all.iter().find(|p| p.name == p_name));
            prov.and_then(|p| p.models.iter().find(|m| m.id == cfg.model))
                .map(|m| m.display_name().to_string())
                .unwrap_or_else(|| cfg.model.clone())
        };

        let temp = cfg.temperature;
        let temp_color = if temp <= 0.2 {
            Color::Rgb(79, 195, 247)
        } else if temp <= 0.5 {
            Color::Rgb(105, 240, 174)
        } else if temp <= 1.0 {
            Color::Rgb(255, 213, 79)
        } else if temp <= 1.5 {
            Color::Rgb(255, 152, 0)
        } else {
            Color::Rgb(244, 67, 54)
        };

        right_spans.push(Span::styled(display_name, Style::default().fg(app.theme.accent_purple).add_modifier(Modifier::BOLD)));
        right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
        right_spans.push(Span::styled(format!("{:.1}", temp), Style::default().fg(temp_color)));
        if !cfg.reasoning_effort.is_empty() {
            right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
            right_spans.push(Span::styled(format!("{} ", cfg.reasoning_effort), Style::default().fg(app.theme.accent_cyan)));
        } else {
            right_spans.push(Span::raw(" "));
        }
    }

    if cfg.local_llm_enabled && !cfg.model.starts_with("local") {
        right_spans.push(Span::styled("+Local ", Style::default().fg(app.theme.accent_cyan).add_modifier(Modifier::BOLD)));
    }

    // Center spans: Local LLM status indicator and Active Background Tasks badge
    let mut center_spans = Vec::new();
    if cfg.local_llm_enabled || cfg.model.starts_with("local") {
        let is_local_online = app.local_llm_online.load(Ordering::Relaxed);
        let dot_color = if is_local_online {
            Color::Rgb(105, 240, 174) // Green (#69f0ae)
        } else {
            Color::Rgb(244, 67, 54)   // Red (#f44336)
        };
        center_spans.push(Span::styled("● ", Style::default().fg(dot_color)));
        center_spans.push(Span::styled(
            "Local LLM",
            Style::default().fg(if is_local_online { app.theme.foreground } else { app.theme.gray })
        ));
    }

    let active_tasks = if !app.active_background_pids.is_empty() {
        app.active_background_pids.len()
    } else {
        corex_tools::background::get_task_manager()
            .lock()
            .map(|m| m.active_count())
            .unwrap_or(0)
    };

    if active_tasks > 0 {
        if !center_spans.is_empty() {
            center_spans.push(Span::styled("  ·  ", Style::default().fg(app.theme.gray)));
        }
        let task_badge = if active_tasks == 1 {
            "[1 Task Running]".to_string()
        } else {
            format!("[{} Tasks Running]", active_tasks)
        };
        center_spans.push(Span::styled(
            task_badge,
            Style::default().fg(app.theme.gray),
        ));
    }

    if app.logs_expanded {
        if !center_spans.is_empty() {
            center_spans.push(Span::styled("  ·  ", Style::default().fg(app.theme.gray)));
        }
        center_spans.push(Span::styled(
            "[Logs: Full (Ctrl+O)]",
            Style::default()
                .fg(app.theme.accent_cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let is_mobile = is_mobile_portrait(size.width, size.height);
    if is_mobile {
        let footer_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(45),
                Constraint::Percentage(55),
            ])
            .split(status_chunk);

        frame.render_widget(Paragraph::new(Line::from(left_spans)).alignment(Alignment::Left), footer_cols[0]);
        let mobile_right = if active_tasks > 0 {
            center_spans
        } else {
            right_spans
        };
        frame.render_widget(Paragraph::new(Line::from(mobile_right)).alignment(Alignment::Right), footer_cols[1]);
    } else {
        let footer_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(33),
                Constraint::Percentage(34),
                Constraint::Percentage(33),
            ])
            .split(status_chunk);

        frame.render_widget(Paragraph::new(Line::from(left_spans)).alignment(Alignment::Left), footer_cols[0]);
        frame.render_widget(Paragraph::new(Line::from(center_spans)).alignment(Alignment::Center), footer_cols[1]);
        frame.render_widget(Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right), footer_cols[2]);
    }

    // 5. Sudo Authentication Dialog Modal if open
    if app.sudo_dialog.is_open {
        render_sudo_dialog(frame, size, &app.sudo_dialog, &app.theme);
    }

    // 6. Model Dialog Modal if open
    if app.model_dialog.is_open {
        render_model_dialog(frame, size, &app.model_dialog, &app.theme);
    }

    // 7. Auth Dialog Modal if open
    if app.auth_dialog.is_open {
        render_auth_dialog(frame, size, &app.auth_dialog, &app.theme);
    }

    // 8. Session Dialog Modal if open
    if app.session_dialog.is_open {
        render_session_dialog(frame, size, &mut app.session_dialog, &app.theme);
    }
}



/// Highlights the selected cells of the chat viewport.
///
/// The feed is drawn from generated lines, so rather than threading selection state through the
/// renderer this simply restyles the cells that already reached the screen.
pub fn paint_selection(
    buffer: &mut Buffer,
    chat: Rect,
    scroll_offset: u16,
    selection: &Selection,
    lines: &[String],
    theme: &Theme,
) {
    use unicode_width::UnicodeWidthStr;

    let ((start_line, start_col), (end_line, end_col)) = selection.ordered();
    let scroll = scroll_offset as usize;
    // Only the background is touched: the text keeps its own colour, so a selection reads like a
    // native terminal one instead of flattening the feed into a slab of accent colour.
    let style = Style::default().bg(theme.selection_bg);

    for line_idx in start_line..=end_line {
        if line_idx < scroll {
            continue;
        }
        let row_in_view = line_idx - scroll;
        if row_in_view >= chat.height as usize {
            break;
        }
        let y = chat.y + row_in_view as u16;

        // Middle lines are selected whole, but only up to the width of their own text:
        // highlighting the padding out to the viewport edge would look like a block of blanks.
        let text_width = lines
            .get(line_idx)
            .map(|line| line.width() as u16)
            .unwrap_or(chat.width);
        let from = if line_idx == start_line { start_col } else { 0 };
        let to = if line_idx == end_line {
            end_col
        } else {
            chat.width.saturating_sub(1)
        };
        let to = to.min(text_width.saturating_sub(1));

        for col in from..=to {
            if col >= chat.width {
                break;
            }
            if let Some(cell) = buffer.cell_mut((chat.x + col, y)) {
                cell.set_style(style);
            }
        }
    }
}

