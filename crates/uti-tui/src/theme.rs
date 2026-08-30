use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub accent_blue: Color,
    pub accent_purple: Color,
    pub accent_cyan: Color,
    pub accent_green: Color,
    pub accent_yellow: Color,
    pub accent_red: Color,
    pub diff_added_bg: Color,
    pub diff_added_fg: Color,
    pub diff_removed_bg: Color,
    pub diff_removed_fg: Color,
    pub gray: Color,
    pub dark_gray: Color,
    pub border: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            // Use Color::Reset to preserve terminal transparency and custom terminal themes
            background: Color::Reset,
            foreground: Color::Reset,
            accent_blue: Color::Rgb(135, 175, 255),
            accent_purple: Color::Rgb(215, 175, 255),
            accent_cyan: Color::Rgb(135, 215, 215),
            accent_green: Color::Rgb(215, 255, 215),
            accent_yellow: Color::Rgb(255, 255, 175),
            accent_red: Color::Rgb(255, 135, 175),
            diff_added_bg: Color::Rgb(20, 66, 18),
            diff_added_fg: Color::Rgb(85, 255, 85),
            diff_removed_bg: Color::Rgb(102, 0, 0),
            diff_removed_fg: Color::Rgb(255, 85, 85),
            gray: Color::Rgb(175, 175, 175),
            dark_gray: Color::Rgb(95, 95, 95),
            border: Color::Rgb(135, 175, 255),
        }
    }
}

pub fn interpolate_rgb(c1: (u8, u8, u8), c2: (u8, u8, u8), t: f32) -> Color {
    let r = (c1.0 as f32 + (c2.0 as f32 - c1.0 as f32) * t).round() as u8;
    let g = (c1.1 as f32 + (c2.1 as f32 - c1.1 as f32) * t).round() as u8;
    let b = (c1.2 as f32 + (c2.2 as f32 - c1.2 as f32) * t).round() as u8;
    Color::Rgb(r, g, b)
}

pub fn gradient_colors(steps: usize) -> Vec<Color> {
    // Gradient: #4796E4 -> #847ACE -> #C3677F
    let c1 = (0x47, 0x96, 0xE4);
    let c2 = (0x84, 0x7A, 0xCE);
    let c3 = (0xC3, 0x67, 0x7F);

    let mut colors = Vec::with_capacity(steps);
    for i in 0..steps {
        let t = if steps <= 1 {
            0.0
        } else {
            i as f32 / (steps - 1) as f32
        };

        let color = if t < 0.5 {
            interpolate_rgb(c1, c2, t * 2.0)
        } else {
            interpolate_rgb(c2, c3, (t - 0.5) * 2.0)
        };
        colors.push(color);
    }
    colors
}
