use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use uti_core::config::{FlashSettings, HybridMode, HybridSettings, ProSettings};

use crate::overlay::render_scrim;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTab {
    Models = 0,
    Flash = 1,
    Pro = 2,
    Hybrid = 3,
}

impl ModelTab {
    pub fn next(self) -> Self {
        match self {
            Self::Models => Self::Flash,
            Self::Flash => Self::Pro,
            Self::Pro => Self::Hybrid,
            Self::Hybrid => Self::Models,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Models => Self::Hybrid,
            Self::Flash => Self::Models,
            Self::Pro => Self::Flash,
            Self::Hybrid => Self::Pro,
        }
    }
}

pub const TEMPERATURE_PRESETS: &[f32] = &[0.0, 0.1, 0.2, 0.3, 0.5, 0.7, 1.0, 1.2, 1.5, 2.0];
pub const REASONING_LEVELS: &[&str] = &["dynamic", "low", "medium", "high"];
pub const COMMAND_REASONING_LEVELS: &[&str] = &["low", "medium", "high"];
pub const CODE_REASONING_LEVELS: &[&str] = &["high", "max", "medium", "low"];
pub const SEARCH_REASONING_LEVELS: &[&str] = &["low", "medium", "high", "max"];
pub const PRO_REASONING_LEVELS: &[&str] = &["max", "high", "medium", "low"];
pub const HYBRID_LOCAL_MODELS: &[&str] = &[
    "Llama-3.2-3B-Instruct",
    "Llama-3.2-3B-Instruct-abliterated",
    "gemma-2-2b-it",
    "qwen2.5-coder-1.5b",
    "local-model",
];
pub const LOCAL_URL_PRESETS: &[&str] = &[
    "http://127.0.0.1:8080/v1",
    "http://127.0.0.1:11434/v1",
    "http://localhost:8080/v1",
    "http://localhost:11434/v1",
];
pub const HYBRID_MODES: &[HybridMode] = &[
    HybridMode::AutoTriage,
    HybridMode::LocalScout,
    HybridMode::DraftAndReview,
    HybridMode::CompressionOnly,
];

pub fn get_temp_info(temp: f32) -> (&'static str, &'static str, Color) {
    if temp <= 0.25 {
        ("Precise", "Exact reproducibility & code generation", Color::Rgb(79, 195, 247))
    } else if temp <= 0.65 {
        ("Balanced", "Code synthesis with balanced heuristics", Color::Rgb(105, 240, 174))
    } else if temp <= 1.05 {
        ("Default", "Standard conversational coding & discovery", Color::Rgb(255, 213, 79))
    } else {
        ("Creative", "High variance & exploratory brainstorming", Color::Rgb(255, 152, 0))
    }
}

pub fn get_reasoning_color(r: &str) -> Color {
    match r {
        "dynamic" => Color::Rgb(105, 240, 174),
        "low" => Color::Rgb(79, 195, 247),
        "medium" => Color::Rgb(255, 213, 79),
        "high" => Color::Rgb(255, 110, 110),
        "max" => Color::Rgb(224, 64, 251),
        _ => Color::Rgb(135, 175, 255),
    }
}

#[derive(Debug, Clone)]
pub struct ModelDialogState {
    pub is_open: bool,
    pub current_tab: ModelTab,
    pub selected_model_idx: usize, // 0..3
    pub active_engine: usize,      // 0..3 strictly which engine is currently running
    pub flash_row_idx: usize,      // 0..5
    pub pro_row_idx: usize,        // 0..2
    pub hybrid_row_idx: usize,     // 0..4

    // Model tab
    pub active_model: String,
    pub persist_model: bool,

    // Flash settings
    pub temperature: f32,
    pub flash_reasoning: String,
    pub flash_command_reasoning: String,
    pub flash_code_reasoning: String,
    pub flash_search_reasoning: String,
    pub flash_persist_permanent: bool,

    // Pro settings
    pub pro_reasoning: String,
    pub pro_search_reasoning: String,
    pub pro_persist_permanent: bool,

    // Local settings
    pub local_prompt_lite: bool,
    pub hybrid_secondary_local_model: String,
    pub hybrid_local_url: String,
    pub hybrid_auto_compression: bool,
    pub hybrid_persist_permanent: bool,

    // Server ping feedback
    pub server_status_msg: Option<(String, Color)>,
}

impl Default for ModelDialogState {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelDialogState {
    pub fn new() -> Self {
        Self {
            is_open: false,
            current_tab: ModelTab::Models,
            selected_model_idx: 0,
            active_engine: 0,
            flash_row_idx: 0,
            pro_row_idx: 0,
            hybrid_row_idx: 0,

            active_model: "deepseek-flash".to_string(),
            persist_model: true,

            temperature: 1.0,
            flash_reasoning: "dynamic".to_string(),
            flash_command_reasoning: "low".to_string(),
            flash_code_reasoning: "high".to_string(),
            flash_search_reasoning: "low".to_string(),
            flash_persist_permanent: true,

            pro_reasoning: "max".to_string(),
            pro_search_reasoning: "low".to_string(),
            pro_persist_permanent: true,

            local_prompt_lite: false,
            hybrid_secondary_local_model: "Llama-3.2-3B-Instruct".to_string(),
            hybrid_local_url: "http://127.0.0.1:8080/v1".to_string(),
            hybrid_auto_compression: true,
            hybrid_persist_permanent: true,

            server_status_msg: None,
        }
    }

    pub fn open(
        &mut self,
        current_model: &str,
        flash_settings: &FlashSettings,
        pro_settings: &ProSettings,
        hybrid_settings: &HybridSettings,
        hybrid_enabled: bool,
        local_prompt_lite: bool,
    ) {
        self.is_open = true;
        self.current_tab = ModelTab::Models;
        self.active_model = current_model.to_string();
        self.server_status_msg = None;
        self.local_prompt_lite = local_prompt_lite;

        if hybrid_enabled {
            self.active_engine = 3;
            self.selected_model_idx = 3;
        } else if current_model.starts_with("local") {
            self.active_engine = 2;
            self.selected_model_idx = 2;
        } else if current_model.contains("pro") || current_model.contains("reasoner") {
            self.active_engine = 1;
            self.selected_model_idx = 1;
        } else {
            self.active_engine = 0;
            self.selected_model_idx = 0;
        }

        self.temperature = flash_settings.temperature;
        self.flash_reasoning = flash_settings.reasoning_effort.clone();
        self.flash_command_reasoning = flash_settings.command_reasoning_effort.clone();
        self.flash_code_reasoning = flash_settings.code_reasoning_effort.clone();
        self.flash_search_reasoning = flash_settings.search_reasoning_effort.clone();

        self.pro_reasoning = pro_settings.reasoning_effort.clone();
        self.pro_search_reasoning = pro_settings.search_reasoning_effort.clone();

        self.hybrid_secondary_local_model = hybrid_settings.secondary_local_model.clone();
        self.hybrid_local_url = hybrid_settings.local_url.clone();
        self.hybrid_auto_compression = hybrid_settings.auto_compression;
    }

    pub fn cycle_flash_row(&mut self, forward: bool) {
        match self.flash_row_idx {
            0 => {
                let curr_idx = TEMPERATURE_PRESETS
                    .iter()
                    .position(|&t| (t - self.temperature).abs() < 0.01)
                    .unwrap_or(6);
                let next_idx = if forward {
                    (curr_idx + 1) % TEMPERATURE_PRESETS.len()
                } else if curr_idx == 0 {
                    TEMPERATURE_PRESETS.len() - 1
                } else {
                    curr_idx - 1
                };
                self.temperature = TEMPERATURE_PRESETS[next_idx];
            }
            1 => {
                let curr_idx = REASONING_LEVELS
                    .iter()
                    .position(|&r| r == self.flash_reasoning)
                    .unwrap_or(0);
                let next_idx = if forward {
                    (curr_idx + 1) % REASONING_LEVELS.len()
                } else if curr_idx == 0 {
                    REASONING_LEVELS.len() - 1
                } else {
                    curr_idx - 1
                };
                self.flash_reasoning = REASONING_LEVELS[next_idx].to_string();
            }
            2 => {
                let curr_idx = COMMAND_REASONING_LEVELS
                    .iter()
                    .position(|&r| r == self.flash_command_reasoning)
                    .unwrap_or(0);
                let next_idx = if forward {
                    (curr_idx + 1) % COMMAND_REASONING_LEVELS.len()
                } else if curr_idx == 0 {
                    COMMAND_REASONING_LEVELS.len() - 1
                } else {
                    curr_idx - 1
                };
                self.flash_command_reasoning = COMMAND_REASONING_LEVELS[next_idx].to_string();
            }
            3 => {
                let curr_idx = CODE_REASONING_LEVELS
                    .iter()
                    .position(|&r| r == self.flash_code_reasoning)
                    .unwrap_or(0);
                let next_idx = if forward {
                    (curr_idx + 1) % CODE_REASONING_LEVELS.len()
                } else if curr_idx == 0 {
                    CODE_REASONING_LEVELS.len() - 1
                } else {
                    curr_idx - 1
                };
                self.flash_code_reasoning = CODE_REASONING_LEVELS[next_idx].to_string();
            }
            4 => {
                let curr_idx = SEARCH_REASONING_LEVELS
                    .iter()
                    .position(|&r| r == self.flash_search_reasoning)
                    .unwrap_or(0);
                let next_idx = if forward {
                    (curr_idx + 1) % SEARCH_REASONING_LEVELS.len()
                } else if curr_idx == 0 {
                    SEARCH_REASONING_LEVELS.len() - 1
                } else {
                    curr_idx - 1
                };
                self.flash_search_reasoning = SEARCH_REASONING_LEVELS[next_idx].to_string();
            }
            5 => {
                self.flash_persist_permanent = !self.flash_persist_permanent;
            }
            _ => {}
        }
    }

    pub fn cycle_pro_row(&mut self, forward: bool) {
        match self.pro_row_idx {
            0 => {
                let curr_idx = PRO_REASONING_LEVELS
                    .iter()
                    .position(|&r| r == self.pro_reasoning)
                    .unwrap_or(0);
                let next_idx = if forward {
                    (curr_idx + 1) % PRO_REASONING_LEVELS.len()
                } else if curr_idx == 0 {
                    PRO_REASONING_LEVELS.len() - 1
                } else {
                    curr_idx - 1
                };
                self.pro_reasoning = PRO_REASONING_LEVELS[next_idx].to_string();
            }
            1 => {
                let curr_idx = SEARCH_REASONING_LEVELS
                    .iter()
                    .position(|&r| r == self.pro_search_reasoning)
                    .unwrap_or(0);
                let next_idx = if forward {
                    (curr_idx + 1) % SEARCH_REASONING_LEVELS.len()
                } else if curr_idx == 0 {
                    SEARCH_REASONING_LEVELS.len() - 1
                } else {
                    curr_idx - 1
                };
                self.pro_search_reasoning = SEARCH_REASONING_LEVELS[next_idx].to_string();
            }
            2 => {
                self.pro_persist_permanent = !self.pro_persist_permanent;
            }
            _ => {}
        }
    }

    pub fn cycle_hybrid_row(&mut self, forward: bool) {
        let _ = forward;
        match self.hybrid_row_idx {
            0 => {
                // System Prompt toggle: Full <-> Lite
                self.local_prompt_lite = !self.local_prompt_lite;
            }
            1 => {
                let curr_idx = HYBRID_LOCAL_MODELS
                    .iter()
                    .position(|&m| m == self.hybrid_secondary_local_model)
                    .unwrap_or(0);
                let next_idx = if forward {
                    (curr_idx + 1) % HYBRID_LOCAL_MODELS.len()
                } else if curr_idx == 0 {
                    HYBRID_LOCAL_MODELS.len() - 1
                } else {
                    curr_idx - 1
                };
                self.hybrid_secondary_local_model = HYBRID_LOCAL_MODELS[next_idx].to_string();
            }
            2 => {
                let curr_idx = LOCAL_URL_PRESETS
                    .iter()
                    .position(|&u| u == self.hybrid_local_url)
                    .unwrap_or(0);
                let next_idx = if forward {
                    (curr_idx + 1) % LOCAL_URL_PRESETS.len()
                } else if curr_idx == 0 {
                    LOCAL_URL_PRESETS.len() - 1
                } else {
                    curr_idx - 1
                };
                self.hybrid_local_url = LOCAL_URL_PRESETS[next_idx].to_string();
            }
            3 => {
                self.hybrid_auto_compression = !self.hybrid_auto_compression;
            }
            4 => {
                self.hybrid_persist_permanent = !self.hybrid_persist_permanent;
            }
            _ => {}
        }
    }

    pub fn to_flash_settings(&self) -> FlashSettings {
        FlashSettings {
            temperature: self.temperature,
            reasoning_effort: self.flash_reasoning.clone(),
            command_reasoning_effort: self.flash_command_reasoning.clone(),
            code_reasoning_effort: self.flash_code_reasoning.clone(),
            search_reasoning_effort: self.flash_search_reasoning.clone(),
        }
    }

    pub fn to_pro_settings(&self) -> ProSettings {
        ProSettings {
            reasoning_effort: self.pro_reasoning.clone(),
            search_reasoning_effort: self.pro_search_reasoning.clone(),
        }
    }

    pub fn to_hybrid_settings(&self) -> HybridSettings {
        HybridSettings {
            primary_model: "deepseek-flash".to_string(),
            local_url: self.hybrid_local_url.clone(),
            secondary_local_model: self.hybrid_secondary_local_model.clone(),
            auto_compression: self.hybrid_auto_compression,
            mode: HybridMode::CompressionOnly,
        }
    }

    pub fn next_tab(&mut self) {
        self.current_tab = self.current_tab.next();
    }

    pub fn prev_tab(&mut self) {
        self.current_tab = self.current_tab.prev();
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }
}

fn key_badge(key: &'static str, desc: &'static str, color: Color, gray: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled("[", Style::default().fg(Color::Rgb(70, 85, 110))),
        Span::styled(key, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled("] ", Style::default().fg(Color::Rgb(70, 85, 110))),
        Span::styled(desc, Style::default().fg(gray)),
        Span::raw("   "),
    ]
}

pub fn render_model_dialog(
    frame: &mut Frame,
    area: Rect,
    state: &ModelDialogState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let dialog_width = 80.min(area.width.saturating_sub(4)).max(54);
    let dialog_height = 18.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    render_scrim(frame, area);
    frame.render_widget(Clear, dialog_area);

    let inner_w = dialog_width.saturating_sub(2) as usize;

    let mut lines = Vec::new();
    lines.push(Line::from(""));

    // 1. Pill Tabs Header
    let tabs = [
        (ModelTab::Models, "Models", "1"),
        (ModelTab::Flash, "Flash CoT", "2"),
        (ModelTab::Pro, "Pro CoT", "3"),
        (ModelTab::Hybrid, "Local", "4"),
    ];

    let mut tab_spans = vec![Span::raw("  ")];
    for (tab_type, title, key) in tabs.iter() {
        let is_active = state.current_tab == *tab_type;
        if is_active {
            tab_spans.push(Span::styled(
                format!(" [{} {}] ", key, title),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(38, 54, 82))
                    .add_modifier(Modifier::BOLD),
            ));
            tab_spans.push(Span::raw(" "));
        } else {
            tab_spans.push(Span::styled(
                format!("  {} {}  ", key, title),
                Style::default().fg(Color::Rgb(110, 125, 150)),
            ));
            tab_spans.push(Span::raw(" "));
        }
    }
    lines.push(Line::from(tab_spans));

    // Divider below tabs
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("─".repeat(inner_w.saturating_sub(4)), Style::default().fg(Color::Rgb(35, 45, 60))),
    ]));

    // 2. Tab Content Body
    match state.current_tab {
        ModelTab::Models => render_models_tab(&mut lines, state, theme, inner_w),
        ModelTab::Flash => render_flash_tab(&mut lines, state, theme, inner_w),
        ModelTab::Pro => render_pro_tab(&mut lines, state, theme, inner_w),
        ModelTab::Hybrid => render_hybrid_tab(&mut lines, state, theme, inner_w),
    }

    // Divider above footer
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("─".repeat(inner_w.saturating_sub(4)), Style::default().fg(Color::Rgb(35, 45, 60))),
    ]));

    // 3. Contextual Footer
    let mut footer = vec![Span::raw("   ")];
    match state.current_tab {
        ModelTab::Models => {
            footer.extend(key_badge("Enter", "Select Engine", theme.accent_blue, theme.gray));
            footer.extend(key_badge("Tab", "Next Tab", theme.accent_cyan, theme.gray));
            footer.extend(key_badge("T", "Persist", theme.accent_green, theme.gray));
            footer.extend(key_badge("Esc", "Close", theme.gray, theme.gray));
        }
        ModelTab::Flash | ModelTab::Pro => {
            footer.extend(key_badge("↑/↓", "Select", theme.accent_blue, theme.gray));
            footer.extend(key_badge("◄/►", "Adjust Value", theme.accent_cyan, theme.gray));
            footer.extend(key_badge("Tab", "Next Tab", theme.accent_yellow, theme.gray));
            footer.extend(key_badge("Esc", "Close", theme.gray, theme.gray));
        }
        ModelTab::Hybrid => {
            footer.extend(key_badge("Enter", "Activate Local", theme.accent_cyan, theme.gray));
            footer.extend(key_badge("C", "Ping Server", theme.accent_green, theme.gray));
            footer.extend(key_badge("◄/►", "Adjust", theme.accent_yellow, theme.gray));
            footer.extend(key_badge("Esc", "Close", theme.gray, theme.gray));
        }
    }
    lines.push(Line::from(footer));

    let block = Block::default()
        .title("  AI Model & Architecture Control Center  ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent_blue));

    frame.render_widget(Paragraph::new(lines).block(block), dialog_area);
}

fn render_models_tab(lines: &mut Vec<Line>, state: &ModelDialogState, theme: &Theme, inner_w: usize) {
    let selected_bg = Color::Rgb(26, 38, 58);

    let models = [
        (
            "DeepSeek-V4.1-Flash",
            "Primary coding engine · Sub-second tool execution & dynamic CoT",
            "[Recommended]",
            theme.accent_green,
        ),
        (
            "DeepSeek-V4-Pro",
            "Deep reasoning architecture · Hard debugging & complex mathematics",
            "[Thinking]",
            theme.accent_purple,
        ),
        (
            "Local Offline Assistant",
            "100% Private local inference · llama.cpp / Ollama ($0.00 cost)",
            "[Air-gapped]",
            theme.accent_yellow,
        ),
        (
            "Smart Hybrid Mode",
            "Cloud MoE power + Local SLM log compression (saves ~70% tokens)",
            "[Dual-Engine]",
            theme.accent_cyan,
        ),
    ];

    for (i, (name, desc, badge, badge_col)) in models.iter().enumerate() {
        let is_selected = i == state.selected_model_idx;
        let is_active = i == state.active_engine;
        let row_bg = if is_selected { selected_bg } else { Color::Reset };

        let cursor = if is_selected { " ❯ " } else { "   " };
        let cursor_style = if is_selected {
            Style::default().fg(theme.accent_blue).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
        };

        let num_style = if is_selected {
            Style::default().fg(theme.accent_blue).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dark_gray).add_modifier(Modifier::BOLD)
        };

        let radio_symbol = if is_active { "[●] " } else { "[ ] " };
        let radio_style = if is_selected {
            Style::default().fg(if is_active { theme.accent_green } else { theme.dark_gray }).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(if is_active { theme.accent_green } else { theme.dark_gray }).add_modifier(Modifier::BOLD)
        };

        let name_style = if is_selected {
            Style::default().fg(Color::White).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)
        };

        let badge_style = if is_selected {
            Style::default().fg(*badge_col).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(*badge_col).add_modifier(Modifier::BOLD)
        };

        // Line 1: Cursor + Num + Radio + Name + Spacing + Badge + Trailing
        let left_len = 3 + 3 + 4 + name.chars().count();
        let badge_len = badge.chars().count();
        let space_len = inner_w.saturating_sub(left_len + badge_len + 2).max(2);

        let space_style = if is_selected { Style::default().bg(row_bg) } else { Style::default() };

        let mut line1_spans = vec![
            Span::styled(cursor, cursor_style),
            Span::styled(format!("{}. ", i + 1), num_style),
            Span::styled(radio_symbol, radio_style),
            Span::styled(*name, name_style),
            Span::styled(" ".repeat(space_len), space_style),
            Span::styled(*badge, badge_style),
        ];
        let used_line1 = left_len + space_len + badge_len;
        if inner_w > used_line1 {
            line1_spans.push(Span::styled(" ".repeat(inner_w - used_line1), space_style));
        }
        lines.push(Line::from(line1_spans));

        // Line 2: Indented description
        let indent = 10;
        let max_desc_w = inner_w.saturating_sub(indent + 2);
        let truncated_desc = uti_core::truncate_ellipsis(desc, max_desc_w);
        let desc_len = truncated_desc.chars().count();
        let desc_style = if is_selected {
            Style::default().fg(Color::Rgb(180, 195, 215)).bg(row_bg)
        } else {
            Style::default().fg(theme.dark_gray)
        };

        let mut line2_spans = vec![
            Span::styled(" ".repeat(indent), space_style),
            Span::styled(truncated_desc, desc_style),
        ];
        let used_line2 = indent + desc_len;
        if inner_w > used_line2 {
            line2_spans.push(Span::styled(" ".repeat(inner_w - used_line2), space_style));
        }
        lines.push(Line::from(line2_spans));
    }

    // Persistence row
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("─".repeat(inner_w.saturating_sub(4)), Style::default().fg(Color::Rgb(35, 45, 60))),
    ]));

    let (p_label, p_col, p_desc) = if state.persist_model {
        ("[PERMANENT]", theme.accent_green, "Saved in ~/.config/uti/config.toml")
    } else {
        ("[SESSION ONLY]", theme.accent_yellow, "Active for current process only")
    };

    lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("Persistence: ", Style::default().fg(theme.foreground)),
        Span::styled(p_label, Style::default().fg(p_col).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  ·  {} (Press T to toggle)", p_desc), Style::default().fg(theme.dark_gray)),
    ]));
}

fn render_flash_tab(lines: &mut Vec<Line>, state: &ModelDialogState, theme: &Theme, inner_w: usize) {
    let selected_bg = Color::Rgb(26, 38, 58);
    let (temp_tag, temp_desc, temp_col) = get_temp_info(state.temperature);

    let rows = [
        (
            0,
            "1. Temperature:       ",
            format!("{:.1}", state.temperature),
            temp_col,
            temp_tag,
            temp_desc,
        ),
        (
            1,
            "2. General Reasoning: ",
            state.flash_reasoning.to_uppercase(),
            get_reasoning_color(&state.flash_reasoning),
            "Adaptive",
            "~200ms tools, deep CoT for complex code",
        ),
        (
            2,
            "3. Command & Tool CoT:",
            state.flash_command_reasoning.to_uppercase(),
            get_reasoning_color(&state.flash_command_reasoning),
            "Fast",
            "~200ms quick checks for shell tools",
        ),
        (
            3,
            "4. Coding & Dev CoT:  ",
            state.flash_code_reasoning.to_uppercase(),
            get_reasoning_color(&state.flash_code_reasoning),
            "Deep CoT",
            "High verification for architecture & code",
        ),
        (
            4,
            "5. Web Search CoT:    ",
            state.flash_search_reasoning.to_uppercase(),
            get_reasoning_color(&state.flash_search_reasoning),
            "Fast",
            "Quick snippets & direct links (~2-4s)",
        ),
        (
            5,
            "6. Save Overrides:    ",
            if state.flash_persist_permanent { "PERMANENT".to_string() } else { "SESSION".to_string() },
            if state.flash_persist_permanent { Color::Rgb(105, 240, 174) } else { Color::Rgb(255, 152, 0) },
            "Storage",
            if state.flash_persist_permanent { "Saves to ~/.config/uti/config.toml" } else { "Active in this session only" },
        ),
    ];

    for (row_idx, label, val_str, val_col, tag, desc) in rows.iter() {
        let is_selected = state.flash_row_idx == *row_idx;
        let row_bg = if is_selected { selected_bg } else { Color::Reset };

        let cursor = if is_selected { " ❯ " } else { "   " };
        let cursor_col = if is_selected { theme.accent_blue } else { theme.dark_gray };
        let label_style = if is_selected {
            Style::default().fg(Color::White).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray)
        };

        let padded_val = format!("{:^9}", val_str);
        let arrow_col = if is_selected { theme.accent_cyan } else { theme.dark_gray };

        let prefix_len = 3 + 22 + 2 + 9 + 2 + 3 + 9 + 2;
        let max_desc_w = inner_w.saturating_sub(prefix_len);
        let truncated_desc = uti_core::truncate_ellipsis(desc, max_desc_w);

        let space_style = if is_selected { Style::default().bg(row_bg) } else { Style::default() };

        let mut row_spans = vec![
            Span::styled(cursor, if is_selected { Style::default().fg(cursor_col).bg(row_bg).add_modifier(Modifier::BOLD) } else { Style::default().fg(cursor_col) }),
            Span::styled(*label, label_style),
            Span::styled("◄ ", if is_selected { Style::default().fg(arrow_col).bg(row_bg) } else { Style::default().fg(arrow_col) }),
            Span::styled(padded_val, if is_selected { Style::default().fg(*val_col).bg(row_bg).add_modifier(Modifier::BOLD) } else { Style::default().fg(*val_col).add_modifier(Modifier::BOLD) }),
            Span::styled(" ►", if is_selected { Style::default().fg(arrow_col).bg(row_bg) } else { Style::default().fg(arrow_col) }),
            Span::styled(" · ", if is_selected { Style::default().fg(Color::Rgb(70, 85, 105)).bg(row_bg) } else { Style::default().fg(Color::Rgb(70, 85, 105)) }),
            Span::styled(format!("{:<9}", tag), if is_selected { Style::default().fg(*val_col).bg(row_bg).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray).add_modifier(Modifier::BOLD) }),
            Span::styled("  ", space_style),
            Span::styled(truncated_desc.clone(), if is_selected { Style::default().fg(theme.foreground).bg(row_bg) } else { Style::default().fg(theme.dark_gray) }),
        ];

        let used_w = prefix_len + truncated_desc.chars().count();
        if inner_w > used_w {
            row_spans.push(Span::styled(" ".repeat(inner_w - used_w), space_style));
        }
        lines.push(Line::from(row_spans));
    }
}

fn render_pro_tab(lines: &mut Vec<Line>, state: &ModelDialogState, theme: &Theme, inner_w: usize) {
    let selected_bg = Color::Rgb(26, 38, 58);

    let rows = [
        (
            0,
            "1. Model Reasoning: ",
            state.pro_reasoning.to_uppercase(),
            get_reasoning_color(&state.pro_reasoning),
            "Max Depth",
            "Exhaustive reasoning for novel architecture",
        ),
        (
            1,
            "2. Web Search CoT:  ",
            state.pro_search_reasoning.to_uppercase(),
            get_reasoning_color(&state.pro_search_reasoning),
            "Fast",
            "Quick snippets & direct links (~2-4s)",
        ),
        (
            2,
            "3. Save Overrides:  ",
            if state.pro_persist_permanent { "PERMANENT".to_string() } else { "SESSION".to_string() },
            if state.pro_persist_permanent { Color::Rgb(105, 240, 174) } else { Color::Rgb(255, 152, 0) },
            "Storage",
            if state.pro_persist_permanent { "Saves to ~/.config/uti/config.toml" } else { "Active in this session only" },
        ),
    ];

    for (row_idx, label, val_str, val_col, tag, desc) in rows.iter() {
        let is_selected = state.pro_row_idx == *row_idx;
        let row_bg = if is_selected { selected_bg } else { Color::Reset };

        let cursor = if is_selected { " ❯ " } else { "   " };
        let cursor_col = if is_selected { theme.accent_blue } else { theme.dark_gray };
        let label_style = if is_selected {
            Style::default().fg(Color::White).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray)
        };

        let padded_val = format!("{:^9}", val_str);
        let arrow_col = if is_selected { theme.accent_cyan } else { theme.dark_gray };

        let prefix_len = 3 + 20 + 2 + 9 + 2 + 3 + 9 + 2;
        let max_desc_w = inner_w.saturating_sub(prefix_len);
        let truncated_desc = uti_core::truncate_ellipsis(desc, max_desc_w);

        let space_style = if is_selected { Style::default().bg(row_bg) } else { Style::default() };

        let mut row_spans = vec![
            Span::styled(cursor, if is_selected { Style::default().fg(cursor_col).bg(row_bg).add_modifier(Modifier::BOLD) } else { Style::default().fg(cursor_col) }),
            Span::styled(*label, label_style),
            Span::styled("◄ ", if is_selected { Style::default().fg(arrow_col).bg(row_bg) } else { Style::default().fg(arrow_col) }),
            Span::styled(padded_val, if is_selected { Style::default().fg(*val_col).bg(row_bg).add_modifier(Modifier::BOLD) } else { Style::default().fg(*val_col).add_modifier(Modifier::BOLD) }),
            Span::styled(" ►", if is_selected { Style::default().fg(arrow_col).bg(row_bg) } else { Style::default().fg(arrow_col) }),
            Span::styled(" · ", if is_selected { Style::default().fg(Color::Rgb(70, 85, 105)).bg(row_bg) } else { Style::default().fg(Color::Rgb(70, 85, 105)) }),
            Span::styled(format!("{:<9}", tag), if is_selected { Style::default().fg(*val_col).bg(row_bg).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray).add_modifier(Modifier::BOLD) }),
            Span::styled("  ", space_style),
            Span::styled(truncated_desc.clone(), if is_selected { Style::default().fg(theme.foreground).bg(row_bg) } else { Style::default().fg(theme.dark_gray) }),
        ];

        let used_w = prefix_len + truncated_desc.chars().count();
        if inner_w > used_w {
            row_spans.push(Span::styled(" ".repeat(inner_w - used_w), space_style));
        }
        lines.push(Line::from(row_spans));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("DeepSeek Pro uses extensive thinking tokens for hard architecture & complex bugs.", Style::default().fg(theme.dark_gray)),
    ]));
}

fn render_hybrid_tab(lines: &mut Vec<Line>, state: &ModelDialogState, theme: &Theme, inner_w: usize) {
    let selected_bg = Color::Rgb(26, 38, 58);

    let rows = [
        (
            0,
            "1. System Prompt:   ",
            if state.local_prompt_lite { "LITE".to_string() } else { "FULL".to_string() },
            if state.local_prompt_lite { Color::Rgb(255, 213, 79) } else { Color::Rgb(105, 240, 174) },
            if state.local_prompt_lite { "Simplified prompt for SLMs (2B-7B)" } else { "Full UTI prompt with tool enforcement" },
        ),
        (
            1,
            "2. Local Model:     ",
            state.hybrid_secondary_local_model.clone(),
            Color::Rgb(105, 240, 174),
            "Air-gapped offline SLM ($0.00 cost)",
        ),
        (
            2,
            "3. Server Endpoint: ",
            state.hybrid_local_url.clone(),
            theme.accent_yellow,
            "llama-server / Ollama HTTP endpoint",
        ),
        (
            3,
            "4. Log Compression: ",
            if state.hybrid_auto_compression { "ENABLED".to_string() } else { "DISABLED".to_string() },
            if state.hybrid_auto_compression { Color::Rgb(105, 240, 174) } else { Color::Rgb(244, 67, 54) },
            "Compresses tool logs (~70% token savings)",
        ),
        (
            4,
            "5. Save Overrides:  ",
            if state.hybrid_persist_permanent { "PERMANENT".to_string() } else { "SESSION".to_string() },
            if state.hybrid_persist_permanent { Color::Rgb(105, 240, 174) } else { Color::Rgb(255, 152, 0) },
            "Saves overrides to ~/.config/uti/config.toml",
        ),
    ];

    for (row_idx, label, val_str, val_col, desc) in rows.iter() {
        let is_selected = state.hybrid_row_idx == *row_idx;
        let row_bg = if is_selected { selected_bg } else { Color::Reset };

        let cursor = if is_selected { " ❯ " } else { "   " };
        let cursor_col = if is_selected { theme.accent_cyan } else { theme.dark_gray };
        let label_style = if is_selected {
            Style::default().fg(Color::White).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray)
        };

        let padded_val = format!("{:^13}", val_str);
        let arrow_col = if is_selected { theme.accent_cyan } else { theme.dark_gray };

        let prefix_len = 3 + 20 + 2 + 13 + 2 + 3;
        let max_desc_w = inner_w.saturating_sub(prefix_len);
        let truncated_desc = uti_core::truncate_ellipsis(desc, max_desc_w);

        let space_style = if is_selected { Style::default().bg(row_bg) } else { Style::default() };

        let mut row_spans = vec![
            Span::styled(cursor, if is_selected { Style::default().fg(cursor_col).bg(row_bg).add_modifier(Modifier::BOLD) } else { Style::default().fg(cursor_col) }),
            Span::styled(*label, label_style),
            Span::styled("◄ ", if is_selected { Style::default().fg(arrow_col).bg(row_bg) } else { Style::default().fg(arrow_col) }),
            Span::styled(padded_val, if is_selected { Style::default().fg(*val_col).bg(row_bg).add_modifier(Modifier::BOLD) } else { Style::default().fg(*val_col).add_modifier(Modifier::BOLD) }),
            Span::styled(" ►", if is_selected { Style::default().fg(arrow_col).bg(row_bg) } else { Style::default().fg(arrow_col) }),
            Span::styled(" · ", if is_selected { Style::default().fg(Color::Rgb(70, 85, 105)).bg(row_bg) } else { Style::default().fg(Color::Rgb(70, 85, 105)) }),
            Span::styled(truncated_desc.clone(), if is_selected { Style::default().fg(*val_col).bg(row_bg) } else { Style::default().fg(theme.dark_gray) }),
        ];

        let used_w = prefix_len + truncated_desc.chars().count();
        if inner_w > used_w {
            row_spans.push(Span::styled(" ".repeat(inner_w - used_w), space_style));
        }
        lines.push(Line::from(row_spans));
    }

    lines.push(Line::from(""));
    if let Some((ref msg, col)) = state.server_status_msg {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled("Server Ping: ", Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)),
            Span::styled(msg.clone(), Style::default().fg(col).add_modifier(Modifier::BOLD)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled("Server Status: Press [C] to ping llama-server / Ollama endpoint", Style::default().fg(theme.dark_gray)),
        ]));
    }
}
