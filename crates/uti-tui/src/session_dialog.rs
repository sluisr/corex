use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, BorderType, Clear, Paragraph};
use ratatui::Frame;
use uti_core::session::{Session, SessionSummary};

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

    let dialog_width = 80.min(area.width).max(50);
    let dialog_height = 15.min(area.height);

    let x = (area.width - dialog_width) / 2;
    let y = (area.height - dialog_height) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Dim the chat behind the modal so it does not visually collide.
    render_scrim(frame, area);
    frame.render_widget(Clear, dialog_area);

    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "Select Conversation Session",
            Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    if state.sessions.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("No sessions found for this project.", Style::default().fg(theme.gray)),
        ]));
    } else {
        // Limit visible items to fit inside the height.
        // Each session is exactly 1 line. Inside area height is dialog_height - 2 (borders).
        // Title (3 lines), Footer (3 lines), leaving dialog_height - 6 lines for items.
        let max_visible = (dialog_height as usize).saturating_sub(6);
        let start_idx = state.selected_idx.saturating_sub(max_visible / 2);
        let end_idx = (start_idx + max_visible).min(state.sessions.len());
        let start_idx = end_idx.saturating_sub(max_visible);

        for i in start_idx..end_idx {
            let s = &state.sessions[i];
            let is_selected = i == state.selected_idx;
            let bullet = if is_selected { "● " } else { "  " };

            let title_style = if is_selected {
                Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };

            let rel_time = Session::format_relative_time(s.updated_at);
            let tag_str = s.tag.as_ref().map(|t| format!(" [{}]", t)).unwrap_or_default();
            let model_name = if s.model.contains("pro") || s.model.contains("reasoner") {
                "Pro"
            } else {
                "Flash"
            };

            // Truncate title to fit nicely on one line alongside details
            let max_title_len = 25;
            let display_title = if s.title.len() > max_title_len {
                format!("{}...", &s.title[..max_title_len.saturating_sub(3)])
            } else {
                s.title.clone()
            };

            let title_span = Span::styled(display_title, title_style);
            let info_span = Span::styled(
                format!(" ({}){} · {} · {} msg", rel_time, tag_str, model_name, s.message_count),
                Style::default().fg(theme.gray)
            );

            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(bullet, Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{}. ", i + 1), title_style),
                title_span,
                Span::raw(" "),
                info_span,
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "(Press Enter to load · x to delete · Esc to cancel)",
            Style::default().fg(theme.gray),
        ),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_blue));

    frame.render_widget(Paragraph::new(lines).block(block), dialog_area);
}
