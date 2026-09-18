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

    let dialog_width = 68.min(area.width.saturating_sub(4)).max(44);
    let dialog_height = 11.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Dim the chat behind the modal so it does not visually collide.
    render_scrim(frame, area);
    frame.render_widget(Clear, dialog_area);

    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter your DeepSeek API key for Cloud AI models:",
        Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled("  Saved securely in ", Style::default().fg(theme.gray)),
        Span::styled("~/.uti/settings.json", Style::default().fg(theme.accent_cyan)),
    ]));
    lines.push(Line::from(""));

    // Masked input box with stylish formatting
    let (input_span, cursor) = if state.input_buffer.is_empty() {
        (
            Span::styled("Paste or type API key (sk-...)", Style::default().fg(theme.dark_gray)),
            Span::styled("█", Style::default().fg(theme.accent_blue)),
        )
    } else {
        let buf = &state.input_buffer;
        let masked = if buf.len() > 8 {
            let prefix = &buf[..buf.len().min(4)];
            let suffix = &buf[buf.len() - 4..];
            let stars = "*".repeat(buf.len().saturating_sub(8));
            format!("{}{}{}", prefix, stars, suffix)
        } else {
            "*".repeat(buf.len())
        };
        (
            Span::styled(masked, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("█", Style::default().fg(theme.accent_blue)),
        )
    };

    lines.push(Line::from(vec![
        Span::styled("  ❯ ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
        input_span,
        cursor,
    ]));
    lines.push(Line::from(""));

    if let Some(ref err) = state.error_msg {
        lines.push(Line::from(Span::styled(
            format!("  ❌ {}", err),
            Style::default().fg(Color::LightRed),
        )));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Get a key at: ", Style::default().fg(theme.dark_gray)),
            Span::styled(
                "https://platform.deepseek.com",
                Style::default().fg(theme.accent_blue).add_modifier(Modifier::UNDERLINED),
            ),
        ]));
    }
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("[Enter]", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
        Span::styled(" Save & Connect    ", Style::default().fg(theme.gray)),
        Span::styled("[Esc]", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" Skip / Offline Mode", Style::default().fg(theme.gray)),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(ratatui::symbols::border::PLAIN)
        .border_style(Style::default().fg(theme.accent_blue))
        .title(" 🔑 DeepSeek API Key ")
        .title_alignment(ratatui::layout::Alignment::Left);

    frame.render_widget(Paragraph::new(lines).block(block), dialog_area);
}
