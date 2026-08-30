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
        description: "Switch active model (DeepSeek-V4-Flash, DeepSeek-V4-Pro)",
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
        description: "List and manage Model Context Protocol servers",
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
        description: "Exit the UTI session",
    },
];

pub fn render_command_popup(
    frame: &mut Frame,
    input: &str,
    selected_idx: usize,
    area: Rect,
    theme: &Theme,
) {
    let filter = if input.starts_with('/') {
        input.to_lowercase()
    } else {
        "/".to_string()
    };
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

    let mut lines = Vec::new();
    for (i, cmd) in matching.iter().enumerate() {
        let is_selected = i == (selected_idx % count);
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
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_blue));

    let widget = Paragraph::new(lines).block(block);
    frame.render_widget(widget, popup_area);
}
