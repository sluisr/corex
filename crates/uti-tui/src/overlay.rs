use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Paints a dark scrim over the whole screen so modal dialogs stand out from
/// the chat content behind them (a terminal-friendly stand-in for a blur).
/// Call this before rendering the dialog itself.
pub fn render_scrim(frame: &mut Frame, area: Rect) {
    let scrim = Paragraph::new("").style(Style::default().bg(Color::Rgb(6, 8, 12)));
    frame.render_widget(scrim, area);
}
