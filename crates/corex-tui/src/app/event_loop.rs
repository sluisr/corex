//! Main interactive event loop, key/mouse dispatch, and terminal session lifecycle.

use std::io::{stdout, Write};
use std::time::{Duration, Instant};
use anyhow::Result;
use crossterm::event::{
    self, EnableBracketedPaste, EnableFocusChange, Event, KeyCode,
    KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use corex_core::client::{get_sudo_password, set_sudo_password, StreamEvent};
use corex_core::session::Session;
use corex_core::types::{Message, ToolCall};
use corex_tools::types::ToolContext;
use corex_tools::{command_requires_sudo, extract_first_sudo_command};

use super::formatting::*;
use super::mobile::{arrows_may_be_swipes, is_android_or_termux};
use super::render::render_ui;
use super::state::{App, PendingToolBatch};
use super::summary::print_session_summary;
use super::terminal::{restore_terminal, TerminalGuard, MOUSE_REPORTING_OFF, MOUSE_REPORTING_ON};
use crate::user_dialog::parse_questions;

pub fn format_call_preview(call: &ToolCall, registry: &corex_tools::ToolRegistry, workspace_dir: &std::path::Path) -> String {
    let args_json = corex_core::parse_lenient_json(&call.function.arguments)
        .unwrap_or(serde_json::Value::Null);
    let tool = registry.get(&call.function.name);
    if call.function.name == "run_shell_command" {
        if let Some(cmd) = args_json.get("command").and_then(|c| c.as_str()) {
            return cmd.to_string();
        }
    } else if call.function.name == "delegate_subagent" {
        // The whole instruction is what gets approved, so the dialog shows all of it.
        let role = args_json.get("role").and_then(|r| r.as_str()).unwrap_or("worker");
        let prompt = args_json.get("prompt").and_then(|p| p.as_str()).unwrap_or("").trim();
        return format!("Subagent [{}]\n{}", role.to_uppercase(), prompt);
    } else if let Some(t) = tool {
        if let Some(diff) = t.format_diff(&args_json, workspace_dir) {
            return diff;
        }
    }
    let summary = if let Some(obj) = args_json.as_object() {
        obj.iter()
            .take(2)
            .map(|(k, v)| {
                let val_str = match v {
                    serde_json::Value::String(s) => s.clone(),
                    _ => v.to_string(),
                };
                let truncated = if val_str.len() > 60 {
                    format!("{}...", corex_core::types::safe_truncate_str(&val_str, 60))
                } else {
                    val_str
                };
                format!("{}: {}", k, truncated)
            })
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        String::new()
    };
    if summary.is_empty() {
        format!("{}(...)", call.function.name)
    } else {
        format!("{}({})", call.function.name, summary)
    }
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

        // Check for finished background tasks and notify the user/session
        let newly_finished: Vec<(u32, Option<i32>, String, String)> = {
            if let Ok(mgr) = corex_tools::background::get_task_manager().lock() {
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
            } else {
                Vec::new()
            }
        };

        const TASK_DONE_TAIL_BYTES: usize = 12_000;
        const SUBAGENT_DONE_MAX_BYTES: usize = 16_000;

        let mut has_finished_tasks = false;
        for (pid, exit_code, dur, output) in newly_finished {
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
            needs_redraw = true;
            if exit_code.is_some() {
                has_finished_tasks = true;
            }
        }

        // Check for finished subagents (reactive wakeup with isolated, clean IPC)
        let newly_finished_subagents = {
            if let Ok(mut mgr) = corex_core::subagent::get_subagent_manager().lock() {
                mgr.drain_unacknowledged()
            } else {
                Vec::new()
            }
        };

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
            needs_redraw = true;
            has_finished_tasks = true;
        }

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
                Err(e) => {
                    format!("Failed to retrieve balance: {}", e)
                }
            };

            if let Some(idx) = app.pending_balance_msg_index.take() {
                if idx < app.session.messages.len() {
                    app.session.messages[idx] = Message::system(msg);
                } else {
                    app.session.add_message(Message::system(msg));
                }
            } else {
                app.session.add_message(Message::system(msg));
            }
            app.invalidate_message_cache();
            needs_redraw = true;
        }

        if !app.is_streaming
            && app.in_flight_tool_calls == 0
            && app.pending_confirmation.is_none()
            && !app.user_dialog.is_open
            && !app.sudo_dialog.is_open
            && app.fleet_dialog.pending_approval.is_none()
            && !app.is_checking_balance
        {
            if app.deliver_steer() {
                // A "send now" message that waited for a dialog to close.
                app.start_stream_turn(event_tx.clone());
                needs_redraw = true;
            } else if !app.message_queue.is_empty() {
                if let Some(next_prompt) = app.message_queue.pop_front() {
                    corex_core::reset_repeat_guard();
                    let message = app.user_message(next_prompt);
                    app.session.add_message(message);
                    app.start_stream_turn(event_tx.clone());
                    needs_redraw = true;
                }
            } else if has_finished_tasks {
                // Resume turn when background tasks complete while idle.
                app.start_stream_turn(event_tx.clone());
                needs_redraw = true;
            }
        }

        let has_active_subagents = {
            if let Ok(mgr) = corex_core::subagent::get_subagent_manager().lock() {
                mgr.active_count() > 0
            } else {
                false
            }
        };
        let has_active_tasks = !app.active_background_pids.is_empty() || has_active_subagents;
        let is_animating = app.is_slash_animating() || app.status_transition.is_animating();

        // The caret blinks off the idle clock; a phase change is the only reason to repaint a still screen.
        if crate::cursor::tick(app.last_interaction.elapsed()) {
            needs_redraw = true;
        }

        if app.fleet_dialog.poll_telemetry() {
            needs_redraw = true;
        }

        while let Ok((node_id, sess)) = app.fleet_dialog.cmd_done_rx.try_recv() {
            corex_fleet::global_session_manager().set_session(node_id.clone(), sess.clone()).await;
            app.fleet_dialog.active_session = Some(sess);
            app.fleet_dialog.status_msg = None;
            needs_redraw = true;
        }

        if app.fleet_dialog.is_open && app.fleet_dialog.auto_connect_pending {
            app.fleet_dialog.auto_connect_pending = false;
            if let Some(id) = app.fleet_dialog.selected_node().map(|n| n.id.clone()) {
                app.fleet_dialog.status_msg = Some(format!("Connecting to {}...", id));
                terminal.draw(|f| {
                    render_ui(f, &mut app);
                })?;
                fleet_ensure_session(&mut app).await;
                needs_redraw = true;
            }
        }

        let pty_dirty = app.fleet_dialog.is_open && app.fleet_dialog.pty_session.as_ref().map(|p| p.is_alive() && p.take_dirty()).unwrap_or(false);
        if needs_redraw || app.is_streaming || app.is_checking_balance || has_active_tasks || is_animating || pty_dirty {
            terminal.draw(|f| {
                render_ui(f, &mut app);
            })?;
            needs_redraw = false;
        }

        while let Ok((event_gen, event)) = event_rx.try_recv() {
            if event_gen != app.current_generation_id {
                // Drop stale/cancelled turn event to prevent concurrent interleaving
                continue;
            }
            needs_redraw = true;
            match event {
                StreamEvent::ReasoningDelta(delta) => {
                    app.thinking_state.content.push_str(&delta);
                    app.set_status("Thinking...");
                }
                StreamEvent::ContentDelta(delta) => {
                    app.streaming_text.push_str(&delta);
                    app.set_status("Generating response...");
                }
                StreamEvent::ToolCallDelta { index, id, name, arguments, extra_content } => {
                    if index > 64 {
                        continue;
                    }
                    while app.streaming_tool_calls.len() <= index {
                        app.streaming_tool_calls.push(ToolCall {
                            id: String::new(),
                            call_type: "function".to_string(),
                            function: corex_core::types::FunctionCall {
                                name: String::new(),
                                arguments: String::new(),
                            },
                            extra_content: None,
                        });
                    }
                    if let Some(i) = id {
                        app.streaming_tool_calls[index].id.push_str(&i);
                    }
                    if let Some(n) = name {
                        app.streaming_tool_calls[index].function.name.push_str(&n);
                    }
                    if let Some(a) = arguments {
                        app.streaming_tool_calls[index].function.arguments.push_str(&a);
                    }
                    if let Some(ec) = extra_content {
                        app.streaming_tool_calls[index].extra_content = Some(ec);
                    }
                }
                StreamEvent::ExtraContentDelta(ec) => {
                    app.streaming_extra_content = Some(ec);
                }
                StreamEvent::UsageUpdate(usage) => {
                    app.session.update_usage(&usage);
                }
                StreamEvent::ToolExecutionStarting { summary, name, .. } => {
                    app.set_status(&summary);
                    if app.fleet_dialog.is_open {
                        app.fleet_dialog.status_msg = Some(summary.clone());
                        if let Some(ref pty) = app.fleet_dialog.pty_session {
                            if name == "fleet_run_command" {
                                let cmd = summary
                                    .strip_prefix("fleet_run_command ")
                                    .unwrap_or(&summary);
                                pty.feed_text(&format!("\r\n\x1b[1;33m› [corex] $ {}\x1b[0m\r\n", cmd));
                            }
                        }
                    }
                }
                StreamEvent::ToolExecutionDone { call_id, output } => {
                    app.in_flight_tool_calls = app.in_flight_tool_calls.saturating_sub(1);
                    if output.contains("incorrect password attempt")
                        || output.contains("sudo: a password is required")
                        || output.contains("sudo: PAM authentication")
                    {
                        set_sudo_password(None);
                    }
                    let tool_name = app.session.messages.iter().rev()
                        .find_map(|m| m.tool_calls.as_ref()?.iter().find(|c| c.id == call_id).map(|c| c.function.name.clone()));
                    // Huge output is cut to head + tail; the full text stays readable from a file.
                    let output = corex_core::spill_large_output(tool_name.as_deref().unwrap_or("tool"), output);
                    // Advisory nudge when the model keeps repeating the exact same call.
                    let tool_args = app.session.messages.iter().rev()
                        .find_map(|m| m.tool_calls.as_ref()?.iter().find(|c| c.id == call_id).map(|c| c.function.arguments.clone()));
                    let output = match (tool_name.as_deref(), tool_args) {
                        (Some(name), Some(args)) => match corex_core::observe_tool_call(name, &args) {
                            Some(reminder) => format!("{}\n\n{}", output, reminder),
                            None => output,
                        },
                        _ => output,
                    };
                    let mut tool_msg = Message::tool_response(call_id, output.clone());
                    tool_msg.name = tool_name.clone();
                    app.session.add_message(tool_msg);

                    if app.fleet_dialog.is_open {
                        let target_node = app.fleet_dialog.active_session.as_ref().map(|s| s.node.id.clone())
                            .or_else(|| app.fleet_dialog.selected_node().map(|n| n.id.clone()));
                        if let Some(ref nid) = target_node {
                            if let Some(sess) = corex_fleet::global_session_manager().get_session(nid).await {
                                app.fleet_dialog.active_session = Some(sess);
                                app.fleet_dialog.terminal_scroll = 0;
                            }
                        }
                        if let Some(ref pty) = app.fleet_dialog.pty_session {
                            if tool_name.as_deref() == Some("fleet_run_command") {
                                let display_out = if let Some(idx) = output.find("--- STDOUT ---\n") {
                                    let rest = &output[idx + "--- STDOUT ---\n".len()..];
                                    if let Some(err_idx) = rest.find("--- STDERR ---\n") {
                                        &rest[..err_idx]
                                    } else {
                                        rest
                                    }
                                } else {
                                    &output
                                };
                                let formatted = display_out
                                    .replace("\r\n", "\n")
                                    .replace('\n', "\r\n");
                                let trimmed = formatted.trim_end();
                                if !trimmed.is_empty() {
                                    pty.feed_text(&format!("{}\r\n", trimmed));
                                }
                            }
                        }
                        app.fleet_dialog.status_msg = None;
                    }
                    if app.pending_confirmation.is_some()
                        || app.sudo_dialog.is_open
                        || app.user_dialog.is_open
                        || app.fleet_dialog.pending_approval.is_some()
                    {
                        if app.in_flight_tool_calls == 0 {
                            app.is_streaming = false;
                            app.clear_status();
                        }
                        needs_redraw = true;
                    } else if app.in_flight_tool_calls == 0 {
                        if let Some(image_msg) = corex_core::drain_tool_images_message() {
                            app.session.add_message(image_msg);
                        }
                        // Every tool call has its response: the safe point to hand the model what
                        // the user sent with "send now".
                        app.deliver_steer();
                        app.start_stream_turn(event_tx.clone());
                    }
                }
                StreamEvent::AllToolsDone => {
                    // Trigger next recursive turn of agent loop only when all in-flight tools and dialogs are complete
                    if app.in_flight_tool_calls == 0
                        && app.pending_confirmation.is_none()
                        && !app.sudo_dialog.is_open
                        && !app.user_dialog.is_open
                        && app.fleet_dialog.pending_approval.is_none()
                    {
                        // Images a tool produced (read_image) follow all tool responses.
                        if let Some(image_msg) = corex_core::drain_tool_images_message() {
                            app.session.add_message(image_msg);
                        }
                        app.deliver_steer();
                        app.start_stream_turn(event_tx.clone());
                    }
                }
                StreamEvent::Completed { finish_reason } => {
                    let truncated = corex_core::is_truncated_finish(finish_reason.as_deref());
                    let assistant_text = if app.streaming_text.is_empty() {
                        None
                    } else {
                        Some(app.streaming_text.clone())
                    };

                    let reasoning = if app.thinking_state.content.is_empty() {
                        None
                    } else {
                        Some(app.thinking_state.content.clone())
                    };

                    if let (Some(ref txt), Some(ref cot)) = (&assistant_text, &reasoning) {
                        let key = corex_core::reasoning_cache::ReasoningCache::compute_key(
                            txt,
                            Some(&app.streaming_tool_calls),
                        );
                        app.llm_client.reasoning_cache().insert(key, cot.clone());
                    }

                    let mut valid_calls: Vec<ToolCall> = app.streaming_tool_calls
                        .drain(..)
                        .filter(|c| !c.function.name.trim().is_empty())
                        .collect();

                    if !valid_calls.is_empty() && truncated {
                        // The output hit the token limit: arguments may be cut mid-file or
                        // mid-command, and lenient JSON repair would happily "fix" them. Answer
                        // every call with an error instead of running it, and let the model retry
                        // once in smaller pieces; a second consecutive truncation stops the loop.
                        for call in &mut valid_calls {
                            if call.extra_content.is_none() {
                                call.extra_content = app.streaming_extra_content.clone();
                            }
                        }
                        let already_retried = app.session.messages.iter().rev()
                            .find(|m| m.role == "tool")
                            .and_then(|m| m.text_content())
                            .is_some_and(|t| t == corex_core::TRUNCATED_TOOL_CALL_ERROR);
                        let call_ids: Vec<String> = valid_calls.iter().map(|c| c.id.clone()).collect();
                        app.session.add_message(Message::assistant_with_tools(
                            assistant_text,
                            reasoning,
                            valid_calls,
                        ));
                        for id in call_ids {
                            app.session.add_message(Message::tool_response(
                                id,
                                corex_core::TRUNCATED_TOOL_CALL_ERROR.to_string(),
                            ));
                        }
                        app.streaming_text.clear();
                        app.streaming_extra_content = None;
                        app.invalidate_message_cache();
                        let _ = app.session.save();
                        if already_retried {
                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.set_status("Response truncated twice by the output token limit; tool calls were not executed.");
                        } else {
                            app.start_stream_turn(event_tx.clone());
                        }
                    } else if !valid_calls.is_empty() {
                        for call in &mut valid_calls {
                            if call.extra_content.is_none() {
                                call.extra_content = app.streaming_extra_content.clone();
                            }
                        }
                        let calls = valid_calls;
                        app.session.add_message(Message::assistant_with_tools(
                            assistant_text,
                            reasoning,
                            calls.clone(),
                        ));

                        app.streaming_text.clear();
                        app.streaming_extra_content = None;
                        let _ = app.session.save();

                        // 1. Check if ANY tool in the batch needs user confirmation
                        let mut requires_confirmation = false;
                        let mut previews = Vec::new();

                        let confirmation_context = ToolContext {
                            workspace_dir: app.workspace_dir.clone(),
                            yolo_mode: app.always_allow_tools,
                            sudo_password: get_sudo_password(),
                            allowed_commands: app.get_effective_allowed_commands(),
                        };

                        for call in &calls {
                            let tool = app.tool_registry.get(&call.function.name);
                            let args_json = corex_core::parse_lenient_json(&call.function.arguments)
                                .unwrap_or(serde_json::Value::Null);

                            if let Some(t) = tool {
                                if t.needs_confirmation(&args_json, &confirmation_context)
                                    && !app.always_allow_tools
                                {
                                    requires_confirmation = true;
                                }
                                if call.function.name == "run_shell_command" {
                                    if let Some(cmd) = args_json.get("command").and_then(|c| c.as_str()) {
                                        previews.push(cmd.to_string());
                                    }
                                } else if let Some(diff) = t.format_diff(&args_json, &app.workspace_dir) {
                                    previews.push(diff);
                                } else if call.function.name == "delegate_subagent" {
                                    let role = args_json.get("role").and_then(|r| r.as_str()).unwrap_or("worker");
                                    let prompt = args_json.get("prompt").and_then(|p| p.as_str()).unwrap_or("").trim();
                                    previews.push(format!("Subagent [{}]\n{}", role.to_uppercase(), prompt));
                                } else {
                                    let summary = if let Some(obj) = args_json.as_object() {
                                        obj.iter()
                                            .take(2)
                                            .map(|(k, v)| {
                                                let val_str = match v {
                                                    serde_json::Value::String(s) => s.clone(),
                                                    _ => v.to_string(),
                                                };
                                                let truncated = if val_str.len() > 60 {
                                                    format!("{}...", corex_core::types::safe_truncate_str(&val_str, 60))
                                                } else {
                                                    val_str
                                                };
                                                format!("{}: {}", k, truncated)
                                            })
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    } else {
                                        String::new()
                                    };
                                    if summary.is_empty() {
                                        previews.push(format!("{}(...)", call.function.name));
                                    } else {
                                        previews.push(format!("{}({})", call.function.name, summary));
                                    }
                                }
                            }
                        }

                        // The interactive ask_user dialog takes priority over the
                        // confirmation modal: it must never run as a plain tool.
                        let ask_questions = calls
                            .iter()
                            .find(|c| c.function.name == "ask_user")
                            .and_then(|c| parse_questions(&c.function.arguments));

                        // Detect if any shell command requires sudo escalation
                        let sudo_needed_cmd = calls.iter().find_map(|c| {
                            if c.function.name == "run_shell_command" {
                                let args_json = corex_core::parse_lenient_json(&c.function.arguments).ok()?;
                                let cmd = args_json.get("command")?.as_str()?;
                                if command_requires_sudo(cmd) {
                                    let extracted = extract_first_sudo_command(cmd).unwrap_or_else(|| cmd.to_string());
                                    return Some(extracted);
                                }
                            }
                            None
                        });

                        if let Some((questions, ask_call)) = ask_questions.and_then(|q| {
                            calls.iter().find(|c| c.function.name == "ask_user").map(|c| (q, c))
                        }) {
                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.clear_status();
                            app.user_dialog.open(questions, calls.clone(), ask_call.id.clone());
                        } else if requires_confirmation {
                            // Only the calls that need approval reach the dialog. The rest (a web
                            // search, a read inside the workspace...) run right away, so the user is
                            // never asked about an action that needs no permission.
                            let (gated, free) = split_by_approval(&app, calls, &confirmation_context);
                            let total_count = gated.len();
                            let first_preview = gated.first().map(|c| format_call_preview(c, &app.tool_registry, &app.workspace_dir));

                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.clear_status();
                            if app.fleet_dialog.is_open {
                                app.fleet_dialog.command_mode = false;
                            }
                            if !free.is_empty() {
                                run_tools_in_parallel(&mut app, free, confirmation_context.clone(), event_tx.clone());
                            }
                            app.pending_confirmation = Some(PendingToolBatch {
                                calls: gated,
                                total_count,
                                current_idx: 1,
                                diff_preview: first_preview,
                                selected_option: 0,
                                diff_expanded: false,
                                input_mode: false,
                                feedback_text: String::new(),
                            });
                        } else if let Some(sudo_cmd) = sudo_needed_cmd {
                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.clear_status();
                            app.sudo_dialog.open_batch(calls.clone(), sudo_cmd);
                        } else {
                            // Execute all tools concurrently in parallel without blocking UI!
                            run_tools_in_parallel(&mut app, calls, confirmation_context, event_tx.clone());
                        }
                    } else {
                        // No tool calls — final model answer received!
                        app.is_streaming = false;
                        app.thinking_state.reset();
                        app.clear_status();

                        if let Some(mut txt) = assistant_text {
                            let user_cjk = corex_core::language::user_has_requested_cjk(&app.session.messages);
                            if corex_core::language::is_unwanted_cjk_drift(&txt, user_cjk) {
                                txt = corex_core::language::clean_cjk_drift_from_text(&txt);
                            }
                            let mut assistant_msg = Message::assistant(txt, reasoning);
                            assistant_msg.extra_content = app.streaming_extra_content.take();
                            app.session.add_message(assistant_msg);
                        }

                        app.streaming_text.clear();
                        app.streaming_tool_calls.clear();
                        app.streaming_extra_content = None;
                        let _ = app.session.save();
                        corex_core::trim_memory();

                        // ⚡ Automatically dequeue and run next queued user prompt or synthesize finished task!
                        // A "send now" message goes first: it was meant to be read before the queue.
                        if app.deliver_steer() {
                            app.scroll_to_bottom();
                            app.start_stream_turn(event_tx.clone());
                        } else if let Some(next_prompt) = app.message_queue.pop_front() {
                            corex_core::reset_repeat_guard();
                    let message = app.user_message(next_prompt);
                    app.session.add_message(message);
                            app.scroll_to_bottom();
                            app.start_stream_turn(event_tx.clone());
                        } else if let Some(last_msg) = app.session.messages.last() {
                            if last_msg.role.as_str() == "system"
                                && last_msg.text_content().map(|t| t.starts_with("TASK_DONE|")).unwrap_or(false)
                            {
                                app.start_stream_turn(event_tx.clone());
                            }
                        }
                    }
                }
                StreamEvent::ContextCompacted { compacted_messages, notice } => {
                    app.session.messages = compacted_messages;
                    app.session.add_message(Message::system(notice));
                    app.invalidate_message_cache();
                    let _ = app.session.save();
                    corex_core::trim_memory();
                }
                StreamEvent::Notice(notice) => {
                    app.session.add_message(Message::system(notice));
                    app.invalidate_message_cache();
                }
                StreamEvent::Error(err) => {
                    app.is_streaming = false;
                    app.in_flight_tool_calls = 0;
                    app.thinking_state.reset();
                    app.clear_status();
                    app.session.add_message(Message::system(format!("Error: {}", err)));
                    app.streaming_text.clear();
                    app.streaming_tool_calls.clear();
                    corex_core::trim_memory();
                    if let Some(next_prompt) = app.message_queue.pop_front() {
                        if app.input_buffer.is_empty() {
                            app.input_buffer = next_prompt;
                            app.cursor_idx = app.input_buffer.len();
                        }
                    }
                }
            }
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
            match event::read()? {
                Event::Paste(pasted) => {
                    let looks_sensitive = app.sudo_dialog.is_open
                        || app.auth_dialog.is_open
                        || app.user_dialog.is_open
                        || app.fleet_dialog.is_open
                        || app.input_buffer.trim_start().starts_with("/key")
                        || app.input_buffer.trim_start().starts_with("/sudo")
                        || pasted.contains("/key")
                        || pasted.contains("sk-")
                        || pasted.contains("API_KEY");
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
                    needs_redraw = true;
                }
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
                Event::Mouse(mouse) => {
                    if app.fleet_dialog.is_open {
                        let (w, h) = terminal.size().map(|s| (s.width, s.height)).unwrap_or((80, 24));
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

                        if matches!(mouse.kind, crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)) {
                            // Check for direct click on VPS tabs in the top header row
                            if is_on_server && mouse.row == 0 && !app.fleet_dialog.items.is_empty() {
                                let start_col = if is_horizontal { split_col + 5 } else { 5 };
                                if mouse.column >= start_col {
                                    let mut curr_x = start_col;
                                    let mut clicked_tab = None;
                                    for (i, item) in app.fleet_dialog.items.iter().enumerate() {
                                        // Tab pattern: " [i] " + id + " ● "
                                        let tab_w = 4 + item.config.id.chars().count() as u16 + 3;
                                        if mouse.column >= curr_x && mouse.column < curr_x + tab_w {
                                            clicked_tab = Some(i);
                                            break;
                                        }
                                        curr_x += tab_w;
                                    }
                                    if let Some(idx) = clicked_tab {
                                        if idx < app.fleet_dialog.items.len() {
                                            app.fleet_dialog.selected_idx = idx;
                                            fleet_ensure_session(&mut app).await;
                                            app.fleet_dialog.command_mode = true;
                                            needs_redraw = true;
                                            continue;
                                        }
                                    }
                                }
                            }

                            if is_on_server {
                                if !app.fleet_dialog.command_mode {
                                    app.fleet_dialog.command_mode = true;
                                    needs_redraw = true;
                                }
                            } else {
                                if app.fleet_dialog.command_mode {
                                    app.fleet_dialog.command_mode = false;
                                    needs_redraw = true;
                                }
                            }
                        } else if is_on_server {
                            match mouse.kind {
                                crossterm::event::MouseEventKind::ScrollUp => {
                                    if let Some(ref pty) = app.fleet_dialog.pty_session {
                                        pty.scroll_up(3);
                                        app.fleet_dialog.terminal_scroll = pty.scrollback();
                                    } else {
                                        let total = app.fleet_dialog.total_output_lines();
                                        let view_h = h.saturating_sub(8) as usize;
                                        app.fleet_dialog.scroll_up(3, total, view_h);
                                    }
                                    needs_redraw = true;
                                    continue;
                                }
                                crossterm::event::MouseEventKind::ScrollDown => {
                                    if let Some(ref pty) = app.fleet_dialog.pty_session {
                                        pty.scroll_down(3);
                                        app.fleet_dialog.terminal_scroll = pty.scrollback();
                                    } else {
                                        app.fleet_dialog.scroll_down(3);
                                    }
                                    needs_redraw = true;
                                    continue;
                                }
                                _ => {}
                            }
                        }
                    }
                    app.handle_mouse(mouse);
                    if is_android_or_termux() && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        app.tap_keyboard.on_tap(Instant::now(), &mut app.mouse_capture);
                    }
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                    let is_alt = key.modifiers.contains(KeyModifiers::ALT);
                    let is_ctrl_p = (is_ctrl && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P'))) || key.code == KeyCode::Char('\x10');
                    let is_ctrl_n = (is_ctrl && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N'))) || key.code == KeyCode::Char('\x0e');

                    // --- Force Quit: Ctrl+Z ---
                    if is_ctrl && matches!(key.code, KeyCode::Char('z') | KeyCode::Char('Z') | KeyCode::Char('\x1a')) {
                        break;
                    }

                    // --- Mouse reporting toggle: F2 ---
                    // Reclaims native click-drag selection/copy on demand without giving up
                    // wheel scrolling (which stays available via PageUp/PageDown when OFF).
                    if key.code == KeyCode::F(2) {
                        app.toggle_mouse_capture();
                        needs_redraw = true;
                        continue;
                    }

                    // Esc drops an active selection without consuming the key, so it still
                    // cancels streaming or closes a dialog further down this handler.
                    if key.code == KeyCode::Esc && app.selection.is_some() {
                        app.clear_selection();
                        needs_redraw = true;
                    }

                    // --- 0. Cancel active streaming / generation immediately on Esc or Ctrl+C ---
                    if app.is_streaming
                        && (key.code == KeyCode::Esc
                            || (key.code == KeyCode::Char('c') && is_ctrl))
                        {
                            // Esc first takes back the newest pending message, one per press; the
                            // generation is only cancelled once nothing is left to take back.
                            if key.code == KeyCode::Esc && app.has_pending_messages() {
                                if let Some(pending) = app.undo_last_pending() {
                                    app.restore_to_input(vec![pending]);
                                }
                                needs_redraw = true;
                                continue;
                            }

                            // Cancelling must not eat what the user typed: whatever never reached
                            // the model goes back to the composer.
                            let undelivered = app.take_all_pending();
                            app.cancel_generation("Generation cancelled by user (Esc).");
                            app.restore_to_input(undelivered);
                            continue;
                        }

                    if app.is_checking_balance && key.code == KeyCode::Esc {
                        app.is_checking_balance = false;
                        app.clear_status();
                        if let Some(idx) = app.pending_balance_msg_index.take() {
                            if idx < app.session.messages.len() {
                                app.session.messages[idx] = Message::system("Balance check cancelled.");
                                app.invalidate_message_cache();
                            }
                        }
                        needs_redraw = true;
                        continue;
                    }

                    // --- 1. Sudo Password Dialog Active ---
                    if app.sudo_dialog.is_open {
                        match key.code {
                            KeyCode::Esc => {
                                let calls = if !app.sudo_dialog.pending_calls.is_empty() {
                                    std::mem::take(&mut app.sudo_dialog.pending_calls)
                                } else if let Some(call) = app.sudo_dialog.pending_call.take() {
                                    vec![call]
                                } else {
                                    Vec::new()
                                };
                                for call in calls {
                                    app.session.add_message(Message::tool_response(
                                        call.id,
                                        "Sudo authentication cancelled by user.",
                                    ));
                                }
                                app.sudo_dialog.close();
                                if app.pending_confirmation.is_none() && app.in_flight_tool_calls == 0 {
                                    app.start_stream_turn(event_tx.clone());
                                }
                            }
                            KeyCode::Backspace => {
                                app.sudo_dialog.password_input.pop();
                            }
                            KeyCode::Char(c) => {
                                app.sudo_dialog.password_input.push(c);
                            }
                            KeyCode::Enter => {
                                let pwd = app.sudo_dialog.password_input.trim().to_string();
                                if pwd.is_empty() {
                                    app.sudo_dialog.error_msg = Some("Password cannot be empty.".to_string());
                                } else {
                                    // Sudo password is not persisted across commands
                                    set_sudo_password(None);
                                    let calls = if !app.sudo_dialog.pending_calls.is_empty() {
                                        std::mem::take(&mut app.sudo_dialog.pending_calls)
                                    } else if let Some(call) = app.sudo_dialog.pending_call.take() {
                                        vec![call]
                                    } else {
                                        Vec::new()
                                    };
                                    if !calls.is_empty() {
                                        app.in_flight_tool_calls += calls.len();
                                        let tx = event_tx.clone();
                                        let registry = app.tool_registry.clone();
                                        let gen = app.current_generation_id;
                                        let context = ToolContext {
                                            workspace_dir: app.workspace_dir.clone(),
                                            yolo_mode: app.always_allow_tools,
                                            sudo_password: Some(pwd),
                                            allowed_commands: app.get_effective_allowed_commands(),
                                        };
                                        app.is_streaming = true;
                                        tokio::spawn(async move {
                                            let calls_count = calls.len();
                                            if calls_count > 1 {
                                                let summary = if calls.iter().all(|c| c.function.name == "run_shell_command" || c.function.name == "execute_command") {
                                                    format!("Running {} commands in parallel...", calls_count)
                                                } else {
                                                    format!("Running {} tasks in parallel...", calls_count)
                                                };
                                                let _ = tx.send((gen, StreamEvent::ToolExecutionStarting {
                                                    call_id: "batch".to_string(),
                                                    name: "batch".to_string(),
                                                    summary,
                                                })).await;
                                            }

                                            let mut handles = Vec::new();
                                            for call in calls {
                                                let reg = registry.clone();
                                                let ctx = context.clone();
                                                let tx_call = tx.clone();
                                                let call_id = call.id.clone();
                                                let tool_name = call.function.name.clone();
                                                let args_str = call.function.arguments.clone();

                                                handles.push(tokio::spawn(async move {
                                                    if calls_count == 1 {
                                                        let summary = format_tool_call_summary(&tool_name, &args_str);
                                                        let _ = tx_call.send((gen, StreamEvent::ToolExecutionStarting {
                                                            call_id: call_id.clone(),
                                                            name: tool_name.clone(),
                                                            summary,
                                                        })).await;
                                                    }
                                                    let output = match corex_core::parse_lenient_json(&args_str) {
                                                        Ok(args_json) => match reg.execute(&tool_name, args_json, &ctx).await {
                                                            Ok(o) => o.output,
                                                            Err(e) => format!("Execution error: {}", e),
                                                        },
                                                        Err(e) => format!("Execution error: invalid tool arguments JSON: {}", e),
                                                    };
                                                    let _ = tx_call.send((gen, StreamEvent::ToolExecutionDone { call_id, output })).await;
                                                }));
                                            }
                                            for h in handles {
                                                let _ = h.await;
                                            }
                                            let _ = tx.send((gen, StreamEvent::AllToolsDone)).await;
                                        });
                                    }
                                    app.sudo_dialog.close();
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- 1. First-Run API Key Auth Dialog Active ---
                    if app.auth_dialog.is_open {
                        match key.code {
                            KeyCode::Esc => {
                                app.auth_dialog.close();
                                app.session.add_message(Message::system(
                                    "Auth dialog cancelled. You can set keys anytime via /model, /key, or run offline via /local."
                                ));
                            }
                            KeyCode::Backspace => {
                                app.auth_dialog.input_buffer.pop();
                            }
                            KeyCode::Char(c) => {
                                app.auth_dialog.input_buffer.push(c);
                            }
                            KeyCode::Enter => {
                                let key_str = app.auth_dialog.input_buffer.trim().to_string();
                                if key_str.is_empty() {
                                    app.auth_dialog.error_msg = Some("API key cannot be empty.".to_string());
                                } else {
                                    let env_var = app.auth_dialog.api_key_env.clone();
                                    let prov_name = app.auth_dialog.provider_name.clone();
                                    let prov_label = app.auth_dialog.provider_label.clone();
                                    let pending = app.auth_dialog.pending_activation;

                                    let mut cfg = app.llm_client.get_config();
                                    if prov_name == "deepseek" {
                                        cfg.api_key = key_str.clone();
                                    } else {
                                        cfg.provider_api_keys.insert(prov_name.clone(), key_str.clone());
                                    }
                                    let _ = cfg.save_with_workspace(Some(&app.workspace_dir));
                                    app.llm_client.update_config(cfg);
                                    corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(app.llm_client.clone()));
                                    if let Some(pos) = app.model_dialog.providers.iter().position(|p| p.name == prov_name) {
                                        if pos < app.model_dialog.ready.len() {
                                            app.model_dialog.ready[pos] = true;
                                        }
                                    }
                                    app.auth_dialog.close();
                                    let note = crate::auth_dialog::env_override_note(&prov_name, &key_str).unwrap_or_default();
                                    app.session.add_message(Message::system(format!(
                                        "API key saved for {} (${})!{}",
                                        prov_label, env_var, note
                                    )));

                                    if let Some((p_idx, m_idx)) = pending {
                                        app.activate_model(p_idx, m_idx);
                                    }
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- 1.5. Session Dialog Active ---
                    if app.session_dialog.is_open {
                        let filtered_count = app.session_dialog.filtered_sessions().len();

                        // A delete is waiting for its answer: `y` confirms, any other key backs out.
                        if app.session_dialog.pending_delete.is_some() {
                            if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                                if let Some(session_id) = app.session_dialog.take_pending_delete() {
                                    let _ = Session::delete_by_id_or_tag(&session_id);
                                    app.session_dialog.reload(&app.workspace_dir.display().to_string());
                                }
                            } else {
                                app.session_dialog.pending_delete = None;
                            }
                            continue;
                        }

                        match key.code {
                            KeyCode::Esc => {
                                if !app.session_dialog.search_query.is_empty() {
                                    app.session_dialog.search_query.clear();
                                    app.session_dialog.selected_idx = 0;
                                } else {
                                    app.session_dialog.close();
                                }
                            }
                            KeyCode::Backspace => {
                                app.session_dialog.search_query.pop();
                                app.session_dialog.selected_idx = 0;
                            }
                            KeyCode::Up => {
                                if app.session_dialog.selected_idx > 0 {
                                    app.session_dialog.selected_idx -= 1;
                                }
                            }
                            KeyCode::Down => {
                                if filtered_count > 0 && app.session_dialog.selected_idx + 1 < filtered_count {
                                    app.session_dialog.selected_idx += 1;
                                }
                            }
                            // Delete only through keys that cannot be typed into the filter: a plain
                            // letter belongs to the search box, and the removal asks to be confirmed.
                            KeyCode::Delete => {
                                app.session_dialog.request_delete();
                            }
                            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.session_dialog.request_delete();
                            }
                            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                                app.session_dialog.search_query.push(c);
                                app.session_dialog.selected_idx = 0;
                            }
                            KeyCode::Enter => {
                                if let Some(selected_session) = app.session_dialog.selected_session().cloned() {
                                    let session_id = selected_session.id.clone();
                                    match Session::load_by_id_or_tag(&session_id) {
                                        Ok(mut loaded) => {
                                            let mut cfg = app.llm_client.get_config();
                                            cfg.model = loaded.model.clone();
                                            if let Some(t) = loaded.temperature {
                                                cfg.temperature = t;
                                            }
                                            if let Some(ref r) = loaded.reasoning_effort {
                                                cfg.reasoning_effort = r.clone();
                                            }
                                            app.llm_client.update_config(cfg);
                                            corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(app.llm_client.clone()));
                                            let is_fleet = app.session_dialog.is_fleet;
                                            if is_fleet {
                                                loaded.tag = Some("fleet".to_string());
                                            }
                                            app.session = loaded;
                                            app.cached_session_id.clear();
                                            app.invalidate_message_cache();
                                            app.clear_selection();
                                            app.scroll_to_bottom();
                                            let label = if is_fleet { "Fleet session" } else { "session" };
                                            app.session.add_message(Message::system(format!(
                                                "Resumed {} '{}' (Model: {}).",
                                                label, app.session.title, app.session.model
                                            )));
                                        }
                                        Err(e) => {
                                            app.session.add_message(Message::system(format!("Failed to load session: {}", e)));
                                        }
                                    }
                                }
                                app.session_dialog.close();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- 1.6. Fleet Operations Dialog Active ---
                    // --- 1.6. Fleet Operations & SysAdmin Console Active ---
                    let mut fleet_chat_passthrough = false;
                    if app.fleet_dialog.is_open && app.pending_confirmation.is_none() {
                        // A. Handling when there is a pending AI Remote Command Approval
                        if let Some(mut pending) = app.fleet_dialog.pending_approval.take() {
                            if pending.edit_mode {
                                match key.code {
                                    KeyCode::Esc => {
                                        pending.edit_mode = false;
                                        app.fleet_dialog.pending_approval = Some(pending);
                                    }
                                    KeyCode::Enter => {
                                        let edited = pending.edited_command.trim().to_string();
                                        let call_id = pending.call_id.clone();
                                        let node_id = pending.node_id.clone();
                                        let sudo = pending.sudo;
                                        let mut session = match app.fleet_dialog.active_session.clone() {
                                            Some(s) if s.node.id == node_id => s,
                                            _ => {
                                                if let Some(node) = app.fleet_dialog.items.iter().find(|i| i.config.id == node_id).map(|i| i.config.clone()) {
                                                    let mut s = corex_fleet::RemoteSession::new(node);
                                                    let _ = s.connect(8).await;
                                                    s
                                                } else {
                                                    let mut tool_msg = Message::tool_response(call_id, format!("Error: Node {} not found", node_id));
                                                    tool_msg.name = Some("fleet_run_command".to_string());
                                                    app.session.add_message(tool_msg);
                                                    continue;
                                                }
                                            }
                                        };

                                        match session.execute_command(&edited, corex_fleet::CommandInitiator::AI, sudo, 15).await {
                                            Ok(rec) => {
                                                corex_fleet::global_session_manager().set_session(node_id.clone(), session.clone()).await;
                                                app.fleet_dialog.active_session = Some(session);
                                                if let Some(ref pty) = app.fleet_dialog.pty_session {
                                                    pty.feed_text(&format!("\r\n\x1b[1;33m› [corex] $ {}\x1b[0m\r\n", edited));
                                                    let clean = rec.stdout.replace("\r\n", "\n").replace('\n', "\r\n");
                                                    let trimmed = clean.trim_end();
                                                    if !trimmed.is_empty() {
                                                        pty.feed_text(&format!("{}\r\n", trimmed));
                                                    }
                                                    let clean_err = rec.stderr.replace("\r\n", "\n").replace('\n', "\r\n");
                                                    let trimmed_err = clean_err.trim_end();
                                                    if !trimmed_err.is_empty() {
                                                        pty.feed_text(&format!("\x1b[31m{}\x1b[0m\r\n", trimmed_err));
                                                    }
                                                }
                                                let output = if rec.stderr.trim().is_empty() {
                                                    format!("[{}] (edited by SysAdmin) Exit code: {} | CWD: {}\n{}", node_id, rec.exit_code, rec.cwd_after, rec.stdout)
                                                } else {
                                                    format!("[{}] (edited by SysAdmin) Exit code: {} | CWD: {}\n{}\n--- STDERR ---\n{}", node_id, rec.exit_code, rec.cwd_after, rec.stdout, rec.stderr)
                                                };
                                                let mut tool_msg = Message::tool_response(call_id, output);
                                                tool_msg.name = Some("fleet_run_command".to_string());
                                                app.session.add_message(tool_msg);
                                                let _ = app.session.save();
                                            }
                                            Err(e) => {
                                                if let Some(ref pty) = app.fleet_dialog.pty_session {
                                                    pty.feed_text(&format!("\r\n\x1b[1;31m! Error: {}\x1b[0m\r\n", e));
                                                }
                                                let mut tool_msg = Message::tool_response(call_id, format!("Remote execution error: {}", e));
                                                tool_msg.name = Some("fleet_run_command".to_string());
                                                app.session.add_message(tool_msg);
                                            }
                                        }
                                        app.start_stream_turn(event_tx.clone());
                                    }
                                    KeyCode::Backspace => {
                                        pending.edited_command.pop();
                                        app.fleet_dialog.pending_approval = Some(pending);
                                    }
                                    KeyCode::Char(c) => {
                                        pending.edited_command.push(c);
                                        app.fleet_dialog.pending_approval = Some(pending);
                                    }
                                    _ => {
                                        app.fleet_dialog.pending_approval = Some(pending);
                                    }
                                }
                                needs_redraw = true;
                                continue;
                            } else if pending.instructions_mode {
                                match key.code {
                                    KeyCode::Esc => {
                                        pending.instructions_mode = false;
                                        app.fleet_dialog.pending_approval = Some(pending);
                                    }
                                    KeyCode::Enter => {
                                        let text = pending.instructions_text.trim().to_string();
                                        let call_id = pending.call_id.clone();
                                        if let Some(ref pty) = app.fleet_dialog.pty_session {
                                            pty.feed_text(&format!("\r\n\x1b[1;35m! Instructions: {}\x1b[0m\r\n", text));
                                        }
                                        let mut tool_msg = Message::tool_response(
                                            call_id,
                                            format!("Command denied by SysAdmin with instructions: {}", text)
                                        );
                                        tool_msg.name = Some("fleet_run_command".to_string());
                                        app.session.add_message(tool_msg);
                                        let _ = app.session.save();
                                        app.start_stream_turn(event_tx.clone());
                                    }
                                    KeyCode::Backspace => {
                                        pending.instructions_text.pop();
                                        app.fleet_dialog.pending_approval = Some(pending);
                                    }
                                    KeyCode::Char(c) => {
                                        pending.instructions_text.push(c);
                                        app.fleet_dialog.pending_approval = Some(pending);
                                    }
                                    _ => {
                                        app.fleet_dialog.pending_approval = Some(pending);
                                    }
                                }
                                needs_redraw = true;
                                continue;
                            } else {
                                // Direct approval actions: [y] Allow, [e] Edit, [w] Instructions, [n] Deny
                                match key.code {
                                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                                        let cmd = pending.command.clone();
                                        let call_id = pending.call_id.clone();
                                        let node_id = pending.node_id.clone();
                                        let sudo = pending.sudo;

                                        let mut session = match app.fleet_dialog.active_session.clone() {
                                            Some(s) if s.node.id == node_id => s,
                                            _ => {
                                                if let Some(node) = app.fleet_dialog.items.iter().find(|i| i.config.id == node_id).map(|i| i.config.clone()) {
                                                    let mut s = corex_fleet::RemoteSession::new(node);
                                                    let _ = s.connect(8).await;
                                                    s
                                                } else {
                                                    let mut tool_msg = Message::tool_response(call_id, format!("Error: Node {} not found", node_id));
                                                    tool_msg.name = Some("fleet_run_command".to_string());
                                                    app.session.add_message(tool_msg);
                                                    continue;
                                                }
                                            }
                                        };

                                        match session.execute_command(&cmd, corex_fleet::CommandInitiator::AI, sudo, 15).await {
                                            Ok(rec) => {
                                                corex_fleet::global_session_manager().set_session(node_id.clone(), session.clone()).await;
                                                app.fleet_dialog.active_session = Some(session);
                                                if let Some(ref pty) = app.fleet_dialog.pty_session {
                                                    pty.feed_text(&format!("\r\n\x1b[1;33m› [corex] $ {}\x1b[0m\r\n", cmd));
                                                    let clean = rec.stdout.replace("\r\n", "\n").replace('\n', "\r\n");
                                                    let trimmed = clean.trim_end();
                                                    if !trimmed.is_empty() {
                                                        pty.feed_text(&format!("{}\r\n", trimmed));
                                                    }
                                                    let clean_err = rec.stderr.replace("\r\n", "\n").replace('\n', "\r\n");
                                                    let trimmed_err = clean_err.trim_end();
                                                    if !trimmed_err.is_empty() {
                                                        pty.feed_text(&format!("\x1b[31m{}\x1b[0m\r\n", trimmed_err));
                                                    }
                                                }
                                                let output = if rec.stderr.trim().is_empty() {
                                                    format!("[{}] Exit code: {} | CWD: {}\n{}", node_id, rec.exit_code, rec.cwd_after, rec.stdout)
                                                } else {
                                                    format!("[{}] Exit code: {} | CWD: {}\n{}\n--- STDERR ---\n{}", node_id, rec.exit_code, rec.cwd_after, rec.stdout, rec.stderr)
                                                };
                                                let mut tool_msg = Message::tool_response(call_id, output);
                                                tool_msg.name = Some("fleet_run_command".to_string());
                                                app.session.add_message(tool_msg);
                                                let _ = app.session.save();
                                            }
                                            Err(e) => {
                                                if let Some(ref pty) = app.fleet_dialog.pty_session {
                                                    pty.feed_text(&format!("\r\n\x1b[1;31m! Error: {}\x1b[0m\r\n", e));
                                                }
                                                let mut tool_msg = Message::tool_response(call_id, format!("Remote execution error: {}", e));
                                                tool_msg.name = Some("fleet_run_command".to_string());
                                                app.session.add_message(tool_msg);
                                            }
                                        }
                                        app.start_stream_turn(event_tx.clone());
                                        needs_redraw = true;
                                        continue;
                                    }
                                    KeyCode::Char('e') | KeyCode::Char('E') => {
                                        pending.edit_mode = true;
                                        pending.edited_command = pending.command.clone();
                                        app.fleet_dialog.pending_approval = Some(pending);
                                        needs_redraw = true;
                                        continue;
                                    }
                                    KeyCode::Char('w') | KeyCode::Char('W') => {
                                        pending.instructions_mode = true;
                                        pending.instructions_text.clear();
                                        app.fleet_dialog.pending_approval = Some(pending);
                                        needs_redraw = true;
                                        continue;
                                    }
                                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                        let call_id = pending.call_id.clone();
                                        if let Some(ref pty) = app.fleet_dialog.pty_session {
                                            pty.feed_text(&format!("\r\n\x1b[1;31m✕ [declined] $ {}\x1b[0m\r\n", pending.command));
                                        }
                                        let mut tool_msg = Message::tool_response(call_id, "Remote command execution denied by SysAdmin.".to_string());
                                        tool_msg.name = Some("fleet_run_command".to_string());
                                        app.session.add_message(tool_msg);
                                        let _ = app.session.save();
                                        app.start_stream_turn(event_tx.clone());
                                        needs_redraw = true;
                                        continue;
                                    }
                                    _ => {
                                        app.fleet_dialog.pending_approval = Some(pending);
                                        needs_redraw = true;
                                        continue;
                                    }
                                }
                            }
                        }

                        // B. Split view: chat (left) and VPS terminal (right).
                        //    Tab swaps keyboard focus; F1-F9 / Alt+1-9 pick a server; Ctrl+R probes metrics.
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
                                fleet_ensure_session(&mut app).await;
                                needs_redraw = true;
                                continue;
                            }
                        } else if next_vps {
                            if !app.fleet_dialog.items.is_empty() {
                                app.fleet_dialog.select_next();
                                fleet_ensure_session(&mut app).await;
                                needs_redraw = true;
                                continue;
                            }
                        } else if prev_vps {
                            if !app.fleet_dialog.items.is_empty() {
                                app.fleet_dialog.select_prev();
                                fleet_ensure_session(&mut app).await;
                                needs_redraw = true;
                                continue;
                            }
                        } else if open_selector {
                            app.fleet_dialog.active_session = None;
                            app.fleet_dialog.command_mode = true;
                            needs_redraw = true;
                            continue;
                        } else if key.code == KeyCode::Char('r') && ctrl {
                            app.fleet_dialog.status_msg = Some("Probing telemetry...".to_string());
                            let nodes: Vec<corex_fleet::NodeConfig> = app.fleet_dialog.items.iter().map(|i| i.config.clone()).collect();
                            let tx = app.fleet_dialog.telemetry_tx.clone();
                            for n in nodes {
                                let tx_clone = tx.clone();
                                tokio::spawn(async move {
                                    let id = n.id.clone();
                                    let t = corex_fleet::TelemetryCollector::fetch(&n, 4).await;
                                    let _ = tx_clone.send((id, t));
                                });
                            }
                        } else if is_pane_toggle {
                            app.fleet_dialog.command_mode = !term_focus;
                            needs_redraw = true;
                            continue;
                        } else if term_focus {
                            let is_connected = app.fleet_dialog.is_selected_connected();
                            if !is_connected {
                                // Server Selector mode: Up/Down/Tab to navigate, Enter to launch session, digits 1-9 direct select, typing filters.
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
                                        needs_redraw = true;
                                        continue;
                                    }
                                    KeyCode::Backspace => {
                                        app.fleet_dialog.search_query.pop();
                                        let filtered = app.fleet_dialog.filtered_items();
                                        if !filtered.iter().any(|(idx, _)| *idx == app.fleet_dialog.selected_idx) {
                                            if let Some((first, _)) = filtered.first() {
                                                app.fleet_dialog.selected_idx = *first;
                                            }
                                        }
                                        needs_redraw = true;
                                        continue;
                                    }
                                    KeyCode::Tab => {
                                        app.fleet_dialog.select_next();
                                        needs_redraw = true;
                                        continue;
                                    }
                                    KeyCode::BackTab => {
                                        app.fleet_dialog.select_prev();
                                        needs_redraw = true;
                                        continue;
                                    }
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
                                            fleet_ensure_session(&mut app).await;
                                        }
                                    }
                                    KeyCode::Char(c) if !ctrl && !alt => {
                                        app.fleet_dialog.search_query.push(c);
                                        let filtered = app.fleet_dialog.filtered_items();
                                        if !filtered.iter().any(|(idx, _)| *idx == app.fleet_dialog.selected_idx) {
                                            if let Some((first, _)) = filtered.first() {
                                                app.fleet_dialog.selected_idx = *first;
                                            }
                                        }
                                        needs_redraw = true;
                                        continue;
                                    }
                                    KeyCode::Enter => {
                                        fleet_ensure_session(&mut app).await;
                                        needs_redraw = true;
                                        continue;
                                    }
                                    _ => {}
                                }
                            } else {
                                // Active Terminal mode: if native PTY is active, send all keys directly to remote bash!
                                if let Some(ref pty) = app.fleet_dialog.pty_session {
                                    if !pty.is_alive() {
                                        app.fleet_dialog.pty_session = None;
                                        app.fleet_dialog.active_session = None;
                                        needs_redraw = true;
                                        continue;
                                    }

                                    // Keyboard terminal scrolling with PageUp / PageDown:
                                    if key.code == KeyCode::PageUp {
                                        pty.scroll_up(15);
                                        app.fleet_dialog.terminal_scroll = pty.scrollback();
                                        needs_redraw = true;
                                        continue;
                                    } else if key.code == KeyCode::PageDown {
                                        pty.scroll_down(15);
                                        app.fleet_dialog.terminal_scroll = pty.scrollback();
                                        needs_redraw = true;
                                        continue;
                                    } else if pty.scrollback() > 0 && matches!(key.code, KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace) {
                                        pty.scroll_reset();
                                        app.fleet_dialog.terminal_scroll = 0;
                                    }

                                    if let Some(bytes) = crate::fleet_pty::key_event_to_bytes(key) {
                                        pty.write_bytes(&bytes);
                                    }
                                    needs_redraw = true;
                                    continue;
                                }

                                // Fallback simulated terminal mode: command input, remote completion and scrolling
                                match key.code {
                                    KeyCode::Tab => {
                                        let input = app.fleet_dialog.command_input.clone();
                                        if !input.trim().is_empty() {
                                            if let Some(sess) = fleet_ensure_session(&mut app).await {
                                                app.fleet_dialog.status_msg = Some("…".to_string());
                                                terminal.draw(|f| {
                                                    render_ui(f, &mut app);
                                                })?;
                                                let result = sess.complete_line(&input).await;
                                                if result.candidates.is_empty() {
                                                    app.fleet_dialog.status_msg = Some("no completions".to_string());
                                                } else if result.candidates.len() == 1 {
                                                    app.fleet_dialog.command_input = result.new_input;
                                                    app.fleet_dialog.status_msg = None;
                                                    app.fleet_dialog.last_tab_input = None;
                                                } else {
                                                    if result.advanced {
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
                                                            app.fleet_dialog.last_tab_input = Some(input.clone());
                                                            app.fleet_dialog.status_msg = None;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
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
                                    KeyCode::Enter => {
                                        app.fleet_dialog.last_tab_input = None;
                                        let cmd = app.fleet_dialog.command_input.trim().to_string();
                                        if !cmd.is_empty() {
                                            if let Some(mut sess) = fleet_ensure_session(&mut app).await {
                                                app.fleet_dialog.command_input.clear();
                                                app.fleet_dialog.terminal_scroll = 0;
                                                if cmd == "clear" || cmd == "cls" {
                                                    sess.clear_history();
                                                    corex_fleet::global_session_manager().set_session(sess.node.id.clone(), sess.clone()).await;
                                                    app.fleet_dialog.active_session = Some(sess);
                                                    app.fleet_dialog.status_msg = None;
                                                } else {
                                                    let first_word = cmd.split_whitespace().next().unwrap_or("");
                                                    if matches!(first_word, "top" | "htop" | "btop" | "vim" | "vi" | "nano" | "emacs" | "less" | "more" | "man") {
                                                        app.fleet_dialog.status_msg = Some(format!("⚠️ '{}' requires an interactive PTY (PTY unavailable on this host)", first_word));
                                                        needs_redraw = true;
                                                        continue;
                                                    }
                                                    app.fleet_dialog.status_msg = Some(format!("$ {}", cmd));
                                                    let tx = app.fleet_dialog.cmd_done_tx.clone();
                                                    let nid = sess.node.id.clone();
                                                    tokio::spawn(async move {
                                                        let _ = sess.execute_command(&cmd, corex_fleet::CommandInitiator::User, false, 20).await;
                                                        let _ = tx.send((nid, sess));
                                                    });
                                                }
                                            }
                                        }
                                    }
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
                        } else {
                            // Chat focus: fall through to the normal chat key handling below.
                            // (Chat PageUp/PageDown scroll the conversation; terminal scroll needs Tab.)
                            fleet_chat_passthrough = true;
                        }

                        if !fleet_chat_passthrough {
                            needs_redraw = true;
                            continue;
                        }
                    }

                    // --- 2. Model Dialog Active ---
                    if app.model_dialog.is_open {
                        let action = app.model_dialog.handle_key(key.code);
                        app.apply_model_dialog_action(action);
                        continue;
                    }

                    // --- 3. Tool Confirmation Modal Active (Modern CLI 5-Option Radio Selection) ---
                    if let Some(ref mut pending) = app.pending_confirmation {
                        let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        let is_ctrl_o = (is_ctrl && matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O')))
                            || key.code == KeyCode::Char('\x0f');

                        if is_ctrl_o {
                            pending.diff_expanded = !pending.diff_expanded;
                            app.logs_expanded = pending.diff_expanded;
                            app.invalidate_message_cache();
                            app.auto_scroll = true;
                            needs_redraw = true;
                            continue;
                        }

                        if pending.input_mode {
                            match key.code {
                                KeyCode::Esc => {
                                    pending.input_mode = false;
                                    pending.feedback_text.clear();
                                    needs_redraw = true;
                                    continue;
                                }
                                KeyCode::Backspace => {
                                    pending.feedback_text.pop();
                                    needs_redraw = true;
                                    continue;
                                }
                                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !c.is_control() => {
                                    pending.feedback_text.push(c);
                                    needs_redraw = true;
                                    continue;
                                }
                                KeyCode::Enter => {
                                    let feedback = pending.feedback_text.trim().to_string();
                                    let Some(pending_batch) = app.pending_confirmation.take() else {
                                        continue;
                                    };
                                    let reason = if feedback.is_empty() {
                                        "Tool execution denied by user.".to_string()
                                    } else {
                                        format!("Tool execution denied by user with instruction: {}", feedback)
                                    };
                                    for call in pending_batch.calls {
                                        app.session.add_message(Message::tool_response(
                                            call.id,
                                            reason.clone(),
                                        ));
                                    }
                                    if app.in_flight_tool_calls == 0 && !app.sudo_dialog.is_open {
                                        app.start_stream_turn(event_tx.clone());
                                    }
                                    continue;
                                }
                                _ => {
                                    continue;
                                }
                            }
                        }

                        match key.code {
                            KeyCode::Up => {
                                pending.selected_option = pending.selected_option.saturating_sub(1);
                                needs_redraw = true;
                                continue;
                            }
                            KeyCode::Down => {
                                pending.selected_option = (pending.selected_option + 1).min(4);
                                needs_redraw = true;
                                continue;
                            }
                            _ => {}
                        }

                        let is_multi = pending.total_count > 1;

                        let should_allow_single = key.code == KeyCode::Char('1')
                            || key.code == KeyCode::Char('y')
                            || key.code == KeyCode::Char('Y')
                            || (key.code == KeyCode::Enter && pending.selected_option == 0);

                        let should_allow_all = is_multi && (
                            key.code == KeyCode::Char('2')
                            || key.code == KeyCode::Char('a')
                            || key.code == KeyCode::Char('A')
                            || (key.code == KeyCode::Enter && pending.selected_option == 1)
                        );

                        let should_allow_session = if is_multi {
                            key.code == KeyCode::Char('3')
                                || key.code == KeyCode::Char('s')
                                || key.code == KeyCode::Char('S')
                                || (key.code == KeyCode::Enter && pending.selected_option == 2)
                        } else {
                            key.code == KeyCode::Char('2')
                                || key.code == KeyCode::Char('s')
                                || key.code == KeyCode::Char('S')
                                || (key.code == KeyCode::Enter && pending.selected_option == 1)
                        };

                        let should_deny = if is_multi {
                            key.code == KeyCode::Char('4')
                                || key.code == KeyCode::Char('n')
                                || key.code == KeyCode::Char('N')
                                || key.code == KeyCode::Esc
                                || (key.code == KeyCode::Enter && pending.selected_option == 3)
                        } else {
                            key.code == KeyCode::Char('3')
                                || key.code == KeyCode::Char('n')
                                || key.code == KeyCode::Char('N')
                                || key.code == KeyCode::Esc
                                || (key.code == KeyCode::Enter && pending.selected_option == 2)
                        };

                        let should_allow_always = !is_multi && (
                            key.code == KeyCode::Char('4')
                                || key.code == KeyCode::Char('p')
                                || key.code == KeyCode::Char('P')
                                || (key.code == KeyCode::Enter && pending.selected_option == 3)
                        );

                        let should_prompt_feedback = key.code == KeyCode::Char('5')
                            || key.code == KeyCode::Char('w')
                            || key.code == KeyCode::Char('W')
                            || (key.code == KeyCode::Enter && pending.selected_option == 4);

                        if should_prompt_feedback {
                            pending.input_mode = true;
                            pending.selected_option = 4;
                            needs_redraw = true;
                            continue;
                        }

                        if should_deny {
                            let Some(mut pending_batch) = app.pending_confirmation.take() else {
                                continue;
                            };
                            if pending_batch.calls.is_empty() {
                                continue;
                            }
                            let call = pending_batch.calls.remove(0);
                            let mut tool_msg = Message::tool_response(call.id, "Tool execution denied by user.".to_string());
                            tool_msg.name = Some(call.function.name);
                            app.session.add_message(tool_msg);
                            let _ = app.session.save();

                            if pending_batch.calls.is_empty() {
                                app.pending_confirmation = None;
                                if app.in_flight_tool_calls == 0 && !app.sudo_dialog.is_open {
                                    app.start_stream_turn(event_tx.clone());
                                }
                            } else {
                                pending_batch.current_idx += 1;
                                let next_preview = pending_batch.calls.first().map(|c| format_call_preview(c, &app.tool_registry, &app.workspace_dir));
                                pending_batch.diff_preview = next_preview;
                                pending_batch.selected_option = 0;
                                pending_batch.diff_expanded = false;
                                app.pending_confirmation = Some(pending_batch);
                                needs_redraw = true;
                            }
                            continue;
                        }

                        if should_allow_session {
                            let mut added_rules = Vec::new();
                            if let Some(ref pb) = app.pending_confirmation {
                                if let Some(call) = pb.calls.first() {
                                    for rule in extract_tool_allow_rules(call) {
                                        if !rule.is_empty() && !app.session_allowed_commands.contains(&rule) {
                                            app.session_allowed_commands.push(rule.clone());
                                            added_rules.push(rule);
                                        }
                                    }
                                }
                            }
                            if !added_rules.is_empty() {
                                let display = if added_rules.contains(&"apply_patch".to_string()) && added_rules.contains(&"edit".to_string()) {
                                    "file modifications (apply_patch, edit, write_file)".to_string()
                                } else {
                                    added_rules.join(", ")
                                };
                                app.session.add_message(Message::system(format!(
                                    "Allowed for this session: {}",
                                    display
                                )));
                            }
                        }

                        if should_allow_always {
                            let mut cfg = app.llm_client.get_config();
                            let mut added_rules = Vec::new();

                            if let Some(ref pb) = app.pending_confirmation {
                                if let Some(call) = pb.calls.first() {
                                    for rule in extract_tool_allow_rules(call) {
                                        if !rule.is_empty() && !cfg.allowed_commands.contains(&rule) {
                                            cfg.allowed_commands.push(rule.clone());
                                            added_rules.push(rule);
                                        }
                                    }
                                }
                            }

                            if let Err(e) = cfg.save_with_workspace(Some(&app.workspace_dir)) {
                                app.session.add_message(Message::system(format!(
                                    "Warning: Could not save allowed_commands to config: {}",
                                    e
                                )));
                            } else if !added_rules.is_empty() {
                                let display = if added_rules.contains(&"apply_patch".to_string()) && added_rules.contains(&"edit".to_string()) {
                                    "file modifications (apply_patch, edit, write_file)".to_string()
                                } else {
                                    added_rules.join(", ")
                                };
                                app.session.add_message(Message::system(format!(
                                    "Added to ~/.corex/settings.json allowed_commands: {}",
                                    display
                                )));
                            }
                            app.llm_client.update_config(cfg);
                        }

                        if should_allow_single {
                            let Some(mut pending_batch) = app.pending_confirmation.take() else {
                                continue;
                            };
                            if pending_batch.calls.len() > 1 {
                                let call = pending_batch.calls.remove(0);

                                let sudo_needed = if call.function.name == "run_shell_command" {
                                    corex_core::parse_lenient_json(&call.function.arguments)
                                        .ok()
                                        .and_then(|a| a.get("command")?.as_str().map(str::to_string))
                                        .filter(|cmd| command_requires_sudo(cmd))
                                        .map(|cmd| extract_first_sudo_command(&cmd).unwrap_or(cmd))
                                } else {
                                    None
                                };

                                if let Some(sudo_cmd) = sudo_needed {
                                    pending_batch.current_idx += 1;
                                    let next_preview = pending_batch.calls.first().map(|c| format_call_preview(c, &app.tool_registry, &app.workspace_dir));
                                    pending_batch.diff_preview = next_preview;
                                    app.pending_confirmation = Some(pending_batch);
                                    app.sudo_dialog.open_batch(vec![call], sudo_cmd);
                                    continue;
                                }

                                app.in_flight_tool_calls += 1;
                                let reg = app.tool_registry.clone();
                                let ctx = ToolContext {
                                    workspace_dir: app.workspace_dir.clone(),
                                    yolo_mode: app.always_allow_tools,
                                    sudo_password: get_sudo_password(),
                                    allowed_commands: app.get_effective_allowed_commands(),
                                };
                                let call_id = call.id.clone();
                                let tool_name = call.function.name.clone();
                                let args_str = call.function.arguments.clone();
                                let tx = event_tx.clone();
                                let gen = app.current_generation_id;
                                app.is_streaming = true;
                                app.last_turn_start = Some(Instant::now());

                                pending_batch.current_idx += 1;
                                let next_preview = pending_batch.calls.first().map(|c| format_call_preview(c, &app.tool_registry, &app.workspace_dir));
                                pending_batch.diff_preview = next_preview;
                                pending_batch.selected_option = 0;
                                pending_batch.diff_expanded = false;
                                app.pending_confirmation = Some(pending_batch);

                                tokio::spawn(async move {
                                    let summary = format_tool_call_summary(&tool_name, &args_str);
                                    let _ = tx.send((gen, StreamEvent::ToolExecutionStarting {
                                        call_id: call_id.clone(),
                                        name: tool_name.clone(),
                                        summary,
                                    })).await;
                                    let start_tool = std::time::Instant::now();
                                    let (output, success) = match corex_core::parse_lenient_json(&args_str) {
                                        Ok(args_json) => match reg.execute(&tool_name, args_json, &ctx).await {
                                            Ok(o) => (o.output, true),
                                            Err(e) => (format!("Error executing {}: {}", tool_name, e), false),
                                        },
                                        Err(e) => (format!("Error executing {}: invalid tool arguments JSON: {}", tool_name, e), false),
                                    };
                                    let duration_ms = start_tool.elapsed().as_millis();
                                    corex_core::ForensicLogger::log_tool_call(&tool_name, &call_id, &args_str, &output, duration_ms, success);
                                    let _ = tx.send((gen, StreamEvent::ToolExecutionDone { call_id, output })).await;
                                });
                                needs_redraw = true;
                                continue;
                            } else {
                                app.pending_confirmation = Some(pending_batch);
                            }
                        }

                        if should_allow_single || should_allow_all || should_allow_session || should_allow_always {
                            let Some(mut pending_batch) = app.pending_confirmation.take() else {
                                continue;
                            };
                            let context = ToolContext {
                                workspace_dir: app.workspace_dir.clone(),
                                yolo_mode: app.always_allow_tools,
                                sudo_password: get_sudo_password(),
                                allowed_commands: app.get_effective_allowed_commands(),
                            };

                            let calls = if (should_allow_session || should_allow_always) && !should_allow_all && pending_batch.calls.len() > 1 {
                                let mut approved = Vec::new();
                                let mut remaining = Vec::new();
                                for (idx, c) in pending_batch.calls.into_iter().enumerate() {
                                    if idx == 0 {
                                        approved.push(c);
                                    } else {
                                        let args_json = corex_core::parse_lenient_json(&c.function.arguments)
                                            .unwrap_or(serde_json::Value::Null);
                                        let still_needs_confirm = app
                                            .tool_registry
                                            .get(&c.function.name)
                                            .map(|t| t.needs_confirmation(&args_json, &context))
                                            .unwrap_or(true);
                                        if still_needs_confirm {
                                            remaining.push(c);
                                        } else {
                                            approved.push(c);
                                        }
                                    }
                                }
                                if !remaining.is_empty() {
                                    pending_batch.current_idx += approved.len();
                                    let next_preview = remaining
                                        .first()
                                        .map(|c| format_call_preview(c, &app.tool_registry, &app.workspace_dir));
                                    pending_batch.calls = remaining;
                                    pending_batch.diff_preview = next_preview;
                                    pending_batch.selected_option = 0;
                                    pending_batch.diff_expanded = false;
                                    app.pending_confirmation = Some(pending_batch);
                                }
                                approved
                            } else {
                                pending_batch.calls
                            };

                            // Check if sudo password is required before running
                            let sudo_needed = calls.iter().find_map(|c| {
                                if c.function.name == "run_shell_command" {
                                    let args_json = corex_core::parse_lenient_json(&c.function.arguments).ok()?;
                                    let cmd = args_json.get("command")?.as_str()?;
                                    if command_requires_sudo(cmd) {
                                        let extracted = extract_first_sudo_command(cmd).unwrap_or_else(|| cmd.to_string());
                                        return Some(extracted);
                                    }
                                }
                                None
                            });

                            if let Some(sudo_cmd) = sudo_needed {
                                app.sudo_dialog.open_batch(calls, sudo_cmd);
                                continue;
                            }

                            app.in_flight_tool_calls += calls.len();
                            let tx = event_tx.clone();
                            let registry = app.tool_registry.clone();
                            let gen = app.current_generation_id;
                            app.is_streaming = true;
                            app.last_turn_start = Some(Instant::now());

                            tokio::spawn(async move {
                                let calls_count = calls.len();
                                if calls_count > 1 {
                                    let summary = if calls.iter().all(|c| c.function.name == "run_shell_command" || c.function.name == "execute_command") {
                                        format!("Running {} commands in parallel...", calls_count)
                                    } else {
                                        format!("Running {} tasks in parallel...", calls_count)
                                    };
                                    let _ = tx.send((gen, StreamEvent::ToolExecutionStarting {
                                        call_id: "batch".to_string(),
                                        name: "batch".to_string(),
                                        summary,
                                    })).await;
                                }

                                let mut handles = Vec::new();
                                for call in calls {
                                    let reg = registry.clone();
                                    let ctx = context.clone();
                                    let tx_call = tx.clone();
                                    let call_id = call.id.clone();
                                    let tool_name = call.function.name.clone();
                                    let args_str = call.function.arguments.clone();

                                    handles.push(tokio::spawn(async move {
                                        if calls_count == 1 {
                                            let summary = format_tool_call_summary(&tool_name, &args_str);
                                            let _ = tx_call.send((gen, StreamEvent::ToolExecutionStarting {
                                                call_id: call_id.clone(),
                                                name: tool_name.clone(),
                                                summary,
                                            })).await;
                                        }
                                        let output = match corex_core::parse_lenient_json(&args_str) {
                                            Ok(args_json) => match reg.execute(&tool_name, args_json, &ctx).await {
                                                Ok(o) => o.output,
                                                Err(e) => format!("Execution error: {}", e),
                                            },
                                            Err(e) => format!("Execution error: invalid tool arguments JSON: {}", e),
                                        };
                                        let _ = tx_call.send((gen, StreamEvent::ToolExecutionDone { call_id, output })).await;
                                    }));
                                }
                                for h in handles {
                                    let _ = h.await;
                                }
                                let _ = tx.send((gen, StreamEvent::AllToolsDone)).await;
                            });
                            continue;
                        }
                    }

                    // --- 3.5. Interactive Ask-User Dialog Active ---
                    if app.user_dialog.is_open {
                        match key.code {
                            KeyCode::Esc => {
                                app.user_dialog.cancelled = true;
                                finish_ask_user(&mut app, event_tx.clone());
                            }
                            KeyCode::Up => {
                                let cur = app.user_dialog.current;
                                if let Some(q) = app.user_dialog.current_question() {
                                    if q.has_options && app.user_dialog.selected[cur] > 0 {
                                        app.user_dialog.selected[cur] -= 1;
                                    }
                                }
                            }
                            KeyCode::Down => {
                                let cur = app.user_dialog.current;
                                if let Some(q) = app.user_dialog.current_question() {
                                    if q.has_options {
                                        let max = q.options.len().saturating_sub(1);
                                        if app.user_dialog.selected[cur] < max {
                                            app.user_dialog.selected[cur] += 1;
                                        }
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                let cur = app.user_dialog.current;
                                if !app.user_dialog.questions[cur].has_options {
                                    app.user_dialog.text_input[cur].pop();
                                }
                            }
                            KeyCode::PageUp => {
                                app.scroll_chat(-10);
                            }
                            KeyCode::PageDown => {
                                app.scroll_chat(10);
                            }
                            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !c.is_control() => {
                                let cur = app.user_dialog.current;
                                if let Some(q) = app.user_dialog.current_question() {
                                    if q.has_options {
                                        if let Some(digit) = c.to_digit(10) {
                                            if digit >= 1 {
                                                let idx = (digit - 1) as usize;
                                                if idx < q.options.len() {
                                                    app.user_dialog.selected[cur] = idx;
                                                }
                                            }
                                        }
                                    } else {
                                        app.user_dialog.text_input[cur].push(c);
                                    }
                                }
                            }
                            KeyCode::Enter => {
                                let cur = app.user_dialog.current;
                                let total = app.user_dialog.questions.len();
                                if cur + 1 < total {
                                    app.user_dialog.current += 1;
                                } else {
                                    finish_ask_user(&mut app, event_tx.clone());
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- 4. Global Control Hotkeys ---
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
                        needs_redraw = true;
                        continue;
                    }

                    if is_ctrl {
                        match key.code {
                            KeyCode::Char('c') | KeyCode::Char('\x03') => {
                                let is_recent = app.last_ctrl_c_press
                                    .map(|i| i.elapsed() <= std::time::Duration::from_secs(2))
                                    .unwrap_or(false);

                                if is_recent {
                                    break;
                                } else {
                                    app.last_ctrl_c_press = Some(std::time::Instant::now());
                                    needs_redraw = true;
                                }
                                continue;
                            }
                            KeyCode::Char('t') | KeyCode::Char('T') | KeyCode::Char('\x14') => {
                                app.thinking_state.is_expanded = !app.thinking_state.is_expanded;
                                app.invalidate_message_cache();
                                needs_redraw = true;
                                continue;
                            }
                            KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Char('\x0c') => {
                                terminal.clear()?;
                                needs_redraw = true;
                                continue;
                            }
                            _ => {}
                        }
                    }

                    // --- 5. Input Prompt & Slash Autocomplete Navigation ---
                    // Candidates are empty while browsing prompt history, so bare
                    // Up/Down always keep walking the history instead of being
                    // captured by the completion menu (e.g. recalled `/resume`).
                    let matching_cmds = app.matching_slash_commands();
                    let is_slash_open = !matching_cmds.is_empty();

                    match key.code {
                        KeyCode::Left => {
                            if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
                                let before = &app.input_buffer[..app.cursor_idx];
                                let trimmed = before.trim_end();
                                let non_space = match trimmed.rfind(' ') {
                                    Some(pos) => pos + 1,
                                    None => 0,
                                };
                                app.cursor_idx = non_space;
                            } else if app.cursor_idx > 0 {
                                let before = &app.input_buffer[..app.cursor_idx];
                                if before.ends_with(']') {
                                    if let Some(open_idx) = paste_tag_last_start(before) {
                                        app.cursor_idx = open_idx;
                                    } else {
                                        let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                        app.cursor_idx = prev;
                                    }
                                } else {
                                    let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                    app.cursor_idx = prev;
                                }
                            }
                            app.clamp_cursor();
                        }
                        KeyCode::Right => {
                            if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
                                let after = &app.input_buffer[app.cursor_idx..];
                                let trimmed = after.trim_start();
                                let skip_spaces = after.len() - trimmed.len();
                                let word_len = trimmed.find(' ').unwrap_or(trimmed.len());
                                app.cursor_idx = (app.cursor_idx + skip_spaces + word_len).min(app.input_buffer.len());
                            } else if app.cursor_idx < app.input_buffer.len() {
                                let after = &app.input_buffer[app.cursor_idx..];
                                if starts_with_paste_tag(after) {
                                    if let Some(end_bracket) = after.find(']') {
                                        app.cursor_idx = (app.cursor_idx + end_bracket + 1).min(app.input_buffer.len());
                                    } else {
                                        let next = after.chars().next().map(|c| app.cursor_idx + c.len_utf8()).unwrap_or(app.input_buffer.len());
                                        app.cursor_idx = next;
                                    }
                                } else {
                                    let next = after.chars().next().map(|c| app.cursor_idx + c.len_utf8()).unwrap_or(app.input_buffer.len());
                                    app.cursor_idx = next;
                                }
                            }
                            app.clamp_cursor();
                        }
                        KeyCode::Home => {
                            app.cursor_idx = 0;
                        }
                        KeyCode::End => {
                            app.cursor_idx = app.input_buffer.len();
                            if !app.auto_scroll {
                                app.scroll_to_bottom();
                                needs_redraw = true;
                            }
                        }
                        KeyCode::Delete => {
                            app.clamp_cursor();
                            if app.cursor_idx < app.input_buffer.len() {
                                let after = &app.input_buffer[app.cursor_idx..];
                                if starts_with_paste_tag(after) {
                                    if let Some(end_bracket) = after.find(']') {
                                        let tag = after[..=end_bracket].to_string();
                                        app.forget_paste_tag(&tag);
                                        app.input_buffer.drain(app.cursor_idx..app.cursor_idx + end_bracket + 1);
                                    } else {
                                        let ch_len = after.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                                        app.input_buffer.drain(app.cursor_idx..app.cursor_idx + ch_len);
                                    }
                                } else {
                                    let ch_len = after.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                                    app.input_buffer.drain(app.cursor_idx..app.cursor_idx + ch_len);
                                }
                                app.clamp_cursor();
                            }
                        }
                        KeyCode::Char(c) => {
                            let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                            if is_ctrl || c.is_control() {
                                match c {
                                    'a' | 'A' | '\x01' => app.cursor_idx = 0,
                                    'e' | 'E' | '\x05' => app.cursor_idx = app.input_buffer.len(),
                                    'u' | 'U' | '\x15' => {
                                        app.input_buffer.drain(..app.cursor_idx);
                                        app.cursor_idx = 0;
                                    }
                                    'k' | 'K' | '\x0b' => {
                                        app.input_buffer.truncate(app.cursor_idx);
                                    }
                                    // Terminals that do not paste on their own pass Ctrl+V through: take
                                    // an image from the clipboard if there is one, else its text.
                                    'v' | 'V' | '\x16' => {
                                        app.paste_from_clipboard();
                                        needs_redraw = true;
                                    }
                                    'w' | 'W' | '\x17' => {
                                        let before = &app.input_buffer[..app.cursor_idx];
                                        let trimmed = before.trim_end();
                                        let non_space = match trimmed.rfind(' ') {
                                            Some(pos) => pos + 1,
                                            None => 0,
                                        };
                                        app.input_buffer.drain(non_space..app.cursor_idx);
                                        app.cursor_idx = non_space;
                                    }
                                    _ => {}
                                }
                            } else {
                                if c == ' ' && app.input_buffer.is_empty() && !app.auto_scroll {
                                    app.scroll_to_bottom();
                                    needs_redraw = true;
                                    continue;
                                }
                                app.clamp_cursor();
                                app.input_buffer.insert(app.cursor_idx, c);
                                app.cursor_idx += c.len_utf8();
                                app.slash_selected_idx = 0;
                                app.last_esc_press = None;
                            }
                        }
                        KeyCode::Backspace => {
                            app.clamp_cursor();
                            if app.cursor_idx > 0 {
                                let before = &app.input_buffer[..app.cursor_idx];
                                if before.ends_with(']') {
                                    if let Some(open_idx) = paste_tag_last_start(before) {
                                        let tag_slice = &before[open_idx..];
                                        if tag_slice.ends_with(']') {
                                            let tag = tag_slice.to_string();
                                            app.forget_paste_tag(&tag);
                                            app.input_buffer.drain(open_idx..app.cursor_idx);
                                            app.cursor_idx = open_idx;
                                        } else {
                                            let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                            app.input_buffer.drain(prev..app.cursor_idx);
                                            app.cursor_idx = prev;
                                        }
                                    } else {
                                        let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                        app.input_buffer.drain(prev..app.cursor_idx);
                                        app.cursor_idx = prev;
                                    }
                                } else {
                                    let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                    app.input_buffer.drain(prev..app.cursor_idx);
                                    app.cursor_idx = prev;
                                }
                            }
                            app.clamp_cursor();
                            app.slash_selected_idx = 0;
                            app.last_esc_press = None;
                        }
                        KeyCode::Esc => {
                            if is_slash_open {
                                app.input_buffer.clear();
                                app.cursor_idx = 0;
                                app.pastes.clear();
                                app.images.clear();
                                app.slash_selected_idx = 0;
                                app.last_esc_press = None;
                            } else if app.fleet_dialog.is_open && !app.fleet_dialog.command_mode {
                                app.exit_fleet_mode();
                                needs_redraw = true;
                                continue;
                            } else if !app.auto_scroll {
                                // 1st ESC when scrolled up -> jump straight to bottom without modifying input buffer!
                                app.scroll_to_bottom();
                                app.last_esc_press = None;
                                needs_redraw = true;
                                continue;
                            } else {
                                let is_recent = app.last_esc_press
                                    .map(|i| i.elapsed() <= Duration::from_millis(500))
                                    .unwrap_or(false);

                                if is_recent {
                                    // 2nd ESC within 500ms -> Clear prompt & reset history navigation!
                                    app.input_buffer.clear();
                                    app.cursor_idx = 0;
                                    app.pastes.clear();
                                    app.images.clear();
                                    app.history_idx = None;
                                    app.saved_draft.clear();
                                    app.slash_selected_idx = 0;
                                    app.slash_popup_height_current = 0.0;
                                    app.last_esc_press = None;
                                } else {
                                    // 1st ESC -> Start 500ms window to show toast
                                    app.last_esc_press = Some(Instant::now());
                                }
                            }
                        }
                        KeyCode::Tab => {
                            if is_slash_open {
                                let selected = matching_cmds[app.slash_selected_idx % matching_cmds.len()];
                                app.input_buffer = format!("{} ", selected);
                                app.cursor_idx = app.input_buffer.len();
                            }
                        }
                        // History navigation via readline shortcuts: Ctrl+P / Alt+Up (previous)
                        _ if is_ctrl_p || (is_alt && key.code == KeyCode::Up) => {
                            if is_slash_open {
                                if app.slash_selected_idx > 0 {
                                    app.slash_selected_idx -= 1;
                                } else {
                                    app.slash_selected_idx = matching_cmds.len().saturating_sub(1);
                                }
                            } else if !app.input_history.is_empty() {
                                match app.history_idx {
                                    None => {
                                        app.saved_draft = app.input_buffer.clone();
                                        let last_idx = app.input_history.len() - 1;
                                        app.history_idx = Some(last_idx);
                                        app.input_buffer = app.input_history[last_idx].clone();
                                        app.cursor_idx = app.input_buffer.len();
                                    }
                                    Some(i) => {
                                        if i > 0 {
                                            let next_i = i - 1;
                                            app.history_idx = Some(next_i);
                                            app.input_buffer = app.input_history[next_i].clone();
                                            app.cursor_idx = app.input_buffer.len();
                                        }
                                    }
                                }
                            }
                        }
                        // History navigation via readline shortcuts: Ctrl+N / Alt+Down (next)
                        _ if is_ctrl_n || (is_alt && key.code == KeyCode::Down) => {
                            if is_slash_open && !matching_cmds.is_empty() {
                                app.slash_selected_idx = (app.slash_selected_idx + 1) % matching_cmds.len();
                            } else if let Some(i) = app.history_idx {
                                if i + 1 < app.input_history.len() {
                                    let next_i = i + 1;
                                    app.history_idx = Some(next_i);
                                    app.input_buffer = app.input_history[next_i].clone();
                                    app.cursor_idx = app.input_buffer.len();
                                } else {
                                    app.history_idx = None;
                                    app.input_buffer = std::mem::take(&mut app.saved_draft);
                                    app.cursor_idx = app.input_buffer.len();
                                }
                            }
                        }
                        // Chat feed scrolling: Ctrl/Shift modifiers or PageUp/PageDown scroll chat.
                        KeyCode::Up
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                || key.modifiers.contains(KeyModifiers::SHIFT) =>
                        {
                            app.scroll_chat(if key.modifiers.contains(KeyModifiers::SHIFT) {
                                -10
                            } else {
                                -3
                            });
                        }
                        KeyCode::Down
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                || key.modifiers.contains(KeyModifiers::SHIFT) =>
                        {
                            app.scroll_chat(if key.modifiers.contains(KeyModifiers::SHIFT) {
                                10
                            } else {
                                3
                            });
                        }
                        KeyCode::Up => {
                            if is_slash_open {
                                if app.slash_selected_idx > 0 {
                                    app.slash_selected_idx -= 1;
                                } else {
                                    app.slash_selected_idx = matching_cmds.len().saturating_sub(1);
                                }
                            } else if app.tap_keyboard.in_tap_window(Instant::now())
                                || (arrows_may_be_swipes(app.mouse_capture)
                                    && app.touch_arrows.is_swipe(Instant::now(), |d| event::poll(d).unwrap_or(false)))
                            {
                                // Termux sends one arrow per row the finger moves: follow it 1:1.
                                app.scroll_chat(-1);
                            } else if !app.input_history.is_empty() {
                                match app.history_idx {
                                    None => {
                                        app.saved_draft = app.input_buffer.clone();
                                        let last_idx = app.input_history.len() - 1;
                                        app.history_idx = Some(last_idx);
                                        app.input_buffer = app.input_history[last_idx].clone();
                                        app.cursor_idx = app.input_buffer.len();
                                    }
                                    Some(i) => {
                                        if i > 0 {
                                            let next_i = i - 1;
                                            app.history_idx = Some(next_i);
                                            app.input_buffer = app.input_history[next_i].clone();
                                            app.cursor_idx = app.input_buffer.len();
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Down => {
                            if is_slash_open && !matching_cmds.is_empty() {
                                app.slash_selected_idx = (app.slash_selected_idx + 1) % matching_cmds.len();
                            } else if app.tap_keyboard.in_tap_window(Instant::now())
                                || (arrows_may_be_swipes(app.mouse_capture)
                                    && app.touch_arrows.is_swipe(Instant::now(), |d| event::poll(d).unwrap_or(false)))
                            {
                                app.scroll_chat(1);
                            } else if app.history_idx.is_none() && !app.auto_scroll && !is_android_or_termux() {
                                // Scrolled up reading the chat: Down scrolls towards the newest message.
                                // While browsing history, Down keeps walking it back to the draft.
                                app.scroll_chat(3);
                            } else if let Some(i) = app.history_idx {
                                if i + 1 < app.input_history.len() {
                                    let next_i = i + 1;
                                    app.history_idx = Some(next_i);
                                    app.input_buffer = app.input_history[next_i].clone();
                                    app.cursor_idx = app.input_buffer.len();
                                } else {
                                    app.history_idx = None;
                                    app.input_buffer = std::mem::take(&mut app.saved_draft);
                                    app.cursor_idx = app.input_buffer.len();
                                }
                            }
                        }
                        KeyCode::PageUp => app.scroll_chat(-10),
                        KeyCode::PageDown => app.scroll_chat(10),
                        KeyCode::Enter => {
                            // Alt+Enter while the agent works: hand the message over at the next
                            // safe point instead of queueing it behind the whole task.
                            let send_now = is_alt && app.is_streaming;
                            let text = app.input_buffer.trim().to_string();
                            if text.is_empty() && send_now {
                                if app.steer_messages.is_empty() {
                                    // Nothing typed: send the oldest queued message now.
                                    app.promote_queued_to_steer();
                                } else {
                                    // Already waiting for the next safe point: stop and send it now.
                                    app.cancel_generation("Generation interrupted to deliver your message.");
                                    app.deliver_steer();
                                    app.scroll_to_bottom();
                                    app.start_stream_turn(event_tx.clone());
                                }
                                needs_redraw = true;
                                continue;
                            }
                            if !text.is_empty() {
                                let cmd_to_run =if is_slash_open && !text.contains(' ') {
                                    matching_cmds[app.slash_selected_idx % matching_cmds.len()].to_string()
                                } else {
                                    text.clone()
                                };

                                let mut user_prompt_clean = cmd_to_run.clone();

                                // Strip $sudo: and set sudo password in memory if present
                                if let Some(pos) = user_prompt_clean.find("$sudo:") {
                                    let after = &user_prompt_clean[pos + 6..];
                                    let end = after.find(' ').unwrap_or(after.len());
                                    let pwd = &after[..end];
                                    if !pwd.is_empty() {
                                        set_sudo_password(Some(pwd.to_string()));
                                    }
                                    let before = &user_prompt_clean[..pos];
                                    let rest = &after[end..];
                                    user_prompt_clean = format!("{} {}", before.trim(), rest.trim()).trim().to_string();
                                }

                                // Clean up $auto / $yolo prefix if present without mutating global session mode
                                if user_prompt_clean.starts_with("$auto") || user_prompt_clean.starts_with("$yolo") {
                                    user_prompt_clean = user_prompt_clean
                                        .trim_start_matches("$auto")
                                        .trim_start_matches("$yolo")
                                        .trim()
                                        .to_string();
                                }

                                // Record sanitized command to input history (NEVER leak sudo password to history on disk)
                                if app.input_history.last().map(|s| s.as_str()) != Some(&user_prompt_clean) && !user_prompt_clean.is_empty() {
                                    app.input_history.push(user_prompt_clean.clone());
                                    corex_core::HistoryStore::append(&user_prompt_clean);
                                }
                                app.history_idx = None;
                                app.saved_draft.clear();
                                app.input_buffer.clear();
                                app.cursor_idx = 0;
                                app.slash_selected_idx = 0;
                                app.slash_popup_height_current = 0.0;

                                if user_prompt_clean == "/quit" || user_prompt_clean == "/exit" {
                                    break;
                                }

                                if user_prompt_clean.starts_with('/')
                                    && app.handle_slash_command(&user_prompt_clean).await {
                                        // Slash commands never expand paste tags; release the buffers.
                                        app.pastes.clear();
                                        app.images.clear();
                                        if user_prompt_clean.starts_with("/fim")
                                            && app.session.messages.last().map(|m| m.role == "user").unwrap_or(false)
                                        {
                                            app.scroll_to_bottom();
                                            app.start_stream_turn(event_tx.clone());
                                        }
                                        continue;
                                    }

                                if user_prompt_clean.is_empty() {
                                    app.pastes.clear();
                                    app.images.clear();
                                    continue;
                                }

                                // Expand pasted tags safely without re-scanning newly inserted text (prevents recursive OOM)
                                for (id, pasted_content) in &app.pastes {
                                    // Trailing space anchors the id so `#1` never matches `#10`, `#11`, ...
                                    let prefix = format!("[Pasted text #{} ", id);
                                    let mut search_from = 0;
                                    while let Some(rel_idx) = user_prompt_clean[search_from..].find(&prefix) {
                                        let start_idx = search_from + rel_idx;
                                        let rest = &user_prompt_clean[start_idx..];
                                        if let Some(end_bracket) = rest.find(']') {
                                            user_prompt_clean.replace_range(start_idx..start_idx + end_bracket + 1, pasted_content);
                                            search_from = start_idx + pasted_content.len();
                                        } else {
                                            break;
                                        }
                                    }
                                }
                                app.pastes.clear();

                                if app.is_streaming {
                                    if send_now {
                                        app.steer_messages.push(user_prompt_clean);
                                    } else {
                                        app.message_queue.push_back(user_prompt_clean);
                                    }
                                    app.scroll_to_bottom();
                                    needs_redraw = true;
                                    continue;
                                }

                                corex_core::reset_repeat_guard();
                                let message = app.user_message(user_prompt_clean);
                                app.session.add_message(message);
                                app.scroll_to_bottom();

                                // ⚡ START TURN!
                                app.start_stream_turn(event_tx.clone());
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
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

/// Splits a tool batch into the calls that wait for the user's approval and the ones that need
/// none. A shell command that asks for sudo goes with the first group: its own dialog handles it.
fn split_by_approval(app: &App, calls: Vec<ToolCall>, context: &ToolContext) -> (Vec<ToolCall>, Vec<ToolCall>) {
    calls.into_iter().partition(|call| {
        let args = corex_core::parse_lenient_json(&call.function.arguments).unwrap_or(serde_json::Value::Null);
        let needs_approval = app
            .tool_registry
            .get(&call.function.name)
            .is_some_and(|t| t.needs_confirmation(&args, context));
        let needs_sudo = call.function.name == "run_shell_command"
            && args.get("command").and_then(|c| c.as_str()).is_some_and(command_requires_sudo);
        needs_approval || needs_sudo
    })
}

/// Runs `calls` concurrently without blocking the UI. Each one reports `ToolExecutionDone`, and
/// `AllToolsDone` follows once the last one has finished.
fn run_tools_in_parallel(
    app: &mut App,
    calls: Vec<ToolCall>,
    context: ToolContext,
    event_tx: mpsc::Sender<(u64, StreamEvent)>,
) {
    app.in_flight_tool_calls += calls.len();
    let tx = event_tx;
    let registry = app.tool_registry.clone();
    let gen = app.current_generation_id;

    tokio::spawn(async move {
        let calls_count = calls.len();
        if calls_count > 1 {
            let summary = if calls.iter().all(|c| c.function.name == "run_shell_command" || c.function.name == "execute_command") {
                format!("Running {} commands in parallel...", calls_count)
            } else {
                format!("Running {} tasks in parallel...", calls_count)
            };
            let _ = tx.send((gen, StreamEvent::ToolExecutionStarting {
                call_id: "batch".to_string(),
                name: "batch".to_string(),
                summary,
            })).await;
        }

        let mut handles = Vec::new();
        for call in calls {
            let reg = registry.clone();
            let ctx = context.clone();
            let tx_call = tx.clone();
            let call_id = call.id.clone();
            let tool_name = call.function.name.clone();
            let args_str = call.function.arguments.clone();

            handles.push(tokio::spawn(async move {
                let summary = format_tool_call_summary(&tool_name, &args_str);
                let _ = tx_call.send((gen, StreamEvent::ToolExecutionStarting {
                    call_id: call_id.clone(),
                    name: tool_name.clone(),
                    summary,
                })).await;
                let start_tool = std::time::Instant::now();
                let (output, success) = match corex_core::parse_lenient_json(&args_str) {
                    Ok(args_json) => {
                        let result = reg.execute(&tool_name, args_json, &ctx).await;
                        match result {
                            Ok(o) => (o.output, true),
                            Err(e) => (format!("Error executing {}: {}", tool_name, e), false),
                        }
                    }
                    Err(e) => (format!("Error executing {}: invalid tool arguments JSON: {}", tool_name, e), false),
                };
                let duration_ms = start_tool.elapsed().as_millis();
                corex_core::ForensicLogger::log_tool_call(
                    &tool_name,
                    &call_id,
                    &args_str,
                    &output,
                    duration_ms,
                    success
                );
                let _ = tx_call.send((gen, StreamEvent::ToolExecutionDone { call_id, output })).await;
            }));
        }
        for h in handles {
            let _ = h.await;
        }
        let _ = tx.send((gen, StreamEvent::AllToolsDone)).await;
    });
}

/// Finishes the interactive ask_user dialog: executes the pending tool batch
/// (feeding the user's answers to the `ask_user` call), then triggers the next
/// agent turn via `AllToolsDone`.
fn finish_ask_user(app: &mut App, event_tx: mpsc::Sender<(u64, StreamEvent)>) {
    let tx = event_tx.clone();
    let registry = app.tool_registry.clone();
    let gen = app.current_generation_id;
    let context = ToolContext {
        workspace_dir: app.workspace_dir.clone(),
        yolo_mode: app.always_allow_tools,
        sudo_password: get_sudo_password(),
        allowed_commands: app.get_effective_allowed_commands(),
    };
    let calls = app.user_dialog.calls.clone();
    let output = app.user_dialog.format_output();
    let ask_call_id = app.user_dialog.call_id.clone();
    let cancelled = app.user_dialog.cancelled;
    app.user_dialog.close();

    if !cancelled {
        let remaining_calls: Vec<ToolCall> = calls
            .iter()
            .filter(|c| c.id != ask_call_id)
            .cloned()
            .collect();

        if !remaining_calls.is_empty() {
            let mut requires_confirmation = false;
            let mut previews = Vec::new();
            for call in &remaining_calls {
                let tool = app.tool_registry.get(&call.function.name);
                let args_json = corex_core::parse_lenient_json(&call.function.arguments)
                    .unwrap_or(serde_json::Value::Null);
                if let Some(t) = tool {
                    if t.needs_confirmation(&args_json, &context) && !app.always_allow_tools {
                        requires_confirmation = true;
                    }
                    if call.function.name == "run_shell_command" {
                        if let Some(cmd) = args_json.get("command").and_then(|c| c.as_str()) {
                            previews.push(cmd.to_string());
                        }
                    } else if let Some(diff) = t.format_diff(&args_json, &app.workspace_dir) {
                        previews.push(diff);
                    }
                }
            }

            let sudo_needed_cmd = remaining_calls.iter().find_map(|c| {
                if c.function.name == "run_shell_command" {
                    let args_json = corex_core::parse_lenient_json(&c.function.arguments).ok()?;
                    let cmd = args_json.get("command")?.as_str()?;
                    if command_requires_sudo(cmd) {
                        let extracted = extract_first_sudo_command(cmd).unwrap_or_else(|| cmd.to_string());
                        return Some(extracted);
                    }
                }
                None
            });

            if requires_confirmation || sudo_needed_cmd.is_some() {
                let mut ask_msg = Message::tool_response(ask_call_id, output);
                ask_msg.name = Some("ask_user".to_string());
                app.session.add_message(ask_msg);

                if requires_confirmation {
                    let total_count = remaining_calls.len();
                    let first_preview = remaining_calls.first().map(|c| format_call_preview(c, &app.tool_registry, &app.workspace_dir));
                    app.pending_confirmation = Some(PendingToolBatch {
                        calls: remaining_calls,
                        total_count,
                        current_idx: 1,
                        diff_preview: first_preview,
                        selected_option: 0,
                        diff_expanded: false,
                        input_mode: false,
                        feedback_text: String::new(),
                    });
                } else if let Some(sudo_cmd) = sudo_needed_cmd {
                    app.sudo_dialog.open_batch(remaining_calls, sudo_cmd);
                }
                return;
            }
        }
    }

    app.in_flight_tool_calls += calls.len();
    app.is_streaming = true;

    tokio::spawn(async move {
        for call in calls {
            if call.id == ask_call_id {
                let _ = tx
                    .send((gen, StreamEvent::ToolExecutionDone {
                        call_id: call.id,
                        output: output.clone(),
                    }))
                    .await;
            } else if cancelled {
                // Batch aborted: report every remaining call as skipped.
                let _ = tx
                    .send((gen, StreamEvent::ToolExecutionDone {
                        call_id: call.id,
                        output: "Skipped: user cancelled the pending questions.".to_string(),
                    }))
                    .await;
            } else {
                let summary = format_tool_call_summary(&call.function.name, &call.function.arguments);
                let _ = tx
                    .send((gen, StreamEvent::ToolExecutionStarting {
                        call_id: call.id.clone(),
                        name: call.function.name.clone(),
                        summary,
                    }))
                    .await;

                let out = match corex_core::parse_lenient_json(&call.function.arguments) {
                    Ok(args_json) => match registry.execute(&call.function.name, args_json, &context).await {
                        Ok(o) => o.output,
                        Err(e) => format!("Error executing {}: {}", call.function.name, e),
                    },
                    Err(e) => format!("Error executing {}: invalid tool arguments JSON: {}", call.function.name, e),
                };
                let _ = tx
                    .send((gen, StreamEvent::ToolExecutionDone { call_id: call.id, output: out }))
                    .await;
            }
        }
        let _ = tx.send((gen, StreamEvent::AllToolsDone)).await;
    });
}


/// Returns a live SSH session for the selected fleet node, connecting (and
/// probing telemetry) when the current one is missing or belongs to another node.
async fn fleet_ensure_session(app: &mut App) -> Option<corex_fleet::RemoteSession> {
    let node = app.fleet_dialog.selected_node().cloned()?;
    if let Some(sess) = app.fleet_dialog.active_session.clone() {
        if sess.is_connected && sess.node.id == node.id {
            if app.fleet_dialog.pty_session.as_ref().map(|p| !p.is_alive() || p.node_id != node.id).unwrap_or(true) {
                match crate::fleet_pty::FleetPtySession::spawn(&node, 24, 80) {
                    Ok(pty) => app.fleet_dialog.pty_session = Some(pty),
                    Err(err) => {
                        app.fleet_dialog.pty_session = None;
                        app.fleet_dialog.status_msg = Some(format!("Interactive PTY unavailable ({}); fallback batch SSH active", err));
                    }
                }
            }
            return Some(sess);
        }
    }
    app.fleet_dialog.status_msg = Some(format!("Connecting to {}...", node.id));
    let mut session = corex_fleet::RemoteSession::new(node.clone());
    match session.connect(8).await {
        Ok(_) => {
            corex_fleet::global_session_manager().set_session(node.id.clone(), session.clone()).await;
            corex_fleet::global_session_manager().set_active_node(Some(node.id.clone())).await;
            app.fleet_dialog.active_session = Some(session.clone());
            app.fleet_dialog.terminal_scroll = 0;
            app.fleet_dialog.status_msg = None;
            match crate::fleet_pty::FleetPtySession::spawn(&node, 24, 80) {
                Ok(pty) => app.fleet_dialog.pty_session = Some(pty),
                Err(err) => {
                    app.fleet_dialog.pty_session = None;
                    app.fleet_dialog.status_msg = Some(format!("Interactive PTY unavailable ({}); fallback batch SSH mode active (interactive tools like htop/nano disabled)", err));
                }
            }
            app.session.add_message(Message::system(format!(
                "Connected to remote server: {} ({}@{}) · Active and ready.",
                node.id, node.user, node.host
            )));
            app.scroll_to_bottom();
            let tx = app.fleet_dialog.telemetry_tx.clone();
            let n_clone = node.clone();
            tokio::spawn(async move {
                let tel = corex_fleet::TelemetryCollector::fetch(&n_clone, 4).await;
                let _ = tx.send((n_clone.id, tel));
            });
            Some(session)
        }
        Err(e) => {
            app.fleet_dialog.status_msg = Some(format!("Connection failed: {}", e));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corex_core::client::LlmClient;
    use std::path::PathBuf;

    fn ids(calls: &[ToolCall]) -> Vec<&str> {
        calls.iter().map(|c| c.id.as_str()).collect()
    }

    #[tokio::test]
    async fn approval_dialog_only_lists_calls_that_need_approval() {
        let app = App::new(LlmClient::new(corex_core::config::Config::default()), PathBuf::from("/tmp"), false);
        let mut context = ToolContext {
            workspace_dir: PathBuf::from("/tmp"),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };
        let search = ToolCall::new("search", "web_search", r#"{"query":"corex"}"#);
        let fetch = ToolCall::new("fetch", "web_fetch", r#"{"url":"https://example.com/page"}"#);
        let write = ToolCall::new("write", "write_file", r#"{"file_path":"notes.txt","content":"hi"}"#);
        let sudo = ToolCall::new("sudo", "run_shell_command", r#"{"command":"sudo systemctl restart nginx"}"#);

        // Searching and reading a web page never ask; writing a file does, and so does sudo.
        let (gated, free) = split_by_approval(&app, vec![search.clone(), fetch.clone(), write.clone(), sudo], &context);
        assert_eq!(ids(&gated), ["write", "sudo"]);
        assert_eq!(ids(&free), ["search", "fetch"]);

        // Once writing files is allowed, nothing is left to ask about.
        context.allowed_commands.push("write_file".to_string());
        let (gated, free) = split_by_approval(&app, vec![search, fetch, write], &context);
        assert!(gated.is_empty());
        assert_eq!(ids(&free), ["search", "fetch", "write"]);
    }
}
