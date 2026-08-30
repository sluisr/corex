use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use uti_core::types::ToolCall;

use crate::overlay::render_scrim;
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct UserQuestion {
    pub header: Option<String>,
    pub question: String,
    pub options: Vec<String>,
    pub has_options: bool,
}

/// Interactive "ask the user" dialog state. Opened by the TUI when the model
/// calls the `ask_user` tool, instead of blindly executing it.
pub struct UserDialogState {
    pub is_open: bool,
    pub questions: Vec<UserQuestion>,
    pub selected: Vec<usize>,
    pub text_input: Vec<String>,
    pub current: usize,
    pub calls: Vec<ToolCall>,
    pub call_id: String,
    pub cancelled: bool,
}

impl UserDialogState {
    pub fn new() -> Self {
        Self {
            is_open: false,
            questions: Vec::new(),
            selected: Vec::new(),
            text_input: Vec::new(),
            current: 0,
            calls: Vec::new(),
            call_id: String::new(),
            cancelled: false,
        }
    }

    pub fn open(&mut self, questions: Vec<UserQuestion>, calls: Vec<ToolCall>, call_id: String) {
        let n = questions.len();
        self.is_open = true;
        self.questions = questions;
        self.selected = vec![0; n];
        self.text_input = vec![String::new(); n];
        self.current = 0;
        self.calls = calls;
        self.call_id = call_id;
        self.cancelled = false;
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.questions.clear();
        self.selected.clear();
        self.text_input.clear();
        self.current = 0;
        self.calls.clear();
        self.call_id.clear();
        self.cancelled = false;
    }

    pub fn current_question(&self) -> Option<&UserQuestion> {
        self.questions.get(self.current)
    }

    /// Formats the user's answers as the `ask_user` tool output.
    pub fn format_output(&self) -> String {
        if self.cancelled {
            return "User cancelled the questions. Proceed without the missing information or adjust your approach.".to_string();
        }
        let mut out = String::from("User's responses:\n");
        for (i, q) in self.questions.iter().enumerate() {
            let answer = if q.has_options {
                let idx = self.selected[i].min(q.options.len().saturating_sub(1));
                q.options.get(idx).cloned().unwrap_or_default()
            } else {
                self.text_input[i].trim().to_string()
            };
            out.push_str(&format!("{}. {} -> {}\n", i + 1, q.question, answer));
        }
        out
    }
}

/// Parses the `ask_user` tool arguments into a list of questions.
pub fn parse_questions(raw_args: &str) -> Option<Vec<UserQuestion>> {
    let val: serde_json::Value = serde_json::from_str(raw_args).ok()?;
    let arr = val.get("questions")?.as_array()?;
    let mut out = Vec::new();
    for q in arr {
        let question = q.get("question")?.as_str()?.to_string();
        let header = q.get("header").and_then(|h| h.as_str()).map(str::to_string);
        let options: Vec<String> = q
            .get("options")
            .and_then(|o| o.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        out.push(UserQuestion {
            header,
            question,
            has_options: !options.is_empty(),
            options,
        });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn wrap_question(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cur = String::new();
        let mut cur_w = 0usize;
        for ch in line.chars() {
            let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if cur_w + w > width && !cur.is_empty() {
                out.push(Line::from(Span::styled(
                    cur.clone(),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                )));
                cur.clear();
                cur_w = 0;
            }
            cur.push(ch);
            cur_w += w;
        }
        if !cur.is_empty() {
            out.push(Line::from(Span::styled(
                cur,
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )));
        }
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

pub fn render_user_dialog(frame: &mut Frame, area: Rect, state: &UserDialogState, theme: &Theme) {
    let Some(q) = state.questions.get(state.current) else {
        return;
    };
    let total = state.questions.len();
    let title = q
        .header
        .clone()
        .unwrap_or_else(|| format!("Question {}/{}", state.current + 1, total));

    let max_opts = q.options.len().min(8);
    let height = (7 + max_opts as u16).min(area.height.saturating_sub(6)).max(8);
    // Compact box with generous side margins so the chat text doesn't touch
    // the frame edges.
    let width = area.width.saturating_sub(24).min(76).max(40);
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let dialog = Rect::new(x, y, width, height);

    // Dim the chat behind the modal so it does not visually collide.
    render_scrim(frame, area);
    frame.render_widget(Clear, dialog);

    let mut lines: Vec<Line> = Vec::new();
    lines.extend(wrap_question(&q.question, width as usize - 4));
    lines.push(Line::from(""));

    if q.has_options {
        for (i, opt) in q.options.iter().enumerate().take(max_opts) {
            let is_sel = i == state.selected[state.current];
            let bullet = if is_sel { "●" } else { "○" };
            let style = if is_sel {
                Style::default()
                    .fg(theme.accent_blue)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.gray)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("   {} ", bullet), style),
                Span::styled(opt.clone(), style),
            ]));
        }
        if q.options.len() > max_opts {
            lines.push(Line::from(Span::styled(
                format!("   ... ({} more)", q.options.len() - max_opts),
                Style::default().fg(theme.dark_gray),
            )));
        }
    } else {
        let input = &state.text_input[state.current];
        lines.push(Line::from(vec![
            Span::styled("   ❯ ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            Span::styled(input.clone(), Style::default().fg(theme.foreground)),
            Span::styled("█", Style::default().fg(theme.accent_blue)),
        ]));
    }

    lines.push(Line::from(""));
    let hint = if q.has_options {
        "[↑/↓] navigate   [Enter] select   [Esc] cancel"
    } else {
        "[Enter] answer   [Esc] cancel"
    };
    lines.push(Line::from(Span::styled(
        format!("   {}", hint),
        Style::default().fg(theme.dark_gray),
    )));

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent_blue));

    frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), dialog);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_questions() {
        let raw = r#"{"questions":[{"header":"H","question":"¿Qué?","options":["A","B"]},{"question":"Texto","type":"text"}]}"#;
        let qs = parse_questions(raw).expect("should parse");
        assert_eq!(qs.len(), 2);
        assert!(qs[0].has_options);
        assert_eq!(qs[0].options, vec!["A", "B"]);
        assert_eq!(qs[0].header.as_deref(), Some("H"));
        assert!(!qs[1].has_options);
        assert_eq!(qs[1].question, "Texto");
    }

    #[test]
    fn test_parse_questions_rejects_empty() {
        assert!(parse_questions(r#"{"questions":[]}"#).is_none());
        assert!(parse_questions(r#"{"foo":1}"#).is_none());
        assert!(parse_questions("not json").is_none());
    }

    #[test]
    fn test_format_output_selected_option() {
        let raw = r#"{"questions":[{"header":"H","question":"¿Qué?","options":["A","B"]}]}"#;
        let qs = parse_questions(raw).unwrap();
        let mut state = UserDialogState::new();
        state.open(qs, Vec::new(), "call-1".to_string());
        state.selected[0] = 1;
        let out = state.format_output();
        assert!(out.contains("B"), "output should contain selected option: {}", out);
    }

    #[test]
    fn test_format_output_cancelled() {
        let raw = r#"{"questions":[{"question":"¿Qué?","options":["A"]}]}"#;
        let qs = parse_questions(raw).unwrap();
        let mut state = UserDialogState::new();
        state.open(qs, Vec::new(), "call-1".to_string());
        state.cancelled = true;
        assert!(state.format_output().contains("cancelled"));
    }
}
