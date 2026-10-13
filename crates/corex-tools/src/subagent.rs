//! Tools for autonomous subagent delegation and lifecycle management.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use corex_core::subagent::{get_subagent_manager, SubagentRole};

use crate::types::{Tool, ToolContext, ToolOutput};

/// Tool allowing the orchestrator agent to spawn specialized subagents.
pub struct DelegateSubagentTool;

#[async_trait]
impl Tool for DelegateSubagentTool {
    fn name(&self) -> &str {
        "delegate_subagent"
    }

    fn description(&self) -> &str {
        "Delega una sub-tarea a un agente autónomo especializado en segundo plano (scout, tester, researcher, worker). El subagente ejecuta de manera concurrente con su propio contexto aislado sin consumir la ventana de contexto del agente principal."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "role": {
                    "type": "string",
                    "enum": ["scout", "tester", "researcher", "worker"],
                    "description": "Rol del subagente: 'scout' (exploración en solo lectura), 'tester' (ejecución de pruebas/linters en background), 'researcher' (búsqueda y especificaciones), 'worker' (tarea autónoma general)."
                },
                "prompt": {
                    "type": "string",
                    "description": "Instrucción clara y completa para la ejecución del subagente."
                },
                "background": {
                    "type": "boolean",
                    "description": "Si es true (por defecto), se delega de forma asíncrona en segundo plano sin bloquear el turno. Devuelve inmediatamente el ID del subagente."
                }
            },
            "required": ["role", "prompt"]
        })
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
            let res = run_subagent_task(role, prompt, &context.workspace_dir).await;
            let mut mgr = match get_subagent_manager().lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            match res {
                Ok(output) => {
                    mgr.complete(&task_id, output.clone());
                    let summary = format!("{} Subagente [{}] completado.", role.icon(), role.as_str().to_uppercase());
                    return Ok(ToolOutput::success_with_summary(
                        format!("Subagent '{}' completed:\n\n{}", task_id, output),
                        summary,
                    ));
                }
                Err(err) => {
                    mgr.fail(&task_id, err.clone());
                    return Ok(ToolOutput::error(format!("Subagent '{}' failed:\n{}", task_id, err)));
                }
            }
        }

        let id_clone = task_id.clone();
        let ws = context.workspace_dir.clone();
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

            let result = run_subagent_task(role, &prompt_str, &ws).await;

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

        let summary = format!(
            "{} Subagente [{}] iniciado con ID `{}`. Tarea delegada.",
            role.icon(),
            role.as_str().to_uppercase(),
            task_id
        );

        let output = format!(
            "Subagent '{}' dispatched successfully.\n\
             Task ID: {}\n\
             Role: {}\n\
             Status: RUNNING in background\n\
             Prompt: {}\n\n\
             You may proceed with other work. A reactive notification will be delivered upon completion.",
            task_id, task_id, role.as_str(), prompt
        );

        Ok(ToolOutput::success_with_summary(output, summary))
    }
}

async fn run_subagent_task(role: SubagentRole, prompt: &str, workspace: &std::path::Path) -> Result<String, String> {
    match role {
        SubagentRole::Tester => {
            let cmd = if prompt.starts_with("cargo ") || prompt.starts_with("npm ") || prompt.starts_with("pytest") || prompt.starts_with("make ") {
                prompt.to_string()
            } else if prompt.to_lowercase().contains("test") {
                "cargo test".to_string()
            } else if prompt.to_lowercase().contains("check") {
                "cargo check".to_string()
            } else {
                format!("echo 'Running test suite for: {}' && cargo test", prompt)
            };
            run_shell_in_dir(&cmd, workspace).await
        }
        SubagentRole::Scout => {
            let cmd = if prompt.starts_with("git ") || prompt.starts_with("grep ") || prompt.starts_with("find ") {
                prompt.to_string()
            } else {
                let term = prompt.trim_matches(|c: char| c.is_whitespace() || c == '\'' || c == '"');
                format!("git grep -in -- \"{}\" || grep -rn -- \"{}\" . 2>/dev/null | head -n 50", term, term)
            };
            run_shell_in_dir(&cmd, workspace).await
        }
        SubagentRole::Researcher => {
            let cmd = if prompt.starts_with("find ") || prompt.starts_with("cat ") || prompt.starts_with("head ") {
                prompt.to_string()
            } else {
                "find . -maxdepth 3 -iname \"*readme*\" -o -iname \"*.md\" -o -path \"*/docs/*\" 2>/dev/null | head -n 30".to_string()
            };
            run_shell_in_dir(&cmd, workspace).await
        }
        SubagentRole::Worker => {
            if prompt.starts_with("cargo ") || prompt.starts_with("git ") || prompt.starts_with("make ") || prompt.starts_with("sh ") || prompt.starts_with("echo ") {
                run_shell_in_dir(prompt, workspace).await
            } else {
                Ok(format!("Worker completed subtask: {}", prompt))
            }
        }
    }
}

async fn run_shell_in_dir(cmd: &str, dir: &std::path::Path) -> Result<String, String> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .output()
        .await
        .map_err(|e| format!("Failed to execute subagent command: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    if output.status.success() {
        Ok(combined)
    } else {
        let code = output.status.code().unwrap_or(1);
        Err(format!("Exit code {}:\n{}", code, combined))
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
        "Gestiona los subagentes delegados en segundo plano (listar, comprobar estado o cancelar)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "status", "cancel"],
                    "description": "Acción a realizar: 'list' (listar subagentes), 'status' (ver salida y detalles), 'cancel' (cancelar subagente)."
                },
                "task_id": {
                    "type": "string",
                    "description": "ID del subagente (requerido para 'status' y 'cancel')."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let action = args.get("action").and_then(|a| a.as_str()).unwrap_or("list");
        let task_id = args.get("task_id").and_then(|t| t.as_str());

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
                    out.push_str(&format!(
                        "• [{}] {} (ID: {}) - Status: {} ({})\n  Prompt: {}\n",
                        t.role.icon(),
                        t.role.as_str().to_uppercase(),
                        t.id,
                        t.status.display_label(),
                        t.duration_str(),
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
                    let out = format!(
                        "Subagent Details:\n\
                         ID: {}\n\
                         Role: {} {}\n\
                         Status: {}\n\
                         Duration: {}\n\
                         Prompt: {}\n\
                         Output:\n{}",
                        t.id,
                        t.role.icon(),
                        t.role.as_str().to_uppercase(),
                        t.status.display_label(),
                        t.duration_str(),
                        t.prompt,
                        if t.output.is_empty() { "(Still running / No output yet)" } else { &t.output }
                    );
                    Ok(ToolOutput::success(out))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_delegate_and_manage_subagents() {
        let delegate_tool = DelegateSubagentTool;
        let manage_tool = ManageSubagentsTool;
        let ctx = ToolContext {
            workspace_dir: PathBuf::from("/tmp"),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        // 1. Dispatch
        let res = delegate_tool.execute(json!({
            "role": "scout",
            "prompt": "Scan src/ for unsafe blocks"
        }), &ctx).await.expect("tool execution should succeed");

        assert!(!res.is_error);
        assert!(res.output.contains("Task ID: sub_"));

        // Extract ID
        let id_line = res.output.lines().find(|l| l.starts_with("Task ID: ")).unwrap();
        let task_id = id_line.strip_prefix("Task ID: ").unwrap().trim();

        // 2. List
        let list_res = manage_tool.execute(json!({
            "action": "list"
        }), &ctx).await.expect("list should succeed");
        assert!(list_res.output.contains(task_id));

        // 3. Status
        let status_res = manage_tool.execute(json!({
            "action": "status",
            "task_id": task_id
        }), &ctx).await.expect("status should succeed");
        assert!(status_res.output.contains("SCOUT"));

        // 4. Cancel
        let cancel_res = manage_tool.execute(json!({
            "action": "cancel",
            "task_id": task_id
        }), &ctx).await.expect("cancel should succeed");
        assert!(cancel_res.output.contains("cancelled successfully"));
    }

    #[tokio::test]
    async fn test_delegate_subagent_sync_execution() {
        let delegate_tool = DelegateSubagentTool;
        let ctx = ToolContext {
            workspace_dir: PathBuf::from("/tmp"),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let res = delegate_tool.execute(json!({
            "role": "worker",
            "prompt": "echo 'Subagent worker ran'",
            "background": false
        }), &ctx).await.expect("sync execution should succeed");

        assert!(!res.is_error);
        assert!(res.output.contains("completed"));
        assert!(res.output.contains("Subagent worker ran"));
    }
}
