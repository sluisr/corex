use std::time::{Duration, Instant};

use corex_core::{truncate_ellipsis, Message};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::theme::Theme;

#[derive(Default)]
pub struct MessageClock {
    session: String,
    seen: Vec<Instant>,
}

impl MessageClock {
    pub fn observe(&mut self, session: &str, count: usize) {
        if self.session != session {
            self.session = session.to_string();
            self.seen.clear();
        }
        self.seen.truncate(count);
        let now = Instant::now();
        self.seen.resize(count, now);
    }

    fn between(&self, start: usize, end: usize, live: bool) -> Option<Duration> {
        let from = *self.seen.get(start)?;
        let to = if live { Instant::now() } else { *self.seen.get(end.checked_sub(1)?)? };
        let took = to.saturating_duration_since(from);
        (live || took >= Duration::from_secs(1)).then_some(took)
    }
}

fn is_browser_step(msg: &Message) -> bool {
    msg.role == "assistant"
        && msg
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty() && calls.iter().all(|c| c.function.name == "browser"))
}

fn joins_run(msg: &Message) -> bool {
    match msg.role.as_str() {
        "tool" => true,
        "user" => corex_core::is_tool_image_message(msg),
        "assistant" => is_browser_step(msg),
        _ => false,
    }
}

pub fn run_start(messages: &[Message], idx: usize) -> Option<usize> {
    let mut start = None;
    for i in (0..=idx.min(messages.len().checked_sub(1)?)).rev() {
        let msg = &messages[i];
        if is_browser_step(msg) {
            start = Some(i);
        }
        if !joins_run(msg) {
            break;
        }
    }
    start
}

pub fn run_end(messages: &[Message], start: usize, limit: usize) -> usize {
    let limit = limit.min(messages.len());
    let mut end = (start + 1).min(limit);
    while end < limit && joins_run(&messages[end]) {
        end += 1;
    }
    end
}

pub fn without_calls(msg: &Message) -> Message {
    Message { tool_calls: None, ..msg.clone() }
}

fn is_error(content: &str) -> bool {
    content.starts_with("Error") || content.starts_with("Failed")
}

pub fn summary_lines(
    run: &[Message],
    start: usize,
    clock: &MessageClock,
    live: bool,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let calls: Vec<&str> = run
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flatten()
        .map(|c| c.function.arguments.as_str())
        .collect();
    let results: Vec<&str> = run
        .iter()
        .filter(|m| m.role == "tool")
        .map(|m| m.text_content().unwrap_or(""))
        .collect();
    let failed = results.iter().filter(|r| is_error(r)).count();

    let (icon, color) = if live {
        ("● ", theme.accent_cyan)
    } else if results.last().is_some_and(|r| is_error(r)) {
        ("✕ ", theme.error)
    } else {
        ("✓ ", theme.success)
    };

    let steps = calls.len();
    let mut meta = format!(" · {} step{}", steps, if steps == 1 { "" } else { "s" });
    if let Some(took) = clock.between(start, start + run.len(), live) {
        meta.push_str(&format!(" · {}", elapsed(took)));
    }
    if failed > 0 {
        meta.push_str(&format!(" · {failed} failed"));
    }

    let mut spans = vec![
        Span::styled(icon, Style::default().fg(color)),
        Span::styled("Browser", Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD)),
        Span::styled(meta, Style::default().fg(theme.gray)),
    ];
    if let Some(last) = calls.last() {
        let mut step = describe(last);
        if live {
            step.push('…');
        }
        spans.push(Span::styled(" — ", Style::default().fg(theme.dark_gray)));
        spans.push(Span::styled(step, Style::default().fg(theme.foreground)));
    }

    let mut lines = crate::markdown::wrap_spans_with_indent(spans, width, "  ", "    ");
    lines.push(Line::from(""));
    lines
}

fn elapsed(took: Duration) -> String {
    let secs = took.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

fn quoted(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    format!("\"{}\"", truncate_ellipsis(line, 40))
}

pub fn describe(raw_args: &str) -> String {
    let args: Value = serde_json::from_str(raw_args).unwrap_or(Value::Null);
    let text = |key: &str| args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty());
    let flag = |key: &str| args.get(key).and_then(Value::as_bool);
    let target = || text("ref").map(|r| format!(" {r}")).unwrap_or_default();

    match text("action").unwrap_or("") {
        "navigate" => {
            let url = text("url").unwrap_or("");
            let shown = url.split_once("://").map_or(url, |(_, rest)| rest).trim_end_matches('/');
            format!("opening {}", truncate_ellipsis(shown, 50))
        }
        "snapshot" => match text("query") {
            Some(q) => format!("looking for {}", quoted(q)),
            None => "reading the page".to_string(),
        },
        "click" => format!("clicking{}", target()),
        "double_click" => format!("double-clicking{}", target()),
        "right_click" => format!("right-clicking{}", target()),
        "hover" => format!("hovering{}", target()),
        "type" => format!("typing {}", quoted(text("text").unwrap_or(""))),
        "select" | "choose" => format!("choosing {}", quoted(text("value").unwrap_or(""))),
        "upload" => {
            let count = args.get("paths").and_then(Value::as_array).map_or(0, Vec::len);
            format!("attaching {} file{}", count, if count == 1 { "" } else { "s" })
        }
        "dialog" if flag("accept") == Some(false) => "dismissing the dialog".to_string(),
        "dialog" => "accepting the dialog".to_string(),
        "press_key" => format!("pressing {}", text("key").unwrap_or("a key")),
        "scroll" => format!("scrolling {}{}", text("direction").unwrap_or("down"), target()),
        "back" => "going back".to_string(),
        "text" => "reading the text".to_string(),
        "screenshot" => "taking a screenshot".to_string(),
        "console" => "reading the console".to_string(),
        "wait" => match text("text") {
            Some(t) if flag("gone") == Some(true) => format!("waiting for {} to go away", quoted(t)),
            Some(t) => format!("waiting for {}", quoted(t)),
            None => {
                let secs = args.get("seconds").and_then(Value::as_f64).unwrap_or(1.0);
                format!("waiting {secs}s")
            }
        },
        "tabs" if flag("close") == Some(true) => "closing a tab".to_string(),
        "tabs" if args.get("tab").is_some() => "switching tabs".to_string(),
        "tabs" => "listing tabs".to_string(),
        "close" => "closing the browser".to_string(),
        other if !other.is_empty() => other.replace('_', " "),
        _ => "working".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corex_core::{FunctionCall, ToolCall};

    fn step(args: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            tool_calls: Some(vec![ToolCall {
                id: args.len().to_string(),
                call_type: "function".to_string(),
                function: FunctionCall { name: "browser".to_string(), arguments: args.to_string() },
                extra_content: None,
            }]),
            ..Default::default()
        }
    }

    fn result(text: &str) -> Message {
        Message { role: "tool".to_string(), ..Message::user(text) }
    }

    #[test]
    fn groups_consecutive_browser_steps() {
        let messages = vec![
            Message::user("send hi"),
            step(r#"{"action":"navigate","url":"https://web.whatsapp.com/"}"#),
            result("Opened"),
            step(r#"{"action":"type","text":"hi","ref":"e4"}"#),
            result("Typed"),
            Message { role: "assistant".to_string(), ..Message::user("Done.") },
        ];
        assert_eq!(run_start(&messages, 0), None);
        assert_eq!(run_start(&messages, 1), Some(1));
        assert_eq!(run_start(&messages, 4), Some(1));
        assert_eq!(run_start(&messages, 5), None);
        assert_eq!(run_end(&messages, 1, messages.len()), 5);
        assert_eq!(run_end(&messages, 1, 3), 3);
    }

    #[test]
    fn describes_the_step() {
        assert_eq!(describe(r#"{"action":"navigate","url":"https://web.whatsapp.com/"}"#), "opening web.whatsapp.com");
        assert_eq!(describe(r#"{"action":"type","text":"hola, ya llegué","ref":"e4"}"#), "typing \"hola, ya llegué\"");
        assert_eq!(describe(r#"{"action":"wait","text":"Saving","gone":true}"#), "waiting for \"Saving\" to go away");
        assert_eq!(describe(r#"{"action":"click","ref":"e12"}"#), "clicking e12");
    }
}
