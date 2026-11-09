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

pub fn fit_path(path: &str, width: usize) -> String {
    let cols = UnicodeWidthStr::width;
    if cols(path) <= width {
        return path.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let trimmed = path.trim_end_matches('/');
    let trailing = &path[trimmed.len()..];
    let parts: Vec<&str> = trimmed.split('/').collect();
    let n = parts.len();
    let tail = |k: usize| format!("{}{trailing}", parts[n - k..].join("/"));
    let Some(k) = (1..n).rev().find(|&k| 2 + cols(&tail(k)) <= width) else {
        let name = tail(1);
        let mut kept: Vec<char> = Vec::new();
        let mut used = 1;
        for ch in name.chars().rev() {
            let w = ch.width().unwrap_or(0);
            if used + w > width {
                break;
            }
            kept.push(ch);
            used += w;
        }
        return format!("…{}", kept.into_iter().rev().collect::<String>());
    };
    let head = (0..n - k)
        .rev()
        .find(|&h| h == 0 || cols(&parts[..h].join("/")) + 3 + cols(&tail(k)) <= width)
        .unwrap_or(0);
    if head == 0 {
        format!("…/{}", tail(k))
    } else {
        format!("{}/…/{}", parts[..head].join("/"), tail(k))
    }
}

pub fn wrap_path(path: &str, width: usize, rows: usize) -> Vec<String> {
    let cols = UnicodeWidthStr::width;
    let mut lines: Vec<String> = Vec::new();
    let mut rest = path;
    while !rest.is_empty() && width > 0 {
        if lines.len() + 1 == rows || cols(rest) <= width {
            lines.push(fit_path(rest, width));
            break;
        }
        let mut used = 0;
        let mut cut = rest.len();
        for (i, ch) in rest.char_indices() {
            used += ch.width().unwrap_or(0);
            if used > width {
                cut = i;
                break;
            }
        }
        if let Some(slash) = rest[..cut].rfind('/').filter(|&s| cols(&rest[..=s]) * 2 >= width) {
            cut = slash + 1;
        }
        if cut == 0 {
            // A character wider than the line cannot be drawn.
            rest = &rest[rest.chars().next().map_or(rest.len(), char::len_utf8)..];
            continue;
        }
        lines.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    lines
}

/// `fit_text` followed by spaces up to exactly `w` columns.
pub fn pad_right(s: &str, w: usize) -> String {
    let fitted = fit_text(s, w);
    let used = UnicodeWidthStr::width(fitted.as_str());
    format!("{}{}", fitted, " ".repeat(w.saturating_sub(used)))
}

/// `text` broken into lines of at most `width` columns at spaces; a word longer than a line is cut
/// with `…` by [`fit_text`].
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let needed = UnicodeWidthStr::width(current.as_str()) + usize::from(!current.is_empty()) + UnicodeWidthStr::width(word);
        if !current.is_empty() && needed > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines.into_iter().map(|l| fit_text(&l, width)).collect()
}

/// `950` -> `950`, `120_000` -> `120k`, `1_000_000` -> `1M`, `1_250_000` -> `1.2M`.
pub fn short_tokens(n: usize) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{}k", n / 1_000)
    } else {
        let whole = n / 1_000_000;
        let tenth = n % 1_000_000 / 100_000;
        if tenth == 0 {
            format!("{}M", whole)
        } else {
            format!("{}.{}M", whole, tenth)
        }
    }
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
    #[test]
    fn short_tokens_reads_at_a_glance() {
        assert_eq!(super::short_tokens(0), "0");
        assert_eq!(super::short_tokens(950), "950");
        assert_eq!(super::short_tokens(120_000), "120k");
        assert_eq!(super::short_tokens(128_000), "128k");
        assert_eq!(super::short_tokens(1_000_000), "1M");
        assert_eq!(super::short_tokens(1_250_000), "1.2M");
    }

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
    fn fit_path_keeps_the_file_name_and_drops_the_middle() {
        let long = "workspaces/engineering/semester_04/Software_Eng/Course_2/examinations/submission/Summary01.pdf";
        assert_eq!(fit_path("src/main.rs", 20), "src/main.rs");
        for width in [20, 40, 71] {
            let shown = fit_path(long, width);
            assert!(UnicodeWidthStr::width(shown.as_str()) <= width, "{width}: {shown}");
            assert!(shown.ends_with("submission/Summary01.pdf") || shown.ends_with("Summary01.pdf"), "{width}: {shown}");
            assert!(shown.contains('…'), "a cut is marked: {shown}");
        }
        assert_eq!(fit_path(long, 71), "…/Software_Eng/Course_2/examinations/submission/Summary01.pdf");
        assert_eq!(fit_path(long, 40), "…/examinations/submission/Summary01.pdf");
        assert_eq!(fit_path(long, 26), "…/submission/Summary01.pdf");
        assert_eq!(fit_path(long, 25), "…/Summary01.pdf");
        assert_eq!(fit_path("alpha/beta_long_dir/gamma/file.rs", 28), "alpha/…/gamma/file.rs", "room left over keeps the start");
        assert_eq!(fit_path("a/b/c/d/e/f/file.rs", 16), "…/d/e/f/file.rs");
        assert_eq!(fit_path("/usr/share/doc/pkg/readme", 18), "/…/doc/pkg/readme");
        assert_eq!(fit_path("~/workspaces/notes/tasks.md", 22), "~/…/notes/tasks.md");
    }

    #[test]
    fn fit_path_keeps_the_trailing_slash_of_a_folder() {
        assert_eq!(fit_path("one/two/three/four/", 14), "…/three/four/");
    }

    #[test]
    fn fit_path_cuts_a_name_longer_than_the_line_from_its_start() {
        let shown = fit_path("dir/an_extremely_long_report_name_final_v2.pdf", 16);
        assert_eq!(UnicodeWidthStr::width(shown.as_str()), 16, "{shown}");
        assert!(shown.starts_with('…') && shown.ends_with("final_v2.pdf"), "{shown}");
        assert_eq!(fit_path("anything", 0), "");
        assert!(UnicodeWidthStr::width(fit_path("日本語/日本語日本語.txt", 9).as_str()) <= 9);
    }

    #[test]
    fn wrap_path_breaks_after_a_slash_and_loses_nothing_when_it_fits() {
        let long = "workspaces/engineering/semester_04/Software_Eng/Course_2/examinations/submission/Summary01.pdf";
        assert_eq!(wrap_path("src/main.rs", 40, 2), ["src/main.rs"]);
        let lines = wrap_path(long, 60, 2);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| UnicodeWidthStr::width(l.as_str()) <= 60), "{lines:?}");
        assert!(lines[0].ends_with('/'), "the first line breaks after a folder: {lines:?}");
        assert_eq!(lines.concat(), long, "wrapping drops nothing while it fits in the rows");
        assert!(lines[1].ends_with("Summary01.pdf"));
        let tight = wrap_path(long, 30, 2);
        assert!(tight[1].starts_with('…') && tight[1].ends_with("Summary01.pdf"), "the last line gives way, not the file: {tight:?}");
        assert!(wrap_path("", 10, 2).is_empty() && wrap_path("abc", 0, 2).is_empty());
        let wide = wrap_path("日本語日本語日本語日本語", 10, 3);
        assert!(wide.iter().all(|l| UnicodeWidthStr::width(l.as_str()) <= 10), "{wide:?}");
    }

    #[test]
    fn path_fitting_holds_on_random_paths() {
        let cols = |s: &str| UnicodeWidthStr::width(s);
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let pieces = ["a", "bb", "long_segment", "x y", "日本", "é", "dir", "_", "-", "."];
        for _ in 0..5000 {
            let segments: Vec<String> = (0..1 + next(9)).map(|_| (0..1 + next(3)).map(|_| pieces[next(pieces.len())]).collect()).collect();
            let path = format!("{}{}{}", ["", "/", "~/"][next(3)], segments.join("/"), ["", "/"][next(2)]);
            let width = next(90);

            let shown = fit_path(&path, width);
            assert!(cols(&shown) <= width, "{path:?} in {width}: {shown:?}");
            if cols(&path) <= width {
                assert_eq!(shown, path);
            } else if width > 0 {
                let name = format!("{}{}", segments.last().unwrap(), if path.ends_with('/') { "/" } else { "" });
                assert!(shown.contains('…'), "a cut is marked: {path:?} in {width}: {shown:?}");
                if width >= cols(&name) + 2 {
                    assert!(shown.ends_with(&name), "the file name stays: {path:?} in {width}: {shown:?}");
                }
            }

            let rows = 1 + next(3);
            let lines = wrap_path(&path, width, rows);
            assert!(lines.len() <= rows && lines.iter().all(|l| cols(l) <= width), "{path:?} in {width}x{rows}: {lines:?}");
            if width >= 2 && !lines.iter().any(|l| l.contains('…')) {
                assert_eq!(lines.concat(), path, "nothing is lost unless a line says so");
            }
        }
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
