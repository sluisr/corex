//! System clipboard integration.
//!
//! While mouse reporting is on, the terminal stops offering its native drag-to-select, so the TUI
//! owns the drag it consumes and is responsible for pushing the copied text to the real system
//! clipboard itself.

use std::io::Write;
use std::process::{Child, Command, Stdio};

/// How the text actually reached the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Wayland session clipboard via `wl-copy`.
    Wayland,
    /// X11 session clipboard via `xclip` or `xsel`.
    X11,
    /// OSC 52 escape sequence, forwarded to the clipboard by the terminal emulator.
    Osc52,
}

impl Route {
    /// Human-readable name of the route, for logs and messages.
    pub fn label(self) -> &'static str {
        match self {
            Route::Wayland => "Wayland clipboard",
            Route::X11 => "X11 clipboard",
            Route::Osc52 => "OSC 52",
        }
    }
}

/// Copies `text` to the system clipboard, trying every route available in this session.
///
/// Returns the route that took it, or the reason every route failed.
pub fn copy(text: &str) -> Result<Route, String> {
    if text.is_empty() {
        return Err("nothing to copy".to_string());
    }

    // Wayland first: it reaches the real session clipboard even from inside tmux, which is
    // exactly the setup where the terminal's own selection is most degraded.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && pipe_to("wl-copy", &[], text).is_ok() {
        return Ok(Route::Wayland);
    }

    if std::env::var_os("DISPLAY").is_some() {
        if pipe_to("xclip", &["-selection", "clipboard"], text).is_ok()
            || pipe_to("xsel", &["--clipboard", "--input"], text).is_ok()
        {
            return Ok(Route::X11);
        }
    }

    // Last resort: let the terminal emulator do it. tmux forwards the sequence too when it runs
    // with `set-clipboard on`.
    write_osc52(text)?;
    Ok(Route::Osc52)
}

/// Feeds `text` to a clipboard utility through its stdin.
///
/// The utility is left running: `wl-copy` and `xclip` both fork and keep serving the selection so
/// whoever asked for it can still paste, which also means waiting on them here would block the
/// UI. A detached thread reaps the process once it finally exits.
fn pipe_to(program: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("{program}: {err}"))?;

    let write_result = match child.stdin.as_mut() {
        Some(stdin) => stdin.write_all(text.as_bytes()).map_err(|err| err.to_string()),
        None => Err(format!("{program}: no stdin")),
    };

    // Closing stdin is what tells the utility the payload is complete.
    drop(child.stdin.take());

    if let Err(err) = write_result {
        let _ = child.kill();
        return Err(err);
    }

    reap(child);
    Ok(())
}

/// Waits for a clipboard utility off the UI thread, so it is not left as a zombie.
fn reap(mut child: Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Emits the OSC 52 clipboard sequence.
///
/// Terminals that implement it (and tmux with `set-clipboard on`) forward the payload to the
/// system clipboard even over a remote shell.
fn write_osc52(text: &str) -> Result<(), String> {
    let payload = osc52_payload(text);
    let mut stdout = std::io::stdout();
    stdout
        .write_all(format!("\x1b]52;c;{payload}\x07").as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|err| err.to_string())
}

/// Base64 payload of an OSC 52 clipboard write.
fn osc52_payload(text: &str) -> String {
    use base64::Engine;

    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_selection_never_touches_the_clipboard() {
        assert!(copy("").is_err());
    }

    #[test]
    fn osc52_payload_is_standard_base64() {
        // "hey" -> aGkZ... the canonical encoding, plus padding on demand.
        assert_eq!(osc52_payload("hey"), "aGV5");
        assert_eq!(osc52_payload("a"), "YQ==");
        assert_eq!(osc52_payload(""), "");
    }

    #[test]
    fn routes_report_a_readable_name() {
        assert_eq!(Route::Wayland.label(), "Wayland clipboard");
        assert_eq!(Route::X11.label(), "X11 clipboard");
        assert_eq!(Route::Osc52.label(), "OSC 52");
    }
}
