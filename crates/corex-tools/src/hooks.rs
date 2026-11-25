//! Lifecycle hooks (`PreToolUse`, `PostToolUse`, `OnStop`) configured in global `~/.corex/settings.json`.
//!
//! Hooks are strictly global-only (never read from repository `.corex/settings.json`) so cloning an
//! untrusted repository can never trigger arbitrary shell execution.

use std::path::Path;
#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
#[cfg(not(test))]
use std::time::{Instant, SystemTime};

use corex_core::config::{HookRule, HooksConfig};

#[cfg(not(test))]
struct CachedHooks {
    checked_at: Instant,
    mtime: Option<SystemTime>,
    len: u64,
    config: HooksConfig,
}

/// Loads global hooks from `~/.corex/settings.json`, cached in memory by `(mtime, size)` with a
/// 2-second stat window so tool bursts perform zero disk I/O on Tokio worker threads.
pub fn load_global_hooks() -> HooksConfig {
    #[cfg(test)]
    {
        return HooksConfig::default();
    }
    #[cfg(not(test))]
    {
        static CACHE: OnceLock<Mutex<Option<CachedHooks>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(None));
        if let Ok(guard) = cache.lock() {
            if let Some(ref entry) = *guard {
                if entry.checked_at.elapsed() < Duration::from_secs(2) {
                    return entry.config.clone();
                }
            }
        }

        let path = corex_core::config::corex_home().join("settings.json");
        let (mtime, len) = match std::fs::metadata(&path) {
            Ok(meta) => (meta.modified().ok(), meta.len()),
            Err(_) => {
                if let Ok(mut guard) = cache.lock() {
                    *guard = Some(CachedHooks {
                        checked_at: Instant::now(),
                        mtime: None,
                        len: 0,
                        config: HooksConfig::default(),
                    });
                }
                return HooksConfig::default();
            }
        };

        if let Ok(mut guard) = cache.lock() {
            if let Some(ref mut entry) = *guard {
                if entry.mtime == mtime && entry.len == len && (mtime.is_some() || len == 0) {
                    entry.checked_at = Instant::now();
                    return entry.config.clone();
                }
            }
        }

        let parsed = std::fs::read_to_string(&path)
            .ok()
            .filter(|raw| raw.contains("\"hooks\""))
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v.get("hooks").cloned())
            .and_then(|h| serde_json::from_value::<HooksConfig>(h).ok())
            .unwrap_or_default();

        if let Ok(mut guard) = cache.lock() {
            *guard = Some(CachedHooks {
                checked_at: Instant::now(),
                mtime,
                len,
                config: parsed.clone(),
            });
        }
        parsed
    }
}

/// Runs matching `PreToolUse` hooks. Any non-zero exit code blocks the tool call and returns
/// the hook's stderr/stdout reason.
pub async fn run_pre_tool_hooks(
    hooks: &HooksConfig,
    tool_name: &str,
    args: &serde_json::Value,
    workspace: &Path,
) -> Result<(), String> {
    if hooks.pre_tool_use.is_empty() {
        return Ok(());
    }
    let args_str = serde_json::to_string(args).unwrap_or_default();
    for rule in &hooks.pre_tool_use {
        if !rule.matches_tool(tool_name) || rule.command.trim().is_empty() {
            continue;
        }
        let outcome = exec_hook(rule, "PreToolUse", tool_name, &args_str, workspace).await;
        if !outcome.ok {
            let reason = if !outcome.stderr.is_empty() {
                outcome.stderr
            } else if !outcome.stdout.is_empty() {
                outcome.stdout
            } else {
                format!("Blocked by PreToolUse hook `{}` (exit {:?})", rule.command, outcome.code)
            };
            return Err(format!("PreToolUse hook blocked `{tool_name}`: {reason}"));
        }
    }
    Ok(())
}

/// Runs matching `PostToolUse` hooks after a tool finishes (e.g. `cargo fmt` after `edit`).
/// Returns optional combined output from hooks that printed something.
pub async fn run_post_tool_hooks(
    hooks: &HooksConfig,
    tool_name: &str,
    args: &serde_json::Value,
    workspace: &Path,
) -> Option<String> {
    if hooks.post_tool_use.is_empty() {
        return None;
    }
    let args_str = serde_json::to_string(args).unwrap_or_default();
    let mut notes = Vec::new();
    for rule in &hooks.post_tool_use {
        if !rule.matches_tool(tool_name) || rule.command.trim().is_empty() {
            continue;
        }
        let outcome = exec_hook(rule, "PostToolUse", tool_name, &args_str, workspace).await;
        if !outcome.ok && !outcome.stderr.is_empty() {
            notes.push(format!("[PostToolUse hook warning] {}", outcome.stderr));
        } else if !outcome.stdout.is_empty() {
            notes.push(format!("[PostToolUse hook] {}", outcome.stdout));
        }
    }
    if notes.is_empty() { None } else { Some(notes.join("\n")) }
}

/// Runs `OnStop` hooks in the background when a turn completes.
pub fn spawn_stop_hooks(hooks: HooksConfig, workspace: std::path::PathBuf) {
    if hooks.on_stop.is_empty() {
        return;
    }
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            for rule in &hooks.on_stop {
                if !rule.command.trim().is_empty() {
                    let _ = exec_hook(rule, "OnStop", "", "", &workspace).await;
                }
            }
        });
    }
}

struct HookOutcome {
    ok: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

async fn exec_hook(
    rule: &HookRule,
    event: &str,
    tool_name: &str,
    args_json: &str,
    workspace: &Path,
) -> HookOutcome {
    let timeout = Duration::from_secs(rule.timeout_secs.unwrap_or(10).clamp(1, 60));

    #[cfg(unix)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(&rule.command);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(&rule.command);
        c
    };

    cmd.current_dir(workspace)
        .env("COREX_HOOK_EVENT", event)
        .env("COREX_TOOL_NAME", tool_name)
        .env("COREX_TOOL_ARGS", corex_core::types::safe_truncate_str(args_json, 8192))
        .env("COREX_WORKSPACE", workspace.display().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => HookOutcome {
            ok: out.status.success(),
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        },
        Ok(Err(e)) => HookOutcome {
            ok: false,
            code: None,
            stdout: String::new(),
            stderr: format!("Failed to spawn hook: {e}"),
        },
        Err(_) => HookOutcome {
            ok: false,
            code: None,
            stdout: String::new(),
            stderr: format!("Hook timed out after {}s", timeout.as_secs()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pre_tool_hook_can_block_matching_tool_and_skip_non_matching() {
        let hooks = HooksConfig {
            pre_tool_use: vec![HookRule {
                matcher: "edit,write_file".to_string(),
                command: "echo 'no edits allowed' >&2; exit 2".to_string(),
                timeout_secs: Some(5),
            }],
            post_tool_use: Vec::new(),
            on_stop: Vec::new(),
        };
        let ws = std::env::temp_dir();

        // Non-matching tool is allowed.
        assert!(run_pre_tool_hooks(&hooks, "read_file", &serde_json::json!({}), &ws).await.is_ok());

        // Matching tool is blocked with stderr message.
        let err = run_pre_tool_hooks(&hooks, "edit", &serde_json::json!({"file_path":"a.rs"}), &ws)
            .await
            .unwrap_err();
        assert!(err.contains("no edits allowed"), "{err}");
    }

    #[tokio::test]
    async fn post_tool_hook_receives_env_vars_and_returns_output() {
        let hooks = HooksConfig {
            pre_tool_use: Vec::new(),
            post_tool_use: vec![HookRule {
                matcher: "*".to_string(),
                command: "echo \"$COREX_HOOK_EVENT:$COREX_TOOL_NAME\"".to_string(),
                timeout_secs: Some(5),
            }],
            on_stop: Vec::new(),
        };
        let ws = std::env::temp_dir();
        let note = run_post_tool_hooks(&hooks, "apply_patch", &serde_json::json!({}), &ws)
            .await
            .expect("hook should return output");
        assert!(note.contains("PostToolUse:apply_patch"), "{note}");
    }
}
