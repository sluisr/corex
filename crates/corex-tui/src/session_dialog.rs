use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use corex_core::session::{Session, SessionSummary};

use crate::overlay::begin_modal;
use crate::theme::Theme;

#[derive(Debug, Default)]
pub struct SessionDialogState {
    pub is_open: bool,
    pub selected_idx: usize,
    pub sessions: Vec<SessionSummary>,
    pub search_query: String,
    pub cached_preview_id: Option<String>,
    pub cached_preview_session: Option<Session>,
}

impl SessionDialogState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, workspace_dir: &str) {
        self.sessions = Session::list_all(Some(workspace_dir));
        self.selected_idx = 0;
        self.search_query.clear();
        self.cached_preview_id = None;
        self.cached_preview_session = None;
        self.is_open = true;
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.cached_preview_session = None;
    }

    pub fn filtered_sessions(&self) -> Vec<&SessionSummary> {
        if self.search_query.is_empty() {
            return self.sessions.iter().collect();
        }
        let q = self.search_query.to_lowercase();
        self.sessions
            .iter()
            .filter(|s| {
                s.title.to_lowercase().contains(&q)
                    || s.tag.as_ref().map(|t| t.to_lowercase().contains(&q)).unwrap_or(false)
                    || s.model.to_lowercase().contains(&q)
                    || s.id.to_lowercase().starts_with(&q)
            })
            .collect()
    }

    pub fn selected_session(&self) -> Option<&SessionSummary> {
        let list = self.filtered_sessions();
        if list.is_empty() {
            None
        } else {
            let idx = self.selected_idx.min(list.len() - 1);
            Some(list[idx])
        }
    }

    pub fn update_preview(&mut self) {
        let sel_id = self.selected_session().map(|s| s.id.clone());
        if let Some(id) = sel_id {
            if self.cached_preview_id.as_deref() != Some(&id) {
                self.cached_preview_id = Some(id.clone());
                self.cached_preview_session = Session::load_by_id_or_tag(&id).ok();
            }
        } else {
            self.cached_preview_id = None;
            self.cached_preview_session = None;
        }
    }
}

pub fn render_session_dialog(
    frame: &mut Frame,
    area: Rect,
    state: &mut SessionDialogState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    // Actualizamos la preview del elemento seleccionado si ha cambiado
    state.update_preview();

    let dialog_width = 104.min(area.width.saturating_sub(4)).max(56);
    let dialog_height = (area.height * 4 / 5)
        .clamp(16, 26)
        .min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Fondo atenuado suave + respiración perimetral
    begin_modal(frame, area, dialog_area, 2, 1);

    let scroll_hint = if !state.sessions.is_empty() {
        format!(" ({} saved) ", state.sessions.len())
    } else {
        String::new()
    };
    let title = format!("  Session Explorer{} ", scroll_hint);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent_blue));

    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    // Layout vertical: Search bar (1) + Divider (1) + Main Split (Min) + Divider (1) + Footer (1)
    let vert_chunks = Layout::vertical([
        Constraint::Length(1), // Search input
        Constraint::Length(1), // Divider
        Constraint::Min(8),    // Split (List vs Preview)
        Constraint::Length(1), // Divider
        Constraint::Length(1), // Footer keys
    ])
    .split(inner);

    // 1. Search Bar Header
    let search_label_style = Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD);
    let mut search_spans = vec![
        Span::raw(" "),
        Span::styled("Filter: ", search_label_style),
        Span::styled(
            if state.search_query.is_empty() {
                "type to search by topic, #tag or model...".to_string()
            } else {
                state.search_query.clone()
            },
            if state.search_query.is_empty() {
                Style::default().fg(theme.dark_gray)
            } else {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            },
        ),
    ];
    if !state.search_query.is_empty() {
        search_spans.push(Span::styled(" _", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::SLOW_BLINK)));
    }
    frame.render_widget(Paragraph::new(Line::from(search_spans)), vert_chunks[0]);

    // Divider 1
    let div_style = Style::default().fg(Color::Rgb(35, 45, 60));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled("─".repeat(inner.width as usize), div_style))),
        vert_chunks[1],
    );

    // 2. Main Content Split
    let filtered = state.filtered_sessions();
    if filtered.is_empty() {
        let empty_msg = if state.sessions.is_empty() {
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("No conversation sessions found for this workspace.", Style::default().fg(theme.gray)),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("Sessions are automatically saved as you chat or via ", Style::default().fg(theme.dark_gray)),
                    Span::styled("/save <tag>", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                    Span::styled(".", Style::default().fg(theme.dark_gray)),
                ]),
            ]
        } else {
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("No sessions matching: ", Style::default().fg(theme.gray)),
                    Span::styled(&state.search_query, Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("Press Backspace to clear filter.", Style::default().fg(theme.dark_gray)),
                ]),
            ]
        };
        frame.render_widget(Paragraph::new(empty_msg), vert_chunks[2]);
    } else {
        // En pantallas suficientemente anchas hacemos Split horizontal (Lista | Preview)
        let split_h = Layout::horizontal([
            Constraint::Percentage(58),
            Constraint::Length(1), // Separador vertical
            Constraint::Percentage(42),
        ])
        .split(vert_chunks[2]);

        render_session_list(frame, split_h[0], &filtered, state.selected_idx, theme);
        
        let v_sep: Vec<Line> = (0..split_h[1].height)
            .map(|_| Line::from(Span::styled("│", div_style)))
            .collect();
        frame.render_widget(Paragraph::new(v_sep), split_h[1]);

        render_session_preview(frame, split_h[2], state.cached_preview_session.as_ref(), theme);
    }

    // Divider 2
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled("─".repeat(inner.width as usize), div_style))),
        vert_chunks[3],
    );

    // 3. Contextual Footer
    let key_badge = |key: &'static str, desc: &'static str, color: Color| -> Vec<Span<'static>> {
        vec![
            Span::styled("[", Style::default().fg(Color::Rgb(70, 85, 110))),
            Span::styled(key, Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::styled("] ", Style::default().fg(Color::Rgb(70, 85, 110))),
            Span::styled(desc, Style::default().fg(theme.gray)),
            Span::raw("   "),
        ]
    };

    let mut footer = vec![Span::raw("  ")];
    footer.extend(key_badge("Enter", "Resume", theme.accent_blue));
    footer.extend(key_badge("x", "Delete", theme.accent_red));
    footer.extend(key_badge("↑/↓", "Navigate", theme.accent_cyan));
    if !state.search_query.is_empty() {
        footer.extend(key_badge("Esc", "Clear Filter", theme.accent_yellow));
    } else {
        footer.extend(key_badge("Esc", "Close", theme.gray));
    }
    frame.render_widget(Paragraph::new(Line::from(footer)), vert_chunks[4]);
}

fn render_session_list(
    frame: &mut Frame,
    area: Rect,
    sessions: &[&SessionSummary],
    selected_idx: usize,
    theme: &Theme,
) {
    let selected_bg = Color::Rgb(26, 38, 58);
    let inner_w = area.width as usize;
    let max_visible = area.height as usize;

    let selected = selected_idx.min(sessions.len().saturating_sub(1));
    let start_idx = if selected >= max_visible {
        selected + 1 - max_visible
    } else {
        0
    };
    let end_idx = (start_idx + max_visible).min(sessions.len());

    let mut lines = Vec::new();
    for i in start_idx..end_idx {
        let s = sessions[i];
        let is_selected = i == selected;
        let row_bg = if is_selected { selected_bg } else { Color::Reset };

        let cursor = if is_selected { " ❯ " } else { "   " };
        let cursor_style = Style::default().fg(theme.accent_blue).bg(row_bg).add_modifier(Modifier::BOLD);

        // Model badge
        let (model_tag, model_fg) = if s.model.starts_with("local") {
            ("[Local]", Color::Rgb(255, 215, 130))
        } else if s.model.contains("pro") || s.model.contains("reasoner") {
            ("[ Pro ]", Color::Rgb(215, 175, 255))
        } else if s.model.contains("flash") {
            ("[Flash]", Color::Rgb(135, 215, 235))
        } else if s.model.contains("gpt") || s.model.contains("openai") {
            ("[ GPT ]", Color::Rgb(105, 240, 174))
        } else if s.model.contains("claude") || s.model.contains("anthropic") {
            ("[Claude]", Color::Rgb(255, 167, 38))
        } else {
            ("[Cloud]", Color::Rgb(129, 199, 245))
        };

        // Tag chip
        let tag_span = if let Some(ref t) = s.tag {
            Span::styled(format!("#{:<8} ", t), Style::default().fg(theme.accent_green).bg(row_bg).add_modifier(Modifier::BOLD))
        } else {
            Span::styled("          ", Style::default().bg(row_bg))
        };

        let rel_time = Session::format_relative_time(s.updated_at);
        let time_str = format!("{:>7} ", rel_time);

        let left_w = 3 + 2 + 10 + 8; // cursor (3) + num (2) + tag (10) + badge (8)
        let right_w = time_str.chars().count() + 1;
        let title_w = inner_w.saturating_sub(left_w + right_w).max(10);

        let truncated_title = corex_core::truncate_ellipsis(&s.title, title_w);
        let pad_title = format!("{:<w$}", truncated_title, w = title_w);

        let title_style = if is_selected {
            Style::default().fg(Color::White).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };

        let row = vec![
            Span::styled(cursor, cursor_style),
            tag_span,
            Span::styled(format!("{} ", model_tag), Style::default().fg(model_fg).bg(row_bg).add_modifier(Modifier::BOLD)),
            Span::styled(pad_title, title_style),
            Span::styled(time_str, Style::default().fg(theme.dark_gray).bg(row_bg)),
            Span::styled(" ", Style::default().bg(row_bg)),
        ];
        lines.push(Line::from(row));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_session_preview(
    frame: &mut Frame,
    area: Rect,
    session: Option<&Session>,
    theme: &Theme,
) {
    let w = area.width as usize;
    let mut lines = Vec::new();

    if let Some(s) = session {
        lines.push(Line::from(vec![
            Span::styled(" PREVIEW", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" · ID: {}", &s.id[..8.min(s.id.len())]), Style::default().fg(theme.dark_gray)),
        ]));
        lines.push(Line::from(""));

        // Metadatos clave
        lines.push(Line::from(vec![
            Span::styled("  Model:    ", Style::default().fg(theme.dark_gray)),
            Span::styled(&s.model, Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Messages: ", Style::default().fg(theme.dark_gray)),
            Span::styled(format!("{} turns", s.messages.len()), Style::default().fg(theme.accent_cyan)),
            Span::styled(format!("  ({} tokens)", s.total_usage.total_tokens), Style::default().fg(theme.dark_gray)),
        ]));
        if let Some(ref tag) = s.tag {
            lines.push(Line::from(vec![
                Span::styled("  Tag:      ", Style::default().fg(theme.dark_gray)),
                Span::styled(format!("#{}", tag), Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
            ]));
        }
        lines.push(Line::from(""));

        // Último mensaje (para contexto inmediato)
        lines.push(Line::from(Span::styled("  Latest Exchange:", Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD))));
        
        let last_user = s.messages.iter().rev().find(|m| m.role == "user");
        let last_assistant = s.messages.iter().rev().find(|m| m.role == "assistant");

        let max_text_w = w.saturating_sub(6).max(10);

        if let Some(u) = last_user {
            let snippet = u.text_content().unwrap_or("...");
            let clean = snippet.lines().next().unwrap_or("").trim();
            lines.push(Line::from(vec![
                Span::styled("  User: ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(corex_core::truncate_ellipsis(clean, max_text_w), Style::default().fg(Color::White)),
            ]));
        }

        if let Some(a) = last_assistant {
            let snippet = a.text_content().unwrap_or("...");
            let clean = snippet.lines().next().unwrap_or("").trim();
            lines.push(Line::from(vec![
                Span::styled("  AI:   ", Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
                Span::styled(corex_core::truncate_ellipsis(clean, max_text_w), Style::default().fg(theme.gray)),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(" No preview available", Style::default().fg(theme.dark_gray))));
    }

    frame.render_widget(Paragraph::new(lines), area);
}
