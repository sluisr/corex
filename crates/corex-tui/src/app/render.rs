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
use crate::fleet_dialog::render_fleet_dialog;
use crate::markdown::render_markdown;
use crate::model_dialog::render_model_dialog;
use crate::session_dialog::render_session_dialog;
use crate::slash_commands::render_command_popup;
use crate::sudo_dialog::render_sudo_dialog;
use crate::theme::Theme;
use crate::user_dialog::render_user_dialog;

use super::formatting::*;
use super::mobile::is_mobile_portrait;
use super::state::{App, PendingToolBatch, Selection, SelectionLines};

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
    // Open gutter instead of a closed box: same visual language as the tool and log blocks, and
    // no right border to drift on narrow terminals.
    let inner_width = (content_max_width.saturating_sub(6)).clamp(44, 72);
    let label_w = 23;
    let usable_w = inner_width.saturating_sub(4);

    let gutter = || Span::styled("  │ ", Style::default().fg(theme.border_dim));

    let make_row = |label: &str, val_spans: Vec<Span<'static>>| -> Line<'static> {
        let mut line_spans = vec![
            gutter(),
            Span::styled(format!("{:<width$}", label, width = label_w), Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
        ];
        line_spans.extend(val_spans);
        Line::from(line_spans)
    };

    let empty_row = || -> Line<'static> {
        Line::from(Span::styled("  │", Style::default().fg(theme.border_dim)))
    };

    let title_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("Corex v{}", version), Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
    ]);

    let section_line = Line::from(vec![
        gutter(),
        Span::styled("Session Telemetry", Style::default().fg(theme.muted).add_modifier(Modifier::BOLD)),
    ]);

    let local_status = if local_enabled { "(Active)" } else { "(Disabled)" };
    let local_color = if local_enabled { theme.success } else { theme.muted };

    let val_max = usable_w.saturating_sub(label_w + 1);

    // The author's site only follows the name when it fits: a row must never wrap past the card.
    let creator = "sluisr";
    let creator_site = " (https://sluisr.com)";
    let mut creator_spans = vec![Span::styled(creator, Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD))];
    if creator.len() + creator_site.len() <= val_max {
        creator_spans.push(Span::styled(creator_site, Style::default().fg(theme.link)));
    }
    let link_row = |label: &str, url: &str| make_row(label, vec![
        Span::styled(corex_core::truncate_ellipsis(url, val_max), Style::default().fg(theme.link)),
    ]);

    vec![
        title_line,
        empty_row(),
        make_row("Creator & Maintainer", creator_spans),
        link_row("Official Website", "https://corex.sluisr.com"),
        link_row("Discord Community", "https://discord.gg/MGMt6M6Yx"),
        link_row("Changelog & Releases", "https://corex.sluisr.com/changelog"),
        link_row("Report Issues & Bugs", "https://github.com/sluisr/corex/issues"),
        link_row("GitHub Repository", "https://github.com/sluisr/corex"),
        empty_row(),
        section_line,
        empty_row(),
        make_row("Active Model", vec![
            Span::styled(corex_core::truncate_ellipsis(model, val_max), Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
        ]),
        make_row("API Base URL", vec![
            Span::styled(corex_core::truncate_ellipsis(base_url, val_max), Style::default().fg(theme.gray)),
        ]),
        make_row("Local SLM Engine", vec![
            Span::styled(format!("{} ", corex_core::truncate_ellipsis(local_engine, val_max.saturating_sub(12))), Style::default().fg(theme.gray)),
            Span::styled(local_status, Style::default().fg(local_color)),
        ]),
        make_row("Session ID", vec![
            Span::styled(corex_core::truncate_ellipsis(session_id, val_max), Style::default().fg(theme.muted)),
        ]),
        make_row("Workspace", vec![
            Span::styled(corex_core::truncate_ellipsis(workspace, val_max), Style::default().fg(theme.accent_green)),
        ]),
        make_row("Git Branch", vec![
            Span::styled(corex_core::truncate_ellipsis(branch, val_max), Style::default().fg(theme.accent_yellow)),
        ]),
        empty_row(),
    ]
}

/// Soft hint shown after a message typed while the agent is busy (leading space is part of it).
const BUSY_INPUT_HINT: &str = " · Enter queues · Alt+Enter sends now";

/// Rows of a command/subagent log that stay visible while logs are collapsed.
const COLLAPSED_LOG_ROWS: usize = 5;

/// Appends `rows` as a gutter-prefixed log: all of it when `expanded`, otherwise only the last
/// `COLLAPSED_LOG_ROWS` behind a `… (+N lines above)` marker (which `is_collapsed_marker` finds).
fn push_log_tail(
    lines: &mut Vec<Line<'static>>,
    rows: &[&str],
    expanded: bool,
    width: usize,
    gutter: &str,
    text: Color,
    dim: Color,
) {
    let skipped = if expanded { 0 } else { rows.len().saturating_sub(COLLAPSED_LOG_ROWS) };
    if skipped > 0 {
        lines.push(Line::from(vec![
            Span::styled(gutter.to_string(), Style::default().fg(dim)),
            Span::styled(format!("… (+{} lines above)", skipped), Style::default().fg(dim)),
        ]));
    }
    for row in &rows[skipped..] {
        let spans = vec![Span::styled(row.replace('\t', "    "), Style::default().fg(text))];
        lines.extend(crate::markdown::wrap_spans(spans, width, gutter));
    }
}

/// Finds the `(name, arguments)` of the call a tool message answers by matching its
/// `tool_call_id` against the assistant turns before it.
fn answered_tool_call(messages: &[Message], idx: usize) -> Option<(&str, &str)> {
    let msg = messages.get(idx)?;
    if msg.role != "tool" {
        return None;
    }
    let id = msg.tool_call_id.as_deref()?;
    messages[..idx]
        .iter()
        .rev()
        .filter(|m| m.role == "assistant")
        .take(16)
        .find_map(|m| {
            m.tool_calls
                .as_ref()?
                .iter()
                .find(|c| c.id == id)
                .map(|c| (c.function.name.as_str(), c.function.arguments.as_str()))
        })
}

/// The `… (+N lines above)` row a collapsed log leaves behind.
fn is_collapsed_marker(line: &Line<'_>) -> bool {
    line.spans.iter().any(|s| s.content.starts_with("… (+") && s.content.ends_with("lines above)"))
}

/// Lowercase role tag shared by every subagent badge (`[scout:sub_1]`), whichever path produced it.
fn subagent_role_label(role: &str) -> String {
    let role = role.trim().to_lowercase();
    if role.is_empty() || role == "subagent" { "agent".to_string() } else { role }
}

/// True only for text carrying block-level markdown (a heading, a fence or a rule on its own
/// line): a stray `#12` or `a---b` inside a sentence must not turn a notice into a heading.
fn looks_like_markdown(text: &str) -> bool {
    text.lines().map(str::trim_start).any(|line| {
        let hashes = line.chars().take_while(|&c| c == '#').count();
        (1..=6).contains(&hashes) && line[hashes..].starts_with(' ')
            || line.starts_with("```")
            || (line.len() >= 3 && line.trim_end().chars().all(|c| c == '-'))
    })
}

/// The kinds of system message the feed draws specially. The producers emit them as plain text
/// (`TASK_DONE|pid|code|time`, `Error: ...`), so this is the single place that knows those
/// prefixes: the renderer and the spacing logic both go through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemNotice {
    InfoCard,
    TaskDone,
    SubagentDone,
    ModelChange,
    Error,
    Checkpoint,
    Session,
    Plain,
}

impl SystemNotice {
    fn classify(text: &str) -> Self {
        let first = text.lines().next().unwrap_or("").trim();
        if first.starts_with("COREX_INFO_CARD|") {
            Self::InfoCard
        } else if first.starts_with("TASK_DONE|") || first.starts_with("[TASK FINISHED]") {
            Self::TaskDone
        } else if first.starts_with("SUBAGENT_DONE|") || first.starts_with("SUBAGENTDONE|") {
            Self::SubagentDone
        } else if first.starts_with("Activated ") || first.starts_with("Switched ") {
            Self::ModelChange
        } else if first.starts_with("Error") || first.starts_with("Failed") {
            Self::Error
        } else if first.contains("checkpoint saved") || first.contains("saved with tag") {
            Self::Checkpoint
        } else if first.starts_with("Resumed session") || first.starts_with("Started new chat") {
            Self::Session
        } else {
            Self::Plain
        }
    }

    /// Background-work completion badges stack tightly, without a blank row between them.
    fn is_completion_badge(self) -> bool {
        matches!(self, Self::TaskDone | Self::SubagentDone)
    }
}

pub fn format_system_message(text: &str, theme: &Theme, content_max_width: usize, logs_expanded: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let raw_lines: Vec<&str> = text.lines().collect();

    if raw_lines.is_empty() {
        return lines;
    }

    let first_line = raw_lines[0].trim();
    let notice = SystemNotice::classify(text);

    if notice == SystemNotice::InfoCard {
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

    let muted_dim = theme.muted;
    let muted_label = theme.muted;
    let muted_text = theme.gray;
    let muted_tag = theme.muted;

    if notice == SystemNotice::TaskDone {
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
            ("  ■ ", theme.warning, "stopped".to_string(), theme.warning)
        } else if is_ok {
            ("  ✓ ", theme.success, "finished (exit 0)".to_string(), theme.success)
        } else {
            let label = format!("finished (exit {})", exit_code_str);
            ("  ✕ ", theme.error, label, theme.error)
        };

        // Output lines
        let output_lines: Vec<&str> = raw_lines[1..]
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();

        let mut badge_spans = vec![
            Span::styled(icon, Style::default().fg(icon_color)),
            Span::styled(format!("[bg:{}] ", pid), Style::default().fg(muted_tag)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ];
        if !duration.trim().is_empty() {
            badge_spans.push(Span::styled(format!(" ({})", duration.trim()), Style::default().fg(muted_dim)));
        }
        lines.push(Line::from(badge_spans));

        push_log_tail(&mut lines, &output_lines, logs_expanded, content_max_width, "    │ ", muted_text, muted_dim);

        return lines;
    }

    if notice == SystemNotice::SubagentDone {
        let parts: Vec<&str> = first_line.split('|').collect();
        let sub_id = parts.get(1).copied().unwrap_or("?");
        let role_str = parts.get(2).copied().unwrap_or("SUBAGENT");
        let dur = parts.get(3).copied().unwrap_or("");

        let role_label = subagent_role_label(role_str);

        let output_lines: Vec<&str> = raw_lines[1..]
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();

        let is_err = output_lines.first().map(|l| l.starts_with("Exit code ") || l.starts_with("Failed")).unwrap_or(false);

        let (icon, status_text, status_color) = if is_err {
            ("  ✕ ", "failed".to_string(), theme.error)
        } else {
            ("  ✓ ", "completed".to_string(), theme.success)
        };

        let mut badge_spans = vec![
            Span::styled(icon, Style::default().fg(status_color)),
            Span::styled(format!("[{}:{}] ", role_label, sub_id), Style::default().fg(muted_tag)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ];
        if !dur.trim().is_empty() {
            badge_spans.push(Span::styled(format!(" ({})", dur.trim()), Style::default().fg(muted_dim)));
        }
        lines.push(Line::from(badge_spans));

        push_log_tail(&mut lines, &output_lines, logs_expanded, content_max_width, "    │ ", muted_text, muted_dim);

        return lines;
    }

    if notice == SystemNotice::ModelChange {
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
                        "dynamic" | "low" | "enabled" => theme.success,
                        "medium" => theme.warning,
                        "high" | "max" => theme.accent_purple,
                        _ => theme.gray,
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
                lines.extend(crate::markdown::wrap_spans_with_indent(spans, content_max_width, "", "    "));
            }
        }
        return lines;
    }

    if notice == SystemNotice::Error {
        let err_spans = vec![
            Span::styled("  ✕ ", Style::default().fg(theme.error)),
            Span::styled("[error] ", Style::default().fg(theme.error)),
            Span::styled(first_line.to_string(), Style::default().fg(muted_text)),
        ];
        lines.extend(crate::markdown::wrap_spans_with_indent(err_spans, content_max_width, "", "    "));
        for sub_line in &raw_lines[1..] {
            let spans = vec![
                Span::styled("    ", Style::default()),
                Span::styled(sub_line.to_string(), Style::default().fg(muted_label)),
            ];
            lines.extend(crate::markdown::wrap_spans_with_indent(spans, content_max_width, "", "    "));
        }
        return lines;
    }

    if notice == SystemNotice::Checkpoint {
        let save_spans = vec![
            Span::styled("  ✓ ", Style::default().fg(theme.success)),
            Span::styled("[checkpoint] ", Style::default().fg(theme.success)),
            Span::styled(first_line.to_string(), Style::default().fg(muted_text)),
        ];
        lines.extend(crate::markdown::wrap_spans_with_indent(save_spans, content_max_width, "", "    "));
        return lines;
    }

    if notice == SystemNotice::Session {
        let session_spans = vec![
            Span::styled("  · ", Style::default().fg(muted_dim)),
            Span::styled("[session] ", Style::default().fg(muted_tag)),
            Span::styled(first_line.to_string(), Style::default().fg(muted_text)),
        ];
        lines.extend(crate::markdown::wrap_spans_with_indent(session_spans, content_max_width, "", "    "));
        return lines;
    }

    if looks_like_markdown(text) {
        return crate::markdown::render_markdown(text, theme, content_max_width);
    }

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
        lines.extend(crate::markdown::wrap_spans_with_indent(sys_spans, content_max_width, "", "    "));
        is_first = false;
    }

    lines
}

pub fn render_single_message_with_pending(
    msg_idx: usize,
    msg: &Message,
    next_msg: Option<&Message>,
    theme: &Theme,
    content_max_width: usize,
    pending_conf: Option<&PendingToolBatch>,
    logs_expanded: bool,
) -> Vec<Line<'static>> {
    render_single_message_with_pending_and_thinking(
        msg_idx,
        msg,
        next_msg,
        theme,
        content_max_width,
        pending_conf,
        logs_expanded,
        logs_expanded,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_single_message_with_pending_and_thinking(
    _msg_idx: usize,
    msg: &Message,
    next_msg: Option<&Message>,
    theme: &Theme,
    content_max_width: usize,
    pending_conf: Option<&PendingToolBatch>,
    logs_expanded: bool,
    thinking_expanded: bool,
    // `(name, arguments)` of the call a tool message answers. Tool responses carry no name of
    // their own, so without it the renderer would have to guess the tool from the output text.
    tool_call: Option<(&str, &str)>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match msg.role.as_str() {
        "user" => {
            let content = msg.text_content().unwrap_or("");
            for (idx, line) in content.split('\n').enumerate() {
                let (spans, first_indent) = if idx == 0 {
                    (
                        vec![
                            Span::styled("[", Style::default().fg(theme.border_dim)),
                            Span::styled("You", Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
                            Span::styled("] ", Style::default().fg(theme.border_dim)),
                            Span::styled(line.to_string(), Style::default().fg(theme.foreground)),
                        ],
                        "  ",
                    )
                } else {
                    (
                        vec![
                            Span::styled(line.to_string(), Style::default().fg(theme.foreground)),
                        ],
                        "        ",
                    )
                };
                let wrapped = crate::markdown::wrap_spans_with_indent(spans, content_max_width, first_indent, "        ");
                lines.extend(wrapped);
            }
            lines.push(Line::from(""));
        }
        "assistant" => {
            if let Some(ref cot) = msg.reasoning_content {
                let trimmed_cot = cot.trim();
                if !trimmed_cot.is_empty() {
                    if thinking_expanded {
                        lines.push(Line::from(vec![
                            Span::styled("  ▾ ", Style::default().fg(theme.accent_purple)),
                            Span::styled("Thought ", Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
                            Span::styled("(Ctrl+T to collapse)", Style::default().fg(theme.dark_gray)),
                        ]));
                        for cot_line in trimmed_cot.lines() {
                            let clean = cot_line.replace('\t', "    ");
                            let spans = vec![Span::styled(clean, Style::default().fg(theme.gray).add_modifier(Modifier::ITALIC))];
                            lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    │ "));
                        }
                        lines.push(Line::from(""));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled("  · ", Style::default().fg(theme.accent_purple)),
                            Span::styled("Thought ", Style::default().fg(theme.accent_purple)),
                            Span::styled("(Ctrl+T to expand)", Style::default().fg(theme.dark_gray)),
                        ]));
                        lines.push(Line::from(""));
                    }
                }
            }

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

            let tool_name = tool_call.map(|(name, _)| name).or(msg.name.as_deref());

            // Skills are internal procedural knowledge loaded for the AI — suppress raw log dumping.
            // The name is authoritative; the prefixes only cover sessions saved without it.
            let is_skill = match tool_name {
                Some(name) => name == "activate_skill",
                None => content.starts_with("---\nname:") || content.starts_with("# Corex — "),
            };

            if is_skill {
                let is_err = content.starts_with("Error") || content.starts_with("Failed") || content.starts_with("Skill '");
                if is_err {
                    let line_spans = vec![
                        Span::styled("    ✕ ", Style::default().fg(theme.error)),
                        Span::styled(content.lines().next().unwrap_or("Failed to load skill").to_string(), Style::default().fg(theme.gray)),
                    ];
                    lines.extend(crate::markdown::wrap_spans_with_indent(line_spans, content_max_width, "", "      "));
                }
                if is_last_tool {
                    lines.push(Line::from(""));
                }
                return lines;
            }

            let denial = content
                .strip_prefix("Tool execution denied by user")
                .or_else(|| content.strip_prefix("Execution denied by user"));
            if let Some(rest) = denial {
                let mut line_spans = vec![
                    Span::styled("    ✕ ", Style::default().fg(theme.error)),
                    Span::styled("Execution declined by user", Style::default().fg(theme.gray)),
                ];
                if let Some(feedback) = rest.strip_prefix(" with instruction:").map(str::trim).filter(|f| !f.is_empty()) {
                    line_spans.push(Span::styled(format!(": {}", feedback), Style::default().fg(theme.muted)));
                }
                let wrapped = crate::markdown::wrap_spans_with_indent(line_spans, content_max_width, "", "      ");
                lines.extend(wrapped);
            } else if content.starts_with("Successfully edited") && content.contains('\n') {
                let mut content_lines = content.lines();
                let header = content_lines.next().unwrap_or("Successfully edited");
                let header_spans = vec![
                    Span::styled("    ✓ ", Style::default().fg(theme.success)),
                    Span::styled(header.to_string(), Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
                ];
                let wrapped_header = crate::markdown::wrap_spans_with_indent(header_spans, content_max_width, "", "      ");
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
                let wrapped = crate::markdown::wrap_spans_with_indent(line_spans, content_max_width, "", "      ");
                lines.extend(wrapped);
            } else if content.starts_with("Task ID: sub_") {
                let sub_id = content.lines().find(|l| l.starts_with("Task ID:")).and_then(|l| l.split(':').nth(1)).map(|s| s.trim()).unwrap_or("?");
                let role = content.lines().find(|l| l.starts_with("Role:")).and_then(|l| l.split(':').nth(1)).map(|s| s.trim()).unwrap_or("subagent");
                let line_spans = vec![
                    Span::styled("    · ", Style::default().fg(theme.accent_purple)),
                    Span::styled(format!("[{}:{}] ", subagent_role_label(role), sub_id), Style::default().fg(theme.accent_purple)),
                    Span::styled("running in background...", Style::default().fg(theme.gray)),
                ];
                let wrapped = crate::markdown::wrap_spans_with_indent(line_spans, content_max_width, "", "      ");
                lines.extend(wrapped);
            } else if content.starts_with("Subagent '") && (content.contains("' completed:") || content.contains("' failed:")) {
                let is_err = content.contains("' failed:");
                let first_line = content.lines().next().unwrap_or("");
                let sub_id = first_line.split('\'').nth(1).unwrap_or("?");
                let (status_icon, status_color, status_text) = if is_err {
                    ("✕ ", theme.error, "failed")
                } else {
                    ("✓ ", theme.success, "completed")
                };
                let role = tool_call
                    .and_then(|(_, args)| serde_json::from_str::<serde_json::Value>(args).ok())
                    .and_then(|v| v.get("role").and_then(|r| r.as_str()).map(subagent_role_label))
                    .unwrap_or_else(|| "agent".to_string());

                let report_body = content.split_once(":\n\n")
                    .map(|(_, b)| b)
                    .or_else(|| content.split_once(":\n").map(|(_, b)| b))
                    .unwrap_or(content);

                let raw_lines: Vec<&str> = report_body.lines().filter(|l| !l.trim().is_empty()).collect();

                let badge_spans = vec![
                    Span::styled(format!("    {}", status_icon), Style::default().fg(status_color)),
                    Span::styled(format!("[{}:{}] ", role, sub_id), Style::default().fg(theme.accent_purple)),
                    Span::styled(status_text, Style::default().fg(status_color)),
                ];
                lines.push(Line::from(badge_spans));

                push_log_tail(&mut lines, &raw_lines, logs_expanded, content_max_width, "        │ ", theme.gray, theme.dark_gray);
            } else {
                let raw_output_lines: Vec<&str> = content
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .collect();

                let is_err = content.starts_with("Error")
                    || content.starts_with("Failed")
                    || content.starts_with("Command exited with status code");

                let icon = if is_err { "    ✕ " } else { "    ✓ " };
                let icon_color = if is_err { theme.error } else { theme.success };

                if raw_output_lines.len() <= 1 {
                    let first_line_raw = raw_output_lines.first().copied().unwrap_or("Done").replace('\t', "    ");
                    let display_first_line = truncate_to_width(&first_line_raw, 140);

                    let line_spans = vec![
                        Span::styled(icon, Style::default().fg(icon_color)),
                        Span::styled(display_first_line, Style::default().fg(theme.gray)),
                    ];
                    let wrapped = crate::markdown::wrap_spans_with_indent(line_spans, content_max_width, "", "      ");
                    lines.extend(wrapped);
                } else {
                    let total = raw_output_lines.len();
                    let header_label = if is_err {
                        format!("Command failed ({} lines)", total)
                    } else {
                        format!("Output ({} lines)", total)
                    };

                    lines.push(Line::from(vec![
                        Span::styled(icon, Style::default().fg(icon_color)),
                        Span::styled(header_label, Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
                    ]));

                    push_log_tail(&mut lines, &raw_output_lines, logs_expanded, content_max_width, "        │ ", theme.gray, theme.dark_gray);
                }
            }

            if is_last_tool {
                lines.push(Line::from(""));
            }
        }
        "system" => {
            if let Some(text) = msg.text_content() {
                lines.extend(format_system_message(text, theme, content_max_width, logs_expanded));
                let next_is_badge = next_msg
                    .and_then(|m| if m.role.as_str() == "system" { m.text_content() } else { None })
                    .is_some_and(|t| SystemNotice::classify(t).is_completion_badge());

                if !SystemNotice::classify(text).is_completion_badge() || !next_is_badge {
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

    // Fleet / SysAdmin mode: normal AI chat on one side, live VPS terminal on the other.
    if app.fleet_dialog.is_open {
        // One divider colour for every fleet layout; the live terminal lights up when focused.
        let divider = app.theme.rule;
        let border = Style::default().fg(divider);

        // Wide terminals: chat | live output | fleet-wide telemetry.
        if app.fleet_dialog.wide_layout && size.width >= crate::fleet_dialog::FLEET_WIDE_MIN_WIDTH {
            let cols = Layout::horizontal([
                Constraint::Percentage(40),
                Constraint::Percentage(35),
                Constraint::Percentage(25),
            ])
            .split(size);
            render_chat_ui(frame, app, cols[0]);

            let out_block = Block::default().borders(Borders::LEFT).border_style(border);
            let out_inner = out_block.inner(cols[1]);
            frame.render_widget(out_block, cols[1]);
            crate::fleet_dialog::render_fleet_output(frame, out_inner, &app.fleet_dialog, &app.theme);

            let fleet_block = Block::default().borders(Borders::LEFT).border_style(border);
            let fleet_inner = fleet_block.inner(cols[2]);
            frame.render_widget(fleet_block, cols[2]);
            crate::fleet_dialog::render_fleet_column(frame, fleet_inner, &app.fleet_dialog, &app.theme);
            return;
        }

        if size.width < 110 {
            // Narrow/Mobile screens (e.g. mobile portrait, narrow terminals):
            // Instead of crushing chat and terminal into a 50/50 vertical split with 0 usable terminal lines,
            // render the active focused pane in FULL SCREEN!
            // Tab / Shift+Tab toggles between Chat and VPS Terminal.
            if app.fleet_dialog.command_mode {
                render_fleet_dialog(frame, size, &app.fleet_dialog, &app.theme);
            } else {
                render_chat_ui(frame, app, size);
            }
            return;
        }

        let h = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(size);
        let chat_area = h[0];
        let term_area = h[1];

        render_chat_ui(frame, app, chat_area);

        let border_color = if app.fleet_dialog.command_mode {
            app.theme.accent_cyan
        } else {
            divider
        };
        let term_block = Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(border_color));
        let inner = term_block.inner(term_area);
        frame.render_widget(term_block, term_area);
        render_fleet_dialog(frame, inner, &app.fleet_dialog, &app.theme);
        return;
    }

    render_chat_ui(frame, app, size);
}

fn render_chat_ui(frame: &mut Frame, app: &mut App, size: Rect) {
    if app.input_buffer.starts_with('/') {
        app.last_slash_filter = app.input_buffer.clone();
    }

    app.advance_slash_popup();

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
        Span::styled("* ", Style::default().fg(app.theme.error).add_modifier(Modifier::BOLD))
    } else if app.plan_mode {
        Span::styled("? ", Style::default().fg(app.theme.warning).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("> ", Style::default().fg(app.theme.accent_blue).add_modifier(Modifier::BOLD))
    };
    let prefix_width = prompt_prefix.width();

    const MAX_COMPOSER_LINES: usize = 6;
    let composer_content_lines = {
        let cursor_at_end = app.cursor_idx >= app.input_buffer.len();
        let capacity = (size.width as usize).saturating_sub(prefix_width).max(1);
        if app.pending_confirmation.is_some() || app.input_buffer.is_empty() {
            1
        } else {
            // Each line break starts a new row (messages taken back from the queue can hold
            // several), and the caret needs a cell of its own when it sits at the very end.
            let last = app.input_buffer.split('\n').count() - 1;
            app.input_buffer
                .split('\n')
                .enumerate()
                .map(|(i, line)| {
                    let width = unicode_width::UnicodeWidthStr::width(line)
                        + usize::from(i == last && cursor_at_end);
                    width.div_ceil(capacity).max(1)
                })
                .sum::<usize>()
                .clamp(1, MAX_COMPOSER_LINES)
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

    // 1. Header Banner (First item in scrollable feed!)
    // Config is snapshotted once per frame and reused by the status footer below.
    let cfg = app.llm_client.get_config();
    let is_auth = !cfg.api_key.trim().is_empty();
    let is_local = cfg.local_llm_enabled;
    let update_notice = app.update_available.lock().ok().and_then(|l| l.clone());
    let header_lines: Vec<Line<'static>> = if is_mobile_portrait(size.width, size.height) {
        render_gradient_logo_adaptive(
            env!("CARGO_PKG_VERSION"),
            &cfg.model,
            is_auth,
            is_local,
            update_notice.as_deref(),
            chat_chunk.width as usize,
        )
    } else {
        render_gradient_logo(
            env!("CARGO_PKG_VERSION"),
            &cfg.model,
            is_auth,
            is_local,
            update_notice.as_deref(),
        )
    };

    // 2. Cached Messages & Tool Executions List
    if app.cached_render_width != content_max_width
        || app.cached_session_id != app.session.id
        || app.cached_logs_expanded != app.logs_expanded
    {
        app.cached_message_lines.clear();
        app.cached_message_line_offsets.clear();
        app.selection_lines.cached.clear();
        app.cached_message_count = 0;
        app.cached_has_collapsed_logs = false;
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
        let keep_lines = if target_cache_count < app.cached_message_line_offsets.len() {
            app.cached_message_line_offsets[target_cache_count]
        } else {
            0
        };
        app.cached_message_lines.truncate(keep_lines);
        app.cached_message_line_offsets.truncate(target_cache_count + 1);
        app.selection_lines.cached.truncate(keep_lines);
        app.cached_message_count = target_cache_count;
    }

    if app.cached_message_count < target_cache_count {
        if app.cached_message_line_offsets.is_empty() {
            app.cached_message_line_offsets.push(0);
        }
        let start_idx = app.cached_message_count;
        for msg_idx in start_idx..target_cache_count {
            let msg = &app.session.messages[msg_idx];
            let next_msg = app.session.messages[..target_cache_count].get(msg_idx + 1);
            let rendered = render_single_message_with_pending_and_thinking(
                msg_idx,
                msg,
                next_msg,
                &app.theme,
                content_max_width,
                None,
                app.logs_expanded,
                app.thinking_state.is_expanded,
                answered_tool_call(&app.session.messages, msg_idx),
            );
            if !app.logs_expanded && rendered.iter().any(is_collapsed_marker) {
                app.cached_has_collapsed_logs = true;
            }
            app.cached_message_lines.extend(rendered);
            app.cached_message_line_offsets.push(app.cached_message_lines.len());
        }
        app.cached_message_count = target_cache_count;
    }

    // Keep the plain-text mirror of the cached history in sync (incremental, never per-frame full rebuild).
    if app.selection_lines.cached.len() > app.cached_message_lines.len() {
        app.selection_lines.cached.clear();
    }
    if app.selection_lines.cached.len() < app.cached_message_lines.len() {
        let from = app.selection_lines.cached.len();
        let new_plain: Vec<String> = app.cached_message_lines[from..]
            .iter()
            .map(line_plain_text)
            .collect();
        app.selection_lines.cached.extend(new_plain);
    }

    // 3. Live tail (uncached pending message, confirmation dialog, streaming output, queue)
    let mut tail_lines: Vec<Line<'static>> = Vec::new();

    if target_cache_count < app.session.messages.len() {
        let msg_idx = target_cache_count;
        let msg = &app.session.messages[msg_idx];
        let next_msg = app.session.messages.get(msg_idx + 1);
        let rendered = render_single_message_with_pending_and_thinking(
            msg_idx,
            msg,
            next_msg,
            &app.theme,
            content_max_width,
            app.pending_confirmation.as_ref(),
            app.logs_expanded,
            app.thinking_state.is_expanded,
            answered_tool_call(&app.session.messages, msg_idx),
        );
        tail_lines.extend(rendered);
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
            pending.current_idx,
            pending.total_count,
            content_max_width,
            &app.theme,
            pending.diff_expanded,
            pending.input_mode,
            &pending.feedback_text,
        );
        tail_lines.extend(conf_lines);
    }

    if app.is_streaming {
        let elapsed = app.last_turn_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);

        if !app.thinking_state.content.trim().is_empty() {
            if app.thinking_state.is_expanded {
                tail_lines.push(Line::from(vec![
                    Span::styled("  ▾ ", Style::default().fg(app.theme.accent_purple)),
                    Span::styled("Thought", Style::default().fg(app.theme.accent_purple).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" · {:.1}s ", elapsed), Style::default().fg(app.theme.gray)),
                    Span::styled("(Ctrl+T to collapse)", Style::default().fg(app.theme.dark_gray)),
                ]));
                for cot_line in app.thinking_state.content.trim().lines() {
                    let clean = cot_line.replace('\t', "    ");
                    let spans = vec![Span::styled(clean, Style::default().fg(app.theme.gray).add_modifier(Modifier::ITALIC))];
                    tail_lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    │ "));
                }
                tail_lines.push(Line::from(""));
            } else {
                tail_lines.push(Line::from(vec![
                    Span::styled("  · ", Style::default().fg(app.theme.accent_purple)),
                    Span::styled("Thought", Style::default().fg(app.theme.accent_purple)),
                    Span::styled(format!(" · {:.1}s ", elapsed), Style::default().fg(app.theme.gray)),
                    Span::styled("(Ctrl+T to expand)", Style::default().fg(app.theme.dark_gray)),
                ]));
                tail_lines.push(Line::from(""));
            }
        }

        if !app.streaming_text.is_empty() {
            // Re-parse markdown only when the streamed text or width actually changed.
            let cache_valid = app.streaming_md_cache_len == app.streaming_text.len()
                && app.streaming_md_cache_width == content_max_width
                && !app.streaming_md_cache.is_empty();
            if !cache_valid {
                app.streaming_md_cache = render_markdown(&app.streaming_text, &app.theme, content_max_width);
                app.streaming_md_cache_len = app.streaming_text.len();
                app.streaming_md_cache_width = content_max_width;
            }
            tail_lines.extend(app.streaming_md_cache.iter().cloned());
            tail_lines.push(Line::from(""));
        } else if !app.streaming_md_cache.is_empty() {
            app.streaming_md_cache = Vec::new();
            app.streaming_md_cache_len = 0;
        }

        if !app.streaming_tool_calls.is_empty() {
            let preview_lines = build_streaming_tool_preview_lines(
                &app.streaming_tool_calls,
                elapsed,
                content_max_width,
                &app.theme,
            );
            tail_lines.extend(preview_lines);
        }
    } else if !app.streaming_md_cache.is_empty() {
        app.streaming_md_cache = Vec::new();
        app.streaming_md_cache_len = 0;
    }

    if app.has_pending_messages() {
        tail_lines.push(Line::from(""));
        // "Send now" messages come first: that is the order in which the model will read them.
        for steer_msg in &app.steer_messages {
            let spans = vec![
                Span::styled("  ", Style::default()),
                Span::styled("[sending] ", Style::default().fg(app.theme.accent_cyan).add_modifier(Modifier::BOLD)),
                Span::styled(steer_msg.clone(), Style::default().fg(app.theme.foreground)),
                Span::styled(" (next step)", Style::default().fg(app.theme.dark_gray)),
            ];
            tail_lines.extend(crate::markdown::wrap_spans_with_indent(spans, content_max_width, "", "              "));
        }
        for (i, queued_msg) in app.message_queue.iter().enumerate() {
            let label = if app.message_queue.len() == 1 {
                "[queued] ".to_string()
            } else {
                format!("[queued #{}] ", i + 1)
            };
            let spans = vec![
                Span::styled("  ", Style::default()),
                Span::styled(label, Style::default().fg(app.theme.accent_yellow).add_modifier(Modifier::BOLD)),
                Span::styled(queued_msg.clone(), Style::default().fg(app.theme.foreground)),
                Span::styled(" (after the answer)", Style::default().fg(app.theme.dark_gray)),
            ];
            tail_lines.extend(crate::markdown::wrap_spans_with_indent(spans, content_max_width, "", "              "));
        }
        tail_lines.push(Line::from(""));
    }

    let total_lines = header_lines.len() + app.cached_message_lines.len() + tail_lines.len();
    app.total_rendered_items = total_lines;

    let visible_height = (chat_chunk.height as usize).max(1);
    let max_scroll = total_lines.saturating_sub(visible_height);

    if app.auto_scroll {
        app.scroll_offset = max_scroll;
    } else {
        app.scroll_offset = app.scroll_offset.min(max_scroll);
    }

    app.last_chat_rect = Some(chat_chunk);

    // Resolving drag columns into characters and finding clicked URLs needs the plain text of every rendered line.
    // Only the small header/tail segments are rebuilt per frame; history plain text is maintained incrementally above.
    app.selection_lines.head = header_lines.iter().map(line_plain_text).collect();
    app.selection_lines.tail = tail_lines.iter().map(line_plain_text).collect();

    // Clone only the lines that are actually visible in the viewport (O(viewport) instead of O(history)).
    let visible_lines: Vec<Line<'static>> = header_lines
        .iter()
        .chain(app.cached_message_lines.iter())
        .chain(tail_lines.iter())
        .skip(app.scroll_offset)
        .take(visible_height)
        .cloned()
        .collect();

    let message_paragraph = Paragraph::new(visible_lines)
        .block(Block::default().borders(Borders::NONE));
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
        let elapsed = app.last_turn_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
        let spinner_char = crate::spinner::frame(elapsed);
        // The colour tells the phase: a tool is running (or being written), the answer is
        // streaming, or the model is still reasoning / waiting for its first token.
        let spinner_color = if app.in_flight_tool_calls > 0 || !app.streaming_tool_calls.is_empty() {
            app.theme.accent_cyan
        } else if !app.streaming_text.is_empty() {
            app.theme.accent_blue
        } else {
            app.theme.accent_purple
        };

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
            Span::styled(spinner_char, Style::default().fg(spinner_color)),
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
        let steer_count = app.steer_messages.len();
        if steer_count > 0 {
            left_spans.push(Span::styled(
                " · [sending at next step]",
                Style::default().fg(app.theme.accent_cyan).add_modifier(Modifier::BOLD),
            ));
        }

        // Key hints, longest first: the status text on the left varies in length, and a hint that
        // does not fit must give way to a shorter one instead of vanishing.
        let is_mobile = is_mobile_portrait(size.width, size.height);
        let right_options: &[&str] = if steer_count > 0 {
            &["(Alt+Enter: interrupt now · Esc: undo) ", "(Alt+Enter: interrupt) ", "(Esc: undo) "]
        } else if queue_count > 0 {
            &["(Alt+Enter: send now · Esc: undo) ", "(Alt+Enter: send now) ", "(Esc: undo) "]
        } else if !app.input_buffer.trim().is_empty() && !is_mobile {
            &[
                "(Enter: queue · Alt+Enter: send now · Esc: cancel) ",
                "(Alt+Enter: send now · Esc: cancel) ",
                "(Alt+Enter: send now) ",
                "(Esc to cancel) ",
            ]
        } else {
            &["(Esc to cancel) "]
        };

        let width_of = |s: &str| unicode_width::UnicodeWidthStr::width(s);
        let left_len: usize = left_spans.iter().map(|s| width_of(s.content.as_ref())).sum();
        let bar_width = activity_chunk.width as usize;
        let right_text = right_options
            .iter()
            .find(|opt| bar_width > left_len + width_of(opt))
            .copied();

        let mut spans = left_spans;
        if let Some(text) = right_text {
            let pad = bar_width - (left_len + width_of(text));
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(text, Style::default().fg(app.theme.dark_gray)));
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
        } else if app.is_streaming {
            "Type to queue a message · Alt+Enter sends it now"
        } else {
            "Type your message or @path/to/file"
        };
        (
            vec![Line::from(vec![
                prompt_prefix.clone(),
                Span::styled(format!("{} ", crate::cursor::block()), Style::default().fg(app.theme.accent_blue)),
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
        })
        .or_else(|| {
            // While the agent works, say what Enter and Alt+Enter will do with the typed message.
            // Only when it fits on the row: a hint must never push the text itself around.
            let hint = BUSY_INPUT_HINT;
            let used = prefix_width
                + unicode_width::UnicodeWidthStr::width(app.input_buffer.as_str())
                + unicode_width::UnicodeWidthStr::width(hint);
            let fits = used < size.width as usize && !app.input_buffer.contains('\n');
            (app.is_streaming && fits && !is_mobile_portrait(size.width, size.height))
                .then(|| Span::styled(hint, Style::default().fg(app.theme.dark_gray)))
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

    let lines_below = max_scroll.saturating_sub(app.scroll_offset);
    let mut composer_block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(app.theme.border_dim));
    if !app.auto_scroll && lines_below > 0 {
        let noun = if lines_below == 1 { "line" } else { "lines" };
        composer_block = composer_block.title_top(
            Line::from(vec![
                Span::styled(format!(" ↓ {} {} below ", lines_below, noun), Style::default().fg(app.theme.accent_blue)),
                Span::styled("(End) ", Style::default().fg(app.theme.dark_gray)),
            ])
            .right_aligned(),
        );
    }

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

    // Right spans: Model info, temperature, reasoning
    let mut right_spans = Vec::new();
    let is_deepseek = cfg.is_deepseek_endpoint();
    let is_flash = is_deepseek && (cfg.model == "deepseek-flash" || cfg.model == "deepseek-v4.1-flash" || cfg.model == "deepseek-v4.1-flash-1017" || cfg.model == "deepseek-v4-flash" || cfg.model == "deepseek-chat" || cfg.model == "DeepSeek-V4.1-Flash" || cfg.model == "DeepSeek-V4.1-Flash-1017" || cfg.model == "DeepSeek-V4-Flash");
    let is_pro = is_deepseek && (cfg.model == "deepseek-pro" || cfg.model == "deepseek-v4-pro" || cfg.model == "deepseek-reasoner" || cfg.model == "DeepSeek-V4-Pro");

    // The provider table holds the exact label of every variant (e.g. Flash vs Flash-1017).
    let display_name = {
        let all = cfg.all_providers();
        let prov = cfg.active_provider.as_deref().and_then(|p_name| all.iter().find(|p| p.name == p_name));
        prov.into_iter()
            .chain(all.iter())
            .find_map(|p| p.models.iter().find(|m| m.id == cfg.model))
            .map(|m| m.display_name().to_string())
    };

    let model_style = Style::default().fg(app.theme.muted);
    let sep_style = Style::default().fg(app.theme.border_dim);
    let temp_color = if cfg.temperature > 1.2 {
        app.theme.warning
    } else {
        app.theme.accent_cyan
    };

    if is_flash {
        let effort = if cfg.local_llm_enabled && cfg.reasoning_effort != "dynamic" {
            // The local engine always runs the adaptive policy, whatever the setting says.
            "dynamic"
        } else {
            cfg.reasoning_effort.as_str()
        };

        let r_color = match effort {
            "none" => app.theme.muted,
            "low" => app.theme.effort_low,
            "high" => app.theme.effort_high,
            "xhigh" | "max" => app.theme.effort_max,
            _ => app.theme.effort_high,
        };

        right_spans.push(Span::styled(display_name.unwrap_or_else(|| "DeepSeek-V4.1-Flash".to_string()), model_style));
        right_spans.push(Span::styled(" · ", sep_style));
        right_spans.push(Span::styled(format!("{:.1}", cfg.temperature), Style::default().fg(temp_color)));
        right_spans.push(Span::styled(" · ", sep_style));
        right_spans.push(Span::styled(format!("{} ", effort), Style::default().fg(r_color)));
    } else if is_pro {
        let effort = &cfg.pro_settings.reasoning_effort;
        let r_color = match effort.as_str() {
            "low" => app.theme.effort_low,
            "medium" => app.theme.effort_high,
            "high" | "max" => app.theme.effort_max,
            _ => app.theme.effort_high,
        };

        // No temperature here: the reasoning engine ignores it, so showing one would mislead.
        right_spans.push(Span::styled(display_name.unwrap_or_else(|| "DeepSeek-V4-Pro".to_string()), model_style));
        right_spans.push(Span::styled(" · ", sep_style));
        right_spans.push(Span::styled(format!("{} ", effort), Style::default().fg(r_color)));
    } else if cfg.model.starts_with("local") {
        right_spans.push(Span::styled(format!("Local: {}", cfg.local_llm_model), model_style));
        right_spans.push(Span::styled(" · ", sep_style));
        right_spans.push(Span::styled("offline ", Style::default().fg(app.theme.success)));
    } else {
        let display_name = display_name.unwrap_or_else(|| cfg.model.clone());

        let r_color = match cfg.reasoning_effort.as_str() {
            "none" => app.theme.muted,
            "low" => app.theme.effort_low,
            "high" => app.theme.effort_high,
            "xhigh" | "max" => app.theme.effort_max,
            _ => app.theme.effort_high,
        };

        right_spans.push(Span::styled(display_name, model_style));
        right_spans.push(Span::styled(" · ", sep_style));
        right_spans.push(Span::styled(format!("{:.1}", cfg.temperature), Style::default().fg(temp_color)));
        if !cfg.reasoning_effort.is_empty() {
            right_spans.push(Span::styled(" · ", sep_style));
            right_spans.push(Span::styled(format!("{} ", cfg.reasoning_effort), Style::default().fg(r_color)));
        } else {
            right_spans.push(Span::raw(" "));
        }
    }

    if cfg.local_llm_enabled && !cfg.model.starts_with("local") {
        right_spans.push(Span::styled("+Local ", Style::default().fg(app.theme.accent_cyan)));
    }

    // Center spans: Local LLM status indicator and Active Background Tasks badge
    let mut center_spans = Vec::new();
    if cfg.local_llm_enabled || cfg.model.starts_with("local") {
        let is_local_online = app.local_llm_online.load(Ordering::Relaxed);
        let dot_color = if is_local_online { app.theme.success } else { app.theme.error };
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

    let active_subagents = corex_core::subagent::get_subagent_manager()
        .lock()
        .map(|m| m.active_count())
        .unwrap_or(0);

    if active_tasks > 0 {
        if !center_spans.is_empty() {
            center_spans.push(Span::styled("  ·  ", Style::default().fg(app.theme.gray)));
        }
        let task_badge = if active_tasks == 1 {
            "1 task".to_string()
        } else {
            format!("{} tasks", active_tasks)
        };
        center_spans.push(Span::styled(
            task_badge,
            Style::default().fg(app.theme.accent_yellow),
        ));
    }

    if active_subagents > 0 {
        if !center_spans.is_empty() {
            center_spans.push(Span::styled("  ·  ", Style::default().fg(app.theme.gray)));
        }
        let subagent_badge = if active_subagents == 1 {
            "1 agent".to_string()
        } else {
            format!("{} agents", active_subagents)
        };
        center_spans.push(Span::styled(
            subagent_badge,
            Style::default()
                .fg(app.theme.effort_high)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let tail_has_collapsed = !app.logs_expanded && tail_lines.iter().any(is_collapsed_marker);
    if app.logs_expanded || app.cached_has_collapsed_logs || tail_has_collapsed {
        if !center_spans.is_empty() {
            center_spans.push(Span::styled("  ·  ", Style::default().fg(app.theme.gray)));
        }
        if app.logs_expanded {
            center_spans.push(Span::styled(
                "[Logs: Full (Ctrl+O)]",
                Style::default().fg(app.theme.accent_cyan).add_modifier(Modifier::BOLD),
            ));
        } else {
            center_spans.push(Span::styled("Ctrl+O full logs", Style::default().fg(app.theme.dark_gray)));
        }
    }

    // Left spans: Workspace directory, git branch, plan mode, and exit warning
    let show_exit_warning = app.last_ctrl_c_press
        .map(|i| i.elapsed() <= std::time::Duration::from_secs(2))
        .unwrap_or(false);

    let right_width = right_spans.iter().map(|s| s.width()).sum::<usize>();
    let center_width = center_spans.iter().map(|s| s.width()).sum::<usize>();

    let mut left_spans = Vec::new();
    if show_exit_warning {
        // Available width for the left side of the status bar (accounting for right & center widgets)
        let avail = (status_chunk.width as usize).saturating_sub(right_width + center_width + 1);
        let path_str = format!(" {} ", app.shorten_path());
        let branch_str = if !app.git_branch.is_empty() {
            format!("({}) ", app.git_branch)
        } else {
            String::new()
        };
        let full_needed = unicode_width::UnicodeWidthStr::width(path_str.as_str())
            + unicode_width::UnicodeWidthStr::width(branch_str.as_str())
            + 28;

        if avail >= full_needed {
            // Full space available: render path + branch + plan mode + warning
            left_spans.push(Span::styled(path_str, Style::default().fg(app.theme.foreground)));
            if !branch_str.is_empty() {
                left_spans.push(Span::styled(branch_str, Style::default().fg(app.theme.accent_cyan)));
            }
            left_spans.extend(mode_badges(app));
            left_spans.push(Span::styled("Press Ctrl+C again to exit. ", Style::default().fg(app.theme.error).add_modifier(Modifier::BOLD)));
        } else if avail >= unicode_width::UnicodeWidthStr::width(branch_str.as_str()) + 28 && !branch_str.is_empty() {
            // Medium space: prioritize branch and the exit warning so it is never truncated
            left_spans.push(Span::styled(format!(" {} ", branch_str.trim()), Style::default().fg(app.theme.accent_cyan)));
            left_spans.push(Span::styled("Press Ctrl+C again to exit. ", Style::default().fg(app.theme.error).add_modifier(Modifier::BOLD)));
        } else {
            // Narrow space (e.g. /fleet split mode on narrower terminals): show complete exit warning
            left_spans.push(Span::styled(" Press Ctrl+C again to exit.", Style::default().fg(app.theme.error).add_modifier(Modifier::BOLD)));
        }
    } else {
        left_spans.push(Span::styled(format!(" {} ", app.shorten_path()), Style::default().fg(app.theme.foreground)));
        if !app.git_branch.is_empty() {
            left_spans.push(Span::styled(format!("({}) ", app.git_branch), Style::default().fg(app.theme.accent_cyan)));
        }
        left_spans.extend(mode_badges(app));
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
        let mobile_right = if active_tasks > 0 || active_subagents > 0 {
            center_spans
        } else {
            right_spans
        };
        frame.render_widget(Paragraph::new(Line::from(mobile_right)).alignment(Alignment::Right), footer_cols[1]);
    } else if center_spans.is_empty() {
        let r_len = (right_width as u16).min(status_chunk.width);
        let footer_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(r_len),
            ])
            .split(status_chunk);

        frame.render_widget(Paragraph::new(Line::from(left_spans)).alignment(Alignment::Left), footer_cols[0]);
        frame.render_widget(Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right), footer_cols[1]);
    } else {
        let r_len = (right_width as u16).min(status_chunk.width);
        let c_len = (center_width as u16 + 4).min(status_chunk.width.saturating_sub(r_len));
        let footer_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(c_len),
                Constraint::Length(r_len),
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



/// Footer badges for the modes that change how tools run. Auto-approve is the riskiest one, so it
/// is spelled out instead of being left to the `*` prompt prefix alone.
fn mode_badges(app: &App) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if app.always_allow_tools {
        spans.push(Span::styled("[AUTO-APPROVE] ", Style::default().fg(app.theme.error).add_modifier(Modifier::BOLD)));
    }
    if app.plan_mode {
        spans.push(Span::styled("[PLAN MODE] ", Style::default().fg(app.theme.warning).add_modifier(Modifier::BOLD)));
    }
    spans
}

/// Highlights the selected cells of the chat viewport.
///
/// The feed is drawn from generated lines, so rather than threading selection state through the
/// renderer this simply restyles the cells that already reached the screen.
pub fn paint_selection(
    buffer: &mut Buffer,
    chat: Rect,
    scroll_offset: usize,
    selection: &Selection,
    lines: &SelectionLines,
    theme: &Theme,
) {
    use unicode_width::UnicodeWidthStr;

    let ((start_line, start_col), (end_line, end_col)) = selection.ordered();
    let scroll = scroll_offset;
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

