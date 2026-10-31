//! `/rewind`: pick a past prompt, see what the agent changed since, then rewind the
//! conversation, revert the file edits, or both.

use corex_core::file_history::{line_stats, FileEdit};
use corex_core::Session;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::text::fit_text;
use crate::theme::Theme;

/// One prompt the user typed, with what the agent changed from it on.
#[derive(Debug, Clone)]
pub struct RewindTurn {
    pub turn: usize,
    pub text: String,
    pub files: usize,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewindAction {
    RewindAndRevert,
    RewindOnly,
    RevertOnly,
    Cancel,
}

#[derive(Debug, Default)]
pub struct RewindDialog {
    pub is_open: bool,
    pub turns: Vec<RewindTurn>,
    /// Row in the list; `turns.len()` is "Stay at current position".
    pub selected: usize,
    /// Set once a turn is picked: the confirmation step for `turns[i]`.
    pub confirming: Option<usize>,
    pub option: usize,
}

/// Edits from `turn` on, the ones rewinding to it would revert.
pub fn edits_since(edits: &[FileEdit], turn: usize) -> Vec<FileEdit> {
    edits.iter().filter(|e| e.turn >= turn).cloned().collect()
}

fn summarize(edits: &[FileEdit]) -> (usize, usize, usize) {
    let mut files: Vec<&std::path::Path> = edits.iter().map(|e| e.path.as_path()).collect();
    files.sort();
    files.dedup();
    let (added, removed) = edits.iter().fold((0, 0), |(a, r), e| {
        let (da, dr) = line_stats(e.before.as_deref(), e.after.as_deref());
        (a + da, r + dr)
    });
    (files.len(), added, removed)
}

impl RewindDialog {
    pub fn open(&mut self, session: &Session) {
        let messages = session.display_messages();
        self.turns = session
            .user_turns()
            .iter()
            .enumerate()
            .map(|(i, &idx)| {
                let turn = i + 1;
                let text = messages[idx].display_text().map(|t| t.trim().to_string()).unwrap_or_default();
                let (files, added, removed) = summarize(&edits_since(&session.file_edits, turn));
                RewindTurn { turn, text, files, added, removed }
            })
            .collect();
        self.selected = self.turns.len();
        self.confirming = None;
        self.option = 0;
        self.is_open = true;
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    /// The choices for the turn being confirmed; reverting is offered only when there is
    /// something to revert.
    pub fn options(&self) -> Vec<(RewindAction, &'static str)> {
        let has_edits = self.confirming.and_then(|i| self.turns.get(i)).is_some_and(|t| t.files > 0);
        let mut options = Vec::new();
        if has_edits {
            options.push((RewindAction::RewindAndRevert, "Rewind conversation and revert code changes"));
        }
        options.push((RewindAction::RewindOnly, "Rewind conversation"));
        if has_edits {
            options.push((RewindAction::RevertOnly, "Revert code changes"));
        }
        options.push((RewindAction::Cancel, "Do nothing"));
        options
    }

    pub fn up(&mut self) {
        if self.confirming.is_some() {
            self.option = self.option.saturating_sub(1);
        } else {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    pub fn down(&mut self) {
        if self.confirming.is_some() {
            self.option = (self.option + 1).min(self.options().len() - 1);
        } else {
            self.selected = (self.selected + 1).min(self.turns.len());
        }
    }

    /// Enter: picks the turn, or returns the chosen action for it. `None` keeps the dialog open.
    pub fn enter(&mut self) -> Option<(RewindAction, usize)> {
        match self.confirming {
            Some(i) => {
                let action = self.options()[self.option].0;
                Some((action, self.turns[i].turn))
            }
            None if self.selected >= self.turns.len() => Some((RewindAction::Cancel, 0)),
            None => {
                self.confirming = Some(self.selected);
                self.option = 0;
                None
            }
        }
    }

    /// Esc: back from the confirmation to the list, or closes the dialog. Returns true to close.
    pub fn back(&mut self) -> bool {
        self.confirming.take().is_none()
    }

    pub fn height(&self, max: u16) -> u16 {
        let rows = match self.confirming {
            Some(_) => 3 + self.options().len(),
            None => (self.turns.len() * 2 + 2).min(14),
        };
        (rows as u16 + 3).min(max.saturating_sub(4)).max(6)
    }
}

fn stats_line(t: &RewindTurn) -> String {
    if t.files == 0 {
        "No code changes".to_string()
    } else {
        format!("{} file{} changed +{} -{}", t.files, if t.files == 1 { "" } else { "s" }, t.added, t.removed)
    }
}

pub fn render_rewind_dialog(frame: &mut Frame, area: Rect, dialog: &RewindDialog, theme: &Theme) {
    frame.render_widget(Clear, area);
    let width = area.width.saturating_sub(6) as usize;
    let strong = Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme.dark_gray);
    let mut lines: Vec<Line> = Vec::new();

    let (title, hint) = match dialog.confirming.and_then(|i| dialog.turns.get(i)) {
        Some(turn) => {
            lines.push(Line::from(vec![Span::styled(" Rewind to: ", dim), Span::styled(fit_text(&turn.text, width.saturating_sub(12)), strong)]));
            lines.push(Line::from(Span::styled(format!(" {} since this message", stats_line(turn)), Style::default().fg(theme.gray))));
            lines.push(Line::from(Span::styled(
                fit_text(" Only edits made with Corex's file tools are reverted; changes from shell commands stay.", width),
                dim,
            )));
            lines.push(Line::default());
            for (i, (_, label)) in dialog.options().iter().enumerate() {
                let selected = i == dialog.option;
                let style = if selected { strong } else { Style::default().fg(theme.foreground) };
                lines.push(Line::from(vec![
                    Span::styled(if selected { "▎ " } else { "  " }, Style::default().fg(theme.accent_blue)),
                    Span::styled(format!("{}. {}", i + 1, label), style),
                ]));
            }
            ("Confirm rewind", vec![("Enter", "confirm"), ("Esc", "back")])
        }
        None => {
            let rows: Vec<(String, String)> = dialog
                .turns
                .iter()
                .map(|t| (t.text.lines().next().unwrap_or("").to_string(), stats_line(t)))
                .chain(std::iter::once(("Stay at current position".to_string(), "Cancel rewind and stay here".to_string())))
                .collect();
            let visible = (area.height.saturating_sub(3) as usize / 2).max(1);
            let start = (dialog.selected + 1).saturating_sub(visible);
            for (i, (text, detail)) in rows.iter().enumerate().skip(start).take(visible) {
                let selected = i == dialog.selected;
                let style = if selected { strong } else { Style::default().fg(theme.foreground) };
                lines.push(Line::from(vec![
                    Span::styled(if selected { "▎ " } else { "  " }, Style::default().fg(theme.accent_blue)),
                    Span::styled(fit_text(text, width.saturating_sub(3)), style),
                ]));
                lines.push(Line::from(Span::styled(format!("   {}", fit_text(detail, width.saturating_sub(3))), dim)));
            }
            ("Rewind", vec![("↑↓", "choose a message"), ("Enter", "select"), ("Esc", "close")])
        }
    };

    // No frame: title on the first row, shortcuts on the last, transparent like the other dialogs.
    let row = |y: u16| Rect::new(area.x, y, area.width, 1);
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {title}"), Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD))),
        row(area.y),
    );
    let body = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(2));
    frame.render_widget(Paragraph::new(lines), body);
    let mut keys = vec![Span::raw(" ")];
    keys.extend(crate::overlay::key_hints(&hint, theme));
    frame.render_widget(Paragraph::new(Line::from(keys)), row(area.bottom().saturating_sub(1)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use corex_core::Message;

    fn session() -> Session {
        let mut s = Session::new();
        for t in ["first", "second", "third"] {
            s.add_message(Message::user(t));
        }
        s.file_edits = vec![
            FileEdit { turn: 2, path: "a.rs".into(), before: Some("x\n".into()), after: Some("y\nz\n".into()) },
            FileEdit { turn: 3, path: "b.rs".into(), before: None, after: Some("new\n".into()) },
        ];
        s
    }

    #[test]
    fn lists_turns_with_what_changed_since_each() {
        let mut d = RewindDialog::default();
        d.open(&session());
        let stats: Vec<_> = d.turns.iter().map(|t| (t.text.as_str(), t.files, t.added, t.removed)).collect();
        assert_eq!(stats, vec![("first", 2, 3, 1), ("second", 2, 3, 1), ("third", 1, 1, 0)]);
        assert_eq!(d.selected, 3, "starts on 'Stay at current position'");
        assert_eq!(d.enter(), Some((RewindAction::Cancel, 0)));
    }

    #[test]
    fn confirming_offers_revert_only_when_there_are_edits() {
        let mut d = RewindDialog::default();
        let mut s = session();
        s.file_edits.clear();
        d.open(&s);
        d.up();
        assert_eq!(d.enter(), None);
        let labels: Vec<_> = d.options().iter().map(|o| o.0).collect();
        assert_eq!(labels, vec![RewindAction::RewindOnly, RewindAction::Cancel]);

        d.open(&session());
        d.up();
        d.up();
        d.enter();
        d.down();
        d.down();
        assert_eq!(d.enter(), Some((RewindAction::RevertOnly, 2)));
        assert!(!d.back(), "Esc goes back to the list first");
        assert!(d.back());
    }
}
