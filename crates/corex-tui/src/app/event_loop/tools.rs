//! Running the model's tool calls: approval checks, sudo detection and the concurrent executor.

use std::time::Instant;

use corex_core::client::{get_sudo_password, StreamEvent};
use corex_core::types::{Message, ToolCall};
use corex_tools::types::ToolContext;
use corex_tools::{command_requires_sudo, extract_first_sudo_command};

use super::EventTx;
use crate::app::formatting::format_tool_call_summary;
use crate::app::state::{App, PendingToolBatch};

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

/// The context tools run with: the workspace, the approval rules and the sudo password in RAM.
pub(super) fn tool_context(app: &App) -> ToolContext {
    ToolContext {
        workspace_dir: app.workspace_dir.clone(),
        yolo_mode: app.always_allow_tools,
        sudo_password: get_sudo_password(),
        allowed_commands: app.get_effective_allowed_commands(),
    }
}

/// The first shell command in `calls` that needs sudo, in the form the password dialog shows.
pub(super) fn first_sudo_command(calls: &[ToolCall]) -> Option<String> {
    calls.iter().find_map(|c| {
        if c.function.name != "run_shell_command" {
            return None;
        }
        let args_json = corex_core::parse_lenient_json(&c.function.arguments).ok()?;
        let cmd = args_json.get("command")?.as_str()?;
        command_requires_sudo(cmd).then(|| extract_first_sudo_command(cmd).unwrap_or_else(|| cmd.to_string()))
    })
}

/// True when a call in `calls` must wait for the user (never with auto-approve on).
pub(super) fn needs_approval(app: &App, calls: &[ToolCall], context: &ToolContext) -> bool {
    !app.always_allow_tools
        && calls.iter().any(|call| {
            let args = corex_core::parse_lenient_json(&call.function.arguments).unwrap_or(serde_json::Value::Null);
            app.tool_registry
                .get(&call.function.name)
                .is_some_and(|t| t.needs_confirmation(&args, context))
        })
}

/// Splits a tool batch into the calls that wait for the user's approval and the ones that need
/// none. A shell command that asks for sudo goes with the first group: its own dialog handles it.
pub(super) fn split_by_approval(app: &App, calls: Vec<ToolCall>, context: &ToolContext) -> (Vec<ToolCall>, Vec<ToolCall>) {
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

/// The approval dialog for `calls`, showing the first one.
pub(super) fn pending_batch(app: &App, calls: Vec<ToolCall>) -> PendingToolBatch {
    let diff_preview = calls.first().map(|c| format_call_preview(c, &app.tool_registry, &app.workspace_dir));
    PendingToolBatch {
        total_count: calls.len(),
        calls,
        current_idx: 1,
        diff_preview,
        selected_option: 0,
        diff_expanded: false,
        input_mode: false,
        feedback_text: String::new(),
    }
}

/// Moves the approval dialog `done` calls further, onto the first call still waiting.
pub(super) fn show_next_call(app: &App, batch: &mut PendingToolBatch, done: usize) {
    batch.current_idx += done;
    batch.diff_preview = batch.calls.first().map(|c| format_call_preview(c, &app.tool_registry, &app.workspace_dir));
    batch.selected_option = 0;
    batch.diff_expanded = false;
}

/// Runs `calls` concurrently without blocking the UI. Each one reports `ToolExecutionDone`, and
/// `AllToolsDone` follows once the last one has finished.
pub(super) fn run_tools_in_parallel(app: &mut App, calls: Vec<ToolCall>, context: ToolContext, event_tx: EventTx) {
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
            handles.push(tokio::spawn(async move {
                run_one_tool(&reg, call, &ctx, gen, &tx_call).await;
            }));
        }
        for h in handles {
            let _ = h.await;
        }
        let _ = tx.send((gen, StreamEvent::AllToolsDone)).await;
    });
}

/// Runs one approved call while the rest of its batch still waits in the approval dialog. No
/// `AllToolsDone` follows: the batch is not finished.
pub(super) fn run_single_tool(app: &mut App, call: ToolCall, event_tx: EventTx) {
    app.in_flight_tool_calls += 1;
    app.is_streaming = true;
    app.last_turn_start = Some(Instant::now());
    let registry = app.tool_registry.clone();
    let context = tool_context(app);
    let gen = app.current_generation_id;
    tokio::spawn(async move {
        run_one_tool(&registry, call, &context, gen, &event_tx).await;
    });
}

/// Executes `call`, logs it to the forensic trail and reports its start and its output.
async fn run_one_tool(registry: &corex_tools::ToolRegistry, call: ToolCall, ctx: &ToolContext, gen: u64, tx: &EventTx) {
    let call_id = call.id;
    let tool_name = call.function.name;
    let args_str = call.function.arguments;
    let summary = format_tool_call_summary(&tool_name, &args_str);
    let _ = tx.send((gen, StreamEvent::ToolExecutionStarting {
        call_id: call_id.clone(),
        name: tool_name.clone(),
        summary,
    })).await;
    let start_tool = Instant::now();
    let (output, success) = match corex_core::parse_lenient_json(&args_str) {
        Ok(args_json) => match registry.execute(&tool_name, args_json, ctx).await {
            Ok(o) => (o.output, true),
            Err(e) => (format!("Error executing {}: {}", tool_name, e), false),
        },
        Err(e) => (format!("Error executing {}: invalid tool arguments JSON: {}", tool_name, e), false),
    };
    let duration_ms = start_tool.elapsed().as_millis();
    corex_core::ForensicLogger::log_tool_call(&tool_name, &call_id, &args_str, &output, duration_ms, success);
    let _ = tx.send((gen, StreamEvent::ToolExecutionDone { call_id, output })).await;
}

/// Finishes the interactive ask_user dialog: executes the pending tool batch
/// (feeding the user's answers to the `ask_user` call), then triggers the next
/// agent turn via `AllToolsDone`.
pub(super) fn finish_ask_user(app: &mut App, event_tx: EventTx) {
    let tx = event_tx;
    let registry = app.tool_registry.clone();
    let gen = app.current_generation_id;
    let context = tool_context(app);
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
        let requires_confirmation = needs_approval(app, &remaining_calls, &context);
        let sudo_needed_cmd = first_sudo_command(&remaining_calls);

        if requires_confirmation || sudo_needed_cmd.is_some() {
            let mut ask_msg = Message::tool_response(ask_call_id, output);
            ask_msg.name = Some("ask_user".to_string());
            app.session.add_message(ask_msg);

            if requires_confirmation {
                app.pending_confirmation = Some(pending_batch(app, remaining_calls));
            } else if let Some(sudo_cmd) = sudo_needed_cmd {
                app.sudo_dialog.open_batch(remaining_calls, sudo_cmd);
            }
            return;
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

    #[test]
    fn first_sudo_command_finds_the_shell_call_that_escalates() {
        let read = ToolCall::new("r", "read_file", r#"{"file_path":"a.txt"}"#);
        let ls = ToolCall::new("ls", "run_shell_command", r#"{"command":"ls"}"#);
        let restart = ToolCall::new("s", "run_shell_command", r#"{"command":"sudo systemctl restart nginx"}"#);
        assert_eq!(first_sudo_command(&[read.clone(), ls.clone()]), None);
        assert!(first_sudo_command(&[read, ls, restart]).is_some_and(|c| c.contains("systemctl restart nginx")));
    }
}
