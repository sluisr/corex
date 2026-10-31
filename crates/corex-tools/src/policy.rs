//! Whether a tool call runs, waits for the user, or is refused. The TUI, the headless CLI and
//! the subagents all decide here, so a permission rule lives in exactly one place. Each tool's
//! own `needs_confirmation` stays the input: it knows what its arguments touch.

use corex_core::subagent::SubagentRole;

use crate::registry::ToolRegistry;
use crate::types::ToolContext;

/// What to do with a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Run,
    Ask,
}

/// The verdict for a call made by the main agent. `ctx.yolo_mode` is auto-approve; `plan` is
/// plan mode, which holds anything that changes things even when auto-approve is on.
pub fn verdict(registry: &ToolRegistry, name: &str, args: &serde_json::Value, ctx: &ToolContext, plan: bool) -> Verdict {
    let tool_asks = || registry.get(name).is_some_and(|t| t.needs_confirmation(args, ctx));
    let ask = (plan && (plan_mode_requires_approval(name, args) || tool_asks()))
        || needs_approval_even_in_yolo(name, args)
        || (!ctx.yolo_mode && tool_asks());
    if ask {
        Verdict::Ask
    } else {
        Verdict::Run
    }
}

/// A shell command that asks for root, which goes through the sudo password dialog.
pub fn needs_sudo(name: &str, args: &serde_json::Value) -> bool {
    name == "run_shell_command"
        && args.get("command").and_then(|c| c.as_str()).is_some_and(crate::shell::command_requires_sudo)
}

/// Calls that wait for the user even with auto-approve (YOLO) on: shell commands aimed at
/// credentials or at files that run code by themselves (git hooks, shell startup files, cron),
/// and notes for the global memory that every future session loads.
pub fn needs_approval_even_in_yolo(name: &str, args: &serde_json::Value) -> bool {
    match name {
        "run_shell_command" => args
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(crate::shell::command_references_sensitive_or_protected_path),
        "save_memory" => args.get("scope").and_then(|v| v.as_str()).unwrap_or("global") != "project",
        _ => false,
    }
}

/// Tools that only change things, so plan mode does not even offer them to the model.
pub fn hidden_in_plan_mode(name: &str) -> bool {
    matches!(
        name,
        "write_file"
            | "edit"
            | "apply_patch"
            | "save_memory"
            | "write_background_input"
            | "kill_background_process"
            | "delegate_subagent"
    ) || name.starts_with("mcp_")
}

/// Whether plan mode holds this call for the user, whatever YOLO or the allowed commands say:
/// anything that writes, plus shell commands that are not known to be read-only and the task
/// actions that drive or stop a process.
pub fn plan_mode_requires_approval(name: &str, args: &serde_json::Value) -> bool {
    let str_arg = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("");
    match name {
        "run_shell_command" => !crate::shell::is_known_safe_command(str_arg("command")),
        "manage_task" => matches!(str_arg("action").to_ascii_lowercase().as_str(), "send_input" | "kill"),
        _ => hidden_in_plan_mode(name),
    }
}

/// Subagents cannot show confirmation dialogs, so every call the main agent would have to ask
/// the user about is decided here instead. Returns the denial message, or `None` to run it.
///
/// - Scout / Researcher run without any approval: anything that would need confirmation is denied.
/// - Worker / Tester shell commands run without asking, but privilege escalation, sensitive or
///   protected paths and a working directory outside the workspace stay denied.
pub fn subagent_denial(
    role: SubagentRole,
    registry: &ToolRegistry,
    name: &str,
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> Option<String> {
    let tool = registry.get(name)?;
    let is_shell = matches!(name, "run_shell_command" | "execute_command" | "bash");
    if is_shell {
        let cmd = args
            .get("command")
            .or_else(|| args.get("cmd"))
            .or_else(|| args.get("CommandLine"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if crate::shell::command_requires_sudo(cmd) {
            return Some(
                "Access denied: subagents cannot run privileged (sudo/doas/pkexec) commands. Report the command to the main agent instead."
                    .to_string(),
            );
        }
        if crate::shell::command_references_sensitive_or_protected_path(cmd) {
            return Some(
                "Access denied: subagents cannot run commands targeting sensitive credentials, protected config/hook paths, or destructive system targets."
                    .to_string(),
            );
        }
        if let Some(dir) = args
            .get("dir_path")
            .or_else(|| args.get("cwd"))
            .or_else(|| args.get("Cwd"))
            .or_else(|| args.get("workdir"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            let target = crate::fs_tools::resolve_path(&ctx.workspace_dir, dir);
            if !crate::fs_tools::is_within_workspace(&target, &ctx.workspace_dir) {
                return Some(
                    "Access denied: subagents cannot run shell commands with a working directory outside the workspace."
                        .to_string(),
                );
            }
        }
        if matches!(role, SubagentRole::Worker | SubagentRole::Tester) {
            return None;
        }
    }
    if tool.needs_confirmation(args, ctx) {
        return Some(match role {
            SubagentRole::Scout | SubagentRole::Researcher => format!(
                "Access denied: the {} subagent is read-only and may only perform actions that need no user approval (`{}` would).",
                role.as_str(),
                name
            ),
            _ => format!(
                "Access denied: `{}` needs user approval (outside the workspace, credentials, or an unapproved host). Ask the main agent to do it.",
                name
            ),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(yolo: bool) -> ToolContext {
        ToolContext {
            workspace_dir: std::env::temp_dir(),
            yolo_mode: yolo,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        }
    }

    #[test]
    fn one_matrix_for_normal_yolo_and_plan_mode() {
        let reg = ToolRegistry::new();
        let read = ("read_file", json!({"file_path": "a.txt"}));
        let write = ("write_file", json!({"file_path": "a.txt", "content": "x"}));
        let ls = ("run_shell_command", json!({"command": "ls"}));
        let wipe = ("run_shell_command", json!({"command": "rm -rf target"}));
        let key = ("run_shell_command", json!({"command": "cat ~/.ssh/id_rsa"}));
        let global = ("save_memory", json!({"fact": "x"}));
        let project = ("save_memory", json!({"fact": "x", "scope": "project"}));

        // (call, normal, yolo, plan with yolo on)
        let table = [
            (&read, Verdict::Run, Verdict::Run, Verdict::Run),
            (&write, Verdict::Ask, Verdict::Run, Verdict::Ask),
            (&ls, Verdict::Run, Verdict::Run, Verdict::Run),
            (&wipe, Verdict::Ask, Verdict::Run, Verdict::Ask),
            (&key, Verdict::Ask, Verdict::Ask, Verdict::Ask),
            (&global, Verdict::Ask, Verdict::Ask, Verdict::Ask),
            (&project, Verdict::Ask, Verdict::Run, Verdict::Ask),
        ];
        for ((name, args), normal, yolo, plan) in table {
            assert_eq!(verdict(&reg, name, args, &ctx(false), false), normal, "{name} {args} normal");
            assert_eq!(verdict(&reg, name, args, &ctx(true), false), yolo, "{name} {args} yolo");
            assert_eq!(verdict(&reg, name, args, &ctx(true), true), plan, "{name} {args} plan");
        }
    }

    #[test]
    fn sudo_goes_to_its_own_dialog() {
        assert!(needs_sudo("run_shell_command", &json!({"command": "sudo apt update"})));
        assert!(!needs_sudo("run_shell_command", &json!({"command": "apt list"})));
        assert!(!needs_sudo("read_file", &json!({"command": "sudo x"})));
    }
}
