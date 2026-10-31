//! File edits made by the agent's tools, kept per user turn so `/rewind` can undo them.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Files larger than this are not snapshotted, so their edits cannot be rewound.
const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024;
/// Edits waiting to be claimed by a turn; one-shot CLI runs never claim them.
const MAX_PENDING: usize = 1_000;

/// One file change. `None` means the file did not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEdit {
    /// 1-based user turn the edit belongs to (see `Session::user_turns`).
    pub turn: usize,
    pub path: PathBuf,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// A change waiting for its turn: path, before, after.
type PendingEdit = (PathBuf, Option<String>, Option<String>);

static PENDING: Mutex<Vec<PendingEdit>> = Mutex::new(Vec::new());

/// The file's text: `Some(None)` when it does not exist, `None` when it cannot be kept
/// (binary, unreadable or too large).
pub fn snapshot(path: &Path) -> Option<Option<String>> {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(None),
        Err(_) => None,
        Ok(meta) if !meta.is_file() || meta.len() > MAX_SNAPSHOT_BYTES => None,
        Ok(_) => std::fs::read_to_string(path).ok().map(Some),
    }
}

pub fn record(path: PathBuf, before: Option<String>, after: Option<String>) {
    if before == after {
        return;
    }
    if let Ok(mut pending) = PENDING.lock() {
        if pending.len() >= MAX_PENDING {
            pending.remove(0);
        }
        pending.push((path, before, after));
    }
}

/// Takes the edits recorded since the last call and assigns them to `turn`.
pub fn drain(turn: usize) -> Vec<FileEdit> {
    let Ok(mut pending) = PENDING.lock() else {
        return Vec::new();
    };
    pending
        .drain(..)
        .map(|(path, before, after)| FileEdit { turn, path, before, after })
        .collect()
}

/// Lines added and removed going from `before` to `after`.
pub fn line_stats(before: Option<&str>, after: Option<&str>) -> (usize, usize) {
    let diff = similar::TextDiff::from_lines(before.unwrap_or(""), after.unwrap_or(""));
    diff.iter_all_changes().fold((0, 0), |(added, removed), change| match change.tag() {
        similar::ChangeTag::Insert => (added + 1, removed),
        similar::ChangeTag::Delete => (added, removed + 1),
        similar::ChangeTag::Equal => (added, removed),
    })
}

/// Undoes `edits`, newest first. A file still as the agent left it gets its old content back;
/// one changed since gets the edit reversed as a patch, and is left alone (with a warning) when
/// that patch no longer applies. Returns the warnings.
pub fn revert(edits: &[FileEdit]) -> Vec<String> {
    let mut warnings = Vec::new();
    for edit in edits.iter().rev() {
        let name = edit.path.display();
        let current = match snapshot(&edit.path) {
            Some(current) => current,
            None => {
                warnings.push(format!("{name}: could not read it, left as is"));
                continue;
            }
        };
        let restored = if current == edit.after {
            edit.before.clone()
        } else {
            match (&current, &edit.after) {
                (Some(current), Some(after)) => {
                    let undo = diffy::create_patch(after, edit.before.as_deref().unwrap_or(""));
                    match diffy::apply(current, &undo) {
                        Ok(text) if text.is_empty() && edit.before.is_none() => None,
                        Ok(text) => Some(text),
                        Err(_) => {
                            warnings.push(format!("{name}: changed since, the edit no longer reverses cleanly"));
                            continue;
                        }
                    }
                }
                (None, Some(_)) => {
                    warnings.push(format!("{name}: deleted since, not restored"));
                    continue;
                }
                (Some(_), None) => {
                    warnings.push(format!("{name}: recreated since, left as is"));
                    continue;
                }
                (None, None) => continue,
            }
        };
        let result = match &restored {
            Some(text) => edit
                .path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| std::fs::write(&edit.path, text)),
            None => std::fs::remove_file(&edit.path),
        };
        if let Err(e) = result {
            warnings.push(format!("{name}: {e}"));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("corex_file_history_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn edit(path: &Path, before: Option<&str>, after: Option<&str>) -> FileEdit {
        FileEdit { turn: 1, path: path.to_path_buf(), before: before.map(String::from), after: after.map(String::from) }
    }

    #[test]
    fn reverts_edits_newest_first_and_removes_created_files() {
        let d = dir("order");
        let (a, b) = (d.join("a.txt"), d.join("new.txt"));
        std::fs::write(&a, "three\n").unwrap();
        std::fs::write(&b, "x\n").unwrap();
        let edits = [
            edit(&a, Some("one\n"), Some("two\n")),
            edit(&b, None, Some("x\n")),
            edit(&a, Some("two\n"), Some("three\n")),
        ];

        let warnings = revert(&edits);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "one\n");
        assert!(!b.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn keeps_later_user_changes_when_reversing_a_patch() {
        let d = dir("merge");
        let f = d.join("code.rs");
        std::fs::write(&f, "user header\na\nB\nc\n").unwrap();

        let warnings = revert(&[edit(&f, Some("a\nb\nc\n"), Some("a\nB\nc\n"))]);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "user header\na\nb\nc\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn warns_instead_of_clobbering_conflicting_changes() {
        let d = dir("conflict");
        let f = d.join("code.rs");
        std::fs::write(&f, "totally different\n").unwrap();

        let warnings = revert(&[edit(&f, Some("a\n"), Some("b\n"))]);

        assert_eq!(warnings.len(), 1);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "totally different\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn restores_a_file_the_agent_deleted() {
        let d = dir("deleted");
        let f = d.join("gone.txt");

        assert!(revert(&[edit(&f, Some("kept\n"), None)]).is_empty());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "kept\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn counts_added_and_removed_lines() {
        assert_eq!(line_stats(Some("a\nb\n"), Some("a\nc\nd\n")), (2, 1));
        assert_eq!(line_stats(None, Some("x\ny\n")), (2, 0));
    }
}
