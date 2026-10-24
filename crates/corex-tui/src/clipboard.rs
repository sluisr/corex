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

    if std::env::var_os("DISPLAY").is_some()
        && (pipe_to("xclip", &["-selection", "clipboard"], text).is_ok()
            || pipe_to("xsel", &["--clipboard", "--input"], text).is_ok())
    {
        return Ok(Route::X11);
    }

    // Last resort: let the terminal emulator do it. tmux forwards the sequence too when it runs
    // with `set-clipboard on`.
    write_osc52(text)?;
    Ok(Route::Osc52)
}

/// An image taken from the system clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`, from the file signature.
    pub media_type: &'static str,
    pub data: Vec<u8>,
}

/// How long a clipboard utility may take; the one holding the selection can be unresponsive.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Image formats the models accept, best first.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// Reads an image from the system clipboard.
///
/// `Ok(None)` means the clipboard holds no image (the usual case: it holds text). `Err` means there
/// is one but it cannot be used, or the clipboard could not be read at all.
pub fn read_image() -> Result<Option<ClipboardImage>, String> {
    let Some(data) = read_image_bytes()? else {
        return Ok(None);
    };
    let limit = corex_tools::fs_tools::READ_IMAGE_MAX_BYTES;
    if data.len() as u64 > limit {
        return Err(format!(
            "the image is {:.1} MB, over the {} MB limit; take a smaller screenshot",
            data.len() as f64 / (1024.0 * 1024.0),
            limit / (1024 * 1024)
        ));
    }
    match corex_tools::fs_tools::sniff_image_media_type(&data) {
        Some(media_type) => Ok(Some(ClipboardImage { media_type, data })),
        None => Err("the clipboard image is not PNG, JPEG, GIF or WebP".to_string()),
    }
}

#[cfg(target_os = "macos")]
fn read_image_bytes() -> Result<Option<Vec<u8>>, String> {
    // `pngpaste` prints the clipboard image as PNG and fails when there is none.
    match capture("pngpaste", &["-"]) {
        Ok(data) if !data.is_empty() => Ok(Some(data)),
        _ => Ok(None),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_image_bytes() -> Result<Option<Vec<u8>>, String> {
    // Same preference as `copy`: Wayland first, then X11.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Ok(listing) = capture("wl-paste", &["--list-types"]) {
            return match pick_image_type(&String::from_utf8_lossy(&listing)) {
                Some(mime) => capture("wl-paste", &["--no-newline", "--type", mime]).map(Some),
                None => Ok(None),
            };
        }
    }
    if std::env::var_os("DISPLAY").is_some() {
        if let Ok(listing) = capture("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"]) {
            return match pick_image_type(&String::from_utf8_lossy(&listing)) {
                Some(mime) => capture("xclip", &["-selection", "clipboard", "-t", mime, "-o"]).map(Some),
                None => Ok(None),
            };
        }
    }
    Ok(None)
}

#[cfg(not(unix))]
fn read_image_bytes() -> Result<Option<Vec<u8>>, String> {
    Ok(None)
}

/// Reads text from the system clipboard, or `None` when it holds none or cannot be read.
#[cfg(target_os = "macos")]
pub fn read_text() -> Option<String> {
    capture("pbpaste", &[]).ok().and_then(|bytes| String::from_utf8(bytes).ok())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn read_text() -> Option<String> {
    let bytes = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        capture("wl-paste", &["--no-newline", "--type", "text"])
    } else if std::env::var_os("DISPLAY").is_some() {
        capture("xclip", &["-selection", "clipboard", "-o"])
    } else {
        return None;
    };
    bytes.ok().and_then(|bytes| String::from_utf8(bytes).ok())
}

#[cfg(not(unix))]
pub fn read_text() -> Option<String> {
    None
}

/// The preferred image format among the types a clipboard offers (one per line).
fn pick_image_type(listing: &str) -> Option<&'static str> {
    IMAGE_TYPES
        .into_iter()
        .find(|mime| listing.lines().any(|line| line.trim().eq_ignore_ascii_case(mime)))
}

/// Runs a clipboard utility and returns what it printed, giving up after [`READ_TIMEOUT`].
#[cfg(unix)]
fn capture(program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("{program}: {err}"))?;

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(READ_TIMEOUT) {
        Ok(Ok(output)) if output.status.success() => Ok(output.stdout),
        Ok(Ok(_)) => Err(format!("{program} could not read the clipboard")),
        Ok(Err(err)) => Err(format!("{program}: {err}")),
        Err(_) => Err(format!("{program} did not answer in time")),
    }
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
    fn the_best_offered_image_type_is_chosen() {
        assert_eq!(pick_image_type("text/plain\nimage/png\nimage/jpeg\n"), Some("image/png"));
        assert_eq!(pick_image_type("text/html\nimage/webp\nimage/gif"), Some("image/webp"));
        assert_eq!(pick_image_type("text/plain\nUTF8_STRING\n"), None);
        assert_eq!(pick_image_type(""), None);
        // Whole lines only: a type that merely contains an image name is not an image.
        assert_eq!(pick_image_type("application/x-image/png-thing"), None);
    }

    /// Round trip through the real clipboard: `cargo test -p corex-tui -- --ignored clipboard_image`.
    #[test]
    #[ignore = "needs a graphical session with wl-copy or xclip"]
    fn clipboard_image_round_trip() {
        use base64::Engine;

        // A 1x1 PNG.
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==")
            .unwrap();
        let (program, args): (&str, &[&str]) = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            ("wl-copy", &["--type", "image/png"])
        } else {
            ("xclip", &["-selection", "clipboard", "-t", "image/png"])
        };
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("clipboard utility");
        child.stdin.take().unwrap().write_all(&png).unwrap();
        reap(child);
        std::thread::sleep(std::time::Duration::from_millis(300));

        let image = read_image().expect("readable").expect("an image on the clipboard");
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.data, png);

        // Text on the clipboard is not an image.
        copy("just text").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(read_image(), Ok(None));
        assert_eq!(read_text().as_deref(), Some("just text"));
    }

    #[test]
    fn routes_report_a_readable_name() {
        assert_eq!(Route::Wayland.label(), "Wayland clipboard");
        assert_eq!(Route::X11.label(), "X11 clipboard");
        assert_eq!(Route::Osc52.label(), "OSC 52");
    }
}
