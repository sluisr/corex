//! Small text helpers shared by the dialogs: column-aware fitting and display formatting.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Fits `s` into `width` columns. Whole words are kept when that loses little, and a `…` always
/// marks that something was dropped, so a cut never reads as if it were the full text.
pub fn fit_text(s: &str, width: usize) -> String {
    if UnicodeWidthStr::width(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    if let Some(pos) = out.rfind(' ') {
        if pos * 2 >= out.len() {
            out.truncate(pos);
        }
    }
    let kept = out.trim_end_matches([' ', '·']);
    format!("{}…", kept)
}

/// `fit_text` followed by spaces up to exactly `w` columns.
pub fn pad_right(s: &str, w: usize) -> String {
    let fitted = fit_text(s, w);
    let used = UnicodeWidthStr::width(fitted.as_str());
    format!("{}{}", fitted, " ".repeat(w.saturating_sub(used)))
}

/// `195373` -> `195,373`.
pub fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Plain one-line rendering of markdown for places that cannot draw it (previews): emphasis and
/// code markers, heading/list prefixes and link targets are dropped, keeping the readable words.
pub fn strip_inline_markdown(text: &str) -> String {
    let line = text.trim_start();
    let line = line.trim_start_matches('#').trim_start();
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("> "))
        .unwrap_or(line);

    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' | '`' | '_' if chars.peek() == Some(&ch) || ch == '`' => {
                // `**`, `__` and single backticks are markers; a lone `*` / `_` may be a real char.
                if ch != '`' {
                    chars.next();
                }
            }
            '[' => {
                // [label](url) -> label
                let rest: String = chars.clone().collect();
                if let Some(close) = rest.find("](") {
                    if let Some(end) = rest[close..].find(')') {
                        out.push_str(&rest[..close]);
                        for _ in 0..rest[..close + end + 1].chars().count() {
                            chars.next();
                        }
                        continue;
                    }
                }
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    drop_single_emphasis(&out).trim().to_string()
}

/// Removes `*word*` / `_word_` emphasis markers. A marker only counts when it opens at a word
/// start, is followed by text, and a closing marker follows on the line: so `a * b`, `2 * 3 * 4`
/// and `snake_case_name` keep every character.
fn drop_single_emphasis(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut dropped = vec![false; chars.len()];

    for i in 0..chars.len() {
        let marker = chars[i];
        if (marker != '*' && marker != '_') || dropped[i] {
            continue;
        }
        let at_word_start = i == 0 || !chars[i - 1].is_alphanumeric();
        let followed_by_text = chars.get(i + 1).is_some_and(|n| !n.is_whitespace() && *n != marker);
        if !at_word_start || !followed_by_text {
            continue;
        }
        let closer = (i + 2..chars.len()).find(|&j| {
            chars[j] == marker
                && !dropped[j]
                && !chars[j - 1].is_whitespace()
                && chars.get(j + 1).is_none_or(|n| !n.is_alphanumeric())
        });
        if let Some(j) = closer {
            dropped[i] = true;
            dropped[j] = true;
        }
    }

    chars
        .iter()
        .zip(&dropped)
        .filter(|(_, &gone)| !gone)
        .map(|(c, _)| *c)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_text_marks_cuts_and_prefers_whole_words() {
        assert_eq!(fit_text("short", 10), "short");
        assert_eq!(fit_text("GitHub Models (Free / Ultra-fast)", 19), "GitHub Models…");
        assert_eq!(fit_text("Everyday coding & discovery", 20), "Everyday coding &…");
        // No usable word boundary: cut mid-word, but still flagged.
        assert_eq!(fit_text("DeepSeek-V4.1-Flash-1017", 10), "DeepSeek-…");
        // Wide glyphs count as two columns; accents are not multi-column.
        assert!(UnicodeWidthStr::width(fit_text("日本語日本語", 7).as_str()) <= 7);
        assert_eq!(UnicodeWidthStr::width(pad_right("café for the señor and the niño", 12).as_str()), 12);
        assert_eq!(pad_right("ab", 5), "ab   ");
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_000), "1,000");
        assert_eq!(group_thousands(195_373), "195,373");
        assert_eq!(group_thousands(12_345_678), "12,345,678");
    }

    #[test]
    fn markdown_markers_are_dropped_but_words_stay() {
        assert_eq!(strip_inline_markdown("Honestly: **I don't know** but `ls -la` helps"), "Honestly: I don't know but ls -la helps");
        assert_eq!(strip_inline_markdown("## Summary"), "Summary");
        assert_eq!(strip_inline_markdown("- first point"), "first point");
        assert_eq!(strip_inline_markdown("see [the café guide](https://x.dev/a) now"), "see the café guide now");
        assert_eq!(strip_inline_markdown("a * b = c"), "a * b = c");
        assert_eq!(strip_inline_markdown("2 * 3 * 4"), "2 * 3 * 4");
        assert_eq!(strip_inline_markdown("the *real why* of it"), "the real why of it");
        assert_eq!(strip_inline_markdown("an _alias_ is *porqué*, not `x`"), "an alias is porqué, not x");
        assert_eq!(strip_inline_markdown("unclosed *star"), "unclosed *star");
        assert_eq!(strip_inline_markdown("snake_case_name"), "snake_case_name");
    }
}
