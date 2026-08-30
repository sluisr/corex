use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use uti_core::types::ToolCall;

use crate::overlay::render_scrim;
use crate::theme::Theme;

pub struct SudoDialogState {
    pub is_open: bool,
    pub pending_call: Option<ToolCall>,
    pub command: String,
    pub password_input: String,
    pub error_msg: Option<String>,
}

impl SudoDialogState {
    pub fn new() -> Self {
        Self {
            is_open: false,
            pending_call: None,
            command: String::new(),
            password_input: String::new(),
            error_msg: None,
        }
    }

    pub fn open(&mut self, call: ToolCall, command: String) {
        self.is_open = true;
        self.pending_call = Some(call);
        self.command = command;
        self.password_input.clear();
        self.error_msg = None;
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.pending_call = None;
        self.command.clear();
        self.password_input.clear();
        self.error_msg = None;
    }
}

pub fn render_sudo_dialog(
    frame: &mut Frame,
    area: Rect,
    state: &SudoDialogState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let title_cmd = if state.command.len() > 30 {
        format!("{}...", &state.command[..27])
    } else {
        state.command.clone()
    };

    let title_str = format!(" Sudo: {} ", title_cmd);

    // Tight auto-compact width (fits content + shortcuts perfectly without void)
    let min_content_width = 54;
    let title_min_width = (title_str.chars().count() + 6) as u16;
    let dialog_width = min_content_width.max(title_min_width).min(area.width.saturating_sub(4));
    let dialog_height = 3;

    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = area.height.saturating_sub(7);
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Dim the chat behind the modal so it does not visually collide.
    render_scrim(frame, area);
    frame.render_widget(Clear, dialog_area);

    let content_line = if state.password_input.is_empty() {
        Line::from(vec![
            Span::styled("❯ ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
            Span::styled("█ ", Style::default().fg(theme.accent_yellow)),
            Span::styled("Type password...", Style::default().fg(theme.dark_gray)),
            Span::raw("   "),
            Span::styled("[Enter] ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            Span::styled("Run  ", Style::default().fg(theme.gray)),
            Span::styled("[Esc] ", Style::default().fg(theme.dark_gray).add_modifier(Modifier::BOLD)),
            Span::styled("Cancel", Style::default().fg(theme.dark_gray)),
        ])
    } else {
        let masked: String = "*".repeat(state.password_input.len());
        Line::from(vec![
            Span::styled("❯ ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
            Span::styled(masked, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("█", Style::default().fg(theme.accent_yellow)),
            Span::raw("   "),
            Span::styled("[Enter] ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            Span::styled("Run  ", Style::default().fg(theme.gray)),
            Span::styled("[Esc] ", Style::default().fg(theme.dark_gray).add_modifier(Modifier::BOLD)),
            Span::styled("Cancel", Style::default().fg(theme.dark_gray)),
        ])
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(symbols::border::ROUNDED)
        .border_style(Style::default().fg(theme.accent_yellow))
        .title(title_str)
        .title_alignment(ratatui::layout::Alignment::Left);

    frame.render_widget(Paragraph::new(content_line).block(block), dialog_area);
}
