use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::overlay::begin_modal;
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

pub fn render_auth_dialog(
    frame: &mut Frame,
    area: Rect,
    state: &AuthDialogState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let is_mobile = crate::app::is_mobile_portrait(area.width, area.height);
    let dialog_width = if is_mobile {
        area.width.saturating_sub(2).max(20)
    } else {
        72.min(area.width.saturating_sub(4)).max(44)
    };
    let dialog_height = 11.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Dim the chat behind the modal so it does not visually collide.
    begin_modal(frame, area, dialog_area, 2, 1);

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Enter your {} API key / token:", state.provider_label),
            Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  Saved for ", Style::default().fg(theme.gray)),
            Span::styled(format!("${}", state.api_key_env), Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            Span::styled(" and ~/.corex/settings.json", Style::default().fg(theme.gray)),
        ]),
        Line::from(""),
    ];

    // Masked input box with stylish formatting
    let (input_span, cursor) = if state.input_buffer.is_empty() {
        (
            Span::styled(format!("Paste or type key ({})", state.api_key_env), Style::default().fg(theme.dark_gray)),
            Span::styled("█", Style::default().fg(theme.accent_blue)),
        )
    } else {
        let chars: Vec<char> = state.input_buffer.chars().collect();
        let total_chars = chars.len();
        let masked = if total_chars > 8 {
            let prefix: String = chars[..4].iter().collect();
            let suffix: String = chars[total_chars - 4..].iter().collect();
            let stars = "*".repeat(total_chars - 8);
            format!("{}{}{}", prefix, stars, suffix)
        } else {
            "*".repeat(total_chars)
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
            format!("  ✕ {}", err),
            Style::default().fg(Color::LightRed),
        )));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Get key/info at: ", Style::default().fg(theme.dark_gray)),
            Span::styled(
                &state.portal_url,
                Style::default().fg(theme.accent_blue).add_modifier(Modifier::UNDERLINED),
            ),
        ]));
    }
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("[Enter]", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
        Span::styled(" Save & Activate    ", Style::default().fg(theme.gray)),
        Span::styled("[Esc]", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" Cancel", Style::default().fg(theme.gray)),
    ]));

    let title_str = format!(" {} API Key ", state.provider_label);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(ratatui::symbols::border::PLAIN)
        .border_style(Style::default().fg(theme.accent_blue))
        .title(title_str)
        .title_alignment(ratatui::layout::Alignment::Left);

    frame.render_widget(Paragraph::new(lines).block(block), dialog_area);
}
