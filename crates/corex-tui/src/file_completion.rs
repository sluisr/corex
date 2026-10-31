//! `@path` completion in the prompt: a gitignore-aware index of the workspace and the popup that
//! lists its matches while a path is typed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::text::{fit_text, pad_right};
use crate::theme::Theme;

/// Enough for real repositories while keeping a walk of `$HOME` short.
const MAX_ENTRIES: usize = 20_000;
const MAX_DEPTH: usize = 12;
const STALE_AFTER: Duration = Duration::from_secs(30);
pub const MAX_MATCHES: usize = 50;

/// Workspace files and directories (with a trailing `/`), relative, shallowest first.
pub struct FileIndex {
    root: PathBuf,
    built: Instant,
    paths: Vec<String>,
}

impl FileIndex {
    pub fn build(root: &Path) -> Self {
        let mut paths: Vec<String> = ignore::WalkBuilder::new(root)
            .max_depth(Some(MAX_DEPTH))
            .build()
            .flatten()
            .filter_map(|entry| {
                let rel = entry.path().strip_prefix(root).ok()?.to_str()?.to_string();
                if rel.is_empty() {
                    return None;
                }
                Some(if entry.file_type()?.is_dir() { format!("{}/", rel) } else { rel })
            })
            .take(MAX_ENTRIES)
            .collect();
        paths.sort_by(|a, b| a.matches('/').count().cmp(&b.matches('/').count()).then_with(|| a.cmp(b)));
        Self { root: root.to_path_buf(), built: Instant::now(), paths }
    }

    pub fn is_fresh_for(&self, root: &Path) -> bool {
        self.root == root && self.built.elapsed() < STALE_AFTER
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Paths matching `query`: prefix of the path, then prefix of the file name, then anywhere.
    pub fn matches(&self, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        let rank = |path: &str| -> Option<u8> {
            let lower = path.to_lowercase();
            let name = lower.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            if lower.starts_with(&query) {
                Some(0)
            } else if name.starts_with(&query) {
                Some(1)
            } else if lower.contains(&query) {
                Some(2)
            } else {
                None
            }
        };
        let mut ranked: Vec<(u8, usize)> = self
            .paths
            .iter()
            .enumerate()
            .filter_map(|(i, p)| rank(p).map(|r| (r, i)))
            .collect();
        ranked.sort();
        ranked.into_iter().take(MAX_MATCHES).map(|(_, i)| self.paths[i].clone()).collect()
    }
}

/// The `@path` being typed at `cursor`: the byte offset of its `@` and the text after it.
/// The `@` must start a word, so e-mail addresses never open the menu.
pub fn at_token(input: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = input.get(..cursor)?;
    let start = before.rfind('@')?;
    let query = &before[start + 1..];
    let starts_word = before[..start].chars().next_back().is_none_or(char::is_whitespace);
    (starts_word && !query.contains(char::is_whitespace)).then_some((start, query))
}

pub fn render_file_popup(
    frame: &mut Frame,
    query: &str,
    matches: &[String],
    index_empty: bool,
    selected_idx: usize,
    area: Rect,
    theme: &Theme,
) {
    if area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let popup_area = Rect::new(area.x, area.y, 75.min(area.width), area.height);
    let inner_w = popup_area.width.saturating_sub(4) as usize;

    let lines: Vec<Line> = if matches.is_empty() {
        let note = if index_empty {
            "No files found in the workspace".to_string()
        } else {
            format!("No files match '{}'", query)
        };
        vec![Line::from(Span::styled(format!("  {}", note), Style::default().fg(theme.gray)))]
    } else {
        let selected = selected_idx % matches.len();
        matches
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let style = if i == selected {
                    Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
                } else if path.ends_with('/') {
                    Style::default().fg(theme.gray)
                } else {
                    Style::default().fg(theme.foreground)
                };
                Line::from(vec![
                    Span::styled(if i == selected { "▎ " } else { "  " }, Style::default().fg(theme.accent_blue)),
                    Span::styled(pad_right(&fit_text(path, inner_w), inner_w + 1), style),
                ])
            })
            .collect()
    };

    let visible = area.height.saturating_sub(2) as usize;
    let selected_line = if matches.is_empty() { 0 } else { selected_idx % matches.len() };
    let scroll = (selected_line + 1).saturating_sub(visible);
    // Like the command menu: no fill, the title on top, then the matches.
    frame.render_widget(
        Paragraph::new(Span::styled("  Files", Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD))),
        Rect::new(popup_area.x, popup_area.y, popup_area.width, 1),
    );
    let body = Rect::new(popup_area.x + 1, popup_area.y + 1, popup_area.width.saturating_sub(2), popup_area.height.saturating_sub(2));
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_token_needs_a_word_start() {
        assert_eq!(at_token("read @src/ma", 12), Some((5, "src/ma")));
        assert_eq!(at_token("@", 1), Some((0, "")));
        assert_eq!(at_token("mail me at a@b.com", 18), None);
        assert_eq!(at_token("@src/main.rs please", 19), None);
        assert_eq!(at_token("no token", 8), None);
    }

    #[test]
    fn index_ranks_prefix_then_name_then_substring() {
        let root = std::env::temp_dir().join(format!("corex-file-index-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        for f in ["src/main.rs", "src/app/mainline.rs", "README.md", "domain.txt"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        let index = FileIndex::build(&root);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(index.matches("src/m"), ["src/main.rs"]);
        assert_eq!(index.matches("main"), ["src/main.rs", "src/app/mainline.rs", "domain.txt"]);
        assert_eq!(index.matches("")[..3], ["README.md", "domain.txt", "src/"]);
        assert!(index.matches("zzz").is_empty());
    }
}
