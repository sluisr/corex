use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use corex_core::types::ToolCall;

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

impl Default for UserDialogState {
    fn default() -> Self {
        Self::new()
    }
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
                    Style::default().add_modifier(Modifier::BOLD),
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
                Style::default().add_modifier(Modifier::BOLD),
            )));
        }
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

pub fn calculate_dialog_height(state: &UserDialogState, width: u16, max_height: u16) -> u16 {
    let Some(q) = state.questions.get(state.current) else {
        return 5;
    };
    let usable_w = (width as usize).saturating_sub(6).max(20);
    let q_lines_count = wrap_question(&q.question, usable_w).len().max(1);
    let opt_count = if q.has_options {
        q.options.len().min(8)
    } else {
        1
    };
    // Content lines: question lines + opt_count + 1 hint
    // Borders: top + bottom = 2
    let content = q_lines_count + opt_count + 1;
    (content as u16 + 2).min(max_height.saturating_sub(6)).max(5)
}

pub fn render_user_dialog(frame: &mut Frame, area: Rect, state: &UserDialogState, theme: &Theme) {
    let Some(q) = state.questions.get(state.current) else {
        return;
    };

    let usable_w = (area.width as usize).saturating_sub(6).max(20);
    let mut lines: Vec<Line> = Vec::new();

    // 1. Question text wrapped with clean prompt bullet
    let q_lines = wrap_question(&q.question, usable_w);
    for (i, ql) in q_lines.into_iter().enumerate() {
        if i == 0 {
            let mut spans = vec![
                Span::styled(" › ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            ];
            spans.extend(ql.spans);
            lines.push(Line::from(spans));
        } else {
            let mut spans = vec![
                Span::raw("   "),
            ];
            spans.extend(ql.spans);
            lines.push(Line::from(spans));
        }
    }

    // 2. Options or text input
    if q.has_options {
        let max_opts = q.options.len().min(8);
        let cur_sel = state.selected[state.current];
        let window_start = if cur_sel >= max_opts {
            cur_sel + 1 - max_opts
        } else {
            0
        };

        for (idx, opt) in q.options.iter().enumerate().skip(window_start).take(max_opts) {
            let is_sel = idx == cur_sel;
            let num = format!("{}. ", idx + 1);
            let cursor = if is_sel { "  › " } else { "    " };

            lines.push(Line::from(vec![
                Span::styled(
                    cursor,
                    if is_sel {
                        Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(
                    num,
                    if is_sel {
                        Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.dark_gray)
                    },
                ),
                Span::styled(
                    opt.clone(),
                    if is_sel {
                        Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.gray)
                    },
                ),
            ]));
        }

        if q.options.len() > max_opts {
            let remaining = q.options.len() - (window_start + max_opts);
            if remaining > 0 {
                lines.push(Line::from(Span::styled(
                    format!("     ... ({} more below)", remaining),
                    Style::default().fg(theme.dark_gray),
                )));
            }
        }
    } else {
        let input = &state.text_input[state.current];
        let q_lower = q.question.to_lowercase();
        let h_lower = q.header.as_deref().unwrap_or("").to_lowercase();
        let is_secret = q_lower.contains("password")
            || q_lower.contains("contraseñ")
            || q_lower.contains("contra")
            || q_lower.contains("secret")
            || h_lower.contains("password")
            || h_lower.contains("contra");

        let display_text = if is_secret {
            "*".repeat(input.chars().count())
        } else {
            input.clone()
        };

        lines.push(Line::from(vec![
            Span::styled("  › ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            Span::styled(display_text, Style::default().fg(theme.foreground)),
            Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_blue)),
        ]));
    }

    // 3. Hint footer
    let hint = if q.has_options {
        if q.options.len() <= 9 {
            format!("  ↑↓ or 1-{} · enter confirm · esc cancel", q.options.len())
        } else {
            "  ↑↓ navigate · enter confirm · esc cancel".to_string()
        }
    } else {
        "  enter confirm · esc cancel".to_string()
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(theme.dark_gray),
    )));

    // Clean unbroken border line
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(Color::Rgb(75, 110, 155)));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_questions() {
        let raw = r#"{"questions":[{"header":"H","question":"What?","options":["A","B"]},{"question":"Text","type":"text"}]}"#;
        let qs = parse_questions(raw).expect("should parse");
        assert_eq!(qs.len(), 2);
        assert!(qs[0].has_options);
        assert_eq!(qs[0].options, vec!["A", "B"]);
        assert_eq!(qs[0].header.as_deref(), Some("H"));
        assert!(!qs[1].has_options);
        assert_eq!(qs[1].question, "Text");
    }

    #[test]
    fn test_parse_questions_rejects_empty() {
        assert!(parse_questions(r#"{"questions":[]}"#).is_none());
        assert!(parse_questions(r#"{"foo":1}"#).is_none());
        assert!(parse_questions("not json").is_none());
    }

    #[test]
    fn test_format_output_selected_option() {
        let raw = r#"{"questions":[{"header":"H","question":"What?","options":["A","B"]}]}"#;
        let qs = parse_questions(raw).unwrap();
        let mut state = UserDialogState::new();
        state.open(qs, Vec::new(), "call-1".to_string());
        state.selected[0] = 1;
        let out = state.format_output();
        assert!(out.contains("B"), "output should contain selected option: {}", out);
    }

    #[test]
    fn test_format_output_cancelled() {
        let raw = r#"{"questions":[{"question":"What?","options":["A"]}]}"#;
        let qs = parse_questions(raw).unwrap();
        let mut state = UserDialogState::new();
        state.open(qs, Vec::new(), "call-1".to_string());
        state.cancelled = true;
        assert!(state.format_output().contains("cancelled"));
    }

    #[test]
    fn test_calculate_dialog_height() {
        let raw = r#"{"questions":[{"question":"Which project?","options":["A","B","C"]}]}"#;
        let qs = parse_questions(raw).unwrap();
        let mut state = UserDialogState::new();
        state.open(qs, Vec::new(), "call-1".to_string());
        let h = calculate_dialog_height(&state, 80, 24);
        assert!(h >= 7 && h <= 12, "height should be reasonably compact: {}", h);
    }
}
