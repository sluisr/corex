use std::collections::HashSet;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use anyhow::Result;
use async_trait::async_trait;
use directories::BaseDirs;
use chrono::Utc;
use regex::Regex;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
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

    if fs::write(&script_path, script_content).is_ok() {
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&script_path).map(|m| m.permissions()).unwrap_or_else(|_| fs::Permissions::from_mode(0o755));
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&script_path, perms);
        }
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

/// Returns true if the given shell command requires sudo, doas, or pkexec privileges.
pub fn command_requires_sudo(cmd_str: &str) -> bool {
    let trimmed = cmd_str.trim();
    if trimmed.is_empty() {
        return false;
    }
    let segments = split_segments(trimmed);
    for segment in segments {
        if is_single_segment_sudo(segment) {
            return true;
        }
    }
    // Also check for command substitutions like $(sudo ...) or `sudo ...`
    if cmd_str.contains("$(sudo") || cmd_str.contains("`sudo") || cmd_str.contains("$(doas") || cmd_str.contains("`doas") {
        return true;
    }
    false
}

/// Extracts the first command segment that requires sudo, doas, or pkexec privileges.
/// For compound commands (e.g. `echo "===..." && sudo sed -i ...` or `echo 123 | sudo tee ...`),
/// this isolates the actual privileged command to display cleanly in prompts and dialogs.
pub fn extract_first_sudo_command(cmd_str: &str) -> Option<String> {
    let trimmed = cmd_str.trim();
    if trimmed.is_empty() {
        return None;
    }

    let segments = split_segments(trimmed);
    for segment in segments {
        if is_single_segment_sudo(segment) {
            let mut clean = segment.trim();
            // Strip outer subshell parenthesis if present: (sudo ...) -> sudo ...
            while clean.starts_with('(') && clean.ends_with(')') {
                clean = clean[1..clean.len() - 1].trim();
            }
            while clean.starts_with('(') {
                clean = clean[1..].trim();
            }
            while clean.ends_with(')') {
                clean = clean[..clean.len() - 1].trim();
            }
            // If bash -c "sudo ...", unwrap inner command
            let parts: Vec<&str> = clean.split_whitespace().collect();
            let raw_cmd = parts.first().map(|p| p.trim_matches(|c| c == '\'' || c == '"')).unwrap_or("");
            let base_cmd = Path::new(raw_cmd).file_name().and_then(|f| f.to_str()).unwrap_or(raw_cmd);
            if (base_cmd == "bash" || base_cmd == "sh" || base_cmd == "zsh") && parts.len() > 2 && parts[1] == "-c" {
                let subcmd = parts[2..].join(" ");
                let unquoted = subcmd.trim_matches(|c| c == '\'' || c == '"');
                if let Some(inner) = extract_first_sudo_command(unquoted) {
                    return Some(inner);
                }
            }
            return Some(clean.to_string());
        }
    }

    // Fallback check for command substitutions like $(sudo ...) or `sudo ...`
    for prefix in &["$(sudo", "$(doas", "$(pkexec"] {
        if let Some(pos) = trimmed.find(prefix) {
            if let Some(end) = trimmed[pos..].find(')') {
                let inner = &trimmed[pos + 2..pos + end];
                return Some(inner.trim().to_string());
            }
        }
    }
    for prefix in &["`sudo", "`doas", "`pkexec"] {
        if let Some(pos) = trimmed.find(prefix) {
            if let Some(end) = trimmed[pos + 1..].find('`') {
                let inner = &trimmed[pos + 1..pos + 1 + end];
                return Some(inner.trim().to_string());
            }
        }
    }

    if command_requires_sudo(trimmed) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn is_single_segment_sudo(segment: &str) -> bool {
    let mut cleaned = segment.trim();
    // Strip leading subshell parenthesis if present
    while cleaned.starts_with('(') {
        cleaned = cleaned[1..].trim();
    }
    let mut parts: Vec<&str> = cleaned.split_whitespace().collect();
    if parts.is_empty() {
        return false;
    }

    // Skip leading VAR=value environment assignments
    while !parts.is_empty() && is_env_assignment(parts[0]) {
        parts.remove(0);
    }

    if parts.is_empty() {
        return false;
    }

    let raw_cmd = parts[0].trim_matches(|c| c == '\'' || c == '"');
    let cmd = Path::new(raw_cmd)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(raw_cmd);

    if cmd == "sudo" || cmd == "doas" || cmd == "pkexec" {
        return true;
    }

    // Check wrappers like `env`, `nohup`, `nice`, `time`, `timeout`, `stdbuf`, `watch`, `xargs`, `exec`
    if is_wrapper(cmd) || cmd == "xargs" || cmd == "exec" {
        let mut j = 1;
        while j < parts.len() && (parts[j].starts_with('-') || parts[j].chars().all(|c| c.is_ascii_digit())) {
            j += 1;
        }
        while j < parts.len() && is_env_assignment(parts[j]) {
            j += 1;
        }
        if j < parts.len() {
            return is_single_segment_sudo(&parts[j..].join(" "));
        }
    }

    // Check find with -exec or -ok
    if cmd == "find" {
        for (i, part) in parts.iter().enumerate() {
            if (*part == "-exec" || *part == "-ok") && i + 1 < parts.len() {
                let target = parts[i + 1].trim_matches(|c| c == '\'' || c == '"');
                let base = Path::new(target).file_name().and_then(|f| f.to_str()).unwrap_or(target);
                if base == "sudo" || base == "doas" || base == "pkexec" {
                    return true;
                }
            }
        }
    }

    // Check bash -c "sudo ..." or sh -c "sudo ..."
    if (cmd == "bash" || cmd == "sh" || cmd == "zsh") && parts.len() > 2 && parts[1] == "-c" {
        let subcmd = parts[2..].join(" ");
        let unquoted = subcmd.trim_matches(|c| c == '\'' || c == '"');
        return command_requires_sudo(unquoted);
    }

    false
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
    cmd.split([';', '&', '|'])
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

fn append_and_truncate_output(buf: &mut String, text: &str, max_len: usize) {
    buf.push_str(text);
    if buf.len() > max_len {
        let mut cut = buf.len() - max_len;
        while cut < buf.len() && !buf.is_char_boundary(cut) {
            cut += 1;
        }
        *buf = buf[cut..].to_string();
    }
}

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        "run_shell_command"
    }

    fn description(&self) -> &'static str {
        "Executes a bash shell command with adaptive execution timeout. By default waits up to 5000ms; if the command completes within that window, returns output immediately. If it runs longer (e.g. servers, long builds, watchers), it automatically detaches to a background task with a Task ID (PID). Set 'wait_ms_before_async' to configure the wait window or 'is_background: true' to detach immediately."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The exact shell command line to execute. Execute direct, clean commands; never prepend decorative echo banners."
                },
                "wait_ms_before_async": {
                    "type": "integer",
                    "description": "Milliseconds to wait before automatically detaching to background (default: 5000ms). If the command completes within this time, returns output synchronously. If it runs longer, detaches into a background task."
                },
                "is_background": {
                    "type": "boolean",
                    "description": "Set to true to immediately launch in background without waiting (equivalent to wait_ms_before_async=0)."
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory for the command (defaults to current workspace directory)."
                }
            },
            "required": ["command"]
        })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        let cmd = args.get("command")
            .or_else(|| args.get("CommandLine"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let allowed: HashSet<String> = context.allowed_commands.iter().cloned().collect();
        !is_known_safe_command_with_allowed(cmd, &allowed)
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let command_str = match args.get("command")
            .or_else(|| args.get("CommandLine"))
            .and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("Missing 'command' parameter.")),
        };

        let is_background = args
            .get("is_background")
            .or_else(|| args.get("isBackground"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let wait_ms = if is_background {
            0
        } else {
            args.get("wait_ms_before_async")
                .or_else(|| args.get("WaitMsBeforeAsync"))
                .and_then(|v| v.as_u64())
                .unwrap_or(5000)
        };

        let working_dir = if let Some(cwd_str) = args.get("cwd")
            .or_else(|| args.get("Cwd"))
            .and_then(|v| v.as_str()) {
            let p = PathBuf::from(cwd_str);
            if p.is_absolute() {
                p
            } else {
                context.workspace_dir.join(p)
            }
        } else {
            context.workspace_dir.clone()
        };

        let askpass_path = get_or_create_askpass_script();
        let full_script = format!("{} {}", BASH_SHOPT_GUARD, command_str);

        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(&full_script)
            .current_dir(&working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("SUDO_ASKPASS", &askpass_path)
            .env("SSH_ASKPASS", &askpass_path);

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        if let Some(ref pwd) = context.sudo_password {
            cmd.env("UTI_SUDO_PASSWORD", pwd);
            cmd.env("DEEPSEEK_SUDO_PASSWORD", pwd);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to execute command: {}", e))),
        };

        let pid = child.id().unwrap_or(0);
        let output_buffer = Arc::new(Mutex::new(String::new()));
        let is_running = Arc::new(Mutex::new(true));
        let finished_at = Arc::new(Mutex::new(None));
        let exit_code = Arc::new(Mutex::new(None));
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<String>(32);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<std::process::ExitStatus>();

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut stdin = child.stdin.take();

        let buf_clone = output_buffer.clone();
        let running_clone = is_running.clone();
        let finished_at_clone = finished_at.clone();
        let exit_code_clone = exit_code.clone();

        tokio::spawn(async move {
            let mut reader_stdout = stdout.map(BufReader::new);
            let mut reader_stderr = stderr.map(BufReader::new);

            let mut line_out = String::new();
            let mut line_err = String::new();
            let mut done_tx_opt = Some(done_tx);

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
                                    append_and_truncate_output(&mut buf, &line_out, 200_000);
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
                                    append_and_truncate_output(&mut buf, &line_err, 200_000);
                                }
                            }
                        }
                    }
                    input_msg = stdin_rx.recv() => {
                        if let Some(data) = input_msg {
                            if let Some(ref mut sin) = stdin {
                                let _ = sin.write_all(data.as_bytes()).await;
                                let _ = sin.flush().await;
                            }
                        }
                    }
                    wait_res = child.wait() => {
                        if let Some(ref mut r) = reader_stdout {
                            let mut rest_bytes = Vec::new();
                            if r.take(200_000).read_to_end(&mut rest_bytes).await.is_ok() && !rest_bytes.is_empty() {
                                let rest = String::from_utf8_lossy(&rest_bytes);
                                if let Ok(mut buf) = buf_clone.lock() {
                                    append_and_truncate_output(&mut buf, &rest, 200_000);
                                }
                            }
                        }
                        if let Some(ref mut r) = reader_stderr {
                            let mut rest_bytes = Vec::new();
                            if r.take(200_000).read_to_end(&mut rest_bytes).await.is_ok() && !rest_bytes.is_empty() {
                                let rest = String::from_utf8_lossy(&rest_bytes);
                                if let Ok(mut buf) = buf_clone.lock() {
                                    append_and_truncate_output(&mut buf, &rest, 200_000);
                                }
                            }
                        }

                        let status_val = wait_res.ok();
                        let code = status_val.as_ref().and_then(|s| s.code());
                        if let Ok(mut r) = running_clone.lock() {
                            *r = false;
                        }
                        if let Ok(mut ec) = exit_code_clone.lock() {
                            *ec = code;
                        }
                        if let Ok(mut fa) = finished_at_clone.lock() {
                            *fa = Some(Utc::now());
                        }
                        if let Some(tx) = done_tx_opt.take() {
                            if let Some(s) = status_val {
                                let _ = tx.send(s);
                            }
                        }
                        break;
                    }
                }

                if reader_stdout.is_none() && reader_stderr.is_none() {
                    if let Ok(r) = running_clone.lock() {
                        if !*r {
                            break;
                        }
                    }
                }
            }
        });

        // Register with TaskManager
        let is_background = Arc::new(Mutex::new(wait_ms == 0));
        let mgr = get_task_manager();
        if let Ok(mut m) = mgr.lock() {
            m.register(
                pid,
                command_str.to_string(),
                output_buffer.clone(),
                is_running.clone(),
                finished_at.clone(),
                exit_code.clone(),
                is_background.clone(),
                Some(stdin_tx),
            );
        }

        if wait_ms == 0 {
            return Ok(ToolOutput::success_with_summary(
                format!(
                    "[BACKGROUND TASK LAUNCHED]\n\
                     Task ID (PID): {}\n\
                     Command: {}\n\
                     Status: RUNNING in background.\n\
                     Use 'manage_task' (action: 'status' | 'kill' | 'send_input') or '/tasks' to monitor.",
                    pid, command_str
                ),
                format!("Background PID: {}", pid),
            ));
        }

        match tokio::time::timeout(std::time::Duration::from_millis(wait_ms), done_rx).await {
            Ok(Ok(status)) => {
                let output = output_buffer.lock().map(|b| b.clone()).unwrap_or_default();
                let display_output = if output.trim().is_empty() {
                    "(Command completed with no output)".to_string()
                } else {
                    output
                };

                if status.success() {
                    Ok(ToolOutput::success(display_output))
                } else {
                    let code = status.code().unwrap_or(-1);
                    Ok(ToolOutput::error(format!("Command exited with status code {}:\n{}", code, display_output)))
                }
            }
            Ok(Err(_)) => {
                let output = output_buffer.lock().map(|b| b.clone()).unwrap_or_default();
                let code = exit_code.lock().ok().and_then(|c| *c).unwrap_or(-1);
                if code == 0 {
                    Ok(ToolOutput::success(output))
                } else {
                    Ok(ToolOutput::error(format!("Command exited with status code {}:\n{}", code, output)))
                }
            }
            Err(_) => {
                if let Ok(mut bg) = is_background.lock() {
                    *bg = true;
                }
                let partial_output = output_buffer.lock().map(|b| b.clone()).unwrap_or_default();
                let snippet = if partial_output.trim().is_empty() {
                    "(No output produced yet)".to_string()
                } else {
                    let lines: Vec<&str> = partial_output.lines().collect();
                    if lines.len() > 10 {
                        format!("... [truncated]\n{}", lines[lines.len() - 10..].join("\n"))
                    } else {
                        partial_output
                    }
                };

                Ok(ToolOutput::success_with_summary(
                    format!(
                        "[COMMAND SENT TO BACKGROUND]\n\
                         Command exceeded wait limit ({}ms) and is continuing in background.\n\
                         Task ID (PID): {}\n\
                         Command: {}\n\
                         Status: RUNNING\n\
                         Recent output:\n{}\n\n\
                         Use tool 'manage_task' (action: 'status' | 'kill' | 'send_input') or '/tasks' to manage.",
                        wait_ms, pid, command_str, snippet
                    ),
                    format!("Sent to background: PID {}", pid),
                ))
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

    #[test]
    fn test_command_requires_sudo() {
        assert!(command_requires_sudo("sudo apt update"));
        assert!(command_requires_sudo("sudo -E systemctl restart nginx"));
        assert!(command_requires_sudo("/usr/bin/sudo ls -la /root"));
        assert!(command_requires_sudo("echo 123 | sudo tee /proc/sys/vm/drop_caches"));
        assert!(command_requires_sudo("cd /var/log && sudo cat syslog"));
        assert!(command_requires_sudo("VAR=val sudo whoami"));
        assert!(command_requires_sudo("env PATH=/usr/bin sudo systemctl stop docker"));
        assert!(command_requires_sudo("xargs sudo kill -9"));
        assert!(command_requires_sudo("find /var/log -name '*.log' -exec sudo rm {} +"));
        assert!(command_requires_sudo("bash -c 'sudo reboot'"));
        assert!(command_requires_sudo("doas reboot"));
        assert!(command_requires_sudo("pkexec systemctl restart bluetooth"));
        assert!(command_requires_sudo("(sudo ls)"));

        // Non-sudo commands should not trigger
        assert!(!command_requires_sudo("ls -la"));
        assert!(!command_requires_sudo("cat /etc/passwd"));
        assert!(!command_requires_sudo("grep sudo /var/log/auth.log"));
        assert!(!command_requires_sudo("echo 'I like sudo'"));
        assert!(!command_requires_sudo("git commit -m 'Fixed sudo bug'"));
        assert!(!command_requires_sudo(""));
    }

    #[test]
    fn test_extract_first_sudo_command() {
        assert_eq!(
            extract_first_sudo_command("sudo apt update"),
            Some("sudo apt update".to_string())
        );
        assert_eq!(
            extract_first_sudo_command("echo '════════ 1) ARREGLAR nsswitch ════════' && sudo sed -i 's/^hosts:.*/hosts: files dns/' /etc/nsswitch.conf"),
            Some("sudo sed -i 's/^hosts:.*/hosts: files dns/' /etc/nsswitch.conf".to_string())
        );
        assert_eq!(
            extract_first_sudo_command("echo 123 | sudo tee /proc/sys/vm/drop_caches"),
            Some("sudo tee /proc/sys/vm/drop_caches".to_string())
        );
        assert_eq!(
            extract_first_sudo_command("cd /var/log && sudo cat syslog"),
            Some("sudo cat syslog".to_string())
        );
        assert_eq!(
            extract_first_sudo_command("(sudo ls -la)"),
            Some("sudo ls -la".to_string())
        );
        assert_eq!(
            extract_first_sudo_command("bash -c 'sudo reboot'"),
            Some("sudo reboot".to_string())
        );
        assert_eq!(
            extract_first_sudo_command("echo $(sudo cat /etc/shadow)"),
            Some("sudo cat /etc/shadow".to_string())
        );
        assert_eq!(
            extract_first_sudo_command("VAR=123 sudo systemctl restart nginx"),
            Some("VAR=123 sudo systemctl restart nginx".to_string())
        );
        assert_eq!(extract_first_sudo_command("ls -la"), None);
        assert_eq!(extract_first_sudo_command("echo 'sudo'"), None);
        assert_eq!(extract_first_sudo_command(""), None);
    }

    #[tokio::test]
    async fn test_shell_tool_fast_command_synchronous() {
        let tool = ShellTool;
        let ctx = ToolContext {
            workspace_dir: std::env::current_dir().unwrap(),
            yolo_mode: true,
            allowed_commands: vec![],
            sudo_password: None,
        };

        let res = tool
            .execute(
                json!({
                    "command": "echo 'antigravity test'",
                    "wait_ms_before_async": 2000
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!res.is_error);
        assert!(res.output.contains("antigravity test"));
        assert!(res.display_summary.is_none());
    }

    #[tokio::test]
    async fn test_shell_tool_slow_command_auto_background() {
        let tool = ShellTool;
        let ctx = ToolContext {
            workspace_dir: std::env::current_dir().unwrap(),
            yolo_mode: true,
            allowed_commands: vec![],
            sudo_password: None,
        };

        let res = tool
            .execute(
                json!({
                    "command": "sleep 1.5 && echo 'done sleeping'",
                    "wait_ms_before_async": 200
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!res.is_error);
        assert!(res.output.contains("[COMMAND SENT TO BACKGROUND]"));
        assert!(res.display_summary.is_some());
    }

    #[test]
    fn test_append_and_truncate_output_utf8() {
        let mut buf = String::new();
        // '€' is 3 bytes in UTF-8
        let euro_text = "a".to_string() + &"€".repeat(10);
        append_and_truncate_output(&mut buf, &euro_text, 10);
        assert!(buf.len() <= 10);
        // Valid UTF-8 must be maintained without panicking
        assert!(!buf.is_empty());
    }
}
