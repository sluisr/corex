use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, BorderType};
use ratatui::Frame;

use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct CommandItem {
    pub name: &'static str,
    pub description: &'static str,
}

pub const ALL_COMMANDS: &[CommandItem] = &[
    CommandItem {
        name: "/chat",
        description: "Manage chat sessions: /chat list, /chat save <tag>, /chat resume <tag/id>, /chat new",
    },
    CommandItem {
        name: "/resume",
        description: "Resume a previous session or checkpoint: /resume [tag/id]",
    },
    CommandItem {
        name: "/save",
        description: "Save current conversation checkpoint: /save <tag>",
    },
    CommandItem {
        name: "/model",
        description: "Switch active model (DeepSeek-V4.1-Flash, DeepSeek-V4-Pro)",
    },
    CommandItem {
        name: "/balance",
        description: "Check current API account balance and token credits (/wallet)",
    },
    CommandItem {
        name: "/fim",
        description: "Fill-in-the-Middle code autocompletion for a file",
    },
    CommandItem {
        name: "/plan",
        description: "Toggle architectural plan mode (read-only discovery)",
    },
    CommandItem {
        name: "/stats",
        description: "View session token metrics and KV Cache discount ratio",
    },
    CommandItem {
        name: "/local",
        description: "Ask local LLM directly ($0.00 cost): /local <prompt> or /local status",
    },
    CommandItem {
        name: "/hybrid",
        description: "Toggle hybrid output compression mode: /hybrid on | off",
    },
    CommandItem {
        name: "/mcp",
        description: "List and manage Model Context Protocol servers: /mcp, /mcp status, /mcp reload",
    },
    CommandItem {
        name: "/tasks",
        description: "List and manage background tasks: /tasks, /tasks status <pid>, /tasks kill <pid>, /tasks send <pid> <input>",
    },
    CommandItem {
        name: "/yolo",
        description: "Toggle auto-approval of all tool executions: /yolo [on | off]",
    },
    CommandItem {
        name: "/rewind",
        description: "Rewind conversation history by 1 turn (undo last query and response)",
    },
    CommandItem {
        name: "/compact",
        description: "Intelligently compact conversation history into a structured memory block (/compress)",
    },
    CommandItem {
        name: "/compress",
        description: "Manually compress conversation history to save tokens and context",
    },
    CommandItem {
        name: "/prefix",
        description: "Set an exact response prefix to force model output formatting",
    },
    CommandItem {
        name: "/info",
        description: "Display Corex version, creator credits, official links, and session telemetry",
    },
    CommandItem {
        name: "/update",
        description: "Check for new Corex updates and display upgrade instructions",
    },
    CommandItem {
        name: "/clear",
        description: "Clear terminal conversation history",
    },
    CommandItem {
        name: "/help",
        description: "Show list of keyboard shortcuts and commands",
    },
    CommandItem {
        name: "/quit",
        description: "Exit the Corex session",
    },
];

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

    let filter = input.to_lowercase();
    let matching: Vec<&CommandItem> = ALL_COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(&filter))
        .collect();

    if matching.is_empty() {
        return;
    }

    let count = matching.len();
    
    // Clear the entire layout chunk so no background chat text is visible anywhere on the row
    frame.render_widget(Clear, area);

    let popup_width = 75.min(area.width);
    let popup_area = Rect::new(area.x, area.y, popup_width, area.height);

    let visible_items = (area.height.saturating_sub(2) as usize).max(1);
    let selected = selected_idx % count;
    let scroll_offset = if selected >= visible_items {
        (selected + 1 - visible_items) as u16
    } else {
        0
    };

    let mut lines = Vec::new();
    for (i, cmd) in matching.iter().enumerate() {
        let is_selected = i == selected;
        let prefix = if is_selected { "❯ " } else { "  " };

        let style = if is_selected {
            Style::default()
                .fg(theme.accent_blue)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(format!("{:<12} ", cmd.name), style),
            Span::styled(cmd.description, Style::default().fg(theme.gray)),
        ]));
    }

    let block = Block::default()
        .title(" Slash Commands ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent_blue));

    let widget = Paragraph::new(lines)
        .block(block)
        .scroll((scroll_offset, 0));
    frame.render_widget(widget, popup_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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
}

