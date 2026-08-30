use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use anyhow::Result;
use async_trait::async_trait;
use directories::BaseDirs;
use regex::Regex;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::background::get_task_manager;
use crate::types::{Tool, ToolContext, ToolOutput};

const BASH_SHOPT_GUARD: &str = r#"sudo() { local -a _a=(); for _x in "$@"; do [ "$_x" != "-n" ] && [ "$_x" != "--non-interactive" ] && _a+=("$_x"); done; if [ -n "$SUDO_ASKPASS" ]; then command sudo -A "${_a[@]}"; else command sudo "${_a[@]}"; fi; }; "#;

pub fn get_or_create_askpass_script() -> PathBuf {
    let dir = BaseDirs::new()
        .map(|d| d.home_dir().join(".uti"))
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let _ = fs::create_dir_all(&dir);
    let script_path = dir.join("askpass.sh");

    let script_content = r#"#!/bin/sh
if [ -n "$UTI_SUDO_PASSWORD" ]; then
    printf '%s\n' "$UTI_SUDO_PASSWORD"
    exit 0
elif [ -n "$DEEPSEEK_SUDO_PASSWORD" ]; then
    printf '%s\n' "$DEEPSEEK_SUDO_PASSWORD"
    exit 0
else
    exit 1
fi
"#;

    if let Ok(_) = fs::write(&script_path, script_content) {
        let mut perms = fs::metadata(&script_path).map(|m| m.permissions()).unwrap_or_else(|_| fs::Permissions::from_mode(0o755));
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&script_path, perms);
    }

    script_path
}

/// Checks if a shell command is known to be safe/read-only and does not require user confirmation.
/// Matches deepseek-cli / Google Gemini CLI commandSafety.ts logic.
///
/// A command is considered safe when every pipeline segment is a known read-only
/// command (with subcommand/flag validation where relevant) and there is no
/// redirection to a real file. Benign redirections (`> /dev/null`, `2>&1`,
/// heredocs) are ignored; leading `VAR=value` prefixes are skipped.
pub fn is_known_safe_command(cmd_str: &str) -> bool {
    let trimmed = cmd_str.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Strip benign redirections (to /dev/null, fd duplication, here-docs) before
    // checking for any remaining redirection to a real file.
    let cleaned = strip_benign_redirects(trimmed);
    if cleaned.contains('>') || cleaned.contains('<') {
        return false;
    }

    let segments = split_segments(&cleaned);
    if segments.is_empty() {
        return false;
    }

    segments.iter().all(|seg| is_single_segment_safe(seg))
}

/// Like [`is_known_safe_command`], but also accepts commands matching the
/// user-configured `allowed_commands` list (from `~/.uti/settings.json`).
/// Entries may be bare binary names or full `cmd subcommand` prefixes.
pub fn is_known_safe_command_with_allowed(cmd_str: &str, allowed: &HashSet<String>) -> bool {
    if is_known_safe_command(cmd_str) {
        return true;
    }
    if allowed.is_empty() {
        return false;
    }

    let trimmed = cmd_str.trim();
    if trimmed.is_empty() {
        return false;
    }

    let cleaned = strip_benign_redirects(trimmed);
    if cleaned.contains('>') || cleaned.contains('<') {
        return false;
    }

    let segments = split_segments(&cleaned);
    !segments.is_empty() && segments.iter().all(|seg| segment_matches_allowed(seg, allowed))
}

/// Removes redirections that never write to a real file: `/dev/null` targets
/// (with or without spaces), fd duplication (`2>&1`, `>&2`) and stdin
/// here-strings/here-docs. Anything else containing `>`/`<` still requires
/// confirmation.
fn strip_benign_redirects(s: &str) -> String {
    static BENIGN_REDIRECT_RE: OnceLock<Regex> = OnceLock::new();
    let re = BENIGN_REDIRECT_RE.get_or_init(|| {
        Regex::new(
            r"(?i)([0-9]?[ \t]*&?[ \t]*>>?[ \t]*/dev/null)|([0-9]?[ \t]*>&[ \t]*[0-9])|([0-9]?[ \t]*<[ \t]*/dev/null)|([0-9]?[ \t]*<<<)|([0-9]?[ \t]*<<-?[ \t]*[A-Za-z_][A-Za-z0-9_]*)",
        )
        .expect("valid benign redirection regex")
    });
    re.replace_all(s, "").to_string()
}

fn split_segments(cmd: &str) -> Vec<&str> {
    cmd.split(|c| c == ';' || c == '&' || c == '|')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
}

/// True for `VAR=value` environment assignment prefixes (e.g. `LANG=C`).
fn is_env_assignment(tok: &str) -> bool {
    let Some(eq) = tok.find('=') else {
        return false;
    };
    if eq == 0 {
        return false;
    }
    let name = &tok[..eq];
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Wrapper commands that merely execute another command; we validate the
/// wrapped command instead of the wrapper (prevents `env rm -rf /` bypasses).
fn is_wrapper(cmd: &str) -> bool {
    matches!(
        cmd,
        "env" | "command" | "time" | "nice" | "nohup" | "setsid" | "timeout" | "stdbuf" | "watch"
    )
}

fn is_single_segment_safe(segment: &str) -> bool {
    let mut parts: Vec<&str> = segment.split_whitespace().collect();
    if parts.is_empty() {
        return true;
    }

    // Skip leading VAR=value environment assignments to find the real command.
    while is_env_assignment(parts[0]) {
        parts.remove(0);
        if parts.is_empty() {
            return true;
        }
    }

    let raw_cmd = parts[0];
    let cmd = Path::new(raw_cmd)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(raw_cmd);

    // Wrapper commands: validate the wrapped command instead.
    if is_wrapper(cmd) {
        let mut j = 1;
        while j < parts.len()
            && (parts[j].starts_with('-') || parts[j].chars().all(|c| c.is_ascii_digit()))
        {
            j += 1;
        }
        while j < parts.len() && is_env_assignment(parts[j]) {
            j += 1;
        }
        if j >= parts.len() {
            // Bare wrapper without a target command (e.g. `env`, `nice`) is harmless.
            return true;
        }
        return is_single_segment_safe(&parts[j..].join(" "));
    }

    // List of unconditionally safe read-only POSIX and system commands
    let safe_tools: HashSet<&str> = [
        "cat", "ls", "dir", "tree", "grep", "egrep", "fgrep", "rg", "head", "tail", "less",
        "more", "wc", "cut", "tr", "uniq", "sort", "tac", "nl", "echo", "printf", "stat",
        "file", "strings", "column", "pwd", "cd", "which", "whereis", "whoami", "id",
        "uname", "uptime", "lscpu", "free", "df", "du", "lsblk", "nproc", "arch", "hostname",
        "date", "printenv", "top", "ps", "ip", "ifconfig", "expr", "seq", "true",
        "false", "test", "numfmt", "awk", "sed", "sensors", "lshw", "lspci", "lsusb",
        "dmidecode", "hdparm", "smartctl", "dmesg", "journalctl",
        // Read-only inspection / text-processing additions
        "ss", "netstat", "lsof", "diff", "cmp", "basename", "dirname",
        "readlink", "realpath", "md5sum", "sha1sum", "sha224sum", "sha256sum",
        "sha384sum", "sha512sum", "cksum", "sum", "od", "hexdump", "xxd",
        "vmstat", "iostat", "pgrep", "pidof", "pstree", "bc", "factor", "zipinfo",
        "apt-cache", "dpkg-query", "who", "w", "last", "groups", "getfacl", "lsattr",
        "blkid", "findmnt", "mountpoint", "zcat", "zgrep", "zless", "bzcat", "xzcat",
        "comm", "join", "paste", "fold", "fmt", "tsort", "look",
    ]
    .into_iter()
    .collect();

    if safe_tools.contains(cmd) {
        // Special check for sed or awk with mutating behavior
        if cmd == "sed" && parts.iter().any(|&p| p == "-i" || p.starts_with("-i")) {
            return false;
        }
        if cmd == "awk" && segment.contains("system(") {
            return false;
        }
        return true;
    }

    // `find` may not mutate (no -exec/-ok/-delete or -fprint family)
    if cmd == "find" {
        return !parts.iter().any(|p| {
            p.starts_with("-exec")
                || p.starts_with("-ok")
                || p.starts_with("-delete")
                || p.starts_with("-fls")
                || p.starts_with("-fprint")
        });
    }

    // `tar` only in list mode (-t / --list), never create/extract/append
    if cmd == "tar" {
        return is_safe_tar(&parts);
    }

    // `unzip` only in list/test/pipe mode, never extraction
    if cmd == "unzip" {
        return is_safe_unzip(&parts);
    }

    // `systemctl` only for status/show/inspection subcommands
    if cmd == "systemctl" {
        return is_safe_systemctl(&parts);
    }

    // `dpkg` only for query/status subcommands, never install/remove/configure
    if cmd == "dpkg" {
        return is_safe_dpkg(&parts);
    }

    // `mount` without arguments lists mounts; any argument mounts something.
    if cmd == "mount" {
        return parts.len() == 1 || parts[1] == "--help" || parts[1] == "--version";
    }

    // Safe Git read-only subcommands
    if cmd == "git" {
        if parts.len() > 1 {
            let subcommand = parts[1];
            let safe_git_subcommands: HashSet<&str> = [
                "status", "log", "diff", "show", "remote", "rev-parse",
                "describe", "config", "check-ignore", "ls-files",
                "blame", "shortlog", "count-objects", "var", "version", "help",
                "for-each-ref", "show-ref", "name-rev", "rev-list", "whatchanged",
                "ls-remote", "branch", "tag",
            ]
            .into_iter()
            .collect();

            if safe_git_subcommands.contains(subcommand) {
                return match subcommand {
                    "branch" => is_safe_git_branch(&parts),
                    "tag" => is_safe_git_tag(&parts),
                    "remote" => is_safe_git_remote(&parts),
                    "config" => is_safe_git_config(&parts),
                    _ => true,
                };
            }
        }
        return false;
    }

    // Safe Cargo/Rust inspection commands
    if cmd == "cargo" {
        if parts.len() > 1 {
            let subcommand = parts[1];
            let safe_cargo_subcommands: HashSet<&str> = [
                "--version", "-V", "check", "clippy", "tree", "test", "metadata", "locate-project", "verify-project"
            ]
            .into_iter()
            .collect();
            return safe_cargo_subcommands.contains(subcommand);
        }
        return false;
    }

    if cmd == "rustc" && parts.iter().any(|&p| p == "--version" || p == "-V") {
        return true;
    }

    // Safe Node/npm/yarn/pnpm commands
    if cmd == "npm" || cmd == "yarn" || cmd == "pnpm" {
        if parts.len() > 1 {
            let subcommand = parts[1];
            let safe_subcommands: HashSet<&str> = [
                "--version", "-v", "list", "ls", "outdated", "audit", "config", "info", "view", "help"
            ]
            .into_iter()
            .collect();
            return safe_subcommands.contains(subcommand);
        }
        return false;
    }

    if cmd == "node" {
        return parts.len() > 1 && (parts[1] == "--version" || parts[1] == "-v" || parts[1] == "--help" || parts[1] == "-h");
    }

    // Safe Python commands
    if cmd == "python" || cmd == "python3" {
        return parts.len() > 1 && (parts[1] == "--version" || parts[1] == "-V" || parts[1] == "--help" || parts[1] == "-h");
    }

    if cmd == "pip" || cmd == "pip3" {
        if parts.len() > 1 {
            let subcommand = parts[1];
            let safe_pip_subcommands: HashSet<&str> = [
                "--version", "-V", "list", "show", "help"
            ]
            .into_iter()
            .collect();
            return safe_pip_subcommands.contains(subcommand);
        }
        return false;
    }

    if cmd == "poetry" {
        if parts.len() > 1 {
            let subcommand = parts[1];
            let safe_poetry_subcommands: HashSet<&str> = [
                "--version", "-V", "show", "env", "info", "help"
            ]
            .into_iter()
            .collect();
            if subcommand == "env" && parts.len() > 2 {
                return parts[2] == "info";
            }
            return safe_poetry_subcommands.contains(subcommand);
        }
        return false;
    }

    // Safe Go commands
    if cmd == "go" {
        if parts.len() > 1 {
            let subcommand = parts[1];
            let safe_go_subcommands: HashSet<&str> = [
                "version", "env", "list", "help"
            ]
            .into_iter()
            .collect();
            return safe_go_subcommands.contains(subcommand);
        }
        return false;
    }

    // Safe Docker commands (read-only inspection)
    if cmd == "docker" {
        if parts.len() > 1 {
            let subcommand = parts[1];
            let safe_docker_subcommands: HashSet<&str> = [
                "ps", "images", "version", "info", "stats", "help", "inspect", "logs",
                "history", "top", "port", "diff", "events",
            ]
            .into_iter()
            .collect();
            if safe_docker_subcommands.contains(subcommand) {
                return true;
            }
            // docker system df/events and docker network ls / volume ls are read-only
            if subcommand == "system" && parts.len() > 2 {
                return parts[2] == "df" || parts[2] == "events";
            }
            if (subcommand == "network" || subcommand == "volume") && parts.len() > 2 {
                return parts[2] == "ls";
            }
        }
        return false;
    }

    if cmd == "docker-compose" {
        if parts.len() > 1 {
            let subcommand = parts[1];
            let safe_docker_compose_subcommands: HashSet<&str> = [
                "ps", "version", "config", "help"
            ]
            .into_iter()
            .collect();
            return safe_docker_compose_subcommands.contains(subcommand);
        }
        return false;
    }

    // `apt` only for query subcommands
    if cmd == "apt" {
        if parts.len() > 1 {
            let subcommand = parts[1];
            let safe_apt_subcommands: HashSet<&str> = [
                "list", "show", "search", "policy", "version", "help", "--version",
            ]
            .into_iter()
            .collect();
            return safe_apt_subcommands.contains(subcommand);
        }
        return false;
    }

    // Safe network utility checks
    if cmd == "curl" && parts.iter().any(|&p| p == "--version" || p == "-V" || p == "-I" || p == "--head") {
        return true;
    }

    if cmd == "wget" && parts.iter().any(|&p| p == "--version" || p == "-V") {
        return true;
    }

    if cmd == "ping" && parts.iter().any(|&p| p == "-c" || p == "--help") {
        return true;
    }

    false
}

fn is_safe_tar(parts: &[&str]) -> bool {
    let mut short_chars: Vec<char> = Vec::new();
    let mut long_flags: Vec<&str> = Vec::new();
    for p in &parts[1..] {
        if p.starts_with("--") {
            long_flags.push(p);
        } else if p.len() > 1 && p.starts_with('-') {
            short_chars.extend(p[1..].chars());
        }
    }
    let write_long = [
        "--create", "--extract", "--get", "--append", "--update", "--concatenate",
        "--delete", "--unlink-first",
    ];
    let has_write = short_chars.iter().any(|c| matches!(c, 'x' | 'c' | 'r' | 'u' | 'A' | 'U'))
        || long_flags.iter().any(|f| write_long.contains(f));
    let has_list = short_chars.contains(&'t') || long_flags.contains(&"--list");
    !has_write && has_list
}

fn is_safe_unzip(parts: &[&str]) -> bool {
    let mut short_chars: Vec<char> = Vec::new();
    let mut long_flags: Vec<&str> = Vec::new();
    for p in &parts[1..] {
        if p.starts_with("--") {
            long_flags.push(p);
        } else if p.len() > 1 && p.starts_with('-') {
            short_chars.extend(p[1..].chars());
        }
    }
    let safe_long = ["--help", "--version", "--test"];
    if long_flags.iter().any(|f| !safe_long.contains(f)) {
        return false;
    }
    if !short_chars.iter().all(|c| matches!(c, 'l' | 'Z' | 'v' | 't' | 'p' | 'q')) {
        return false;
    }
    // No operation flag at all means `unzip archive.zip` extracts to disk — unsafe.
    short_chars.iter().any(|c| *c != 'q') || !long_flags.is_empty()
}

fn is_safe_systemctl(parts: &[&str]) -> bool {
    let safe_subs: HashSet<&str> = [
        "status", "show", "is-active", "is-enabled", "is-failed", "is-system-running",
        "list-units", "list-unit-files", "list-timers", "list-sockets", "list-machines",
        "list-jobs", "list-dependencies", "list-automounts", "list-paths", "list-swaps",
        "cat", "get-default", "get-property", "help",
    ]
    .into_iter()
    .collect();
    let unsafe_subs: HashSet<&str> = [
        "start", "stop", "restart", "reload", "reload-or-restart", "try-restart",
        "enable", "disable", "reenable", "mask", "unmask", "daemon-reload",
        "daemon-reexec", "reset-failed", "set-property", "edit", "kill", "isolate",
        "default", "rescue", "emergency", "halt", "poweroff", "reboot", "suspend",
        "hibernate", "hybrid-sleep", "kexec", "switch-root", "link", "unlink",
        "preset", "preset-all", "add-wants", "add-requires", "set-default",
        "set-environment", "unset-environment", "import-environment", "cancel",
        "clean", "freeze", "thaw",
    ]
    .into_iter()
    .collect();
    let benign_opts: HashSet<&str> = [
        "--help", "--version", "--failed", "--all", "--no-pager", "--plain",
        "--user", "--system", "--state", "--type", "--output", "--no-legend",
        "--full", "--lines", "--no-ask-password", "--quiet", "-a", "-l", "-q",
    ]
    .into_iter()
    .collect();

    let is_benign_opt = |tok: &str| {
        let name = tok.split('=').next().unwrap_or(tok);
        benign_opts.contains(name)
    };

    let mut i = 1;
    let mut saw_benign_option = false;
    while i < parts.len() && parts[i].starts_with('-') {
        if !is_benign_opt(parts[i]) {
            return false;
        }
        saw_benign_option = true;
        i += 1;
    }
    if i >= parts.len() {
        // e.g. `systemctl --failed`
        return saw_benign_option;
    }
    if !safe_subs.contains(parts[i]) {
        return false;
    }
    for tok in &parts[i + 1..] {
        if tok.starts_with('-') {
            if !is_benign_opt(tok) {
                return false;
            }
        } else if unsafe_subs.contains(tok) {
            return false;
        }
    }
    true
}

fn is_safe_dpkg(parts: &[&str]) -> bool {
    let safe_long: HashSet<&str> = [
        "--list", "--status", "--print-avail", "--listfiles", "--search", "--verify",
        "--info", "--contents", "--control", "--field", "--show", "--audit",
        "--print-architecture", "--print-foreign-architectures",
        "--print-installation-architecture", "--get-selections", "--version", "--help",
    ]
    .into_iter()
    .collect();
    let unsafe_long: HashSet<&str> = [
        "--install", "--remove", "--purge", "--unpack", "--configure", "--triggers-only",
        "--update-avail", "--clear-avail", "--record-avail", "--set-selections",
        "--add-architecture", "--remove-architecture", "--set-architecture",
        "--merge-avail", "--forget-old-unavail",
    ]
    .into_iter()
    .collect();
    let mut saw_op = false;
    for p in &parts[1..] {
        if p.starts_with("--") {
            let name = p.split('=').next().unwrap_or(p);
            if unsafe_long.contains(name) {
                return false;
            }
            if !safe_long.contains(name) {
                return false;
            }
            saw_op = true;
        } else if p.len() > 1 && p.starts_with('-') {
            for c in p[1..].chars() {
                if !matches!(c, 'l' | 's' | 'p' | 'L' | 'S' | 'V' | 'I' | 'c' | 'e' | 'f' | 'W') {
                    return false;
                }
                saw_op = true;
            }
        }
        // plain package/file arguments are ignored
    }
    saw_op
}

fn is_safe_git_branch(parts: &[&str]) -> bool {
    // bare `git branch` lists; only listing flags are read-only.
    if parts.len() == 2 {
        return true;
    }
    let flag = parts[2];
    if !flag.starts_with('-') {
        return false;
    }
    !matches!(
        flag,
        "-d" | "-D" | "-m" | "-M" | "-c" | "-C" | "-f" | "--delete" | "--move" | "--copy"
            | "--force" | "--set-upstream-to" | "--edit-description"
    )
}

fn is_safe_git_tag(parts: &[&str]) -> bool {
    // bare `git tag` lists tags; only explicit list/query flags are safe.
    if parts.len() == 2 {
        return true;
    }
    matches!(
        parts[2],
        "-l" | "--list" | "-n" | "--column" | "--contains" | "--merged" | "--no-merged"
    )
}

fn is_safe_git_remote(parts: &[&str]) -> bool {
    // bare `git remote` lists; `-v/--verbose`, `show`, `get-url` are read-only.
    if parts.len() == 2 {
        return true;
    }
    matches!(parts[2], "-v" | "--verbose" | "show" | "get-url")
}

fn is_safe_git_config(parts: &[&str]) -> bool {
    // Reading config is safe; setting a key=value pair writes to .git/config.
    let non_opts: Vec<&str> = parts[2..]
        .iter()
        .copied()
        .filter(|p| !p.starts_with('-'))
        .collect();
    !(non_opts.len() > 1 || non_opts.iter().any(|p| p.contains('=')))
}

fn segment_matches_allowed(segment: &str, allowed: &HashSet<String>) -> bool {
    let seg = segment.trim();
    if seg.is_empty() {
        return true;
    }
    let mut parts: Vec<&str> = seg.split_whitespace().collect();
    while is_env_assignment(parts[0]) {
        parts.remove(0);
        if parts.is_empty() {
            return true;
        }
    }
    let base = Path::new(parts[0])
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(parts[0]);
    allowed.iter().any(|entry| {
        let e = entry.trim();
        !e.is_empty() && (seg == e || seg.starts_with(&format!("{} ", e)) || base == e)
    })
}

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        "run_shell_command"
    }

    fn description(&self) -> &'static str {
        "Executes a bash shell command. Set 'is_background: true' for long-running servers, deep scans, or watchers. [USE ONLY when no purpose-built tool applies]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The exact shell command line to execute."
                },
                "is_background": {
                    "type": "boolean",
                    "description": "Set to true to launch in background and receive PID."
                }
            },
            "required": ["command"]
        })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let allowed: HashSet<String> = context.allowed_commands.iter().cloned().collect();
        !is_known_safe_command_with_allowed(cmd, &allowed)
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let command_str = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("Missing 'command' parameter.")),
        };

        let is_background = args
            .get("is_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let askpass_path = get_or_create_askpass_script();
        let full_script = format!("{} {}", BASH_SHOPT_GUARD, command_str);

        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(&full_script)
            .current_dir(&context.workspace_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("SUDO_ASKPASS", &askpass_path)
            .env("SSH_ASKPASS", &askpass_path);

        if let Some(ref pwd) = context.sudo_password {
            cmd.env("UTI_SUDO_PASSWORD", pwd);
            cmd.env("DEEPSEEK_SUDO_PASSWORD", pwd);
        }

        if is_background {
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => return Ok(ToolOutput::error(format!("Failed to launch background process: {}", e))),
            };

            let pid = child.id().unwrap_or(0);
            let output_buffer = Arc::new(Mutex::new(String::new()));
            let is_running = Arc::new(Mutex::new(true));

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let buf_clone = output_buffer.clone();
            let running_clone = is_running.clone();

            tokio::spawn(async move {
                let mut reader_stdout = stdout.map(BufReader::new);
                let mut reader_stderr = stderr.map(BufReader::new);

                let mut line_out = String::new();
                let mut line_err = String::new();

                loop {
                    tokio::select! {
                        res = async {
                            if let Some(ref mut r) = reader_stdout {
                                line_out.clear();
                                r.read_line(&mut line_out).await
                            } else {
                                std::future::pending().await
                            }
                        } => {
                            match res {
                                Ok(0) | Err(_) => reader_stdout = None,
                                Ok(_) => {
                                    if let Ok(mut buf) = buf_clone.lock() {
                                        buf.push_str(&line_out);
                                        if buf.len() > 100_000 {
                                            let cut = buf.len() - 100_000;
                                            *buf = buf[cut..].to_string();
                                        }
                                    }
                                }
                            }
                        }
                        res = async {
                            if let Some(ref mut r) = reader_stderr {
                                line_err.clear();
                                r.read_line(&mut line_err).await
                            } else {
                                std::future::pending().await
                            }
                        } => {
                            match res {
                                Ok(0) | Err(_) => reader_stderr = None,
                                Ok(_) => {
                                    if let Ok(mut buf) = buf_clone.lock() {
                                        buf.push_str(&line_err);
                                        if buf.len() > 100_000 {
                                            let cut = buf.len() - 100_000;
                                            *buf = buf[cut..].to_string();
                                        }
                                    }
                                }
                            }
                        }
                        _ = child.wait() => {
                            if let Ok(mut r) = running_clone.lock() {
                                *r = false;
                            }
                            break;
                        }
                    }

                    if reader_stdout.is_none() && reader_stderr.is_none() {
                        if let Ok(mut r) = running_clone.lock() {
                            *r = false;
                        }
                        break;
                    }
                }
            });

            if let Ok(mut mgr) = get_task_manager().lock() {
                mgr.register(pid, command_str.to_string(), output_buffer, is_running, None);
            }

            Ok(ToolOutput::success_with_summary(
                format!("Process launched in background (PID: {}). Command: {}", pid, command_str),
                format!("Background PID: {}", pid),
            ))
        } else {
            let output = match tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await {
                Ok(Ok(o)) => o,
                Ok(Err(e)) => return Ok(ToolOutput::error(format!("Failed to execute command: {}", e))),
                Err(_) => return Ok(ToolOutput::error("Command execution timed out after 30s.".to_string())),
            };

            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let stderr_str = String::from_utf8_lossy(&output.stderr);

            let mut combined = String::new();
            if !stdout_str.is_empty() {
                combined.push_str(&stdout_str);
            }
            if !stderr_str.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&stderr_str);
            }

            if combined.is_empty() {
                combined = "(Command completed with no output)".to_string();
            }

            if output.status.success() {
                Ok(ToolOutput::success(combined))
            } else {
                let code = output.status.code().unwrap_or(-1);
                Ok(ToolOutput::error(format!("Command exited with status code {}:\n{}", code, combined)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_known_safe_command() {
        assert!(is_known_safe_command("cd /home/user/Documents && df -h /home | tail -1"));
        assert!(!is_known_safe_command("cd /home/user/Documents && df -h /home | tail -1 && rm -v archive.tar.gz"));
        assert!(!is_known_safe_command("rm -rf /"));
    }

    #[test]
    fn test_env_prefix_is_ignored() {
        assert!(is_known_safe_command("LANG=C df -h /home"));
        assert!(is_known_safe_command("GIT_PAGER=cat LC_ALL=C git log --oneline"));
        assert!(is_known_safe_command("FOO=bar baz=qux ls -la"));
        assert!(!is_known_safe_command("LANG=C rm -rf /"));
    }

    #[test]
    fn test_benign_redirections() {
        assert!(is_known_safe_command("df -h > /dev/null"));
        assert!(is_known_safe_command("df -h 2> /dev/null"));
        assert!(is_known_safe_command("df -h >/dev/null 2>&1"));
        assert!(is_known_safe_command("df -h 2>&1 | tail -1"));
        assert!(is_known_safe_command("cat /etc/hosts < /dev/null"));
        assert!(is_known_safe_command("bc <<< \"2+2\""));
        assert!(!is_known_safe_command("df -h > /tmp/out.txt"));
        assert!(!is_known_safe_command("df -h 2> /tmp/err.log"));
        assert!(!is_known_safe_command("cat /etc/hosts < input.txt"));
    }

    #[test]
    fn test_expanded_safe_list() {
        assert!(is_known_safe_command("ss -tulpn"));
        assert!(is_known_safe_command("netstat -tulpn"));
        assert!(is_known_safe_command("lsof -i :8080"));
        assert!(is_known_safe_command("md5sum file.iso"));
        assert!(is_known_safe_command("sha256sum file.iso"));
        assert!(is_known_safe_command("diff -u a.txt b.txt"));
        assert!(is_known_safe_command("basename /tmp/x"));
        assert!(is_known_safe_command("readlink -f /usr/bin/python3"));
        assert!(is_known_safe_command("pgrep -f nginx"));
        assert!(is_known_safe_command("mount"));
        assert!(!is_known_safe_command("mount /dev/sdb1 /mnt"));
        assert!(is_known_safe_command("bc -l"));
        assert!(is_known_safe_command("dpkg -l"));
        assert!(!is_known_safe_command("dpkg -i pkg.deb"));
        assert!(is_known_safe_command("dpkg --list"));
        assert!(!is_known_safe_command("dpkg --configure -a"));
    }

    #[test]
    fn test_find_validation() {
        assert!(is_known_safe_command("find . -name \"*.rs\" -print"));
        assert!(is_known_safe_command("find /var/log -type f -mtime -1"));
        assert!(!is_known_safe_command("find . -exec rm {} \\;"));
        assert!(!is_known_safe_command("find . -delete"));
        assert!(!is_known_safe_command("find . -fprintf /tmp/out.txt '%p\\n'"));
    }

    #[test]
    fn test_tar_unzip_validation() {
        assert!(is_known_safe_command("tar -tf archive.tar.gz"));
        assert!(is_known_safe_command("tar -tvf archive.tar.gz"));
        assert!(is_known_safe_command("tar --list -f archive.tar"));
        assert!(!is_known_safe_command("tar -xzf archive.tar.gz"));
        assert!(!is_known_safe_command("tar -czf out.tgz ."));
        assert!(is_known_safe_command("unzip -l archive.zip"));
        assert!(is_known_safe_command("unzip -t archive.zip"));
        assert!(!is_known_safe_command("unzip archive.zip"));
        assert!(!is_known_safe_command("unzip -o archive.zip -d /tmp"));
    }

    #[test]
    fn test_systemctl_validation() {
        assert!(is_known_safe_command("systemctl status sshd"));
        assert!(is_known_safe_command("systemctl is-enabled sshd"));
        assert!(is_known_safe_command("systemctl list-units --type=service --all"));
        assert!(is_known_safe_command("systemctl --failed"));
        assert!(!is_known_safe_command("systemctl start nginx"));
        assert!(!is_known_safe_command("systemctl stop sshd"));
        assert!(!is_known_safe_command("systemctl enable sshd"));
    }

    #[test]
    fn test_git_validation() {
        assert!(is_known_safe_command("git status"));
        assert!(is_known_safe_command("git log --oneline -10"));
        assert!(is_known_safe_command("git branch"));
        assert!(is_known_safe_command("git branch -a"));
        assert!(!is_known_safe_command("git branch feature/x"));
        assert!(!is_known_safe_command("git branch -d old-branch"));
        assert!(is_known_safe_command("git tag -l"));
        assert!(!is_known_safe_command("git tag v1.0"));
        assert!(is_known_safe_command("git remote -v"));
        assert!(!is_known_safe_command("git remote add origin git@x"));
        assert!(is_known_safe_command("git config --list"));
        assert!(is_known_safe_command("git config user.name"));
        assert!(!is_known_safe_command("git config user.name \"John Doe\""));
    }

    #[test]
    fn test_wrapper_commands() {
        assert!(is_known_safe_command("env"));
        assert!(is_known_safe_command("env | grep PATH"));
        assert!(is_known_safe_command("LANG=C env PATH=/usr/bin ls"));
        assert!(is_known_safe_command("nice ls -la"));
        assert!(is_known_safe_command("timeout 5 curl -I https://example.com"));
        assert!(!is_known_safe_command("env rm -rf /"));
        assert!(!is_known_safe_command("timeout 5 rm -rf /tmp/x"));
        assert!(!is_known_safe_command("time rm -rf /"));
    }

    #[test]
    fn test_allowed_commands() {
        let allowed: HashSet<String> = ["mycli", "toolbox inspect"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(is_known_safe_command_with_allowed("mycli do-stuff", &allowed));
        assert!(is_known_safe_command_with_allowed("toolbox inspect foo", &allowed));
        assert!(is_known_safe_command_with_allowed("MYVAR=1 mycli do-stuff", &allowed));
        assert!(!is_known_safe_command_with_allowed("mycli do-stuff > /tmp/out", &allowed));
        assert!(!is_known_safe_command_with_allowed("othercli do-stuff", &allowed));
    }
}
