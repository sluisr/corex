//! Events from the model stream and from running tools: text, tool calls and the end of a turn.

use corex_core::client::{set_sudo_password, StreamEvent};
use corex_core::types::{Message, ToolCall};

use super::tools::{first_sudo_command, needs_approval, pending_batch, run_tools_in_parallel, split_by_approval, tool_context};
use super::EventTx;
use crate::app::state::App;
use crate::user_dialog::parse_questions;

pub(super) async fn handle_stream_event(app: &mut App, event: StreamEvent, event_tx: &EventTx) {
    match event {
        StreamEvent::ReasoningDelta(delta) => {
            if let Some(t) = app.last_turn_start {
                app.latency.record_first_token(t.elapsed().as_millis() as u64);
            }
            app.thinking_state.content.push_str(&delta);
            app.set_status("Thinking...");
        }
        StreamEvent::ContentDelta(delta) => {
            if let Some(t) = app.last_turn_start {
                app.latency.record_first_token(t.elapsed().as_millis() as u64);
            }
            app.streaming_text.push_str(&delta);
            app.set_status("Generating response...");
        }
        StreamEvent::ToolCallDelta { index, id, name, arguments, extra_content } => {
            if let Some(t) = app.last_turn_start {
                app.latency.record_first_token(t.elapsed().as_millis() as u64);
            }
            if index > 64 {
                return;
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
        StreamEvent::ToolExecutionStarting { summary, name, call_id } => {
            app.set_status(&summary);
            if call_id != "batch" {
                app.tool_starts.insert(call_id.clone(), std::time::Instant::now());
            }
            if app.fleet_dialog.is_open {
                app.fleet_dialog.status_msg = Some(summary.clone());
                if name == "fleet_run_command" || name == "fleet" {
                    super::fleet::echo_remote_command(app, &call_id);
                }
            }
        }
        StreamEvent::ToolExecutionDone { call_id, output } => on_tool_done(app, call_id, output, event_tx).await,
        StreamEvent::AllToolsDone => {
            // Trigger next recursive turn of agent loop only when all in-flight tools and dialogs are complete
            if app.in_flight_tool_calls == 0 && !waiting_for_user(app) {
                // Images a tool produced (read_image) follow all tool responses.
                if let Some(image_msg) = corex_core::drain_tool_images_message() {
                    app.session.add_message(image_msg);
                }
                app.deliver_steer();
                app.start_stream_turn(event_tx.clone());
            }
        }
        StreamEvent::Completed { finish_reason } => on_completed(app, finish_reason.as_deref(), event_tx),
        StreamEvent::ContextCompacted { mut compacted_messages, notice } => {
            corex_core::session_images::restore_left_out_images(&mut compacted_messages);
            let before = std::mem::replace(&mut app.session.messages, compacted_messages);
            app.session.keep_history_before_compaction(&before);
            app.session.add_message(Message::system(notice));
            app.invalidate_message_cache();
            app.session.save_async();
            corex_core::trim_memory();
        }
        StreamEvent::Notice(notice) => {
            app.session.add_message(Message::system(notice));
            app.invalidate_message_cache();
        }
        StreamEvent::ServerToolCalls(calls) => {
            app.streaming_server_tool_calls = Some(calls);
        }
        StreamEvent::Title(title) => {
            if app.session.messages.iter().filter(|m| m.role == "user").count() <= 1 {
                app.session.title = title.chars().take(50).collect();
                app.session.save_async();
            }
        }
        StreamEvent::ServerTool { done, summary, .. } => {
            if done {
                app.session.add_message(Message::system(summary));
                app.invalidate_message_cache();
            } else {
                app.set_status(&format!("{summary}…"));
            }
        }
        StreamEvent::Error(err) => {
            app.is_streaming = false;
            app.in_flight_tool_calls = 0;
            app.retried_unexpected_stop = false;
            keep_partial_answer(app);
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

fn keep_partial_answer(app: &mut App) {
    let text = app.streaming_text.trim_end();
    if text.is_empty() {
        return;
    }
    let reasoning = (!app.thinking_state.content.is_empty()).then(|| app.thinking_state.content.clone());
    app.session.add_message(Message::assistant(format!("{text} *(interrupted)*"), reasoning));
}

const REFUSED_NOTICE: &str = "The provider's content filter refused this answer.";

fn unexpected_stop_notice(finish_reason: &str) -> Option<&'static str> {
    Some(match finish_reason {
        "content_filter" | "refusal" | "safety" | "SAFETY" | "blocklist" | "prohibited_content" => REFUSED_NOTICE,
        "malformed_function_call" | "MALFORMED_FUNCTION_CALL" => "The model wrote a tool call the provider could not parse.",
        "insufficient_system_resource" => "The provider ran out of capacity and cut the answer.",
        "recitation" | "RECITATION" => "The provider stopped the answer for repeating source material.",
        _ => return None,
    })
}

fn is_retryable_stop(finish_reason: &str) -> bool {
    matches!(finish_reason, "malformed_function_call" | "MALFORMED_FUNCTION_CALL" | "insufficient_system_resource")
}

/// True while a dialog waits for the user's answer before the turn can go on.
pub(super) fn waiting_for_user(app: &App) -> bool {
    app.pending_confirmation.is_some()
        || app.sudo_dialog.is_open
        || app.user_dialog.is_open
        || app.fleet_dialog.pending_approval.is_some()
}

async fn apply_saved_setting(app: &mut App, args: &str) {
    let Ok(args) = corex_core::parse_lenient_json(args) else { return };
    let Some(key) = args.get("key").and_then(|k| k.as_str()) else { return };
    let mut cfg = app.llm_client.get_config();
    if cfg.set_agent_setting(key, args.get("value").cloned().unwrap_or_default()).is_err() {
        return;
    }
    app.wheel_acceleration = cfg.wheel_acceleration;
    let yolo = cfg.yolo_mode;
    app.llm_client.update_config(cfg);
    match key {
        "yolo_mode" => app.set_yolo_mode(yolo),
        "mcp_servers" => {
            app.reload_mcp_servers().await;
        }
        _ => {}
    }
}

async fn on_tool_done(app: &mut App, call_id: String, output: String, event_tx: &EventTx) {
    app.in_flight_tool_calls = app.in_flight_tool_calls.saturating_sub(1);
    if let Some(started) = app.tool_starts.remove(&call_id) {
        app.latency.record_tool_duration(started.elapsed().as_millis() as u64);
    }
    let turn = app.session.user_turns().len().max(1);
    app.session.file_edits.extend(corex_core::file_history::drain(turn));
    if output.contains("incorrect password attempt")
        || output.contains("sudo: a password is required")
        || output.contains("sudo: PAM authentication")
    {
        set_sudo_password(None);
    }
    let called = app.session.messages.iter().rev()
        .find_map(|m| m.tool_calls.as_ref()?.iter().find(|c| c.id == call_id))
        .map(|c| (c.function.name.clone(), c.function.arguments.clone()));
    let tool_name = called.as_ref().map(|(name, _)| name.clone());
    let member_name = called.as_ref().map(|(name, args)| corex_tools::grouped::canonical_name_raw(name, args));
    if let Some(("corex_settings", args)) = called.as_ref().map(|(n, a)| (n.as_str(), a)) {
        if output.starts_with(corex_tools::settings_tool::SAVED_PREFIX) {
            apply_saved_setting(app, args).await;
        }
    }
    // Huge output is cut to head + tail; the full text stays readable from a file.
    let output = corex_core::spill_large_output(tool_name.as_deref().unwrap_or("tool"), output);
    // Advisory nudge when the model keeps repeating the exact same call.
    let output = match called.and_then(|(name, args)| corex_core::observe_tool_call(&name, &args)) {
        Some(reminder) => format!("{}\n\n{}", output, reminder),
        None => output,
    };
    let mut tool_msg = Message::tool_response(call_id.clone(), output.clone());
    tool_msg.name = tool_name.clone();
    app.session.add_message(tool_msg);
    // The response is drawn under the call that produced it, so that block has to be rebuilt.
    let owner = app.session.display_messages().iter().rposition(|m| {
        m.tool_calls
            .as_ref()
            .is_some_and(|calls| calls.iter().any(|c| c.id == call_id))
    });
    if let Some(idx) = owner {
        app.invalidate_message_cache_from(idx);
    }

    if app.fleet_dialog.is_open {
        super::fleet::show_tool_output(app, member_name.as_deref(), &output).await;
    }
    if waiting_for_user(app) {
        if app.in_flight_tool_calls == 0 {
            app.is_streaming = false;
            app.clear_status();
        }
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

/// The model finished its answer: run its tool calls (asking first where needed) or, when
/// there are none, record the answer and move on to whatever the user queued.
fn on_completed(app: &mut App, finish_reason: Option<&str>, event_tx: &EventTx) {
    if let Some(t) = app.last_turn_start {
        app.latency.record_turn_completed(t.elapsed().as_millis() as u64);
    }
    let truncated = corex_core::is_truncated_finish(finish_reason);
    let assistant_text = (!app.streaming_text.is_empty()).then(|| app.streaming_text.clone());
    let reasoning = (!app.thinking_state.content.is_empty()).then(|| app.thinking_state.content.clone());

    if let (Some(txt), Some(cot)) = (&assistant_text, &reasoning) {
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

    if valid_calls.is_empty() {
        if let Some(notice) = finish_reason.and_then(unexpected_stop_notice) {
            let reason = finish_reason.unwrap_or_default();
            let retry = is_retryable_stop(reason) && !app.retried_unexpected_stop;
            // A refused answer must not stay on screen; DeepSeek asks clients to remove it.
            let refused = notice == REFUSED_NOTICE;
            if let Some(txt) = assistant_text.filter(|_| !retry && !refused) {
                app.session.add_message(Message::assistant(format!("{} *(interrupted)*", txt.trim_end()), reasoning));
            }
            app.streaming_text.clear();
            let next = if retry { "Retrying once." } else { "Send a message to continue." };
            app.session.add_message(Message::system(format!("{notice} (finish_reason: {reason}) {next}")));
            app.invalidate_message_cache();
            app.session.save_async();
            if retry {
                app.retried_unexpected_stop = true;
                app.start_stream_turn(event_tx.clone());
            } else {
                app.retried_unexpected_stop = false;
                pause_for_user(app);
            }
            return;
        }
        app.retried_unexpected_stop = false;
        finish_answer(app, assistant_text, reasoning, event_tx);
        return;
    }
    app.retried_unexpected_stop = false;

    for call in &mut valid_calls {
        if call.extra_content.is_none() {
            call.extra_content = app.streaming_extra_content.clone();
        }
    }

    if truncated {
        // The output hit the token limit: arguments may be cut mid-file or
        // mid-command, and lenient JSON repair would happily "fix" them. Answer
        // every call with an error instead of running it, and let the model retry
        // once in smaller pieces; a second consecutive truncation stops the loop.
        let already_retried = app.session.messages.iter().rev()
            .find(|m| m.role == "tool")
            .and_then(|m| m.text_content())
            .is_some_and(|t| t == corex_core::TRUNCATED_TOOL_CALL_ERROR);
        let call_ids: Vec<String> = valid_calls.iter().map(|c| c.id.clone()).collect();
        let mut msg = Message::assistant_with_tools(assistant_text, reasoning, valid_calls);
        msg.server_tool_calls = app.streaming_server_tool_calls.take();
        app.session.add_message(msg);
        for id in call_ids {
            app.session.add_message(Message::tool_response(
                id,
                corex_core::TRUNCATED_TOOL_CALL_ERROR.to_string(),
            ));
        }
        app.streaming_text.clear();
        app.streaming_extra_content = None;
        app.invalidate_message_cache();
        app.session.save_async();
        if already_retried {
            app.is_streaming = false;
            app.thinking_state.reset();
            app.set_status("Response truncated twice by the output token limit; tool calls were not executed.");
        } else {
            app.start_stream_turn(event_tx.clone());
        }
        return;
    }

    let calls = valid_calls;
    let mut msg = Message::assistant_with_tools(assistant_text, reasoning, calls.clone());
    msg.server_tool_calls = app.streaming_server_tool_calls.take();
    app.session.add_message(msg);
    app.streaming_text.clear();
    app.streaming_extra_content = None;
    app.thinking_state.reset();
    app.thinking_state.is_streaming = true;
    app.session.save_async();
    dispatch_tool_calls(app, calls, event_tx);
}

/// Sends a finished batch of tool calls where it belongs: the ask_user dialog, the approval
/// dialog, the sudo password dialog, or straight to execution.
fn dispatch_tool_calls(app: &mut App, calls: Vec<ToolCall>, event_tx: &EventTx) {
    let context = tool_context(app);
    let requires_confirmation = needs_approval(app, &calls, &context);

    // The interactive ask_user dialog takes priority over the
    // confirmation modal: it must never run as a plain tool.
    let ask = calls
        .iter()
        .find(|c| c.function.name == "ask_user")
        .and_then(|c| parse_questions(&c.function.arguments).map(|q| (q, c.id.clone())));

    if let Some((questions, ask_call_id)) = ask {
        pause_for_user(app);
        app.user_dialog.open(questions, calls, ask_call_id);
    } else if requires_confirmation {
        // Only the calls that need approval reach the dialog. The rest (a web
        // search, a read inside the workspace...) run right away, so the user is
        // never asked about an action that needs no permission.
        let (gated, free) = split_by_approval(app, calls, &context);
        pause_for_user(app);
        if app.fleet_dialog.is_open {
            app.fleet_dialog.command_mode = false;
        }
        if !free.is_empty() {
            run_tools_in_parallel(app, free, context, event_tx.clone());
        }
        app.pending_confirmation = Some(pending_batch(app, gated));
    } else if let Some(sudo_cmd) = first_sudo_command(&calls) {
        pause_for_user(app);
        app.sudo_dialog.open_batch(calls, sudo_cmd);
    } else {
        // Execute all tools concurrently in parallel without blocking UI!
        run_tools_in_parallel(app, calls, context, event_tx.clone());
    }
}

/// Stops the turn's activity indicators while a dialog asks the user something.
fn pause_for_user(app: &mut App) {
    app.is_streaming = false;
    app.thinking_state.reset();
    app.clear_status();
}

/// No tool calls: the final answer of the turn. Records it and starts whatever comes next.
fn finish_answer(app: &mut App, assistant_text: Option<String>, reasoning: Option<String>, event_tx: &EventTx) {
    pause_for_user(app);

    if let Some(mut txt) = assistant_text {
        let user_cjk = corex_core::language::user_has_requested_cjk(&app.session.messages);
        if corex_core::language::is_unwanted_cjk_drift(&txt, user_cjk) {
            txt = corex_core::language::clean_cjk_drift_from_text(&txt);
        }
        let mut assistant_msg = Message::assistant(txt, reasoning);
        assistant_msg.server_tool_calls = app.streaming_server_tool_calls.take();
        assistant_msg.extra_content = app.streaming_extra_content.take();
        app.session.add_message(assistant_msg);
    }

    app.streaming_text.clear();
    app.streaming_tool_calls.clear();
    app.streaming_extra_content = None;
    app.session.save_async();
    corex_core::trim_memory();

    // Automatically dequeue and run the next queued prompt.
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corex_core::client::LlmClient;
    use std::path::PathBuf;

    #[tokio::test]
    async fn the_thought_moves_into_the_message_once_its_tool_calls_run() {
        let mut app = App::new(LlmClient::new(corex_core::config::Config::default()), PathBuf::from("/tmp"), false);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        app.is_streaming = true;
        app.thinking_state.content = "the button is there".to_string();
        app.streaming_tool_calls.push(ToolCall::new("a", "missing_tool", "{}"));

        handle_stream_event(&mut app, StreamEvent::Completed { finish_reason: Some("tool_calls".to_string()) }, &tx).await;

        assert!(app.thinking_state.content.is_empty());
        let last = app.session.messages.last().unwrap();
        assert_eq!(last.reasoning_content.as_deref(), Some("the button is there"));
    }

    fn test_app() -> (App, EventTx) {
        let app = App::new(LlmClient::new(corex_core::config::Config::default()), PathBuf::from("/tmp"), false);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        (app, tx)
    }

    #[tokio::test]
    async fn an_error_mid_answer_keeps_what_was_written() {
        let (mut app, tx) = test_app();
        app.is_streaming = true;
        app.streaming_text = "Half of the answer".to_string();

        handle_stream_event(&mut app, StreamEvent::Error("closed early".to_string()), &tx).await;

        let texts: Vec<String> = app.session.messages.iter().filter_map(|m| m.text_content().map(str::to_string)).collect();
        assert!(texts.iter().any(|t| t == "Half of the answer *(interrupted)*"), "{texts:?}");
        assert!(texts.last().unwrap().contains("closed early"), "{texts:?}");
        assert!(!app.is_streaming);
    }

    #[tokio::test]
    async fn a_content_filter_stop_says_why_the_answer_ended() {
        let (mut app, tx) = test_app();
        app.is_streaming = true;
        app.streaming_text = "Partial".to_string();

        handle_stream_event(&mut app, StreamEvent::Completed { finish_reason: Some("content_filter".to_string()) }, &tx).await;

        let notice = app.session.messages.last().and_then(|m| m.text_content()).unwrap().to_string();
        assert!(notice.contains("content filter") && notice.contains("content_filter"), "{notice}");
        assert!(!app.is_streaming);
        assert!(!app.retried_unexpected_stop);
    }

    #[tokio::test]
    async fn the_answer_keeps_its_server_tool_calls_and_names_the_session() {
        let (mut app, tx) = test_app();
        app.session.add_message(Message::user("explícame un mutex"));
        app.is_streaming = true;
        app.streaming_text = "Un mutex protege un dato.".to_string();

        handle_stream_event(&mut app, StreamEvent::ServerToolCalls(serde_json::json!([{"calls": [{"signature": "abc"}]}])), &tx).await;
        handle_stream_event(&mut app, StreamEvent::Title("Mutex en Rust".to_string()), &tx).await;
        handle_stream_event(&mut app, StreamEvent::Completed { finish_reason: Some("stop".to_string()) }, &tx).await;

        assert_eq!(app.session.title, "Mutex en Rust");
        let answer = app.session.messages.last().unwrap();
        assert_eq!(answer.server_tool_calls.as_ref().unwrap()[0]["calls"][0]["signature"], "abc");
    }

    #[tokio::test]
    async fn a_refused_answer_leaves_the_screen() {
        let (mut app, tx) = test_app();
        app.is_streaming = true;
        app.streaming_text = "Something unsafe".to_string();

        handle_stream_event(&mut app, StreamEvent::Completed { finish_reason: Some("content_filter".to_string()) }, &tx).await;

        assert!(!app.session.messages.iter().any(|m| m.text_content().is_some_and(|t| t.contains("Something unsafe"))));
    }

    #[tokio::test]
    async fn a_malformed_tool_call_is_retried_once() {
        let (mut app, tx) = test_app();
        app.is_streaming = true;
        let stop = || StreamEvent::Completed { finish_reason: Some("malformed_function_call".to_string()) };

        handle_stream_event(&mut app, stop(), &tx).await;
        assert!(app.retried_unexpected_stop);
        assert!(app.is_streaming);

        handle_stream_event(&mut app, stop(), &tx).await;
        assert!(!app.retried_unexpected_stop);
        assert!(!app.is_streaming);
        let notice = app.session.messages.last().and_then(|m| m.text_content()).unwrap().to_string();
        assert!(notice.contains("Send a message to continue"), "{notice}");
    }

    #[tokio::test]
    async fn saving_providers_via_corex_settings_updates_model_dialog_live() {
        let (mut app, _tx) = test_app();
        let args = serde_json::json!({
            "action": "set",
            "key": "providers",
            "value": [{"name": "nvidia", "models": [{"id": "moonshotai/kimi-k3.1-pro", "label": "Kimi K3.1 Pro"}]}]
        }).to_string();
        apply_saved_setting(&mut app, &args).await;
        let cfg = app.llm_client.get_config();
        app.model_dialog.open(&cfg);
        let nv = app.model_dialog.providers.iter().find(|p| p.is_nvidia()).expect("nvidia");
        assert_eq!(nv.models.len(), 1);
        assert_eq!(nv.models[0].id, "moonshotai/kimi-k3.1-pro");
    }
}
