use chrono::{DateTime, Duration, Local, Utc};
use corex_core::session::{Session, SessionSummary};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::overlay::{docked_height, docked_panel, highlight_cursor, key_hints, rail, row_text, Row};
use crate::text::{fit_path, fit_text, group_thousands, pad_right, strip_inline_markdown};
use crate::theme::Theme;

#[derive(Debug, Default)]
pub struct SessionDialogState {
    pub is_open: bool,
    pub is_fleet: bool,
    pub selected_idx: usize,
    pub sessions: Vec<SessionSummary>,
    pub search_query: String,
    pub cached_preview_id: Option<String>,
    pub cached_preview_session: Option<Session>,
    /// Id of the session waiting for a delete confirmation. While it is set, the next key either
    /// confirms (`y`) or cancels, so a stray keystroke can never remove a session.
    pub pending_delete: Option<String>,
}

impl SessionDialogState {
    pub fn new() -> Self {
        Self::default()
    }

    fn list(&self, workspace_dir: &str, is_fleet: bool) -> Vec<SessionSummary> {
        if is_fleet {
            Session::list_fleet(None)
        } else {
            Session::list_normal(Some(workspace_dir))
        }
    }

    pub fn open(&mut self, workspace_dir: &str, is_fleet: bool) {
        self.is_fleet = is_fleet;
        self.sessions = self.list(workspace_dir, is_fleet);
        self.selected_idx = 0;
        self.search_query.clear();
        self.cached_preview_id = None;
        self.cached_preview_session = None;
        self.pending_delete = None;
        self.is_open = true;
    }

    /// Re-reads the sessions from disk while keeping the filter and the cursor where they were.
    pub fn reload(&mut self, workspace_dir: &str) {
        self.sessions = self.list(workspace_dir, self.is_fleet);
        let last = self.filtered_sessions().len().saturating_sub(1);
        self.selected_idx = self.selected_idx.min(last);
        self.cached_preview_id = None;
        self.cached_preview_session = None;
        self.pending_delete = None;
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.cached_preview_session = None;
        self.pending_delete = None;
    }

    pub fn filtered_sessions(&self) -> Vec<&SessionSummary> {
        if self.search_query.is_empty() {
            return self.sessions.iter().collect();
        }
        let q = self.search_query.to_lowercase();
        self.sessions
            .iter()
            .filter(|s| {
                s.title.to_lowercase().contains(&q)
                    || s.tag.as_ref().map(|t| t.to_lowercase().contains(&q)).unwrap_or(false)
                    || s.model.to_lowercase().contains(&q)
                    || s.id.to_lowercase().starts_with(&q)
            })
            .collect()
    }

    pub fn selected_session(&self) -> Option<&SessionSummary> {
        let list = self.filtered_sessions();
        if list.is_empty() {
            None
        } else {
            let idx = self.selected_idx.min(list.len() - 1);
            Some(list[idx])
        }
    }

    /// Asks for confirmation to delete the highlighted session. False when there is none.
    pub fn request_delete(&mut self) -> bool {
        self.pending_delete = self.selected_session().map(|s| s.id.clone());
        self.pending_delete.is_some()
    }

    /// Consumes the pending request, returning the session id to delete.
    pub fn take_pending_delete(&mut self) -> Option<String> {
        self.pending_delete.take()
    }

    pub fn update_preview(&mut self) {
        let sel_id = self.selected_session().map(|s| s.id.clone());
        if let Some(id) = sel_id {
            if self.cached_preview_id.as_deref() != Some(&id) {
                self.cached_preview_id = Some(id.clone());
                self.cached_preview_session = Session::load_by_id_or_tag(&id).ok();
            }
        } else {
            self.cached_preview_id = None;
            self.cached_preview_session = None;
        }
    }
}

/// "5m ago" for recent sessions; from a week on a date ("Oct 17"), which tells two old sessions
/// apart where "7d ago" would not.
fn list_time(dt: DateTime<Utc>) -> String {
    if Utc::now().signed_duration_since(dt) < Duration::days(7) {
        Session::format_relative_time(dt)
    } else {
        dt.with_timezone(&Local).format("%b %-d").to_string()
    }
}

const BODY_ROWS: usize = 12;
const CONTENT_MAX_WIDTH: u16 = 120;
const PREVIEW_FROM_WIDTH: u16 = 80;
const COLUMN_GAP: u16 = 2;

pub fn dialog_height(screen_height: u16) -> u16 {
    docked_height(BODY_ROWS, screen_height)
}

pub fn render_session_dialog(
    frame: &mut Frame,
    area: Rect,
    state: &mut SessionDialogState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    state.update_preview();

    let panel = docked_panel(frame, area, theme);
    let inner = Rect::new(panel.x, panel.y, panel.width.min(CONTENT_MAX_WIDTH), panel.height);
    let with_preview = inner.width >= PREVIEW_FROM_WIDTH && !crate::app::is_mobile_portrait(inner.width, 0);

    let vert_chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let title = if state.is_fleet { "Fleet sessions" } else { "Sessions" };
    let mut search_spans = vec![
        Span::styled(format!(" {title}  "), Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD)),
        if state.search_query.is_empty() {
            Span::styled("type to filter by title, #tag or model", Style::default().fg(theme.dark_gray))
        } else {
            Span::styled(state.search_query.clone(), Style::default().fg(theme.foreground))
        },
    ];
    if !state.search_query.is_empty() {
        search_spans.push(Span::styled(crate::cursor::underscore(), Style::default().fg(theme.accent_blue)));
    }
    frame.render_widget(Paragraph::new(Line::from(search_spans)), vert_chunks[0]);

    let filtered = state.filtered_sessions();
    if filtered.is_empty() {
        let dim = Style::default().fg(theme.dark_gray);
        let empty_msg = if state.sessions.is_empty() {
            vec![
                Line::from(Span::styled("   No sessions in this workspace yet.", Style::default().fg(theme.gray))),
                Line::from(vec![
                    Span::styled("   They are saved as you chat, or with ", dim),
                    Span::styled("/save <tag>", Style::default().fg(theme.foreground)),
                    Span::styled(".", dim),
                ]),
            ]
        } else {
            vec![
                Line::from(vec![
                    Span::styled("   Nothing matches ", Style::default().fg(theme.gray)),
                    Span::styled(state.search_query.clone(), Style::default().fg(theme.foreground)),
                ]),
                Line::from(Span::styled("   Backspace clears the filter.", dim)),
            ]
        };
        frame.render_widget(Paragraph::new(empty_msg), vert_chunks[2]);
    } else {
        let tag_col = state
            .sessions
            .iter()
            .filter_map(|s| s.tag.as_ref())
            .map(|t| UnicodeWidthStr::width(t.as_str()).min(8))
            .max();
        let models: std::collections::HashSet<&str> = state.sessions.iter().map(|s| s.model.as_str()).collect();
        // Beside the preview the model is already named there, and the title needs the room.
        let model_col = (models.len() > 1 && !with_preview)
            .then(|| models.iter().map(|m| UnicodeWidthStr::width(*m)).max().unwrap_or(0).min(MODEL_COL_MAX));
        let columns = ListColumns { tag_col, model_col };

        if with_preview {
            let split_h = Layout::horizontal([
                Constraint::Percentage(56),
                Constraint::Length(COLUMN_GAP),
                Constraint::Min(0),
            ])
            .split(vert_chunks[2]);

            render_session_list(frame, split_h[0], &filtered, state.selected_idx, &columns, theme);
            render_session_preview(frame, split_h[2], state.cached_preview_session.as_ref(), theme);
        } else {
            render_session_list(frame, vert_chunks[2], &filtered, state.selected_idx, &columns, theme);
        }
    }
    let total = state.filtered_sessions().len();
    let position = (total > 0).then(|| state.selected_idx.min(total - 1) + 1);

    let mut footer = vec![Span::raw(" ")];
    if let Some(pending) = state.pending_delete.as_ref() {
        let title = state
            .sessions
            .iter()
            .find(|s| &s.id == pending)
            .map(|s| s.title.as_str())
            .unwrap_or("this session");
        let room = (inner.width as usize).saturating_sub(34).max(10);
        footer.push(Span::styled(
            format!("Delete \"{}\"?  ", fit_text(title, room)),
            Style::default().fg(theme.error).add_modifier(Modifier::BOLD),
        ));
        footer.extend(key_hints(&[("y", "yes"), ("n", "no")], theme));
    } else {
        let esc = if state.search_query.is_empty() { ("Esc", "close") } else { ("Esc", "clear filter") };
        let pairs = match (total, with_preview) {
            (0, _) => vec![esc],
            (_, true) => vec![("Enter", "resume"), ("Del", "delete"), ("↑↓", "navigate"), esc],
            (_, false) => vec![("Enter", "open"), ("^X", "delete"), esc],
        };
        footer.extend(key_hints(&pairs, theme));
    }
    if let Some(pos) = position {
        let counter = format!("{}/{} ", pos, total);
        let used: usize = footer.iter().map(|s| s.width()).sum();
        let room = (inner.width as usize).saturating_sub(used + counter.len());
        if room > 0 {
            footer.push(Span::raw(" ".repeat(room)));
            footer.push(Span::styled(counter, Style::default().fg(theme.dark_gray)));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(footer)), vert_chunks[4]);
}

struct ListColumns {
    tag_col: Option<usize>,
    model_col: Option<usize>,
}

const MODEL_COL_MAX: usize = 20;
const MIN_TITLE_WIDTH: usize = 28;
const TIME_WIDTH: usize = 8;

fn render_session_list(
    frame: &mut Frame,
    area: Rect,
    sessions: &[&SessionSummary],
    selected_idx: usize,
    columns: &ListColumns,
    theme: &Theme,
) {
    let inner_w = area.width as usize;
    let max_visible = area.height as usize;

    let selected = selected_idx.min(sessions.len().saturating_sub(1));
    let start_idx = (selected + 1).saturating_sub(max_visible);
    let end_idx = (start_idx + max_visible).min(sessions.len());

    let tag_w = columns.tag_col.map_or(0, |w| w + 2);
    let fixed_w = 3 + tag_w + 2 + TIME_WIDTH + 1;
    let model_col = columns.model_col.filter(|w| inner_w >= fixed_w + w + 2 + MIN_TITLE_WIDTH);
    let title_w = inner_w.saturating_sub(fixed_w + model_col.map_or(0, |w| w + 2)).max(10);
    let dim = Style::default().fg(theme.dark_gray);

    let mut lines = Vec::new();
    for (i, &s) in sessions.iter().enumerate().take(end_idx).skip(start_idx) {
        let row = Row::of(i == selected, true);
        let mut spans = vec![rail(row, theme)];

        if let Some(tag_col) = columns.tag_col {
            let chip = match s.tag.as_ref() {
                Some(t) => format!("#{} ", pad_right(t, tag_col)),
                None => " ".repeat(tag_col + 2),
            };
            spans.push(Span::styled(chip, Style::default().fg(theme.muted)));
        }

        // A session without messages has nothing to resume: it stays listed but recedes.
        let idle = if s.message_count == 0 { theme.dark_gray } else { theme.foreground };
        spans.push(Span::styled(pad_right(&s.title, title_w), row_text(row, idle, theme)));
        if let Some(model_col) = model_col {
            spans.push(Span::styled(format!("  {}", pad_right(&s.model, model_col)), dim));
        }
        spans.push(Span::styled(format!("  {:>w$} ", list_time(s.updated_at), w = TIME_WIDTH), dim));
        lines.push(highlight_cursor(row, Line::from(spans), inner_w, theme));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Keeps the first `max` lines, marking the cut with `…` so a clipped message never looks whole.
fn limit_lines(mut lines: Vec<Line<'static>>, max: usize) -> Vec<Line<'static>> {
    if lines.len() > max {
        lines.truncate(max);
        if let Some(last) = lines.last_mut() {
            last.spans.push(Span::raw("…"));
        }
    }
    lines
}

/// One-line plain text of a message, markdown markers removed, capped so a huge reply stays cheap.
fn message_snippet(msg: &corex_core::types::Message) -> String {
    let text = msg.display_text().unwrap_or_default();
    let joined = text
        .lines()
        .map(strip_inline_markdown)
        .filter(|l| !l.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
    corex_core::types::safe_truncate_str(&joined, 600).to_string()
}

fn with_home_as_tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

fn render_session_preview(
    frame: &mut Frame,
    area: Rect,
    session: Option<&Session>,
    theme: &Theme,
) {
    let w = area.width as usize;
    let dim = Style::default().fg(theme.dark_gray);

    let Some(s) = session else {
        frame.render_widget(Paragraph::new(Span::styled("no preview", dim)), area);
        return;
    };

    let counts = format!(
        " · {} messages · {} tokens",
        s.messages.len(),
        group_thousands(s.total_usage.total_tokens as u64)
    );
    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::styled(fit_text(&s.model, w.saturating_sub(counts.chars().count()).max(8)), Style::default().fg(theme.foreground)),
        Span::styled(counts, dim),
    ])];

    let mut origin = vec![
        s.updated_at.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string(),
        corex_core::types::safe_truncate_str(&s.id, 8).to_string(),
    ];
    if let Some(tag) = s.tag.as_ref() {
        origin.push(format!("#{tag}"));
    }
    let mut origin = origin.join(" · ");
    if let Some(dir) = s.workspace_dir.as_deref().filter(|d| !d.is_empty()) {
        let room = w.saturating_sub(origin.chars().count() + 3);
        if room >= 8 {
            origin = format!("{origin} · {}", fit_path(&with_home_as_tilde(dir), room));
        }
    }
    lines.push(Line::from(Span::styled(fit_text(&origin, w), dim)));
    lines.push(Line::default());

    let excerpt = |who: &'static str, text: String, text_color: Color, rows: usize| {
        let spans = vec![Span::styled(format!("{who:<7}"), dim), Span::styled(text, Style::default().fg(text_color))];
        limit_lines(crate::markdown::wrap_spans_with_indent(spans, w.max(10), "", "       "), rows)
    };

    let turns = || s.messages.iter().filter(|m| matches!(m.role.as_str(), "user" | "assistant"));
    let last_user = turns().filter(|m| m.role == "user").last();
    let last_assistant = turns().filter(|m| m.role == "assistant").last();
    let first_user = turns().find(|m| m.role == "user");

    if let Some(u) = last_user {
        lines.extend(excerpt("you", message_snippet(u), theme.foreground, 3));
    }
    if let Some(a) = last_assistant {
        lines.extend(excerpt("corex", message_snippet(a), theme.gray, 4));
    }

    // Only when it differs from the latest one: otherwise it is the same text twice.
    let rows_left = (area.height as usize).saturating_sub(lines.len());
    if let Some(first) = first_user.filter(|_| rows_left >= 3) {
        if !last_user.is_some_and(|last| std::ptr::eq(last, first)) {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("started with", dim)));
            lines.extend(excerpt("you", message_snippet(first), theme.gray, (rows_left - 2).min(2)));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, title: &str, tag: Option<&str>, model: &str, count: usize) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            title: title.to_string(),
            tag: tag.map(str::to_string),
            model: model.to_string(),
            message_count: count,
            updated_at: Utc::now(),
            workspace_dir: None,
        }
    }

    fn state_with(sessions: Vec<SessionSummary>) -> SessionDialogState {
        SessionDialogState { is_open: true, sessions, ..Default::default() }
    }

    fn screen(state: &mut SessionDialogState, width: u16) -> Vec<String> {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let height = dialog_height(30);
        let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
        terminal
            .draw(|f| render_session_dialog(f, Rect::new(0, 30 - height, width, height), state, &Theme::default()))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    #[test]
    fn delete_needs_a_confirmation_and_targets_the_highlighted_session() {
        let mut s = state_with(vec![summary("aaaa1111", "one", None, "deepseek-flash", 3), summary("bbbb2222", "two", None, "deepseek-flash", 3)]);
        s.selected_idx = 1;
        assert!(s.pending_delete.is_none());
        assert!(s.request_delete());
        assert_eq!(s.pending_delete.as_deref(), Some("bbbb2222"));
        assert_eq!(s.take_pending_delete().as_deref(), Some("bbbb2222"));
        assert!(s.pending_delete.is_none());

        let mut empty = state_with(vec![]);
        assert!(!empty.request_delete());
    }

    #[test]
    fn confirmation_prompt_replaces_the_footer_keys() {
        let mut s = state_with(vec![summary("aaaa1111", "clean up logs", None, "deepseek-flash", 3)]);
        s.request_delete();
        let rows = screen(&mut s, 120).join("\n");
        assert!(rows.contains("Delete \"clean up logs\"?"), "{rows}");
        assert!(rows.contains("y yes") && rows.contains("n no"), "{rows}");
        assert!(!rows.contains("Enter resume"));
    }

    #[test]
    fn list_draws_no_empty_tag_gap_and_names_the_model_only_when_they_differ() {
        let mut s = state_with(vec![
            summary("aaaa1111", "first session", None, "deepseek-flash", 3),
            summary("bbbb2222", "second session", None, "deepseek-flash", 5),
        ]);
        let rows = screen(&mut s, 120);
        let row = rows.iter().find(|r| r.contains("first session")).expect("row");
        assert!(!row.contains("deepseek-flash"), "one model only: the column adds nothing: {row}");
        let cursor = row.find('▎').unwrap();
        let title = row.find("first").unwrap();
        assert!(title - cursor <= 8, "title sits next to the cursor, no reserved tag column: {row}");

        s.sessions.push(summary("cccc3333", "tagged session", Some("work"), "gpt-4o", 2));
        let wide = screen(&mut s, 120).join("\n");
        assert!(wide.contains("#work") && !wide.contains("gpt-4o"), "the preview names the model: {wide}");
        let narrow = screen(&mut s, 76).join("\n");
        assert!(narrow.contains("deepseek-flash") && narrow.contains("gpt-4o"), "{narrow}");
        assert!(!narrow.contains('['), "the model is named, not guessed into a badge: {narrow}");
    }

    #[test]
    fn preview_shows_readable_text_and_formatted_numbers() {
        use corex_core::types::Message;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut session = Session::new();
        session.add_message(Message::user("research which cpu suits my workload"));
        session.add_message(Message::assistant("Try **this**: run `lscpu`", None));
        session.add_message(Message::user("do you know why my username is what it is? this is a long question that does not fit on one line of the panel, nor on two, nor on three, so the panel has to cut it and say there is more text behind the cut"));
        session.add_message(Message::assistant("Honestly: **I don't know**, but I can look at `/etc/passwd`, señor.", None));
        session.total_usage.total_tokens = 195_373;
        session.workspace_dir = Some("/home/sluisr/Juegos".to_string());

        let mut terminal = Terminal::new(TestBackend::new(48, 24)).unwrap();
        terminal
            .draw(|f| render_session_preview(f, f.area(), Some(&session), &Theme::default()))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect();
        let text = rows.join("\n");

        assert!(text.contains("195,373 tokens"), "{text}");
        assert!(text.contains("I don't know"), "markdown markers are gone: {text}");
        assert!(!text.contains("**") && !text.contains('`'), "{text}");
        assert!(text.contains("started with") && text.contains("research which cpu"), "{text}");
        assert!(text.contains("4 messages") && text.contains("Juegos"), "{text}");
        assert!(!text.contains("Preview") && !text.contains("AI:"), "{text}");
        assert!(text.contains('…'), "a clipped message is marked: {text}");
    }

    #[test]
    fn counter_is_drawn_on_a_frameless_panel() {
        let mut s = state_with((0..30).map(|i| summary(&format!("id{i:06}"), &format!("session {i}"), None, "deepseek-flash", 2)).collect());
        s.selected_idx = 11;
        let rows = screen(&mut s, 120);
        let all = rows.join("\n");
        assert!(all.contains("12/30"), "position counter");
        assert!(all.contains("Sessions") && all.contains("Enter resume · Del delete"), "{all}");
        assert!(!all.contains(['┌', '┐', '└', '┘', '┬', '┴', '├', '┤', '│']), "no box, only rules:\n{all}");
    }

    #[test]
    fn title_columns_stay_aligned_with_accents_and_wide_text() {
        let mut s = state_with(vec![
            summary("aaaa1111", "café menu for the señor and the niño, with extra detail", None, "deepseek-flash", 3),
            summary("bbbb2222", "hello", None, "deepseek-flash", 3),
        ]);
        let rows = screen(&mut s, 120);
        let a = rows.iter().find(|r| r.contains("café menu")).unwrap();
        let b = rows.iter().find(|r| r.contains("hello")).unwrap();
        // The time column lines up whatever the title holds.
        let time_col = |r: &str| r[..r.find("just now").unwrap()].chars().count();
        assert_eq!(time_col(a), time_col(b), "{a:?}\n{b:?}");
    }
}
