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

/// Restores the terminal to its standard state by disabling raw mode,
/// leaving alternate screen, disabling mouse capture/bracketed paste, and showing the cursor.
pub fn restore_terminal() {
    let _ = disable_raw_mode();
    let mut stdout = stdout();
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

/// Sets up POSIX signal listeners for clean terminal teardown on hangup/terminate/quit,
/// and proper terminal suspension (SIGSTOP/SIGCONT) when sent to background via Ctrl+Z.
#[cfg(unix)]
pub fn setup_signal_handlers(resumed_flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use crossterm::event::{EnableBracketedPaste, EnableFocusChange};
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
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

    // Suspend signals (SIGTSTP, SIGTTIN, SIGTTOU):
    // Clean up terminal modes so mouse escape codes don't leak into the shell,
    // then pause via SIGSTOP. On resume (SIGCONT), restore TUI terminal state.
    let resumed_clone = resumed_flag;
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

            restore_terminal();

            // SAFETY: `libc::raise(SIGSTOP)` sends the stop signal strictly to the calling
            // process/thread itself. It has no memory-safety preconditions and cleanly yields
            // execution to the parent shell process group after terminal attributes have been restored.
            unsafe {
                libc::raise(libc::SIGSTOP);
            }

            // Resumed by SIGCONT from shell.
            let _ = enable_raw_mode();
            let mut out = stdout();
            let _ = out.write_all(MOUSE_REPORTING_OFF.as_bytes());
            let _ = execute!(out, EnterAlternateScreen, EnableBracketedPaste, EnableFocusChange);
            let _ = out.write_all(MOUSE_REPORTING_ON.as_bytes());
            let _ = out.flush();
            resumed_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
}
