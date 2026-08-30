use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, BorderType};
use ratatui::Frame;
use uti_core::config::{FlashSettings, HybridSettings, ProSettings};

use crate::overlay::render_scrim;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDialogView {
    Main,
    CloudMenu,
    FlashConfig,
    ProConfig,
    LocalMenu,
    HybridConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashConfigRow {
    Temperature,
    ModelReasoning,
    CommandReasoning,
    CodeReasoning,
    SearchReasoning,
    Persistence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProConfigRow {
    ModelReasoning,
    SearchReasoning,
    Persistence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridConfigRow {
    Mode,
    PrimaryModel,
    CommandReasoning,
    CodeReasoning,
    SecondaryLocalModel,
    LocalUrl,
    AutoCompression,
    Persistence,
}

pub const HYBRID_MODES: &[uti_core::config::HybridMode] = &[
    uti_core::config::HybridMode::AutoTriage,
    uti_core::config::HybridMode::LocalScout,
    uti_core::config::HybridMode::DraftAndReview,
    uti_core::config::HybridMode::CompressionOnly,
];

pub const TEMPERATURE_PRESETS: &[f32] = &[0.0, 0.1, 0.2, 0.3, 0.5, 0.7, 1.0, 1.2, 1.5, 2.0];

pub fn get_temp_label(temp: f32) -> &'static str {
    if temp <= 0.05 { "Deterministic — exact reproducibility" }
    else if temp <= 0.25 { "Precise — best for code & bugs" }
    else if temp <= 0.55 { "Balanced — code + light creativity" }
    else if temp <= 0.75 { "Creative — docs & brainstorming" }
    else if temp <= 1.05 { "Default — natural conversation" }
    else if temp <= 1.55 { "Very creative — experimental" }
    else { "Maximum randomness" }
}

pub fn get_temp_color(temp: f32) -> Color {
    if temp <= 0.25 { Color::Rgb(79, 195, 247) }
    else if temp <= 0.55 { Color::Rgb(105, 240, 174) }
    else if temp <= 1.05 { Color::Rgb(255, 213, 79) }
    else if temp <= 1.55 { Color::Rgb(255, 152, 0) }
    else { Color::Rgb(244, 67, 54) }
}

pub const REASONING_LEVELS: &[&str] = &["dynamic", "low", "medium", "high"];
pub const COMMAND_REASONING_LEVELS: &[&str] = &["low", "medium", "high"];
pub const CODE_REASONING_LEVELS: &[&str] = &["high", "max", "medium", "low"];
pub const PRO_REASONING_LEVELS: &[&str] = &["low", "medium", "high", "max"];

pub fn get_reasoning_label(r: &str) -> &'static str {
    match r {
        "dynamic" => "Dynamic — ~200ms for tools/commands, high for code",
        "low" => "Fast & concise — simple tasks (~200-300ms)",
        "medium" => "Balanced — most problems",
        "high" => "Deep reasoning — complex architecture & hard bugs",
        "max" => "Maximum depth — large scale refactoring & novel design",
        _ => "Standard reasoning",
    }
}

pub fn get_command_reasoning_label(r: &str) -> &'static str {
    match r {
        "low" => "Fast (~200ms) — for shell tools & inspection",
        "medium" => "Balanced (~600ms) — multi-command planning",
        "high" => "Deep safety (~2s) — extensive pre-checks",
        _ => "Command reasoning",
    }
}

pub fn get_code_reasoning_label(r: &str) -> &'static str {
    match r {
        "high" => "Deep CoT (Recommended) — architecture & complex code",
        "max" => "Maximum depth — large scale refactoring & design",
        "medium" => "Balanced — rapid prototyping & scripting",
        "low" => "Fast & concise — trivial edits & quick fixes",
        _ => "Code reasoning",
    }
}

pub fn get_reasoning_color(r: &str) -> Color {
    match r {
        "dynamic" => Color::Rgb(105, 240, 174),
        "low" => Color::Rgb(79, 195, 247),
        "medium" => Color::Rgb(255, 152, 0),
        "high" => Color::Rgb(244, 67, 54),
        "max" => Color::Rgb(224, 64, 251),
        _ => Color::Rgb(135, 175, 255),
    }
}

pub const SEARCH_REASONING_LEVELS: &[&str] = &["low", "medium", "high", "max"];

pub fn get_search_reasoning_label(r: &str) -> &'static str {
    match r {
        "low" => "Fast snippets & links (~2-4s)",
        "medium" => "Balanced search & overview (~6-10s)",
        "high" => "Deep multi-page crawl & full synthesis (~15-20s)",
        "max" => "Exhaustive multi-source reasoning (~25-30s)",
        _ => "Web search",
    }
}

pub const HYBRID_PRIMARY_MODELS: &[&str] = &["deepseek-v4-flash", "deepseek-v4-pro"];
pub const HYBRID_LOCAL_MODELS: &[&str] = &[
    "Llama-3.2-3B-Instruct",
    "Llama-3.2-3B-Instruct-abliterated",
    "gemma-2-2b-it",
    "qwen2.5-coder-1.5b",
    "local-model",
];

pub struct ModelDialogState {
    pub is_open: bool,
    pub view: ModelDialogView,
    pub selected_main_idx: usize,
    pub selected_cloud_idx: usize,
    pub selected_local_idx: usize,
    pub persist_model: bool,

    // Flash config
    pub flash_row: FlashConfigRow,
    pub temperature: f32,
    pub flash_reasoning: String,
    pub flash_command_reasoning: String,
    pub flash_code_reasoning: String,
    pub flash_search_reasoning: String,
    pub flash_persist_permanent: bool,

    // Pro config
    pub pro_row: ProConfigRow,
    pub pro_reasoning: String,
    pub pro_search_reasoning: String,
    pub pro_persist_permanent: bool,

    // Hybrid config
    pub hybrid_row: HybridConfigRow,
    pub hybrid_mode: uti_core::config::HybridMode,
    pub hybrid_primary_model: String,
    pub hybrid_command_reasoning: String,
    pub hybrid_code_reasoning: String,
    pub hybrid_secondary_local_model: String,
    pub hybrid_local_url: String,
    pub hybrid_auto_compression: bool,
    pub hybrid_persist_permanent: bool,
}

impl ModelDialogState {
    pub fn new() -> Self {
        Self {
            is_open: false,
            view: ModelDialogView::Main,
            selected_main_idx: 0,
            selected_cloud_idx: 0,
            selected_local_idx: 0,
            persist_model: false,

            flash_row: FlashConfigRow::Temperature,
            temperature: 1.0,
            flash_reasoning: "dynamic".to_string(),
            flash_command_reasoning: "low".to_string(),
            flash_code_reasoning: "high".to_string(),
            flash_search_reasoning: "low".to_string(),
            flash_persist_permanent: true,

            pro_row: ProConfigRow::ModelReasoning,
            pro_reasoning: "max".to_string(),
            pro_search_reasoning: "low".to_string(),
            pro_persist_permanent: true,

            hybrid_row: HybridConfigRow::Mode,
            hybrid_mode: uti_core::config::HybridMode::AutoTriage,
            hybrid_primary_model: "deepseek-v4-flash".to_string(),
            hybrid_command_reasoning: "low".to_string(),
            hybrid_code_reasoning: "high".to_string(),
            hybrid_secondary_local_model: "Llama-3.2-3B-Instruct".to_string(),
            hybrid_local_url: "http://127.0.0.1:8080/v1".to_string(),
            hybrid_auto_compression: true,
            hybrid_persist_permanent: true,
        }
    }

    pub fn open(
        &mut self,
        current_model: &str,
        flash_settings: &FlashSettings,
        pro_settings: &ProSettings,
        hybrid_settings: &HybridSettings,
        _hybrid_enabled: bool,
    ) {
        self.is_open = true;
        self.view = ModelDialogView::Main;
        self.selected_main_idx = 0; // Always start with option 1 selected

        self.temperature = flash_settings.temperature;
        self.flash_reasoning = flash_settings.reasoning_effort.clone();
        self.flash_command_reasoning = flash_settings.command_reasoning_effort.clone();
        self.flash_code_reasoning = flash_settings.code_reasoning_effort.clone();
        self.flash_search_reasoning = flash_settings.search_reasoning_effort.clone();

        self.pro_reasoning = pro_settings.reasoning_effort.clone();
        self.pro_search_reasoning = pro_settings.search_reasoning_effort.clone();

        self.hybrid_mode = hybrid_settings.mode;
        self.hybrid_primary_model = hybrid_settings.primary_model.clone();
        self.hybrid_command_reasoning = flash_settings.command_reasoning_effort.clone();
        self.hybrid_code_reasoning = flash_settings.code_reasoning_effort.clone();
        self.hybrid_secondary_local_model = hybrid_settings.secondary_local_model.clone();
        self.hybrid_local_url = hybrid_settings.local_url.clone();
        self.hybrid_auto_compression = hybrid_settings.auto_compression;

        if current_model.contains("reasoner") || current_model.contains("pro") {
            self.selected_cloud_idx = 1;
        } else {
            self.selected_cloud_idx = 0;
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
            primary_model: self.hybrid_primary_model.clone(),
            local_url: self.hybrid_local_url.clone(),
            secondary_local_model: self.hybrid_secondary_local_model.clone(),
            auto_compression: self.hybrid_auto_compression,
            mode: self.hybrid_mode,
        }
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.view = ModelDialogView::Main;
    }
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

    let dialog_width = 88.min(area.width).max(60);
    let dialog_height = match state.view {
        ModelDialogView::Main => 14,
        ModelDialogView::CloudMenu => 16,
        ModelDialogView::FlashConfig => 17,
        ModelDialogView::ProConfig => 13,
        ModelDialogView::LocalMenu => 14,
        ModelDialogView::HybridConfig => 22,
    }.min(area.height);

    let x = (area.width - dialog_width) / 2;
    let y = (area.height - dialog_height) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    render_scrim(frame, area);
    frame.render_widget(Clear, dialog_area);

    match state.view {
        ModelDialogView::Main => render_main_categories_view(frame, dialog_area, state, theme),
        ModelDialogView::CloudMenu => render_cloud_menu_view(frame, dialog_area, state, theme),
        ModelDialogView::FlashConfig => render_flash_config_view(frame, dialog_area, state, theme),
        ModelDialogView::ProConfig => render_pro_config_view(frame, dialog_area, state, theme),
        ModelDialogView::LocalMenu => render_local_menu_view(frame, dialog_area, state, theme),
        ModelDialogView::HybridConfig => render_hybrid_config_view(frame, dialog_area, state, theme),
    }
}

fn render_main_categories_view(
    frame: &mut Frame,
    area: Rect,
    state: &ModelDialogState,
    theme: &Theme,
) {
    let categories = [
        (
            "1. Cloud API Models (DeepSeek)",
            "Standard & Thinking models running 100% on DeepSeek Cloud",
        ),
        (
            "2. Local LLM Assistant",
            "Standalone offline local model for zero-cost queries & quick chat",
        ),
        (
            "3. Hybrid Mode (Cloud + Local)",
            "Combines Cloud coding power with Local LLM log compression (saves ~70% tokens)",
        ),
    ];

    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "    Model & Architecture Categories",
        Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (i, (title, desc)) in categories.iter().enumerate() {
        let is_selected = i == state.selected_main_idx;
        let bullet = if is_selected { "● " } else { "  " };

        let title_style = if is_selected {
            Style::default()
                .fg(theme.accent_blue)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };

        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(bullet, Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            Span::styled(*title, title_style),
        ]));

        lines.push(Line::from(vec![
            Span::raw("         "),
            Span::styled(*desc, Style::default().fg(theme.gray)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "(Press Enter or 1-3 to open category · Esc to cancel)",
            Style::default().fg(theme.gray),
        ),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_blue));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_cloud_menu_view(
    frame: &mut Frame,
    area: Rect,
    state: &ModelDialogState,
    theme: &Theme,
) {
    let options = [
        (
            "1. DeepSeek-V4-Flash",
            format!(
                "Select ultra-fast model  ·  t:{:.1}  r:{}",
                state.temperature, state.flash_reasoning
            ),
        ),
        (
            "2. DeepSeek-V4-Pro (Thinking)",
            format!(
                "Select deep reasoning model  ·  r:{}",
                state.pro_reasoning
            ),
        ),
        (
            "3. Configure Flash Settings",
            "Adjust temperature & reasoning effort for V4-Flash".to_string(),
        ),
        (
            "4. Configure Pro Settings",
            "Adjust reasoning effort (low / high / max) for V4-Pro".to_string(),
        ),
    ];

    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "    Cloud API Models (DeepSeek)",
        Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (i, (title, desc)) in options.iter().enumerate() {
        let is_selected = i == state.selected_cloud_idx;
        let bullet = if is_selected { "● " } else { "  " };

        let title_style = if is_selected {
            Style::default()
                .fg(theme.accent_blue)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };

        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(bullet, Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
            Span::styled(*title, title_style),
        ]));

        lines.push(Line::from(vec![
            Span::raw("         "),
            Span::styled(desc.as_str(), Style::default().fg(theme.gray)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    Remember model for future sessions: "),
        Span::styled(
            if state.persist_model { "true" } else { "false" },
            Style::default().fg(if state.persist_model { Color::Green } else { theme.accent_yellow }),
        ),
        Span::styled(" (Press Tab to toggle)", Style::default().fg(theme.gray)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "(Press Enter or 1-4 to select · Tab for persistence · Esc to go back)",
            Style::default().fg(theme.gray),
        ),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_blue));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_local_menu_view(
    frame: &mut Frame,
    area: Rect,
    state: &ModelDialogState,
    theme: &Theme,
) {
    let options = [
        (
            "1. Activate Standalone Offline Mode (100% Local)",
            format!("Run completely offline @ $0.00 (Model: {})", state.hybrid_secondary_local_model),
        ),
        (
            "2. Check Local Server Status",
            format!("Endpoint: {}  ·  Model: {}", state.hybrid_local_url, state.hybrid_secondary_local_model),
        ),
        (
            "3. Change Local LLM Model",
            format!("Current: {} (Press 3 to cycle)", state.hybrid_secondary_local_model),
        ),
        (
            "4. Switch Endpoint URL",
            format!("Current: {} (llama-server / Ollama)", state.hybrid_local_url),
        ),
    ];

    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "    Local LLM Assistant",
        Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (i, (title, desc)) in options.iter().enumerate() {
        let is_selected = i == state.selected_local_idx;
        let bullet = if is_selected { "● " } else { "  " };

        let title_style = if is_selected {
            Style::default()
                .fg(Color::Rgb(105, 240, 174))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };

        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(bullet, Style::default().fg(Color::Rgb(105, 240, 174)).add_modifier(Modifier::BOLD)),
            Span::styled(*title, title_style),
        ]));

        lines.push(Line::from(vec![
            Span::raw("         "),
            Span::styled(desc.as_str(), Style::default().fg(theme.gray)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "(Press Enter or 1-3 to action · Esc to go back)",
            Style::default().fg(theme.gray),
        ),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(105, 240, 174)));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_flash_config_view(
    frame: &mut Frame,
    area: Rect,
    state: &ModelDialogState,
    theme: &Theme,
) {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "Configure Flash Settings",
            Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "Use up/down to switch row · left/right to change value · Esc to go back",
            Style::default().fg(theme.gray),
        ),
    ]));
    lines.push(Line::from(""));

    let is_temp = state.flash_row == FlashConfigRow::Temperature;
    let temp_col = get_temp_color(state.temperature);
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_temp { "▶ " } else { "  " }, Style::default().fg(theme.accent_blue)),
        Span::styled("Temperature:          ", if is_temp { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_blue)),
        Span::styled(format!("{:.1}", state.temperature), Style::default().fg(temp_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_blue)),
        Span::styled(get_temp_label(state.temperature), Style::default().fg(temp_col)),
    ]));

    let is_reasoning = state.flash_row == FlashConfigRow::ModelReasoning;
    let r_col = get_reasoning_color(&state.flash_reasoning);
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_reasoning { "▶ " } else { "  " }, Style::default().fg(theme.accent_blue)),
        Span::styled("Model Reasoning:      ", if is_reasoning { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_blue)),
        Span::styled(state.flash_reasoning.to_uppercase(), Style::default().fg(r_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_blue)),
        Span::styled(get_reasoning_label(&state.flash_reasoning), Style::default().fg(r_col)),
    ]));

    let is_cmd_r = state.flash_row == FlashConfigRow::CommandReasoning;
    let cmd_r_col = get_reasoning_color(&state.flash_command_reasoning);
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_cmd_r { "▶ " } else { "  " }, Style::default().fg(theme.accent_blue)),
        Span::styled("Command & Tool CoT:   ", if is_cmd_r { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_blue)),
        Span::styled(&state.flash_command_reasoning, Style::default().fg(cmd_r_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_blue)),
        Span::styled(get_command_reasoning_label(&state.flash_command_reasoning), Style::default().fg(theme.gray)),
    ]));

    let is_code_r = state.flash_row == FlashConfigRow::CodeReasoning;
    let code_r_col = get_reasoning_color(&state.flash_code_reasoning);
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_code_r { "▶ " } else { "  " }, Style::default().fg(theme.accent_blue)),
        Span::styled("Coding & Dev CoT:     ", if is_code_r { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_blue)),
        Span::styled(&state.flash_code_reasoning, Style::default().fg(code_r_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_blue)),
        Span::styled(get_code_reasoning_label(&state.flash_code_reasoning), Style::default().fg(theme.gray)),
    ]));

    let is_search = state.flash_row == FlashConfigRow::SearchReasoning;
    let s_col = get_reasoning_color(&state.flash_search_reasoning);
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_search { "▶ " } else { "  " }, Style::default().fg(theme.accent_blue)),
        Span::styled("Search Reasoning:     ", if is_search { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_blue)),
        Span::styled(state.flash_search_reasoning.to_uppercase(), Style::default().fg(s_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_blue)),
        Span::styled(get_search_reasoning_label(&state.flash_search_reasoning), Style::default().fg(s_col)),
    ]));

    let is_persist = state.flash_row == FlashConfigRow::Persistence;
    let p_mode = if state.flash_persist_permanent { "PERMANENT" } else { "SESSION ONLY" };
    let p_col = if state.flash_persist_permanent { Color::Rgb(105, 240, 174) } else { Color::Rgb(255, 152, 0) };
    let p_desc = if state.flash_persist_permanent { "Saved to disk (~/.uti/flash_settings.json)" } else { "Active in this conversation (resets on restart)" };
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_persist { "▶ " } else { "  " }, Style::default().fg(theme.accent_blue)),
        Span::styled("Persistence:          ", if is_persist { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_blue)),
        Span::styled(p_mode, Style::default().fg(p_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_blue)),
        Span::styled(p_desc, Style::default().fg(p_col)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("(Press Esc to go back)", Style::default().fg(theme.gray)),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_blue));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_pro_config_view(
    frame: &mut Frame,
    area: Rect,
    state: &ModelDialogState,
    theme: &Theme,
) {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "Configure Pro Settings",
            Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "Use up/down to switch row · left/right to change value · Esc to go back",
            Style::default().fg(theme.gray),
        ),
    ]));
    lines.push(Line::from(""));

    let is_reasoning = state.pro_row == ProConfigRow::ModelReasoning;
    let r_col = get_reasoning_color(&state.pro_reasoning);
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_reasoning { "▶ " } else { "  " }, Style::default().fg(theme.accent_blue)),
        Span::styled("Model Reasoning:   ", if is_reasoning { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_blue)),
        Span::styled(state.pro_reasoning.to_uppercase(), Style::default().fg(r_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_blue)),
        Span::styled(get_reasoning_label(&state.pro_reasoning), Style::default().fg(r_col)),
    ]));

    let is_search = state.pro_row == ProConfigRow::SearchReasoning;
    let s_col = get_reasoning_color(&state.pro_search_reasoning);
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_search { "▶ " } else { "  " }, Style::default().fg(theme.accent_blue)),
        Span::styled("Search Reasoning:  ", if is_search { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_blue)),
        Span::styled(state.pro_search_reasoning.to_uppercase(), Style::default().fg(s_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_blue)),
        Span::styled(get_search_reasoning_label(&state.pro_search_reasoning), Style::default().fg(s_col)),
    ]));

    let is_persist = state.pro_row == ProConfigRow::Persistence;
    let p_mode = if state.pro_persist_permanent { "PERMANENT" } else { "SESSION ONLY" };
    let p_col = if state.pro_persist_permanent { Color::Rgb(105, 240, 174) } else { Color::Rgb(255, 152, 0) };
    let p_desc = if state.pro_persist_permanent { "Saved to disk (~/.uti/pro_settings.json)" } else { "Active in this conversation (resets on restart)" };
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_persist { "▶ " } else { "  " }, Style::default().fg(theme.accent_blue)),
        Span::styled("Persistence:       ", if is_persist { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_blue)),
        Span::styled(p_mode, Style::default().fg(p_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_blue)),
        Span::styled(p_desc, Style::default().fg(p_col)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("(Press Esc to go back)", Style::default().fg(theme.gray)),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_blue));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_hybrid_config_view(
    frame: &mut Frame,
    area: Rect,
    state: &ModelDialogState,
    theme: &Theme,
) {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "Configure Hybrid Architecture (Cloud API + Local Assistant)",
            Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            "Use up/down to switch row · left/right to change options · Enter to activate",
            Style::default().fg(theme.gray),
        ),
    ]));
    lines.push(Line::from(""));

    // 1. Hybrid Mode / Strategy Row
    let is_mode = state.hybrid_row == HybridConfigRow::Mode;
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_mode { "▶ " } else { "  " }, Style::default().fg(theme.accent_cyan)),
        Span::styled("1. Hybrid Strategy:      ", if is_mode { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_cyan)),
        Span::styled(state.hybrid_mode.display_name(), Style::default().fg(Color::Rgb(105, 240, 174)).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_cyan)),
        Span::styled(state.hybrid_mode.description(), Style::default().fg(theme.gray)),
    ]));

    // 2. Primary Model Row
    let is_primary = state.hybrid_row == HybridConfigRow::PrimaryModel;
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_primary { "▶ " } else { "  " }, Style::default().fg(theme.accent_cyan)),
        Span::styled("2. Primary Cloud Model:  ", if is_primary { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_cyan)),
        Span::styled(&state.hybrid_primary_model, Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_cyan)),
        Span::styled("(Writes code & executes tools)", Style::default().fg(theme.gray)),
    ]));

    // 3. Command & Tool Reasoning Row
    let is_cmd_r = state.hybrid_row == HybridConfigRow::CommandReasoning;
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_cmd_r { "▶ " } else { "  " }, Style::default().fg(theme.accent_cyan)),
        Span::styled("3. Command & Tool CoT:   ", if is_cmd_r { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_cyan)),
        Span::styled(&state.hybrid_command_reasoning, Style::default().fg(get_reasoning_color(&state.hybrid_command_reasoning)).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_cyan)),
        Span::styled(get_command_reasoning_label(&state.hybrid_command_reasoning), Style::default().fg(theme.gray)),
    ]));

    // 4. Coding & Dev Reasoning Row
    let is_code_r = state.hybrid_row == HybridConfigRow::CodeReasoning;
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_code_r { "▶ " } else { "  " }, Style::default().fg(theme.accent_cyan)),
        Span::styled("4. Coding & Dev CoT:     ", if is_code_r { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_cyan)),
        Span::styled(&state.hybrid_code_reasoning, Style::default().fg(get_reasoning_color(&state.hybrid_code_reasoning)).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_cyan)),
        Span::styled(get_code_reasoning_label(&state.hybrid_code_reasoning), Style::default().fg(theme.gray)),
    ]));

    // 5. Secondary Local Model Row
    let is_sec = state.hybrid_row == HybridConfigRow::SecondaryLocalModel;
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_sec { "▶ " } else { "  " }, Style::default().fg(theme.accent_cyan)),
        Span::styled("5. Secondary Local LLM:  ", if is_sec { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_cyan)),
        Span::styled(&state.hybrid_secondary_local_model, Style::default().fg(Color::Rgb(105, 240, 174)).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_cyan)),
        Span::styled("(Compresses logs & answers quick queries @ $0.00)", Style::default().fg(theme.gray)),
    ]));

    // 6. Local Server URL Endpoint
    let is_url = state.hybrid_row == HybridConfigRow::LocalUrl;
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_url { "▶ " } else { "  " }, Style::default().fg(theme.accent_cyan)),
        Span::styled("6. Local Server URL:     ", if is_url { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_cyan)),
        Span::styled(&state.hybrid_local_url, Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_cyan)),
        Span::styled("(llama-server / Ollama endpoint)", Style::default().fg(theme.gray)),
    ]));

    // 7. Auto Compression
    let is_comp = state.hybrid_row == HybridConfigRow::AutoCompression;
    let comp_str = if state.hybrid_auto_compression { "ENABLED (Saves ~70% API tokens)" } else { "DISABLED (Raw logs sent to Cloud)" };
    let comp_col = if state.hybrid_auto_compression { Color::Rgb(105, 240, 174) } else { Color::Rgb(244, 67, 54) };
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_comp { "▶ " } else { "  " }, Style::default().fg(theme.accent_cyan)),
        Span::styled("7. Auto Log Compression: ", if is_comp { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_cyan)),
        Span::styled(comp_str, Style::default().fg(comp_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►", Style::default().fg(theme.accent_cyan)),
    ]));

    // 8. Persistence
    let is_persist = state.hybrid_row == HybridConfigRow::Persistence;
    let p_mode = if state.hybrid_persist_permanent { "PERMANENT" } else { "SESSION ONLY" };
    let p_col = if state.hybrid_persist_permanent { Color::Rgb(105, 240, 174) } else { Color::Rgb(255, 152, 0) };
    let p_desc = if state.hybrid_persist_permanent { "Saved to disk (~/.uti/hybrid_settings.json)" } else { "Active in this session" };
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(if is_persist { "▶ " } else { "  " }, Style::default().fg(theme.accent_cyan)),
        Span::styled("8. Persistence:          ", if is_persist { Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.gray) }),
        Span::styled("◄ ", Style::default().fg(theme.accent_cyan)),
        Span::styled(p_mode, Style::default().fg(p_col).add_modifier(Modifier::BOLD)),
        Span::styled(" ►  ", Style::default().fg(theme.accent_cyan)),
        Span::styled(p_desc, Style::default().fg(p_col)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("(Press Enter to Activate Hybrid Mode · Esc to go back)", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent_cyan));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}
