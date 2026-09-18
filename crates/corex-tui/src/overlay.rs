use ratatui::layout::Rect;
use ratatui::Frame;

/// Preserves native terminal transparency and custom terminal themes.
/// Modal dialogs use a solid opaque surface background instead of blacking out the entire screen.
pub fn render_scrim(_frame: &mut Frame, _area: Rect) {
    // No-op: prevents darkening the entire background while modals remain transparent.
}
