//! Main interactive event loop, key/mouse dispatch, and terminal session lifecycle.

use std::io::{stdout, Write};
use std::time::{Duration, Instant};
use anyhow::Result;
use crossterm::event::{
    self, EnableBracketedPaste, EnableFocusChange, Event, KeyCode,
    KeyEventKind, KeyModifiers,
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
use super::mobile::{is_mobile_portrait, try_show_termux_soft_keyboard};
use super::render::render_ui;
use super::state::{App, PendingToolBatch};
use super::terminal::{restore_terminal, TerminalGuard, MOUSE_REPORTING_OFF, MOUSE_REPORTING_ON};
use crate::user_dialog::parse_questions;

pub async fn run_tui(mut app: App) -> Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        restore_terminal();
        default_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = stdout();
    // Clear any stale mouse mode left by a previously killed session before starting.
    let _ = stdout.write_all(MOUSE_REPORTING_OFF.as_bytes());
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, EnableFocusChange)?;
    let _guard = TerminalGuard;

    // Closing the terminal window (SIGHUP) or `kill` (SIGTERM) skips Drop; restore explicitly.
    #[cfg(unix)]
    tokio::spawn(async {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut hup), Ok(mut term)) =
            (signal(SignalKind::hangup()), signal(SignalKind::terminate()))
        else {
            return;
        };
        tokio::select! {
            _ = hup.recv() => {}
            _ = term.recv() => {}
        }
        restore_terminal();
        std::process::exit(130);
    });

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
        app.poll_local_health_check();

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

        let mut has_finished_tasks = false;
        for (pid, exit_code, dur, output) in newly_finished {
            let code_str = match exit_code {
                Some(c) => c.to_string(),
                None => "stopped".to_string(),
            };
            let mut msg_text = format!("TASK_DONE|{}|{}|{}\n", pid, code_str, dur);
            msg_text.push_str(output.trim_end());
            app.session.add_message(Message::system(msg_text));
            app.invalidate_message_cache();
            needs_redraw = true;
            if exit_code.is_some() {
                has_finished_tasks = true;
            }
        }

        // Check for finished subagents (reactive wakeup)
        let newly_finished_subagents = {
            if let Ok(mut mgr) = corex_core::subagent::get_subagent_manager().lock() {
                mgr.drain_unacknowledged()
            } else {
                Vec::new()
            }
        };

        for (sub_id, role, dur, output) in newly_finished_subagents {
            let mut msg_text = format!("SUBAGENT_DONE|{}|{}|{}\n", sub_id, role.as_str().to_uppercase(), dur);
            msg_text.push_str(output.trim_end());
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
            && app.pending_confirmation.is_none()
            && !app.user_dialog.is_open
            && !app.sudo_dialog.is_open
            && !app.is_checking_balance
        {
            if !app.message_queue.is_empty() {
                if let Some(next_prompt) = app.message_queue.pop_front() {
                    app.session.add_message(Message::user(next_prompt));
                    app.start_stream_turn(event_tx.clone());
                    needs_redraw = true;
                }
            } else if has_finished_tasks {
                // Reactive Wakeup: Background tasks completed while idle -> autonomously synthesize response!
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
        let is_header_animating = app.scroll_offset <= 4 && !app.is_streaming && app.last_interaction.elapsed() < Duration::from_secs(4);
        let is_animating = app.is_slash_animating() || app.status_transition.is_animating() || is_header_animating;

        if needs_redraw || app.is_streaming || app.is_checking_balance || has_active_tasks || is_animating {
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
                StreamEvent::ToolExecutionStarting { summary, .. } => {
                    app.set_status(&summary);
                }
                StreamEvent::ToolExecutionDone { call_id, output } => {
                    if output.contains("incorrect password attempt")
                        || output.contains("sudo: a password is required")
                        || output.contains("sudo: PAM authentication")
                    {
                        set_sudo_password(None);
                    }
                    app.session.add_message(Message::tool_response(call_id, output));
                }
                StreamEvent::AllToolsDone => {
                    // ⚡ TRIGGER NEXT RECURSIVE TURN OF AGENT LOOP!
                    app.start_stream_turn(event_tx.clone());
                }
                StreamEvent::Completed { .. } => {
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

                    if !valid_calls.is_empty() {
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
                        } else if let Some(sudo_cmd) = sudo_needed_cmd {
                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.clear_status();
                            app.sudo_dialog.open_batch(calls.clone(), sudo_cmd);
                        } else if requires_confirmation {
                            let combined_preview = if previews.is_empty() {
                                None
                            } else {
                                Some(previews.join("\n"))
                            };

                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.clear_status();
                            app.pending_confirmation = Some(PendingToolBatch {
                                calls: calls.clone(),
                                diff_preview: combined_preview,
                                selected_option: 0,
                                diff_expanded: false,
                                input_mode: false,
                                feedback_text: String::new(),
                            });
                        } else {
                            // Execute all tools concurrently in parallel without blocking UI!
                            let tx = event_tx.clone();
                            let registry = app.tool_registry.clone();
                            let context = confirmation_context;
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
                                        if calls_count == 1 {
                                            let summary = format_tool_call_summary(&tool_name, &args_str);
                                            let _ = tx_call.send((gen, StreamEvent::ToolExecutionStarting {
                                                call_id: call_id.clone(),
                                                name: tool_name.clone(),
                                                summary,
                                            })).await;
                                        }
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
                        if let Some(next_prompt) = app.message_queue.pop_front() {
                            app.session.add_message(Message::user(next_prompt));
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

        if app.is_streaming && app.thinking_state.is_streaming {
            if let Some(start) = app.last_turn_start {
                app.thinking_state.elapsed_secs = start.elapsed().as_secs_f32();
            }
        }

        let is_fast_animating = app.is_streaming || app.is_slash_animating() || app.status_transition.is_animating();
        let tick_rate = if is_fast_animating {
            Duration::from_millis(16)
        } else if is_header_animating {
            Duration::from_millis(60)
        } else {
            Duration::from_millis(80)
        };
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            needs_redraw = true;
            app.last_interaction = Instant::now();
            match event::read()? {
                Event::Paste(pasted) => {
                    corex_core::forensic::ForensicLogger::log_event(
                        "TUI_PASTE",
                        &format!("Paste event ({} chars)", pasted.len()),
                        &format!("Content:\n{}", pasted),
                    );
                    app.handle_paste(pasted);
                }
                // A resize or regaining focus can leave ghost cells the diff renderer believes
                // are correct; wipe the backing buffer so the next draw repaints everything.
                Event::Resize(w, h) => {
                    let _ = terminal.clear();
                    app.handle_resize(w, h);
                }
                Event::FocusGained => {
                    let _ = terminal.clear();
                }
                Event::Mouse(mouse) => {
                    app.handle_mouse(mouse);
                    let (w, h) = terminal.size().map(|s| (s.width, s.height)).unwrap_or((80, 24));
                    if is_mobile_portrait(w, h) {
                        try_show_termux_soft_keyboard();
                    }
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

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
                            if key.code == KeyCode::Esc && !app.message_queue.is_empty() {
                                if let Some(queued) = app.message_queue.pop_back() {
                                    if app.input_buffer.is_empty() {
                                        app.input_buffer = queued;
                                        app.cursor_idx = app.input_buffer.len();
                                    }
                                }
                                needs_redraw = true;
                                continue;
                            }

                            if let Some(token) = app.cancel_token.take() {
                                token.cancel();
                            }
                            app.current_generation_id = app.current_generation_id.wrapping_add(1);
                            app.is_streaming = false;
                            app.thinking_state.is_streaming = false;
                            app.clear_status();
                            app.message_queue.clear();

                            let partial_text = if app.streaming_text.is_empty() {
                                None
                            } else {
                                Some(format!("{} *(interrupted)*", app.streaming_text.trim_end()))
                            };

                            let reasoning = if app.thinking_state.content.is_empty() {
                                None
                            } else {
                                Some(app.thinking_state.content.clone())
                            };

                            if let Some(txt) = partial_text {
                                app.session.add_message(Message::assistant(txt, reasoning));
                            } else {
                                app.session.add_message(Message::system("Generation cancelled by user (Esc)."));
                            }

                            app.streaming_text.clear();
                            app.streaming_tool_calls.clear();
                            let _ = app.session.save();
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
                                app.start_stream_turn(event_tx.clone());
                                app.sudo_dialog.close();
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

                                    if !env_var.is_empty() {
                                        std::env::set_var(&env_var, &key_str);
                                    }

                                    let mut cfg = app.llm_client.get_config();
                                    if prov_name == "deepseek" {
                                        cfg.api_key = key_str.clone();
                                    } else {
                                        cfg.provider_api_keys.insert(prov_name.clone(), key_str.clone());
                                    }
                                    let _ = cfg.save_with_workspace(Some(&app.workspace_dir));
                                    app.llm_client.update_config(cfg);
                                    if let Some(pos) = app.model_dialog.providers.iter().position(|p| p.name == prov_name) {
                                        if pos < app.model_dialog.ready.len() {
                                            app.model_dialog.ready[pos] = true;
                                        }
                                    }
                                    app.auth_dialog.close();
                                    app.session.add_message(Message::system(format!(
                                        "API key saved for {} (${})!",
                                        prov_label, env_var
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
                            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) || app.session_dialog.search_query.is_empty() => {
                                if let Some(selected_session) = app.session_dialog.selected_session().cloned() {
                                    let session_id = selected_session.id.clone();
                                    let _ = Session::delete_by_id_or_tag(&session_id);
                                    // Refresh list
                                    app.session_dialog.open(&app.workspace_dir.display().to_string());
                                }
                            }
                            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                                app.session_dialog.search_query.push(c);
                                app.session_dialog.selected_idx = 0;
                            }
                            KeyCode::Enter => {
                                if let Some(selected_session) = app.session_dialog.selected_session().cloned() {
                                    let session_id = selected_session.id.clone();
                                    match Session::load_by_id_or_tag(&session_id) {
                                        Ok(loaded) => {
                                            let mut cfg = app.llm_client.get_config();
                                            cfg.model = loaded.model.clone();
                                            if let Some(t) = loaded.temperature {
                                                cfg.temperature = t;
                                            }
                                            if let Some(ref r) = loaded.reasoning_effort {
                                                cfg.reasoning_effort = r.clone();
                                            }
                                            app.llm_client.update_config(cfg);
                                            app.session = loaded;
                                            app.invalidate_message_cache();
                                            app.session.add_message(Message::system(format!(
                                                "Resumed session '{}' (Model: {}).",
                                                app.session.title, app.session.model
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
                                    app.start_stream_turn(event_tx.clone());
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

                        let should_allow_once = key.code == KeyCode::Char('1')
                            || key.code == KeyCode::Char('y')
                            || key.code == KeyCode::Char('Y')
                            || (key.code == KeyCode::Enter && pending.selected_option == 0);

                        let should_allow_session = key.code == KeyCode::Char('2')
                            || key.code == KeyCode::Char('s')
                            || key.code == KeyCode::Char('S')
                            || (key.code == KeyCode::Enter && pending.selected_option == 1);

                        let should_deny = key.code == KeyCode::Char('3')
                            || key.code == KeyCode::Char('n')
                            || key.code == KeyCode::Char('N')
                            || key.code == KeyCode::Esc
                            || (key.code == KeyCode::Enter && pending.selected_option == 2);

                        let should_allow_always = key.code == KeyCode::Char('4')
                            || key.code == KeyCode::Char('p')
                            || key.code == KeyCode::Char('P')
                            || (key.code == KeyCode::Enter && pending.selected_option == 3);

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

                        if should_allow_session {
                            let mut added_rules = Vec::new();
                            for call in &pending.calls {
                                let rule = extract_tool_allow_rule(call);
                                if !rule.is_empty() && !app.session_allowed_commands.contains(&rule) {
                                    app.session_allowed_commands.push(rule.clone());
                                    added_rules.push(rule);
                                }
                            }
                            if !added_rules.is_empty() {
                                app.session.add_message(Message::system(format!(
                                    "Allowed for this session: {}",
                                    added_rules.join(", ")
                                )));
                            }
                        }

                        if should_allow_always {
                            let mut cfg = app.llm_client.get_config();
                            let mut added_rules = Vec::new();

                            for call in &pending.calls {
                                let rule = extract_tool_allow_rule(call);
                                if !rule.is_empty() && !cfg.allowed_commands.contains(&rule) {
                                    cfg.allowed_commands.push(rule.clone());
                                    added_rules.push(rule);
                                }
                            }

                            if let Err(e) = cfg.save_with_workspace(Some(&app.workspace_dir)) {
                                app.session.add_message(Message::system(format!(
                                    "Warning: Could not save allowed_commands to config: {}",
                                    e
                                )));
                            } else if !added_rules.is_empty() {
                                app.session.add_message(Message::system(format!(
                                    "Added to ~/.corex/settings.json allowed_commands: {}",
                                    added_rules.join(", ")
                                )));
                            }
                            app.llm_client.update_config(cfg);
                        }

                        if should_allow_once || should_allow_session || should_allow_always {
                            let Some(pending_batch) = app.pending_confirmation.take() else {
                                continue;
                            };
                            let calls = pending_batch.calls;

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

                            let context = ToolContext {
                                workspace_dir: app.workspace_dir.clone(),
                                yolo_mode: app.always_allow_tools,
                                sudo_password: get_sudo_password(),
                                allowed_commands: app.get_effective_allowed_commands(),
                            };

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
                        } else if should_deny {
                            let Some(pending_batch) = app.pending_confirmation.take() else {
                                continue;
                            };
                            for call in pending_batch.calls {
                                app.session.add_message(Message::tool_response(
                                    call.id,
                                    "Tool execution denied by user.",
                                ));
                            }

                            // ⚡ TRIGGER NEXT RECURSIVE TURN OF AGENT LOOP!
                            app.start_stream_turn(event_tx.clone());
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
                            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !c.is_control() => {
                                let cur = app.user_dialog.current;
                                if !app.user_dialog.questions[cur].has_options {
                                    app.user_dialog.text_input[cur].push(c);
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
                                    if let Some(open_idx) = before.rfind("[Pasted text #") {
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
                                if after.starts_with("[Pasted text #") {
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
                        }
                        KeyCode::Delete => {
                            app.clamp_cursor();
                            if app.cursor_idx < app.input_buffer.len() {
                                let after = &app.input_buffer[app.cursor_idx..];
                                if after.starts_with("[Pasted text #") {
                                    if let Some(end_bracket) = after.find(']') {
                                        let tag_slice = &after[..=end_bracket];
                                        if let Some(num_str) = tag_slice.strip_prefix("[Pasted text #") {
                                            if let Some(space_pos) = num_str.find(' ') {
                                                if let Ok(id) = num_str[..space_pos].parse::<usize>() {
                                                    app.pastes.remove(&id);
                                                }
                                            }
                                        }
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
                                    if let Some(open_idx) = before.rfind("[Pasted text #") {
                                        let tag_slice = &before[open_idx..];
                                        if tag_slice.ends_with(']') {
                                            if let Some(num_str) = tag_slice.strip_prefix("[Pasted text #") {
                                                if let Some(space_pos) = num_str.find(' ') {
                                                    if let Ok(id) = num_str[..space_pos].parse::<usize>() {
                                                        app.pastes.remove(&id);
                                                    }
                                                }
                                            }
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
                                app.slash_selected_idx = 0;
                                app.last_esc_press = None;
                            } else {
                                let is_recent = app.last_esc_press
                                    .map(|i| i.elapsed() <= Duration::from_millis(500))
                                    .unwrap_or(false);

                                if is_recent {
                                    // 2nd ESC within 500ms -> Clear prompt & reset history navigation!
                                    app.input_buffer.clear();
                                    app.cursor_idx = 0;
                                    app.pastes.clear();
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
                        // Chat feed scrolling. Bare Up/Down are reserved for prompt history and
                        // the slash menu, so scrolling uses a modifier (Shift = full page).
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
                            let text = app.input_buffer.trim().to_string();
                            if !text.is_empty() {
                                let cmd_to_run = if is_slash_open && !text.contains(' ') {
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
                                        continue;
                                    }

                                if user_prompt_clean.is_empty() {
                                    continue;
                                }

                                // Expand pasted tags safely without re-scanning newly inserted text (prevents recursive OOM)
                                for (id, pasted_content) in &app.pastes {
                                    let prefix = format!("[Pasted text #{}", id);
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
                                    app.message_queue.push_back(user_prompt_clean);
                                    needs_redraw = true;
                                    continue;
                                }

                                app.session.add_message(Message::user(user_prompt_clean));

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

fn print_session_summary(app: &App) {
    let cfg = app.llm_client.get_config();
    let is_local = cfg.local_llm_enabled;

    let is_deepseek = cfg.is_deepseek_endpoint();
    let model_display = if is_local {
        if !cfg.local_llm_model.is_empty() {
            format!("Local Offline Assistant ({})", cfg.local_llm_model)
        } else {
            "Local Offline Assistant (Air-gapped SLM)".to_string()
        }
    } else if is_deepseek && (cfg.model == "deepseek-flash" || cfg.model == "deepseek-v4.1-flash" || cfg.model == "deepseek-v4-flash" || cfg.model == "deepseek-chat") {
        "DeepSeek-V4.1-Flash".to_string()
    } else if is_deepseek && (cfg.model == "deepseek-v4-pro" || cfg.model == "deepseek-reasoner") {
        "DeepSeek-V4-Pro".to_string()
    } else {
        let all = cfg.all_providers();
        let prov = cfg.active_provider.as_deref().and_then(|p_name| all.iter().find(|p| p.name == p_name));
        prov.and_then(|p| p.models.iter().find(|m| m.id == cfg.model))
            .map(|m| format!("{} · {}", prov.map(|p| p.display_name()).unwrap_or(""), m.display_name()))
            .unwrap_or_else(|| cfg.model.clone())
    };

    let elapsed = chrono::Utc::now().signed_duration_since(app.session.created_at).num_seconds().max(0);
    let duration_str = if elapsed < 60 {
        format!("{}s", elapsed)
    } else {
        format!("{}m {}s", elapsed / 60, elapsed % 60)
    };

    let msg_count = app.session.messages.len();
    let u = &app.session.total_usage;

    let (cost_str, tokens_str, cache_str) = if is_local {
        (
            "\x1b[38;2;105;240;174m$0.000000 USD\x1b[0m \x1b[90m(100% Free Local Hardware)\x1b[0m".to_string(),
            format!("{} in · {} out ({} total)", u.prompt_tokens, u.completion_tokens, u.total_tokens),
            None,
        )
    } else if u.total_tokens > 0 {
        let is_pro = cfg.model.contains("pro") || cfg.model.contains("reasoner");
        let (cached_rate, miss_rate, comp_rate) = if is_pro {
            (0.14 / 1_000_000.0, 0.55 / 1_000_000.0, 2.19 / 1_000_000.0)
        } else {
            (0.014 / 1_000_000.0, 0.14 / 1_000_000.0, 0.28 / 1_000_000.0)
        };
        let miss = u.prompt_tokens.saturating_sub(u.prompt_cache_hit_tokens);
        let actual_cost = (u.prompt_cache_hit_tokens as f64 * cached_rate)
            + (miss as f64 * miss_rate)
            + (u.completion_tokens as f64 * comp_rate);
        let un_cached = (u.prompt_tokens as f64 * miss_rate) + (u.completion_tokens as f64 * comp_rate);
        let saved = (un_cached - actual_cost).max(0.0);

        let hit_rate = u.cache_hit_percentage();
        let cost_text = if saved > 0.00001 {
            format!("\x1b[38;2;105;240;174m${:.6} USD\x1b[0m \x1b[90m(Saved ${:.6} with KV Cache)\x1b[0m", actual_cost, saved)
        } else {
            format!("\x1b[38;2;105;240;174m${:.6} USD\x1b[0m", actual_cost)
        };

        let cache_info = if u.prompt_tokens > 0 {
            Some(format!("{:.1}% Hit Rate ({} cached / {} in)", hit_rate, u.prompt_cache_hit_tokens, u.prompt_tokens))
        } else {
            None
        };

        (
            cost_text,
            format!("{} in · {} out ({} total)", u.prompt_tokens, u.completion_tokens, u.total_tokens),
            cache_info,
        )
    } else {
        (
            "\x1b[90m$0.000000 USD (No LLM turns)\x1b[0m".to_string(),
            "0 total".to_string(),
            None,
        )
    };

    println!();
    println!("\x1b[1;38;2;56;189;248m✦ Corex Session Summary\x1b[0m");
    println!("  \x1b[90mModel:\x1b[0m     \x1b[1m{}\x1b[0m", model_display);
    println!("  \x1b[90mDuration:\x1b[0m  {} \x1b[90m·\x1b[0m {} messages", duration_str, msg_count);
    println!("  \x1b[90mTokens:\x1b[0m    {}", tokens_str);
    if let Some(c_info) = cache_str {
        println!("  \x1b[90mKV Cache:\x1b[0m  {}", c_info);
    }
    println!("  \x1b[90mCost:\x1b[0m      {}", cost_str);
    println!();
    println!("  \x1b[90mResume:\x1b[0m    \x1b[38;2;135;215;215mcx --resume {}\x1b[0m", app.session.id);
    println!();
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

