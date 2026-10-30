//! The fleet console: approving the model's remote commands, the server selector, the SSH
//! terminal pane and the mouse on the split view.
//!
//! Nothing here waits on the network. Connecting, running an approved command and completing a
//! command line run as background tasks and report through `FleetDialogState::task_rx`, which
//! `poll_tasks` drains once per loop iteration, so the keyboard answers while SSH works.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use corex_core::types::Message;
use corex_fleet::session::CompletionResult;
use corex_fleet::{CommandInitiator, RemoteSession};

use super::{EventTx, Flow};
use crate::app::state::App;
use crate::fleet_dialog::{ApprovedCommand, FleetTask, PendingRemoteApproval};

/// Handles a mouse event on the split view. Returns true when it was consumed and must not
/// reach the chat (a click on a server tab, the wheel over the terminal pane).
pub(super) fn handle_mouse(app: &mut App, mouse: MouseEvent, (w, h): (u16, u16)) -> bool {
    let is_wide = app.fleet_dialog.wide_layout && w >= crate::fleet_dialog::FLEET_WIDE_MIN_WIDTH;
    let is_horizontal = w >= 110;
    let split_col = if is_wide {
        (w * 40) / 100
    } else if is_horizontal {
        (w * 55) / 100
    } else {
        w
    };
    let is_on_server = if is_horizontal {
        mouse.column >= split_col
    } else {
        app.fleet_dialog.command_mode
    };

    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        // Check for direct click on VPS tabs in the top header row
        if is_on_server && mouse.row == 0 && !app.fleet_dialog.items.is_empty() {
            let start_col = if is_horizontal { split_col + 5 } else { 5 };
            if let Some(idx) = clicked_tab(app, mouse.column, start_col) {
                app.fleet_dialog.selected_idx = idx;
                ensure_session(app);
                app.fleet_dialog.command_mode = true;
                return true;
            }
        }
        app.fleet_dialog.command_mode = is_on_server;
        return false;
    }

    if !is_on_server {
        return false;
    }
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if let Some(ref pty) = app.fleet_dialog.pty_session {
                pty.scroll_up(3);
                app.fleet_dialog.terminal_scroll = pty.scrollback();
            } else {
                let total = app.fleet_dialog.total_output_lines();
                let view_h = h.saturating_sub(8) as usize;
                app.fleet_dialog.scroll_up(3, total, view_h);
            }
            true
        }
        MouseEventKind::ScrollDown => {
            if let Some(ref pty) = app.fleet_dialog.pty_session {
                pty.scroll_down(3);
                app.fleet_dialog.terminal_scroll = pty.scrollback();
            } else {
                app.fleet_dialog.scroll_down(3);
            }
            true
        }
        _ => false,
    }
}

/// The server tab under `column` in the header row, whose first tab starts at `start_col`.
fn clicked_tab(app: &App, column: u16, start_col: u16) -> Option<usize> {
    if column < start_col {
        return None;
    }
    let mut curr_x = start_col;
    for (i, item) in app.fleet_dialog.items.iter().enumerate() {
        // Tab pattern: " [i] " + id + " ● "
        let tab_w = 4 + item.config.id.chars().count() as u16 + 3;
        if column >= curr_x && column < curr_x + tab_w {
            return Some(i);
        }
        curr_x += tab_w;
    }
    None
}

/// Keys while the fleet console is open. `Flow::Pass` hands a chat-pane key to the chat.
pub(super) fn handle_key(app: &mut App, key: KeyEvent, event_tx: &EventTx) -> Result<Flow> {
    if !app.fleet_dialog.is_open || app.pending_confirmation.is_some() {
        return Ok(Flow::Pass);
    }
    if let Some(pending) = app.fleet_dialog.pending_approval.take() {
        approval_key(app, pending, key, event_tx);
        return Ok(Flow::Handled);
    }

    // Split view: chat (left) and VPS terminal (right).
    // Tab swaps keyboard focus; F1-F9 / Alt+1-9 pick a server; Ctrl+R probes metrics.
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let term_focus = app.fleet_dialog.command_mode;

    let is_pane_toggle = key.code == KeyCode::F(6)
        || (ctrl && matches!(key.code, KeyCode::Char('w') | KeyCode::Char('W')))
        || key.code == KeyCode::BackTab
        || (shift && key.code == KeyCode::Tab)
        || (alt && key.code == KeyCode::Tab)
        || (alt && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')))
        || (!term_focus && key.code == KeyCode::Tab && app.input_buffer.trim().is_empty());

    let pick_server: Option<usize> = match key.code {
        KeyCode::F(n) if (1..=9).contains(&n) && n != 2 => Some((n - 1) as usize),
        KeyCode::Char(d) if alt && d.is_ascii_digit() && d != '0' => Some((d as usize) - ('1' as usize)),
        KeyCode::Char(d) if ctrl && d.is_ascii_digit() && d != '0' => Some((d as usize) - ('1' as usize)),
        _ => None,
    };

    let next_vps = alt && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Right);
    let prev_vps = alt && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P') | KeyCode::Left);
    let open_selector = alt && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S'));

    if let Some(idx) = pick_server {
        if idx < app.fleet_dialog.items.len() {
            app.fleet_dialog.selected_idx = idx;
            ensure_session(app);
        }
    } else if next_vps {
        if !app.fleet_dialog.items.is_empty() {
            app.fleet_dialog.select_next();
            ensure_session(app);
        }
    } else if prev_vps {
        if !app.fleet_dialog.items.is_empty() {
            app.fleet_dialog.select_prev();
            ensure_session(app);
        }
    } else if open_selector {
        app.fleet_dialog.active_session = None;
        app.fleet_dialog.command_mode = true;
    } else if key.code == KeyCode::Char('r') && ctrl {
        probe_telemetry(app);
    } else if is_pane_toggle {
        app.fleet_dialog.command_mode = !term_focus;
    } else if !term_focus {
        // Chat focus: the normal chat keys apply.
        // (Chat PageUp/PageDown scroll the conversation; terminal scroll needs Tab.)
        return Ok(Flow::Pass);
    } else if app.fleet_dialog.is_selected_connected() {
        terminal_key(app, key);
    } else {
        selector_key(app, key);
    }
    Ok(Flow::Handled)
}

/// Asks every server for fresh telemetry (Ctrl+R).
fn probe_telemetry(app: &mut App) {
    app.fleet_dialog.status_msg = Some("Probing telemetry...".to_string());
    let tx = app.fleet_dialog.telemetry_tx.clone();
    for n in app.fleet_dialog.items.iter().map(|i| i.config.clone()) {
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let id = n.id.clone();
            let t = corex_fleet::TelemetryCollector::fetch(&n, 4).await;
            let _ = tx_clone.send((id, t));
        });
    }
}

/// Server selector: Up/Down/Tab to navigate, Enter to launch a session, digits 1-9 pick
/// directly, typing filters.
fn selector_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Esc => {
            if !app.fleet_dialog.search_query.is_empty() {
                app.fleet_dialog.search_query.clear();
                let filtered = app.fleet_dialog.filtered_items();
                if let Some((first, _)) = filtered.first() {
                    app.fleet_dialog.selected_idx = *first;
                }
            } else {
                app.fleet_dialog.command_mode = false;
            }
        }
        KeyCode::Backspace => {
            app.fleet_dialog.search_query.pop();
            keep_selection_in_filter(app);
        }
        KeyCode::Tab => app.fleet_dialog.select_next(),
        KeyCode::BackTab => app.fleet_dialog.select_prev(),
        KeyCode::Up | KeyCode::Char('k') if app.fleet_dialog.search_query.is_empty() || key.code == KeyCode::Up => {
            app.fleet_dialog.select_prev();
        }
        KeyCode::Down | KeyCode::Char('j') if app.fleet_dialog.search_query.is_empty() || key.code == KeyCode::Down => {
            app.fleet_dialog.select_next();
        }
        KeyCode::Char(d) if d.is_ascii_digit() && d != '0' && app.fleet_dialog.search_query.is_empty() => {
            let idx = (d as usize) - ('1' as usize);
            if idx < app.fleet_dialog.items.len() {
                app.fleet_dialog.selected_idx = idx;
                ensure_session(app);
            }
        }
        KeyCode::Char(c) if !ctrl && !alt => {
            app.fleet_dialog.search_query.push(c);
            keep_selection_in_filter(app);
        }
        KeyCode::Enter => {
            ensure_session(app);
        }
        _ => {}
    }
}

/// After the filter changed, moves the selection to the first match if it no longer matches.
fn keep_selection_in_filter(app: &mut App) {
    let filtered = app.fleet_dialog.filtered_items();
    if !filtered.iter().any(|(idx, _)| *idx == app.fleet_dialog.selected_idx) {
        if let Some((first, _)) = filtered.first() {
            app.fleet_dialog.selected_idx = *first;
        }
    }
}

/// Keys for a connected server: straight to the remote shell when a PTY is live, else the
/// simulated terminal (command line, remote completion, scrolling).
fn terminal_key(app: &mut App, key: KeyEvent) {
    if let Some(ref pty) = app.fleet_dialog.pty_session {
        if !pty.is_alive() {
            app.fleet_dialog.pty_session = None;
            app.fleet_dialog.active_session = None;
            return;
        }

        // Keyboard terminal scrolling with PageUp / PageDown:
        if key.code == KeyCode::PageUp {
            pty.scroll_up(15);
            app.fleet_dialog.terminal_scroll = pty.scrollback();
            return;
        } else if key.code == KeyCode::PageDown {
            pty.scroll_down(15);
            app.fleet_dialog.terminal_scroll = pty.scrollback();
            return;
        } else if pty.scrollback() > 0 && matches!(key.code, KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace) {
            pty.scroll_reset();
            app.fleet_dialog.terminal_scroll = 0;
        }

        if let Some(bytes) = crate::fleet_pty::key_event_to_bytes(key) {
            pty.write_bytes(&bytes);
        }
        return;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Tab => complete_remote_command(app),
        KeyCode::Esc => {
            app.fleet_dialog.last_tab_input = None;
            if app.fleet_dialog.command_input.is_empty() {
                // Return to server selector
                app.fleet_dialog.active_session = None;
            } else {
                app.fleet_dialog.command_input.clear();
                app.fleet_dialog.status_msg = None;
            }
        }
        KeyCode::Up => {
            let total = app.fleet_dialog.total_output_lines();
            app.fleet_dialog.scroll_up(1, total, 15);
        }
        KeyCode::Down => app.fleet_dialog.scroll_down(1),
        KeyCode::PageUp => {
            let total = app.fleet_dialog.total_output_lines();
            app.fleet_dialog.scroll_up(10, total, 15);
        }
        KeyCode::PageDown => app.fleet_dialog.scroll_down(10),
        KeyCode::Home => {
            let total = app.fleet_dialog.total_output_lines();
            app.fleet_dialog.scroll_up(total, total, 15);
        }
        KeyCode::End => app.fleet_dialog.terminal_scroll = 0,
        KeyCode::Enter => run_typed_command(app),
        KeyCode::Backspace => {
            app.fleet_dialog.command_input.pop();
            app.fleet_dialog.status_msg = None;
            app.fleet_dialog.last_tab_input = None;
        }
        KeyCode::Char(c) if !ctrl && !alt => {
            app.fleet_dialog.command_input.push(c);
            app.fleet_dialog.status_msg = None;
            app.fleet_dialog.last_tab_input = None;
        }
        _ => {}
    }
}

/// Tab in the simulated terminal: asks the remote shell to complete the command line.
fn complete_remote_command(app: &mut App) {
    let input = app.fleet_dialog.command_input.clone();
    if input.trim().is_empty() {
        return;
    }
    let Some(sess) = ensure_session(app) else {
        return;
    };
    app.fleet_dialog.status_msg = Some("…".to_string());
    let tx = app.fleet_dialog.task_tx.clone();
    tokio::spawn(async move {
        let result = sess.complete_line(&input).await;
        let _ = tx.send(FleetTask::Completed { input, result });
    });
}

/// Applies the answer to a completion request. A second Tab with several candidates lists them.
fn apply_completion(app: &mut App, input: String, result: CompletionResult) {
    if app.fleet_dialog.command_input != input {
        return;
    }
    if result.candidates.is_empty() {
        app.fleet_dialog.status_msg = Some("no completions".to_string());
    } else if result.candidates.len() == 1 {
        app.fleet_dialog.command_input = result.new_input;
        app.fleet_dialog.status_msg = None;
        app.fleet_dialog.last_tab_input = None;
    } else if result.advanced {
        app.fleet_dialog.command_input = result.new_input;
        app.fleet_dialog.status_msg = None;
        app.fleet_dialog.last_tab_input = Some(app.fleet_dialog.command_input.clone());
    } else {
        let is_double_tab = input.ends_with(char::is_whitespace)
            || input.ends_with('/')
            || app.fleet_dialog.last_tab_input.as_deref() == Some(&input);
        if is_double_tab {
            app.fleet_dialog.last_tab_input = None;
            let preview = if result.candidates.len() > 8 {
                format!("{}  … (+{} more)", result.candidates[..8].join("  "), result.candidates.len() - 8)
            } else {
                result.candidates.join("  ")
            };
            app.fleet_dialog.status_msg = Some(preview);
        } else {
            app.fleet_dialog.last_tab_input = Some(input);
            app.fleet_dialog.status_msg = None;
        }
    }
}

/// Enter in the simulated terminal: runs the typed command on the server in the background.
fn run_typed_command(app: &mut App) {
    app.fleet_dialog.last_tab_input = None;
    let cmd = app.fleet_dialog.command_input.trim().to_string();
    if cmd.is_empty() {
        return;
    }
    let Some(mut sess) = ensure_session(app) else {
        app.fleet_dialog.status_msg = Some("Connecting... press Enter again once it is connected".to_string());
        return;
    };
    app.fleet_dialog.command_input.clear();
    app.fleet_dialog.terminal_scroll = 0;
    if cmd == "clear" || cmd == "cls" {
        sess.clear_history();
        let stored = sess.clone();
        tokio::spawn(async move {
            corex_fleet::global_session_manager().set_session(stored.node.id.clone(), stored).await;
        });
        app.fleet_dialog.active_session = Some(sess);
        app.fleet_dialog.status_msg = None;
        return;
    }
    let first_word = cmd.split_whitespace().next().unwrap_or("");
    if matches!(first_word, "top" | "htop" | "btop" | "vim" | "vi" | "nano" | "emacs" | "less" | "more" | "man") {
        app.fleet_dialog.status_msg = Some(format!("⚠️ '{}' requires an interactive PTY (PTY unavailable on this host)", first_word));
        return;
    }
    app.fleet_dialog.status_msg = Some(format!("$ {}", cmd));
    let tx = app.fleet_dialog.cmd_done_tx.clone();
    let nid = sess.node.id.clone();
    tokio::spawn(async move {
        let _ = sess.execute_command(&cmd, CommandInitiator::User, false, 20).await;
        corex_fleet::global_session_manager().set_session(nid.clone(), sess.clone()).await;
        let _ = tx.send((nid, sess));
    });
}

/// The approval box for a remote command the model wants to run: [y] allow, [e] edit,
/// [w] deny with instructions, [n] deny. Typing goes to the edit or instructions line when open.
fn approval_key(app: &mut App, mut pending: PendingRemoteApproval, key: KeyEvent, event_tx: &EventTx) {
    if pending.edit_mode {
        match key.code {
            KeyCode::Esc => pending.edit_mode = false,
            KeyCode::Enter => {
                let cmd = pending.edited_command.trim().to_string();
                spawn_approved(app, pending, cmd, true, event_tx);
                return;
            }
            KeyCode::Backspace => {
                pending.edited_command.pop();
            }
            KeyCode::Char(c) => pending.edited_command.push(c),
            _ => {}
        }
        app.fleet_dialog.pending_approval = Some(pending);
        return;
    }

    if pending.instructions_mode {
        match key.code {
            KeyCode::Esc => pending.instructions_mode = false,
            KeyCode::Enter => {
                let text = pending.instructions_text.trim().to_string();
                if let Some(ref pty) = app.fleet_dialog.pty_session {
                    pty.feed_text(&format!("\r\n\x1b[1;35m! Instructions: {}\x1b[0m\r\n", text));
                }
                answer_remote_call(app, pending.call_id, format!("Command denied by SysAdmin with instructions: {}", text));
                let _ = app.session.save();
                app.start_stream_turn(event_tx.clone());
                return;
            }
            KeyCode::Backspace => {
                pending.instructions_text.pop();
            }
            KeyCode::Char(c) => pending.instructions_text.push(c),
            _ => {}
        }
        app.fleet_dialog.pending_approval = Some(pending);
        return;
    }

    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            let cmd = pending.command.clone();
            spawn_approved(app, pending, cmd, false, event_tx);
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            pending.edit_mode = true;
            pending.edited_command = pending.command.clone();
            app.fleet_dialog.pending_approval = Some(pending);
        }
        KeyCode::Char('w') | KeyCode::Char('W') => {
            pending.instructions_mode = true;
            pending.instructions_text.clear();
            app.fleet_dialog.pending_approval = Some(pending);
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            if let Some(ref pty) = app.fleet_dialog.pty_session {
                pty.feed_text(&format!("\r\n\x1b[1;31m✕ [declined] $ {}\x1b[0m\r\n", pending.command));
            }
            answer_remote_call(app, pending.call_id, "Remote command execution denied by SysAdmin.".to_string());
            let _ = app.session.save();
            app.start_stream_turn(event_tx.clone());
        }
        _ => app.fleet_dialog.pending_approval = Some(pending),
    }
}

/// Answers the model's `fleet_run_command` call `call_id` with `output`.
fn answer_remote_call(app: &mut App, call_id: String, output: String) {
    let mut tool_msg = Message::tool_response(call_id, output);
    tool_msg.name = Some("fleet_run_command".to_string());
    app.session.add_message(tool_msg);
}

/// Runs an approved remote command (`edited` when the SysAdmin changed it) in the background.
/// The turn waits for it, shown as activity, and `on_approved` finishes the job.
fn spawn_approved(app: &mut App, pending: PendingRemoteApproval, command: String, edited: bool, event_tx: &EventTx) {
    let node_id = pending.node_id.clone();
    let live = app.fleet_dialog.active_session.clone().filter(|s| s.node.id == node_id);
    let node = app.fleet_dialog.items.iter().find(|i| i.config.id == node_id).map(|i| i.config.clone());
    if live.is_none() && node.is_none() {
        answer_remote_call(app, pending.call_id, format!("Error: Node {} not found", node_id));
        return;
    }
    let _ = event_tx;
    app.is_streaming = true;
    app.set_status(&format!("Running on {}: {}", node_id, corex_core::truncate_ellipsis(&command, 60)));
    let generation = app.current_generation_id;
    let tx = app.fleet_dialog.task_tx.clone();
    tokio::spawn(async move {
        let mut session = match live {
            Some(s) => s,
            None => {
                let mut s = RemoteSession::new(node.expect("checked above"));
                let _ = s.connect(8).await;
                s
            }
        };
        let result = session
            .execute_command(&command, CommandInitiator::AI, pending.sudo, 15)
            .await
            .map_err(|e| e.to_string());
        if result.is_ok() {
            corex_fleet::global_session_manager().set_session(node_id, session.clone()).await;
        }
        let result = result.map(|record| (session, record));
        let _ = tx.send(FleetTask::Approved(Box::new(ApprovedCommand { pending, command, edited, generation, result })));
    });
}

/// An approved command finished: echo it in the terminal pane, answer the model's call with the
/// result and let the model go on, unless the turn was cancelled meanwhile.
fn on_approved(app: &mut App, done: ApprovedCommand, event_tx: &EventTx) {
    let ApprovedCommand { pending, command, edited, generation, result } = done;
    let call_id = pending.call_id;
    let node_id = pending.node_id;
    match result {
        Ok((session, rec)) => {
            app.fleet_dialog.active_session = Some(session);
            if let Some(ref pty) = app.fleet_dialog.pty_session {
                pty.feed_text(&format!("\r\n\x1b[1;33m› [corex] $ {}\x1b[0m\r\n", command));
                feed_output(pty, &rec.stdout, &rec.stderr);
            }
            let by = if edited { " (edited by SysAdmin)" } else { "" };
            let output = if rec.stderr.trim().is_empty() {
                format!("[{}]{} Exit code: {} | CWD: {}\n{}", node_id, by, rec.exit_code, rec.cwd_after, rec.stdout)
            } else {
                format!("[{}]{} Exit code: {} | CWD: {}\n{}\n--- STDERR ---\n{}", node_id, by, rec.exit_code, rec.cwd_after, rec.stdout, rec.stderr)
            };
            answer_remote_call(app, call_id, output);
            let _ = app.session.save();
        }
        Err(e) => {
            if let Some(ref pty) = app.fleet_dialog.pty_session {
                pty.feed_text(&format!("\r\n\x1b[1;31m! Error: {}\x1b[0m\r\n", e));
            }
            answer_remote_call(app, call_id, format!("Remote execution error: {}", e));
        }
    }
    if generation == app.current_generation_id {
        app.start_stream_turn(event_tx.clone());
    }
}

/// Writes a command's output to the terminal pane, stderr in red, with every line break a
/// carriage return too.
fn feed_output(pty: &crate::fleet_pty::FleetPtySession, stdout: &str, stderr: &str) {
    let crlf = |s: &str| s.replace("\r\n", "\n").replace('\n', "\r\n");
    let out = crlf(stdout);
    if !out.trim_end().is_empty() {
        pty.feed_text(&format!("{}\r\n", out.trim_end()));
    }
    let err = crlf(stderr);
    if !err.trim_end().is_empty() {
        pty.feed_text(&format!("\x1b[31m{}\x1b[0m\r\n", err.trim_end()));
    }
}

/// Handles what the background tasks have finished since the last iteration.
pub(super) fn poll_tasks(app: &mut App, event_tx: &EventTx) -> bool {
    let mut changed = false;
    while let Ok(task) = app.fleet_dialog.task_rx.try_recv() {
        changed = true;
        match task {
            FleetTask::Connected { node_id, result } => on_connected(app, node_id, result),
            FleetTask::Approved(done) => on_approved(app, *done, event_tx),
            FleetTask::Completed { input, result } => apply_completion(app, input, result),
        }
    }
    changed
}

/// Shows a finished tool's output in the fleet panes: refreshes the session of the node it ran
/// on and echoes a `fleet_run_command` result in the terminal.
pub(super) async fn show_tool_output(app: &mut App, tool_name: Option<&str>, output: &str) {
    let target_node = app.fleet_dialog.active_session.as_ref().map(|s| s.node.id.clone())
        .or_else(|| app.fleet_dialog.selected_node().map(|n| n.id.clone()));
    if let Some(ref nid) = target_node {
        if let Some(sess) = corex_fleet::global_session_manager().get_session(nid).await {
            app.fleet_dialog.active_session = Some(sess);
            app.fleet_dialog.terminal_scroll = 0;
        }
    }
    if let Some(ref pty) = app.fleet_dialog.pty_session {
        if tool_name == Some("fleet_run_command") {
            let (stdout, stderr) = split_streams(output);
            feed_output(pty, stdout, stderr);
        }
    }
    app.fleet_dialog.status_msg = None;
}

/// The stdout and stderr parts of a `fleet_run_command` result.
fn split_streams(output: &str) -> (&str, &str) {
    const STDOUT: &str = "--- STDOUT ---\n";
    const STDERR: &str = "--- STDERR ---\n";
    let Some(start) = output.find(STDOUT) else {
        return (output, "");
    };
    let rest = &output[start + STDOUT.len()..];
    match rest.find(STDERR) {
        Some(err) => (&rest[..err], &rest[err + STDERR.len()..]),
        None => (rest, ""),
    }
}

/// Echoes the command a `fleet_run_command` call starts, in full, in the terminal pane.
pub(super) fn echo_remote_command(app: &App, call_id: &str) {
    let Some(ref pty) = app.fleet_dialog.pty_session else {
        return;
    };
    let call = app
        .session
        .messages
        .iter()
        .rev()
        .find_map(|m| m.tool_calls.as_ref()?.iter().find(|c| c.id == call_id));
    let Some(call) = call else {
        return;
    };
    let args = corex_core::parse_lenient_json(&call.function.arguments).unwrap_or(serde_json::Value::Null);
    let Some(command) = args.get("command").and_then(|c| c.as_str()) else {
        return;
    };
    let node = args.get("node_id").and_then(|n| n.as_str()).filter(|n| !n.is_empty());
    let target = node.map(|n| format!("[{}] ", n)).unwrap_or_default();
    pty.feed_text(&format!("\r\n\x1b[1;33m› [corex] {}$ {}\x1b[0m\r\n", target, command.replace('\n', "\r\n")));
}

/// Returns the live SSH session for the selected node when there is one. Otherwise it starts
/// connecting in the background and returns `None`: `on_connected` picks the session up.
pub(super) fn ensure_session(app: &mut App) -> Option<RemoteSession> {
    let node = app.fleet_dialog.selected_node().cloned()?;
    if let Some(sess) = app.fleet_dialog.active_session.clone() {
        if sess.is_connected && sess.node.id == node.id {
            if app.fleet_dialog.pty_session.as_ref().map(|p| !p.is_alive() || p.node_id != node.id).unwrap_or(true) {
                spawn_pty(app, &node, "fallback batch SSH active");
            }
            return Some(sess);
        }
    }
    if app.fleet_dialog.connecting.as_deref() == Some(node.id.as_str()) {
        return None;
    }
    app.fleet_dialog.connecting = Some(node.id.clone());
    app.fleet_dialog.status_msg = Some(format!("Connecting to {}...", node.id));
    let tx = app.fleet_dialog.task_tx.clone();
    tokio::spawn(async move {
        let mut session = RemoteSession::new(node.clone());
        let result = match session.connect(8).await {
            Ok(_) => {
                let manager = corex_fleet::global_session_manager();
                manager.set_session(node.id.clone(), session.clone()).await;
                manager.set_active_node(Some(node.id.clone())).await;
                Ok(Box::new(session))
            }
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(FleetTask::Connected { node_id: node.id, result });
    });
    None
}

/// Opens the interactive terminal pane for `node`, saying in the status line why it could not.
fn spawn_pty(app: &mut App, node: &corex_fleet::NodeConfig, fallback: &str) {
    match crate::fleet_pty::FleetPtySession::spawn(node, 24, 80) {
        Ok(pty) => app.fleet_dialog.pty_session = Some(pty),
        Err(err) => {
            app.fleet_dialog.pty_session = None;
            app.fleet_dialog.status_msg = Some(format!("Interactive PTY unavailable ({}); {}", err, fallback));
        }
    }
}

/// A background connection finished.
fn on_connected(app: &mut App, node_id: String, result: Result<Box<RemoteSession>, String>) {
    if app.fleet_dialog.connecting.as_deref() == Some(node_id.as_str()) {
        app.fleet_dialog.connecting = None;
    }
    let session = match result {
        Ok(session) => session,
        Err(e) => {
            app.fleet_dialog.status_msg = Some(format!("Connection failed: {}", e));
            return;
        }
    };
    let node = session.node.clone();
    app.fleet_dialog.status_msg = None;
    if app.fleet_dialog.selected_node().map(|n| n.id.as_str()) == Some(node_id.as_str()) {
        app.fleet_dialog.active_session = Some(*session);
        app.fleet_dialog.terminal_scroll = 0;
        spawn_pty(app, &node, "fallback batch SSH mode active (interactive tools like htop/nano disabled)");
    }
    app.session.add_message(Message::system(format!(
        "Connected to remote server: {} ({}@{}) · Active and ready.",
        node.id, node.user, node.host
    )));
    app.scroll_to_bottom();
    let tx = app.fleet_dialog.telemetry_tx.clone();
    tokio::spawn(async move {
        let tel = corex_fleet::TelemetryCollector::fetch(&node, 4).await;
        let _ = tx.send((node.id, tel));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_split_into_its_streams() {
        let both = "[web] Exit code: 0\n--- STDOUT ---\nline one\nline two\n--- STDERR ---\nwarn\n";
        assert_eq!(split_streams(both), ("line one\nline two\n", "warn\n"));
        assert_eq!(split_streams("--- STDOUT ---\nonly\n"), ("only\n", ""));
        assert_eq!(split_streams("no markers"), ("no markers", ""));
    }
}
