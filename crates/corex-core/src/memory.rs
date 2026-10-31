//! Instruction files ("memory") appended to the system prompt, and the notes `/memory add` and
//! the `save_memory` tool write into them.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use directories::BaseDirs;

/// Per-file cap; longer instruction files are cut and reported as truncated.
pub const MAX_MEMORY_BYTES: usize = 20_000;

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Bumped on every write, so prompt caches know memory changed.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

pub fn mark_changed() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Global,
    Project,
}

/// An instruction file loaded into the system prompt.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub scope: MemoryScope,
    pub path: PathBuf,
    /// `~/…` or `./…`, as shown to the model and the user.
    pub display: String,
    /// The content as loaded, cut to `MAX_MEMORY_BYTES`.
    pub content: String,
    pub original_len: usize,
}

impl MemoryFile {
    pub fn is_truncated(&self) -> bool {
        self.content.len() < self.original_len
    }
}

/// The memory files of one workspace.
pub struct Memory {
    workspace_dir: PathBuf,
}

impl Memory {
    pub fn new(workspace_dir: impl AsRef<Path>) -> Self {
        Self { workspace_dir: workspace_dir.as_ref().to_path_buf() }
    }

    /// Every instruction file that can be loaded, in the order it is appended. The private
    /// project notes come last, after the files that live in the repository.
    pub fn candidates(&self) -> Vec<(MemoryScope, PathBuf)> {
        let mut candidates = Vec::new();
        if let Some(dirs) = BaseDirs::new() {
            let home = dirs.home_dir();
            candidates.extend(
                [
                    home.join(".corex").join("SYSTEM.md"),
                    home.join(".corex").join("COREX.md"),
                    home.join(".deepseek").join("DEEPSEEK.md"),
                ]
                .map(|p| (MemoryScope::Global, p)),
            );
        }
        candidates.extend(
            [
                self.workspace_dir.join(".corex").join("SYSTEM.md"),
                self.workspace_dir.join("COREX.md"),
                self.workspace_dir.join("AGENTS.md"),
                self.workspace_dir.join("DEEPSEEK.md"),
            ]
            .map(|p| (MemoryScope::Project, p)),
        );
        if let Some(private) = self.private_project_file() {
            candidates.push((MemoryScope::Project, private));
        }
        // The workspace can be $HOME, where project and global paths are the same file.
        let mut seen = HashSet::new();
        candidates.retain(|(_, p)| seen.insert(fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
        candidates
    }

    /// The candidates that exist and are readable. All of them are loaded, so a
    /// `COREX.md` never silently hides an `AGENTS.md` next to it.
    pub fn files(&self) -> Vec<MemoryFile> {
        let mut files = Vec::new();
        for (scope, path) in self.candidates() {
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let original_len = raw.len();
            let content = crate::safe_truncate_str(&raw, MAX_MEMORY_BYTES).to_string();
            files.push(MemoryFile {
                scope,
                display: self.display_path(&path),
                path,
                content,
                original_len,
            });
        }
        files
    }

    /// `./…` inside the workspace, `~/…` inside the home directory, absolute otherwise. The
    /// private project file, whose folder name is the whole workspace path, is abbreviated.
    pub fn display_path(&self, path: &Path) -> String {
        if self.private_project_file().as_deref() == Some(path) {
            return "~/.corex/projects/…/MEMORY.md (private to this project)".to_string();
        }
        let home = BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        let bases = [(Some(self.workspace_dir.clone()), "."), (home, "~")];
        bases
            .iter()
            .find_map(|(base, prefix)| {
                let rel = path.strip_prefix(base.as_ref()?).ok()?;
                Some(format!("{}/{}", prefix, rel.display()))
            })
            .unwrap_or_else(|| path.display().to_string())
    }

    /// Notes about this project that stay out of the repository, like deepseek-cli's project
    /// memory: `~/.corex/projects/<workspace path>/MEMORY.md`.
    pub fn private_project_file(&self) -> Option<PathBuf> {
        let workspace = fs::canonicalize(&self.workspace_dir).unwrap_or_else(|_| self.workspace_dir.clone());
        let key: String = workspace
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' })
            .collect();
        let dirs = BaseDirs::new()?;
        Some(dirs.home_dir().join(".corex").join("projects").join(key).join("MEMORY.md"))
    }

    /// Where notes are saved: `~/.corex/COREX.md` for global ones, so `SYSTEM.md` stays
    /// hand-written, and the private project file for project ones.
    pub fn add_target(&self, scope: MemoryScope) -> Option<PathBuf> {
        match scope {
            MemoryScope::Global => BaseDirs::new().map(|d| d.home_dir().join(".corex").join("COREX.md")),
            MemoryScope::Project => self.private_project_file(),
        }
    }

    /// Appends `text` as a bullet to the `scope` target file and returns its display path.
    pub fn add(&self, scope: MemoryScope, text: &str) -> io::Result<String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "nothing to save"));
        }
        let path = self
            .add_target(scope)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory not found"))?;
        let existing = fs::read_to_string(&path).unwrap_or_default();
        let separator = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
        let mut file = crate::secure_fs::open_private(&path, true)?;
        writeln!(file, "{}- {}", separator, text)?;
        mark_changed();
        Ok(self.display_path(&path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corex_memory_{}_{}", name, std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn loads_every_project_file_in_order() {
        let ws = temp_workspace("all");
        let _ = fs::write(ws.join("COREX.md"), "corex rules");
        let _ = fs::write(ws.join("AGENTS.md"), "agents rules");

        let project: Vec<String> = Memory::new(&ws)
            .files()
            .into_iter()
            .filter(|f| f.scope == MemoryScope::Project && f.path.starts_with(&ws))
            .map(|f| f.display)
            .collect();

        let _ = fs::remove_dir_all(&ws);
        assert_eq!(project, ["./COREX.md", "./AGENTS.md"]);
    }

    #[test]
    fn reports_truncation() {
        let ws = temp_workspace("cut");
        let _ = fs::write(ws.join("AGENTS.md"), "x".repeat(MAX_MEMORY_BYTES + 10));

        let files = Memory::new(&ws).files();

        let _ = fs::remove_dir_all(&ws);
        let agents = files.iter().find(|f| f.display == "./AGENTS.md").unwrap();
        assert!(agents.is_truncated());
        assert_eq!(agents.content.len(), MAX_MEMORY_BYTES);
        assert_eq!(agents.original_len, MAX_MEMORY_BYTES + 10);
    }

    #[test]
    fn candidates_skip_duplicates_when_workspace_is_home() {
        let Some(dirs) = BaseDirs::new() else { return };
        let candidates = Memory::new(dirs.home_dir()).candidates();

        let system_md = dirs.home_dir().join(".corex").join("SYSTEM.md");
        let hits: Vec<_> = candidates.iter().filter(|(_, p)| *p == system_md).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, MemoryScope::Global);
    }

    #[test]
    fn notes_go_to_corex_md_or_the_private_project_file() {
        let Some(dirs) = BaseDirs::new() else { return };
        let memory = Memory::new("/tmp/corex-test-workspace");
        let projects = dirs.home_dir().join(".corex").join("projects");

        assert_eq!(memory.add_target(MemoryScope::Global), Some(dirs.home_dir().join(".corex").join("COREX.md")));
        let private = memory.add_target(MemoryScope::Project).unwrap();
        assert_eq!(private, projects.join("-tmp-corex-test-workspace").join("MEMORY.md"));
        assert_eq!(memory.candidates().last().map(|(_, p)| p), Some(&private));
        assert_eq!(memory.display_path(&private), "~/.corex/projects/…/MEMORY.md (private to this project)");
    }

    #[test]
    fn empty_notes_are_rejected() {
        let memory = Memory::new("/tmp/corex-test-workspace");
        assert!(memory.add(MemoryScope::Project, "   ").is_err());
    }
}
