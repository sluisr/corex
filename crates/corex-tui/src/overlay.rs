use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
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

/// Dialog shortcuts, the same everywhere: the key bright, its action dim, `·` between pairs.
pub fn key_hints(pairs: &[(&'static str, &'static str)], theme: &crate::theme::Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (key, action)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(theme.dark_gray)));
        }
        spans.push(Span::styled(format!("{key} "), Style::default().fg(theme.text_strong)));
        spans.push(Span::styled(*action, Style::default().fg(theme.dark_gray)));
    }
    spans
}

pub fn panel_rules(frame: &mut Frame, area: Rect, theme: &crate::theme::Theme) {
    if area.height < 2 {
        return;
    }
    let rule = || ratatui::widgets::Paragraph::new(Span::styled("─".repeat(area.width as usize), Style::default().fg(theme.border_dim)));
    frame.render_widget(rule(), Rect::new(area.x, area.y, area.width, 1));
    frame.render_widget(rule(), Rect::new(area.x, area.bottom() - 1, area.width, 1));
}

pub fn docked_panel(frame: &mut Frame, area: Rect, theme: &crate::theme::Theme) -> Rect {
    frame.render_widget(Clear, area);
    panel_rules(frame, area, theme);
    Rect::new(area.x, area.y.saturating_add(1), area.width, area.height.saturating_sub(2))
}

pub fn docked_height(body_rows: usize, screen_height: u16) -> u16 {
    (body_rows as u16 + DOCKED_CHROME_ROWS).min(screen_height.saturating_sub(6)).max(3)
}

pub const DOCKED_CHROME_ROWS: u16 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Cursor,
    Current,
    Idle,
}

impl Row {
    pub fn of(selected: bool, focused: bool) -> Self {
        match (selected, focused) {
            (true, true) => Self::Cursor,
            (true, false) => Self::Current,
            _ => Self::Idle,
        }
    }
}

pub fn rail(row: Row, theme: &crate::theme::Theme) -> Span<'static> {
    match row {
        Row::Cursor => Span::styled(" › ", Style::default().fg(theme.accent_blue)),
        Row::Current => Span::styled(" › ", Style::default().fg(theme.border_dim)),
        Row::Idle => Span::raw("   "),
    }
}

pub fn row_text(row: Row, idle: Color, theme: &crate::theme::Theme) -> Style {
    match row {
        Row::Cursor => Style::default().fg(theme.text_on_selected).add_modifier(Modifier::BOLD),
        _ => Style::default().fg(idle),
    }
}

pub fn highlight_cursor(row: Row, mut line: Line<'static>, width: usize, theme: &crate::theme::Theme) -> Line<'static> {
    if row != Row::Cursor {
        return line;
    }
    let used: usize = line.spans.iter().map(|s| s.width()).sum();
    if used < width {
        line.spans.push(Span::raw(" ".repeat(width - used)));
    }
    for span in &mut line.spans {
        span.style = span.style.bg(theme.selected_bg);
    }
    line
}
