use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::widgets::Clear;
use ratatui::Frame;

/// Preserves native terminal transparency and custom terminal themes.
/// Modal dialogs use a solid opaque surface background instead of blacking out the entire screen.
pub fn render_scrim(_frame: &mut Frame, _area: Rect) {
    // No-op: prevents darkening the entire background while modals remain transparent.
}

fn dim_color(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * 0.38) as u8,
            (g as f32 * 0.38) as u8,
            (b as f32 * 0.38) as u8,
        ),
        // Reset / named / indexed colors have no known RGB value: use a muted slate.
        _ => Color::Rgb(78, 86, 100),
    }
}

/// Terminals cannot blur, so "blur" is approximated by dimming the foreground of everything
/// behind a modal. Backgrounds are left untouched so terminal transparency keeps working.
pub fn dim_background(frame: &mut Frame, area: Rect) {
    let buf = frame.buffer_mut();
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_fg(dim_color(cell.fg));
                cell.modifier.remove(Modifier::BOLD);
                cell.modifier.insert(Modifier::DIM);
            }
        }
    }
}

/// Common modal setup: dims the chat behind it, clears a `h_margin` x `v_margin` halo so chat
/// text never touches the border, then clears the dialog area itself.
pub fn begin_modal(frame: &mut Frame, area: Rect, dialog: Rect, h_margin: u16, v_margin: u16) {
    dim_background(frame, area);
    let hx = dialog.x.saturating_sub(h_margin);
    let hy = dialog.y.saturating_sub(v_margin);
    let halo = Rect::new(
        hx,
        hy,
        (dialog.right() + h_margin).saturating_sub(hx),
        (dialog.bottom() + v_margin).saturating_sub(hy),
    )
    .intersection(area);
    frame.render_widget(Clear, halo);
    frame.render_widget(Clear, dialog);
}
