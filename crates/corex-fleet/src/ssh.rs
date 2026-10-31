use std::process::Stdio;
use std::time::{Duration, Instant};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::task::JoinSet;

use crate::config::NodeConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandRiskLevel {
    Safe,
    Mutating,
    Dangerous,
}

impl CommandRiskLevel {
    pub fn badge(&self) -> &'static str {
        match self {
            CommandRiskLevel::Safe => "SAFE",
            CommandRiskLevel::Mutating => "MUTATING",
            CommandRiskLevel::Dangerous => "DANGEROUS",
        }
    }
}

pub fn shell_quote_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

const SENSITIVE_PATHS: &[&str] = &[
    "/etc/shadow",
    "/etc/gshadow",
    "/proc/self/environ",
    "/environ",
    ".ssh/",
    ".ssh",
    ".gnupg",
    ".kube",
    ".aws",
    ".corex/",
    ".corex",
    ".deepseek/",
    ".deepseek",
    ".askpass_cred",
    ".aws/credentials",
    ".env",
];

const SAFE_PIPE_FILTERS: &[&str] = &[
    "head", "tail", "grep", "egrep", "fgrep", "wc", "sort", "uniq", "cut", "tr", "awk",
];

/// Commands that merely run another command. If one leads a segment, the safe-list match on its
/// name must not apply — the wrapped command is what actually executes (`env rm -rf /var/lib`).
const COMMAND_WRAPPERS: &[&str] = &[
    "env", "sudo", "doas", "pkexec", "nice", "nohup", "setsid", "timeout", "stdbuf", "watch",
    "xargs", "nsenter", "chroot", "unshare", "time", "ionice", "taskset", "exec",
];

/// True if `needle` appears in `haystack` bounded by non-alphanumeric characters on both sides,
/// so `dd` matches `dd if=…` but not `ip a**dd**r`. An empty needle never matches.
fn contains_at_word_boundary(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let needle_starts_word = needle.chars().next().is_some_and(&is_word);
    let needle_ends_word = needle.chars().next_back().is_some_and(&is_word);
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let left_ok = !needle_starts_word
            || start == 0
            || !(bytes[start - 1] as char).is_ascii_alphanumeric() && bytes[start - 1] != b'_';
        let right_ok = !needle_ends_word
            || end == haystack.len()
            || !(bytes[end] as char).is_ascii_alphanumeric() && bytes[end] != b'_';
        if left_ok && right_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

fn has_dangerous_flags_on_safe_cmd(seg: &str, tokens: &[&str]) -> bool {
    let Some(&first) = tokens.first() else {
        return false;
    };
    let args = &tokens[1..];
    // A wrapper around a safe-listed name still runs an arbitrary payload: never treat it as safe.
    if COMMAND_WRAPPERS.contains(&first) {
        return true;
    }
    match first {
        "find" => args.iter().any(|t| {
            matches!(
                *t,
                "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
            ) || t.starts_with("-fprint")
                || t.starts_with("-fls")
        }),
        "sed" => args.iter().any(|t| {
            *t == "-i"
                || t.starts_with("-i")
                || *t == "--in-place"
                || t.starts_with("--in-place=")
                || *t == "-f"
                || t.starts_with("-f")
                || *t == "--file"
                || t.starts_with("--file=")
        }),
        "git" => args.iter().any(|t| {
            *t == "--output"
                || t.starts_with("--output=")
                || *t == "--ext-diff"
                || *t == "--upload-pack"
                || t.starts_with("--upload-pack=")
                || t.starts_with("-c")
                || t.starts_with("-C")
        }),
        "uniq" => args.iter().filter(|t| !t.starts_with('-')).count() > 1,
        "sort" => args.iter().any(|t| {
            (t.starts_with('-') && !t.starts_with("--") && t[1..].chars().take_while(|c| *c != '=').any(|c| c == 'o'))
                || t.split('=').next().map(|n| "--output".starts_with(n) && n.len() > 3).unwrap_or(false)
                || t.split('=').next().map(|n| "--compress-program".starts_with(n) && n.len() > 3).unwrap_or(false)
        }),
        "awk" => {
            let no_spaces: String = seg.chars().filter(|c| !c.is_whitespace()).collect();
            no_spaces.contains("system(")
                || seg.contains("getline")
                || args.iter().any(|t| *t == "-f" || t.starts_with("--file"))
        }
        // `ip`/`nmcli` mutate the network (can cut off the SSH session itself).
        "ip" | "nmcli" => args.iter().any(|t| {
            matches!(*t, "set" | "add" | "del" | "delete" | "change" | "replace" | "flush" | "up" | "down")
        }),
        // Host identity / clock / firewall mutations.
        "hostname" => args.iter().any(|t| !t.starts_with('-')) || args.iter().any(|t| *t == "-b" || *t == "-F" || t.starts_with("--file")),
        "date" => args.iter().any(|t| *t == "-s" || t.starts_with("--set")),
        "iptables" | "ip6tables" | "nft" | "ufw" => true,
        "systemctl" => args.iter().any(|t| {
            matches!(
                *t,
                "start"
                    | "stop"
                    | "restart"
                    | "reload"
                    | "try-restart"
                    | "enable"
                    | "disable"
                    | "mask"
                    | "unmask"
                    | "daemon-reload"
                    | "edit"
                    | "kill"
                    | "reset-failed"
                    | "poweroff"
                    | "reboot"
                    | "halt"
                    | "isolate"
                    | "default"
                    | "rescue"
                    | "emergency"
                    | "set-default"
                    | "set-environment"
                    | "unset-environment"
                    | "set-property"
                    | "switch-root"
                    | "kexec"
                    | "suspend"
                    | "hibernate"
            )
        }),
        "journalctl" => args.iter().any(|t| {
            let name = t.split('=').next().unwrap_or(t);
            name.starts_with("--vacuum") || matches!(name, "--rotate" | "--flush" | "--relinquish-var" | "--sync")
        }),
        "hostnamectl" => args.iter().any(|t| *t == "set-hostname" || t.starts_with("set-")),
        "timedatectl" => args.iter().any(|t| t.starts_with("set-")),
        "localectl" => args.iter().any(|t| t.starts_with("set-")),
        _ => false,
    }
}

pub fn assess_command_risk(cmd: &str, safe_list: &[String], dangerous_list: &[String]) -> CommandRiskLevel {
    // The shell joins `"re""start"` into `restart`: judge the command with its quotes removed too.
    let dequoted: String = cmd.chars().filter(|c| *c != '"' && *c != '\'').collect();
    let rank = |r: CommandRiskLevel| match r {
        CommandRiskLevel::Safe => 0,
        CommandRiskLevel::Mutating => 1,
        CommandRiskLevel::Dangerous => 2,
    };
    let quoted = assess_raw(cmd, safe_list, dangerous_list);
    let plain = assess_raw(&dequoted, safe_list, dangerous_list);
    let risk = if rank(plain) > rank(quoted) { plain } else { quoted };
    if risk == CommandRiskLevel::Safe && sed_writes_or_executes(&dequoted) {
        return CommandRiskLevel::Mutating;
    }
    risk
}

/// `sed` scripts can write files (`w`) or run commands (`e`) without `-i`.
fn sed_writes_or_executes(cmd: &str) -> bool {
    let script_is_unsafe = |script: &str| -> bool {
        let s = script.trim().trim_start_matches(|c: char| {
            c.is_ascii_digit() || matches!(c, ',' | '$' | '!' | ' ' | '\t')
        });
        if s.starts_with('e') || s.starts_with('w') {
            return true;
        }
        if let Some(rest) = s.strip_prefix('s') {
            if let Some(delim) = rest.chars().next() {
                if let Some((_, tail)) = rest.rsplit_once(delim) {
                    if tail.contains('e') || tail.contains('w') {
                        return true;
                    }
                }
            }
        }
        false
    };

    cmd.split(['|', ';', '&']).any(|seg| {
        let seg = seg.trim();
        if !seg.starts_with("sed ") {
            return false;
        }
        let tokens: Vec<&str> = seg.split_whitespace().skip(1).collect();
        let mut i = 0;
        let mut checked_script = false;
        while i < tokens.len() {
            let t = tokens[i];
            if t == "-e" {
                if let Some(&next) = tokens.get(i + 1) {
                    if script_is_unsafe(next) {
                        return true;
                    }
                    checked_script = true;
                }
                i += 2;
                continue;
            }
            if let Some(inline) = t.strip_prefix("-e").or_else(|| t.strip_prefix("--expression=")) {
                if script_is_unsafe(inline) {
                    return true;
                }
                checked_script = true;
                i += 1;
                continue;
            }
            if !t.starts_with('-') && !checked_script {
                if script_is_unsafe(t) {
                    return true;
                }
                checked_script = true;
            }
            i += 1;
        }
        false
    })
}

fn assess_raw(cmd: &str, safe_list: &[String], dangerous_list: &[String]) -> CommandRiskLevel {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return CommandRiskLevel::Mutating;
    }
    let lower = trimmed.to_ascii_lowercase();

    // 1. Check dangerous patterns first, matched at word boundaries so a short entry like `dd`
    //    does not fire on an unrelated substring (`ip addr`, `mkdir`).
    for d in dangerous_list {
        if contains_at_word_boundary(&lower, &d.to_ascii_lowercase()) {
            return CommandRiskLevel::Dangerous;
        }
    }

    // Built-in dangerous heuristics (checked across the full command and each line)
    if lower.starts_with("rm -rf")
        || lower.contains("rm -rf /")
        || lower.contains("mkfs")
        || lower.contains("dd if=")
        || lower.contains(":(){ :|:& };:")
        || lower.starts_with("reboot")
        || lower.starts_with("poweroff")
        || lower.starts_with("shutdown")
        || lower.contains("> /dev/sd")
        || lower.contains("iptables -f")
        || lower.lines().any(|line| {
            let l = line.trim();
            l.starts_with("rm -rf")
                || l.starts_with("reboot")
                || l.starts_with("poweroff")
                || l.starts_with("shutdown")
        })
    {
        return CommandRiskLevel::Dangerous;
    }

    // Reject multiline commands, backslashes, or any variable/command/process substitution (`$VAR`,
    // `$(...)`, backticks, `${...}`, `<(...)`, `>(...)`) from being Safe.
    if cmd.contains('\n')
        || cmd.contains('\r')
        || cmd.contains('\\')
        || cmd.contains('$')
        || cmd.contains('`')
        || cmd.contains("<(")
        || cmd.contains(">(")
    {
        return CommandRiskLevel::Mutating;
    }

    // Allow harmless standard output/error discard idioms (2>/dev/null, >/dev/null, 2>&1)
    let clean_redirs = lower
        .replace("2>/dev/null", "")
        .replace("2> /dev/null", "")
        .replace("1>/dev/null", "")
        .replace("1> /dev/null", "")
        .replace(">/dev/null", "")
        .replace("> /dev/null", "")
        .replace("2>&1", "")
        .replace("1>&2", "");

    // Reject file redirections, semicolons, OR-chains, or background `&` operators from being Safe
    if clean_redirs.contains('>')
        || clean_redirs.contains('<')
        || clean_redirs.contains(';')
        || clean_redirs.contains("||")
        || clean_redirs.replace("&&", "").contains('&')
    {
        return CommandRiskLevel::Mutating;
    }

    // Reject commands referencing sensitive paths from being Safe
    if SENSITIVE_PATHS.iter().any(|p| lower.contains(p))
        || lower.split_whitespace().any(|tok| {
            let t = tok.trim_matches(|c| c == '\'' || c == '"');
            let has_glob = t.contains('*') || t.contains('?') || t.contains('[');
            has_glob
                && (t.starts_with('.')
                    || t.contains("/.")
                    || t.starts_with("/etc/")
                    || t.starts_with("/proc/")
                    || t.starts_with("/root"))
        })
    {
        return CommandRiskLevel::Mutating;
    }

    // 2. Evaluate compound read-only chains (`&&`) and pipelines (`|`)
    for chain_part in lower.split("&&") {
        let chain_part = chain_part.trim();
        if chain_part.is_empty() {
            return CommandRiskLevel::Mutating;
        }

        for (pipe_idx, pipe_seg) in chain_part.split('|').enumerate() {
            let seg = pipe_seg.trim();
            if seg.is_empty() {
                return CommandRiskLevel::Mutating;
            }

            let clean_tokens: Vec<String> = seg
                .split_whitespace()
                .map(|t| t.trim_matches(|c| c == '\'' || c == '"').to_string())
                .collect();
            let tokens: Vec<&str> = clean_tokens.iter().map(|s| s.as_str()).collect();
            let first_word = tokens.first().copied().unwrap_or("");
            if first_word.is_empty() {
                return CommandRiskLevel::Mutating;
            }

            if has_dangerous_flags_on_safe_cmd(seg, &tokens) {
                return CommandRiskLevel::Mutating;
            }

            let matches_safe_list = safe_list.iter().any(|s| {
                let s_lower = s.trim().to_ascii_lowercase();
                if s_lower.is_empty() {
                    return false;
                }
                seg == s_lower
                    || seg.starts_with(&format!("{} ", s_lower))
                    || first_word == s_lower
            });

            let matches_pipe_filter =
                pipe_idx > 0 && SAFE_PIPE_FILTERS.contains(&first_word);

            if !matches_safe_list && !matches_pipe_filter {
                return CommandRiskLevel::Mutating;
            }
        }
    }

    CommandRiskLevel::Safe
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshOutput {
    pub node_id: String,
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub success: bool,
}

pub struct SshExecutor;

impl SshExecutor {
    /// Constructs a base `tokio::process::Command` preconfigured with SSH options,
    /// multiplexing socket path, timeout, and host parameters.
    pub fn build_ssh_command(node: &NodeConfig, timeout_secs: u64) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.kill_on_drop(true);

        // Fast failure and multiplexing configurations
        cmd.arg("-o").arg(format!("ConnectTimeout={}", timeout_secs.min(10)));
        cmd.arg("-o").arg("BatchMode=yes");
        cmd.arg("-o").arg("StrictHostKeyChecking=accept-new");

        // ControlMaster socket in a private 0700 directory for connection multiplexing
        let socket_dir = directories::BaseDirs::new()
            .map(|d| d.home_dir().join(".corex").join("ssh"))
            .unwrap_or_else(|| std::env::temp_dir().join(format!("corex-ssh-{}", std::process::id())));
        if corex_core::ensure_private_dir(&socket_dir).is_ok() {
            let socket_path = socket_dir.join("cx-%r@%h:%p");
            cmd.arg("-o").arg("ControlMaster=auto");
            cmd.arg("-o").arg(format!("ControlPath={}", socket_path.display()));
            cmd.arg("-o").arg("ControlPersist=10m");
        } else {
            // A control socket in a directory another user controls could be hijacked to ride
            // our authenticated connection: skip multiplexing instead.
            cmd.arg("-o").arg("ControlMaster=no");
            cmd.arg("-o").arg("ControlPath=none");
        }

        // Port
        if node.port != 22 {
            cmd.arg("-p").arg(node.port.to_string());
        }

        // Identity Key (resolved from key_path or AuthMethod::KeyFile with tilde expansion)
        if let Some(key) = node.resolved_key_path() {
            cmd.arg("-i").arg(key);
        }

        // Proxy Jump (reject leading '-' to prevent option injection)
        if let Some(ref jump) = node.proxy_jump {
            let clean_jump = jump.trim_start_matches('-');
            if !clean_jump.is_empty() {
                cmd.arg("-J").arg(clean_jump);
            }
        }

        // Target: end options with `--` and strip any leading `-` from user/host to prevent SSH option injection
        let safe_user = node.user.trim_start_matches('-');
        let safe_host = node.host.trim_start_matches('-');
        cmd.arg("--");
        cmd.arg(format!("{}@{}", safe_user, safe_host));

        cmd
    }

    /// Formats the remote command argument, wrapping in `sudo -n -- sh -c '...'` when sudo is active.
    pub fn format_remote_command(node: &NodeConfig, command: &str, run_sudo: bool) -> String {
        if run_sudo || node.sudo {
            format!("sudo -n -- sh -c {}", shell_quote_single(command))
        } else {
            command.to_string()
        }
    }

    /// Tests the connection to a node and measures the round-trip latency in milliseconds.
    pub async fn test_connection(node: &NodeConfig, timeout_secs: u64) -> Result<u64> {
        let start = Instant::now();
        let mut cmd = Self::build_ssh_command(node, timeout_secs);
        cmd.arg("exit 0");
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());

        let output = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output())
            .await
            .context("Connection test timed out")?
            .context("Failed to spawn ssh command")?;

        if output.status.success() {
            Ok(start.elapsed().as_millis() as u64)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .unwrap_or("");
            if reason.is_empty() {
                bail!("SSH handshake failed with exit code: {:?}", output.status.code());
            }
            bail!("{}", reason);
        }
    }

    /// Executes a command on a remote node, capturing stdout, stderr, exit code, and execution duration.
    pub async fn execute(
        node: &NodeConfig,
        command: &str,
        run_sudo: bool,
        timeout_secs: u64,
    ) -> Result<SshOutput> {
        const MAX_SSH_STREAM_BYTES: u64 = 2 * 1024 * 1024;
        let start = Instant::now();
        let mut cmd = Self::build_ssh_command(node, timeout_secs);

        if run_sudo || node.sudo {
            cmd.arg(format!("sudo -n -- sh -c {}", shell_quote_single(command)));
        } else {
            cmd.arg(command);
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn remote ssh command")?;

        let stdout_handle = child.stdout.take().context("Failed to capture stdout")?;
        let stderr_handle = child.stderr.take().context("Failed to capture stderr")?;

        let stdout_fut = async {
            let mut buf = Vec::new();
            let _ = stdout_handle.take(MAX_SSH_STREAM_BYTES).read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).to_string()
        };

        let stderr_fut = async {
            let mut buf = Vec::new();
            let _ = stderr_handle.take(MAX_SSH_STREAM_BYTES).read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).to_string()
        };

        let wait_fut = async {
            let (status, stdout, stderr) = tokio::join!(child.wait(), stdout_fut, stderr_fut);
            let exit_code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            (exit_code, stdout, stderr)
        };

        let (exit_code, stdout, stderr) = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            wait_fut,
        )
        .await
        .map_err(|_| anyhow::anyhow!("Remote command on '{}' timed out after {}s", node.id, timeout_secs))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(SshOutput {
            node_id: node.id.clone(),
            command: command.to_string(),
            stdout,
            stderr,
            exit_code,
            duration_ms,
            success: exit_code == 0,
        })
    }

    /// Broadcasts a command concurrently across multiple nodes using `tokio::task::JoinSet`,
    /// bounded to 16 concurrent SSH executions while preserving input node order.
    pub async fn broadcast(
        nodes: &[&NodeConfig],
        command: &str,
        run_sudo: bool,
        timeout_secs: u64,
    ) -> Vec<(String, Result<SshOutput>)> {
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
        let mut set = JoinSet::new();

        for (idx, node) in nodes.iter().enumerate() {
            let n = (*node).clone();
            let cmd = command.to_string();
            let sem = semaphore.clone();
            set.spawn(async move {
                let _permit = sem.acquire_owned().await;
                let node_id = n.id.clone();
                let res = Self::execute(&n, &cmd, run_sudo, timeout_secs).await;
                (idx, node_id, res)
            });
        }

        let mut indexed_results = Vec::with_capacity(nodes.len());
        while let Some(res) = set.join_next().await {
            match res {
                Ok(tuple) => indexed_results.push(tuple),
                Err(e) => {
                    tracing::error!("Join error during broadcast: {:?}", e);
                }
            }
        }

        indexed_results.sort_by_key(|(idx, _, _)| *idx);
        indexed_results
            .into_iter()
            .map(|(_, node_id, res)| (node_id, res))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::config::{AuthMethod, NodeConfig};

    #[test]
    fn test_default_safe_list_rejects_mutating_bypasses() {
        let c = crate::config::FleetConfig::default();
        let risk = |cmd: &str| assess_command_risk(cmd, &c.safe_commands, &c.dangerous_commands);
        let safe = |cmd: &str| risk(cmd) == CommandRiskLevel::Safe;

        // Wrapper and mutating forms that used to pass by matching the first word.
        assert!(!safe("env rm -rf /var/lib/mysql"));
        assert!(!safe("sudo systemctl restart nginx"));
        assert!(!safe("ip link set eth0 down"));
        assert!(!safe("ip route del default"));
        assert!(!safe("systemctl isolate rescue.target"));
        assert!(!safe("systemctl set-default rescue.target"));
        assert!(!safe("journalctl --vacuum-time=1s"));
        assert!(!safe("hostname pwned"));
        assert!(!safe("date -s 2000-01-01"));
        assert!(!safe("sort -ro /etc/passwd /tmp/x"));
        assert!(!safe("iptables -A INPUT -j DROP"));
        assert!(!safe("echo $HOME"));

        // Read-only operations stay Safe (no confirmation needed).
        assert!(safe("uptime"));
        assert!(safe("systemctl status nginx"));
        assert!(safe("ip addr show"));
        assert!(safe("ip route"));
        assert!(safe("journalctl -u nginx -n 100"));
        assert!(safe("date"));
        assert!(safe("hostname"));
        assert!(safe("free -m && uptime"));

        // `dd` must stay Dangerous but must not fire on an unrelated substring (`ip addr`).
        assert_eq!(risk("dd if=/dev/zero of=/dev/sda"), CommandRiskLevel::Dangerous);
        assert_ne!(risk("ip addr show"), CommandRiskLevel::Dangerous);
    }

    #[test]
    fn test_contains_at_word_boundary() {
        assert!(contains_at_word_boundary("dd if=/dev/zero", "dd"));
        assert!(!contains_at_word_boundary("ip addr show", "dd"));
        assert!(!contains_at_word_boundary("mkdir -p x", "dd"));
        assert!(contains_at_word_boundary("sudo rm -rf /", "rm -rf"));
    }

    #[test]
    fn test_risk_classification() {
        let safe = vec![
            "uptime".to_string(),
            "df".to_string(),
            "free".to_string(),
            "ps".to_string(),
            "cat".to_string(),
            "docker ps".to_string(),
            "echo".to_string(),
            "systemctl".to_string(),
            "hostnamectl".to_string(),
        ];
        let dangerous = vec!["rm -rf".to_string(), "reboot".to_string()];

        assert_eq!(assess_command_risk("uptime", &safe, &dangerous), CommandRiskLevel::Safe);
        assert_eq!(assess_command_risk("df -h", &safe, &dangerous), CommandRiskLevel::Safe);
        assert_eq!(assess_command_risk("docker ps", &safe, &dangerous), CommandRiskLevel::Safe);

        // Compound safe pipelines and chains
        assert_eq!(
            assess_command_risk("uptime && free -h && df -h /", &safe, &dangerous),
            CommandRiskLevel::Safe
        );
        assert_eq!(
            assess_command_risk("uptime && echo \"---\" && free -h && echo \"---\" && df -h / /home 2>/dev/null", &safe, &dangerous),
            CommandRiskLevel::Safe
        );
        assert_eq!(
            assess_command_risk("uptime && echo \"---\" && free -h && echo \"---\" && df -h / && echo \"---\" && hostnamectl 2>/dev/null | head -8", &safe, &dangerous),
            CommandRiskLevel::Safe
        );
        assert_eq!(
            assess_command_risk("systemctl --failed", &safe, &dangerous),
            CommandRiskLevel::Safe
        );
        assert_eq!(
            assess_command_risk("systemctl status nginx", &safe, &dangerous),
            CommandRiskLevel::Safe
        );
        assert_eq!(
            assess_command_risk("systemctl restart nginx", &safe, &dangerous),
            CommandRiskLevel::Mutating
        );
        assert_eq!(
            assess_command_risk("hostnamectl set-hostname evil", &safe, &dangerous),
            CommandRiskLevel::Mutating
        );
        assert_eq!(
            assess_command_risk("ps aux --sort=-%mem | head -10", &safe, &dangerous),
            CommandRiskLevel::Safe
        );

        // Sensitive path rejection
        assert_eq!(
            assess_command_risk("cat /etc/shadow", &safe, &dangerous),
            CommandRiskLevel::Mutating
        );
        assert_eq!(
            assess_command_risk("cat ~/.ssh/id_ed25519", &safe, &dangerous),
            CommandRiskLevel::Mutating
        );
        assert_eq!(
            assess_command_risk("cat .env", &safe, &dangerous),
            CommandRiskLevel::Mutating
        );

        assert_eq!(assess_command_risk("systemctl restart nginx", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("apt-get update", &safe, &dangerous), CommandRiskLevel::Mutating);

        assert_eq!(assess_command_risk("rm -rf /var/log", &safe, &dangerous), CommandRiskLevel::Dangerous);
        assert_eq!(assess_command_risk("reboot", &safe, &dangerous), CommandRiskLevel::Dangerous);
        assert_eq!(assess_command_risk("mkfs.ext4 /dev/sda", &safe, &dangerous), CommandRiskLevel::Dangerous);
        assert_eq!(assess_command_risk("uptime\nrm -rf /", &safe, &dangerous), CommandRiskLevel::Dangerous);
        assert_eq!(assess_command_risk("uptime\nwhoami", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("uptime $(id)", &safe, &dangerous), CommandRiskLevel::Mutating);
    }

    #[test]
    fn test_risk_classification_destructive_variants() {
        let safe = vec!["uptime".to_string(), "cat".to_string()];
        let dangerous = vec!["reboot".to_string()];

        assert_eq!(assess_command_risk(":(){ :|:& };:", &safe, &dangerous), CommandRiskLevel::Dangerous);
        assert_eq!(assess_command_risk("dd if=/dev/zero of=/dev/sda", &safe, &dangerous), CommandRiskLevel::Dangerous);
        assert_eq!(assess_command_risk("poweroff", &safe, &dangerous), CommandRiskLevel::Dangerous);
        assert_eq!(assess_command_risk("shutdown -h now", &safe, &dangerous), CommandRiskLevel::Dangerous);
        assert_eq!(assess_command_risk("cat /dev/null > /dev/sda1", &safe, &dangerous), CommandRiskLevel::Dangerous);
        assert_eq!(assess_command_risk("iptables -F", &safe, &dangerous), CommandRiskLevel::Dangerous);
    }

    #[test]
    fn test_risk_classification_subshell_and_pipe_chaining() {
        let safe = vec![
            "uptime".to_string(),
            "df".to_string(),
            "find".to_string(),
            "sed".to_string(),
            "git".to_string(),
        ];
        let dangerous = vec!["reboot".to_string()];

        assert_eq!(assess_command_risk("uptime; echo hacked", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("uptime && echo hacked", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("df | tee /tmp/output", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("uptime `whoami`", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("uptime ${USER}", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("find /var/log -name '*.log' -delete", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("sed -i 's/a/b/g' file.txt", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("git diff --output=/tmp/p", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("df | sort -o /tmp/out", &safe, &dangerous), CommandRiskLevel::Mutating);
        assert_eq!(assess_command_risk("df | awk '{system(\"id\")}'", &safe, &dangerous), CommandRiskLevel::Mutating);
    }

    #[test]
    fn test_sudo_wrapping_and_shell_quote_single() {
        let mut node = NodeConfig::new("srv-1", "10.0.0.1");
        let cmd = "export FOO='bar'; cd /etc && cat shadow";
        assert_eq!(
            SshExecutor::format_remote_command(&node, cmd, true),
            "sudo -n -- sh -c 'export FOO='\\''bar'\\''; cd /etc && cat shadow'"
        );
        node.sudo = true;
        assert_eq!(
            SshExecutor::format_remote_command(&node, "uptime", false),
            "sudo -n -- sh -c 'uptime'"
        );
    }

    #[test]
    fn test_build_ssh_command_args_and_injection_defense() {
        let node = NodeConfig {
            id: "prod-node-1".to_string(),
            name: Some("Production VPS".to_string()),
            host: "-oProxyCommand=calc.exe".to_string(),
            port: 2222,
            user: "-oBadUser".to_string(),
            key_path: None,
            auth: AuthMethod::KeyFile {
                path: PathBuf::from("/home/sluisr/.ssh/id_ed25519"),
            },
            tags: vec!["prod".to_string()],
            sudo: true,
            proxy_jump: Some("-oInjectedJump".to_string()),
        };

        let cmd = SshExecutor::build_ssh_command(&node, 8);
        let std_cmd = cmd.as_std();
        let args: Vec<String> = std_cmd.get_args().map(|s| s.to_string_lossy().to_string()).collect();

        // Check timeout and strict options
        assert!(args.contains(&"ConnectTimeout=8".to_string()));
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));

        // Check non-default port
        let port_idx = args.iter().position(|a| a == "-p").expect("Port -p arg must exist");
        assert_eq!(args[port_idx + 1], "2222");

        // Check key path resolved via AuthMethod::KeyFile fallback
        let key_idx = args.iter().position(|a| a == "-i").expect("Key -i arg must exist");
        assert_eq!(args[key_idx + 1], "/home/sluisr/.ssh/id_ed25519");

        // Check proxy jump sanitization (leading '-' stripped)
        let jump_idx = args.iter().position(|a| a == "-J").expect("Jump -J arg must exist");
        assert_eq!(args[jump_idx + 1], "oInjectedJump");

        // Check option injection barrier (`--` must precede user@host)
        let sep_idx = args.iter().position(|a| a == "--").expect("Option separator -- must exist");
        let target = &args[sep_idx + 1];
        assert_eq!(target, "oBadUser@oProxyCommand=calc.exe");
    }

    #[test]
    fn quotes_and_sed_scripts_do_not_hide_risk() {
        let safe: Vec<String> = ["cat", "systemctl", "systemctl status", "sed"].map(String::from).to_vec();
        let dangerous: Vec<String> = vec!["reboot".to_string()];
        let risk = |c: &str| assess_command_risk(c, &safe, &dangerous);
        assert_ne!(risk("cat /etc/\"shadow\""), CommandRiskLevel::Safe);
        assert_ne!(risk("systemctl \"re\"\"start\" nginx"), CommandRiskLevel::Safe);
        assert_ne!(risk("sed -n 'w /tmp/x' /etc/hosts"), CommandRiskLevel::Safe);
        assert_ne!(risk("sed 's/.*/id/e' /etc/hosts"), CommandRiskLevel::Safe);
        assert_eq!(risk("cat /etc/hostname"), CommandRiskLevel::Safe);
        assert_eq!(risk("sed -n '1,5p' /etc/hosts"), CommandRiskLevel::Safe);
    }

    #[test]
    fn sed_printenv_and_sensitive_globs_need_confirmation() {
        let cfg = crate::config::FleetConfig::default();
        let default_risk = |c: &str| assess_command_risk(c, &cfg.safe_commands, &cfg.dangerous_commands);
        assert_ne!(default_risk("printenv"), CommandRiskLevel::Safe);
        assert_ne!(default_risk("cat /proc/self/environ"), CommandRiskLevel::Safe);
        assert_ne!(default_risk("cat /proc/1/environ"), CommandRiskLevel::Safe);
        assert_ne!(default_risk("cat ~/.?sh/id_rsa"), CommandRiskLevel::Safe);
        assert_ne!(default_risk("cat /home/*/.?sh/id_rsa"), CommandRiskLevel::Safe);
        assert_ne!(default_risk("cat .?nv"), CommandRiskLevel::Safe);

        let safe_with_sed: Vec<String> = ["cat", "sed"].map(String::from).to_vec();
        let sed_risk = |c: &str| assess_command_risk(c, &safe_with_sed, &cfg.dangerous_commands);
        assert_ne!(sed_risk("sed '1e id' /etc/hosts"), CommandRiskLevel::Safe);
        assert_ne!(sed_risk("sed 's#.*#id#e' /etc/hosts"), CommandRiskLevel::Safe);
        assert_ne!(sed_risk("sed -e's/.*/id/e' /etc/hosts"), CommandRiskLevel::Safe);
        assert_ne!(sed_risk("sed -e 's/.*/id/e' /etc/hosts"), CommandRiskLevel::Safe);
        assert_ne!(sed_risk("sed '1w /tmp/out' /etc/hosts"), CommandRiskLevel::Safe);
        assert_eq!(sed_risk("sed -n '1,5p' /etc/hosts"), CommandRiskLevel::Safe);
    }
}
