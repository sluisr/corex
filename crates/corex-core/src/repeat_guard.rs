//! Advisory nudge for a model stuck calling the same tool with identical arguments.
//!
//! It never blocks a call: at the 3rd, 5th and 8th consecutive identical call the tool result gets
//! a short reminder appended, asking the model to look at the previous result and change approach.

use std::sync::{Mutex, OnceLock};

/// Consecutive identical calls at which a reminder is attached.
const THRESHOLDS: [usize; 3] = [3, 5, 8];
/// Longest argument preview shown in a detailed reminder.
const ARGS_PREVIEW_CHARS: usize = 300;
/// Tools that legitimately repeat the same call while they wait for something to change.
const EXEMPT_TOOLS: &[&str] = &[
    "write_todos",
    "read_background_output",
    "list_background_processes",
    "manage_subagents",
    "subagents",
    "manage_task",
    "tasks",
    "update_topic",
];

struct Chain {
    key: String,
    count: usize,
}

fn chain() -> &'static Mutex<Option<Chain>> {
    static CHAIN: OnceLock<Mutex<Option<Chain>>> = OnceLock::new();
    CHAIN.get_or_init(Default::default)
}

/// Same JSON value with object keys sorted, so `{"a":1,"b":2}` and `{"b":2,"a":1}` compare equal.
fn canonical(value: &serde_json::Value) -> String {
    fn sorted(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                serde_json::Value::Object(entries.into_iter().map(|(k, v)| (k.clone(), sorted(v))).collect())
            }
            serde_json::Value::Array(items) => serde_json::Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(value).to_string()
}

/// Records a tool call and returns a reminder when it is the Nth identical call in a row.
pub fn observe_tool_call(name: &str, raw_arguments: &str) -> Option<String> {
    if EXEMPT_TOOLS.contains(&name) {
        return None;
    }
    let args = crate::parse_lenient_json(raw_arguments).unwrap_or(serde_json::Value::Null);
    let canon = canonical(&args);
    let key = format!("{}\u{0}{}", name, canon);

    let mut guard = chain().lock().ok()?;
    let count = match guard.as_ref() {
        Some(c) if c.key == key => c.count + 1,
        _ => 1,
    };
    *guard = Some(Chain { key, count });

    if !THRESHOLDS.contains(&count) {
        return None;
    }
    if count == THRESHOLDS[0] {
        return Some(format!(
            "[Reminder: `{}` has now been called {} times in a row with identical arguments. Check the previous result: if it did not give you what you need, change your approach; if it did, move on or finish.]",
            name, count
        ));
    }
    let preview: String = canon.chars().take(ARGS_PREVIEW_CHARS).collect();
    Some(format!(
        "[Reminder: `{}` has been called {} times in a row with identical arguments ({}). Repeating it will not change the result. Stop, re-read what it returned, and try a different approach or report what is blocking you.]",
        name, count, preview
    ))
}

/// Forgets the current chain; call it when the user sends a new message.
pub fn reset_repeat_guard() {
    if let Ok(mut guard) = chain().lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The chain is process-wide, so one test owns the whole scenario.
    #[test]
    fn reminders_fire_at_the_thresholds_for_identical_calls_only() {
        reset_repeat_guard();
        let a = r#"{"pattern":"x","dir":"src"}"#;
        let a_reordered = r#"{"dir":"src","pattern":"x"}"#;
        assert!(observe_tool_call("grep_search", a).is_none());
        assert!(observe_tool_call("grep_search", a_reordered).is_none());
        let third = observe_tool_call("grep_search", a).expect("reminder at 3");
        assert!(third.contains("3 times"));
        assert!(observe_tool_call("grep_search", a).is_none()); // 4
        let fifth = observe_tool_call("grep_search", a).expect("detailed reminder at 5");
        assert!(fifth.contains("5 times") && fifth.contains("\"pattern\""));

        // A different call breaks the chain.
        assert!(observe_tool_call("grep_search", r#"{"pattern":"y"}"#).is_none());
        assert!(observe_tool_call("grep_search", a).is_none());

        // Exempt tools never count.
        reset_repeat_guard();
        for _ in 0..10 {
            assert!(observe_tool_call("write_todos", "{}").is_none());
        }
    }
}
