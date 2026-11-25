//! Main interactive event loop and terminal session lifecycle.
//!
//! Each iteration polls finished background work, draws, applies the model's stream events and
//! waits briefly for one terminal event. Input is dispatched to the handler of whatever is on
//! screen, in priority order: the sudo, API key and session dialogs, the fleet console, the
//! model picker, tool approval, the ask_user questions, global hotkeys and finally the prompt.

mod background;
mod dialogs;
pub(crate) mod fleet;
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
use crossterm::terminal::{enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen};
use crossterm::{execute, queue};
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

/// Input events applied before a frame is drawn; caps the stall a flood of them could cause.
const MAX_EVENTS_PER_FRAME: usize = 64;

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
    let _ = stdout.write_all(crate::app::terminal::WHEEL_AS_ARROWS_OFF.as_bytes());
    let title_enabled = crate::app::terminal::window_title_enabled();
    crate::app::terminal::save_title(&mut stdout);
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, EnableFocusChange)?;
    let _guard = TerminalGuard;

    // Closing the terminal window (SIGHUP), `kill` (SIGTERM), quit (SIGQUIT), or suspend (SIGTSTP/SIGCONT)
    // are cleanly managed by signal handlers to ensure terminal modes never leak to shell.
    #[cfg(unix)]
    crate::app::terminal::setup_signal_handlers();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Mouse reporting is applied by the loop below (reconcile step) so that startup
    // and F2 share a single code path.
    let mut mouse_capture_applied = false;
    let mut title_applied: Option<String> = None;

    let (event_tx, mut event_rx) = mpsc::channel::<(u64, StreamEvent)>(100);

    let mut last_tick = Instant::now();
    let mut last_draw = Instant::now();
    let mut needs_redraw = true;
    let mut stream_delta_pending = false;
    let mut prev_streaming = false;
    let mut prev_waiting_kind: Option<&'static str> = None;
    let mut streaming_started_at: Option<Instant> = None;

    loop {
        #[cfg(unix)]
        if crate::app::terminal::RESUMED.swap(false, std::sync::atomic::Ordering::Relaxed) {
            let _ = terminal.clear();
            mouse_capture_applied = false;
            crate::app::terminal::save_title(terminal.backend_mut());
            title_applied = None;
            needs_redraw = true;
        }

        app.poll_local_health_check();
        let _ = app.poll_mcp_tools();
        if app.poll_file_index() | app.poll_user_shell() {
            needs_redraw = true;
        }

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

        if title_enabled {
            let title = app.window_title();
            if title_applied.as_deref() != Some(title.as_str()) {
                let out = terminal.backend_mut();
                let _ = out.write_all(crate::app::terminal::title_sequence(&title).as_bytes());
                let _ = out.flush();
                title_applied = Some(title);
            }
        }

        // Notify the user (OSC 9 + BEL + desktop notification) when Corex pauses for sudo,
        // tool approval, or ask_user, or finishes a turn that ran for >= 10s (if enabled in /settings).
        let notify_on = app.llm_client.with_config(|c| c.notifications);
        let waiting_kind: Option<&'static str> = if app.sudo_dialog.is_open {
            Some("sudo")
        } else if app.user_dialog.is_open {
            Some("ask_user")
        } else if app.pending_confirmation.is_some() {
            Some("approval")
        } else {
            None
        };
        if waiting_kind.is_some() && waiting_kind != prev_waiting_kind && notify_on {
            let msg = match waiting_kind {
                Some("sudo") => "Sudo password required".to_string(),
                Some("ask_user") => "Waiting for your answer".to_string(),
                Some("approval") => match app
                    .pending_confirmation
                    .as_ref()
                    .and_then(|p| p.calls.get(p.current_idx.saturating_sub(1)).or_else(|| p.calls.first()))
                {
                    Some(call) => format!("Approval needed: {}", call.function.name),
                    None => "Approval needed to run a tool".to_string(),
                },
                _ => "Action required".to_string(),
            };
            crate::app::terminal::notify_user(terminal.backend_mut(), "Corex", &msg);
        }
        let now_waiting_user = waiting_kind.is_some();
        prev_waiting_kind = waiting_kind;

        if app.is_streaming && !prev_streaming {
            streaming_started_at = Some(Instant::now());
        } else if !app.is_streaming && prev_streaming {
            if !now_waiting_user && app.in_flight_tool_calls == 0 {
                let outcome = turn_finish_notification(&app.session.messages);
                if let Some(started) = streaming_started_at.take() {
                    if notify_on && started.elapsed() >= Duration::from_secs(10) {
                        if let Some(msg) = outcome {
                            crate::app::terminal::notify_user(terminal.backend_mut(), "Corex", msg);
                        }
                    }
                }
                if outcome == Some("Turn completed") {
                    corex_tools::hooks::spawn_stop_hooks(
                        corex_tools::hooks::load_global_hooks(),
                        app.workspace_dir.clone(),
                    );
                }
            }
        }
        prev_streaming = app.is_streaming;

        if background::poll_background_work(&mut app, &event_tx) {
            needs_redraw = true;
        }

        let has_active_tasks = background::has_active_tasks(&app);
        if app.step_scroll() {
            needs_redraw = true;
        }
        let is_animating = app.is_slash_animating() || app.status_transition.is_animating() || app.pending_scroll != 0;

        if app.copied_block.is_some_and(|(_, at)| at.elapsed() >= crate::app::copy::COPIED_FLASH) {
            app.copied_block = None;
            needs_redraw = true;
        }

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
                crate::app::mobile::clean_trailing_margins(app.last_chat_rect, &app.visible_line_widths, terminal.backend_mut());
                fleet::ensure_session(&mut app);
                needs_redraw = true;
            }
        }

        if fleet::watch_pty(&mut app) {
            needs_redraw = true;
        }

        // Apply any stream/tool events that already arrived BEFORE drawing so token deltas
        // render on the very frame they arrive instead of waiting an extra tick cycle.
        while let Ok((event_gen, event)) = event_rx.try_recv() {
            if event_gen != app.current_generation_id {
                continue;
            }
            let is_pure_delta = matches!(
                event,
                corex_core::client::StreamEvent::ContentDelta(_)
                    | corex_core::client::StreamEvent::ReasoningDelta(_)
                    | corex_core::client::StreamEvent::ToolCallDelta { .. }
            );
            if !is_pure_delta || last_draw.elapsed() >= Duration::from_millis(16) {
                needs_redraw = true;
            } else {
                stream_delta_pending = true;
            }
            stream::handle_stream_event(&mut app, event, &event_tx).await;
        }

        let pty_dirty = app.fleet_dialog.is_open && app.fleet_dialog.pty_session.as_ref().map(|p| p.is_alive() && p.take_dirty()).unwrap_or(false);
        let is_transitioning = app.is_slash_animating() || app.status_transition.is_animating() || app.pending_scroll != 0;
        let tick_rate = if pty_dirty || is_transitioning {
            Duration::from_millis(16)
        } else if app.is_streaming {
            Duration::from_millis(33)
        } else {
            Duration::from_millis(250)
        };
        // Coalesce high-frequency SSE token deltas into 60 FPS (16ms) frames while drawing the
        // first delta after a quiet period immediately (0ms delay).
        let stream_tick = stream_delta_pending && last_draw.elapsed() >= Duration::from_millis(16);
        // A running task changes nothing on screen by itself; once a second covers what it may show.
        let task_tick = (has_active_tasks && last_draw.elapsed() >= Duration::from_secs(1))
            || (app.tasks_dialog.is_open && last_draw.elapsed() >= Duration::from_millis(250));
        let anim_tick = (app.is_streaming || app.is_compacting() || app.is_checking_balance || is_animating)
            && last_draw.elapsed() >= tick_rate;
        if needs_redraw || stream_tick || anim_tick || task_tick || pty_dirty {
            // Synchronized output (DEC 2026) so the terminal never shows a half-drawn or cleared frame.
            queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
            if std::mem::take(&mut app.pending_full_repaint) {
                terminal.clear()?;
            }
            terminal.draw(|f| render_ui(f, &mut app))?;
            crate::app::mobile::clean_trailing_margins(app.last_chat_rect, &app.visible_line_widths, terminal.backend_mut());
            execute!(terminal.backend_mut(), EndSynchronizedUpdate)?;
            needs_redraw = false;
            stream_delta_pending = false;
            last_draw = Instant::now();
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        let awaiting_async_events =
            app.is_streaming || app.is_compacting() || app.in_flight_tool_calls > 0 || app.is_checking_balance || has_active_tasks;

        // While streaming or waiting on async work, yield in `tokio::select!` so incoming LLM
        // deltas and tool completions wake the loop in <1ms. When truly idle, block directly in
        // `crossterm::event::poll(timeout)` (kernel `poll(2)`) so idle CPU stays ~0.1% while
        // keypresses still wake the loop instantaneously.
        if !event::poll(Duration::ZERO)? && !timeout.is_zero() {
            if awaiting_async_events {
                let poll_slice = Duration::from_millis(8).min(timeout);
                tokio::select! {
                    biased;
                    maybe_ev = event_rx.recv() => {
                        if let Some((event_gen, event)) = maybe_ev {
                            if event_gen == app.current_generation_id {
                                let is_pure_delta = matches!(
                                    event,
                                    corex_core::client::StreamEvent::ContentDelta(_)
                                        | corex_core::client::StreamEvent::ReasoningDelta(_)
                                        | corex_core::client::StreamEvent::ToolCallDelta { .. }
                                );
                                if !is_pure_delta || last_draw.elapsed() >= Duration::from_millis(16) {
                                    needs_redraw = true;
                                } else {
                                    stream_delta_pending = true;
                                }
                                stream::handle_stream_event(&mut app, event, &event_tx).await;
                            }
                        }
                        while let Ok((event_gen, event)) = event_rx.try_recv() {
                            if event_gen == app.current_generation_id {
                                let is_pure_delta = matches!(
                                    event,
                                    corex_core::client::StreamEvent::ContentDelta(_)
                                        | corex_core::client::StreamEvent::ReasoningDelta(_)
                                        | corex_core::client::StreamEvent::ToolCallDelta { .. }
                                );
                                if !is_pure_delta || last_draw.elapsed() >= Duration::from_millis(16) {
                                    needs_redraw = true;
                                } else {
                                    stream_delta_pending = true;
                                }
                                stream::handle_stream_event(&mut app, event, &event_tx).await;
                            }
                        }
                    }
                    _ = tokio::time::sleep(poll_slice) => {}
                }
            } else {
                let _ = event::poll(timeout)?;
            }
        }

        if event::poll(Duration::ZERO)? {
            needs_redraw = true;
            app.last_interaction = Instant::now();
            // Key repeat (holding Backspace) queues events faster than a frame draws; apply every
            // one already waiting and draw once, instead of trailing behind one frame per key.
            let mut quit = false;
            for _ in 0..MAX_EVENTS_PER_FRAME {
                let ev = event::read()?;
                if let Flow::Quit = handle_terminal_event(&mut app, ev, &event_tx, &mut terminal).await? {
                    quit = true;
                    break;
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
            if quit {
                break;
            }
        } else if last_tick.elapsed() >= tick_rate && (is_animating || app.is_streaming) {
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
    let edits_prompt = match &ev {
        Event::Paste(_) => true,
        Event::Key(key) => key.kind == KeyEventKind::Press && !is_ctrl_z(key),
        _ => false,
    };
    if !edits_prompt {
        return dispatch_terminal_event(app, ev, event_tx, terminal).await;
    }
    let before = (app.input_buffer.clone(), app.cursor_idx);
    let flow = dispatch_terminal_event(app, ev, event_tx, terminal).await;
    if app.input_buffer != before.0 {
        app.record_prompt_edit(before);
    }
    flow
}

fn is_ctrl_z(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('z') | KeyCode::Char('Z') | KeyCode::Char('\x1a'))
}

/// Classifies how a streaming turn ended so errors and user cancellations never fire a
/// misleading "Turn completed" notification or `OnStop` hook.
fn turn_finish_notification(messages: &[corex_core::types::Message]) -> Option<&'static str> {
    let last = messages.last()?;
    let text = last.text_content().unwrap_or("").trim();
    match last.role.as_str() {
        "assistant" if !text.ends_with("*(interrupted)*") => Some("Turn completed"),
        "system" if text.starts_with("Error:") => Some("Turn failed"),
        _ => None,
    }
}

pub(super) fn is_ctrl_c(key: &KeyEvent) -> bool {
    (key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')))
        || key.code == KeyCode::Char('\x03')
}

async fn dispatch_terminal_event(app: &mut App, ev: Event, event_tx: &EventTx, terminal: &mut Tui) -> Result<Flow> {
    match ev {
        Event::Paste(pasted) => handle_paste(app, pasted),
        // A resize or regaining focus can leave ghost cells the diff renderer believes
        // are correct; wipe the backing buffer so the next draw repaints everything.
        Event::Resize(w, h) => {
            app.pending_full_repaint = true;
            app.handle_resize(w, h);
        }
        Event::FocusGained => {
            app.pending_full_repaint = true;
            // A terminal reset drops the mouse modes, and the wheel then arrives as arrow keys.
            if app.mouse_capture {
                let _ = terminal.backend_mut().write_all(MOUSE_REPORTING_ON.as_bytes());
            }
            app.last_focus_gained = Some(Instant::now());
            app.mouse_down_time = None;
            app.mouse_down_pos = None;
            app.cancel_drag();
        }
        Event::FocusLost => {
            app.mouse_down_time = None;
            app.mouse_down_pos = None;
            app.cancel_drag();
        }
        Event::Mouse(mouse) => handle_mouse(app, mouse, terminal).await,
        Event::Key(key) if key.kind == KeyEventKind::Press => return handle_key(app, key, event_tx, terminal).await,
        _ => {}
    }
    Ok(Flow::Handled)
}

fn handle_paste(app: &mut App, pasted: String) {
    corex_core::forensic::ForensicLogger::log_event(
        "TUI_PASTE",
        &format!("Paste event ({} chars)", pasted.len()),
        "Content: [not logged]",
    );
    // A terminal that has nothing to paste as text (the clipboard holds an image)
    // sends an empty paste: that is the cue to look for an image instead.
    if !(pasted.trim().is_empty() && app.paste_clipboard_image()) && !app.paste_image_files(&pasted) {
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
    handle_key_inner(app, key, event_tx, Some(terminal)).await
}

async fn handle_key_inner(
    app: &mut App,
    key: KeyEvent,
    event_tx: &EventTx,
    terminal: Option<&mut Tui>,
) -> Result<Flow> {
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
    offer!(dialogs::tasks_key(app, key));
    offer!(dialogs::settings_key(app, key));
    offer!(fleet::handle_key(app, key, event_tx)?);
    offer!(model_key(app, key));
    offer!(dialogs::confirmation_key(app, key, event_tx));
    offer!(dialogs::ask_user_key(app, key, event_tx));
    offer!(dialogs::rewind_key(app, key));
    offer!(global_hotkey(app, key, terminal)?);
    Ok(prompt::prompt_key(app, key, event_tx).await)
}

fn model_key(app: &mut App, key: KeyEvent) -> Flow {
    if !app.model_dialog.is_open {
        return Flow::Pass;
    }
    if is_ctrl_c(&key) {
        app.model_dialog.close();
        app.last_ctrl_c_press = Some(Instant::now());
        return Flow::Handled;
    }
    let action = app.model_dialog.handle_key(key.code);
    app.apply_model_dialog_action(action);
    Flow::Handled
}

/// Keys that act whatever is on screen: quitting, the mouse toggle, and Esc / Ctrl+C while the
/// agent works or a balance query runs.
fn early_key(app: &mut App, key: KeyEvent) -> Flow {
    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // With text in the prompt Ctrl+Z undoes, a letter or an edit at a time. On an empty prompt it
    // suspends, and like Ctrl+C only on a second press within two seconds.
    if is_ctrl_z(&key) {
        app.last_ctrl_c_press = None;
        if !app.input_buffer.is_empty() {
            app.last_ctrl_z_press = None;
            app.undo_prompt_edit();
            return Flow::Handled;
        }
        let is_recent = app.last_ctrl_z_press.is_some_and(|i| i.elapsed() <= Duration::from_secs(2));
        if is_recent {
            app.last_ctrl_z_press = None;
            #[cfg(unix)]
            crate::app::terminal::suspend();
        } else {
            app.last_ctrl_z_press = Some(Instant::now());
        }
        return Flow::Handled;
    }

    // With text highlighted, Ctrl+C copies it, as in a terminal; the next one cancels or exits.
    if is_ctrl
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Char('\x03'))
        && app.copy_selection()
    {
        return Flow::Handled;
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

    // When a modal panel (/settings, /model, /resume, /tasks, /key, /rewind, approval, sudo,
    // ask_user) is open, Esc and Ctrl+C belong to that panel so pressing Esc closes the panel
    // instead of cancelling the agent's in-flight generation behind it.
    let modal_open = app.sudo_dialog.is_open
        || app.auth_dialog.is_open
        || app.session_dialog.is_open
        || app.tasks_dialog.is_open
        || app.settings_dialog.is_open
        || app.model_dialog.is_open
        || app.pending_confirmation.is_some()
        || app.user_dialog.is_open
        || app.rewind_dialog.is_open;

    // Cancel the generation on Esc or Ctrl+C. While something is typed, Esc belongs to the prompt
    // (a double Esc clears it), so it only cancels from an empty prompt.
    let esc_for_prompt = key.code == KeyCode::Esc && !app.input_buffer.trim().is_empty();
    if app.is_streaming && !modal_open && !esc_for_prompt && (key.code == KeyCode::Esc || (key.code == KeyCode::Char('c') && is_ctrl)) {
        // Cancelling must not eat what the user typed: whatever never reached
        // the model goes back to the composer.
        let undelivered = app.take_all_pending();
        app.cancel_generation("Generation cancelled by user (Esc).");
        app.restore_to_input(undelivered);
        return Flow::Handled;
    }

    if app.is_compacting() && !modal_open && !esc_for_prompt && (key.code == KeyCode::Esc || (key.code == KeyCode::Char('c') && is_ctrl)) {
        let undelivered = app.take_all_pending();
        app.cancel_compaction("Context compaction cancelled.");
        app.restore_to_input(undelivered);
        return Flow::Handled;
    }

    if app.is_checking_balance && !modal_open && key.code == KeyCode::Esc {
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
fn global_hotkey(app: &mut App, key: KeyEvent, terminal: Option<&mut Tui>) -> Result<Flow> {
    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let is_ctrl_o = (is_ctrl && matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O')))
        || key.code == KeyCode::Char('\x0f');

    if is_ctrl_o {
        app.toggle_logs();
        return Ok(Flow::Handled);
    }

    if is_ctrl_c(&key) {
        let is_recent = app.last_ctrl_c_press.is_some_and(|i| i.elapsed() <= Duration::from_secs(2));
        if is_recent {
            return Ok(Flow::Quit);
        }
        app.last_ctrl_c_press = Some(Instant::now());
        app.last_ctrl_z_press = None;
        return Ok(Flow::Handled);
    }

    if !is_ctrl {
        return Ok(Flow::Pass);
    }
    match key.code {
        KeyCode::Char('t') | KeyCode::Char('T') | KeyCode::Char('\x14') => {
            app.thinking_state.is_expanded = !app.thinking_state.is_expanded;
            app.invalidate_message_cache();
        }
        KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Char('\x0c') => {
            if let Some(term) = terminal {
                term.clear()?;
            }
        }
        _ => return Ok(Flow::Pass),
    }
    Ok(Flow::Handled)
}

#[cfg(test)]
mod ctrl_c_tests {
    use super::*;
    use corex_core::client::LlmClient;
    use corex_core::config::Config;
    use std::path::PathBuf;

    #[tokio::test]
    async fn first_ctrl_c_closes_model_or_session_dialog_and_second_quits() {
        let mut app = App::new(LlmClient::new(Config::default()), PathBuf::from("/tmp"), false);
        let (tx, _rx) = mpsc::channel(4);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        app.model_dialog.open(&Config::default());
        assert!(app.model_dialog.is_open);
        let flow1 = handle_key_inner(&mut app, ctrl_c, &tx, None).await.unwrap();
        assert!(matches!(flow1, Flow::Handled));
        assert!(!app.model_dialog.is_open, "first Ctrl+C closes /model");

        let flow2 = handle_key_inner(&mut app, ctrl_c, &tx, None).await.unwrap();
        assert!(matches!(flow2, Flow::Quit), "second Ctrl+C within 2s quits");

        app.last_ctrl_c_press = None;
        app.session_dialog.is_open = true;
        let flow3 = handle_key_inner(&mut app, ctrl_c, &tx, None).await.unwrap();
        assert!(matches!(flow3, Flow::Handled));
        assert!(!app.session_dialog.is_open, "first Ctrl+C closes /resume");
        let flow4 = handle_key_inner(&mut app, ctrl_c, &tx, None).await.unwrap();
        assert!(matches!(flow4, Flow::Quit), "second Ctrl+C after /resume quits");
    }

    #[tokio::test]
    async fn esc_closes_open_modal_instead_of_cancelling_streaming() {
        let mut app = App::new(LlmClient::new(Config::default()), PathBuf::from("/tmp"), false);
        app.auth_dialog.close();
        let (tx, _rx) = mpsc::channel(4);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

        app.is_streaming = true;
        app.settings_dialog.open(&Config::default());
        let flow1 = handle_key_inner(&mut app, esc, &tx, None).await.unwrap();
        assert!(matches!(flow1, Flow::Handled));
        assert!(!app.settings_dialog.is_open, "Esc must close /settings first");
        assert!(app.is_streaming, "Esc in /settings must NOT cancel the active generation");

        // Second Esc with no modal open cancels generation as expected.
        let flow2 = handle_key_inner(&mut app, esc, &tx, None).await.unwrap();
        assert!(matches!(flow2, Flow::Handled));
        assert!(!app.is_streaming, "Esc with no modal open cancels generation");
    }
}

#[cfg(test)]
mod ctrl_o_tests {
    use super::*;
    use corex_core::client::LlmClient;
    use corex_core::config::Config;
    use corex_core::types::{Message, ToolCall};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn rows(app: &mut App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| crate::app::render::render_ui(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()).collect()
    }

    #[tokio::test]
    async fn ctrl_o_keeps_the_reading_position() {
        let mut app = App::new(LlmClient::new(Config::default()), PathBuf::from("/tmp"), false);
        app.auth_dialog.close();
        for i in 0..12 {
            app.session.add_message(Message::user(format!("question {i}")));
            let id = format!("c{i}");
            let call = ToolCall::new(&id, "run_shell_command", &format!(r#"{{"command":"seq {i}"}}"#));
            app.session.add_message(Message::assistant_with_tools(None, None, vec![call]));
            let out: Vec<String> = (0..40).map(|n| format!("out{i}-{n}")).collect();
            app.session.add_message(Message::tool_response(&id, out.join("\n")));
        }
        rows(&mut app);
        let target = app.cached_message_line_offsets[3 * 5];
        app.auto_scroll = false;
        app.scroll_offset = app.chat_header_lines + target;
        let before = rows(&mut app);
        assert!(before[0].contains("question 5"), "{}", before.join("\n"));
        assert!(before.iter().any(|r| r.contains("lines above) · ctrl+o")), "{}", before.join("\n"));
        assert!(!before.last().unwrap().contains("logs"), "{}", before.join("\n"));

        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        for expanded in [true, false] {
            global_hotkey(&mut app, ctrl_o, None).unwrap();
            let after = rows(&mut app);
            assert_eq!(app.logs_expanded, expanded);
            assert!(!app.auto_scroll);
            assert_eq!(after[0], before[0], "{}", after.join("\n"));
            assert_eq!(after.last().unwrap().contains("logs"), expanded, "{}", after.join("\n"));
        }

        app.auto_scroll = true;
        global_hotkey(&mut app, ctrl_o, None).unwrap();
        let bottom = rows(&mut app);
        assert!(app.auto_scroll && bottom.join("\n").contains("out11-39"), "{}", bottom.join("\n"));

        assert!(app.handle_slash_command("/logs").await);
        assert!(!app.logs_expanded);
    }
}

#[cfg(test)]
mod log_click_tests {
    use super::*;
    use corex_core::client::LlmClient;
    use corex_core::config::Config;
    use corex_core::types::{Message, ToolCall};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn draw(app: &mut App) {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| crate::app::render::render_ui(f, app)).unwrap();
    }

    fn line_of(app: &App, needle: &str, nth: usize) -> usize {
        let head = app.selection_lines.head.len();
        head + app.selection_lines.cached.iter().enumerate().filter(|(_, l)| l.contains(needle)).nth(nth).unwrap().0
    }

    fn shown(app: &App, needle: &str) -> bool {
        app.selection_lines.cached.iter().any(|l| l.trim_end().ends_with(needle))
    }

    #[tokio::test]
    async fn clicking_a_collapsed_log_expands_only_that_one() {
        let mut app = App::new(LlmClient::new(Config::default()), PathBuf::from("/tmp"), false);
        app.auth_dialog.close();
        for i in 0..3 {
            app.session.add_message(Message::user(format!("question {i}")));
            let id = format!("c{i}");
            let call = ToolCall::new(&id, "run_shell_command", &format!(r#"{{"command":"seq {i}"}}"#));
            app.session.add_message(Message::assistant_with_tools(None, None, vec![call]));
            let out: Vec<String> = (0..40).map(|n| format!("out{i}-{n}")).collect();
            app.session.add_message(Message::tool_response(&id, out.join("\n")));
        }
        draw(&mut app);
        assert!(!shown(&app, "out1-0") && !shown(&app, "out0-0"));

        assert!(app.toggle_log_at(line_of(&app, "lines above) · ctrl+o", 1)));
        draw(&mut app);
        assert!(shown(&app, "out1-0"), "the clicked log opens");
        assert!(!shown(&app, "out0-0") && !shown(&app, "out2-0"), "the others stay collapsed");
        assert!(!app.logs_expanded);

        assert!(app.toggle_log_at(line_of(&app, "Output (40 lines)", 1)));
        draw(&mut app);
        assert!(!shown(&app, "out1-0"), "its header folds it back");

        assert!(!app.toggle_log_at(line_of(&app, "question 0", 0)), "plain lines are not toggles");
    }
}



#[cfg(test)]
mod narrow_screens {
    use super::*;
    use corex_core::types::{Message, ToolCall};
    use unicode_width::UnicodeWidthStr;

    fn fill(app: &mut App) {
        app.session.add_message(Message::user("check this link https://example.com/org/project/blob/main/crates/app/src/render/module.rs#L1260 please".to_string()));
        let reply = "Reply with `an_inline_code_identifier_without_spaces_that_cannot_fit` and a URL https://example.com/a/very/long/path/that/never/ends/anywhere.html\n\n```rust\nfn main() { let a_variable_with_a_long_name = another_function_with_a_long_name(first_argument, second_argument); }\n```\n\n| Column A | Longer column B | C |\n|---|---|---|\n| value one | a value with quite a lot of text here | x |\n\n- a list item with many words to see whether it wraps on small screens\n  - nested AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n\n> a quote with long text that should wrap inside the quote block\n\n# A very long heading that should wrap on small screens too";
        let calls = vec![
            ToolCall::new("a", "run_shell_command", r#"{"command":"cargo test --workspace --all-features -- --nocapture --test-threads=1 some_really_long_filter_name"}"#),
            ToolCall::new("b", "web_fetch", r#"{"url":"https://example.com/org/project/blob/main/crates/app/src/event_loop/module.rs"}"#),
            ToolCall::new("c", "edit", r#"{"file_path":"/tmp/project/crates/app/src/event_loop/a_very_long_module_name_here.rs","old_string":"let a = 1;","new_string":"let a_much_longer_variable_name_here = compute_something_really_long(1, 2, 3);"}"#),
        ];
        app.session.add_message(Message::assistant_with_tools(Some(reply.to_string()), None, calls));
        app.session.add_message(Message::tool_response("a", (0..8).map(|i| format!("test module::submodule::a_really_long_test_name_number_{i}_xxxxxxxxxxxxxxxxxxxx ... ok")).collect::<Vec<_>>().join("\n")));
        app.session.add_message(Message::tool_response("b", "--- a/x.rs\n+++ b/x.rs\n@@ -1,3 +1,3 @@\n-let aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa = 1;\n+let bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb = 2;\n context".to_string()));
        app.session.add_message(Message::tool_response("c", "Successfully edited /tmp/project/crates/app/src/event_loop/a_very_long_module_name_here.rs".to_string()));
        app.session.add_message(Message::system("Activated Google (Gemini) · Gemini 3.5 Flash-Lite\n- Temperature: 1.0\n- Endpoint: https://generativelanguage.googleapis.com/v1beta/openai/"));
        app.input_buffer = "a fairly long message typed in the prompt to see whether it wraps correctly".to_string();
        app.cursor_idx = app.input_buffer.len();
        app.git_branch = "feature/a-really-long-branch-name".to_string();
    }

    fn screen(width: u16, height: u16) -> (App, Vec<String>) {
        let mut app = App::new(corex_core::client::LlmClient::new(corex_core::config::Config::default()), std::path::PathBuf::from("/tmp/project/with/a/deep/path"), false);
        app.auth_dialog.close();
        fill(&mut app);
        app.auto_scroll = false;
        app.scroll_offset = 0;
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        t.draw(|f| crate::app::render::render_ui(f, &mut app)).unwrap();
        let b = t.backend().buffer().clone();
        let rows = (0..height).map(|y| (0..width).map(|x| b[(x, y)].symbol()).collect::<String>()).collect();
        (app, rows)
    }

    #[tokio::test]
    async fn nothing_runs_past_the_right_edge_on_small_screens() {
        for &(w, h) in &[(30u16, 40u16), (40, 40), (60, 30)] {
            let (app, rows) = screen(w, h);
            let chat_w = app.last_chat_rect.unwrap().width as usize;
            for line in &app.cached_message_lines {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(text.width() <= chat_w, "{w} cols: {text:?}");
            }
            let banner = rows[..8].join("\n");
            assert!(banner.contains("Corex") && banner.contains("/help") && banner.contains("/key"), "{banner}");
        }
    }

    #[tokio::test]
    async fn a_split_url_opens_whole() {
        let (app, _) = screen(40, 40);
        let url = "https://example.com/org/project/blob/main/crates/app/src/render/module.rs#L1260";
        let lines = &app.selection_lines;
        let start = (0..lines.len()).find(|&i| lines.get(i).is_some_and(|l| l.contains("https://example.com/org"))).unwrap();
        let middle = lines.get(start + 1).unwrap();
        let col = middle.len() - middle.trim_start().len() + 2;
        assert_eq!(app.url_at(start + 1, col).as_deref(), Some(url));
        assert_eq!(app.url_at(start, 14).as_deref(), Some(url));
    }

    #[test]
    fn turn_finish_notification_distinguishes_success_error_and_cancellation() {
        use corex_core::types::Message;
        assert_eq!(
            super::turn_finish_notification(&[Message::assistant("Done!".to_string(), None)]),
            Some("Turn completed")
        );
        assert_eq!(
            super::turn_finish_notification(&[Message::system("Error: 429 Too Many Requests".to_string())]),
            Some("Turn failed")
        );
        assert_eq!(
            super::turn_finish_notification(&[Message::system("Generation cancelled by user.".to_string())]),
            None
        );
        assert_eq!(
            super::turn_finish_notification(&[Message::assistant("partial *(interrupted)*".to_string(), None)]),
            None
        );
        assert_eq!(super::turn_finish_notification(&[]), None);
    }

}
