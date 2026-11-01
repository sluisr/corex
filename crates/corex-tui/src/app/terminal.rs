//! Terminal initialization, cleanup and RAII guard utilities.

use std::io::{stdout, Write};
use crossterm::execute;
use crossterm::event::{DisableBracketedPaste, DisableFocusChange, DisableMouseCapture};
use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};

/// Mouse tracking modes the TUI actually consumes: `1000` (wheel and buttons), `1002` (drags,
/// which drive text selection) and `1006` (SGR coordinates). crossterm's `EnableMouseCapture`
/// additionally switches on `1003` (any-event motion), which turns every pixel of mouse travel
/// into an event worth a redraw.
pub const MOUSE_REPORTING_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";
pub const MOUSE_REPORTING_OFF: &str = "\x1b[?1006l\x1b[?1002l\x1b[?1000l";

/// Saves and restores the window title on xterm's title stack, so the shell gets its own title
/// back after the agent's states. Terminals without the stack ignore both.
const TITLE_PUSH: &str = "\x1b[22;0t";
const TITLE_POP: &str = "\x1b[23;0t";

/// Whether a title is on the stack: teardown runs more than once (panic hook, then the guard),
/// and a second pop would restore a title older than the shell's.
static TITLE_SAVED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn save_title(out: &mut impl Write) {
    if window_title_enabled() && !TITLE_SAVED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let _ = out.write_all(TITLE_PUSH.as_bytes());
    }
}

fn restore_title(out: &mut impl Write) {
    if TITLE_SAVED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        let _ = out.write_all(TITLE_POP.as_bytes());
    }
}

/// The window title follows the agent's state unless `COREX_TITLE` is `0`, `false` or `off`.
pub fn window_title_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        !std::env::var("COREX_TITLE").is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off"))
    })
}

/// The OSC 0 sequence that sets the window and tab title, without control characters that
/// would end it early.
pub fn title_sequence(title: &str) -> String {
    let clean: String = title.chars().filter(|c| !c.is_control()).collect();
    format!("\x1b]0;{clean}\x07")
}

/// Restores the terminal to its standard state by disabling raw mode,
/// leaving alternate screen, disabling mouse capture/bracketed paste, and showing the cursor.
pub fn restore_terminal() {
    let _ = disable_raw_mode();
    let mut stdout = stdout();
    restore_title(&mut stdout);
    // Explicitly switch every mouse mode off (also the ones enabled by hand in the TUI loop)
    // and reset attributes so a dead session never leaves escape junk in the shell.
    let _ = stdout.write_all(MOUSE_REPORTING_OFF.as_bytes());
    let _ = stdout.write_all(b"\x1b[?1003l\x1b[0m");
    let _ = execute!(
        stdout,
        DisableFocusChange,
        LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableMouseCapture,
        crossterm::cursor::Show
    );
    let _ = stdout.flush();
}

/// RAII guard ensuring the terminal is always cleanly restored on drop (including panics and early returns).
pub struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
        // Normal exit path: tasks run under `setsid`, so nothing else stops them.
        corex_tools::background::shutdown_all_tasks();
    }
}

/// Set after the process comes back from a suspend, so the loop repaints the whole screen.
pub static RESUMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Hands the terminal back to the shell and stops the process, as Ctrl+Z does in any program;
/// `fg` resumes it with the TUI restored.
#[cfg(unix)]
pub fn suspend() {
    use crossterm::event::{EnableBracketedPaste, EnableFocusChange};
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};

    restore_terminal();

    // SAFETY: `raise` only signals the calling process; it has no memory-safety preconditions.
    unsafe {
        libc::raise(libc::SIGSTOP);
    }

    let _ = enable_raw_mode();
    let mut out = stdout();
    let _ = out.write_all(MOUSE_REPORTING_OFF.as_bytes());
    let _ = execute!(out, EnterAlternateScreen, EnableBracketedPaste, EnableFocusChange);
    let _ = out.flush();
    RESUMED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Sets up POSIX signal listeners for clean terminal teardown on hangup/terminate/quit,
/// and proper terminal suspension (SIGSTOP/SIGCONT) when sent to background via Ctrl+Z.
#[cfg(unix)]
pub fn setup_signal_handlers() {
    use tokio::signal::unix::{signal, SignalKind};

    // Closing the terminal window (SIGHUP), `kill` (SIGTERM), or quit (SIGQUIT) skips Drop; restore
    // explicitly, stop the detached (setsid) child process groups and wipe any staged sudo
    // credential, which `process::exit` would otherwise leave behind.
    tokio::spawn(async {
        let (Ok(mut hup), Ok(mut term), Ok(mut quit)) = (
            signal(SignalKind::hangup()),
            signal(SignalKind::terminate()),
            signal(SignalKind::quit()),
        ) else {
            return;
        };
        let signo = tokio::select! {
            _ = hup.recv() => libc::SIGHUP,
            _ = term.recv() => libc::SIGTERM,
            _ = quit.recv() => libc::SIGQUIT,
        };
        restore_terminal();
        corex_tools::background::shutdown_all_tasks();
        std::process::exit(128 + signo);
    });

    // Suspend signals sent from outside (`kill -TSTP`): same teardown as Ctrl+Z.
    tokio::spawn(async move {
        let (Ok(mut tstp), Ok(mut ttin), Ok(mut ttou)) = (
            signal(SignalKind::from_raw(libc::SIGTSTP)),
            signal(SignalKind::from_raw(libc::SIGTTIN)),
            signal(SignalKind::from_raw(libc::SIGTTOU)),
        ) else {
            return;
        };

        loop {
            tokio::select! {
                _ = tstp.recv() => {}
                _ = ttin.recv() => {}
                _ = ttou.recv() => {}
            }
            suspend();
        }
    });
}
