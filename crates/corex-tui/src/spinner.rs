//! The activity-bar spinner: light braille dots, or plain ASCII where braille cannot be drawn.

use std::sync::OnceLock;

const DOTS: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const ASCII: [&str; 4] = ["|", "/", "-", "\\"];

/// Frames per second, shared with the rest of the activity animations.
const FPS: f32 = 10.0;

/// ASCII is used on the Linux virtual console (no braille in its font) and when the locale is
/// explicitly not UTF-8. An unset locale keeps the dots: most terminals draw them fine.
fn needs_ascii(term: Option<&str>, locale: Option<&str>) -> bool {
    if term == Some("linux") {
        return true;
    }
    locale.is_some_and(|l| {
        let l = l.to_ascii_lowercase();
        !(l.contains("utf-8") || l.contains("utf8"))
    })
}

fn frames() -> &'static [&'static str] {
    static FRAMES: OnceLock<&'static [&'static str]> = OnceLock::new();
    FRAMES.get_or_init(|| {
        let term = std::env::var("TERM").ok();
        // The first non-empty of LC_ALL / LC_CTYPE / LANG decides, as in the C library.
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()));
        if needs_ascii(term.as_deref(), locale.as_deref()) { &ASCII } else { &DOTS }
    })
}

/// The glyph to draw `elapsed_secs` into the turn.
pub fn frame(elapsed_secs: f32) -> &'static str {
    let f = frames();
    f[(elapsed_secs * FPS) as usize % f.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_only_on_the_console_or_a_non_utf8_locale() {
        assert!(needs_ascii(Some("linux"), Some("en_US.UTF-8")));
        assert!(needs_ascii(Some("xterm-256color"), Some("C")));
        assert!(needs_ascii(None, Some("POSIX")));
        assert!(needs_ascii(None, Some("es_ES.ISO-8859-1")));
        assert!(!needs_ascii(Some("xterm-256color"), Some("es_ES.UTF-8")));
        assert!(!needs_ascii(Some("xterm-256color"), Some("en_US.utf8")));
        assert!(!needs_ascii(None, None));
    }

    #[test]
    fn frames_advance_ten_times_a_second_and_wrap() {
        let f = frames();
        assert_eq!(frame(0.0), f[0]);
        assert_eq!(frame(0.1), f[1 % f.len()]);
        assert_eq!(frame(0.1 * f.len() as f32 + 0.01), f[0]);
    }
}
