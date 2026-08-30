use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Converts a markdown string into beautifully styled, wrapped Ratatui Lines with support for:
/// - Bold: `**text**` or `__text__`
/// - Italic: `*text*` or `_text_`
/// - Inline Code: `` `code` ``
/// - Headers: `# `, `## `, `### `, `#### `
/// - Bullet Lists: `- `, `* `, `+ ` -> `  • `
/// - Ordered Lists: `1. `, `2. ` -> `  1. `
/// - Code Blocks: ```` ```lang ... ``` ```` with borders and syntax coloring
/// - Blockquotes: `> ` -> `  ▎ `
/// - Horizontal Rules: `---`
pub fn render_markdown(text: &str, theme: &Theme, max_width: usize) -> Vec<Line<'static>> {
    let mut result_lines = Vec::new();
    let mut in_code_block = false;
    let mut code_block_lang = String::new();
    let mut code_buffer: Vec<String> = Vec::new();
    let effective_width = max_width.max(20);
    let mut table_buffer: Vec<String> = Vec::new();

    let flush_table = |table_buf: &mut Vec<String>, res_lines: &mut Vec<Line<'static>>, theme: &Theme, w: usize| {
        if !table_buf.is_empty() {
            res_lines.extend(render_table(table_buf, theme, w));
            table_buf.clear();
        }
    };

    for line in text.lines() {
        let trimmed = line.trim();

        // 1. Code Fence Start / End
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush_table(&mut table_buffer, &mut result_lines, theme, effective_width);
            if in_code_block {
                in_code_block = false;
                flush_code_block(
                    &mut code_buffer,
                    &code_block_lang,
                    theme,
                    effective_width,
                    &mut result_lines,
                );
            } else {
                in_code_block = true;
                code_buffer.clear();
                let lang = trimmed.trim_start_matches(|c| c == '`' || c == '~').trim();
                code_block_lang = lang.to_string();
            }
            continue;
        }

        // Inside code block
        if in_code_block {
            code_buffer.push(line.to_string());
            continue;
        }

        // Table Row Buffering
        let is_table_row = trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() > 1;
        if is_table_row {
            table_buffer.push(line.to_string());
            continue;
        } else {
            flush_table(&mut table_buffer, &mut result_lines, theme, effective_width);
        }

        // 2. Horizontal Rule
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            result_lines.push(Line::from(Span::styled(
                "  ────────────────────────────────────────────────────",
                Style::default().fg(theme.dark_gray),
            )));
            continue;
        }

        // 3. Headers
        if let Some(h1) = line.strip_prefix("# ") {
            let mut spans = vec![
                Span::styled("  # ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            ];
            spans.extend(parse_inline_spans(h1, theme, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
            result_lines.extend(wrap_spans(spans, effective_width, "  "));
            continue;
        }
        if let Some(h2) = line.strip_prefix("## ") {
            let mut spans = vec![
                Span::styled("  ## ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            ];
            spans.extend(parse_inline_spans(h2, theme, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
            result_lines.extend(wrap_spans(spans, effective_width, "  "));
            continue;
        }
        if let Some(h3) = line.strip_prefix("### ") {
            let mut spans = vec![
                Span::styled("  ### ", Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
            ];
            spans.extend(parse_inline_spans(h3, theme, Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)));
            result_lines.extend(wrap_spans(spans, effective_width, "  "));
            continue;
        }

        // 4. Blockquotes
        if let Some(quote) = line.strip_prefix("> ") {
            let mut spans = vec![
                Span::styled("  ▎ ", Style::default().fg(theme.accent_cyan)),
            ];
            spans.extend(parse_inline_spans(quote, theme, Style::default().fg(theme.gray).add_modifier(Modifier::ITALIC)));
            result_lines.extend(wrap_spans(spans, effective_width, "  ▎ "));
            continue;
        }

        // 5. Unordered List Items
        let bullet_prefixes = ["- ", "* ", "+ "];
        let mut is_bullet = false;
        for prefix in bullet_prefixes {
            let leading_spaces = line.len() - line.trim_start().len();
            let indent = " ".repeat(leading_spaces);
            let trimmed_lead = line.trim_start();
            if let Some(item_text) = trimmed_lead.strip_prefix(prefix) {
                let mut spans = vec![
                    Span::raw(format!("  {}• ", indent)),
                ];
                spans.extend(parse_inline_spans(item_text, theme, Style::default().fg(theme.foreground)));
                result_lines.extend(wrap_spans(spans, effective_width, "    "));
                is_bullet = true;
                break;
            }
        }
        if is_bullet {
            continue;
        }

        // 6. Ordered List Items (e.g. "1. ", "2. ")
        let trimmed_lead = line.trim_start();
        if let Some(dot_idx) = trimmed_lead.find(". ") {
            if let Ok(num) = trimmed_lead[..dot_idx].parse::<usize>() {
                let leading_spaces = line.len() - line.trim_start().len();
                let indent = " ".repeat(leading_spaces);
                let item_text = &trimmed_lead[dot_idx + 2..];
                let mut spans = vec![
                    Span::styled(format!("  {}{}. ", indent, num), Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                ];
                spans.extend(parse_inline_spans(item_text, theme, Style::default().fg(theme.foreground)));
                result_lines.extend(wrap_spans(spans, effective_width, "    "));
                continue;
            }
        }

        // 7. Regular Paragraph Text
        let mut spans = Vec::new();
        spans.extend(parse_inline_spans(line, theme, Style::default().fg(theme.foreground)));
        result_lines.extend(wrap_spans(spans, effective_width, "  "));
    }

    // Flush any remaining table at the end of text
    flush_table(&mut table_buffer, &mut result_lines, theme, effective_width);

    // Flush an unclosed code block (missing closing fence)
    if in_code_block {
        flush_code_block(
            &mut code_buffer,
            &code_block_lang,
            theme,
            effective_width,
            &mut result_lines,
        );
    }

    result_lines
}

/// Renders a buffered code block as a closed box that sizes itself to the
/// widest content line (capped at `max_width`), so short snippets produce
/// compact frames instead of stretching across the whole terminal.
fn flush_code_block(
    buf: &mut Vec<String>,
    lang: &str,
    theme: &Theme,
    max_width: usize,
    result: &mut Vec<Line<'static>>,
) {
    if buf.is_empty() {
        return;
    }

    let title = if lang.is_empty() {
        "code".to_string()
    } else {
        format!(" {} ", lang)
    };
    let title_w = title.chars().count();
    let content_w = buf
        .iter()
        .map(|l| unicode_width::UnicodeWidthStr::width(l.as_str()))
        .max()
        .unwrap_or(0);

    // Box width = widest content + "  │ " / " │" margins, at least as wide as
    // the title frame, capped by the available terminal width.
    let box_w = (content_w + 6).max(title_w + 5).min(max_width).max(6);

    // Top border
    let fill = box_w.saturating_sub(5).saturating_sub(title_w);
    let top = format!("  ╭─{}{}╮", title, "─".repeat(fill));
    result.push(Line::from(Span::styled(
        top,
        Style::default().fg(theme.dark_gray),
    )));

    // Code lines (each closes with a right border)
    let inner = box_w.saturating_sub(6);
    for line in buf.iter() {
        let mut spans = vec![
            Span::styled("  │ ", Style::default().fg(theme.dark_gray)),
        ];
        let code_spans = colorize_code_line(line, lang, theme);
        let code_width: usize = code_spans.iter().map(|s| s.width()).sum();
        let (code_spans, code_w) = if code_width > inner {
            let truncated = truncate_spans_to_width(&code_spans, inner);
            let w = truncated.iter().map(|s| s.width()).sum();
            (truncated, w)
        } else {
            (code_spans, code_width)
        };
        spans.extend(code_spans);
        let pad = inner.saturating_sub(code_w);
        spans.push(Span::styled(
            format!("{} │", " ".repeat(pad)),
            Style::default().fg(theme.dark_gray),
        ));
        result.push(Line::from(spans));
    }

    // Bottom border
    let bottom = format!("  ╰{}╯", "─".repeat(box_w.saturating_sub(4)));
    result.push(Line::from(Span::styled(
        bottom,
        Style::default().fg(theme.dark_gray),
    )));
}

/// Wraps a list of spans so that no line exceeds `max_width` visible characters,
/// preserving indentation and span styling.
pub fn wrap_spans(
    spans: Vec<Span<'static>>,
    max_width: usize,
    indent: &str,
) -> Vec<Line<'static>> {
    if max_width < 10 {
        return vec![Line::from(spans)];
    }

    let mut lines = Vec::new();
    let mut current_line = Vec::new();
    let mut current_len = 0;

    let indent_len = indent.chars().count();
    if indent_len > 0 {
        current_line.push(Span::raw(indent.to_string()));
        current_len += indent_len;
    }

    for span in spans {
        let style = span.style;
        let text = span.content;

        let words: Vec<&str> = text.split(' ').collect();
        for (i, word) in words.iter().enumerate() {
            let is_first = i == 0;
            let word_len = word.chars().count();
            let need_space = !is_first;
            let added_len = word_len + if need_space { 1 } else { 0 };

            if current_len + added_len > max_width && current_len > indent_len {
                lines.push(Line::from(std::mem::take(&mut current_line)));
                current_len = 0;
                if indent_len > 0 {
                    current_line.push(Span::raw(indent.to_string()));
                    current_len += indent_len;
                }
                if !word.is_empty() {
                    current_line.push(Span::styled(word.to_string(), style));
                    current_len += word_len;
                }
            } else {
                if need_space {
                    current_line.push(Span::styled(" ", style));
                    current_len += 1;
                }
                if !word.is_empty() {
                    current_line.push(Span::styled(word.to_string(), style));
                    current_len += word_len;
                }
            }
        }
    }

    if !current_line.is_empty() {
        lines.push(Line::from(current_line));
    }

    if lines.is_empty() {
        lines.push(Line::from(""));
    }

    lines
}

/// Truncates a slice of styled spans so the total visible width is at most
/// `max_width`, appending an ellipsis when content had to be cut.
fn truncate_spans_to_width(spans: &[Span<'static>], max_width: usize) -> Vec<Span<'static>> {
    let mut result = Vec::new();
    let mut used = 0usize;
    for span in spans {
        if used >= max_width {
            break;
        }
        let span_width = span.width();
        if used + span_width <= max_width {
            result.push(span.clone());
            used += span_width;
        } else {
            let remaining = max_width - used;
            let mut cut = String::new();
            let mut cut_w = 0usize;
            for c in span.content.chars() {
                let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if cut_w + w > remaining.saturating_sub(1) {
                    break;
                }
                cut.push(c);
                cut_w += w;
            }
            result.push(Span::styled(cut, span.style));
            used += cut_w;
            if used + 1 <= max_width {
                result.push(Span::styled("…".to_string(), span.style));
            }
            break;
        }
    }
    result
}

/// Parses inline markdown tokens: `**bold**`, `*italic*`, `` `code` `` into Spans.
fn parse_inline_spans(
    input: &str,
    theme: &Theme,
    base_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut chars = input.chars().peekable();
    let mut buffer = String::new();

    while let Some(ch) = chars.next() {
        // 1. Inline Code: `code`
        if ch == '`' {
            if !buffer.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buffer), base_style));
            }
            let mut code = String::new();
            while let Some(&next_ch) = chars.peek() {
                if next_ch == '`' {
                    chars.next();
                    break;
                }
                code.push(chars.next().unwrap());
            }
            spans.push(Span::styled(
                format!(" {} ", code),
                Style::default().fg(theme.accent_cyan).bg(Color::Rgb(30, 35, 45)),
            ));
            continue;
        }

        // 2. Bold: **text** or __text__
        if (ch == '*' && chars.peek() == Some(&'*')) || (ch == '_' && chars.peek() == Some(&'_')) {
            chars.next(); // consume second delimiter
            if !buffer.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buffer), base_style));
            }
            let mut bold_text = String::new();
            let delim = ch;
            while let Some(next_ch) = chars.next() {
                if next_ch == delim && chars.peek() == Some(&delim) {
                    chars.next(); // consume second delim
                    break;
                }
                bold_text.push(next_ch);
            }
            spans.push(Span::styled(
                bold_text,
                base_style
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));
            continue;
        }

        // 3. Italic: *text* or _text_
        if (ch == '*' || ch == '_') && chars.peek() != Some(&' ') {
            if !buffer.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buffer), base_style));
            }
            let mut italic_text = String::new();
            let delim = ch;
            while let Some(next_ch) = chars.next() {
                if next_ch == delim {
                    break;
                }
                italic_text.push(next_ch);
            }
            spans.push(Span::styled(
                italic_text,
                base_style.add_modifier(Modifier::ITALIC),
            ));
            continue;
        }

        buffer.push(ch);
    }

    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, base_style));
    }

    spans
}

fn colorize_code_line(line: &str, lang: &str, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let trimmed = line.trim_start();

    // Simple fast syntax highlighting heuristics
    if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") || trimmed.starts_with("--") {
        spans.push(Span::styled(line.to_string(), Style::default().fg(theme.gray).add_modifier(Modifier::ITALIC)));
        return spans;
    }

    if lang == "bash" || lang == "sh" || lang == "shell" {
        if trimmed.starts_with('$') {
            spans.push(Span::styled("$ ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled(trimmed[1..].to_string(), Style::default().fg(Color::White)));
            return spans;
        }
    }

    spans.push(Span::styled(line.to_string(), Style::default().fg(Color::White)));
    spans
}

fn render_table(rows: &[String], theme: &Theme, max_width: usize) -> Vec<Line<'static>> {
    let mut grid: Vec<Vec<String>> = Vec::new();
    for row in rows {
        let trimmed = row.trim();
        let mut parts: Vec<String> = trimmed
            .split('|')
            .map(|s| s.trim().to_string())
            .collect();
        if parts.first().map(|s| s.is_empty()).unwrap_or(false) {
            parts.remove(0);
        }
        if parts.last().map(|s| s.is_empty()).unwrap_or(false) {
            parts.pop();
        }
        grid.push(parts);
    }

    if grid.is_empty() {
        return Vec::new();
    }

    let num_cols = grid.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 {
        return Vec::new();
    }

    let mut col_widths = vec![0; num_cols];
    for row in &grid {
        let is_separator = !row.is_empty()
            && row.iter().any(|cell| cell.contains('-'))
            && row.iter().all(|cell| cell.chars().all(|c| c == '-' || c == ':' || c == ' '));
        if is_separator {
            continue;
        }
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                let cell_spans = parse_inline_spans(cell, theme, Style::default());
                let cell_width: usize = cell_spans.iter().map(|s| s.width()).sum();
                col_widths[i] = col_widths[i].max(cell_width);
            }
        }
    }

    // Shrink the last column if the total table width exceeds max_width
    let border_padding_width = 3 + 3 * num_cols;
    let total_col_width: usize = col_widths.iter().sum();
    let total_table_width = total_col_width + border_padding_width;

    if total_table_width > max_width && num_cols > 1 {
        let last_col_idx = num_cols - 1;
        let other_cols_width: usize = col_widths.iter().take(last_col_idx).sum();
        let avail_last_col_width = max_width
            .saturating_sub(border_padding_width)
            .saturating_sub(other_cols_width)
            .max(10); // keep at least 10 chars for description

        if col_widths[last_col_idx] > avail_last_col_width {
            col_widths[last_col_idx] = avail_last_col_width;
            
            // Truncate cell values in the grid for the last column
            for row in &mut grid {
                let is_separator = row.iter().all(|cell| cell.chars().all(|c| c == '-' || c == ':' || c == ' '));
                if is_separator {
                    continue;
                }
                if let Some(cell) = row.get_mut(last_col_idx) {
                    let cell_width = unicode_width::UnicodeWidthStr::width(cell.as_str());
                    if cell_width > avail_last_col_width {
                        let mut truncated = String::new();
                        let mut current_width = 0;
                        let target_width = avail_last_col_width.saturating_sub(3);
                        for c in cell.chars() {
                            let char_w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                            if current_width + char_w > target_width {
                                break;
                            }
                            truncated.push(c);
                            current_width += char_w;
                        }
                        truncated.push_str("...");
                        *cell = truncated;
                    }
                }
            }
        }
    }

    let mut result_lines = Vec::new();
    let mut is_header_row = true;
    for row in grid {
        let is_separator = !row.is_empty()
            && row.iter().any(|cell| cell.contains('-'))
            && row.iter().all(|cell| cell.chars().all(|c| c == '-' || c == ':' || c == ' '));
        
        if is_separator {
            is_header_row = false;
            let mut sep_line = String::new();
            for &w in &col_widths {
                sep_line.push_str(&"─".repeat(w + 2));
                sep_line.push('┼');
            }
            if !sep_line.is_empty() {
                sep_line.pop();
            }
            result_lines.push(Line::from(vec![
                Span::styled("  ├", Style::default().fg(theme.dark_gray)),
                Span::styled(sep_line, Style::default().fg(theme.dark_gray)),
                Span::styled("┤", Style::default().fg(theme.dark_gray)),
            ]));
        } else {
            let mut line_spans = vec![Span::styled("  │", Style::default().fg(theme.dark_gray))];
            let base_cell_style = if is_header_row {
                Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };

            for (i, cell) in row.iter().enumerate() {
                if i >= num_cols {
                    break;
                }
                // Parse inline markdown (bold, code, italic) within each table cell
                let cell_spans = parse_inline_spans(cell, theme, base_cell_style);
                let visible_width: usize = cell_spans.iter().map(|s| s.width()).sum();
                let pad = col_widths[i].saturating_sub(visible_width);
                
                line_spans.push(Span::raw(" "));
                line_spans.extend(cell_spans);
                if pad > 0 {
                    line_spans.push(Span::raw(" ".repeat(pad)));
                }
                line_spans.push(Span::raw(" "));
                line_spans.push(Span::styled("│", Style::default().fg(theme.dark_gray)));
            }
            result_lines.push(Line::from(line_spans));
        }
    }

    result_lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_table() {
        let text = "## 📊 Desglose de los 53 GB\n\n| Directorio | Tamaño | Qué es |\n|---|---|---|\n| `tmux-server-26992.log` | **12 GB** | ⚠️ Log verbose de tmux (creciendo) |\n| `Projects/` | 13 GB | triade-app (7.9G: node_modules 3.6G + build android 4G), uti-cli/target 2.5G |\n";
        let theme = Theme::default();
        let lines = render_markdown(text, &theme, 100);
        for (i, line) in lines.iter().enumerate() {
            let s: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
            println!("Line {}: {}", i, s);
        }
    }

    #[test]
    fn test_markdown_code_block_borders() {
        let theme = Theme::default();
        let text = "```rust\nlet first_line = content.lines().next().unwrap_or(\"Done\").replace('\\t', \"    \");\nlet short = 1;\n```\n";
        let max_width: usize = 60;
        let lines = render_markdown(text, &theme, max_width);
        assert!(lines.len() >= 4, "expected >= 4 lines, got {}", lines.len());

        let top: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let bottom: String = lines[3].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(top.starts_with("  ╭─"), "top border malformed: {}", top);
        assert!(top.ends_with('╮'), "top border does not close: {}", top);
        assert!(bottom.starts_with("  ╰"), "bottom border malformed: {}", bottom);
        assert!(bottom.ends_with('╯'), "bottom border does not close: {}", bottom);

        // Every border and code line must span exactly the effective width.
        for (i, line) in lines.iter().enumerate() {
            let w = line.width();
            assert_eq!(w, max_width, "line {} has width {}, expected {}", i, w, max_width);
        }

        // Code lines must always close with a right border.
        for line in &lines[1..3] {
            let s: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
            assert!(s.ends_with("│"), "code line lacks right border: {}", s);
        }
    }

    #[test]
    fn test_markdown_code_block_autosizes() {
        let theme = Theme::default();
        let text = "```bash\n~/.cargo/bin/uti\n```\n";
        let max_width: usize = 120;
        let lines = render_markdown(text, &theme, max_width);

        assert_eq!(lines.len(), 3, "expected top + content + bottom");
        let top: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let content: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        let bottom: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();

        // The box must be compact (much narrower than max_width) and closed.
        let w = lines[0].width();
        assert!(w < max_width, "box {} should be narrower than {} (autosize)", w, max_width);
        assert!(top.ends_with('╮'), "top border does not close: {}", top);
        assert!(bottom.ends_with('╯'), "bottom border does not close: {}", bottom);
        assert!(content.ends_with("│"), "content line lacks right border: {}", content);
        // Top, content and bottom must all have the same width.
        for line in &lines {
            assert_eq!(line.width(), w, "mismatched box width: {:?}", line.spans);
        }
    }
}
