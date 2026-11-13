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

    pub fn on_custom_row(&self, i: usize) -> bool {
        self.questions.get(i).is_some_and(|q| q.has_options && self.selected[i] == q.options.len())
    }

    pub fn current_needs_text(&self) -> bool {
        self.questions.get(self.current).is_some_and(|q| !q.has_options) || self.on_custom_row(self.current)
    }

    /// Formats the user's answers as the `ask_user` tool output.
    pub fn format_output(&self) -> String {
        if self.cancelled {
            return "User cancelled the questions. Proceed without the missing information or adjust your approach.".to_string();
        }
        let mut out = String::from("User's responses:\n");
        for (i, q) in self.questions.iter().enumerate() {
            let answer = if q.has_options && !self.on_custom_row(i) {
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
        let mut options: Vec<String> = q
            .get("options")
            .and_then(|o| o.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if options.len() > 1 && options.last().is_some_and(|o| is_catch_all(o)) {
            options.pop();
        }
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

fn is_catch_all(option: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "other", "something else", "none of", "otra", "otro", "algo más", "algo mas", "ninguna", "ninguno",
    ];
    let lower = option.trim().to_lowercase();
    PREFIXES.iter().any(|p| {
        lower.strip_prefix(p).is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
    })
}

/// `text` broken into rows of at most `width` columns, at spaces when it can. A word wider than a
/// row (a URL, a path) is split across rows instead of cut, so no part of an option is ever lost.
fn wrap_rows(text: &str, width: usize) -> Vec<String> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut cur = String::new();
        let mut cur_w = 0usize;
        for word in line.split_whitespace() {
            let word_w = UnicodeWidthStr::width(word);
            let gap = usize::from(!cur.is_empty());
            if cur_w + gap + word_w <= width {
                if gap == 1 {
                    cur.push(' ');
                }
                cur.push_str(word);
                cur_w += gap + word_w;
                continue;
            }
            if !cur.is_empty() {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            for ch in word.chars() {
                let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if cur_w + w > width && !cur.is_empty() {
                    rows.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push(ch);
                cur_w += w;
            }
        }
        if !cur.is_empty() {
            rows.push(cur);
        }
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

/// Most options listed at once; the rest scroll with the selection.
const MAX_VISIBLE_OPTIONS: usize = 8;

const CUSTOM_PLACEHOLDER: &str = "Type something else…";

/// Every row of the dialog body for `width` columns, plus the rows the selected option spans so
/// the renderer can keep it on screen when the terminal is too short for everything.
/// Long questions and options wrap with a hanging indent instead of being clipped at the edge,
/// which on narrow terminals (Termux, split panes) used to hide most of the text.
fn dialog_lines(q: &UserQuestion, state: &UserDialogState, width: u16, theme: &Theme) -> (Vec<Line<'static>>, std::ops::Range<usize>) {
    let width = width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    // 1. The question, behind a prompt bullet; continuation rows align under its text.
    let bullet = Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    for (i, row) in wrap_rows(&q.question, width.saturating_sub(4).max(10)).into_iter().enumerate() {
        let lead = if i == 0 { Span::styled(" › ", bullet) } else { Span::raw("   ") };
        lines.push(Line::from(vec![lead, Span::styled(row, bold)]));
    }

    let mut selected_rows = lines.len()..lines.len();

    // 2. Options or text input.
    if q.has_options {
        let total = q.options.len() + 1;
        let max_opts = total.min(MAX_VISIBLE_OPTIONS);
        let cur_sel = state.selected[state.current];
        let window_start = if cur_sel >= max_opts { cur_sel + 1 - max_opts } else { 0 };
        let num_w = total.to_string().len() + 2;
        let typed = &state.text_input[state.current];

        for idx in (window_start..total).take(max_opts) {
            let is_sel = idx == cur_sel;
            let is_custom = idx == q.options.len();
            let (marker_style, num_style, mut text_style) = if is_sel {
                (bullet, bullet, Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD))
            } else {
                (Style::default(), Style::default().fg(theme.dark_gray), Style::default().fg(theme.gray))
            };
            let text = if !is_custom {
                q.options[idx].clone()
            } else if typed.is_empty() {
                text_style = Style::default().fg(theme.dark_gray).add_modifier(Modifier::ITALIC);
                CUSTOM_PLACEHOLDER.to_string()
            } else {
                typed.clone()
            };
            let first = lines.len();
            let text_w = width.saturating_sub(4 + num_w + 1).max(10);
            let rows = wrap_rows(&text, text_w);
            let last = rows.len() - 1;
            for (i, row) in rows.into_iter().enumerate() {
                let lead = if i == 0 {
                    vec![
                        Span::styled(if is_sel { "  › " } else { "    " }, marker_style),
                        Span::styled(format!("{:<num_w$}", format!("{}.", idx + 1)), num_style),
                    ]
                } else {
                    vec![Span::raw(" ".repeat(4 + num_w))]
                };
                let mut spans = lead;
                spans.push(Span::styled(row, text_style));
                if is_custom && is_sel && i == last {
                    spans.push(Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_blue)));
                }
                lines.push(Line::from(spans));
            }
            if is_sel {
                selected_rows = first..lines.len();
            }
        }

        let remaining = total.saturating_sub(window_start + max_opts);
        if remaining > 0 {
            lines.push(Line::from(Span::styled(
                format!("     ... ({} more below)", remaining),
                Style::default().fg(theme.dark_gray),
            )));
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

        let display_text = if is_secret { "*".repeat(input.chars().count()) } else { input.clone() };

        lines.push(Line::from(vec![
            Span::styled("  › ", bullet),
            Span::styled(display_text, Style::default().fg(theme.foreground)),
            Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_blue)),
        ]));
        selected_rows = lines.len() - 1..lines.len();
    }

    // 3. Hint footer.
    let hint = if state.on_custom_row(state.current) {
        "  ↑↓ options · enter confirm · esc cancel".to_string()
    } else if q.has_options {
        if q.options.len() < 9 {
            format!("  ↑↓ or 1-{} · enter confirm · esc cancel", q.options.len() + 1)
        } else {
            "  ↑↓ navigate · enter confirm · esc cancel".to_string()
        }
    } else {
        "  enter confirm · esc cancel".to_string()
    };
    lines.push(Line::from(Span::styled(hint, Style::default().fg(theme.dark_gray))));

    (lines, selected_rows)
}

pub fn calculate_dialog_height(state: &UserDialogState, width: u16, max_height: u16) -> u16 {
    let Some(q) = state.questions.get(state.current) else {
        return 5;
    };
    let (lines, _) = dialog_lines(q, state, width, &Theme::default());
    // Borders: top + bottom = 2
    (lines.len() as u16 + 2).min(max_height.saturating_sub(6)).max(5)
}

pub fn render_user_dialog(frame: &mut Frame, area: Rect, state: &UserDialogState, theme: &Theme) {
    let Some(q) = state.questions.get(state.current) else {
        return;
    };

    let (lines, selected_rows) = dialog_lines(q, state, area.width, theme);

    // When the terminal is too short for every row, scroll just enough that the selected option
    // stays fully visible rather than clipping it off the bottom.
    let inner_h = area.height.saturating_sub(2) as usize;
    let scroll = selected_rows.end.saturating_sub(inner_h).min(selected_rows.start) as u16;

    // Clean unbroken border line
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(Color::Rgb(75, 110, 155)));

    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
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
    fn trailing_catch_all_option_is_dropped() {
        let raw = r#"{"questions":[{"question":"Q?","options":["Lanzar la TUI","Otra cosa (te la digo)"]},{"question":"Q2?","options":["Otros ajustes","Other"]}]}"#;
        let qs = parse_questions(raw).unwrap();
        assert_eq!(qs[0].options, vec!["Lanzar la TUI"]);
        assert_eq!(qs[1].options, vec!["Otros ajustes"]);
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
    fn custom_row_answer_replaces_options() {
        let mut state = open_with(r#"{"questions":[{"question":"What?","options":["A","B"]}]}"#);
        state.selected[0] = 2;
        state.text_input[0] = "something else".to_string();
        assert!(state.on_custom_row(0));
        let out = state.format_output();
        assert!(out.contains("-> something else"), "{out}");
        let (lines, sel) = dialog_lines(state.current_question().unwrap(), &state, 80, &Theme::default());
        assert!(plain(&lines[sel.start]).contains("3. something else"));
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

    fn open_with(raw: &str) -> UserDialogState {
        let mut state = UserDialogState::new();
        state.open(parse_questions(raw).unwrap(), Vec::new(), "call-1".to_string());
        state
    }

    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn long_options_wrap_instead_of_being_clipped() {
        let state = open_with(
            r#"{"questions":[{"question":"Which one?","options":["Rewrite the whole renderer so every option wraps on narrow terminals","B"]}]}"#,
        );
        let q = state.current_question().unwrap();
        let (lines, sel) = dialog_lines(q, &state, 40, &Theme::default());
        let rows: Vec<String> = lines.iter().map(plain).collect();
        for row in &rows {
            assert!(unicode_width::UnicodeWidthStr::width(row.as_str()) <= 40, "row overflows: {row:?}");
        }
        assert!(sel.len() > 1, "the long option should span several rows: {rows:#?}");
        let joined = rows[sel.clone()].iter().map(|r| r.trim()).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("narrow terminals"), "{joined}");
        // Continuation rows hang under the option text, not under the number.
        assert!(rows[sel.start + 1].starts_with("       "), "{rows:#?}");
    }

    #[test]
    fn height_grows_with_wrapped_options() {
        let state = open_with(
            r#"{"questions":[{"question":"Q?","options":["one two three four five six seven eight nine ten eleven twelve","B"]}]}"#,
        );
        assert!(calculate_dialog_height(&state, 30, 60) > calculate_dialog_height(&state, 200, 60));
    }

    #[test]
    fn words_wider_than_a_row_are_split_not_dropped() {
        let rows = wrap_rows("see https://example.com/a/very/long/path/that/does/not/fit", 12);
        assert!(rows.iter().all(|r| unicode_width::UnicodeWidthStr::width(r.as_str()) <= 12), "{rows:?}");
        assert_eq!(rows.concat().replace(' ', ""), "seehttps://example.com/a/very/long/path/that/does/not/fit");
    }
}
