//! `@path` completion in the prompt: a gitignore-aware index of the workspace and of the
//! directories added with `/directory add`, and the popup that lists its matches while a path is
//! typed.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use unicode_width::UnicodeWidthStr;

use crate::text::{fit_path, pad_right, wrap_path};
use crate::theme::Theme;

/// Enough for real repositories while keeping a walk of `$HOME` short. Per root.
const MAX_ENTRIES: usize = 20_000;
const MAX_DEPTH: usize = 12;
/// Generated directories that outweigh everything else and are never worth completing into, even
/// where no `.gitignore` names them.
const SKIPPED_DIRS: [&str; 5] = ["node_modules", "target", "dist", "venv", "__pycache__"];
const STALE_AFTER: Duration = Duration::from_secs(30);
pub const MAX_MATCHES: usize = 50;

/// Files and directories (with a trailing `/`), shallowest first: relative to the workspace, the
/// first root, and `~/…` or absolute for the added directories, so the folder's name shows and the
/// inserted path still reads the same file.
pub struct FileIndex {
    roots: Vec<PathBuf>,
    built: Instant,
    paths: Vec<String>,
    /// A root had more than [`MAX_ENTRIES`] entries: the levels that did not fit are missing, and
    /// [`FileIndex::matches`] lists the folder being typed from disk to reach them.
    truncated: bool,
    /// `paths` lowercased once, so matching on every keystroke allocates nothing per entry.
    lower: Vec<String>,
    /// The last query and its matches: the prompt, the popup height and the render all ask for
    /// the same query several times per frame.
    last: RefCell<Option<(String, Vec<String>)>>,
}

impl FileIndex {
    pub fn build(roots: &[PathBuf]) -> Self {
        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        let mut paths: Vec<String> = Vec::new();
        let mut truncated = false;
        for (i, root) in roots.iter().enumerate() {
            let prefix = if i == 0 { String::new() } else { root_prefix(root, home.as_deref()) };
            if i > 0 {
                paths.push(prefix.clone());
            }
            let (found, cut) = walk_by_level(root, &prefix, MAX_ENTRIES);
            truncated |= cut;
            paths.extend(found);
        }
        paths.sort_by(|a, b| a.matches('/').count().cmp(&b.matches('/').count()).then_with(|| a.cmp(b)));
        let lower = paths.iter().map(|p| p.to_lowercase()).collect();
        Self { roots: roots.to_vec(), built: Instant::now(), paths, truncated, lower, last: RefCell::new(None) }
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn is_fresh_for(&self, roots: &[PathBuf]) -> bool {
        self.is_for(roots) && self.built.elapsed() < STALE_AFTER
    }

    pub fn is_for(&self, roots: &[PathBuf]) -> bool {
        self.roots == roots
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Paths matching `query`: prefix of the path, then prefix of the file name, then anywhere.
    pub fn matches(&self, query: &str) -> Vec<String> {
        if let Some((q, hits)) = self.last.borrow().as_ref() {
            if q == query {
                return hits.clone();
            }
        }
        let needle = query.to_lowercase();
        let rank = |lower: &str| -> Option<u8> {
            let name = lower.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            if lower.starts_with(&needle) {
                Some(0)
            } else if name.starts_with(&needle) {
                Some(1)
            } else if lower.contains(&needle) {
                Some(2)
            } else {
                None
            }
        };
        let mut ranked: Vec<(u8, usize)> = self
            .lower
            .iter()
            .enumerate()
            .filter_map(|(i, p)| rank(p).map(|r| (r, i)))
            .collect();
        if ranked.len() > MAX_MATCHES {
            ranked.select_nth_unstable(MAX_MATCHES);
            ranked.truncate(MAX_MATCHES);
        }
        ranked.sort_unstable();
        let mut hits: Vec<String> = ranked.into_iter().map(|(_, i)| self.paths[i].clone()).collect();
        if self.truncated {
            let mut listed = self.list_typed_dir(query);
            hits.retain(|h| !listed.contains(h));
            listed.append(&mut hits);
            listed.truncate(MAX_MATCHES);
            hits = listed;
        }
        *self.last.borrow_mut() = Some((query.to_string(), hits.clone()));
        hits
    }

    /// The entries of the folder named in `query` (`Escritorio/Pro`, `~/Docs/`) read from disk,
    /// for the levels a truncated index does not hold: typing down a path always finds its next
    /// step. Only inside a root, and never what `/directory add` refuses.
    fn list_typed_dir(&self, query: &str) -> Vec<String> {
        use corex_tools::fs_tools::{is_sensitive_system_path, is_within_workspace, resolve_path};

        let Some(base) = self.roots.first() else { return Vec::new() };
        let Some(split) = query.rfind('/') else { return Vec::new() };
        let (dir, prefix) = (&query[..=split], query[split + 1..].to_lowercase());
        let folder = resolve_path(base, dir.trim_end_matches('/'));
        if !folder.is_dir() || is_sensitive_system_path(&folder) || !self.roots.iter().any(|r| is_within_workspace(&folder, r)) {
            return Vec::new();
        }
        let mut names: Vec<(String, bool)> = std::fs::read_dir(&folder)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| {
                        let name = entry.file_name().to_str()?.to_string();
                        let shown = !name.starts_with('.') || prefix.starts_with('.');
                        (shown && name.to_lowercase().starts_with(&prefix)).then(|| (name, entry.path().is_dir()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort_by_key(|(name, _)| name.to_lowercase());
        names.truncate(MAX_MATCHES);
        names.into_iter().map(|(name, is_dir)| format!("{dir}{name}{}", if is_dir { "/" } else { "" })).collect()
    }
}

/// The entries of `root` down to the deepest level that fits in [`MAX_ENTRIES`], and whether a
/// deeper level was left out. Bucketing by depth in a single walk keeps every shallow folder in
/// the index without walking the directory tree up to `MAX_DEPTH` times.
fn walk_by_level(root: &Path, prefix: &str, limit: usize) -> (Vec<String>, bool) {
    let max_depth = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(MAX_DEPTH));
    let max_depth_filter = max_depth.clone();
    let mut buckets: Vec<Vec<String>> = (0..=MAX_DEPTH).map(|_| Vec::new()).collect();
    let mut overflowed_from = MAX_DEPTH + 1;

    let walker = ignore::WalkBuilder::new(root)
        .max_depth(Some(MAX_DEPTH))
        .filter_entry(move |entry| {
            let depth = entry.depth();
            if depth == 0 {
                return true;
            }
            if depth > max_depth_filter.load(std::sync::atomic::Ordering::Relaxed) {
                return false;
            }
            !(entry.file_type().is_some_and(|t| t.is_dir())
                && entry.file_name().to_str().is_some_and(|name| SKIPPED_DIRS.contains(&name)))
        })
        .build();

    for entry in walker.flatten() {
        let depth = entry.depth();
        if depth == 0 || depth > max_depth.load(std::sync::atomic::Ordering::Relaxed) {
            continue;
        }
        let Some(rel) = entry.path().strip_prefix(root).ok().and_then(|p| p.to_str()) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        let Some(ft) = entry.file_type() else {
            continue;
        };
        let slash = if ft.is_dir() { "/" } else { "" };
        buckets[depth].push(format!("{prefix}{rel}{slash}"));

        let cumulative: usize = buckets[1..=depth].iter().map(Vec::len).sum();
        if cumulative > limit {
            overflowed_from = overflowed_from.min(depth);
            let new_max = depth.saturating_sub(1);
            max_depth.store(new_max, std::sync::atomic::Ordering::Relaxed);
        }
    }

    let mut kept: Vec<String> = Vec::new();
    for depth in 1..=MAX_DEPTH {
        if depth >= overflowed_from || kept.len() + buckets[depth].len() > limit {
            if depth == 1 {
                buckets[1].truncate(limit);
                kept = std::mem::take(&mut buckets[1]);
            }
            return (kept, true);
        }
        if buckets[depth].is_empty() {
            break;
        }
        kept.append(&mut buckets[depth]);
    }
    (kept, false)
}

/// How an added directory starts its paths: `~/Escritorio/` inside the home directory, which
/// `read_file` resolves, and the absolute path elsewhere.
fn root_prefix(root: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| root.strip_prefix(h).ok()) {
        Some(rel) if !rel.as_os_str().is_empty() => format!("~/{}/", rel.display()),
        _ => format!("{}/", root.display()),
    }
}

pub fn escape_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c == '\\' {
            out.push_str("\\\\");
        } else if c.is_control() {
            out.push_str(&format!("\\x{:02x}", c as u32));
        } else if c.is_whitespace() {
            out.push('\\');
            out.push(c);
        } else {
            out.push(c);
        }
    }
    out
}

fn escaped_char(after: &str) -> Option<(char, usize)> {
    let hex = after.strip_prefix('x').and_then(|h| h.get(..2)).filter(|h| h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    if let Some(control) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()).map(char::from).filter(|c| c.is_control()) {
        return Some((control, 3));
    }
    let next = after.chars().next()?;
    (next == '\\' || next.is_whitespace()).then_some((next, next.len_utf8()))
}

/// A `\` before any other character stays as typed, so `C:\temp\new` is not turned into tabs.
pub fn unescape_path(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    let mut rest = token;
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        if let Some((unescaped, used)) = escaped_char(rest).filter(|_| c == '\\') {
            out.push(unescaped);
            rest = &rest[used..];
        } else {
            out.push(c);
        }
    }
    out
}

pub fn printable(path: &str) -> String {
    path.chars().map(|c| if c.is_control() { format!("\\x{:02x}", c as u32) } else { c.to_string() }).collect()
}

pub fn token_query(token: &str) -> String {
    let trailing = token.chars().rev().take_while(|&c| c == '\\').count();
    unescape_path(if trailing % 2 == 1 { &token[..token.len() - 1] } else { token })
}

pub fn token_end(text: &str) -> usize {
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() && !escaped {
            return i;
        }
        escaped = c == '\\' && !escaped;
    }
    text.len()
}

/// The `@path` being typed at `cursor`: the byte offset of its `@` and the text after it.
/// The `@` must start a word, so e-mail addresses never open the menu.
pub fn at_token(input: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = input.get(..cursor)?;
    before.rmatch_indices('@').find_map(|(start, _)| {
        let query = &before[start + 1..];
        (starts_token(before, start) && token_end(query) == query.len()).then_some((start, query))
    })
}

pub fn starts_token(text: &str, at: usize) -> bool {
    let before = &text[..at];
    match before.chars().next_back() {
        None => true,
        Some(c) if c.is_whitespace() => {
            let backslashes = before[..before.len() - c.len_utf8()].chars().rev().take_while(|&b| b == '\\').count();
            backslashes % 2 == 0
        }
        Some(_) => false,
    }
}

/// The folder being typed after `/directory add` (or `/dir add`) at `cursor`: the byte offset where
/// it starts and its text. With several comma-separated folders, the last one.
pub fn dir_arg_token(input: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = input.get(..cursor)?;
    let args = ["/directory add ", "/dir add "].iter().find_map(|p| before.strip_prefix(p))?;
    let last = args.rsplit(',').next().unwrap_or(args).trim_start();
    Some((cursor - last.len(), last))
}

/// `~` and `~name` are read as `~/` and `~/name`, so a missing slash still finds the folder.
fn normalize_folder_query(query: &str) -> String {
    match query.strip_prefix('~') {
        Some(rest) if !rest.starts_with('/') => format!("~/{rest}"),
        _ if query.is_empty() => "~/".to_string(),
        _ => query.to_string(),
    }
}

/// True when `query` already names an existing folder (`~/Escritorio` or `~/Escritorio/`), so
/// Enter can run `/directory add` instead of completing it.
pub fn is_existing_folder(query: &str, workspace: &Path) -> bool {
    let query = normalize_folder_query(query);
    query != "~/" && corex_tools::fs_tools::resolve_path(workspace, query.trim_end_matches('/')).is_dir()
}

/// Folders whose name starts with what is typed after the last `/` of `query`, read from disk:
/// `~/…`, absolute, or relative to the workspace. Hidden folders only when the name starts with
/// `.`, and never ones that hold credentials, which `/directory add` refuses anyway. An empty
/// query lists the home directory.
pub fn folder_matches(query: &str, workspace: &Path) -> Vec<String> {
    static CACHE: std::sync::Mutex<Option<(String, PathBuf, Instant, Vec<String>)>> = std::sync::Mutex::new(None);
    let query = normalize_folder_query(query);
    if let Ok(cache) = CACHE.lock() {
        if let Some((q, ws, at, hits)) = cache.as_ref() {
            if *q == query && ws == workspace && at.elapsed() < Duration::from_secs(2) {
                return hits.clone();
            }
        }
    }
    let (dir, prefix) = match query.rfind('/') {
        Some(i) => (&query[..=i], &query[i + 1..]),
        None => ("", query.as_str()),
    };
    let base = if dir.is_empty() { workspace.to_path_buf() } else { corex_tools::fs_tools::resolve_path(workspace, dir) };
    let needle = prefix.to_lowercase();
    let mut hits: Vec<String> = std::fs::read_dir(&base)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_str()?.to_string();
                    let path = entry.path();
                    let visible = !name.starts_with('.') || prefix.starts_with('.');
                    (visible
                        && name.to_lowercase().starts_with(&needle)
                        && path.is_dir()
                        && !corex_tools::fs_tools::is_sensitive_system_path(&path))
                    .then(|| format!("{dir}{name}/"))
                })
                .collect()
        })
        .unwrap_or_default();
    hits.sort_by_key(|h| h.to_lowercase());
    hits.truncate(MAX_MATCHES);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((query, workspace.to_path_buf(), Instant::now(), hits.clone()));
    }
    hits
}

/// True when `query` is a `~/…` or absolute path that exists but lies outside every root, so the
/// popup can say to add its folder instead of only that nothing matched.
pub fn is_outside_roots(query: &str, roots: &[PathBuf]) -> bool {
    use corex_tools::fs_tools::{is_within_workspace, resolve_path};
    if !(query.starts_with("~/") || query.starts_with('/')) {
        return false;
    }
    let Some(base) = roots.first() else { return false };
    let path = resolve_path(base, query.trim_end_matches('/'));
    path.exists() && !roots.iter().any(|root| is_within_workspace(&path, root))
}

/// What the popup says when nothing matches.
#[derive(Clone, Copy)]
pub enum IndexState {
    Building,
    Empty,
    Ready,
    /// The index stopped at [`MAX_ENTRIES`]: deeper files appear once their folder is typed.
    Truncated,
    OutsideRoots,
    /// Folders for `/directory add`, read from disk.
    Folders,
}

const POPUP_MAX_WIDTH: u16 = 110;
const DETAIL_ROWS: usize = 2;

fn popup_inner_width(area_width: u16) -> usize {
    POPUP_MAX_WIDTH.min(area_width).saturating_sub(4) as usize
}

/// Depends on the whole list, not the selection, so the popup keeps its height while it moves.
pub fn detail_rows(matches: &[String], area_width: u16) -> usize {
    let inner = popup_inner_width(area_width);
    if matches.iter().any(|m| UnicodeWidthStr::width(printable(m).as_str()) > inner) {
        DETAIL_ROWS
    } else {
        0
    }
}

pub fn render_file_popup(
    frame: &mut Frame,
    query: &str,
    matches: &[String],
    index: IndexState,
    selected_idx: usize,
    area: Rect,
    theme: &Theme,
) {
    let folders = matches!(index, IndexState::Folders);
    if area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let popup_area = Rect::new(area.x, area.y, POPUP_MAX_WIDTH.min(area.width), area.height);
    let inner_w = popup_inner_width(area.width);
    let wanted = detail_rows(matches, area.width);
    let detail = if area.height as usize >= 3 + wanted { wanted } else { 0 };

    let lines: Vec<Line> = if matches.is_empty() {
        let note = match index {
            IndexState::Building => "Indexing the workspace…".to_string(),
            IndexState::Empty => "No files found in the workspace".to_string(),
            IndexState::Ready => format!("No files match '{}' in the workspace or added folders", query),
            IndexState::Truncated => {
                format!("No files match '{}': the index stops at {MAX_ENTRIES} entries, type the folder (@folder/) to go deeper", query)
            }
            IndexState::OutsideRoots => "Outside the workspace: /directory add <folder> to search it".to_string(),
            IndexState::Folders => format!("No folders match '{}'", query),
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
                } else if path.ends_with('/') && !folders {
                    Style::default().fg(theme.gray)
                } else {
                    Style::default().fg(theme.foreground)
                };
                Line::from(vec![
                    Span::styled(if i == selected { "› " } else { "  " }, Style::default().fg(theme.accent_blue)),
                    Span::styled(pad_right(&fit_path(&printable(path), inner_w), inner_w + 1), style),
                ])
            })
            .collect()
    };

    let visible = (area.height as usize).saturating_sub(2 + detail);
    let selected_line = if matches.is_empty() { 0 } else { selected_idx % matches.len() };
    let scroll = (selected_line + 1).saturating_sub(visible);
    // Like the command menu: no fill, the title on top, then the matches.
    let title = if folders {
        "  Folders".to_string()
    } else if matches!(index, IndexState::Truncated) {
        format!("  Files · partial index ({MAX_ENTRIES} entries): type a folder to go deeper")
    } else {
        "  Files".to_string()
    };
    frame.render_widget(
        Paragraph::new(Span::styled(title, Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD))),
        Rect::new(popup_area.x, popup_area.y, popup_area.width, 1),
    );
    let body = Rect::new(popup_area.x + 1, popup_area.y + 1, popup_area.width.saturating_sub(2), visible as u16);
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), body);

    if detail > 0 {
        let wrapped = wrap_path(&printable(&matches[selected_line]), inner_w, detail);
        let top = popup_area.y + popup_area.height - 1 - detail as u16;
        for (i, text) in wrapped.into_iter().enumerate() {
            frame.render_widget(
                Paragraph::new(Span::styled(format!("  {text}"), Style::default().fg(theme.foreground))),
                Rect::new(popup_area.x + 1, top + i as u16, popup_area.width.saturating_sub(2), 1),
            );
        }
    }

    // The free last row says where the selection is and whether more lies below, as the
    // command menu does.
    if !matches.is_empty() {
        let more_below = scroll + visible < matches.len();
        let counter = format!("  {}({}/{})", if more_below { "▼ " } else { "" }, selected_line + 1, matches.len());
        frame.render_widget(
            Paragraph::new(Span::styled(counter, Style::default().fg(theme.dark_gray))),
            Rect::new(popup_area.x, popup_area.y + popup_area.height - 1, popup_area.width, 1),
        );
    }
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
    fn escaped_spaces_keep_a_name_in_one_token() {
        assert_eq!(escape_path("Project Files/"), "Project\\ Files/");
        for name in [
            "a b  c.md", "back\\slash", "ends\\ ", "a\\ b", "two\\\\ x", " lead", "trail ", "日本 語", "q\"uo'te s", "me@host notes.md",
            "tab\there", "line\nbreak", "cr\rlf", "\\t is not a tab", "\\n", "\\x09 is not either", "\u{1b}[0m", "nb\u{a0}space", "ideo\u{3000}space", "all \t\n\r\\ in one",
        ] {
            let token = escape_path(name);
            assert_eq!(token_end(&token), token.len(), "{name:?} is one token: {token:?}");
            assert_eq!(unescape_path(&token), name, "{name:?} survives the round trip");
        }
        assert_eq!(unescape_path("plain\\x"), "plain\\x", "a backslash before anything else stays");
        assert_eq!(unescape_path("end\\"), "end\\");
        assert_eq!(escape_path("tab\there\nnew"), "tab\\x09here\\x0anew", "no raw control character reaches the prompt");
        assert_eq!(printable("a\tb\nc é"), "a\\x09b\\x0ac é");
        assert_eq!(unescape_path("a\\x09b\\x0Ac\\x41d\\x9"), "a\tb\\x0Ac\\x41d\\x9", "only a lowercase hex pair that is a control character");
        for typed in ["C:\\temp\\new\\notes\\readme", "dir\\tmp", "~/new\\tab", "back\\slash", "\\xyz", "\\x4", "\\é"] {
            assert_eq!(unescape_path(typed), typed, "a backslash typed before an ordinary character is kept: {typed}");
            assert_eq!(token_query(typed), typed, "and so is the search for it: {typed}");
        }
        assert_eq!(token_query("Project\\ Files/Sprint\\"), "Project Files/Sprint", "an unfinished escape is not searched");
        assert_eq!(token_query("odd\\\\\\"), "odd\\", "three backslashes: one escaped pair, one unfinished");
        assert_eq!(token_query("even\\\\"), "even\\", "an escaped backslash is a real one");
        assert_eq!(token_query(""), "");

        let input = "look at @Project\\ Files/";
        assert_eq!(at_token(input, input.len()), Some((8, "Project\\ Files/")));
        let input = "look at @Project Files";
        assert_eq!(at_token(input, input.len()), None, "an unescaped space ends the token");
        let input = "@a\\ b.md next";
        assert_eq!(token_end(input), "@a\\ b.md".len());
        assert_eq!(token_end("a\\\\ b"), 3, "an escaped backslash does not escape the space after it");
        assert_eq!(token_end("tab\there"), 3);
        assert_eq!(token_end("whole"), 5);
    }

    #[test]
    fn an_at_sign_inside_a_name_does_not_hide_the_token() {
        let token = escape_path("me@host notes.md");
        let input = format!("see @{token}");
        assert_eq!(at_token(&input, input.len()), Some((4, token.as_str())));
        let input = "see @@types/node";
        assert_eq!(at_token(input, input.len()), Some((4, "@types/node")));
        assert_eq!(at_token("write to a@b.com", 16), None, "an address still opens nothing");
    }

    #[test]
    fn any_name_survives_being_written_and_read_back() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let pieces = ["a", "t", "n", "x", "0", "9", " ", " ", "\\", "\\", "@", "é", "日", "/", ".", "-", "'", "\"", "\t", "\n", "\r", "\u{1b}", "\u{7f}", "\u{85}", "\u{a0}", "\u{3000}"];
        for _ in 0..5000 {
            let name: String = (0..1 + next(12)).map(|_| pieces[next(pieces.len())]).collect();
            let token = escape_path(&name);
            let input = format!("look at @{token}");
            assert_eq!(unescape_path(&token), name, "{name:?}");
            assert_eq!(at_token(&input, input.len()), Some((8, token.as_str())), "{name:?} -> {token:?}");
            assert_eq!(token_end(&input[8..]), token.len() + 1, "{name:?}");
        }
    }

    #[test]
    fn a_folder_with_spaces_can_be_entered_and_searched() {
        let root = std::env::temp_dir().join(format!("corex-file-index-spaces-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Project Files/Sprint 1")).unwrap();
        std::fs::write(root.join("Project Files/Sprint 1/final notes.md"), "").unwrap();
        let index = FileIndex::build(std::slice::from_ref(&root));
        let _ = std::fs::remove_dir_all(&root);

        let typed = "@Project\\ Files/Sp";
        let (_, raw) = at_token(typed, typed.len()).unwrap();
        let hits = index.matches(&unescape_path(raw));
        assert_eq!(hits, ["Project Files/Sprint 1/", "Project Files/Sprint 1/final notes.md"]);
        assert_eq!(escape_path(&hits[1]), "Project\\ Files/Sprint\\ 1/final\\ notes.md");
    }

    #[test]
    fn index_ranks_prefix_then_name_then_substring() {
        let root = std::env::temp_dir().join(format!("corex-file-index-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        for f in ["src/main.rs", "src/app/mainline.rs", "README.md", "domain.txt"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        let index = FileIndex::build(std::slice::from_ref(&root));
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(index.matches("src/m"), ["src/main.rs"]);
        assert_eq!(index.matches("main"), ["src/main.rs", "src/app/mainline.rs", "domain.txt"]);
        assert_eq!(index.matches("")[..3], ["README.md", "domain.txt", "src/"]);
        assert!(index.matches("zzz").is_empty());
        assert_eq!(index.matches("MAIN"), index.matches("main"), "a cached query must not leak into another");
    }

    #[test]
    fn a_dense_subtree_cannot_hide_the_folders_above_it() {
        let root = std::env::temp_dir().join(format!("corex-file-index-dense-{}", std::process::id()));
        let dense = Path::new("big").join("inner").join("leaf");
        let top = format!("{}/", dense.components().next().unwrap().as_os_str().to_str().unwrap());
        let wanted = format!("{}/needle.md", dense.display());
        let generated = Path::new(SKIPPED_DIRS[0]).join("pkg");
        std::fs::create_dir_all(root.join(&dense)).unwrap();
        std::fs::create_dir_all(root.join(&generated)).unwrap();
        std::fs::write(root.join(&wanted), "").unwrap();
        std::fs::write(root.join(&generated).join("index.js"), "").unwrap();
        for i in 0..30 {
            std::fs::write(root.join(&dense).join(format!("filler{i}.txt")), "").unwrap();
        }

        let (shallow, cut) = walk_by_level(&root, "", 10);
        let (all, whole) = walk_by_level(&root, "", 1000);
        let mut index = FileIndex::build(std::slice::from_ref(&root));
        index.truncated = true;
        index.lower = shallow.iter().map(|p| p.to_lowercase()).collect();
        index.paths = shallow.clone();
        let listed = index.matches(&wanted[..wanted.len() - 4]);
        let _ = std::fs::remove_dir_all(&root);

        assert!(cut && shallow.contains(&top), "{shallow:?}");
        assert!(!shallow.iter().any(|p| p.contains("filler")), "the level that overflowed is left out whole: {shallow:?}");
        assert!(!whole && all.contains(&wanted));
        assert!(!all.iter().any(|p| p.starts_with(SKIPPED_DIRS[0])), "generated folders are skipped: {all:?}");
        assert_eq!(listed, [wanted], "typing the folder reaches the file");
    }

    #[test]
    fn added_directories_are_searchable_by_absolute_path() {
        let base = std::env::temp_dir().join(format!("corex-file-index-extra-{}", std::process::id()));
        let (ws, extra) = (base.join("ws"), base.join("Escritorio"));
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(extra.join("notas")).unwrap();
        std::fs::write(ws.join("src/main.rs"), "").unwrap();
        std::fs::write(extra.join("notas/tarea.md"), "").unwrap();
        let index = FileIndex::build(&[ws.clone(), extra.clone()]);
        let _ = std::fs::remove_dir_all(&base);

        let extra_root = format!("{}/", extra.display());
        assert_eq!(index.matches("escritorio").first(), Some(&extra_root));
        assert!(index.matches("tarea").contains(&format!("{extra_root}notas/tarea.md")));
        assert_eq!(index.matches("main"), ["src/main.rs"], "workspace paths stay relative");
        assert!(index.is_for(&[ws.clone(), extra.clone()]) && !index.is_for(&[ws.clone()]), "adding a directory rebuilds the index");

        std::fs::create_dir_all(&extra).unwrap();
        let outside = format!("{}", extra.display());
        assert!(is_outside_roots(&outside, std::slice::from_ref(&ws)), "an existing folder that was not added");
        assert!(!is_outside_roots(&outside, &[ws.clone(), extra]), "an added folder is in reach");
        assert!(!is_outside_roots("Escritorio", &[ws]), "a bare name is a search, not a path");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_folder_after_directory_add_is_the_token() {
        let input = "/directory add ~/Es";
        assert_eq!(dir_arg_token(input, input.len()), Some((15, "~/Es")));
        let input = "/dir add ~/a, ~/Do";
        assert_eq!(dir_arg_token(input, input.len()), Some((14, "~/Do")), "the last of several folders");
        assert_eq!(dir_arg_token("/directory add ", 15), Some((15, "")));
        assert_eq!(dir_arg_token("/directory show", 15), None);
        assert_eq!(dir_arg_token("tell me about /directory add", 28), None);
        assert_eq!(normalize_folder_query("~Es"), "~/Es");
        assert_eq!(normalize_folder_query("~"), "~/");
        assert_eq!(normalize_folder_query(""), "~/");
        assert_eq!(normalize_folder_query("/srv/"), "/srv/");
    }

    #[test]
    fn folder_matches_list_folders_only() {
        let base = std::env::temp_dir().join(format!("corex-folders-{}", std::process::id()));
        for d in ["Escritorio", "escuela", "Documents", ".oculta"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        std::fs::write(base.join("Es-un-archivo.txt"), "").unwrap();
        let root = format!("{}/", base.display());
        let ws = Path::new("/");

        let es = folder_matches(&format!("{root}es"), ws);
        let hidden = folder_matches(&format!("{root}."), ws);
        let all = folder_matches(&root, ws);
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(es, [format!("{root}Escritorio/"), format!("{root}escuela/")], "folders only, any case");
        assert_eq!(hidden, [format!("{root}.oculta/")], "hidden folders when the name starts with a dot");
        assert!(!all.iter().any(|f| f.contains(".oculta")) && all.len() == 3, "{all:?}");
    }

    #[test]
    fn folders_in_the_home_directory_show_their_name() {
        let home = std::env::temp_dir().join("corex-fake-home");
        assert_eq!(root_prefix(&home.join("Escritorio"), Some(&home)), "~/Escritorio/");
        let elsewhere = std::env::temp_dir().join("corex-elsewhere");
        assert_eq!(root_prefix(&elsewhere, Some(&home)), format!("{}/", elsewhere.display()));
        assert_eq!(root_prefix(&home, Some(&home)), format!("{}/", home.display()));
    }
}
