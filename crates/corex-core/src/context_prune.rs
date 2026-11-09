//! Deterministic trimming of oversized tool results, applied before a conversation is summarized.
//!
//! Large `grep`, `read_file` or shell outputs dominate the context window long after the model has
//! acted on them. Cutting each to its head and tail costs no model call and often frees enough
//! room that no summary is needed at all.

use std::collections::HashSet;

use crate::spill::{trim_with_spill, TRIM_MARKER};
use crate::types::{Message, MessageContent};

const STALE_SNAPSHOT_NOTE: &str = "[page snapshot omitted: a newer browser result follows, and refs from this one are stale]";
const STALE_SNAPSHOT_MIN_LINES: usize = 20;

/// Tool results with more characters than this are trimmed.
pub const PRUNE_THRESHOLD_CHARS: usize = 8192;
/// Leading characters kept from a trimmed result.
pub const PRUNE_HEAD_CHARS: usize = 4096;
/// Trailing characters kept from a trimmed result.
pub const PRUNE_TAIL_CHARS: usize = 1024;

/// Result of a pruning pass.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneStats {
    pub pruned: usize,
    pub chars_removed: usize,
}

/// Trims every oversized `tool` message in place and reports what changed.
///
/// The results of the turn in progress — `tool` messages after the last assistant tool-call batch —
/// are left whole, because the model has not acted on them yet. Already-trimmed results are skipped.
pub fn prune_tool_results(messages: &mut [Message]) -> PruneStats {
    let protected_from = messages
        .iter()
        .rposition(|m| m.role == "assistant" && m.tool_calls.is_some())
        .map_or(messages.len(), |i| i + 1);

    let mut stats = PruneStats::default();
    for message in messages.iter_mut().take(protected_from) {
        if message.role != "tool" {
            continue;
        }
        let Some(MessageContent::Text(text)) = &message.content else {
            continue;
        };
        let total = text.chars().count();
        if total <= PRUNE_THRESHOLD_CHARS || text.contains(TRIM_MARKER) {
            continue;
        }
        let trimmed = trim_with_spill("pruned", text, PRUNE_HEAD_CHARS, PRUNE_TAIL_CHARS);
        stats.chars_removed += total.saturating_sub(trimmed.chars().count());
        stats.pruned += 1;
        message.content = Some(MessageContent::Text(trimmed));
    }
    stats
}

pub fn collapse_stale_browser_snapshots(messages: &mut [Message]) {
    let browser_calls: HashSet<String> = messages
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flatten()
        .filter(|call| call.function.name == "browser")
        .map(|call| call.id.clone())
        .collect();
    let results: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "tool" && m.tool_call_id.as_ref().is_some_and(|id| browser_calls.contains(id)))
        .map(|(index, _)| index)
        .collect();
    let Some((_, older)) = results.split_last() else { return };
    for &index in older {
        let Some(MessageContent::Text(text)) = &messages[index].content else { continue };
        let lines: Vec<&str> = text.lines().collect();
        let Some(tree_start) = lines.iter().position(|line| line.trim_start().starts_with("- ")) else { continue };
        if lines.len() - tree_start < STALE_SNAPSHOT_MIN_LINES {
            continue;
        }
        let mut collapsed = lines[..tree_start].join("\n");
        collapsed.push('\n');
        collapsed.push_str(STALE_SNAPSHOT_NOTE);
        messages[index].content = Some(MessageContent::Text(collapsed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCall;

    fn call_batch() -> Message {
        Message::assistant_with_tools(None, None, vec![ToolCall::new("c", "grep_search", "{}")])
    }

    #[test]
    fn oversized_results_keep_head_and_tail_and_the_current_turn_is_untouched() {
        let big = format!("{}{}{}", "A".repeat(5000), "M".repeat(20_000), "Z".repeat(3000));
        let mut msgs = vec![
            Message::user("go"),
            call_batch(),
            Message::tool_response("c", big.clone()),
            Message::assistant("done", None),
            call_batch(),
            Message::tool_response("c", big.clone()), // current turn: protected
        ];
        let stats = prune_tool_results(&mut msgs);
        assert_eq!(stats.pruned, 1);
        let pruned = msgs[2].text_content().unwrap();
        assert!(pruned.starts_with(&"A".repeat(4096)));
        assert!(pruned.ends_with(&"Z".repeat(1024)));
        assert!(pruned.contains("middle pruned"));
        assert!(!pruned.contains(&"M".repeat(100)));
        // The cut points at a file holding the complete original.
        let path = pruned.split("saved at ").nth(1).and_then(|r| r.split(" — ").next()).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), big);
        let _ = std::fs::remove_file(path);
        assert_eq!(msgs[5].text_content().unwrap(), big);
        // A second pass finds nothing left to trim.
        assert_eq!(prune_tool_results(&mut msgs), PruneStats::default());
    }

    #[test]
    fn small_results_and_multibyte_text_are_safe() {
        let mut msgs = vec![
            Message::user("go"),
            call_batch(),
            Message::tool_response("c", "ok"),
            Message::tool_response("c", "ñ".repeat(9000)),
            Message::assistant("done", None),
            call_batch(),
        ];
        let stats = prune_tool_results(&mut msgs);
        assert_eq!(stats.pruned, 1);
        assert_eq!(msgs[2].text_content().unwrap(), "ok");
        assert!(msgs[3].text_content().unwrap().starts_with("ññ"));
    }

    #[test]
    fn keeps_only_the_newest_browser_snapshot() {
        let tree = |label: &str| format!("{label}\nPage: Inbox (https://mail.test/)\n{}", "- button \"x\" [ref=e1]\n".repeat(30));
        let mut msgs = vec![
            Message::assistant_with_tools(None, None, vec![ToolCall::new("b1", "browser", "{}")]),
            Message::tool_response("b1", tree("Clicked e4")),
            Message::assistant_with_tools(None, None, vec![ToolCall::new("g1", "grep_search", "{}")]),
            Message::tool_response("g1", "- grep line\n".repeat(30)),
            Message::assistant_with_tools(None, None, vec![ToolCall::new("b2", "browser", "{}")]),
            Message::tool_response("b2", tree("Typed into e9")),
        ];
        collapse_stale_browser_snapshots(&mut msgs);
        let first = msgs[1].text_content().unwrap();
        assert!(first.starts_with("Clicked e4\nPage: Inbox"));
        assert!(first.ends_with(STALE_SNAPSHOT_NOTE));
        assert!(!first.contains("[ref="));
        assert!(msgs[3].text_content().unwrap().contains("grep line"));
        assert!(msgs[5].text_content().unwrap().contains("[ref=e1]"));
    }
}
