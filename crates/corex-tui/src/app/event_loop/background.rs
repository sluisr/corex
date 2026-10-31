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
    let mut changed = refresh_active_tasks(app);

    if !app.is_streaming && app.in_flight_tool_calls == 0 {
        for msg_text in finished_task_reports().into_iter().chain(finished_subagent_reports()) {
            app.session.add_message(Message::system(msg_text));
            app.invalidate_message_cache();
            app.pending_task_wakeup = true;
            changed = true;
        }
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
        } else if app.pending_task_wakeup {
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
        && !app.rewind_dialog.is_open
        && !app.sudo_dialog.is_open
        && app.fleet_dialog.pending_approval.is_none()
        && !app.is_checking_balance
}

fn finished_task_reports() -> Vec<String> {
    let manager = corex_tools::background::get_task_manager();
    let Ok(mut mgr) = manager.lock() else {
        return Vec::new();
    };
    mgr.drain_unreported().into_iter().map(task_report).collect()
}

fn task_report(task: corex_tools::background::FinishedTask) -> String {
    let code_str = match task.exit_code {
        Some(c) => c.to_string(),
        None => "stopped".to_string(),
    };
    let mut msg_text = format!("TASK_DONE|{}|{}|{}\n", task.pid, code_str, task.duration);
    let trimmed = task.output.trim_end();
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
    msg_text
}

fn finished_subagent_reports() -> Vec<String> {
    corex_core::subagent::get_subagent_manager()
        .lock()
        .map(|mut mgr| mgr.drain_unacknowledged())
        .unwrap_or_default()
        .into_iter()
        .map(|(sub_id, role, dur, output)| {
            let mut msg_text = format!("SUBAGENT_DONE|{}|{}|{}\n", sub_id, role.as_str().to_uppercase(), dur);
            let trimmed = output.trim_end();
            if trimmed.len() > SUBAGENT_DONE_MAX_BYTES {
                msg_text.push_str(corex_core::safe_truncate_str(trimmed, SUBAGENT_DONE_MAX_BYTES));
                msg_text.push_str("\n[... report truncated; use manage_subagents status/log for the full output ...]");
            } else {
                msg_text.push_str(trimmed);
            }
            msg_text
        })
        .collect()
}

fn refresh_active_tasks(app: &mut App) -> bool {
    let manager = corex_tools::background::get_task_manager();
    let Ok(mgr) = manager.lock() else {
        return false;
    };
    let still_active: std::collections::HashSet<u32> = mgr
        .all_processes()
        .iter()
        .filter(|(_, p)| p.is_active() && p.is_backgrounded())
        .map(|(pid, _)| *pid)
        .collect();
    let changed = still_active != app.active_background_pids;
    app.active_background_pids = still_active;
    changed
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
