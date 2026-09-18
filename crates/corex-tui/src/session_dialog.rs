use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, BorderType, Clear, Paragraph};
use ratatui::Frame;
use corex_core::session::{Session, SessionSummary};

use crate::overlay::render_scrim;
use crate::theme::Theme;

#[derive(Debug, Default)]
pub struct SessionDialogState {
    pub is_open: bool,
    pub selected_idx: usize,
    pub sessions: Vec<SessionSummary>,
}

impl SessionDialogState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, workspace_dir: &str) {
        self.sessions = Session::list_all(Some(workspace_dir));
        self.selected_idx = 0;
        self.is_open = true;
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }
}

pub fn render_session_dialog(
    frame: &mut Frame,
    area: Rect,
    state: &SessionDialogState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let dialog_width = 92.min(area.width.saturating_sub(4)).max(56);
    let dialog_height = (area.height * 3 / 4)
        .clamp(14, 24)
        .min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Dim the chat behind the modal
    render_scrim(frame, area);
    frame.render_widget(Clear, dialog_area);

    let selected_bg = Color::Rgb(32, 44, 68);
    let inner_w = dialog_width.saturating_sub(2) as usize;
    let inner_h = dialog_height.saturating_sub(2) as usize;

    let mut lines = Vec::new();

    if state.sessions.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("No conversation sessions found for this workspace.", Style::default().fg(theme.gray)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("Sessions are automatically saved as you chat or via ", Style::default().fg(theme.dark_gray)),
            Span::styled("/save <tag>", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            Span::styled(".", Style::default().fg(theme.dark_gray)),
        ]));
    } else {
        // Fixed column widths:
        // Index: " ❯ 1. " = 6 chars
        // Model:  " [Flash] " = 9 chars
        // Msgs:   " 693 msg " = 9 chars
        // Date:   "  3d ago " = 9 chars
        // Spacing = 2 chars
        let metadata_cols = 6 + 9 + 9 + 9 + 2;
        let title_col_width = inner_w.saturating_sub(metadata_cols).max(18);

        // Header column labels
        let header_style = Style::default().fg(Color::Rgb(100, 115, 140)).add_modifier(Modifier::BOLD);
        let header_title = format!("{:<width$}", "Session Topic / Name", width = title_col_width);
        lines.push(Line::from(vec![
            Span::styled("   #  ", header_style),
            Span::styled(header_title, header_style),
            Span::styled("   Model ", header_style),
            Span::styled("  Messages", header_style),
            Span::styled("   Updated ", header_style),
        ]));

        // Subtle divider below header
        let divider_line = "─".repeat(inner_w.saturating_sub(2));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(divider_line, Style::default().fg(Color::Rgb(35, 45, 60))),
        ]));

        // Calculate visible viewport
        // Inner height minus header (2 lines) minus footer (2 lines)
        let max_visible = inner_h.saturating_sub(4).max(1);
        let start_idx = state.selected_idx.saturating_sub(max_visible / 2);
        let end_idx = (start_idx + max_visible).min(state.sessions.len());
        let start_idx = end_idx.saturating_sub(max_visible);

        for i in start_idx..end_idx {
            let s = &state.sessions[i];
            let is_selected = i == state.selected_idx;
            let row_bg = if is_selected { selected_bg } else { Color::Reset };

            let cursor = if is_selected { " ❯ " } else { "   " };
            let cursor_style = if is_selected {
                Style::default().fg(theme.accent_blue).bg(row_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
            };

            let num_style = if is_selected {
                Style::default().fg(theme.accent_blue).bg(row_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.dark_gray)
            };

            // Combine title with tag if present
            let full_title = if let Some(tag) = &s.tag {
                format!("{} [{}]", s.title, tag)
            } else {
                s.title.clone()
            };

            let truncated_title = corex_core::truncate_ellipsis(&full_title, title_col_width);
            let title_len = truncated_title.chars().count();
            let padded_title = if title_len < title_col_width {
                format!("{}{}", truncated_title, " ".repeat(title_col_width - title_len))
            } else {
                truncated_title
            };

            let title_style = if is_selected {
                Style::default().fg(Color::White).bg(row_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };

            // Model pill badge
            let (model_tag, model_fg) = if s.model.starts_with("local") {
                ("[Local]", Color::Rgb(255, 215, 130))
            } else if s.model.contains("pro") || s.model.contains("reasoner") {
                ("[ Pro ]", Color::Rgb(215, 175, 255))
            } else {
                ("[Flash]", Color::Rgb(135, 215, 235))
            };

            let model_style = if is_selected {
                Style::default().fg(model_fg).bg(row_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(model_fg)
            };

            let msg_count_str = format!("{:>5} msg ", s.message_count);
            let msg_style = if is_selected {
                Style::default().fg(theme.foreground).bg(row_bg)
            } else {
                Style::default().fg(theme.gray)
            };

            let rel_time = Session::format_relative_time(s.updated_at);
            let time_str = format!("{:>9} ", rel_time);
            let time_style = if is_selected {
                Style::default().fg(theme.accent_cyan).bg(row_bg)
            } else {
                Style::default().fg(theme.dark_gray)
            };

            // Trailing space for full-row highlight
            let used_chars = 3 + 3 + title_col_width + 9 + 9 + 10;
            let trailing_pad = inner_w.saturating_sub(used_chars);
            let pad_str = " ".repeat(trailing_pad);

            let space_style = if is_selected { Style::default().bg(row_bg) } else { Style::default() };

            lines.push(Line::from(vec![
                Span::styled(cursor, cursor_style),
                Span::styled(format!("{:>2}. ", i + 1), num_style),
                Span::styled(padded_title, title_style),
                Span::styled(format!(" {} ", model_tag), model_style),
                Span::styled(msg_count_str, msg_style),
                Span::styled(time_str, time_style),
                Span::styled(pad_str, space_style),
            ]));
        }

        // Fill empty rows if sessions are fewer than max_visible to maintain stable layout
        let current_rendered = end_idx.saturating_sub(start_idx);
        if current_rendered < max_visible {
            for _ in 0..(max_visible - current_rendered) {
                lines.push(Line::from(""));
            }
        }
    }

    // Bottom divider & shortcut buttons
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("─".repeat(inner_w.saturating_sub(2)), Style::default().fg(Color::Rgb(35, 45, 60))),
    ]));

    let key_badge = |key: &'static str, desc: &'static str, color: Color| -> Vec<Span> {
        vec![
            Span::styled("[", Style::default().fg(Color::Rgb(80, 95, 120))),
            Span::styled(key, Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::styled("] ", Style::default().fg(Color::Rgb(80, 95, 120))),
            Span::styled(desc, Style::default().fg(theme.gray)),
            Span::raw("   "),
        ]
    };

    let mut footer_spans = vec![Span::raw("  ")];
    footer_spans.extend(key_badge("Enter", "Resume", theme.accent_blue));
    footer_spans.extend(key_badge("x", "Delete", theme.accent_red));
    footer_spans.extend(key_badge("↑/↓", "Navigate", theme.accent_cyan));
    footer_spans.extend(key_badge("Esc", "Close", theme.gray));

    lines.push(Line::from(footer_spans));

    let scroll_hint = if state.sessions.len() > 1 {
        format!(" ({} sessions) ", state.sessions.len())
    } else if state.sessions.len() == 1 {
        " (1 session) ".to_string()
    } else {
        String::new()
    };

    let title = format!(" Resume Session{} ", scroll_hint);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent_blue));

    frame.render_widget(Paragraph::new(lines).block(block), dialog_area);
}
