use anyhow::Result;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::NodeConfig;
use crate::ssh::{shell_quote_single, SshExecutor};

static NEXT_CMD_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

const DELIM: &str = "__CX_REMOTE_CWD_END__";

pub fn format_cd_expr(cwd: &str) -> String {
    if cwd == "~" || cwd.is_empty() {
        "cd ~".to_string()
    } else if let Some(rest) = cwd.strip_prefix("~/") {
        format!("cd ~/{}", shell_quote_single(rest))
    } else {
        format!("cd {}", shell_quote_single(cwd))
    }
}

pub fn build_wrapped_command(cwd_before: &str, cmd: &str) -> String {
    let cd_expr = format_cd_expr(cwd_before);
    format!(
        "export TERM=xterm-256color; export COLUMNS=80; export LINES=24; {} 2>/dev/null || cd ~; {}\n__ret=$?; echo \"{}\"; pwd; exit $__ret",
        cd_expr,
        cmd,
        DELIM
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandInitiator {
    User,
    AI,
}

impl CommandInitiator {
    pub fn badge(&self) -> &'static str {
        match self {
            CommandInitiator::User => "user",
            CommandInitiator::AI => "corex",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCommandRecord {
    pub id: String,
    pub command: String,
    pub initiator: CommandInitiator,
    pub cwd_before: String,
    pub cwd_after: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub timestamp: String,
    pub success: bool,
}

/// A persistent interactive remote session connected to a specific VPS / node.
/// Maintains the remote working directory (`current_dir`) across sequential commands
/// and keeps connection multiplexing alive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSession {
    pub node: NodeConfig,
    pub current_dir: String,
    pub history: Vec<RemoteCommandRecord>,
    pub is_connected: bool,
    pub connected_at: Option<String>,
    pub last_ping_ms: Option<u64>,
}

impl RemoteSession {
    pub fn new(node: NodeConfig) -> Self {
        Self {
            node,
            current_dir: "~".to_string(),
            history: Vec::new(),
            is_connected: false,
            connected_at: None,
            last_ping_ms: None,
        }
    }

    /// Tests the SSH connection and probes the initial remote working directory.
    pub async fn connect(&mut self, timeout_secs: u64) -> Result<u64> {
        let latency = SshExecutor::test_connection(&self.node, timeout_secs).await?;
        self.last_ping_ms = Some(latency);
        self.is_connected = true;
        self.connected_at = Some(Local::now().format("%H:%M:%S").to_string());

        // Probe initial remote directory
        if let Ok(out) = SshExecutor::execute(&self.node, "pwd", false, timeout_secs.min(6)).await {
            let pwd = out.stdout.trim();
            if !pwd.is_empty() {
                self.current_dir = pwd.to_string();
            }
        }

        Ok(latency)
    }

    pub fn disconnect(&mut self) {
        self.is_connected = false;
        self.connected_at = None;
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    pub fn push_history(&mut self, record: RemoteCommandRecord) {
        self.history.push(record);
        if self.history.len() > 100 {
            self.history.drain(..self.history.len() - 100);
        }
    }

    /// Executes a command in the remote session, preserving remote working directory
    /// and recording execution metrics and output.
    pub async fn execute_command(
        &mut self,
        cmd: &str,
        initiator: CommandInitiator,
        sudo: bool,
        timeout_secs: u64,
    ) -> Result<RemoteCommandRecord> {
        let timestamp = Local::now().format("%H:%M:%S").to_string();
        let cwd_before = self.current_dir.clone();

        let wrapped = build_wrapped_command(&cwd_before, cmd);

        let out = SshExecutor::execute(&self.node, &wrapped, sudo, timeout_secs).await?;

        // Split stdout by DELIM
        let (actual_stdout, cwd_after) = if let Some(idx) = out.stdout.find(DELIM) {
            let pre = out.stdout[..idx].trim_end().to_string();
            let post = out.stdout[idx + DELIM.len()..].trim();
            let new_pwd = post.lines().next().unwrap_or("").trim().to_string();
            let final_pwd = if !new_pwd.is_empty() { new_pwd } else { cwd_before.clone() };
            (pre, final_pwd)
        } else {
            (out.stdout.clone(), cwd_before.clone())
        };

        self.current_dir = cwd_after.clone();

        let seq = NEXT_CMD_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let record = RemoteCommandRecord {
            id: format!("cmd_{}", seq),
            command: cmd.to_string(),
            initiator,
            cwd_before,
            cwd_after,
            stdout: actual_stdout,
            stderr: out.stderr,
            exit_code: out.exit_code,
            duration_ms: out.duration_ms,
            timestamp,
            success: out.success,
        };

        self.push_history(record.clone());
        Ok(record)
    }

    /// Formats the last N lines of terminal output for visual display.
    pub fn live_output_lines(&self, max_lines: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for rec in &self.history {
            let status_badge = if rec.success { "✔" } else { "✘" };
            lines.push(format!(
                "[{}] {} {} $ {}",
                rec.timestamp,
                rec.initiator.badge(),
                status_badge,
                rec.command
            ));
            for out_l in rec.stdout.lines() {
                lines.push(format!("  {}", out_l));
            }
            if !rec.stderr.is_empty() {
                for err_l in rec.stderr.lines() {
                    lines.push(format!("  ! {}", err_l));
                }
            }
            lines.push(String::new());
        }

        if lines.len() > max_lines {
            lines.drain(..lines.len() - max_lines);
        }
        lines
    }

    /// Builds the remote shell script for autocompleting `token` given `cmd_prefix`.
    pub fn build_completion_script(&self, cmd_prefix: &str, token: &str) -> String {
        let quoted_token = shell_quote_single(token);
        let cd_expr = format_cd_expr(&self.current_dir);

        if cmd_prefix.trim().is_empty() && !token.starts_with('/') && !token.starts_with("./") {
            // First token / command completion: search aliases, builtins, and PATH commands via bash -i
            format!(
                "bash -i -c {} 2>/dev/null | awk '!seen[$0]++' | head -40",
                shell_quote_single(&format!("compgen -A command -- {}", quoted_token))
            )
        } else if cmd_prefix.trim().starts_with("cd") || cmd_prefix.trim().starts_with("pushd") {
            // Directory-only completion for cd/pushd (filter hidden files unless token explicitly starts with .)
            let hidden_check = if token.starts_with('.') || token.contains("/.") {
                ""
            } else {
                "case \"$f\" in .*|*/.*) continue ;; esac;"
            };
            format!(
                "{} 2>/dev/null || cd ~; compgen -A directory -- {} 2>/dev/null | head -40 | while IFS= read -r f; do [ -z \"$f\" ] && continue; {} echo \"$f/\"; done",
                cd_expr, quoted_token, hidden_check
            )
        } else {
            // File & directory completion (filter hidden files unless token explicitly starts with .)
            let hidden_check = if token.starts_with('.') || token.contains("/.") {
                ""
            } else {
                "case \"$f\" in .*|*/.*) continue ;; esac;"
            };
            format!(
                "{} 2>/dev/null || cd ~; compgen -A file -- {} 2>/dev/null | head -40 | while IFS= read -r f; do [ -z \"$f\" ] && continue; {} [ -d \"$f\" ] && echo \"$f/\" || echo \"$f\"; done",
                cd_expr, quoted_token, hidden_check
            )
        }
    }

    /// Queries the remote bash environment for autocompletion candidates matching `input`.
    /// Handles commands, subcommands, and paths/files relative to `self.current_dir`.
    pub async fn complete(&self, input: &str) -> Vec<String> {
        let (cmd_prefix, token) = match input.rfind(char::is_whitespace) {
            Some(pos) => (&input[..=pos], &input[pos + 1..]),
            None => ("", input),
        };

        let first_word = input.split_whitespace().next().unwrap_or("");

        // 1. Check known subcommands when typing the first argument of common tools
        if input.split_whitespace().count() <= 1 && !cmd_prefix.is_empty() {
            let subcmds: &[&str] = match first_word {
                "docker" => &[
                    "ps", "images", "run", "exec", "logs", "stop", "restart", "compose", "build",
                    "pull", "push", "rm", "rmi", "network", "volume", "stats", "top", "inspect",
                ],
                "systemctl" => &[
                    "status", "restart", "start", "stop", "reload", "enable", "disable",
                    "daemon-reload", "is-active", "is-enabled", "mask", "unmask",
                ],
                "git" => &[
                    "status", "diff", "log", "commit", "push", "pull", "checkout", "branch",
                    "add", "reset", "merge", "rebase", "stash", "fetch", "remote", "clone",
                ],
                "apt" | "apt-get" => &[
                    "update", "upgrade", "install", "remove", "purge", "autoremove", "clean",
                    "search", "show",
                ],
                "service" => &["status", "restart", "start", "stop", "reload"],
                "ufw" => &["status", "enable", "disable", "allow", "deny", "reload", "default"],
                _ => &[],
            };

            let matches: Vec<String> = subcmds
                .iter()
                .filter(|s| s.starts_with(token))
                .map(|s| s.to_string())
                .collect();

            if !matches.is_empty() {
                return matches;
            }
        }

        // 2. Build the remote compgen query
        let script = self.build_completion_script(cmd_prefix, token);

        if let Ok(out) = SshExecutor::execute(&self.node, &script, false, 3).await {
            let list: Vec<String> = out
                .stdout
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            list
        } else {
            Vec::new()
        }
    }

    /// Autocompletes the given command line input, returning the advanced line and candidates.
    pub async fn complete_line(&self, input: &str) -> CompletionResult {
        let (cmd_prefix, token) = match input.rfind(char::is_whitespace) {
            Some(pos) => (&input[..=pos], &input[pos + 1..]),
            None => ("", input),
        };

        let candidates = self.complete(input).await;
        if candidates.is_empty() {
            return CompletionResult {
                new_input: input.to_string(),
                candidates: Vec::new(),
                advanced: false,
            };
        }

        // Special case: if completing command name (first word) and token is an exact match
        // for an existing command/alias (e.g. "ls", "cd", "cat", "git"), complete by adding a space!
        if cmd_prefix.is_empty() && candidates.iter().any(|c| c == token) {
            return CompletionResult {
                new_input: format!("{} ", token),
                candidates,
                advanced: true,
            };
        }

        if candidates.len() == 1 {
            let m = &candidates[0];
            let new_input = if m.ends_with('/') {
                format!("{}{}", cmd_prefix, m)
            } else {
                format!("{}{} ", cmd_prefix, m)
            };
            return CompletionResult {
                new_input,
                candidates,
                advanced: true,
            };
        }

        let lcp = longest_common_prefix(&candidates);
        if lcp.len() > token.len() {
            let new_input = format!("{}{}", cmd_prefix, lcp);
            CompletionResult {
                new_input,
                candidates,
                advanced: true,
            }
        } else {
            CompletionResult {
                new_input: input.to_string(),
                candidates,
                advanced: false,
            }
        }
    }
}

/// Result of an autocompletion attempt on a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResult {
    pub new_input: String,
    pub candidates: Vec<String>,
    pub advanced: bool,
}

pub fn longest_common_prefix(items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut prefix = items[0].clone();
    for item in &items[1..] {
        while !item.starts_with(&prefix) {
            if prefix.is_empty() {
                return String::new();
            }
            prefix.pop();
        }
    }
    prefix
}

/// Global session manager that caches active live remote sessions per node ID.
#[derive(Default, Clone)]
pub struct FleetSessionManager {
    sessions: Arc<Mutex<std::collections::HashMap<String, RemoteSession>>>,
    active_node_id: Arc<Mutex<Option<String>>>,
}

impl FleetSessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get_session(&self, node_id: &str) -> Option<RemoteSession> {
        let lock = self.sessions.lock().await;
        lock.get(node_id).cloned()
    }

    pub async fn set_session(&self, node_id: String, mut session: RemoteSession) {
        let mut lock = self.sessions.lock().await;
        if !session.history.is_empty() {
            if let Some(existing) = lock.get(&node_id) {
                let mut merged = existing.history.clone();
                for rec in session.history {
                    if !merged.iter().any(|r| r.id == rec.id) {
                        merged.push(rec);
                    }
                }
                if merged.len() > 100 {
                    merged.drain(..merged.len() - 100);
                }
                session.history = merged;
            } else if session.history.len() > 100 {
                let excess = session.history.len() - 100;
                session.history.drain(..excess);
            }
        }
        lock.insert(node_id, session);
    }

    /// Atomically updates the stored session for `node_id` after a command execution,
    /// appending `record` without clobbering concurrent command history.
    pub async fn update_session_after_command(
        &self,
        node_id: &str,
        template_node: &NodeConfig,
        cwd_after: String,
        record: RemoteCommandRecord,
    ) -> RemoteSession {
        let mut lock = self.sessions.lock().await;
        let entry = lock
            .entry(node_id.to_string())
            .or_insert_with(|| RemoteSession::new(template_node.clone()));
        entry.is_connected = true;
        if entry.connected_at.is_none() {
            entry.connected_at = Some(Local::now().format("%H:%M:%S").to_string());
        }
        entry.current_dir = cwd_after;
        if !entry.history.iter().any(|r| r.id == record.id) {
            entry.push_history(record);
        }
        entry.clone()
    }

    pub async fn active_node(&self) -> Option<String> {
        let lock = self.active_node_id.lock().await;
        lock.clone()
    }

    pub async fn set_active_node(&self, id: Option<String>) {
        let mut lock = self.active_node_id.lock().await;
        *lock = id;
    }
}

static SESSION_MANAGER: std::sync::OnceLock<FleetSessionManager> = std::sync::OnceLock::new();

pub fn global_session_manager() -> &'static FleetSessionManager {
    SESSION_MANAGER.get_or_init(FleetSessionManager::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_node() -> NodeConfig {
        let mut node = NodeConfig::new("test-node", "127.0.0.1");
        node.user = "testuser".to_string();
        node.tags = vec!["web".to_string()];
        node
    }

    #[test]
    fn test_remote_session_initial_state() {
        let node = sample_node();
        let session = RemoteSession::new(node);
        assert_eq!(session.current_dir, "~");
        assert!(!session.is_connected);
        assert!(session.history.is_empty());
        assert_eq!(session.node.id, "test-node");
    }

    #[test]
    fn test_wrapped_command_trailing_comment_and_tilde_cwd() {
        let wrapped_tilde = build_wrapped_command("~", "echo hello # trailing comment");
        assert!(wrapped_tilde.contains("cd ~ 2>/dev/null || cd ~; echo hello # trailing comment\n__ret=$?;"));

        let wrapped_sub = build_wrapped_command("~/my project's dir", "pwd");
        assert!(wrapped_sub.contains("cd ~/'my project'\\''s dir' 2>/dev/null || cd ~;"));

        let wrapped_abs = build_wrapped_command("/var/www/app", "ls");
        assert!(wrapped_abs.contains("cd '/var/www/app' 2>/dev/null || cd ~;"));
    }

    #[test]
    fn test_completion_script_uses_shell_quote_single() {
        let mut session = RemoteSession::new(sample_node());
        session.current_dir = "~/app's dir".to_string();

        let cmd_script = session.build_completion_script("", "doc'ker");
        let expected_inner = format!("compgen -A command -- {}", shell_quote_single("doc'ker"));
        assert_eq!(
            cmd_script,
            format!(
                "bash -i -c {} 2>/dev/null | awk '!seen[$0]++' | head -40",
                shell_quote_single(&expected_inner)
            )
        );

        let dir_script = session.build_completion_script("cd ", "sub'dir");
        assert!(dir_script.starts_with("cd ~/'app'\\''s dir' 2>/dev/null || cd ~; compgen -A directory -- 'sub'\\''dir'"));

        let file_script = session.build_completion_script("cat ", "file'name");
        assert!(file_script.starts_with("cd ~/'app'\\''s dir' 2>/dev/null || cd ~; compgen -A file -- 'file'\\''name'"));
    }

    #[test]
    fn test_history_cap_at_100() {
        let mut session = RemoteSession::new(sample_node());
        for i in 1..=125 {
            session.push_history(RemoteCommandRecord {
                id: format!("cmd_{}", i),
                command: format!("echo {}", i),
                initiator: CommandInitiator::User,
                cwd_before: "~".to_string(),
                cwd_after: "~".to_string(),
                stdout: format!("{}", i),
                stderr: String::new(),
                exit_code: 0,
                duration_ms: 1,
                timestamp: "12:00:00".to_string(),
                success: true,
            });
        }
        assert_eq!(session.history.len(), 100);
        assert_eq!(session.history.first().unwrap().id, "cmd_26");
        assert_eq!(session.history.last().unwrap().id, "cmd_125");
    }

    #[test]
    fn test_live_output_lines_formatting() {
        let node = sample_node();
        let mut session = RemoteSession::new(node);
        session.history.push(RemoteCommandRecord {
            id: "cmd_1".to_string(),
            command: "uptime".to_string(),
            initiator: CommandInitiator::User,
            cwd_before: "~".to_string(),
            cwd_after: "~".to_string(),
            stdout: "up 42 days, 1 user".to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration_ms: 15,
            timestamp: "12:00:00".to_string(),
            success: true,
        });

        session.history.push(RemoteCommandRecord {
            id: "cmd_2".to_string(),
            command: "cat /nonexistent".to_string(),
            initiator: CommandInitiator::AI,
            cwd_before: "~".to_string(),
            cwd_after: "~".to_string(),
            stdout: String::new(),
            stderr: "No such file or directory".to_string(),
            exit_code: 1,
            duration_ms: 8,
            timestamp: "12:00:05".to_string(),
            success: false,
        });

        let lines = session.live_output_lines(20);
        let text = lines.join("\n");
        assert!(text.contains("user"));
        assert!(text.contains("uptime"));
        assert!(text.contains("up 42 days"));
        assert!(text.contains("corex"));
        assert!(text.contains("! No such file"));
    }

    #[tokio::test]
    async fn test_fleet_session_manager() {
        let mgr = FleetSessionManager::new();
        assert!(mgr.active_node().await.is_none());

        mgr.set_active_node(Some("srv1".to_string())).await;
        assert_eq!(mgr.active_node().await.as_deref(), Some("srv1"));

        let node = sample_node();
        let sess = RemoteSession::new(node.clone());
        mgr.set_session("srv1".to_string(), sess).await;

        let retrieved = mgr.get_session("srv1").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().node.id, "test-node");

        // Verify concurrent updates preserve history
        let rec1 = RemoteCommandRecord {
            id: "cmd_a".to_string(),
            command: "ls".to_string(),
            initiator: CommandInitiator::User,
            cwd_before: "~".to_string(),
            cwd_after: "/var".to_string(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            duration_ms: 5,
            timestamp: "12:00:00".to_string(),
            success: true,
        };
        let rec2 = RemoteCommandRecord {
            id: "cmd_b".to_string(),
            command: "pwd".to_string(),
            initiator: CommandInitiator::AI,
            cwd_before: "/var".to_string(),
            cwd_after: "/var/log".to_string(),
            stdout: "/var/log".to_string(),
            stderr: String::new(),
            exit_code: 0,
            duration_ms: 5,
            timestamp: "12:00:01".to_string(),
            success: true,
        };
        mgr.update_session_after_command("srv1", &node, "/var".to_string(), rec1).await;
        let updated = mgr
            .update_session_after_command("srv1", &node, "/var/log".to_string(), rec2)
            .await;
        assert_eq!(updated.history.len(), 2);
        assert_eq!(updated.current_dir, "/var/log");
    }

    #[test]
    fn test_longest_common_prefix() {
        assert_eq!(longest_common_prefix(&[]), "");
        assert_eq!(longest_common_prefix(&["ubuntu/".to_string()]), "ubuntu/");
        assert_eq!(
            longest_common_prefix(&[
                "triade-admin-web/".to_string(),
                "triade-web/".to_string(),
            ]),
            "triade-"
        );
        assert_eq!(
            longest_common_prefix(&["alpha".to_string(), "beta".to_string()]),
            ""
        );
    }
}


