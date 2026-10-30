//! Work that finishes outside the stream: background tasks, subagents and the balance query.
//! Each loop iteration collects what finished and, when idle, starts the next turn.

use corex_core::types::Message;

use super::EventTx;
use crate::app::state::App;

const TASK_DONE_TAIL_BYTES: usize = 12_000;
const SUBAGENT_DONE_MAX_BYTES: usize = 16_000;

/// Reports finished background tasks and subagents, applies the balance answer and, when
/// nothing else is going on, sends the next queued message. Returns true when the screen changed.
pub(super) fn poll_background_work(app: &mut App, event_tx: &EventTx) -> bool {
    let mut changed = false;
    let mut has_finished_tasks = false;

    for (pid, exit_code, dur, output) in drain_finished_tasks(app) {
        let code_str = match exit_code {
            Some(c) => c.to_string(),
            None => "stopped".to_string(),
        };
        let mut msg_text = format!("TASK_DONE|{}|{}|{}\n", pid, code_str, dur);
        let trimmed = output.trim_end();
        if trimmed.len() > TASK_DONE_TAIL_BYTES {
            let mut cut = trimmed.len() - TASK_DONE_TAIL_BYTES;
            while !trimmed.is_char_boundary(cut) {
                cut += 1;
            }
            msg_text.push_str(&format!("[... {} earlier bytes omitted; use manage_task status for more ...]\n", cut));
            msg_text.push_str(&trimmed[cut..]);
        } else {
            msg_text.push_str(trimmed);
        }
        app.session.add_message(Message::system(msg_text));
        app.invalidate_message_cache();
        changed = true;
        if exit_code.is_some() {
            has_finished_tasks = true;
        }
    }

    // Finished subagents (reactive wakeup with isolated, clean IPC).
    let newly_finished_subagents = corex_core::subagent::get_subagent_manager()
        .lock()
        .map(|mut mgr| mgr.drain_unacknowledged())
        .unwrap_or_default();
    for (sub_id, role, dur, output) in newly_finished_subagents {
        let mut msg_text = format!("SUBAGENT_DONE|{}|{}|{}\n", sub_id, role.as_str().to_uppercase(), dur);
        let trimmed = output.trim_end();
        if trimmed.len() > SUBAGENT_DONE_MAX_BYTES {
            msg_text.push_str(corex_core::safe_truncate_str(trimmed, SUBAGENT_DONE_MAX_BYTES));
            msg_text.push_str("\n[... report truncated; use manage_subagents status/log for the full output ...]");
        } else {
            msg_text.push_str(trimmed);
        }
        app.session.add_message(Message::system(msg_text));
        app.invalidate_message_cache();
        changed = true;
        has_finished_tasks = true;
    }

    if poll_balance(app) {
        changed = true;
    }

    if is_idle(app) {
        if app.deliver_steer() {
            // A "send now" message that waited for a dialog to close.
            app.start_stream_turn(event_tx.clone());
            changed = true;
        } else if let Some(next_prompt) = app.message_queue.pop_front() {
            corex_core::reset_repeat_guard();
            let message = app.user_message(next_prompt);
            app.session.add_message(message);
            app.start_stream_turn(event_tx.clone());
            changed = true;
        } else if has_finished_tasks {
            // Resume turn when background tasks complete while idle.
            app.start_stream_turn(event_tx.clone());
            changed = true;
        }
    }
    changed
}

/// True when a background task or subagent is still running.
pub(super) fn has_active_tasks(app: &App) -> bool {
    let has_active_subagents = corex_core::subagent::get_subagent_manager()
        .lock()
        .map(|mgr| mgr.active_count() > 0)
        .unwrap_or(false);
    !app.active_background_pids.is_empty() || has_active_subagents
}

/// Nothing is streaming, running or waiting for an answer: a new turn may start.
fn is_idle(app: &App) -> bool {
    !app.is_streaming
        && app.in_flight_tool_calls == 0
        && app.pending_confirmation.is_none()
        && !app.user_dialog.is_open
        && !app.sudo_dialog.is_open
        && app.fleet_dialog.pending_approval.is_none()
        && !app.is_checking_balance
}

/// Backgrounded tasks that ended since the last call, as `(pid, exit code, duration, output)`.
/// Also refreshes the set of tasks still running.
fn drain_finished_tasks(app: &mut App) -> Vec<(u32, Option<i32>, String, String)> {
    let manager = corex_tools::background::get_task_manager();
    let Ok(mgr) = manager.lock() else {
        return Vec::new();
    };
    let mut finished = Vec::new();
    let mut still_active = std::collections::HashSet::new();

    for &pid in &app.active_background_pids {
        if let Some(proc) = mgr.get_process(pid) {
            if proc.is_active() && proc.is_backgrounded() {
                still_active.insert(pid);
            } else if proc.is_backgrounded() {
                let dur = proc.duration_str();
                let output = proc.output_buffer.lock().map(|b| b.clone()).unwrap_or_default();
                finished.push((pid, proc.get_exit_code(), dur, output));
            }
        } else {
            finished.push((pid, None, String::new(), String::new()));
        }
    }

    for (pid, proc) in mgr.all_processes() {
        if proc.is_active() && proc.is_backgrounded() {
            still_active.insert(*pid);
        }
    }

    app.active_background_pids = still_active;
    finished
}

/// Puts the answer of a `/balance` query in place of its placeholder message.
fn poll_balance(app: &mut App) -> bool {
    let mut changed = false;
    while let Ok(res) = app.balance_rx.try_recv() {
        app.clear_status();
        app.is_checking_balance = false;

        let msg = match res {
            Ok(bal) => {
                let mut msg = format!("DeepSeek Account Balance (Available: {}):\n", bal.is_available);
                for info in bal.balance_infos {
                    msg.push_str(&format!(
                        "- Total: {} {}\n  (Topped up: {} {}, Granted: {} {})\n",
                        info.total_balance, info.currency,
                        info.topped_up_balance, info.currency,
                        info.granted_balance, info.currency
                    ));
                }
                msg
            }
            Err(e) => format!("Failed to retrieve balance: {}", e),
        };

        match app.pending_balance_msg_index.take() {
            Some(idx) if idx < app.session.messages.len() => {
                app.session.replace_message(idx, Message::system(msg));
            }
            _ => app.session.add_message(Message::system(msg)),
        }
        app.invalidate_message_cache();
        changed = true;
    }
    changed
}
