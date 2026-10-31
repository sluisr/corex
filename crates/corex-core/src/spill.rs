//! Oversized tool output kept on disk instead of in the context window.
//!
//! A result that is too big to send is cut to its head and tail, and the complete text is saved to a
//! private file whose path goes into the cut itself. Nothing is lost: the model can open the file
//! with `read_file` (by line range) when the middle matters.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Substring present in every cut made by this module; it is how already-trimmed text is recognised.
pub const TRIM_MARKER: &str = "tool result middle pruned";

/// A tool result longer than this (in characters) is cut when it is produced.
pub const SPILL_THRESHOLD_CHARS: usize = 30_000;
const SPILL_HEAD_CHARS: usize = 10_000;
const SPILL_TAIL_CHARS: usize = 2_000;

/// Spill files older than this are deleted the first time the directory is used in a process.
const SPILL_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(3 * 24 * 3600);

/// Private directory holding spilled output, one per user.
pub fn spill_dir() -> PathBuf {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string());
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        std::env::temp_dir().join(format!("corex-spill-{}-{}", safe, uid))
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir().join(format!("corex-spill-{}", safe))
    }
}

/// True when `path` lies inside the spill directory (symlinks resolved), so reading it needs no prompt.
pub fn is_spill_path(path: &Path) -> bool {
    let (Ok(dir), Ok(target)) = (spill_dir().canonicalize(), path.canonicalize()) else {
        return false;
    };
    target.starts_with(dir)
}

fn prepared_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = spill_dir();
        if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
            return None;
        }
        crate::ensure_private_dir(&dir).ok()?;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > SPILL_MAX_AGE);
                if old {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Some(dir)
    })
    .clone()
}

/// Saves `text` to a new private file and returns its path.
pub fn spill_text(label: &str, text: &str) -> Option<PathBuf> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = prepared_dir()?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(40)
        .collect();
    let path = dir.join(format!("{}-{}-{}-{}.txt", ts, std::process::id(), n, label));
    crate::write_private(&path, text).ok()?;
    Some(path)
}

/// Keeps the first `head` and last `tail` characters of `text`, with a note between them that says
/// where the complete text was saved (when saving worked).
pub fn trim_with_spill(label: &str, text: &str, head: usize, tail: usize) -> String {
    let total = text.chars().count();
    let head_text: String = text.chars().take(head).collect();
    let tail_text: String = text.chars().skip(total.saturating_sub(tail)).collect();
    let note = match spill_text(label, text) {
        Some(path) => format!(
            "[... {} ({} characters in total); the full output is saved at {} — read it with read_file using start_line/end_line if you need the middle ...]",
            TRIM_MARKER,
            total,
            path.display()
        ),
        None => format!("[... {} ({} characters in total) ...]", TRIM_MARKER, total),
    };
    format!("{}\n\n{}\n\n{}", head_text, note, tail_text)
}

/// Cuts a freshly produced tool result when it is too large to send whole.
pub fn spill_large_output(tool_name: &str, output: String) -> String {
    if output.chars().count() <= SPILL_THRESHOLD_CHARS {
        return output;
    }
    trim_with_spill(tool_name, &output, SPILL_HEAD_CHARS, SPILL_TAIL_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_output_is_cut_and_the_full_text_is_recoverable() {
        let full: String = (0..6000).map(|i| format!("line {:05}\n", i)).collect(); // 66k chars
        let out = spill_large_output("grep_search", full.clone());
        assert!(out.chars().count() < full.chars().count() / 3);
        assert!(out.starts_with("line 00000"));
        assert!(out.trim_end().ends_with("line 05999"));
        assert!(out.contains(TRIM_MARKER));

        let path = out
            .split("saved at ")
            .nth(1)
            .and_then(|r| r.split(" — ").next())
            .expect("a path in the note");
        assert!(is_spill_path(Path::new(path)));
        assert_eq!(std::fs::read_to_string(path).unwrap(), full);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn small_output_is_untouched_and_foreign_paths_are_not_spill_paths() {
        assert_eq!(spill_large_output("x", "short".to_string()), "short");
        assert!(!is_spill_path(Path::new("/etc/hostname")));
    }

    #[cfg(unix)]
    #[test]
    fn spill_dir_is_named_after_the_uid() {
        let uid = unsafe { libc::getuid() }.to_string();
        let dir_name = spill_dir()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(
            dir_name.contains(&uid),
            "spill_dir ({dir_name}) must include numeric UID ({uid}) on Unix"
        );
    }
}
