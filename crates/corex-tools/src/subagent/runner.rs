//! The loop a subagent runs against the model, and the forensic trace it leaves behind.

use std::path::{Path, PathBuf};
use async_trait::async_trait;
use corex_core::subagent::{get_subagent_manager, SubagentRole};
use corex_core::client::{LlmClient, StreamEvent};
use corex_core::types::{Message, MessageContent, ToolCall, FunctionCall, ToolDefinition};
use crate::registry::ToolRegistry;
use crate::types::ToolContext;

#[cfg(not(test))]
use super::get_subagent_llm_client;

/// Forensic tracer that records every decision step, shell execution, exit code, and timing
/// for a delegated subagent, persisting an exhaustive audit log on completion.
pub(super) struct ForensicTrace {
    task_id: String,
    role: SubagentRole,
    prompt: String,
    workspace: PathBuf,
    started_at: chrono::DateTime<chrono::Utc>,
    steps: Vec<String>,
}

impl ForensicTrace {
    pub(super) fn new(task_id: &str, role: SubagentRole, prompt: &str, workspace: &Path) -> Self {
        Self {
            task_id: task_id.to_string(),
            role,
            prompt: prompt.to_string(),
            workspace: workspace.to_path_buf(),
            started_at: chrono::Utc::now(),
            steps: Vec::new(),
        }
    }

    pub(super) fn record_step(&mut self, step: impl Into<String>) {
        self.steps.push(step.into());
    }

    #[allow(dead_code)]
    fn record_command(&mut self, cmd: &str, exit_code: i32, duration_ms: u128, stdout: &str, stderr: &str) {
        let mut msg = format!("Executed shell command: `{}` [Exit code: {} in {}ms]", cmd, exit_code, duration_ms);
        let clean_stdout = stdout.trim();
        let clean_stderr = stderr.trim();
        if !clean_stdout.is_empty() {
            let lines: Vec<&str> = clean_stdout.lines().take(20).collect();
            msg.push_str(&format!("\n   Stdout ({} lines):\n   {}", clean_stdout.lines().count(), lines.join("\n   ")));
        }
        if !clean_stderr.is_empty() {
            let lines: Vec<&str> = clean_stderr.lines().take(15).collect();
            msg.push_str(&format!("\n   Stderr ({} lines):\n   {}", clean_stderr.lines().count(), lines.join("\n   ")));
        }
        self.steps.push(msg);
    }

    pub(super) fn finish(self, success: bool, output: &str) -> PathBuf {
        let finished_at = chrono::Utc::now();
        let duration_ms = (finished_at - self.started_at).num_milliseconds().max(0);
        let duration_str = format!("{}ms ({}s)", duration_ms, (finished_at - self.started_at).num_seconds());
        let status = if success { "completed" } else { "failed" };

        corex_core::ForensicLogger::write_subagent_forensic_log(
            &self.task_id,
            self.role.as_str(),
            status,
            &self.started_at.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            &finished_at.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            &duration_str,
            &self.workspace.display().to_string(),
            &self.prompt,
            &self.steps,
            output,
        )
    }
}

pub(super) async fn run_subagent_task(
    task_id: &str,
    role: SubagentRole,
    prompt: &str,
    workspace: &Path,
    parent_allowed: &[String],
    extra_dirs: &[PathBuf],
) -> Result<String, String> {
    let mut trace = ForensicTrace::new(task_id, role, prompt, workspace);

    // Tests drive the real loop with an offline backend, so no test ever reaches the network.
    #[cfg(test)]
    let result = run_subagent_llm_loop_with(task_id, role, prompt, workspace, parent_allowed, extra_dirs, &OfflineChat, &mut trace).await;
    #[cfg(not(test))]
    let result = match get_subagent_llm_client(workspace) {
        Some(client) => {
            trace.record_step(format!("Starting autonomous LLM execution loop for subagent [{}]", role.as_str().to_uppercase()));
            run_subagent_llm_loop(task_id, role, prompt, workspace, parent_allowed, extra_dirs, &client, &mut trace)
                .await
                .map_err(|e| {
                    let err = format!("Subagent LLM execution failed: {}", e);
                    trace.record_step(&err);
                    err
                })
        }
        None => {
            let err = "No LLM client is available for subagent execution. Configure an API key (or a local model) before delegating tasks."
                .to_string();
            trace.record_step(&err);
            Err(err)
        }
    };

    let log_path = match &result {
        Ok(out) => trace.finish(true, out),
        Err(err) => trace.finish(false, err),
    };

    if let Ok(mut mgr) = get_subagent_manager().lock() {
        mgr.set_log_file(task_id, log_path);
    }

    result
}

/// Abstraction over the chat backend used by the subagent loop. It exists so the
/// turn-exhaustion path (budget nudge + forced synthesis) can be exercised deterministically
/// in tests via a mock, with zero network access and no side effects.
#[async_trait]
pub(super) trait SubagentChatBackend: Send + Sync {
    async fn stream_turn(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, String>;
}

#[async_trait]
impl SubagentChatBackend for LlmClient {
    async fn stream_turn(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, String> {
        self.stream_chat(messages, tools, cancel_token)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(test))]
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_subagent_llm_loop(
    task_id: &str,
    role: SubagentRole,
    prompt: &str,
    workspace: &Path,
    parent_allowed: &[String],
    extra_dirs: &[PathBuf],
    client: &LlmClient,
    trace: &mut ForensicTrace,
) -> Result<String, String> {
    run_subagent_llm_loop_with(task_id, role, prompt, workspace, parent_allowed, extra_dirs, client, trace).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_subagent_llm_loop_with<C: SubagentChatBackend>(
    task_id: &str,
    role: SubagentRole,
    prompt: &str,
    workspace: &Path,
    parent_allowed: &[String],
    extra_dirs: &[PathBuf],
    client: &C,
    trace: &mut ForensicTrace,
) -> Result<String, String> {
    let mut sub_reg = ToolRegistry::new();
    // Prevent recursive subagent spawning
    sub_reg.unregister("delegate_subagent");
    sub_reg.unregister("subagent");
    sub_reg.unregister("invoke_subagent");
    sub_reg.unregister("manage_subagents");
    sub_reg.unregister("subagents");
    sub_reg.unregister("ask_user");
    // Memory belongs to the user: a subagent reading untrusted content must not persist it.
    sub_reg.unregister("save_memory");
    // Images are queued process-wide for the main conversation, never for a subagent.
    sub_reg.unregister("read_image");
    sub_reg.unregister("activate_skill");

    // Scout and Researcher start without any user approval, so they must not be able to drive
    // the user's own background processes (type into a REPL/shell, kill a server).
    if matches!(role, SubagentRole::Scout | SubagentRole::Researcher) {
        sub_reg.unregister("write_background_input");
        sub_reg.unregister("kill_background_process");
        sub_reg.unregister("manage_task");
    }

    match role {
        SubagentRole::Scout => {
            sub_reg.unregister("write_file");
            sub_reg.unregister("edit");
            sub_reg.unregister("edit_file");
            sub_reg.unregister("replace");
            sub_reg.unregister("apply_patch");
        }
        SubagentRole::Tester => {
            sub_reg.unregister("write_file");
            sub_reg.unregister("edit");
            sub_reg.unregister("edit_file");
            sub_reg.unregister("replace");
            sub_reg.unregister("apply_patch");
            sub_reg.unregister("web_search");
            sub_reg.unregister("google_web_search");
            sub_reg.unregister("web_fetch");
        }
        SubagentRole::Researcher => {
            sub_reg.unregister("write_file");
            sub_reg.unregister("edit");
            sub_reg.unregister("edit_file");
            sub_reg.unregister("replace");
            sub_reg.unregister("apply_patch");
            sub_reg.unregister("run_shell_command");
            sub_reg.unregister("execute_command");
            sub_reg.unregister("bash");
        }
        SubagentRole::Worker => {
            // Worker retains full tool capabilities
        }
    }

    let tools_defs = sub_reg.list_definitions();

    // Subagents never get YOLO mode or the cached sudo password. They inherit the user's
    // approval rules; an approved Worker may additionally edit files inside the workspace
    // (the tools still refuse paths outside it, credentials and persistence vectors).
    let mut allowed_commands = parent_allowed.to_vec();
    if role == SubagentRole::Worker {
        allowed_commands.extend(["edit", "write_file", "apply_patch"].map(String::from));
    }
    let subagent_ctx = ToolContext {
        workspace_dir: workspace.to_path_buf(),
        yolo_mode: false,
        sudo_password: None,
        allowed_commands,
        extra_dirs: extra_dirs.to_vec(),
    };

    let role_instructions = match role {
        SubagentRole::Scout => {
            "You are Scout, an autonomous read-only repository and system exploration subagent for Corex.\n\
             Your responsibility: inspect directories, count items, search symbols, analyze AST/code signatures, check configurations, and inventory system resources accurately as requested.\n\
             Rules:\n\
             1. You have read-only access: read_file, read_many_files, grep_search, glob, list_directory, and run_shell_command (for read-only queries like find, wc, ls, du, git status).\n\
             2. NEVER create, write, edit, or delete any files.\n\
             3. Report real numbers, exact paths, and verified facts directly from the environment. Do NOT guess or hallucinate.\n\
             4. Once your investigation is complete, provide a structured and thorough final report answering the user's prompt directly in the language of the prompt."
        }
        SubagentRole::Tester => {
            "You are Tester, an autonomous test runner, compiler checker, and environment verifier subagent for Corex.\n\
             Your responsibility: verify compiler status, check tool versions, run test suites, or execute verification scripts as requested.\n\
             Rules:\n\
             1. You have tools: run_shell_command, read_file, read_many_files, grep_search, glob, list_directory.\n\
             2. Execute the required commands (e.g. `python3 --version`, `node --version`, `cargo test`, `pytest`, etc.) directly.\n\
             3. Report exact versions, exit codes, and test pass/fail results faithfully without inventing or assuming.\n\
             4. Once verification is complete, summarize all results in a clear and concise report in the language of the prompt."
        }
        SubagentRole::Researcher => {
            "You are Researcher, an autonomous technical researcher and documentation subagent for Corex.\n\
             Your responsibility: research technical specifications, protocols, documentation, and compare technologies/libraries.\n\
             Rules:\n\
             1. You have tools: web_search, web_fetch, read_file, read_many_files, grep_search, glob, list_directory.\n\
             2. Answer the user's specific research question directly (e.g. if asked to compare X vs Y, compare both thoroughly).\n\
             3. Provide detailed, factual, and verified explanations with citations where appropriate in the language of the prompt."
        }
        SubagentRole::Worker => {
            "You are Worker, an autonomous general-purpose task execution subagent for Corex.\n\
             Your responsibility: perform assigned development tasks, generate code, create/edit files, and run commands.\n\
             Rules:\n\
             1. You have full tool access: run_shell_command, read_file, write_file, edit, apply_patch, grep_search, glob, list_directory, web_search, web_fetch.\n\
             2. When asked to create or edit files, write the exact requested content to the exact specified paths (never duplicate directory paths or invent placeholder content).\n\
             3. Always verify files on disk or command outcomes after performing work.\n\
             4. When finished, provide a concise summary of what was completed and verified in the language of the prompt."
        }
    };

    let user_msg = format!("Working directory: {}\n\nTask instruction:\n{}", workspace.display(), prompt);
    let mut messages = vec![
        Message::system(role_instructions),
        Message::user(user_msg),
    ];

    let cancel_token = {
        let mgr = match get_subagent_manager().lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        mgr.get_cancel_token(task_id).unwrap_or_default()
    };
    let mut turns = 0;
    let mut last_assistant_text = String::new();
    let max_turns = max_turns_for_role(role);

    while turns < max_turns {
        // Check if task was cancelled before starting turn
        if cancel_token.is_cancelled() {
            trace.record_step("Subagent execution cancelled by user");
            return Err("Subagent task was cancelled".to_string());
        }
        {
            let mgr = match get_subagent_manager().lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            if let Some(t) = mgr.get_task(task_id) {
                if !t.status.is_active() {
                    cancel_token.cancel();
                    trace.record_step("Subagent execution cancelled by user");
                    return Err("Subagent task was cancelled".to_string());
                }
            }
        }

        turns += 1;
        trace.record_step(format!("Subagent turn {}/{}", turns, max_turns));

        // (A) Budget nudge: on the penultimate turn, warn the model that its tool-turn
        // budget is about to run out so it can close gracefully instead of opening yet
        // another line of investigation it cannot finish.
        if turns == max_turns - 1 {
            messages.push(build_budget_nudge());
            trace.record_step("Injected penultimate-turn budget nudge (stop tool use, prepare final report)");
        }

        let mut rx = client.stream_turn(messages.clone(), Some(tools_defs.clone()), cancel_token.clone()).await
            .map_err(|e| format!("LLM stream error: {}", e))?;

        let mut assistant_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut finish_reason: Option<String> = None;

        while let Some(evt) = rx.recv().await {
            if cancel_token.is_cancelled() {
                trace.record_step("Subagent execution cancelled during stream");
                return Err("Subagent task was cancelled".to_string());
            }
            match evt {
                StreamEvent::ContentDelta(delta) => {
                    assistant_text.push_str(&delta);
                }
                StreamEvent::ToolCallDelta { index, id, name, arguments, extra_content } => {
                    if index > 64 {
                        continue;
                    }
                    while tool_calls.len() <= index {
                        tool_calls.push(ToolCall {
                            id: String::new(),
                            call_type: "function".to_string(),
                            function: FunctionCall {
                                name: String::new(),
                                arguments: String::new(),
                            },
                            extra_content: None,
                        });
                    }
                    if let Some(i) = id {
                        tool_calls[index].id.push_str(&i);
                    }
                    if let Some(n) = name {
                        tool_calls[index].function.name.push_str(&n);
                    }
                    if let Some(a) = arguments {
                        tool_calls[index].function.arguments.push_str(&a);
                    }
                    if let Some(ec) = extra_content {
                        tool_calls[index].extra_content = Some(ec);
                    }
                }
                StreamEvent::Completed { finish_reason: Some(r) } => {
                    finish_reason = Some(r);
                }
                StreamEvent::Error(err) => {
                    trace.record_step(format!("Stream error: {}", err));
                    return Err(err);
                }
                _ => {}
            }
        }

        // Anthropic indexes tool calls by content block, which leaves empty placeholders.
        tool_calls.retain(|c| !c.function.name.trim().is_empty());
        let truncated = corex_core::is_truncated_finish(finish_reason.as_deref());

        if !assistant_text.trim().is_empty() {
            last_assistant_text = assistant_text.trim().to_string();
        }

        if tool_calls.is_empty() {
            let res = if assistant_text.trim().is_empty() {
                "Subagent completed task successfully.".to_string()
            } else {
                assistant_text.trim().to_string()
            };
            trace.record_step("Subagent completed task without requesting additional tool calls");
            return Ok(res);
        }

        // Add assistant turn with tool calls to context
        messages.push(Message {
            role: "assistant".to_string(),
            content: if assistant_text.trim().is_empty() {
                None
            } else {
                Some(MessageContent::Text(assistant_text.clone()))
            },
            reasoning_content: None,
            tool_calls: Some(tool_calls.clone()),
            tool_call_id: None,
            name: None,
            extra_content: None,
        });

        // Execute tool calls
        for call in tool_calls {
            if cancel_token.is_cancelled() {
                trace.record_step("Subagent execution cancelled before tool execution");
                return Err("Subagent task was cancelled".to_string());
            }
            let name = call.function.name.trim();
            let args_raw = &call.function.arguments;
            trace.record_step(format!("Subagent calling tool `{}` with args: {}", name, args_raw));

            let start = std::time::Instant::now();
            let (out_str, is_err) = if truncated {
                (corex_core::TRUNCATED_TOOL_CALL_ERROR.to_string(), true)
            } else {
                match corex_core::parse_lenient_json(args_raw) {
                    Ok(args_val) => {
                        if let Some(reason) = crate::policy::subagent_denial(role, &sub_reg, name, &args_val, &subagent_ctx) {
                            (reason, true)
                        } else {
                            match sub_reg.execute(name, args_val, &subagent_ctx).await {
                                Ok(res) => (res.output, res.is_error),
                                Err(e) => (format!("Error executing tool {}: {}", name, e), true),
                            }
                        }
                    }
                    Err(e) => (format!("Invalid arguments JSON for tool {}: {}", name, e), true),
                }
            };
            let duration_ms = start.elapsed().as_millis();
            trace.record_step(format!(
                "Tool `{}` completed in {}ms (is_error={}):\n{}",
                name, duration_ms, is_err, corex_core::truncate_ellipsis(&out_str, 500)
            ));

            let out_str = corex_core::spill_large_output(name, out_str);
            messages.push(Message::tool_response(call.id, out_str));
        }
    }

    // (B) The model never yielded a tool-free turn within the budget. Force one final
    // completion with NO tools available so it MUST synthesize a textual report from
    // everything already gathered, instead of silently discarding the investigation.
    trace.record_step("Maximum tool turns reached; forcing final synthesis without tools");
    messages.push(build_forced_synthesis_prompt());

    match client.stream_turn(messages.clone(), None, cancel_token.clone()).await {
        Ok(mut rx) => {
            let mut final_text = String::new();
            while let Some(evt) = rx.recv().await {
                match evt {
                    StreamEvent::ContentDelta(delta) => final_text.push_str(&delta),
                    StreamEvent::Error(err) => {
                        trace.record_step(format!("Final synthesis stream error: {}", err));
                        break;
                    }
                    _ => {}
                }
            }
            let final_text = final_text.trim();
            if !final_text.is_empty() {
                trace.record_step("Forced final synthesis produced a report after exhausting the tool-turn budget");
                return Ok(final_text.to_string());
            }
        }
        Err(e) => {
            trace.record_step(format!("Forced final synthesis failed: {}", e));
        }
    }

    // (C) Last-resort fallback that preserves whatever the model had gathered instead of
    // returning a generic string and losing the whole investigation.
    trace.record_step("Turn limit reached; returning archived context (last assistant text or generic notice)");
    Ok(resolve_exhausted_budget(&last_assistant_text))
}

/// (A) Penultimate-turn nudge. Injected into the conversation so the model becomes aware
/// that its tool-turn budget is nearly exhausted and closes gracefully with a report
/// rather than starting an investigation it cannot finish.
pub(super) fn build_budget_nudge() -> Message {
    Message::user(
        "⚠️ Turn budget notice: your tool-turn budget is almost exhausted. \
         Do NOT start any new line of investigation and do NOT request further tools. \
         Immediately deliver your complete final report using only the information you have already gathered.",
    )
}

/// (B) Forced-synthesis prompt. Sent together with a tool-less completion request once the
/// tool-turn budget is exhausted, guaranteeing a textual report instead of a bare cutoff.
pub(super) fn build_forced_synthesis_prompt() -> Message {
    Message::user(
        "You have reached the maximum number of tool turns. You can no longer call any tools. \
         Based ONLY on the information you have already gathered, produce your final structured \
         report now, answering the original task directly.",
    )
}

/// (C) Fallback used when the tool-turn budget is exhausted and no tool-less report was
/// produced. Preserves the model's last gathered text instead of discarding the work.
pub(super) fn resolve_exhausted_budget(last_assistant_text: &str) -> String {
    let trimmed = last_assistant_text.trim();
    if trimmed.is_empty() {
        DEFAULT_TURN_LIMIT_MESSAGE.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Default tool-turn budget per subagent role. Roles with broad, open-ended mandates (scout,
/// researcher) need far more turns than narrowly scoped ones (tester), which previously all
/// shared a single hard-coded budget of 12 and starved the exploratory roles.
pub(super) fn default_max_turns_for_role(role: SubagentRole) -> usize {
    match role {
        SubagentRole::Tester => 10,
        SubagentRole::Worker => 16,
        SubagentRole::Researcher => 20,
        SubagentRole::Scout => 24,
    }
}

/// Parses the optional `COREX_SUBAGENT_MAX_TURNS` override. Returns `None` for absent, empty,
/// non-numeric, or non-positive values so callers fall back to the role default.
pub(super) fn parse_turn_override(raw: Option<&str>) -> Option<usize> {
    raw.and_then(|v| v.trim().parse::<usize>().ok()).filter(|n| *n > 0)
}

/// Effective tool-turn budget: the role default, overridable globally via the
/// `COREX_SUBAGENT_MAX_TURNS` environment variable (must parse to a positive integer).
pub(super) fn max_turns_for_role(role: SubagentRole) -> usize {
    let raw = std::env::var("COREX_SUBAGENT_MAX_TURNS").ok();
    parse_turn_override(raw.as_deref()).unwrap_or_else(|| default_max_turns_for_role(role))
}

/// Generic notice returned only when the subagent produced no usable text at all.
pub(super) const DEFAULT_TURN_LIMIT_MESSAGE: &str =
    "Subagent reached maximum turn limit. No synthesized report was produced before the tool-turn budget was exhausted.";

/// Answers every turn with a short report and no tool calls, so tests that dispatch subagents
/// exercise the real loop and the manager without a model.
#[cfg(test)]
pub(super) struct OfflineChat;

#[cfg(test)]
#[async_trait]
impl SubagentChatBackend for OfflineChat {
    async fn stream_turn(
        &self,
        _messages: Vec<Message>,
        _tools: Option<Vec<ToolDefinition>>,
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, String> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let _ = tx.send(StreamEvent::ContentDelta(OFFLINE_REPORT.to_string())).await;
        let _ = tx.send(StreamEvent::Completed { finish_reason: None }).await;
        Ok(rx)
    }
}

#[cfg(test)]
pub(super) const OFFLINE_REPORT: &str = "Offline test backend: task finished without a model.";
