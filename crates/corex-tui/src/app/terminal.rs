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
    }
}
