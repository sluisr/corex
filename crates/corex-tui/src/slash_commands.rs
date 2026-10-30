use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, BorderType};
use ratatui::Frame;

use crate::text::{fit_text, pad_right};
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct CommandItem {
    pub name: &'static str,
    /// Argument hint shown dimmed next to the name (empty when the command takes none).
    pub args: &'static str,
    /// One short sentence; syntax belongs in `args`.
    pub description: &'static str,
    /// Alternative names handled by the same command (matched while typing, never listed twice).
    pub aliases: &'static [&'static str],
    pub group: &'static str,
}

const SESSION: &str = "Session";
const MODEL: &str = "Model";
const CONTEXT: &str = "Context";
const TOOLS: &str = "Tools";
const APP: &str = "App";

/// Ordered by group: the order here is the order shown when the menu opens on a bare `/`.
pub const ALL_COMMANDS: &[CommandItem] = &[
    // --- Session ---
    CommandItem {
        name: "/chat",
        args: "list|save|resume|new",
        description: "Manage chat sessions",
        aliases: &[],
        group: SESSION,
    },
    CommandItem {
        name: "/resume",
        args: "[tag|id]",
        description: "Resume a previous session or checkpoint",
        aliases: &[],
        group: SESSION,
    },
    CommandItem {
        name: "/save",
        args: "<tag>",
        description: "Save a conversation checkpoint",
        aliases: &[],
        group: SESSION,
    },
    CommandItem {
        name: "/new",
        args: "",
        description: "Start a fresh chat session",
        aliases: &[],
        group: SESSION,
    },
    CommandItem {
        name: "/rewind",
        args: "",
        description: "Undo the last query and response",
        aliases: &[],
        group: SESSION,
    },
    CommandItem {
        name: "/clear",
        args: "",
        description: "Clear the conversation view",
        aliases: &[],
        group: SESSION,
    },
    // --- Model ---
    CommandItem {
        name: "/model",
        args: "[name]",
        description: "Pick provider, model and reasoning settings",
        aliases: &[],
        group: MODEL,
    },
    CommandItem {
        name: "/key",
        args: "[api_key]",
        description: "Set or replace the API key of the active provider",
        aliases: &["/auth"],
        group: MODEL,
    },
    CommandItem {
        name: "/local",
        args: "<prompt>|status",
        description: "Ask the local LLM directly ($0.00 cost)",
        aliases: &[],
        group: MODEL,
    },
    CommandItem {
        name: "/balance",
        args: "",
        description: "Show API account balance and token credits",
        aliases: &["/wallet"],
        group: MODEL,
    },
    CommandItem {
        name: "/stats",
        args: "",
        description: "Session token metrics, context in use and limits",
        aliases: &["/context"],
        group: MODEL,
    },
    // --- Context ---
    CommandItem {
        name: "/plan",
        args: "",
        description: "Toggle plan mode (read-only discovery)",
        aliases: &[],
        group: CONTEXT,
    },
    CommandItem {
        name: "/compact",
        args: "",
        description: "Compact history into a structured memory block",
        aliases: &["/compress"],
        group: CONTEXT,
    },
    CommandItem {
        name: "/prefix",
        args: "<text>",
        description: "Force an exact response prefix",
        aliases: &[],
        group: CONTEXT,
    },
    // --- Tools ---
    CommandItem {
        name: "/web",
        args: "<query>",
        description: "Search the live internet",
        aliases: &["/search"],
        group: TOOLS,
    },
    CommandItem {
        name: "/fetch",
        args: "<url>",
        description: "Fetch web page content as Markdown",
        aliases: &[],
        group: TOOLS,
    },
    CommandItem {
        name: "/fim",
        args: "<file>",
        description: "Fill-in-the-Middle code completion for a file",
        aliases: &[],
        group: TOOLS,
    },
    CommandItem {
        name: "/mcp",
        args: "[status|catalog|add|remove|reload]",
        description: "Manage MCP servers, discover catalog templates, or add tools",
        aliases: &[],
        group: TOOLS,
    },
    CommandItem {
        name: "/tasks",
        args: "[status|kill|send]",
        description: "List and manage background tasks",
        aliases: &[],
        group: TOOLS,
    },
    CommandItem {
        name: "/yolo",
        args: "[on|off]",
        description: "Toggle auto-approval of all tool executions",
        aliases: &[],
        group: TOOLS,
    },
    CommandItem {
        name: "/fleet",
        args: "[connect <node>|status]",
        description: "SysAdmin console: interactive live SSH session, server dashboard & AI supervisor",
        aliases: &["/sysadmin", "/vps"],
        group: TOOLS,
    },
    // --- App ---
    CommandItem {
        name: "/info",
        args: "",
        description: "Version, credits, links and session telemetry",
        aliases: &[],
        group: APP,
    },
    CommandItem {
        name: "/update",
        args: "[apply|force]",
        description: "Check or apply in-place Corex updates",
        aliases: &[],
        group: APP,
    },
    CommandItem {
        name: "/help",
        args: "",
        description: "Show commands and keyboard shortcuts",
        aliases: &[],
        group: APP,
    },
    CommandItem {
        name: "/mouse",
        args: "",
        description: "Toggle mouse reporting (wheel/drag vs native touch)",
        aliases: &[],
        group: APP,
    },
    CommandItem {
        name: "/keyboard",
        args: "",
        description: "Pause the mouse so the next tap raises the keyboard (Termux)",
        aliases: &[],
        group: APP,
    },
    CommandItem {
        name: "/quit",
        args: "",
        description: "Exit the Corex session",
        aliases: &[],
        group: APP,
    },
];

/// Commands matching what the user typed, best matches first.
///
/// Rank 0: name or alias starts with the query. Rank 1: name or alias contains it
/// (so `/sess` finds nothing noisy, but `/ume` finds `/resume`). A bare `/` returns everything
/// in group order. Input with arguments (a space) never matches, which closes the menu.
pub fn match_commands(input: &str) -> Vec<&'static CommandItem> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let query = input[1..].to_lowercase();
    if query.is_empty() {
        return ALL_COMMANDS.iter().collect();
    }

    let rank = |c: &CommandItem| -> Option<u8> {
        let names = std::iter::once(c.name).chain(c.aliases.iter().copied());
        let mut best: Option<u8> = None;
        for n in names {
            let n = n[1..].to_lowercase();
            let r = if n.starts_with(&query) {
                0
            } else if n.contains(&query) {
                1
            } else {
                continue;
            };
            best = Some(best.map_or(r, |b: u8| b.min(r)));
        }
        best
    };

    let mut ranked: Vec<(u8, usize, &'static CommandItem)> = ALL_COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(i, c)| rank(c).map(|r| (r, i, c)))
        .collect();
    ranked.sort_by_key(|(r, i, _)| (*r, *i));
    ranked.into_iter().map(|(_, _, c)| c).collect()
}

/// Splits `text` so the part matching `query` (case-insensitive) is highlighted.
fn highlighted(text: &str, query: &str, base: Style, hit: Style) -> Vec<Span<'static>> {
    if query.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let lower = text.to_lowercase();
    match lower.find(query) {
        Some(pos) if lower.len() == text.len() => {
            let end = pos + query.len();
            vec![
                Span::styled(text[..pos].to_string(), base),
                Span::styled(text[pos..end].to_string(), hit),
                Span::styled(text[end..].to_string(), base),
            ]
        }
        _ => vec![Span::styled(text.to_string(), base)],
    }
}

pub fn render_command_popup(
    frame: &mut Frame,
    input: &str,
    selected_idx: usize,
    area: Rect,
    theme: &Theme,
) {
    if !input.starts_with('/') || area.height < 3 {
        return;
    }

    let matching = match_commands(input);
    if matching.is_empty() {
        return;
    }

    let count = matching.len();
    
    // Clear the entire layout chunk so no background chat text is visible anywhere on the row
    frame.render_widget(Clear, area);

    let popup_width = 75.min(area.width);
    let popup_area = Rect::new(area.x, area.y, popup_width, area.height);

    let selected = selected_idx % count;
    let query = input[1..].to_lowercase();
    // Group headers only when browsing everything (bare `/`): filtered lists are flat and ranked.
    let show_groups = query.is_empty();

    let inner_w = popup_width.saturating_sub(2) as usize;
    let name_w = 11usize;
    let args_w = 20usize.min(inner_w / 3);
    // Two spaces between the argument hint and the description, one of air before the border.
    let desc_w = inner_w.saturating_sub(2 + name_w + args_w + 2 + 1);

    let mut lines: Vec<Line> = Vec::new();
    let mut selected_line = 0usize;
    let mut last_group = "";
    // Line index of every group header and of every command, to place the scroll window cleanly.
    let mut header_lines: Vec<usize> = Vec::new();
    let mut command_lines: Vec<usize> = Vec::new();
    for (i, cmd) in matching.iter().enumerate() {
        if show_groups && cmd.group != last_group {
            last_group = cmd.group;
            header_lines.push(lines.len());
            lines.push(Line::from(Span::styled(
                format!(" ── {} ", cmd.group),
                Style::default().fg(theme.dark_gray).add_modifier(Modifier::BOLD),
            )));
        }

        let is_selected = i == selected;
        if is_selected {
            selected_line = lines.len();
        }
        command_lines.push(lines.len());
        let prefix = if is_selected { "› " } else { "  " };

        let style = if is_selected {
            Style::default()
                .fg(theme.accent_blue)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        let hit = style.fg(theme.accent_yellow);

        // Show which alias matched, e.g. typing `/wallet` surfaces `/balance`.
        let alias_hit = cmd
            .aliases
            .iter()
            .find(|a| !query.is_empty() && a[1..].to_lowercase().contains(&query))
            .filter(|_| !cmd.name[1..].to_lowercase().contains(&query));

        let mut spans = vec![Span::styled(prefix, style)];
        let name_text = format!("{:<w$}", cmd.name, w = name_w);
        spans.extend(highlighted(&name_text, &format!("/{}", query), style, hit));
        spans.push(Span::styled(
            format!("{}  ", pad_right(cmd.args, args_w)),
            Style::default().fg(theme.dark_gray),
        ));
        let desc = match alias_hit {
            Some(a) => format!("{} (alias {})", cmd.description, a),
            None => cmd.description.to_string(),
        };
        spans.push(Span::styled(fit_text(&desc, desc_w), Style::default().fg(theme.gray)));
        lines.push(Line::from(spans));
    }

    let visible_items = (area.height.saturating_sub(2) as usize).max(1);
    let scroll_offset = if selected_line >= visible_items {
        selected_line + 1 - visible_items
    } else {
        0
    };

    // A group header on the last visible row would announce commands that are all below the
    // fold: leave that row empty instead (the header shows up together with its first command).
    let last_visible = scroll_offset + visible_items - 1;
    if header_lines.contains(&last_visible) && last_visible + 1 < lines.len() {
        lines.truncate(last_visible);
    }

    // What is out of sight, counted in commands: the list never says it goes on otherwise.
    let hidden_above = command_lines.iter().filter(|&&l| l < scroll_offset).count();
    let hidden_below = command_lines.iter().filter(|&&l| l > last_visible).count();

    let mut block = Block::default()
        .title(" Slash Commands ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent_blue));
    if hidden_above > 0 || hidden_below > 0 {
        let mut hint = format!(" {}/{}", selected + 1, count);
        if hidden_above > 0 {
            hint.push_str(&format!(" · ↑ {}", hidden_above));
        }
        if hidden_below > 0 {
            hint.push_str(&format!(" · ↓ {} more", hidden_below));
        }
        hint.push(' ');
        block = block.title_bottom(
            Line::from(Span::styled(hint, Style::default().fg(theme.dark_gray))).right_aligned(),
        );
    }

    let widget = Paragraph::new(lines)
        .block(block)
        .scroll((scroll_offset as u16, 0));
    frame.render_widget(widget, popup_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn names(input: &str) -> Vec<&'static str> {
        match_commands(input).iter().map(|c| c.name).collect()
    }

    #[test]
    fn bare_slash_lists_everything_in_group_order() {
        let all = names("/");
        assert_eq!(all.len(), ALL_COMMANDS.len());
        assert_eq!(all.first(), Some(&"/chat"));
        assert_eq!(all.last(), Some(&"/quit"));
    }

    #[test]
    fn matches_by_prefix_then_substring() {
        assert_eq!(names("/mod"), vec!["/model"]);
        // `/ume` is only a substring of `/resume`.
        assert_eq!(names("/ume"), vec!["/resume"]);
        // Prefix matches rank above substring matches (`/s` is a prefix of /save, /search-alias web...).
        let s = names("/s");
        assert!(s.iter().position(|n| *n == "/save").unwrap() < s.iter().position(|n| *n == "/clear").unwrap_or(usize::MAX));
    }

    #[test]
    fn aliases_resolve_to_their_command_and_never_list_twice() {
        assert_eq!(names("/compress"), vec!["/compact"]);
        assert_eq!(names("/wallet"), vec!["/balance"]);
        assert_eq!(names("/search"), vec!["/web"]);
        assert_eq!(names("/auth"), vec!["/key"]);
        assert!(!ALL_COMMANDS.iter().any(|c| c.name == "/compress"));
    }

    #[test]
    fn arguments_close_the_menu() {
        assert!(match_commands("/save tag").is_empty());
        assert!(match_commands("hello").is_empty());
        assert!(match_commands("/zzz").is_empty());
    }

    #[test]
    fn test_render_command_popup_guards() {
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();

        // 1. Empty input should not render anything
        terminal.draw(|f| {
            render_command_popup(f, "", 0, Rect::new(0, 0, 80, 5), &theme);
        }).unwrap();
        let buf = terminal.backend().buffer().clone();
        for cell in buf.content() {
            assert_eq!(cell.symbol(), " ", "Empty input should not render any characters");
        }

        // 2. Area height < 3 (e.g. 1 or 2) should not render anything even with slash input
        terminal.draw(|f| {
            render_command_popup(f, "/model", 0, Rect::new(0, 0, 80, 1), &theme);
        }).unwrap();
        let buf = terminal.backend().buffer().clone();
        for cell in buf.content() {
            assert_eq!(cell.symbol(), " ", "Height < 3 should not render any characters");
        }

        // 3. Area height >= 3 with "/model" renders the popup
        terminal.draw(|f| {
            render_command_popup(f, "/model", 0, Rect::new(0, 0, 80, 5), &theme);
        }).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Slash Commands"));
        assert!(text.contains("/model"));
    }

    #[test]
    fn bare_slash_renders_group_headers() {
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal.draw(|f| {
            render_command_popup(f, "/", 0, Rect::new(0, 0, 80, 10), &theme);
        }).unwrap();
        let text: String = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Session"));
        assert!(text.contains("/chat"));
    }

    fn popup_rows(input: &str, selected: usize, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(90, height)).unwrap();
        terminal
            .draw(|f| render_command_popup(f, input, selected, Rect::new(0, 0, 90, height), &Theme::default()))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    #[test]
    fn a_group_header_is_never_the_last_visible_row() {
        // Every window height, from the top of the list: no row may be a header whose commands
        // are all below the fold.
        for height in 5..=12u16 {
            let rows = popup_rows("/", 0, height);
            let body = &rows[1..rows.len() - 1];
            let last = body.last().unwrap();
            assert!(!last.contains("──") || last.trim_matches(|c| c == '│' || c == ' ').is_empty(), "height {height}: {last:?}");
        }
    }

    #[test]
    fn a_long_list_says_how_much_is_hidden_and_where_the_cursor_is() {
        let rows = popup_rows("/", 0, 10);
        let bottom = rows.last().unwrap();
        assert!(bottom.contains("1/") && bottom.contains("more"), "{bottom:?}");

        // Scrolled to the end nothing is left below, only what is above.
        let last = ALL_COMMANDS.len() - 1;
        let rows = popup_rows("/", last, 10);
        let bottom = rows.last().unwrap();
        assert!(bottom.contains('↑') && !bottom.contains("more"), "{bottom:?}");

        // A list that fits shows no counter at all.
        let rows = popup_rows("/mod", 0, 10);
        let bottom = rows.last().unwrap();
        assert!(!bottom.contains("more") && !bottom.contains('↑'), "{bottom:?}");
    }

    #[test]
    fn argument_hint_and_description_do_not_touch() {
        let rows = popup_rows("/chat", 0, 6);
        let row = rows.iter().find(|r| r.contains("/chat")).expect("row");
        let args_end = row.find("list|save|resume|new").unwrap() + "list|save|resume|new".len();
        assert!(row[args_end..].starts_with("  "), "{row:?}");
    }

    #[test]
    fn long_descriptions_leave_air_before_the_border() {
        // `/resume`'s description is the longest one: it gets cut, but not against the border.
        let rows = popup_rows("/resume", 0, 6);
        let row = rows.iter().find(|r| r.contains("/resume")).expect("row");
        let border = row.rfind('│').expect("right border");
        let before = row[..border].chars().last().unwrap();
        assert_eq!(before, ' ', "{row:?}");
    }
}
