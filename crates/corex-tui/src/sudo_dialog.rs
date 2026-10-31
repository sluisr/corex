use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use corex_core::types::ToolCall;

use crate::overlay::begin_modal;
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
    let dialog_width = 58.max(display_cmd.chars().count() as u16 + 16).min(area.width.saturating_sub(4));
    let dialog_area = Rect::new(
        area.width.saturating_sub(dialog_width) / 2,
        area.height.saturating_sub(7),
        dialog_width,
        2.min(area.height),
    );

    // Dim the chat behind the modal so it does not visually collide. No vertical halo: this
    // bar sits right above the composer.
    begin_modal(frame, area, dialog_area, 2, 0);

    // Red is the colour terminals give root: this command will run with full privileges.
    let rail = || Span::styled("▎ ", Style::default().fg(theme.error));
    let chip = Style::default().bg(theme.code_bg);
    let command_room = (dialog_width as usize).saturating_sub(12);
    let header = Line::from(vec![
        rail(),
        Span::styled("sudo  ", Style::default().fg(theme.error).add_modifier(Modifier::BOLD)),
        Span::styled(" $ ", chip.fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
        Span::styled(corex_core::truncate_ellipsis(display_cmd, command_room).to_string(), chip.fg(theme.text_strong)),
        Span::styled(" ", chip),
    ]);

    let mut input = vec![rail(), Span::styled("› ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD))];
    if state.password_input.is_empty() {
        input.push(Span::styled(format!("{} ", crate::cursor::block()), Style::default().fg(theme.accent_yellow)));
        match &state.error_msg {
            Some(err) => input.push(Span::styled(err.clone(), Style::default().fg(theme.error))),
            None => input.push(Span::styled("Password", Style::default().fg(theme.dark_gray))),
        }
    } else {
        input.push(Span::styled("*".repeat(state.password_input.chars().count()), Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)));
        input.push(Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_yellow)));
    }
    let hints = Line::from(vec![
        Span::styled("Enter ", Style::default().fg(theme.text_strong)),
        Span::styled("run · ", Style::default().fg(theme.dark_gray)),
        Span::styled("Esc ", Style::default().fg(theme.text_strong)),
        Span::styled("cancel", Style::default().fg(theme.dark_gray)),
    ]);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(dialog_area);
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(23)])
        .split(rows[1]);
    frame.render_widget(Paragraph::new(header), rows[0]);
    frame.render_widget(Paragraph::new(Line::from(input)), bottom[0]);
    frame.render_widget(Paragraph::new(hints).alignment(Alignment::Right), bottom[1]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sudo_dialog_open_and_close() {
        let mut state = SudoDialogState::new();
        assert!(!state.is_open);
        assert!(state.pending_call.is_none());
        assert!(state.pending_calls.is_empty());

        let call = ToolCall::new("call_123", "run_shell_command", r#"{"command":"sudo apt update"}"#);

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
    fn the_dialog_is_a_red_rail_with_the_command_on_a_chip() {
        use ratatui::{backend::TestBackend, Terminal};
        let theme = Theme::default();
        let mut state = SudoDialogState::new();
        state.open(ToolCall::new("c", "run_shell_command", r#"{"command":"sudo whoami"}"#), "sudo whoami".into());
        let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();
        terminal.draw(|f| render_sudo_dialog(f, f.area(), &state, &theme)).unwrap();
        let buf = terminal.backend().buffer();

        let rows: Vec<String> = (0..20).map(|y| (0..90).map(|x| buf[(x, y)].symbol()).collect()).collect();
        let header = rows.iter().position(|r| r.contains("sudo") && r.contains("$ whoami")).expect("header row");
        assert!(rows[header + 1].contains("Password") && rows[header + 1].contains("Enter run"), "{rows:#?}");
        assert!(!rows.iter().any(|r| r.contains(['┌', '┐', '└', '┘', '─', '│'])), "no box: {rows:#?}");

        let cell = |needle: &str, row: usize| {
            let x = rows[row].find(needle).map(|b| rows[row][..b].chars().count()).unwrap() as u16;
            buf[(x, row as u16)].clone()
        };
        assert_eq!(cell("▎", header).fg, theme.error, "root gets the red rail");
        assert_eq!(cell("whoami", header).bg, theme.code_bg, "the command sits on the shaded chip");
    }

    #[test]
    fn test_sudo_dialog_open_batch() {
        let mut state = SudoDialogState::new();
        let call1 = ToolCall::new("call_1", "run_shell_command", r#"{"command":"sudo apt update"}"#);
        let call2 = ToolCall::new("call_2", "run_shell_command", r#"{"command":"sudo apt upgrade -y"}"#);

        state.open_batch(vec![call1, call2], "sudo apt update && sudo apt upgrade -y".to_string());
        assert!(state.is_open);
        assert_eq!(state.pending_calls.len(), 2);
        assert_eq!(state.pending_call.as_ref().unwrap().id, "call_1");
        assert_eq!(state.command, "sudo apt update && sudo apt upgrade -y");
    }
}
