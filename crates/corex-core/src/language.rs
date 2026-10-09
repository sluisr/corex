use crate::types::Message;

/// Range check for CJK ideographs and common Chinese punctuation.
pub fn is_cjk_char(c: char) -> bool {
    matches!(c,
        '\u{4e00}'..='\u{9fff}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4dbf}' // CJK Extension A
        | '\u{20000}'..='\u{2a6df}' // CJK Extension B
        | '\u{f900}'..='\u{faff}' // CJK Compatibility Ideographs
        | '\u{3000}'..='\u{303f}' // CJK Symbols and Punctuation (，。！？【】《》)
    )
}

/// Japanese Kana characters (Hiragana and Katakana).
pub fn is_japanese_kana(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{309f}'   // Hiragana
        | '\u{30a0}'..='\u{30ff}' // Katakana
    )
}

/// Counts the number of CJK characters in a string.
pub fn count_cjk_characters(s: &str) -> usize {
    s.chars().filter(|c| is_cjk_char(*c)).count()
}

/// Checks if string contains any CJK characters.
pub fn contains_cjk(s: &str) -> bool {
    s.chars().any(is_cjk_char)
}

/// Checks if any user message in the session legitimately used CJK (Chinese) or Japanese Kana.
/// If the user wrote Chinese or Japanese, the model is fully allowed to respond in that language.
pub fn user_has_requested_cjk(messages: &[Message]) -> bool {
    for msg in messages {
        if msg.role == "user" {
            if let Some(text) = msg.text_content() {
                if text.chars().any(is_japanese_kana) {
                    return true;
                }
                let cjk_count = count_cjk_characters(text);
                if cjk_count >= 2 && (cjk_count as f32 / text.chars().count().max(1) as f32) > 0.15 {
                    return true;
                }
            }
        }
    }
    false
}

/// Checks if an assistant response contains an accidental/unwanted language drift
/// into Chinese when the user did NOT write in Chinese or Japanese.
pub fn is_unwanted_cjk_drift(assistant_text: &str, user_has_cjk: bool) -> bool {
    if user_has_cjk {
        return false;
    }

    let cjk_count = count_cjk_characters(assistant_text);
    // If there are at least 5 CJK characters and user never wrote in CJK, it is an unprompted drift
    cjk_count >= 5
}

/// Universal, completely dynamic recency anchor injected as a standalone trailing message.
/// Does NOT hardcode any language; instead directs the LLM to dynamically match
/// the user's input language while explicitly preventing Chinese fallback.
///
/// It is deliberately emitted as its own message rather than concatenated onto the trailing
/// tool output. Tool results are untrusted passive data (see the `Untrusted Data` rule in the
/// system prompt), so an instruction appended there is indistinguishable from real file
/// contents or command output: it corrupts the model's view of the data and simultaneously
/// fights the very rule that tells the model to ignore directives inside tool results.
pub const UNIVERSAL_RECENCY_LANGUAGE_ANCHOR: &str =
    "[DIRECTIVE: Synthesize and respond strictly in the exact language used by the user in their query. Do NOT drift into Chinese or any unprompted language.]";

/// Marker used to detect an already-injected anchor so it is never stacked twice.
pub const RECENCY_ANCHOR_MARKER: &str = "DIRECTIVE: Synthesize and respond strictly";

/// Builds the standalone recency anchor message.
pub fn recency_anchor_message() -> Message {
    Message::system(UNIVERSAL_RECENCY_LANGUAGE_ANCHOR)
}

/// Appends the recency anchor as a standalone message at the tail of the context window.
///
/// Skipped when the user legitimately wrote in CJK (Chinese/Japanese), and skipped when the
/// anchor is already the trailing message, so repeated calls cannot stack duplicates.
pub fn apply_recency_anchor(messages: &mut Vec<Message>) {
    if user_has_requested_cjk(messages) {
        return;
    }

    let already_present = messages.last().is_some_and(|m| {
        m.text_content()
            .is_some_and(|t| t.contains(RECENCY_ANCHOR_MARKER))
    });

    if !already_present {
        messages.push(recency_anchor_message());
    }
}

/// Strips the recency anchor from the tail of a content string, which is where older
/// builds spliced it. Returns `None` when nothing was removed, so callers can skip a write.
fn strip_trailing_recency_anchor(text: &str) -> Option<String> {
    let mut out = text.trim_end();
    let mut stripped = false;

    while out.ends_with(UNIVERSAL_RECENCY_LANGUAGE_ANCHOR) {
        out = out[..out.len() - UNIVERSAL_RECENCY_LANGUAGE_ANCHOR.len()].trim_end();
        stripped = true;
    }

    if stripped {
        Some(out.to_string())
    } else {
        None
    }
}

/// Removes the recency anchor that older builds spliced into message content.
///
/// Those builds appended the anchor onto the trailing tool result and the result was then
/// persisted to the session file on disk, so the contamination survives a restart and keeps
/// being replayed to the model on every turn. This heals such history in place: the next time
/// the session is saved, it is written back clean.
///
/// Only the trailing splice is removed, never a mid-text mention, so ordinary prose that
/// happens to quote the directive is left untouched.
pub fn sanitize_messages_recency_anchor(messages: &mut Vec<Message>) {
    // 1. Drop whole messages that are nothing but a persisted anchor.
    messages.retain(|msg| {
        !(msg.role == "system"
            && msg
                .text_content()
                .is_some_and(|t| t.trim() == UNIVERSAL_RECENCY_LANGUAGE_ANCHOR))
    });

    // 2. Strip the anchor where it was spliced onto the tail of a message's content.
    for msg in messages.iter_mut() {
        let Some(ref mut content) = msg.content else {
            continue;
        };

        match content {
            crate::types::MessageContent::Text(s) => {
                if let Some(clean) = strip_trailing_recency_anchor(s) {
                    *s = clean;
                }
            }
            crate::types::MessageContent::Parts(parts) => {
                for part in parts.iter_mut() {
                    if let Some(ref mut text) = part.text {
                        if let Some(clean) = strip_trailing_recency_anchor(text) {
                            *text = clean;
                        }
                    }
                }
            }
        }
    }
}

/// Sanitizes historical assistant messages that suffered from past CJK drift,
/// so the LLM does not see Chinese in its prior turns and continue the drift pattern.
pub fn sanitize_messages_cjk_drift(messages: &mut [Message]) {
    let user_cjk = user_has_requested_cjk(messages);
    if user_cjk {
        return;
    }

    for msg in messages.iter_mut() {
        if msg.role == "assistant" {
            if let Some(ref mut content) = msg.content {
                match content {
                    crate::types::MessageContent::Text(ref mut s) => {
                        if is_unwanted_cjk_drift(s, false) {
                            *s = clean_cjk_drift_from_text(s);
                        }
                    }
                    crate::types::MessageContent::Parts(ref mut parts) => {
                        for p in parts.iter_mut() {
                            if let Some(ref mut t) = p.text {
                                if is_unwanted_cjk_drift(t, false) {
                                    *t = clean_cjk_drift_from_text(t);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Fallback cleaner for text containing CJK drift.
/// Filters out lines containing Chinese ideographs.
pub fn clean_cjk_drift_from_text(text: &str) -> String {
    let mut clean_lines = Vec::new();
    let mut stripped_any = false;

    for line in text.lines() {
        if count_cjk_characters(line) >= 2 {
            stripped_any = true;
        } else {
            clean_lines.push(line);
        }
    }

    let joined = clean_lines.join("\n").trim().to_string();
    if joined.is_empty() || stripped_any {
        if joined.is_empty() {
            "[Response synthesized from tool outputs]".to_string()
        } else {
            joined
        }
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_cjk_drift() {
        let chinese_text = "找过了——而且我会对你坦诚，因为在这个问题上编造事实是最糟糕的事。🔎\n## 关于“11-12岁学电子”这一具体说法";
        assert!(contains_cjk(chinese_text));
        assert!(count_cjk_characters(chinese_text) > 20);
        assert!(is_unwanted_cjk_drift(chinese_text, false));
        assert!(!is_unwanted_cjk_drift(chinese_text, true));
    }

    #[test]
    fn test_user_requested_cjk() {
        let msgs_spanish = vec![Message::user("hola, cómo estás?")];
        assert!(!user_has_requested_cjk(&msgs_spanish));

        let msgs_german = vec![Message::user("Hallo, wie geht es dir?")];
        assert!(!user_has_requested_cjk(&msgs_german));

        let msgs_chinese = vec![Message::user("你好，请帮我检查代码")];
        assert!(user_has_requested_cjk(&msgs_chinese));

        let msgs_japanese = vec![Message::user("こんにちは、テストです")];
        assert!(user_has_requested_cjk(&msgs_japanese));
    }

    #[test]
    fn test_clean_cjk_drift() {
        let mixed = "Information:\n找过了——而且我会对你坦诚\n- Source: https://sluisr.com";
        let cleaned = clean_cjk_drift_from_text(mixed);
        assert!(!contains_cjk(&cleaned));
        assert!(cleaned.contains("Information:"));
        assert!(cleaned.contains("https://sluisr.com"));
    }

    #[test]
    fn test_recency_anchor_does_not_contaminate_tool_output() {
        // The exact regression: a tool result must come out byte-for-byte identical.
        let tool_payload = "alpha\nbeta\ngamma";
        let mut msgs = vec![
            Message::user("analiza esto"),
            Message::tool_response("call_1", tool_payload),
        ];

        apply_recency_anchor(&mut msgs);

        // The anchor lands as its own message at the tail...
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2].role, "system");
        assert!(msgs[2].text_content().unwrap().contains(RECENCY_ANCHOR_MARKER));

        // ...and the untrusted tool payload is untouched.
        assert_eq!(msgs[1].role, "tool");
        assert_eq!(msgs[1].text_content().unwrap(), tool_payload);
    }

    #[test]
    fn test_recency_anchor_is_skipped_without_tool_output() {
        // With no tool call in flight the anchor is still appended, but as a clean message.
        let mut msgs = vec![Message::user("hola")];
        apply_recency_anchor(&mut msgs);

        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text_content().unwrap(), "hola");
        assert_eq!(msgs[1].role, "system");
    }

    #[test]
    fn test_recency_anchor_is_never_duplicated() {
        let mut msgs = vec![Message::user("hola")];
        apply_recency_anchor(&mut msgs);
        apply_recency_anchor(&mut msgs);
        apply_recency_anchor(&mut msgs);

        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn test_recency_anchor_skipped_when_user_wrote_cjk() {
        let mut msgs = vec![
            Message::user("你好，请帮我检查代码"),
            Message::tool_response("call_1", "输出"),
        ];

        apply_recency_anchor(&mut msgs);

        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].role, "tool");
        assert_eq!(msgs[1].text_content().unwrap(), "输出");
    }

    #[test]
    fn test_sanitize_heals_anchor_spliced_into_tool_output() {
        // Exactly what the old build wrote to disk: anchor glued onto the tool result's tail.
        let payload = "alpha\nbeta\ngamma";
        let contaminated = format!("{}\n\n{}", payload, UNIVERSAL_RECENCY_LANGUAGE_ANCHOR);

        let mut msgs = vec![
            Message::user("analiza esto"),
            Message::tool_response("call_1", contaminated),
        ];

        sanitize_messages_recency_anchor(&mut msgs);

        assert_eq!(msgs[1].role, "tool");
        assert_eq!(msgs[1].text_content().unwrap(), payload);
    }

    #[test]
    fn test_sanitize_drops_standalone_anchor_message() {
        let mut msgs = vec![
            Message::user("hola"),
            recency_anchor_message(),
            Message::tool_response("call_1", "salida"),
        ];

        sanitize_messages_recency_anchor(&mut msgs);

        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "tool");
    }

    #[test]
    fn test_sanitize_leaves_mid_text_mention_untouched() {
        // Prose that *quotes* the directive must survive: only the trailing splice is removed.
        let prose = format!("El ancla dice {} y eso es todo.", UNIVERSAL_RECENCY_LANGUAGE_ANCHOR);
        let mut msgs = vec![Message::assistant(prose.clone(), None)];

        sanitize_messages_recency_anchor(&mut msgs);

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text_content().unwrap(), prose);
    }

    #[test]
    fn test_sanitize_is_a_noop_on_clean_history() {
        let mut msgs = vec![
            Message::user("hola"),
            Message::tool_response("call_1", "salida limpia"),
        ];

        sanitize_messages_recency_anchor(&mut msgs);

        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].text_content().unwrap(), "salida limpia");
    }

    #[test]
    fn test_healed_history_gets_exactly_one_fresh_anchor() {
        // Full round trip: contaminated history in, clean tool output + a single anchor out.
        let payload = "contenido real";
        let contaminated = format!("{}\n\n{}", payload, UNIVERSAL_RECENCY_LANGUAGE_ANCHOR);
        let mut msgs = vec![
            Message::user("analiza esto"),
            Message::tool_response("call_1", contaminated),
            recency_anchor_message(),
        ];

        sanitize_messages_recency_anchor(&mut msgs);
        apply_recency_anchor(&mut msgs);

        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1].text_content().unwrap(), payload);
        assert_eq!(msgs[2].role, "system");
        assert!(msgs[2].text_content().unwrap().contains(RECENCY_ANCHOR_MARKER));
    }
}
