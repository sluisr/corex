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
    /// Readable secondary text (tool output, thought bodies).
    pub gray: Color,
    /// Hints and key legends (`(Esc to cancel)`). Kept at >= 4.5:1 on both black and white
    /// terminals: it is text people need to read, not decoration.
    pub dark_gray: Color,
    pub border: Color,
    /// Decorative frames and separators: allowed to be quiet because nothing is written in them.
    pub border_dim: Color,
    /// Background used to highlight a mouse selection in the chat feed.
    pub selection_bg: Color,
    /// Semantic status colours: every ✕ / ✓ / warning in the UI draws from these, so one
    /// failure never shows up in four different reds.
    pub error: Color,
    pub success: Color,
    pub warning: Color,
    /// Secondary text (labels, hints, separators) that should recede behind the content.
    pub muted: Color,
    pub link: Color,
    /// Text drawn on top of an explicit dark row background (selected list rows).
    pub text_on_selected: Color,
    /// Emphasised text that must stand out from the plain foreground (titles, values).
    pub text_strong: Color,
    /// Background of the highlighted row in dialogs and lists.
    pub selected_bg: Color,
    /// Hairline rules between dialog sections.
    pub rule: Color,
    /// Background of inline code spans.
    pub code_bg: Color,
    /// Reasoning-effort scale shown in the footer: low < high < max (`none` uses `muted`).
    pub effort_low: Color,
    pub effort_high: Color,
    pub effort_max: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// Palette for dark terminals.
    pub fn dark() -> Self {
        Self {
            // Use Color::Reset to preserve terminal transparency and custom terminal themes
            background: Color::Reset,
            foreground: Color::Reset,
            accent_blue: Color::Rgb(135, 175, 255),
            accent_purple: Color::Rgb(215, 175, 255),
            accent_cyan: Color::Rgb(135, 215, 215),
            accent_green: Color::Rgb(135, 205, 155),
            accent_yellow: Color::Rgb(225, 195, 110),
            accent_red: Color::Rgb(225, 105, 105),
            diff_added_bg: Color::Rgb(20, 66, 18),
            diff_added_fg: Color::Rgb(85, 255, 85),
            diff_removed_bg: Color::Rgb(102, 0, 0),
            diff_removed_fg: Color::Rgb(255, 85, 85),
            gray: Color::Rgb(175, 175, 175),
            dark_gray: Color::Rgb(140, 140, 140),
            border: Color::Rgb(135, 175, 255),
            border_dim: Color::Rgb(95, 95, 95),
            // The accent blue at about 30% over a dark background, a stand-in for a translucent
            // highlight (terminals have no alpha): the text keeps its colour and stays readable.
            selection_bg: Color::Rgb(44, 66, 110),
            error: Color::Rgb(225, 105, 105),
            success: Color::Rgb(105, 185, 135),
            warning: Color::Rgb(220, 180, 90),
            muted: Color::Rgb(128, 138, 152),
            link: Color::Rgb(105, 185, 235),
            text_on_selected: Color::White,
            text_strong: Color::Rgb(225, 230, 240),
            selected_bg: Color::Rgb(26, 38, 58),
            rule: Color::Rgb(50, 60, 75),
            code_bg: Color::Rgb(30, 35, 45),
            effort_low: Color::Rgb(160, 200, 225),
            effort_high: Color::Rgb(185, 160, 225),
            effort_max: Color::Rgb(215, 165, 240),
        }
    }

    /// Palette for light terminals: every text colour is darkened to stay >= 4.5:1 on white.
    pub fn light() -> Self {
        Self {
            background: Color::Reset,
            foreground: Color::Reset,
            accent_blue: Color::Rgb(25, 80, 190),
            accent_purple: Color::Rgb(115, 55, 175),
            accent_cyan: Color::Rgb(0, 110, 125),
            accent_green: Color::Rgb(20, 115, 50),
            accent_yellow: Color::Rgb(135, 90, 0),
            accent_red: Color::Rgb(180, 35, 35),
            diff_added_bg: Color::Rgb(212, 245, 212),
            diff_added_fg: Color::Rgb(0, 100, 0),
            diff_removed_bg: Color::Rgb(250, 215, 215),
            diff_removed_fg: Color::Rgb(160, 0, 0),
            gray: Color::Rgb(80, 80, 80),
            dark_gray: Color::Rgb(105, 105, 105),
            border: Color::Rgb(25, 80, 190),
            border_dim: Color::Rgb(185, 185, 185),
            selection_bg: Color::Rgb(190, 210, 245),
            error: Color::Rgb(180, 35, 35),
            success: Color::Rgb(20, 115, 50),
            warning: Color::Rgb(150, 95, 0),
            muted: Color::Rgb(95, 105, 120),
            link: Color::Rgb(0, 95, 170),
            text_on_selected: Color::Black,
            text_strong: Color::Rgb(25, 30, 40),
            selected_bg: Color::Rgb(215, 226, 245),
            rule: Color::Rgb(205, 210, 220),
            code_bg: Color::Rgb(236, 238, 243),
            effort_low: Color::Rgb(20, 105, 150),
            effort_high: Color::Rgb(100, 60, 170),
            effort_max: Color::Rgb(140, 40, 175),
        }
    }

    /// Picks the palette for the running terminal.
    ///
    /// `COREX_THEME=light|dark` always wins. Otherwise `COLORFGBG` ("fg;bg", set by rxvt,
    /// Konsole, iTerm2 and others) decides: background colours 7 and 15 are light. Anything
    /// unknown keeps the dark palette the UI was designed around.
    pub fn detect() -> Self {
        Self::from_env(
            std::env::var("COREX_THEME").ok().as_deref(),
            std::env::var("COLORFGBG").ok().as_deref(),
        )
    }

    fn from_env(corex_theme: Option<&str>, colorfgbg: Option<&str>) -> Self {
        match corex_theme.map(|v| v.trim().to_lowercase()).as_deref() {
            Some("light") => return Self::light(),
            Some("dark") => return Self::dark(),
            _ => {}
        }
        let light_bg = colorfgbg
            .and_then(|v| v.rsplit(';').next())
            .and_then(|bg| bg.trim().parse::<u8>().ok())
            .is_some_and(|bg| bg == 7 || bg == 15);
        if light_bg { Self::light() } else { Self::dark() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color) -> f64 {
        let Color::Rgb(r, g, b) = c else { return 0.0 };
        let f = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn dark_text_colours_are_readable_on_black() {
        let t = Theme::dark();
        for (name, c) in [("dark_gray", t.dark_gray), ("muted", t.muted), ("gray", t.gray)] {
            assert!(contrast(c, Color::Rgb(0, 0, 0)) >= 4.5, "{name}");
        }
    }

    #[test]
    fn light_text_colours_are_readable_on_white() {
        let t = Theme::light();
        let white = Color::Rgb(255, 255, 255);
        for (name, c) in [
            ("gray", t.gray),
            ("dark_gray", t.dark_gray),
            ("muted", t.muted),
            ("accent_blue", t.accent_blue),
            ("accent_purple", t.accent_purple),
            ("accent_cyan", t.accent_cyan),
            ("accent_green", t.accent_green),
            ("accent_yellow", t.accent_yellow),
            ("error", t.error),
            ("success", t.success),
            ("warning", t.warning),
            ("link", t.link),
            ("effort_low", t.effort_low),
            ("effort_high", t.effort_high),
            ("effort_max", t.effort_max),
        ] {
            assert!(contrast(c, white) >= 4.5, "{name} = {:.1}", contrast(c, white));
        }
    }

    #[test]
    fn theme_is_picked_from_the_environment() {
        let is_light = |t: Theme| t.gray == Theme::light().gray;
        assert!(!is_light(Theme::from_env(None, None)));
        assert!(is_light(Theme::from_env(None, Some("0;15"))));
        assert!(is_light(Theme::from_env(None, Some("0;default;7"))));
        assert!(!is_light(Theme::from_env(None, Some("15;0"))));
        assert!(is_light(Theme::from_env(Some("LIGHT"), Some("15;0"))));
        assert!(!is_light(Theme::from_env(Some("dark"), Some("0;15"))));
    }
}
