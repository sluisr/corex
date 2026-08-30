use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::overlay::render_scrim;
use crate::theme::Theme;

pub struct AuthDialogState {
    pub is_open: bool,
    pub input_buffer: String,
    pub error_msg: Option<String>,
}

impl AuthDialogState {
    pub fn new() -> Self {
        Self {
            is_open: false,
            input_buffer: String::new(),
            error_msg: None,
        }
    }

    pub fn open(&mut self) {
        self.is_open = true;
        self.input_buffer.clear();
        self.error_msg = None;
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.input_buffer.clear();
        self.error_msg = None;
    }
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

    let dialog_width = (area.width * 80 / 100).max(50).min(area.width);
    let dialog_height = 11.min(area.height);

    let x = (area.width - dialog_width) / 2;
    let y = (area.height - dialog_height) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Dim the chat behind the modal so it does not visually collide.
    render_scrim(frame, area);
    frame.render_widget(Clear, dialog_area);

    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter DeepSeek API Key",
        Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Please enter your DeepSeek API key. It will be securely stored in ~/.uti/settings.json",
        Style::default().fg(theme.gray),
    )));
    lines.push(Line::from(vec![
        Span::styled("You can get an API key from: ", Style::default().fg(theme.gray)),
        Span::styled("https://platform.deepseek.com/api_keys", Style::default().fg(theme.accent_blue).add_modifier(Modifier::UNDERLINED)),
    ]));
    lines.push(Line::from(""));

    // Input box
    let masked_or_raw = if state.input_buffer.is_empty() {
        Span::styled("Paste your API key here (sk-...)", Style::default().fg(theme.dark_gray))
    } else {
        Span::styled(&state.input_buffer, Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD))
    };

    lines.push(Line::from(vec![
        Span::styled("❯ ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
        masked_or_raw,
        Span::styled("█", Style::default().fg(theme.accent_blue)),
    ]));

    if let Some(ref err) = state.error_msg {
        lines.push(Line::from(Span::styled(format!("❌ {}", err), Style::default().fg(Color::Red))));
    } else {
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        "(Press Enter to submit, Esc to exit)",
        Style::default().fg(theme.gray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent_blue));

    frame.render_widget(Paragraph::new(lines).block(block), dialog_area);
}
