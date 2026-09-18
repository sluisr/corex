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
                    &code_buffer,
                    &code_block_lang,
                    theme,
                    effective_width,
                    &mut result_lines,
                );
            } else {
                in_code_block = true;
                code_buffer.clear();
                let lang = trimmed.trim_start_matches(['`', '~']).trim();
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
            if !result_lines.is_empty() && !result_lines.last().map(|l| l.width() == 0).unwrap_or(false) {
                result_lines.push(Line::from(""));
            }
            let mut spans = vec![
                Span::styled("◈ ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            ];
            spans.extend(parse_inline_spans(h1, theme, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
            result_lines.extend(wrap_spans_with_indent(spans, effective_width, "  ", "    "));
            continue;
        }
        if let Some(h2) = line.strip_prefix("## ") {
            if !result_lines.is_empty() && !result_lines.last().map(|l| l.width() == 0).unwrap_or(false) {
                result_lines.push(Line::from(""));
            }
            let mut spans = vec![
                Span::styled("◆ ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            ];
            spans.extend(parse_inline_spans(h2, theme, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
            result_lines.extend(wrap_spans_with_indent(spans, effective_width, "  ", "    "));
            continue;
        }
        if let Some(h3) = line.strip_prefix("### ") {
            if !result_lines.is_empty() && !result_lines.last().map(|l| l.width() == 0).unwrap_or(false) {
                result_lines.push(Line::from(""));
            }
            let mut spans = vec![
                Span::styled("▸ ", Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
            ];
            spans.extend(parse_inline_spans(h3, theme, Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)));
            result_lines.extend(wrap_spans_with_indent(spans, effective_width, "  ", "    "));
            continue;
        }

        // 4. Blockquotes
        if let Some(quote) = line.strip_prefix("> ") {
            let spans = parse_inline_spans(quote, theme, Style::default().fg(theme.gray).add_modifier(Modifier::ITALIC));
            result_lines.extend(wrap_spans_with_indent(spans, effective_width, "  ▎ ", "  ▎ "));
            continue;
        }

        // 5. Unordered List Items
        let bullet_prefixes = ["- ", "* ", "+ "];
        let mut is_bullet = false;
        for prefix in bullet_prefixes {
            let trimmed_lead = line.trim_start();
            if let Some(item_text) = trimmed_lead.strip_prefix(prefix) {
                let leading_spaces = line.len() - trimmed_lead.len();
                let base_indent = " ".repeat(leading_spaces);
                let mut spans = vec![
                    Span::styled("• ", Style::default().fg(theme.accent_cyan)),
                ];
                spans.extend(parse_inline_spans(item_text, theme, Style::default().fg(theme.foreground)));
                let first_pfx = format!("  {}", base_indent);
                let cont_pfx = format!("    {}", base_indent);
                result_lines.extend(wrap_spans_with_indent(spans, effective_width, &first_pfx, &cont_pfx));
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
                let leading_spaces = line.len() - trimmed_lead.len();
                let base_indent = " ".repeat(leading_spaces);
                let item_text = &trimmed_lead[dot_idx + 2..];
                let num_str = format!("{}. ", num);
                let num_len = num_str.chars().count();
                let mut spans = vec![
                    Span::styled(num_str, Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                ];
                spans.extend(parse_inline_spans(item_text, theme, Style::default().fg(theme.foreground)));
                let first_pfx = format!("  {}", base_indent);
                let cont_pfx = format!("  {}{}", base_indent, " ".repeat(num_len));
                result_lines.extend(wrap_spans_with_indent(spans, effective_width, &first_pfx, &cont_pfx));
                continue;
            }
        }

        // 7. Regular Paragraph Text
        if trimmed.is_empty() {
            result_lines.push(Line::from(""));
            continue;
        }
        let spans = parse_inline_spans(line, theme, Style::default().fg(theme.foreground));
        result_lines.extend(wrap_spans_with_indent(spans, effective_width, "  ", "  "));
    }

    // Flush any remaining table at the end of text
    flush_table(&mut table_buffer, &mut result_lines, theme, effective_width);

    // Flush an unclosed code block (missing closing fence)
    if in_code_block {
        flush_code_block(
            &code_buffer,
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
    buf: &[String],
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

/// Wraps a list of spans with distinct first-line and continuation indents (hanging indent),
/// ensuring wrapped bullet points, numbered lists, and quotes align cleanly.
pub fn wrap_spans_with_indent(
    spans: Vec<Span<'static>>,
    max_width: usize,
    first_line_indent: &str,
    continuation_indent: &str,
) -> Vec<Line<'static>> {
    if max_width < 10 {
        let mut line = Vec::new();
        if !first_line_indent.is_empty() {
            line.push(Span::raw(first_line_indent.to_string()));
        }
        line.extend(spans);
        return vec![Line::from(line)];
    }

    let mut lines = Vec::new();
    let mut current_line = Vec::new();
    let mut current_len = 0;

    let first_len = first_line_indent.chars().count();
    let cont_len = continuation_indent.chars().count();

    if first_len > 0 {
        current_line.push(Span::raw(first_line_indent.to_string()));
        current_len += first_len;
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

            let current_indent_len = if lines.is_empty() { first_len } else { cont_len };

            if current_len + added_len > max_width && current_len > current_indent_len {
                lines.push(Line::from(std::mem::take(&mut current_line)));
                current_len = 0;
                if cont_len > 0 {
                    current_line.push(Span::raw(continuation_indent.to_string()));
                    current_len += cont_len;
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

/// Wraps a list of spans so that no line exceeds `max_width` visible characters,
/// preserving indentation and span styling.
pub fn wrap_spans(
    spans: Vec<Span<'static>>,
    max_width: usize,
    indent: &str,
) -> Vec<Line<'static>> {
    wrap_spans_with_indent(spans, max_width, indent, indent)
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
            if used < max_width {
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
            for next_ch in chars.by_ref() {
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

    if lang == "diff" || lang == "patch" {
        if line.starts_with('+') && !line.starts_with("+++") {
            spans.push(Span::styled(line.to_string(), Style::default().fg(theme.diff_added_fg)));
            return spans;
        } else if line.starts_with('-') && !line.starts_with("---") {
            spans.push(Span::styled(line.to_string(), Style::default().fg(theme.diff_removed_fg)));
            return spans;
        } else if line.starts_with("@@") {
            spans.push(Span::styled(line.to_string(), Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)));
            return spans;
        } else if line.starts_with("---") || line.starts_with("+++") {
            spans.push(Span::styled(line.to_string(), Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)));
            return spans;
        }
    }

    // Simple fast syntax highlighting heuristics
    if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") || trimmed.starts_with("--") {
        spans.push(Span::styled(line.to_string(), Style::default().fg(theme.gray).add_modifier(Modifier::ITALIC)));
        return spans;
    }

    if (lang == "bash" || lang == "sh" || lang == "shell")
        && trimmed.starts_with('$') {
            spans.push(Span::styled("$ ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled(trimmed[1..].to_string(), Style::default().fg(Color::White)));
            return spans;
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

    // Proportionally shrink columns if total table width exceeds max_width
    let border_padding_width = 3 + 3 * num_cols;
    let avail_content_width = max_width.saturating_sub(border_padding_width).max(num_cols * 6);
    let mut total_col_width: usize = col_widths.iter().sum();

    if total_col_width > avail_content_width {
        let min_col_width = 8;
        // Iteratively shrink the widest columns until total fits within avail_content_width
        while total_col_width > avail_content_width {
            let max_w = *col_widths.iter().max().unwrap_or(&0);
            if max_w <= min_col_width {
                break;
            }

            // Find the second widest column width (or min_col_width)
            let second_max = col_widths
                .iter()
                .copied()
                .filter(|&w| w < max_w)
                .max()
                .unwrap_or(min_col_width)
                .max(min_col_width);

            // How many columns share the maximum width?
            let count_max = col_widths.iter().filter(|&&w| w == max_w).count();
            let excess = total_col_width - avail_content_width;

            let target_reduction_per_col = (max_w - second_max).max(1);
            let needed_reduction_per_col = (excess + count_max - 1) / count_max;
            let reduce_by = target_reduction_per_col.min(needed_reduction_per_col).max(1);

            let mut reduced_any = false;
            for w in col_widths.iter_mut() {
                if *w == max_w && *w > min_col_width {
                    let actual_dec = reduce_by.min(*w - min_col_width);
                    if actual_dec > 0 {
                        *w -= actual_dec;
                        total_col_width -= actual_dec;
                        reduced_any = true;
                    }
                }
            }

            if !reduced_any {
                break;
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
                let target_w = col_widths[i];
                let cell_spans = parse_inline_spans(cell, theme, base_cell_style);
                let visible_width: usize = cell_spans.iter().map(|s| s.width()).sum();

                let (rendered_spans, actual_w) = if visible_width > target_w {
                    let truncated = truncate_spans_to_width(&cell_spans, target_w);
                    let tr_w: usize = truncated.iter().map(|s| s.width()).sum();
                    (truncated, tr_w)
                } else {
                    (cell_spans, visible_width)
                };

                let pad = target_w.saturating_sub(actual_w);
                
                line_spans.push(Span::raw(" "));
                line_spans.extend(rendered_spans);
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

/// Converts a Ratatui Span with styling into a string with standard ANSI escape sequences.
pub fn span_to_ansi(span: &Span) -> String {
    let mut prefix = String::new();

    if span.style.add_modifier.contains(Modifier::BOLD) {
        prefix.push_str("\x1b[1m");
    }
    if span.style.add_modifier.contains(Modifier::DIM) {
        prefix.push_str("\x1b[2m");
    }
    if span.style.add_modifier.contains(Modifier::ITALIC) {
        prefix.push_str("\x1b[3m");
    }
    if span.style.add_modifier.contains(Modifier::UNDERLINED) {
        prefix.push_str("\x1b[4m");
    }

    if let Some(fg) = span.style.fg {
        match fg {
            Color::Reset => prefix.push_str("\x1b[39m"),
            Color::Black => prefix.push_str("\x1b[30m"),
            Color::Red => prefix.push_str("\x1b[31m"),
            Color::Green => prefix.push_str("\x1b[32m"),
            Color::Yellow => prefix.push_str("\x1b[33m"),
            Color::Blue => prefix.push_str("\x1b[34m"),
            Color::Magenta => prefix.push_str("\x1b[35m"),
            Color::Cyan => prefix.push_str("\x1b[36m"),
            Color::Gray => prefix.push_str("\x1b[37m"),
            Color::DarkGray => prefix.push_str("\x1b[90m"),
            Color::LightRed => prefix.push_str("\x1b[91m"),
            Color::LightGreen => prefix.push_str("\x1b[92m"),
            Color::LightYellow => prefix.push_str("\x1b[93m"),
            Color::LightBlue => prefix.push_str("\x1b[94m"),
            Color::LightMagenta => prefix.push_str("\x1b[95m"),
            Color::LightCyan => prefix.push_str("\x1b[96m"),
            Color::White => prefix.push_str("\x1b[97m"),
            Color::Rgb(r, g, b) => prefix.push_str(&format!("\x1b[38;2;{};{};{}m", r, g, b)),
            Color::Indexed(i) => prefix.push_str(&format!("\x1b[38;5;{}m", i)),
        }
    }

    if let Some(bg) = span.style.bg {
        match bg {
            Color::Reset => prefix.push_str("\x1b[49m"),
            Color::Black => prefix.push_str("\x1b[40m"),
            Color::Red => prefix.push_str("\x1b[41m"),
            Color::Green => prefix.push_str("\x1b[42m"),
            Color::Yellow => prefix.push_str("\x1b[43m"),
            Color::Blue => prefix.push_str("\x1b[44m"),
            Color::Magenta => prefix.push_str("\x1b[45m"),
            Color::Cyan => prefix.push_str("\x1b[46m"),
            Color::Gray => prefix.push_str("\x1b[47m"),
            Color::DarkGray => prefix.push_str("\x1b[100m"),
            Color::LightRed => prefix.push_str("\x1b[101m"),
            Color::LightGreen => prefix.push_str("\x1b[102m"),
            Color::LightYellow => prefix.push_str("\x1b[103m"),
            Color::LightBlue => prefix.push_str("\x1b[104m"),
            Color::LightMagenta => prefix.push_str("\x1b[105m"),
            Color::LightCyan => prefix.push_str("\x1b[106m"),
            Color::White => prefix.push_str("\x1b[107m"),
            Color::Rgb(r, g, b) => prefix.push_str(&format!("\x1b[48;2;{};{};{}m", r, g, b)),
            Color::Indexed(i) => prefix.push_str(&format!("\x1b[48;5;{}m", i)),
        }
    }

    if prefix.is_empty() {
        span.content.to_string()
    } else {
        format!("{}{}\x1b[0m", prefix, span.content)
    }
}

/// Converts a Ratatui Line into an ANSI-formatted string.
pub fn line_to_ansi(line: &Line) -> String {
    line.spans.iter().map(span_to_ansi).collect()
}

/// Renders markdown directly into ANSI-formatted text suitable for terminal stdout.
pub fn render_markdown_to_ansi(text: &str, theme: &Theme, max_width: usize) -> String {
    let lines = render_markdown(text, theme, max_width);
    lines.iter().map(line_to_ansi).collect::<Vec<_>>().join("\n")
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
    fn test_markdown_table_wide_middle_column() {
        let theme = Theme::default();
        let text = "| Vector | Prueba | Resultado |\n|---|---|---|\n| LFI sobre scripts ocultos | ?file/page/inc/… = php://filter/…resource=fun.php en los 7 send/*.php | ❌ ignorado |\n| Fuzz de campos POST | 7 endpoints × 35 nombres ( page,file,include,tpl,view,load,path,url,func,callback… ) con php://filter | ❌ 0 resultados |\n";
        let max_width = 100;
        let lines = render_markdown(text, &theme, max_width);

        // Verify table was rendered and no line exceeds max_width
        assert!(!lines.is_empty());
        for line in &lines {
            let w = line.width();
            assert!(w <= max_width, "table line width {} exceeds max_width {}", w, max_width);
            let s: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
            // Verify right border is intact
            assert!(s.ends_with('│') || s.ends_with('┤'), "table line missing right border: {}", s);
        }

        // Verify the Resultado column was NOT crushed to "Resul" or "❌ ig"
        let table_str: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(table_str.contains("Resultado"), "Resultado header was crushed: {}", table_str);
        assert!(table_str.contains("ignorado"), "ignorado was crushed: {}", table_str);
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

    #[test]
    fn test_hanging_indent_bullets_and_headers() {
        let theme = Theme::default();
        let text = "## Lo bueno ✅\n- CPU muy capaz: El procesador es eficiente y con buena potencia para su gama.\n";
        let lines = render_markdown(text, &theme, 40);

        // Header should not contain raw literal "## "
        let header_str: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!header_str.contains("##"), "header contains raw hashes: {}", header_str);
        assert!(header_str.contains("Lo bueno"), "header text missing: {}", header_str);

        // First bullet line starts with "  • " (4 visible chars)
        let bullet_line1: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(bullet_line1.starts_with("  • "), "bullet line 1 missing prefix: {}", bullet_line1);

        // Wrapped bullet line 2 MUST have hanging indent of 4 spaces ("    "), not 2 or 6
        if lines.len() > 2 {
            let bullet_line2: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(bullet_line2.starts_with("    "), "hanging indent missing: {}", bullet_line2);
            assert!(!bullet_line2.starts_with("     "), "over-indented: {}", bullet_line2);
        }
    }
}
