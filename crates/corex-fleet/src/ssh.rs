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
            CommandRiskLevel::Safe => "🟢 SAFE",
            CommandRiskLevel::Mutating => "🟡 MUTATING",
            CommandRiskLevel::Dangerous => "🔴 DANGEROUS",
        }
    }
}

pub fn shell_quote_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

const SENSITIVE_PATHS: &[&str] = &[
    "/etc/shadow",
    "/etc/gshadow",
    ".ssh/",
    ".ssh",
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

fn has_dangerous_flags_on_safe_cmd(seg: &str, tokens: &[&str]) -> bool {
    let Some(&first) = tokens.first() else {
        return false;
    };
    let args = &tokens[1..];
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
        }),
        "git" => args.iter().any(|t| {
            *t == "--output"
                || t.starts_with("--output=")
                || *t == "--ext-diff"
                || *t == "--upload-pack"
                || t.starts_with("--upload-pack=")
                || *t == "-c"
        }),
        "sort" => args.iter().any(|t| {
            *t == "-o"
                || t.starts_with("-o")
                || *t == "--output"
                || t.starts_with("--output=")
        }),
        "awk" => seg.contains("system(") || seg.contains("getline"),
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
            )
        }),
        "hostnamectl" => args.iter().any(|t| *t == "set-hostname" || t.starts_with("set-")),
        "timedatectl" => args.iter().any(|t| t.starts_with("set-")),
        "localectl" => args.iter().any(|t| t.starts_with("set-")),
        _ => false,
    }
}

pub fn assess_command_risk(cmd: &str, safe_list: &[String], dangerous_list: &[String]) -> CommandRiskLevel {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return CommandRiskLevel::Mutating;
    }
    let lower = trimmed.to_ascii_lowercase();

    // 1. Check dangerous patterns first
    for d in dangerous_list {
        if lower.contains(&d.to_ascii_lowercase()) {
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

    // Reject multiline commands, command substitutions, or process substitutions from being Safe
    if cmd.contains('\n')
        || cmd.contains('\r')
        || cmd.contains("$(")
        || cmd.contains('`')
        || cmd.contains("${")
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
    if SENSITIVE_PATHS.iter().any(|p| lower.contains(p)) {
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

            let tokens: Vec<&str> = seg.split_whitespace().collect();
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
        let _ = corex_core::ensure_private_dir(&socket_dir);
        let socket_path = socket_dir.join("cx-%r@%h:%p");
        cmd.arg("-o").arg("ControlMaster=auto");
        cmd.arg("-o").arg(format!("ControlPath={}", socket_path.display()));
        cmd.arg("-o").arg("ControlPersist=10m");

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
}
