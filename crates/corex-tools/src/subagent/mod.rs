//! Tools for autonomous subagent delegation and lifecycle management.

use std::path::Path;
use std::sync::{Arc, Mutex};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use corex_core::subagent::{get_subagent_manager, SubagentRole, SubagentStatus, SubagentTask};
use corex_core::client::LlmClient;
use crate::types::{Tool, ToolContext, ToolOutput};

mod runner;
#[cfg(test)]
mod tests;

use runner::*;

static SUBAGENT_LLM_CLIENT: Mutex<Option<Arc<LlmClient>>> = Mutex::new(None);

pub fn set_subagent_llm_client(client: Arc<LlmClient>) {
    if let Ok(mut g) = SUBAGENT_LLM_CLIENT.lock() {
        *g = Some(client);
    }
}

pub fn get_subagent_llm_client(workspace: &Path) -> Option<Arc<LlmClient>> {
    if let Ok(g) = SUBAGENT_LLM_CLIENT.lock() {
        if let Some(ref c) = *g {
            return Some(c.clone());
        }
    }
    let cfg = corex_core::config::Config::load_with_workspace(Some(workspace));
    let (_, key) = cfg.endpoint();
    if !key.trim().is_empty() || cfg.model.starts_with("local") {
        Some(Arc::new(LlmClient::new(cfg)))
    } else {
        None
    }
}

/// Tool allowing the orchestrator agent to spawn specialized subagents.
pub struct DelegateSubagentTool;

#[async_trait]
impl Tool for DelegateSubagentTool {
    fn name(&self) -> &str {
        "delegate_subagent"
    }

    fn description(&self) -> &str {
        "Delegate a sub-task to a specialized autonomous agent running in the background (scout, tester, researcher, worker). The subagent runs concurrently with its own isolated context without consuming the main agent's context window."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "role": {
                    "type": "string",
                    "enum": ["scout", "tester", "researcher", "worker"],
                    "description": "Subagent role: 'scout' (read-only exploration), 'tester' (runs tests/linters in the background), 'researcher' (search and specifications), 'worker' (general autonomous task)."
                },
                "prompt": {
                    "type": "string",
                    "description": "A clear and complete instruction for the subagent to execute."
                },
                "background": {
                    "type": "boolean",
                    "description": "If true (default), the sub-task is delegated asynchronously in the background without blocking the turn. The subagent ID is returned immediately."
                }
            },
            "required": ["role", "prompt"]
        })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        // Worker and Tester run shell commands unattended, so the task itself is approved once.
        // Scout and Researcher are read-only: `subagent_denial` already limits them.
        if context.yolo_mode
            || context
                .allowed_commands
                .iter()
                .any(|c| matches!(c.as_str(), "delegate_subagent" | "invoke_subagent" | "subagent"))
        {
            return false;
        }
        let role = SubagentRole::parse(args.get("role").and_then(|r| r.as_str()).unwrap_or("worker"));
        matches!(role, SubagentRole::Worker | SubagentRole::Tester)
    }

    fn format_diff(&self, args: &serde_json::Value, _workspace: &std::path::Path) -> Option<String> {
        // Show the whole instruction: the user is approving all of it, not just its first line.
        let role = args.get("role").and_then(|r| r.as_str()).unwrap_or("worker");
        let prompt = args.get("prompt").and_then(|p| p.as_str()).unwrap_or("").trim();
        Some(format!("Subagent [{}]\n{}", role.to_uppercase(), prompt))
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let role_str = args.get("role").and_then(|r| r.as_str()).unwrap_or("worker");
        let prompt = match args.get("prompt").and_then(|p| p.as_str()) {
            Some(p) => p.trim(),
            None => return Ok(ToolOutput::error("Missing required parameter 'prompt'")),
        };

        if prompt.is_empty() {
            return Ok(ToolOutput::error("Subagent prompt cannot be empty"));
        }

        let role = SubagentRole::parse(role_str);
        let task_id = {
            let mut mgr = match get_subagent_manager().lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            mgr.register(role, prompt.to_string())
        };

        let is_background = args.get("background").and_then(|b| b.as_bool()).unwrap_or(true);
        if !is_background {
            let res = run_subagent_task(&task_id, role, prompt, &context.workspace_dir, &context.allowed_commands, &context.extra_dirs).await;
            let mut mgr = match get_subagent_manager().lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            match res {
                Ok(output) => {
                    mgr.complete_sync(&task_id, output.clone());
                    let summary = format!("{} Subagent [{}] completed.", role.icon(), role.as_str().to_uppercase());
                    return Ok(ToolOutput::success_with_summary(
                        format!("Subagent '{}' completed:\n\n{}", task_id, output),
                        summary,
                    ));
                }
                Err(err) => {
                    mgr.fail_sync(&task_id, err.clone());
                    return Ok(ToolOutput::error(format!("Subagent '{}' failed:\n{}", task_id, err)));
                }
            }
        }

        let id_clone = task_id.clone();
        let ws = context.workspace_dir.clone();
        let parent_allowed = context.allowed_commands.clone();
        let extra_dirs = context.extra_dirs.clone();
        let prompt_str = prompt.to_string();

        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

            {
                let mgr = match get_subagent_manager().lock() {
                    Ok(g) => g,
                    Err(e) => e.into_inner(),
                };
                if let Some(t) = mgr.get_task(&id_clone) {
                    if !t.status.is_active() {
                        return;
                    }
                }
            }

            let result = run_subagent_task(&id_clone, role, &prompt_str, &ws, &parent_allowed, &extra_dirs).await;

            let mut mgr = match get_subagent_manager().lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            if let Some(t) = mgr.get_task(&id_clone) {
                if t.status.is_active() {
                    match result {
                        Ok(output) => mgr.complete(&id_clone, output),
                        Err(err) => mgr.fail(&id_clone, err),
                    }
                }
            }
        });

        let log_dir = corex_core::ForensicLogger::get_subagents_log_dir();
        let log_file = log_dir.join(format!("{}.log", task_id));

        let summary = format!(
            "{} Subagent [{}] started (`{}`).",
            role.icon(),
            role.as_str().to_uppercase(),
            task_id
        );

        let output = format!(
            "Task ID: {}\nRole: {}\nStatus: RUNNING in background\nLog File: {}\nSubagent dispatched in background. Keep working, or call manage_subagents with action 'wait' to block until it reports (do not poll 'status' in a loop).",
            task_id, role.as_str(), log_file.display()
        );

        Ok(ToolOutput::success_with_summary(output, summary))
    }
}

/// Blocks until the watched subagents (`task_id`, or all of them) stop running, or `timeout`
/// passes, and reports each one with its output, so the model never has to poll in a loop.
async fn wait_for_subagents(task_id: Option<&str>, timeout: std::time::Duration) -> ToolOutput {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let (running, report) = {
            let mgr = match get_subagent_manager().lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            let watched: Vec<&SubagentTask> = match task_id {
                Some(id) => mgr.get_task(id).into_iter().collect(),
                None => mgr.list(),
            };
            if watched.is_empty() {
                return ToolOutput::error(match task_id {
                    Some(id) => format!("Subagent with ID '{}' not found", id),
                    None => "No subagents have been dispatched in this session.".to_string(),
                });
            }
            let running = watched
                .iter()
                .filter(|t| matches!(t.status, SubagentStatus::Pending | SubagentStatus::Running))
                .count();
            let report: Vec<String> = watched
                .iter()
                .map(|t| {
                    let body = if t.output.is_empty() { "(no output yet)" } else { t.output.as_str() };
                    format!(
                        "## {} {} ({}) - {} after {}\n{}",
                        t.role.icon(),
                        t.role.as_str().to_uppercase(),
                        t.id,
                        t.status.display_label(),
                        t.duration_str(),
                        corex_core::truncate_ellipsis(body, 6000)
                    )
                })
                .collect();
            (running, report.join("\n\n"))
        };
        if running == 0 {
            return ToolOutput::success(format!("All watched subagents have finished.\n\n{report}"));
        }
        if std::time::Instant::now() >= deadline {
            return ToolOutput::success(format!(
                "{running} subagent(s) still running after {}s. Call 'wait' again or continue with other work.\n\n{report}",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Tool for querying or canceling delegated subagents.
pub struct ManageSubagentsTool;

#[async_trait]
impl Tool for ManageSubagentsTool {
    fn name(&self) -> &str {
        "manage_subagents"
    }

    fn description(&self) -> &str {
        "Manage delegated background subagents: list them, check status, view forensic logs, cancel, or wait until they finish (returns their reports)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "status", "log", "cancel", "wait"],
                    "description": "Action to perform: 'list' (list subagents), 'status' (view output and details), 'log' (view step-by-step forensic log), 'cancel' (cancel subagent), 'wait' (block until the subagent, or every running one when task_id is omitted, finishes, and return the reports)."
                },
                "task_id": {
                    "type": "string",
                    "description": "Subagent task ID (required for 'status', 'log', and 'cancel'; optional for 'wait')."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "For 'wait': give up after this many seconds (default 120, max 600)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let action = args.get("action").and_then(|a| a.as_str()).unwrap_or("list");
        let task_id = args.get("task_id").and_then(|t| t.as_str()).map(str::trim);

        if action == "wait" {
            let timeout = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(120).clamp(1, 600);
            return Ok(wait_for_subagents(task_id, std::time::Duration::from_secs(timeout)).await);
        }

        let mgr = match get_subagent_manager().lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };

        match action {
            "list" => {
                let list = mgr.list();
                if list.is_empty() {
                    return Ok(ToolOutput::success("No subagents have been dispatched in this session."));
                }

                let mut out = format!("Subagents ({} total):\n", list.len());
                for t in list {
                    let log_hint = if let Some(ref p) = t.log_file {
                        format!(" | Log: {}", p.display())
                    } else {
                        String::new()
                    };
                    out.push_str(&format!(
                        "• [{}] {} (ID: {}) - Status: {} ({}{})\n  Prompt: {}\n",
                        t.role.icon(),
                        t.role.as_str().to_uppercase(),
                        t.id,
                        t.status.display_label(),
                        t.duration_str(),
                        log_hint,
                        corex_core::truncate_ellipsis(&t.prompt, 60)
                    ));
                }
                Ok(ToolOutput::success(out))
            }
            "status" => {
                let id = match task_id {
                    Some(i) => i.trim(),
                    None => return Ok(ToolOutput::error("Action 'status' requires 'task_id' parameter")),
                };

                if let Some(t) = mgr.get_task(id) {
                    let log_info = t.log_file.as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "N/A".to_string());

                    let out = format!(
                        "Subagent Details:\n\
                         ID: {}\n\
                         Role: {} {}\n\
                         Status: {}\n\
                         Duration: {}\n\
                         Log File: {}\n\
                         Prompt: {}\n\
                         Output:\n{}",
                        t.id,
                        t.role.icon(),
                        t.role.as_str().to_uppercase(),
                        t.status.display_label(),
                        t.duration_str(),
                        log_info,
                        t.prompt,
                        if t.output.is_empty() { "(Still running / No output yet)" } else { &t.output }
                    );
                    Ok(ToolOutput::success(out))
                } else {
                    Ok(ToolOutput::error(format!("Subagent with ID '{}' not found", id)))
                }
            }
            "log" => {
                let id = match task_id {
                    Some(i) => i.trim(),
                    None => return Ok(ToolOutput::error("Action 'log' requires 'task_id' parameter")),
                };

                if let Some(t) = mgr.get_task(id) {
                    let path = t.log_file.clone().unwrap_or_else(|| {
                        corex_core::ForensicLogger::get_subagents_log_dir().join(format!("{}.log", id))
                    });

                    if path.exists() {
                        match std::fs::read_to_string(&path) {
                            Ok(content) => Ok(ToolOutput::success(format!("Forensic Audit Log for Subagent '{}' ({}):\n\n{}", id, path.display(), content))),
                            Err(e) => Ok(ToolOutput::error(format!("Failed to read forensic log file '{}': {}", path.display(), e))),
                        }
                    } else {
                        Ok(ToolOutput::error(format!("Forensic log file for subagent '{}' not found yet at '{}'", id, path.display())))
                    }
                } else {
                    Ok(ToolOutput::error(format!("Subagent with ID '{}' not found", id)))
                }
            }
            "cancel" => {
                let id = match task_id {
                    Some(i) => i.trim(),
                    None => return Ok(ToolOutput::error("Action 'cancel' requires 'task_id' parameter")),
                };

                drop(mgr);
                let mut mgr_mut = match get_subagent_manager().lock() {
                    Ok(g) => g,
                    Err(e) => e.into_inner(),
                };

                if mgr_mut.cancel(id) {
                    Ok(ToolOutput::success(format!("Subagent '{}' cancelled successfully.", id)))
                } else {
                    Ok(ToolOutput::error(format!("Subagent '{}' could not be cancelled (not found or already finished).", id)))
                }
            }
            _ => Ok(ToolOutput::error(format!("Unknown action '{}'", action))),
        }
    }
}
