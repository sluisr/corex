use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use crate::theme::Theme;

pub struct ThinkingState {
    pub content: String,
    pub is_streaming: bool,
    pub is_expanded: bool,
    pub elapsed_secs: f32,
}

impl Default for ThinkingState {
    fn default() -> Self {
        Self::new()
    }
}

impl ThinkingState {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            is_streaming: false,
            is_expanded: false,
            elapsed_secs: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.content.clear();
        self.is_streaming = false;
        self.elapsed_secs = 0.0;
    }

    pub fn render_line(&self, theme: &Theme) -> Line<'static> {
        let spinner = if self.is_streaming {
            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let idx = ((self.elapsed_secs * 10.0) as usize) % frames.len();
            frames[idx]
        } else {
            "✓"
        };

        let status_text = if self.is_streaming {
            format!("Thinking... ({:.1}s) [Ctrl+T to toggle]", self.elapsed_secs)
        } else {
            format!("Thinking completed ({:.1}s) [Ctrl+T to toggle]", self.elapsed_secs)
        };

        Line::from(vec![
            Span::styled(format!("{} ", spinner), Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
            Span::styled(status_text, Style::default().fg(theme.accent_purple)),
        ])
    }

    pub fn render_expanded_block(&self, theme: &Theme) -> Paragraph<'static> {
        let lines: Vec<Line> = self
            .content
            .lines()
            .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(theme.gray))))
            .collect();

        let block = Block::default()
            .title(format!(" Thinking Process ({:.1}s) ", self.elapsed_secs))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.dark_gray));

        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
    }
}
