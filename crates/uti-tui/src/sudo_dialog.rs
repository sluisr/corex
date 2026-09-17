use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
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
    pub pending_calls: Vec<ToolCall>,
    pub command: String,
    pub password_input: String,
    pub error_msg: Option<String>,
}

impl Default for SudoDialogState {
    fn default() -> Self {
        Self::new()
    }
}

impl SudoDialogState {
    pub fn new() -> Self {
        Self {
            is_open: false,
            pending_call: None,
            pending_calls: Vec::new(),
            command: String::new(),
            password_input: String::new(),
            error_msg: None,
        }
    }

    pub fn open(&mut self, call: ToolCall, command: String) {
        self.is_open = true;
        self.pending_call = Some(call.clone());
        self.pending_calls = vec![call];
        self.command = command;
        self.password_input.clear();
        self.error_msg = None;
    }

    pub fn open_batch(&mut self, calls: Vec<ToolCall>, command: String) {
        self.is_open = true;
        self.pending_call = calls.first().cloned();
        self.pending_calls = calls;
        self.command = command;
        self.password_input.clear();
        self.error_msg = None;
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.pending_call = None;
        self.pending_calls.clear();
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

    let display_cmd = state.command.strip_prefix("sudo ").unwrap_or(&state.command).trim();
    let title_cmd = uti_core::truncate_ellipsis(display_cmd, 35);

    let title_str = format!(" Sudo: {} ", title_cmd);

    let min_content_width = 58;
    let title_min_width = (title_str.chars().count() + 6) as u16;
    let dialog_width = min_content_width.max(title_min_width).min(area.width.saturating_sub(4));
    let dialog_height = 3;

    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = area.height.saturating_sub(7);
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Dim the chat behind the modal so it does not visually collide.
    render_scrim(frame, area);
    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(symbols::border::PLAIN)
        .border_style(Style::default().fg(theme.accent_yellow))
        .title(title_str)
        .title_alignment(Alignment::Left);

    let inner_area = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    let shortcuts_width = 25;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(shortcuts_width),
        ])
        .split(inner_area);

    let left_spans = if state.password_input.is_empty() {
        if let Some(ref err) = state.error_msg {
            vec![
                Span::styled("❯ ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
                Span::styled("█ ", Style::default().fg(theme.accent_yellow)),
                Span::styled(err.clone(), Style::default().fg(Color::LightRed)),
            ]
        } else {
            vec![
                Span::styled("❯ ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
                Span::styled("█ ", Style::default().fg(theme.accent_yellow)),
                Span::styled("Type password...", Style::default().fg(theme.dark_gray)),
            ]
        }
    } else {
        let masked: String = "*".repeat(state.password_input.len());
        vec![
            Span::styled("❯ ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
            Span::styled(masked, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("█", Style::default().fg(theme.accent_yellow)),
        ]
    };

    let right_spans = vec![
        Span::styled("[Enter] ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
        Span::styled("Run  ", Style::default().fg(theme.gray)),
        Span::styled("[Esc] ", Style::default().fg(theme.dark_gray).add_modifier(Modifier::BOLD)),
        Span::styled("Cancel", Style::default().fg(theme.dark_gray)),
    ];

    frame.render_widget(
        Paragraph::new(Line::from(left_spans)),
        cols[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(right_spans))
            .alignment(Alignment::Right),
        cols[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use uti_core::types::FunctionCall;

    #[test]
    fn test_sudo_dialog_open_and_close() {
        let mut state = SudoDialogState::new();
        assert!(!state.is_open);
        assert!(state.pending_call.is_none());
        assert!(state.pending_calls.is_empty());

        let call = ToolCall {
            id: "call_123".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "run_shell_command".to_string(),
                arguments: r#"{"command":"sudo apt update"}"#.to_string(),
            },
        };

        state.open(call.clone(), "sudo apt update".to_string());
        assert!(state.is_open);
        assert_eq!(state.command, "sudo apt update");
        assert_eq!(state.pending_calls.len(), 1);
        assert_eq!(state.pending_call.as_ref().unwrap().id, "call_123");

        state.close();
        assert!(!state.is_open);
        assert!(state.pending_call.is_none());
        assert!(state.pending_calls.is_empty());
        assert!(state.command.is_empty());
    }

    #[test]
    fn test_sudo_dialog_open_batch() {
        let mut state = SudoDialogState::new();
        let call1 = ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "run_shell_command".to_string(),
                arguments: r#"{"command":"sudo apt update"}"#.to_string(),
            },
        };
        let call2 = ToolCall {
            id: "call_2".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "run_shell_command".to_string(),
                arguments: r#"{"command":"sudo apt upgrade -y"}"#.to_string(),
            },
        };

        state.open_batch(vec![call1, call2], "sudo apt update && sudo apt upgrade -y".to_string());
        assert!(state.is_open);
        assert_eq!(state.pending_calls.len(), 2);
        assert_eq!(state.pending_call.as_ref().unwrap().id, "call_1");
        assert_eq!(state.command, "sudo apt update && sudo apt upgrade -y");
    }
}
