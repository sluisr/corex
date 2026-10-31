//! Mobile / Termux responsive layout and keyboard routines.

/// True if running inside an Android or Termux environment.
pub fn is_android_or_termux() -> bool {
    std::env::var("TERMUX_VERSION").is_ok()
        || std::env::var("PREFIX").map(|p| p.contains("com.termux")).unwrap_or(false)
        || std::env::var("ANDROID_ROOT").is_ok()
        || std::path::Path::new("/data/data/com.termux").exists()
}

/// Returns true if the TUI is running in a mobile vertical (portrait) viewport.
///
/// Triggers exclusively when:
/// 1. Running inside Android/Termux on a portrait screen (`height >= width || width < 75`).
/// 2. Manually requested via `COREX_MOBILE=1` environment variable.
///
/// On PC / desktop terminals, this always returns false to preserve the standard full desktop interface.
pub fn is_mobile_portrait(width: u16, height: u16) -> bool {
    if let Ok(v) = std::env::var("COREX_MOBILE") {
        return matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on");
    }
    if is_android_or_termux() && (height >= width || width < 75) {
        return true;
    }
    false
}

/// Returns true if running in a touch or mobile environment (Android, Termux, or mobile portrait).
pub fn is_mobile_touch_environment(width: u16, height: u16) -> bool {
    is_android_or_termux() || is_mobile_portrait(width, height)
}

/// How long mouse reporting stays off after a tap so Termux can confirm it and raise the keyboard.
const TAP_KEYBOARD_PAUSE: std::time::Duration = std::time::Duration::from_millis(700);

/// How long `/keyboard` leaves mouse reporting off, waiting for the tap that raises the keyboard.
const KEYBOARD_REQUEST_PAUSE: std::time::Duration = std::time::Duration::from_secs(8);

/// Lets a tap raise the soft keyboard while mouse reporting stays on for swipes.
///
/// Termux reports a tap to the app as soon as the finger lifts, but only decides about the keyboard
/// when the gesture detector confirms the tap, some 300 ms later (`onSingleTapConfirmed`), and it
/// raises it only if mouse tracking is off at that moment (`TermuxTerminalViewClient.onSingleTapUp`).
/// There is no command that shows it: `termux-api` has none. So on a tap, reporting is switched off
/// for a moment and back on afterwards.
#[derive(Debug, Default)]
pub struct TapKeyboardGate {
    resume_at: Option<std::time::Instant>,
    /// End of the pause a tap started. Not set by `/keyboard`, whose pause is much longer.
    tap_until: Option<std::time::Instant>,
}

impl TapKeyboardGate {
    /// A tap arrived at `now`: turn mouse reporting off until Termux has handled it.
    pub(crate) fn on_tap(&mut self, now: std::time::Instant, mouse_capture: &mut bool) {
        if *mouse_capture {
            self.tap_until = Some(now + TAP_KEYBOARD_PAUSE);
        }
        self.pause_for(now, TAP_KEYBOARD_PAUSE, mouse_capture);
    }

    /// True while a tap holds reporting off: a swipe started now arrives as bare arrows, and
    /// nobody presses an arrow key in the blink after tapping to raise the keyboard.
    pub(crate) fn in_tap_window(&self, now: std::time::Instant) -> bool {
        self.tap_until.is_some_and(|t| now < t)
    }

    /// `/keyboard`: leave the next tap to Termux, as nothing can raise the keyboard from here.
    pub(crate) fn request_keyboard(&mut self, now: std::time::Instant, mouse_capture: &mut bool) {
        self.pause_for(now, KEYBOARD_REQUEST_PAUSE, mouse_capture);
    }

    fn pause_for(&mut self, now: std::time::Instant, pause: std::time::Duration, mouse_capture: &mut bool) {
        if *mouse_capture {
            *mouse_capture = false;
            self.resume_at = Some(now + pause);
        }
    }

    /// Turns mouse reporting back on once the pause is over.
    pub(crate) fn resume_if_due(&mut self, now: std::time::Instant, mouse_capture: &mut bool) {
        if self.resume_at.is_some_and(|t| now >= t) {
            self.resume_at = None;
            *mouse_capture = true;
        }
    }

    /// The user flipped mouse reporting themselves (F2, `/mouse`): their choice stands.
    pub(crate) fn cancel(&mut self) {
        self.resume_at = None;
        self.tap_until = None;
    }
}

/// Lines the feed moves per wheel event. Termux sends one event per text row the finger travels,
/// so one line each keeps the chat under the finger; a desktop wheel notch is one event for three lines.
pub(crate) fn wheel_lines() -> i32 {
    if is_android_or_termux() { 1 } else { 3 }
}

/// Whether the TUI should request mouse reporting from the terminal.
///
/// Always, unless `COREX_MOUSE` turns it off. With reporting off, terminals send the wheel (and
/// Termux a finger swipe) as bare Up/Down keys, indistinguishable from the arrows that browse the
/// prompt history, and tmux drops it altogether. With it on, the wheel is a mouse event and the
/// arrows are always keys. Shift+drag still gives the terminal's own selection.
pub(crate) fn mouse_capture_enabled() -> bool {
    std::env::var("COREX_MOUSE")
        .map(|v| !matches!(v.trim().to_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(true)
}

/// Longest gap between two bare arrows that still counts as one finger swipe.
///
/// With mouse reporting off, Termux turns a finger drag on the alternate screen into one bare
/// Up/Down per text row (`TerminalView.doScroll`), byte-identical to the extra-keys arrows. Only
/// the pace tells them apart: a swipe emits rows back to back (0-16 ms apart), while a tapped
/// arrow arrives alone and a held one repeats every 80 ms (`ExtraKeysView` default).
/// `COREX_TOUCH_SCROLL_MS` tunes it; `0` turns swipe detection off.
const DEFAULT_SWIPE_BURST_MS: u64 = 50;

/// Once a swipe is detected, arrows keep scrolling until the stream pauses this long, so a slow
/// stretch in the middle of a drag does not fall back to history navigation.
const SWIPE_CONTINUE: std::time::Duration = std::time::Duration::from_millis(250);

fn swipe_burst_window() -> std::time::Duration {
    let ms = std::env::var("COREX_TOUCH_SCROLL_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|ms| ms.min(500))
        .unwrap_or(DEFAULT_SWIPE_BURST_MS);
    std::time::Duration::from_millis(ms)
}

/// True when bare Up/Down may come from a finger swipe: Termux with mouse reporting off.
/// With mouse reporting on, Termux sends swipes as wheel events, so arrows are always keys.
pub(crate) fn arrows_may_be_swipes(mouse_capture: bool) -> bool {
    !mouse_capture && is_android_or_termux()
}

/// Tells Termux finger swipes apart from real arrow key presses (see [`DEFAULT_SWIPE_BURST_MS`]).
#[derive(Debug, Default)]
pub struct TouchArrowClassifier {
    last_swipe_arrow: Option<std::time::Instant>,
}

impl TouchArrowClassifier {
    /// Classifies a bare arrow received at `now`. `input_within` waits up to the given duration
    /// for more input without consuming it; another event that quickly means a swipe burst.
    pub(crate) fn is_swipe(
        &mut self,
        now: std::time::Instant,
        input_within: impl FnOnce(std::time::Duration) -> bool,
    ) -> bool {
        let window = swipe_burst_window();
        let continues_swipe = self
            .last_swipe_arrow
            .is_some_and(|t| now.saturating_duration_since(t) <= SWIPE_CONTINUE);
        let swipe = !window.is_zero() && (continues_swipe || input_within(window));
        self.last_swipe_arrow = swipe.then_some(now);
        swipe
    }
}
