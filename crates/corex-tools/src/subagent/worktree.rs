//! Isolated `git worktree` sandbox for parallel subagents.
//!
//! When a subagent is launched with `isolated: true`, it runs inside a temporary detached
//! `git worktree` so its edits and builds never collide with the user's working tree.
//! On completion, any diff produced in the worktree is captured and attached to the report
//! before the temporary worktree is removed.

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct IsolatedWorktree {
    repo_root: PathBuf,
    path: PathBuf,
}

impl IsolatedWorktree {
    /// Creates a detached temporary `git worktree` rooted at `HEAD` for `workspace`.
    /// Returns `Err` if `workspace` is not inside a git repository or `git` fails.
    pub fn create(workspace: &Path, task_id: &str) -> Result<Self, String> {
        let root_out = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(workspace)
            .output()
            .map_err(|e| format!("Failed to invoke git: {e}"))?;
        if !root_out.status.success() {
            return Err("Workspace is not a git repository; cannot create isolated worktree.".to_string());
        }
        let repo_root = PathBuf::from(String::from_utf8_lossy(&root_out.stdout).trim());

        let safe_id: String = task_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(24)
            .collect();
        let wt_dir = std::env::temp_dir().join(format!(
            "corex-wt-{}-{}",
            std::process::id(),
            if safe_id.is_empty() { "task" } else { &safe_id }
        ));
        if wt_dir.exists() {
            let _ = std::fs::remove_dir_all(&wt_dir);
        }

        let add = Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&wt_dir)
            .arg("HEAD")
            .current_dir(&repo_root)
            .output()
            .map_err(|e| format!("Failed to run git worktree add: {e}"))?;
        if !add.status.success() {
            let err = String::from_utf8_lossy(&add.stderr).trim().to_string();
            return Err(format!("git worktree add failed: {err}"));
        }

        Ok(Self {
            repo_root,
            path: wt_dir,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Captures a summary of modified/untracked files and unified diff inside the worktree.
    pub fn capture_diff_summary(&self) -> Option<String> {
        let status = Command::new("git")
            .args(["status", "--short"])
            .current_dir(&self.path)
            .output()
            .ok()?;
        let status_text = String::from_utf8_lossy(&status.stdout).trim().to_string();
        if status_text.is_empty() {
            return None;
        }
        let diff = Command::new("git")
            .args(["diff", "--no-color"])
            .current_dir(&self.path)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        let mut summary = format!("\n\n---\nIsolated worktree changes:\n```\n{status_text}\n```");
        if !diff.is_empty() {
            let clipped = corex_core::types::safe_truncate_str(&diff, 16_000);
            summary.push_str(&format!("\n```diff\n{clipped}\n```"));
        }
        Some(summary)
    }
}

impl Drop for IsolatedWorktree {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.repo_root)
            .output();
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_worktree_creates_captures_diff_and_cleans_up() {
        let repo = std::env::temp_dir().join(format!("corex-wt-test-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();

        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .expect("git must run");
            assert!(out.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
        };

        git(&["init"]);
        std::fs::write(repo.join("hello.txt"), "v1\n").unwrap();
        git(&["add", "hello.txt"]);
        git(&["commit", "-m", "init"]);

        let wt_path;
        {
            let wt = IsolatedWorktree::create(&repo, "task_wt_1").expect("worktree should create");
            wt_path = wt.path().to_path_buf();
            assert!(wt_path.join("hello.txt").exists());
            std::fs::write(wt_path.join("hello.txt"), "v2\n").unwrap();
            // Main working tree stays untouched!
            assert_eq!(std::fs::read_to_string(repo.join("hello.txt")).unwrap(), "v1\n");
            let summary = wt.capture_diff_summary().expect("diff should be captured");
            assert!(summary.contains("+v2"), "{summary}");
        }
        assert!(!wt_path.exists(), "worktree directory should be removed on drop");
        let _ = std::fs::remove_dir_all(&repo);
    }
}
