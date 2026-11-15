use chrono::{DateTime, Utc};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use corex_tools::background::TaskManager;

use crate::overlay::{docked_height, docked_panel_open_bottom, highlight_cursor, key_hints, rail, row_text, Row};
use crate::text::{expand_tabs, pad_right, strip_ansi};
use crate::theme::Theme;

const LIST_ROWS: usize = 8;
const TAIL_ROWS: usize = 6;
const QUICK_MILLIS: i64 = 1_000;

#[derive(Debug, Default)]
pub struct TasksDialogState {
    pub is_open: bool,
    pub selected: usize,
    pub show_all: bool,
    pub input: Option<String>,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub pid: u32,
    pub command: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub duration: String,
}

#[derive(Debug, Default)]
pub struct TaskList {
    pub rows: Vec<TaskRow>,
    pub hidden: usize,
    pub running: usize,
    pub done: usize,
}

pub enum TasksAction {
    None,
    Close,
    Details(u32),
    Kill(u32),
    Send(u32, String),
}

pub fn task_list(mgr: &TaskManager, show_all: bool) -> TaskList {
    let mut entries: Vec<(TaskRow, DateTime<Utc>, bool)> = mgr
        .all_processes()
        .values()
        .map(|p| {
            let running = p.is_active();
            let exit_code = p.get_exit_code();
            let finished = p.get_finished_at();
            let quick = !running
                && exit_code == Some(0)
                && finished.is_some_and(|f| (f - p.started_at).num_milliseconds() < QUICK_MILLIS);
            let row = TaskRow { pid: p.pid, command: p.command.clone(), running, exit_code, duration: p.duration_str() };
            (row, finished.unwrap_or(p.started_at), quick)
        })
        .collect();
    entries.sort_by(|a, b| b.0.running.cmp(&a.0.running).then(b.1.cmp(&a.1)));
    let running = entries.iter().filter(|e| e.0.running).count();
    let done = entries.len() - running;
    let hidden = if show_all { 0 } else { entries.iter().filter(|e| e.2).count() };
    let rows = entries.into_iter().filter(|e| show_all || !e.2).map(|e| e.0).collect();
    TaskList { rows, hidden, running, done }
}

pub fn dialog_height(screen_height: u16) -> u16 {
    docked_height(LIST_ROWS + 1 + TAIL_ROWS, screen_height).saturating_sub(1)
}

impl TasksDialogState {
    pub fn open(&mut self) {
        *self = Self { is_open: true, ..Self::default() };
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.input = None;
    }

    pub fn handle_key(&mut self, code: crossterm::event::KeyCode, list: &TaskList) -> TasksAction {
        use crossterm::event::KeyCode;
        let selected = list.rows.get(self.selected.min(list.rows.len().saturating_sub(1)));
        if let Some(input) = self.input.as_mut() {
            match code {
                KeyCode::Esc => self.input = None,
                KeyCode::Enter => {
                    let text = self.input.take().unwrap_or_default();
                    if let Some(row) = selected {
                        return TasksAction::Send(row.pid, text);
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            }
            return TasksAction::None;
        }
        self.notice = None;
        match code {
            KeyCode::Esc => return TasksAction::Close,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                if self.selected + 1 < list.rows.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.show_all = !self.show_all;
                self.selected = 0;
            }
            KeyCode::Enter => {
                if let Some(row) = selected {
                    return TasksAction::Details(row.pid);
                }
            }
            KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Delete => {
                if let Some(row) = selected.filter(|r| r.running) {
                    return TasksAction::Kill(row.pid);
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                if selected.is_some_and(|r| r.running) {
                    self.input = Some(String::new());
                }
            }
            _ => {}
        }
        TasksAction::None
    }
}

fn fit_left(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

fn status_icon(row: &TaskRow, theme: &Theme) -> Span<'static> {
    match (row.running, row.exit_code) {
        (true, _) => Span::styled("● ", Style::default().fg(theme.accent_cyan)),
        (false, Some(0)) => Span::styled("✓ ", Style::default().fg(theme.success)),
        (false, Some(_)) => Span::styled("✕ ", Style::default().fg(theme.error)),
        (false, None) => Span::styled("■ ", Style::default().fg(theme.dark_gray)),
    }
}

pub fn render_tasks_dialog(frame: &mut Frame, area: Rect, state: &mut TasksDialogState, mgr: &TaskManager, theme: &Theme) {
    if !state.is_open {
        return;
    }
    let list = task_list(mgr, state.show_all);
    state.selected = state.selected.min(list.rows.len().saturating_sub(1));
    let selected = list.rows.get(state.selected);

    let inner = docked_panel_open_bottom(frame, area, theme);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(LIST_ROWS as u16),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let width = inner.width as usize;
    let dim = Style::default().fg(theme.dark_gray);

    let meta = state
        .notice
        .clone()
        .unwrap_or_else(|| format!("{} running · {} done", list.running, list.done));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Tasks", Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD)),
            Span::styled(format!("   {meta}"), dim),
        ])),
        rows[0],
    );

    let mut lines: Vec<Line<'static>> = Vec::new();
    if list.rows.is_empty() && list.hidden == 0 {
        lines.push(Line::from(Span::styled("   No background tasks in this session.", dim)));
    }
    let visible = if list.hidden > 0 { LIST_ROWS - 1 } else { LIST_ROWS };
    let offset = state.selected.saturating_sub(visible.saturating_sub(1));
    for (i, row) in list.rows.iter().enumerate().skip(offset).take(visible) {
        let kind = Row::of(i == state.selected, true);
        let prefix = 3 + 2 + 8 + 9;
        let command = fit_left(&row.command, width.saturating_sub(prefix + 1));
        let line = Line::from(vec![
            rail(kind, theme),
            status_icon(row, theme),
            Span::styled(pad_right(&row.pid.to_string(), 8), dim),
            Span::styled(format!("{:>7}  ", row.duration), dim),
            Span::styled(command, row_text(kind, theme.foreground, theme)),
        ]);
        lines.push(highlight_cursor(kind, line, width, theme));
    }
    if list.hidden > 0 {
        let noun = if list.hidden == 1 { "command" } else { "commands" };
        lines.push(Line::from(Span::styled(format!("   + {} quick {noun} hidden (a: show all)", list.hidden), dim)));
    }
    frame.render_widget(Paragraph::new(lines), rows[1]);

    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(width), Style::default().fg(theme.border_dim))),
        rows[2],
    );

    let tail_rows = rows[3].height as usize;
    let tail = selected.and_then(|r| mgr.tail(r.pid, tail_rows)).unwrap_or_default();
    let tail_lines: Vec<Line<'static>> = if selected.is_some() && tail.is_empty() {
        vec![Line::from(Span::styled("   (no output yet)", dim))]
    } else {
        tail.lines()
            .map(|l| {
                let clean = expand_tabs(&strip_ansi(l)).into_owned();
                Line::from(Span::styled(format!("   {}", crate::text::fit_text(&clean, width.saturating_sub(4))), Style::default().fg(theme.gray)))
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(tail_lines), rows[3]);

    let footer = match (&state.input, selected) {
        (Some(input), Some(row)) => Line::from(vec![
            Span::styled(format!(" send to {}: ", row.pid), Style::default().fg(theme.text_strong)),
            Span::styled(input.clone(), Style::default().fg(theme.foreground)),
            Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_yellow)),
            Span::styled("   enter send · esc cancel", dim),
        ]),
        _ => {
            let mut pairs = vec![("↑↓", "select"), ("enter", "details")];
            if selected.is_some_and(|r| r.running) {
                pairs.push(("k", "kill"));
                pairs.push(("s", "send input"));
            }
            pairs.push(("a", if state.show_all { "hide quick" } else { "show all" }));
            pairs.push(("esc", "close"));
            let mut spans = vec![Span::raw(" ")];
            spans.extend(key_hints(&pairs, theme));
            Line::from(spans)
        }
    };
    frame.render_widget(Paragraph::new(footer), rows[4]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use std::sync::{Arc, Mutex};

    fn add(mgr: &mut TaskManager, pid: u32, command: &str, running: bool, code: Option<i32>, millis: i64, output: &str) {
        let finished = (!running).then(|| Utc::now());
        mgr.register(
            pid,
            command.to_string(),
            Arc::new(Mutex::new(output.to_string())),
            Arc::new(Mutex::new(running)),
            Arc::new(Mutex::new(finished)),
            Arc::new(Mutex::new(code)),
            Arc::new(Mutex::new(true)),
            None,
        );
        if let Some(p) = mgr.all_processes().get(&pid) {
            let started = p.started_at;
            if let (Some(_), Ok(mut f)) = (finished, p.finished_at.lock()) {
                *f = Some(started + chrono::Duration::milliseconds(millis));
            }
        }
    }

    fn manager() -> TaskManager {
        let mut mgr = TaskManager::new();
        add(&mut mgr, 1, "git status", false, Some(0), 10, "clean");
        add(&mut mgr, 2, "cargo build --release", false, Some(101), 5_000, "error[E0425]");
        add(&mut mgr, 3, "/tmp/project/target/debug/deps/app_tests-0123abcd --test-threads=1", true, None, 0, "running 276 tests\ntest a ... ok\ntest b ... ok");
        mgr
    }

    #[test]
    fn running_first_and_quick_commands_folded() {
        let mgr = manager();
        let list = task_list(&mgr, false);
        assert_eq!(list.rows.iter().map(|r| r.pid).collect::<Vec<_>>(), [3, 2]);
        assert_eq!((list.hidden, list.running, list.done), (1, 1, 2));
        assert_eq!(task_list(&mgr, true).rows.len(), 3);
    }

    #[test]
    fn keys_pick_the_selected_task() {
        let mgr = manager();
        let list = task_list(&mgr, false);
        let mut state = TasksDialogState::default();
        state.open();
        assert!(matches!(state.handle_key(KeyCode::Char('k'), &list), TasksAction::Kill(3)));
        assert!(matches!(state.handle_key(KeyCode::Char('s'), &list), TasksAction::None));
        for c in "yes".chars() {
            state.handle_key(KeyCode::Char(c), &list);
        }
        assert!(matches!(state.handle_key(KeyCode::Enter, &list), TasksAction::Send(3, ref t) if t == "yes"));
        state.handle_key(KeyCode::Down, &list);
        assert!(matches!(state.handle_key(KeyCode::Char('k'), &list), TasksAction::None), "a finished task cannot be killed");
        assert!(matches!(state.handle_key(KeyCode::Enter, &list), TasksAction::Details(2)));
        assert!(matches!(state.handle_key(KeyCode::Esc, &list), TasksAction::Close));
    }

    #[test]
    fn panel_shows_the_end_of_the_command_and_a_live_tail() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mgr = manager();
        let mut state = TasksDialogState::default();
        state.open();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        let h = dialog_height(30);
        terminal.draw(|f| render_tasks_dialog(f, Rect::new(0, 30 - h, 80, h), &mut state, &mgr, &Theme::default())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let screen: String = (0..30).map(|y| (0..80).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n").collect();
        assert!(screen.contains("1 running · 2 done"), "{screen}");
        assert!(screen.contains("--test-threads=1"), "the end of a long command stays visible:\n{screen}");
        assert!(screen.contains("+ 1 quick command hidden"), "{screen}");
        assert!(screen.contains("test b ... ok"), "{screen}");
        assert!(screen.contains("k kill") && screen.contains("esc close"), "{screen}");
        assert!(!screen.contains("git status"), "{screen}");
    }
}
