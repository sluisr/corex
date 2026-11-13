use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::overlay::{docked_height, docked_panel, key_hints, rail, Row};
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct AuthDialogState {
    pub is_open: bool,
    pub input_buffer: String,
    pub error_msg: Option<String>,
    pub provider_name: String,
    pub provider_label: String,
    pub api_key_env: String,
    pub portal_url: String,
    pub pending_activation: Option<(usize, usize)>,
}

impl Default for AuthDialogState {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthDialogState {
    pub fn new() -> Self {
        Self {
            is_open: false,
            input_buffer: String::new(),
            error_msg: None,
            provider_name: "deepseek".to_string(),
            provider_label: "DeepSeek".to_string(),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            portal_url: "https://platform.deepseek.com".to_string(),
            pending_activation: None,
        }
    }

    pub fn open(&mut self) {
        self.open_for_provider(None, "deepseek", "DeepSeek", "DEEPSEEK_API_KEY", "https://platform.deepseek.com");
    }

    pub fn open_for_provider(
        &mut self,
        pending: Option<(usize, usize)>,
        name: &str,
        label: &str,
        env_var: &str,
        portal_url: &str,
    ) {
        self.is_open = true;
        self.input_buffer.clear();
        self.error_msg = None;
        self.provider_name = name.to_string();
        self.provider_label = label.to_string();
        self.api_key_env = env_var.to_string();
        self.portal_url = portal_url.to_string();
        self.pending_activation = pending;
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.input_buffer.clear();
        self.error_msg = None;
        self.pending_activation = None;
    }
}

/// Note to show after saving `new_key` for `provider` when an environment variable still holds a
/// different DeepSeek key: `Config::load` lets it override `settings.json` on the next launch.
pub fn env_override_note(provider: &str, new_key: &str) -> Option<String> {
    if provider != "deepseek" {
        return None;
    }
    ["COREX_API_KEY", "DEEPSEEK_API_KEY"].into_iter().find_map(|var| {
        let val = std::env::var(var).ok()?;
        (!val.trim().is_empty() && val.trim() != new_key).then(|| {
            format!(
                " ${var} is set to a different key and overrides it on the next launch: update or unset it."
            )
        })
    })
}

const BODY_ROWS: usize = 4;

pub fn dialog_height(screen_height: u16) -> u16 {
    docked_height(BODY_ROWS, screen_height)
}

pub fn render_auth_dialog(
    frame: &mut Frame,
    area: Rect,
    state: &AuthDialogState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let inner = docked_panel(frame, area, theme);
    let dim = Style::default().fg(theme.dark_gray);

    // Fully masked: no character of the key is ever drawn.
    let input = if state.input_buffer.is_empty() {
        Span::styled("paste or type the key", dim)
    } else {
        Span::styled("*".repeat(state.input_buffer.chars().count()), Style::default().fg(theme.foreground))
    };
    let note = match &state.error_msg {
        Some(err) => Line::from(Span::styled(format!(" {err}"), Style::default().fg(theme.error))),
        None => Line::from(vec![
            Span::styled(" Get one at ", dim),
            Span::styled(state.portal_url.clone(), Style::default().fg(theme.link).add_modifier(Modifier::UNDERLINED)),
        ]),
    };
    let mut hints = vec![Span::raw(" ")];
    hints.extend(key_hints(&[("Enter", "save & activate"), ("Esc", "cancel")], theme));

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {} API key", state.provider_label),
            Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD),
        )),
        rows[0],
    );
    let caret = Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_blue));
    let field = if state.input_buffer.is_empty() {
        vec![rail(Row::Cursor, theme), caret, Span::raw(" "), input]
    } else {
        vec![rail(Row::Cursor, theme), input, caret]
    };
    let body = vec![
        Line::from(field),
        Line::default(),
        Line::from(Span::styled(format!(" Saved for ${} and the system keyring", state.api_key_env), dim)),
        note,
    ];
    frame.render_widget(Paragraph::new(body), rows[2]);
    frame.render_widget(Paragraph::new(Line::from(hints)), rows[4]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_note_only_for_deepseek_with_a_different_env_key() {
        // Only the DeepSeek key is overridden by the environment at load time.
        assert_eq!(env_override_note("openai", "sk-new"), None);
    }

    #[test]
    fn auth_dialog_masks_api_key_completely() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let theme = Theme::default();
        let mut auth = AuthDialogState::new();
        auth.open();
        auth.input_buffer = "sk-ant-api03-secret123456789".to_string();

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render_auth_dialog(f, f.area(), &auth, &theme)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content().iter().map(|c| c.symbol()).collect();

        // Must not contain any plaintext fragments of the key
        assert!(!text.contains("sk-ant"));
        assert!(!text.contains("6789"));
        // Must contain fully masked stars
        assert!(text.contains(&"*".repeat(auth.input_buffer.len())));
    }

    #[test]
    fn dialogs_fit_tiny_terminals() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let theme = Theme::default();
        let mut auth = AuthDialogState::new();
        auth.open();
        let mut sudo = crate::sudo_dialog::SudoDialogState::new();
        sudo.open(corex_core::types::ToolCall::new("1", "run_shell_command", "{}"), "sudo ls".to_string());
        for (w, h) in [(30, 24), (43, 24), (10, 5), (80, 2), (80, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| render_auth_dialog(f, f.area(), &auth, &theme)).unwrap();
            terminal.draw(|f| crate::sudo_dialog::render_sudo_dialog(f, f.area(), &sudo, &theme)).unwrap();
        }
    }
}
