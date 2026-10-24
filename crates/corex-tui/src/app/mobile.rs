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

/// Attempts to trigger Termux soft keyboard via termux-api if installed.
/// Rate-limited to at most once per 1000ms to prevent thread storms during rapid taps.
pub fn try_show_termux_soft_keyboard() {
    if is_android_or_termux() {
        static LAST_TRIGGER_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = LAST_TRIGGER_MS.load(std::sync::atomic::Ordering::Relaxed);
        if now_ms.saturating_sub(last) < 1000 {
            return;
        }
        LAST_TRIGGER_MS.store(now_ms, std::sync::atomic::Ordering::Relaxed);

        std::thread::spawn(|| {
            if let Ok(mut child) = std::process::Command::new("termux-keyboard")
                .arg("-s")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                let _ = child.wait();
            }
        });
    }
}

/// Whether the TUI should request mouse reporting from the terminal.
///
/// On mobile vertical viewports (e.g., Termux on Android), mouse reporting is disabled by default
/// so touch events trigger the native soft keyboard rather than being intercepted as mouse clicks.
///
/// Set `COREX_MOUSE=1` to force mouse reporting on mobile, or `COREX_MOUSE=0` to force it off everywhere.
pub(crate) fn mouse_capture_enabled() -> bool {
    if let Ok(v) = std::env::var("COREX_MOUSE") {
        return !matches!(v.trim().to_lowercase().as_str(), "0" | "false" | "no" | "off");
    }
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    if is_mobile_portrait(cols, rows) {
        return false;
    }
    true
}
