//! Shared blink phase for the text cursors drawn as glyphs (`█`, `_`) in the composer and dialogs.
//!
//! The event loop owns the clock: it derives the phase from the time since the last input event
//! (so the caret stays solid while typing) and publishes it with [`set_visible`]. Renderers only
//! read it, which keeps them pure and makes tests see a steady caret.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Half period of the blink: the caret is on for this long, then off for this long.
pub const BLINK_HALF_PERIOD: Duration = Duration::from_millis(530);

static VISIBLE: AtomicBool = AtomicBool::new(true);

/// Whether the caret is lit `idle` after the last input. Solid for the first half period.
pub fn phase_visible(idle: Duration) -> bool {
    (idle.as_millis() / BLINK_HALF_PERIOD.as_millis()) % 2 == 0
}

/// Publishes the current phase. Returns true when it changed, i.e. when a redraw is due.
pub fn set_visible(visible: bool) -> bool {
    VISIBLE.swap(visible, Ordering::Relaxed) != visible
}

/// Advances the blink to `idle` after the last input. Returns true when a redraw is due.
pub fn tick(idle: Duration) -> bool {
    set_visible(phase_visible(idle))
}

/// The block caret, or a blank of the same width while it is dark.
pub fn block() -> &'static str {
    if VISIBLE.load(Ordering::Relaxed) { "█" } else { " " }
}

/// The underscore caret, or a blank of the same width while it is dark.
pub fn underscore() -> &'static str {
    if VISIBLE.load(Ordering::Relaxed) { "_" } else { " " }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_is_solid_while_typing_then_blinks() {
        assert!(phase_visible(Duration::ZERO));
        assert!(phase_visible(Duration::from_millis(529)));
        assert!(!phase_visible(Duration::from_millis(530)));
        assert!(!phase_visible(Duration::from_millis(1059)));
        assert!(phase_visible(Duration::from_millis(1060)));
    }
}
