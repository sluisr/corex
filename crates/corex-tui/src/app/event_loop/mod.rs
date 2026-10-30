//! Main interactive event loop and terminal session lifecycle.
//!
//! Each iteration polls finished background work, draws, applies the model's stream events and
//! waits briefly for one terminal event. Input is dispatched to the handler of whatever is on
//! screen, in priority order: the sudo, API key and session dialogs, the fleet console, the
//! model picker, tool approval, the ask_user questions, global hotkeys and finally the prompt.

mod background;
mod dialogs;
mod fleet;
mod prompt;
mod stream;
mod tools;

pub use tools::format_call_preview;

use std::io::{stdout, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, EnableBracketedPaste, EnableFocusChange, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use corex_core::client::StreamEvent;
use corex_core::types::Message;

use super::mobile::is_android_or_termux;
use super::render::render_ui;
use super::state::App;
use super::summary::print_session_summary;
use super::terminal::{restore_terminal, TerminalGuard, MOUSE_REPORTING_OFF, MOUSE_REPORTING_ON};

type Tui = Terminal<CrosstermBackend<std::io::Stdout>>;
/// Stream and tool events, tagged with the generation (turn) that produced them.
type EventTx = mpsc::Sender<(u64, StreamEvent)>;

/// What happens after an input handler looked at an event.
enum Flow {
    /// The event was used; nothing else sees it.
    Handled,
    /// Not for this handler: the next one in line gets it.
    Pass,
    /// Leave the TUI.
    Quit,
}

pub async fn run_tui(mut app: App) -> Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        restore_terminal();
        // Release builds use `panic = "abort"`, so no Drop runs after this hook.
        corex_tools::background::shutdown_all_tasks();
        default_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = stdout();
    // Clear any stale mouse mode left by a previously killed session before starting.
    let _ = stdout.write_all(MOUSE_REPORTING_OFF.as_bytes());
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, EnableFocusChange)?;
    let _guard = TerminalGuard;

    let resumed_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Closing the terminal window (SIGHUP), `kill` (SIGTERM), quit (SIGQUIT), or suspend (SIGTSTP/SIGCONT)
    // are cleanly managed by signal handlers to ensure terminal modes never leak to shell.
    #[cfg(unix)]
    crate::app::terminal::setup_signal_handlers(resumed_flag.clone());

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Mouse reporting is applied by the loop below (reconcile step) so that startup
    // and F2 share a single code path.
    let mut mouse_capture_applied = false;

    let (event_tx, mut event_rx) = mpsc::channel::<(u64, StreamEvent)>(100);

    let mut last_tick = Instant::now();
    let mut needs_redraw = true;

    loop {
        #[cfg(unix)]
        if resumed_flag.swap(false, std::sync::atomic::Ordering::Relaxed) {
            let _ = terminal.clear();
            mouse_capture_applied = false;
            needs_redraw = true;
        }

        app.poll_local_health_check();

        app.tap_keyboard.resume_if_due(Instant::now(), &mut app.mouse_capture);

        // Reconcile terminal mouse reporting with the app state (COREX_MOUSE or F2).
        if app.mouse_capture != mouse_capture_applied {
            let out = terminal.backend_mut();
            let codes = if app.mouse_capture {
                MOUSE_REPORTING_ON
            } else {
                MOUSE_REPORTING_OFF
            };
            let _ = out.write_all(codes.as_bytes());
            let _ = out.flush();
            mouse_capture_applied = app.mouse_capture;
            needs_redraw = true;
        }

        if background::poll_background_work(&mut app, &event_tx) {
            needs_redraw = true;
        }

        let has_active_tasks = background::has_active_tasks(&app);
        let is_animating = app.is_slash_animating() || app.status_transition.is_animating();

        // The caret blinks off the idle clock; a phase change is the only reason to repaint a still screen.
        if crate::cursor::tick(app.last_interaction.elapsed()) {
            needs_redraw = true;
        }

        if app.fleet_dialog.poll_telemetry() {
            needs_redraw = true;
        }

        if fleet::poll_tasks(&mut app, &event_tx) {
            needs_redraw = true;
        }

        while let Ok((_, sess)) = app.fleet_dialog.cmd_done_rx.try_recv() {
            app.fleet_dialog.active_session = Some(sess);
            app.fleet_dialog.status_msg = None;
            needs_redraw = true;
        }

        if app.fleet_dialog.is_open && app.fleet_dialog.auto_connect_pending {
            app.fleet_dialog.auto_connect_pending = false;
            if let Some(id) = app.fleet_dialog.selected_node().map(|n| n.id.clone()) {
                app.fleet_dialog.status_msg = Some(format!("Connecting to {}...", id));
                terminal.draw(|f| render_ui(f, &mut app))?;
                fleet::ensure_session(&mut app);
                needs_redraw = true;
            }
        }

        let pty_dirty = app.fleet_dialog.is_open && app.fleet_dialog.pty_session.as_ref().map(|p| p.is_alive() && p.take_dirty()).unwrap_or(false);
        if needs_redraw || app.is_streaming || app.is_checking_balance || has_active_tasks || is_animating || pty_dirty {
            terminal.draw(|f| render_ui(f, &mut app))?;
            needs_redraw = false;
        }

        while let Ok((event_gen, event)) = event_rx.try_recv() {
            if event_gen != app.current_generation_id {
                // Drop stale/cancelled turn event to prevent concurrent interleaving
                continue;
            }
            needs_redraw = true;
            stream::handle_stream_event(&mut app, event, &event_tx).await;
        }

        // Short transitions (popup slide, status wave) last ~200 ms, so they keep 60 FPS to look
        // smooth; the long-running streaming only needs 30 FPS (the tool-preview beam is the
        // fastest thing on screen). The live PTY keeps the tight tick for immediate remote output.
        let is_transitioning = app.is_slash_animating() || app.status_transition.is_animating();
        let tick_rate = if pty_dirty || is_transitioning {
            Duration::from_millis(16)
        } else if app.is_streaming {
            Duration::from_millis(33)
        } else {
            Duration::from_millis(80)
        };
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            needs_redraw = true;
            app.last_interaction = Instant::now();
            let ev = event::read()?;
            if let Flow::Quit = handle_terminal_event(&mut app, ev, &event_tx, &mut terminal).await? {
                break;
            }
        } else if is_animating || app.is_streaming {
            needs_redraw = true;
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();

            // A drag held above or below the feed keeps scrolling, the way a terminal does, so a
            // selection can climb past the edge instead of stopping one line short of the text.
            if app.autoscroll_selection() {
                needs_redraw = true;
            }
        }
    }

    drop(_guard);

    print_session_summary(&app);
    Ok(())
}

async fn handle_terminal_event(app: &mut App, ev: Event, event_tx: &EventTx, terminal: &mut Tui) -> Result<Flow> {
    match ev {
        Event::Paste(pasted) => handle_paste(app, pasted),
        // A resize or regaining focus can leave ghost cells the diff renderer believes
        // are correct; wipe the backing buffer so the next draw repaints everything.
        Event::Resize(w, h) => {
            let _ = terminal.clear();
            app.handle_resize(w, h);
        }
        Event::FocusGained => {
            let _ = terminal.clear();
            app.last_focus_gained = Some(Instant::now());
            app.mouse_down_time = None;
            app.mouse_down_pos = None;
            app.clear_selection();
        }
        Event::FocusLost => {
            app.mouse_down_time = None;
            app.mouse_down_pos = None;
            app.clear_selection();
        }
        Event::Mouse(mouse) => handle_mouse(app, mouse, terminal).await,
        Event::Key(key) if key.kind == KeyEventKind::Press => return handle_key(app, key, event_tx, terminal).await,
        _ => {}
    }
    Ok(Flow::Handled)
}

fn handle_paste(app: &mut App, pasted: String) {
    let looks_sensitive = app.sudo_dialog.is_open
        || app.auth_dialog.is_open
        || app.user_dialog.is_open
        || app.fleet_dialog.is_open
        || app.input_buffer.trim_start().starts_with("/key")
        || app.input_buffer.trim_start().starts_with("/sudo")
        || pasted.contains("/key")
        || pasted.contains("/sudo")
        || pasted.contains("$sudo:")
        || pasted.contains("API_KEY")
        || ["sk-", "AIza", "gsk_", "ghp_", "gho_", "github_pat_", "xox", "-----BEGIN"]
            .iter()
            .any(|p| pasted.contains(p));
    if looks_sensitive {
        corex_core::forensic::ForensicLogger::log_event(
            "TUI_PASTE",
            &format!("Paste event ({} chars, redacted)", pasted.len()),
            "Content: [REDACTED]",
        );
    } else {
        corex_core::forensic::ForensicLogger::log_event(
            "TUI_PASTE",
            &format!("Paste event ({} chars)", pasted.len()),
            &format!("Content:\n{}", pasted),
        );
    }
    // A terminal that has nothing to paste as text (the clipboard holds an image)
    // sends an empty paste: that is the cue to look for an image instead.
    if !(pasted.trim().is_empty() && app.paste_clipboard_image()) {
        app.handle_paste(pasted);
    }
}

async fn handle_mouse(app: &mut App, mouse: MouseEvent, terminal: &mut Tui) {
    if app.fleet_dialog.is_open {
        let size = terminal.size().map(|s| (s.width, s.height)).unwrap_or((80, 24));
        if fleet::handle_mouse(app, mouse, size) {
            return;
        }
    }
    app.handle_mouse(mouse);
    if is_android_or_termux() && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        app.tap_keyboard.on_tap(Instant::now(), &mut app.mouse_capture);
    }
}

/// Offers `key` to each handler in priority order until one uses it.
async fn handle_key(app: &mut App, key: KeyEvent, event_tx: &EventTx, terminal: &mut Tui) -> Result<Flow> {
    macro_rules! offer {
        ($flow:expr) => {
            match $flow {
                Flow::Pass => {}
                flow => return Ok(flow),
            }
        };
    }
    offer!(early_key(app, key));
    offer!(dialogs::sudo_key(app, key, event_tx));
    offer!(dialogs::auth_key(app, key));
    offer!(dialogs::session_key(app, key));
    offer!(fleet::handle_key(app, key, event_tx)?);
    offer!(model_key(app, key));
    offer!(dialogs::confirmation_key(app, key, event_tx));
    offer!(dialogs::ask_user_key(app, key, event_tx));
    offer!(global_hotkey(app, key, terminal)?);
    Ok(prompt::prompt_key(app, key, event_tx).await)
}

fn model_key(app: &mut App, key: KeyEvent) -> Flow {
    if !app.model_dialog.is_open {
        return Flow::Pass;
    }
    let action = app.model_dialog.handle_key(key.code);
    app.apply_model_dialog_action(action);
    Flow::Handled
}

/// Keys that act whatever is on screen: quitting, the mouse toggle, and Esc / Ctrl+C while the
/// agent works or a balance query runs.
fn early_key(app: &mut App, key: KeyEvent) -> Flow {
    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Force quit.
    if is_ctrl && matches!(key.code, KeyCode::Char('z') | KeyCode::Char('Z') | KeyCode::Char('\x1a')) {
        return Flow::Quit;
    }

    // Mouse reporting toggle. Reclaims native click-drag selection/copy on demand without
    // giving up wheel scrolling (which stays available via PageUp/PageDown when OFF).
    if key.code == KeyCode::F(2) {
        app.toggle_mouse_capture();
        return Flow::Handled;
    }

    // Esc drops an active selection without consuming the key, so it still
    // cancels streaming or closes a dialog further down.
    if key.code == KeyCode::Esc && app.selection.is_some() {
        app.clear_selection();
    }

    // Cancel the generation on Esc or Ctrl+C.
    if app.is_streaming && (key.code == KeyCode::Esc || (key.code == KeyCode::Char('c') && is_ctrl)) {
        // Esc first takes back the newest pending message, one per press; the
        // generation is only cancelled once nothing is left to take back.
        if key.code == KeyCode::Esc && app.has_pending_messages() {
            if let Some(pending) = app.undo_last_pending() {
                app.restore_to_input(vec![pending]);
            }
            return Flow::Handled;
        }

        // Cancelling must not eat what the user typed: whatever never reached
        // the model goes back to the composer.
        let undelivered = app.take_all_pending();
        app.cancel_generation("Generation cancelled by user (Esc).");
        app.restore_to_input(undelivered);
        return Flow::Handled;
    }

    if app.is_checking_balance && key.code == KeyCode::Esc {
        app.is_checking_balance = false;
        app.clear_status();
        if let Some(idx) = app.pending_balance_msg_index.take() {
            if app.session.replace_message(idx, Message::system("Balance check cancelled.")) {
                app.invalidate_message_cache();
            }
        }
        return Flow::Handled;
    }
    Flow::Pass
}

/// Ctrl+O (expand logs), Ctrl+C twice (quit), Ctrl+T (thinking box), Ctrl+L (repaint).
fn global_hotkey(app: &mut App, key: KeyEvent, terminal: &mut Tui) -> Result<Flow> {
    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let is_ctrl_o = (is_ctrl && matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O')))
        || key.code == KeyCode::Char('\x0f');

    if is_ctrl_o {
        app.logs_expanded = !app.logs_expanded;
        if let Some(ref mut pending) = app.pending_confirmation {
            pending.diff_expanded = app.logs_expanded;
        }
        app.invalidate_message_cache();
        app.auto_scroll = true;
        return Ok(Flow::Handled);
    }

    if !is_ctrl {
        return Ok(Flow::Pass);
    }
    match key.code {
        KeyCode::Char('c') | KeyCode::Char('\x03') => {
            let is_recent = app.last_ctrl_c_press.is_some_and(|i| i.elapsed() <= Duration::from_secs(2));
            if is_recent {
                return Ok(Flow::Quit);
            }
            app.last_ctrl_c_press = Some(Instant::now());
        }
        KeyCode::Char('t') | KeyCode::Char('T') | KeyCode::Char('\x14') => {
            app.thinking_state.is_expanded = !app.thinking_state.is_expanded;
            app.invalidate_message_cache();
        }
        KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Char('\x0c') => {
            terminal.clear()?;
        }
        _ => return Ok(Flow::Pass),
    }
    Ok(Flow::Handled)
}
