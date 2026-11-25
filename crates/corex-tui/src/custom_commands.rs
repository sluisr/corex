//! User-defined custom slash commands loaded from `.corex/commands/*.md` (project)
//! and `~/.corex/commands/*.md` (global).
//!
//! Each `.md` file defines `/<stem>` (e.g., `review.md` -> `/review`).
//! Optional YAML frontmatter may provide `description: ...`; otherwise the first
//! non-empty non-heading line is used as the description.
//! Inside the prompt body, `$ARGUMENTS` is replaced with whatever follows `/<name>`.
//! If `$ARGUMENTS` is not present and the user passed arguments, they are appended.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommand {
    /// Slash-prefixed name, e.g. `"/review"`.
    pub name: String,
    pub description: String,
    pub body: String,
    pub source: PathBuf,
    pub is_project: bool,
}

impl CustomCommand {
    /// Expands the command template with user-supplied arguments.
    pub fn expand(&self, args: &str) -> String {
        let trimmed = args.trim();
        if self.body.contains("$ARGUMENTS") {
            self.body.replace("$ARGUMENTS", trimmed)
        } else if trimmed.is_empty() {
            self.body.clone()
        } else {
            format!("{}\n\n{}", self.body.trim_end(), trimmed)
        }
    }
}

/// Cached variant of [`discover_custom_commands`] (5 s TTL per workspace) so 60 FPS popup
/// rendering never hits the filesystem on every frame.
pub fn discover_custom_commands_cached(workspace: &Path) -> Vec<CustomCommand> {
    static CACHE: OnceLock<Mutex<Option<(PathBuf, Instant, Vec<CustomCommand>)>>> = OnceLock::new();
    let lock = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = lock.lock() {
        if let Some((ref ws, at, ref cmds)) = *guard {
            if ws == workspace && at.elapsed() < Duration::from_secs(5) {
                return cmds.clone();
            }
        }
    }
    let fresh = discover_custom_commands(workspace);
    if let Ok(mut guard) = lock.lock() {
        *guard = Some((workspace.to_path_buf(), Instant::now(), fresh.clone()));
    }
    fresh
}

/// Discovers custom commands from `~/.corex/commands/*.md` and `<workspace>/.corex/commands/*.md`.
/// Project commands override global commands with the same name.
pub fn discover_custom_commands(workspace: &Path) -> Vec<CustomCommand> {
    let mut by_name: BTreeMap<String, CustomCommand> = BTreeMap::new();

    let global_dir = corex_core::config::corex_home().join("commands");
    load_dir(&global_dir, false, &mut by_name);

    let project_dir = workspace.join(".corex").join("commands");
    if project_dir != global_dir {
        load_dir(&project_dir, true, &mut by_name);
    }

    by_name.into_values().collect()
}

/// Looks up a single custom command by slash name (e.g. `"/review"` or `"review"`).
pub fn find_custom_command(workspace: &Path, name: &str) -> Option<CustomCommand> {
    let needle = if name.starts_with('/') {
        name.to_ascii_lowercase()
    } else {
        format!("/{}", name.to_ascii_lowercase())
    };
    discover_custom_commands(workspace)
        .into_iter()
        .find(|c| c.name == needle)
}

fn load_dir(dir: &Path, is_project: bool, out: &mut BTreeMap<String, CustomCommand>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_md = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if !is_md {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let clean_stem = stem.trim().to_ascii_lowercase();
        if clean_stem.is_empty()
            || !clean_stem
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (desc, body) = parse_command_markdown(&raw);
        if body.trim().is_empty() {
            continue;
        }
        let slash_name = format!("/{clean_stem}");
        let description = desc.unwrap_or_else(|| {
            if is_project {
                "Project custom command".to_string()
            } else {
                "Global custom command".to_string()
            }
        });
        out.insert(
            slash_name.clone(),
            CustomCommand {
                name: slash_name,
                description,
                body,
                source: path,
                is_project,
            },
        );
    }
}

fn parse_command_markdown(raw: &str) -> (Option<String>, String) {
    let mut description: Option<String> = None;
    let body_str = if let Some(rest) = raw.strip_prefix("---\n").or_else(|| raw.strip_prefix("---\r\n")) {
        if let Some(end_idx) = rest.find("\n---") {
            let frontmatter = &rest[..end_idx];
            for line in frontmatter.lines() {
                if let Some(val) = line.trim().strip_prefix("description:") {
                    let v = val.trim().trim_matches('"').trim_matches('\'').trim();
                    if !v.is_empty() {
                        description = Some(v.to_string());
                    }
                }
            }
            let after = &rest[end_idx + 4..];
            after.trim_start_matches(['\r', '\n'])
        } else {
            raw
        }
    } else {
        raw
    };

    if description.is_none() {
        for line in body_str.lines() {
            let trimmed = line.trim().trim_start_matches('#').trim();
            if !trimmed.is_empty() {
                description = Some(corex_core::truncate_ellipsis(trimmed, 70));
                break;
            }
        }
    }

    (description, body_str.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_expands_arguments() {
        let raw = "---\ndescription: Review a git diff carefully\n---\nReview the following changes:\n$ARGUMENTS\nFocus on bugs.";
        let (desc, body) = parse_command_markdown(raw);
        assert_eq!(desc.as_deref(), Some("Review a git diff carefully"));
        let cmd = CustomCommand {
            name: "/review".to_string(),
            description: desc.unwrap(),
            body,
            source: PathBuf::from("review.md"),
            is_project: true,
        };
        assert_eq!(
            cmd.expand("src/main.rs"),
            "Review the following changes:\nsrc/main.rs\nFocus on bugs."
        );
    }

    #[test]
    fn appends_arguments_when_placeholder_absent() {
        let cmd = CustomCommand {
            name: "/commit".to_string(),
            description: "Write a commit".to_string(),
            body: "Write a concise conventional commit message.".to_string(),
            source: PathBuf::from("commit.md"),
            is_project: true,
        };
        assert_eq!(
            cmd.expand("feat(cli): add json"),
            "Write a concise conventional commit message.\n\nfeat(cli): add json"
        );
        assert_eq!(cmd.expand(""), "Write a concise conventional commit message.");
    }

    #[test]
    fn discovers_project_commands_from_dot_corex_commands() {
        let dir = std::env::temp_dir().join(format!("corex-custom-cmd-{}", std::process::id()));
        let cmd_dir = dir.join(".corex").join("commands");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&cmd_dir).unwrap();
        std::fs::write(
            cmd_dir.join("review.md"),
            "---\ndescription: Code review helper\n---\nInspect $ARGUMENTS for edge cases.",
        )
        .unwrap();

        let found = find_custom_command(&dir, "/review").expect("custom command should be found");
        assert_eq!(found.name, "/review");
        assert_eq!(found.description, "Code review helper");
        assert!(found.is_project);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
