//! Shared blink phase for the text cursors drawn as glyphs (`█`, `_`) in the composer and dialogs.
//!
//! The event loop owns the clock: it derives the phase from the time since the last input event
//! (so the caret stays solid while typing) and publishes it with [`set_visible`]. Renderers only
//! read it, which keeps them pure and makes tests see a steady caret.
//!
//! The blink stops after [`BLINK_MAX_IDLE`]: every repaint is terminal output, and the terminal
//! cancels its own selection (Shift+drag) the moment the application writes anything. Blinking
//! forever keeps wiping the selection the user is dragging.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Half period of the blink: the caret is on for this long, then off for this long.
pub const BLINK_HALF_PERIOD: Duration = Duration::from_millis(530);

/// How long the caret blinks after the last input; past that it stays lit and the screen goes
/// silent, so the terminal's selection survives. Being a multiple of the half period lands the
/// freeze on a lit phase, which avoids a stray flash when the blink stops.
pub const BLINK_MAX_IDLE: Duration = Duration::from_millis(4 * BLINK_HALF_PERIOD.as_millis() as u64);

static VISIBLE: AtomicBool = AtomicBool::new(true);

/// Whether the caret is lit `idle` after the last input. Solid for the first half period, blinking
/// for a couple of seconds, then solid again so an idle screen writes nothing.
pub fn phase_visible(idle: Duration) -> bool {
    if idle >= BLINK_MAX_IDLE {
        return true;
    }
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

    /// Past the idle window the phase must stay lit: `tick` only redraws on a change, so a stable
    /// phase is what keeps an idle screen from writing and cancelling the terminal's selection.
    #[test]
    fn caret_freezes_lit_and_stops_repainting_when_idle() {
        assert!(phase_visible(BLINK_MAX_IDLE));
        assert!(phase_visible(BLINK_MAX_IDLE + BLINK_HALF_PERIOD));
        assert!(phase_visible(Duration::from_secs(60)));
        assert!(BLINK_MAX_IDLE.as_millis() % BLINK_HALF_PERIOD.as_millis() == 0);
    }
}
