use chrono::{DateTime, Duration, Local, Utc};
use corex_core::session::{Session, SessionSummary};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::overlay::{begin_modal, key_hints, panel_rules};
use crate::text::{fit_text, group_thousands, pad_right, strip_inline_markdown};
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

/// Short label and colour of the model family a session was run with.
fn model_badge(model: &str) -> (&'static str, Color) {
    if model.starts_with("local") {
        ("[Local]", Color::Rgb(255, 215, 130))
    } else if model.contains("claude") || model.contains("anthropic") {
        ("[Claude]", Color::Rgb(255, 167, 38))
    } else if model.contains("gemini") {
        ("[Gemini]", Color::Rgb(79, 195, 247))
    } else if model.contains("gpt") || model.contains("openai") || model.starts_with("o1") || model.starts_with("o3") {
        ("[GPT]", Color::Rgb(105, 240, 174))
    } else if model.contains("llama") {
        ("[Llama]", Color::Rgb(179, 136, 255))
    } else if model.contains("codestral") || model.contains("mistral") {
        ("[Mistral]", Color::Rgb(255, 112, 67))
    } else if model.contains("pro") || model.contains("reasoner") {
        ("[Pro]", Color::Rgb(215, 175, 255))
    } else if model.contains("flash") {
        ("[Flash]", Color::Rgb(135, 215, 235))
    } else {
        ("[Cloud]", Color::Rgb(129, 199, 245))
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

pub fn render_session_dialog(
    frame: &mut Frame,
    area: Rect,
    state: &mut SessionDialogState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    // Update the selected element's preview if it changed
    state.update_preview();

    let is_mobile = crate::app::is_mobile_portrait(area.width, area.height);

    let dialog_width = if is_mobile {
        area.width.saturating_sub(2).max(20)
    } else {
        104.min(area.width.saturating_sub(4)).max(56.min(area.width))
    };
    let dialog_height = if is_mobile {
        area.height.saturating_sub(2).min(28)
    } else {
        (area.height * 4 / 5)
            .clamp(16, 26)
            .min(area.height.saturating_sub(2))
    };

    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Soft dimmed background + perimeter breathing
    begin_modal(frame, area, dialog_area, 2, 1);

    // No fill, so a transparent terminal shows through; thin rules mark the top and bottom.
    let panel = Block::default().padding(Padding::new(1, 1, 1, 1));
    let inner = panel.inner(dialog_area);
    frame.render_widget(panel, dialog_area);
    panel_rules(frame, dialog_area, theme);

    // Title and filter, a gap, the list beside its preview, a gap, the shortcuts.
    let vert_chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let title = if state.is_fleet { "Fleet sessions" } else { "Sessions" };
    let mut search_spans = vec![
        Span::styled(format!("   {title}   "), Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD)),
        Span::styled(
            if state.search_query.is_empty() {
                "type to search by topic, #tag or model...".to_string()
            } else {
                state.search_query.clone()
            },
            if state.search_query.is_empty() {
                Style::default().fg(theme.dark_gray)
            } else {
                Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)
            },
        ),
    ];
    if !state.search_query.is_empty() {
        search_spans.push(Span::styled(format!(" {}", crate::cursor::underscore()), Style::default().fg(theme.accent_cyan)));
    }
    frame.render_widget(Paragraph::new(Line::from(search_spans)), vert_chunks[0]);

    let filtered = state.filtered_sessions();
    if filtered.is_empty() {
        let empty_msg = if state.sessions.is_empty() {
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("No conversation sessions found for this workspace.", Style::default().fg(theme.gray)),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("Sessions are automatically saved as you chat or via ", Style::default().fg(theme.dark_gray)),
                    Span::styled("/save <tag>", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                    Span::styled(".", Style::default().fg(theme.dark_gray)),
                ]),
            ]
        } else {
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("No sessions matching: ", Style::default().fg(theme.gray)),
                    Span::styled(&state.search_query, Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("Press Backspace to clear filter.", Style::default().fg(theme.dark_gray)),
                ]),
            ]
        };
        frame.render_widget(Paragraph::new(empty_msg), vert_chunks[2]);
    } else {
        // Columns that carry no information are not drawn: the tag chip only exists when some
        // session has a tag, and the model badge only when sessions were run with different models.
        let tag_col = state
            .sessions
            .iter()
            .filter_map(|s| s.tag.as_ref())
            .map(|t| UnicodeWidthStr::width(t.as_str()).min(8))
            .max();
        let mixed_models = state
            .sessions
            .iter()
            .map(|s| model_badge(&s.model).0)
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 1;
        let columns = ListColumns { tag_col, badges: mixed_models };

        if is_mobile {
            render_session_list(frame, vert_chunks[2], &filtered, state.selected_idx, &columns, theme);
        } else {
            let split_h = Layout::horizontal([
                Constraint::Percentage(58),
                Constraint::Length(1), // Separador vertical
                Constraint::Percentage(42),
            ])
            .split(vert_chunks[2]);

            render_session_list(frame, split_h[0], &filtered, state.selected_idx, &columns, theme);
            render_session_preview(frame, split_h[2], state.cached_preview_session.as_ref(), theme);
        }
    }
    let total = state.filtered_sessions().len();
    let position = (total > 0).then(|| state.selected_idx.min(total - 1) + 1);

    let mut footer = vec![Span::raw("  ")];
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
    } else if is_mobile {
        footer.extend(key_hints(&[("Enter", "open"), ("^X", "delete"), ("Esc", "close")], theme));
    } else {
        let esc = if state.search_query.is_empty() { ("Esc", "close") } else { ("Esc", "clear filter") };
        footer.extend(key_hints(&[("Enter", "resume"), ("Del", "delete"), ("↑↓", "navigate"), esc], theme));
    }
    // Where the cursor is in the list: only a screenful of sessions is ever visible.
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

/// Which optional columns the session list draws.
struct ListColumns {
    /// Width of the longest `#tag` (capped), or `None` when no session has one.
    tag_col: Option<usize>,
    /// Whether the model badge is shown (only useful when models are mixed).
    badges: bool,
}

const BADGE_WIDTH: usize = 9; // "[Mistral]"
const TIME_WIDTH: usize = 7;

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
    let start_idx = if selected >= max_visible {
        selected + 1 - max_visible
    } else {
        0
    };
    let end_idx = (start_idx + max_visible).min(sessions.len());

    // cursor (3) + tag chip ("#" + tag + space) + badge + space, then the title, then the time.
    let tag_w = columns.tag_col.map_or(0, |w| w + 2);
    let badge_w = if columns.badges { BADGE_WIDTH + 1 } else { 0 };
    let right_w = TIME_WIDTH + 2;
    let title_w = inner_w.saturating_sub(3 + tag_w + badge_w + right_w).max(10);

    let mut lines = Vec::new();
    for (i, &s) in sessions.iter().enumerate().take(end_idx).skip(start_idx) {
        let is_selected = i == selected;

        let cursor = if is_selected { " ▎ " } else { "   " };
        let cursor_style = Style::default().fg(theme.accent_blue);

        let mut row = vec![Span::styled(cursor, cursor_style)];

        if let Some(tag_col) = columns.tag_col {
            let chip = match s.tag.as_ref() {
                Some(t) => format!("#{} ", pad_right(t, tag_col)),
                None => " ".repeat(tag_col + 2),
            };
            row.push(Span::styled(
                chip,
                Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD),
            ));
        }

        if columns.badges {
            let (label, fg) = model_badge(&s.model);
            row.push(Span::styled(
                format!("{} ", pad_right(label, BADGE_WIDTH)),
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ));
        }

        // A session without messages has nothing to resume: it stays listed but recedes.
        let title_style = if s.message_count == 0 {
            Style::default().fg(theme.dark_gray)
        } else if is_selected {
            Style::default().fg(theme.text_on_selected).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        row.push(Span::styled(pad_right(&s.title, title_w), title_style));
        row.push(Span::styled(
            format!("{:>w$} ", list_time(s.updated_at), w = TIME_WIDTH),
            Style::default().fg(theme.dark_gray),
        ));
        row.push(Span::styled(" ", Style::default()));
        lines.push(Line::from(row));
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
    let mut lines: Vec<Line<'static>> = Vec::new();

    let Some(s) = session else {
        lines.push(Line::from(Span::styled(" No preview available", Style::default().fg(theme.dark_gray))));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };

    let label = Style::default().fg(theme.dark_gray);
    let meta = |name: &'static str, value: String, style: Style| {
        Line::from(vec![Span::styled(format!("  {:<11}", name), label), Span::styled(value, style)])
    };
    let value_w = w.saturating_sub(13).max(8);

    lines.push(Line::from(vec![
        Span::styled(" Preview", Style::default().fg(theme.dark_gray).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(" · ID: {}", corex_core::types::safe_truncate_str(&s.id, 8)),
            Style::default().fg(theme.dark_gray),
        ),
    ]));
    lines.push(Line::from(""));

    lines.push(meta("Model:", fit_text(&s.model, value_w), Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)));
    lines.push(Line::from(vec![
        Span::styled(format!("  {:<11}", "Messages:"), label),
        Span::styled(s.messages.len().to_string(), Style::default().fg(theme.accent_cyan)),
        Span::styled(
            format!("  ({} tokens)", group_thousands(s.total_usage.total_tokens as u64)),
            Style::default().fg(theme.dark_gray),
        ),
    ]));
    let fmt_time = |dt: DateTime<Utc>| dt.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string();
    lines.push(meta("Updated:", fmt_time(s.updated_at), Style::default().fg(theme.gray)));
    lines.push(meta("Created:", fmt_time(s.created_at), Style::default().fg(theme.gray)));
    if let Some(dir) = s.workspace_dir.as_deref().filter(|d| !d.is_empty()) {
        lines.push(meta("Workspace:", fit_text(&with_home_as_tilde(dir), value_w), Style::default().fg(theme.gray)));
    }
    if let Some(ref tag) = s.tag {
        lines.push(meta("Tag:", format!("#{}", tag), Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)));
    }
    lines.push(Line::from(""));

    // A role-labelled excerpt: the label on the first row, the text wrapped under it.
    let max_text_w = w.saturating_sub(2).max(10);
    let excerpt = |tag: &'static str, tag_color: Color, text: String, text_color: Color, rows: usize| {
        let spans = vec![
            Span::styled(format!("{:<7}", tag), Style::default().fg(tag_color).add_modifier(Modifier::BOLD)),
            Span::styled(text, Style::default().fg(text_color)),
        ];
        limit_lines(crate::markdown::wrap_spans_with_indent(spans, max_text_w, "  ", "         "), rows)
    };

    let turns = || s.messages.iter().filter(|m| matches!(m.role.as_str(), "user" | "assistant"));
    let last_user = turns().filter(|m| m.role == "user").last();
    let last_assistant = turns().filter(|m| m.role == "assistant").last();
    let first_user = turns().find(|m| m.role == "user");

    if last_user.is_some() || last_assistant.is_some() {
        lines.push(Line::from(Span::styled("  Latest exchange", Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD))));
        if let Some(u) = last_user {
            lines.extend(excerpt("You:", theme.accent_blue, message_snippet(u), theme.foreground, 3));
        }
        if let Some(a) = last_assistant {
            lines.extend(excerpt("AI:", theme.accent_green, message_snippet(a), theme.gray, 4));
        }
    }

    // Only when it differs from the latest one: otherwise it is the same text twice.
    if let Some(first) = first_user {
        if !last_user.is_some_and(|last| std::ptr::eq(last, first)) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("  Started with", Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD))));
            lines.extend(excerpt("You:", theme.accent_blue, message_snippet(first), theme.gray, 2));
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
        let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
        terminal
            .draw(|f| render_session_dialog(f, f.area(), state, &Theme::default()))
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
    fn list_draws_no_empty_tag_gap_and_hides_redundant_badges() {
        let mut s = state_with(vec![
            summary("aaaa1111", "first session", None, "deepseek-flash", 3),
            summary("bbbb2222", "second session", None, "deepseek-flash", 5),
        ]);
        let rows = screen(&mut s, 120);
        let row = rows.iter().find(|r| r.contains("first session")).expect("row");
        assert!(!row.contains("[Flash]"), "one model only: the badge adds nothing: {row}");
        let cursor = row.find('▎').unwrap();
        let title = row.find("first").unwrap();
        assert!(title - cursor <= 8, "title sits next to the cursor, no reserved tag column: {row}");

        // With mixed models and a tag, both columns come back.
        s.sessions.push(summary("cccc3333", "tagged session", Some("work"), "gpt-4o", 2));
        let rows = screen(&mut s, 120);
        let all = rows.join("\n");
        assert!(all.contains("[Flash]") && all.contains("[GPT]") && all.contains("#work"));
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
        assert!(text.contains("Started with") && text.contains("research which cpu"), "{text}");
        assert!(text.contains("Updated:") && text.contains("Created:") && text.contains("Workspace:"), "{text}");
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
