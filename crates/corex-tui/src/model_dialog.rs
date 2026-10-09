use corex_core::config::{Config, FlashSettings, ProSettings};
use corex_core::providers::{locate_active, ModelFamily, ModelProfile, ProviderConfig};
use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::overlay::begin_modal;
use crate::theme::Theme;

pub const TEMPERATURE_PRESETS: &[f32] = &[0.0, 0.1, 0.2, 0.3, 0.5, 0.7, 1.0, 1.2, 1.5, 2.0];
pub const REASONING_LEVELS: &[&str] = &["dynamic", "low", "medium", "high"];
pub const COMMAND_REASONING_LEVELS: &[&str] = &["low", "medium", "high"];
pub const CODE_REASONING_LEVELS: &[&str] = &["high", "max", "medium", "low"];
pub const SEARCH_REASONING_LEVELS: &[&str] = &["low", "medium", "high", "max"];
pub const PRO_REASONING_LEVELS: &[&str] = &["max", "high", "medium", "low"];

const SELECTED_BG: Color = Color::Rgb(26, 38, 58);
const RULE: Color = Color::Rgb(35, 45, 60);

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

/// Which of the three panes has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Providers,
    Models,
    Settings,
}

impl Pane {
    fn next(self) -> Self {
        match self {
            Self::Providers => Self::Models,
            Self::Models => Self::Settings,
            Self::Settings => Self::Providers,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Providers => Self::Settings,
            Self::Models => Self::Providers,
            Self::Settings => Self::Models,
        }
    }
}

/// A tunable row in the settings pane. Which rows exist depends on the model's family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingRow {
    Temperature,
    Reasoning,
    CommandCot,
    CodeCot,
    SearchCot,
    Persist,
}

/// What the app must do after a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogAction {
    None,
    Close,
    /// Make `provider`/`model` (indices into the dialog's provider list) the active model.
    Activate { provider: usize, model: usize },
    /// A setting value changed; the app should apply (and optionally persist) it.
    SettingsChanged,
}

#[derive(Debug, Clone)]
pub struct ModelDialogState {
    pub is_open: bool,
    pub pane: Pane,
    pub providers: Vec<ProviderConfig>,
    /// Per provider: can it be used right now (key present / local)?
    pub ready: Vec<bool>,
    /// Per provider: short human hint about credentials / endpoint.
    pub hints: Vec<String>,
    pub prov_idx: usize,
    pub model_idx: usize,
    pub row_idx: usize,
    pub active_provider: usize,
    pub active_model: usize,

    /// Persist activation and setting changes to disk (vs. this session only).
    pub persist: bool,

    // Flash family
    pub temperature: f32,
    pub flash_reasoning: String,
    pub flash_command_reasoning: String,
    pub flash_code_reasoning: String,
    pub flash_search_reasoning: String,

    // Pro family
    pub pro_reasoning: String,
    pub pro_search_reasoning: String,

    // Generic family (OpenAI-compatible providers, local)
    pub generic_temperature: f32,
    pub generic_reasoning: String,

    pub local_prompt_lite: bool,
}

impl Default for ModelDialogState {
    fn default() -> Self {
        Self::new()
    }
}

fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n == 0 {
        return String::new();
    }
    let tail: String = key.chars().skip(n.saturating_sub(4)).collect();
    format!("••••{}", tail)
}

fn cycle_str(levels: &[String], current: &str, forward: bool) -> String {
    if levels.is_empty() {
        return current.to_string();
    }
    let cur = levels.iter().position(|l| l == current).unwrap_or(0);
    let next = if forward {
        (cur + 1) % levels.len()
    } else if cur == 0 {
        levels.len() - 1
    } else {
        cur - 1
    };
    levels[next].clone()
}

fn cycle_static(levels: &[&str], current: &str, forward: bool) -> String {
    let owned: Vec<String> = levels.iter().map(|s| s.to_string()).collect();
    cycle_str(&owned, current, forward)
}

fn cycle_temperature(current: f32, forward: bool) -> f32 {
    let cur = TEMPERATURE_PRESETS
        .iter()
        .position(|&t| (t - current).abs() < 0.01)
        .unwrap_or(6);
    let next = if forward {
        (cur + 1) % TEMPERATURE_PRESETS.len()
    } else if cur == 0 {
        TEMPERATURE_PRESETS.len() - 1
    } else {
        cur - 1
    };
    TEMPERATURE_PRESETS[next]
}

impl ModelDialogState {
    pub fn new() -> Self {
        Self {
            is_open: false,
            pane: Pane::Models,
            providers: Vec::new(),
            ready: Vec::new(),
            hints: Vec::new(),
            prov_idx: 0,
            model_idx: 0,
            row_idx: 0,
            active_provider: 0,
            active_model: 0,
            persist: true,

            temperature: 1.0,
            flash_reasoning: "dynamic".to_string(),
            flash_command_reasoning: "low".to_string(),
            flash_code_reasoning: "high".to_string(),
            flash_search_reasoning: "low".to_string(),

            pro_reasoning: "max".to_string(),
            pro_search_reasoning: "low".to_string(),

            generic_temperature: 1.0,
            generic_reasoning: String::new(),

            local_prompt_lite: false,
        }
    }

    pub fn open(&mut self, cfg: &Config) {
        self.is_open = true;
        self.pane = Pane::Models;
        self.local_prompt_lite = cfg.local_prompt_lite;

        self.providers = cfg.all_providers();
        self.ready = Vec::with_capacity(self.providers.len());
        self.hints = Vec::with_capacity(self.providers.len());
        for p in &self.providers {
            if p.is_local() {
                self.ready.push(true);
                self.hints.push(format!("endpoint: {}", cfg.local_llm_url));
            } else if p.is_deepseek() {
                let key = if cfg.api_key.is_empty() { p.api_key() } else { cfg.api_key.clone() };
                self.ready.push(!key.trim().is_empty());
                self.hints.push(if key.is_empty() {
                    format!("export {}=…", p.api_key_env)
                } else {
                    format!("key: {}", mask_key(&key))
                });
            } else {
                let key = p.api_key();
                self.ready.push(!key.trim().is_empty());
                self.hints.push(if key.is_empty() {
                    format!("export {}=…  to enable", p.api_key_env)
                } else {
                    format!("key: {} (env {})", mask_key(&key), p.api_key_env)
                });
            }
        }

        let (pi, mi) = locate_active(
            &self.providers,
            cfg.active_provider.as_deref(),
            &cfg.model,
            cfg.local_llm_enabled,
        );
        self.active_provider = pi;
        self.active_model = mi;
        self.prov_idx = pi;
        self.model_idx = mi;
        self.row_idx = 0;

        self.temperature = cfg.flash_settings.temperature;
        self.flash_reasoning = cfg.flash_settings.reasoning_effort.clone();
        self.flash_command_reasoning = cfg.flash_settings.command_reasoning_effort.clone();
        self.flash_code_reasoning = cfg.flash_settings.code_reasoning_effort.clone();
        self.flash_search_reasoning = cfg.flash_settings.search_reasoning_effort.clone();

        self.pro_reasoning = cfg.pro_settings.reasoning_effort.clone();
        self.pro_search_reasoning = cfg.pro_settings.search_reasoning_effort.clone();

        self.generic_temperature = cfg.temperature;
        self.generic_reasoning = if cfg.is_deepseek_endpoint() && !cfg.local_llm_enabled {
            String::new()
        } else {
            cfg.reasoning_effort.clone()
        };
    }

    pub fn selected_provider(&self) -> Option<&ProviderConfig> {
        self.providers.get(self.prov_idx)
    }

    pub fn selected_model(&self) -> Option<&ModelProfile> {
        self.selected_provider().and_then(|p| p.models.get(self.model_idx))
    }

    /// True when the highlighted model is the one currently running.
    pub fn selection_is_active(&self) -> bool {
        self.prov_idx == self.active_provider && self.model_idx == self.active_model
    }

    pub fn settings_rows(&self) -> Vec<SettingRow> {
        let Some(m) = self.selected_model() else { return vec![SettingRow::Persist] };
        match m.family {
            ModelFamily::Flash => vec![
                SettingRow::Temperature,
                SettingRow::Reasoning,
                SettingRow::CommandCot,
                SettingRow::CodeCot,
                SettingRow::SearchCot,
                SettingRow::Persist,
            ],
            ModelFamily::Pro => vec![SettingRow::Reasoning, SettingRow::SearchCot, SettingRow::Persist],
            ModelFamily::Generic => {
                let mut rows = vec![SettingRow::Temperature];
                if !m.reasoning.is_empty() {
                    rows.push(SettingRow::Reasoning);
                }
                rows.push(SettingRow::Persist);
                rows
            }
        }
    }

    fn current_temperature(&self) -> f32 {
        match self.selected_model().map(|m| m.family) {
            Some(ModelFamily::Flash) => self.temperature,
            _ => self.generic_temperature,
        }
    }

    fn current_reasoning(&self) -> String {
        match self.selected_model().map(|m| m.family) {
            Some(ModelFamily::Flash) => self.flash_reasoning.clone(),
            Some(ModelFamily::Pro) => self.pro_reasoning.clone(),
            _ => self.generic_reasoning.clone(),
        }
    }

    pub fn adjust_row(&mut self, forward: bool) {
        let rows = self.settings_rows();
        let Some(row) = rows.get(self.row_idx).copied() else { return };
        let family = self.selected_model().map(|m| m.family).unwrap_or_default();
        match row {
            SettingRow::Temperature => {
                if family == ModelFamily::Flash {
                    self.temperature = cycle_temperature(self.temperature, forward);
                } else {
                    self.generic_temperature = cycle_temperature(self.generic_temperature, forward);
                }
            }
            SettingRow::Reasoning => match family {
                ModelFamily::Flash => {
                    self.flash_reasoning = cycle_static(REASONING_LEVELS, &self.flash_reasoning, forward)
                }
                ModelFamily::Pro => {
                    self.pro_reasoning = cycle_static(PRO_REASONING_LEVELS, &self.pro_reasoning, forward)
                }
                ModelFamily::Generic => {
                    let levels = self.selected_model().map(|m| m.reasoning.clone()).unwrap_or_default();
                    self.generic_reasoning = cycle_str(&levels, &self.generic_reasoning, forward);
                }
            },
            SettingRow::CommandCot => {
                self.flash_command_reasoning =
                    cycle_static(COMMAND_REASONING_LEVELS, &self.flash_command_reasoning, forward)
            }
            SettingRow::CodeCot => {
                self.flash_code_reasoning =
                    cycle_static(CODE_REASONING_LEVELS, &self.flash_code_reasoning, forward)
            }
            SettingRow::SearchCot => {
                if family == ModelFamily::Pro {
                    self.pro_search_reasoning =
                        cycle_static(SEARCH_REASONING_LEVELS, &self.pro_search_reasoning, forward)
                } else {
                    self.flash_search_reasoning =
                        cycle_static(SEARCH_REASONING_LEVELS, &self.flash_search_reasoning, forward)
                }
            }
            SettingRow::Persist => self.persist = !self.persist,
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

    /// `(temperature, reasoning_effort)` the highlighted model would run with.
    pub fn effective_params(&self) -> (f32, String) {
        (self.current_temperature(), self.current_reasoning())
    }

    fn select_provider(&mut self, idx: usize) {
        self.prov_idx = idx;
        self.model_idx = if idx == self.active_provider { self.active_model } else { 0 };
        self.row_idx = 0;
    }

    pub fn handle_key(&mut self, code: KeyCode) -> DialogAction {
        match code {
            KeyCode::Esc => return DialogAction::Close,
            KeyCode::Tab => self.pane = self.pane.next(),
            KeyCode::BackTab => self.pane = self.pane.prev(),
            KeyCode::Char('t') | KeyCode::Char('T') => {
                self.persist = !self.persist;
                return DialogAction::SettingsChanged;
            }
            KeyCode::Up | KeyCode::Char('k') => match self.pane {
                Pane::Providers => {
                    if self.prov_idx > 0 {
                        self.select_provider(self.prov_idx - 1);
                    }
                }
                Pane::Models => {
                    self.model_idx = self.model_idx.saturating_sub(1);
                    self.row_idx = 0;
                }
                Pane::Settings => self.row_idx = self.row_idx.saturating_sub(1),
            },
            KeyCode::Down | KeyCode::Char('j') => match self.pane {
                Pane::Providers => {
                    if self.prov_idx + 1 < self.providers.len() {
                        self.select_provider(self.prov_idx + 1);
                    }
                }
                Pane::Models => {
                    let n = self.selected_provider().map(|p| p.models.len()).unwrap_or(0);
                    if self.model_idx + 1 < n {
                        self.model_idx += 1;
                        self.row_idx = 0;
                    }
                }
                Pane::Settings => {
                    let n = self.settings_rows().len();
                    if self.row_idx + 1 < n {
                        self.row_idx += 1;
                    }
                }
            },
            KeyCode::Left | KeyCode::Char('h') => match self.pane {
                Pane::Providers => {}
                Pane::Models => self.pane = Pane::Providers,
                Pane::Settings => {
                    self.adjust_row(false);
                    return DialogAction::SettingsChanged;
                }
            },
            KeyCode::Right | KeyCode::Char('l') => match self.pane {
                Pane::Providers => self.pane = Pane::Models,
                Pane::Models => self.pane = Pane::Settings,
                Pane::Settings => {
                    self.adjust_row(true);
                    return DialogAction::SettingsChanged;
                }
            },
            KeyCode::Enter => match self.pane {
                Pane::Providers => self.pane = Pane::Models,
                Pane::Models => {
                    if self.selected_model().is_some() {
                        return DialogAction::Activate { provider: self.prov_idx, model: self.model_idx };
                    }
                }
                Pane::Settings => {
                    self.adjust_row(true);
                    return DialogAction::SettingsChanged;
                }
            },
            _ => {}
        }
        DialogAction::None
    }

    /// Marks `provider`/`model` as the running one (after the app applied it).
    pub fn mark_active(&mut self, provider: usize, model: usize) {
        self.active_provider = provider;
        self.active_model = model;
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
        Span::raw("  "),
    ]
}

fn pad_right(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w {
        s.chars().take(w).collect()
    } else {
        format!("{}{}", s, " ".repeat(w - n))
    }
}

fn pane_title(label: &str, focused: bool, theme: &Theme) -> Line<'static> {
    let style = if focused {
        Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.dark_gray).add_modifier(Modifier::BOLD)
    };
    Line::from(vec![Span::styled(format!(" {}", label), style)])
}

pub fn render_model_dialog(frame: &mut Frame, area: Rect, state: &ModelDialogState, theme: &Theme) {
    if !state.is_open {
        return;
    }

    let dialog_width = 100.min(area.width.saturating_sub(4)).max(60.min(area.width));
    let dialog_height = 22.min(area.height.saturating_sub(2));
    let x = area.width.saturating_sub(dialog_width) / 2;
    let y = area.height.saturating_sub(dialog_height) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    begin_modal(frame, area, dialog_area, 2, 1);

    let block = Block::default()
        .title("  Models  ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent_blue));
    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    let vertical = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).split(inner);
    let (body, footer_area) = (vertical[0], vertical[1]);
    let horizontal = Layout::horizontal([
        Constraint::Length(26),
        Constraint::Length(1),
        Constraint::Min(20),
    ])
    .split(body);

    render_providers_pane(frame, horizontal[0], state, theme);
    let sep: Vec<Line> = (0..horizontal[1].height)
        .map(|_| Line::from(Span::styled("│", Style::default().fg(RULE))))
        .collect();
    frame.render_widget(Paragraph::new(sep), horizontal[1]);
    render_detail_pane(frame, horizontal[2], state, theme);

    let mut footer = vec![Span::raw(" ")];
    footer.extend(key_badge("↑/↓", "Select", theme.accent_blue, theme.gray));
    footer.extend(key_badge("Tab", "Pane", theme.accent_cyan, theme.gray));
    match state.pane {
        Pane::Providers => footer.extend(key_badge("→", "Models", theme.accent_green, theme.gray)),
        Pane::Models => footer.extend(key_badge("Enter", "Use model", theme.accent_green, theme.gray)),
        Pane::Settings => footer.extend(key_badge("◄/►", "Adjust", theme.accent_green, theme.gray)),
    }
    footer.extend(key_badge("T", "Persist", theme.accent_yellow, theme.gray));
    footer.extend(key_badge("Esc", "Close", theme.gray, theme.gray));
    let footer_lines = vec![
        Line::from(Span::styled("─".repeat(footer_area.width as usize), Style::default().fg(RULE))),
        Line::from(footer),
    ];
    frame.render_widget(Paragraph::new(footer_lines), footer_area);
}

fn render_providers_pane(frame: &mut Frame, area: Rect, state: &ModelDialogState, theme: &Theme) {
    let focused = state.pane == Pane::Providers;
    let w = area.width as usize;
    let mut lines = vec![pane_title("PROVIDERS", focused, theme), Line::from("")];

    for (i, p) in state.providers.iter().enumerate() {
        let selected = i == state.prov_idx;
        let is_active = i == state.active_provider;
        let ready = state.ready.get(i).copied().unwrap_or(false);

        let (dot, dot_col) = if is_active {
            ("●", theme.accent_green)
        } else if ready {
            ("○", theme.gray)
        } else {
            ("◌", theme.accent_yellow)
        };
        let bg = if selected && focused { SELECTED_BG } else { Color::Reset };
        let name_fg = if selected { Color::White } else { theme.foreground };
        let count = format!("{}", p.models.len());
        let name_w = w.saturating_sub(5 + count.chars().count() + 1);

        let mut name_style = Style::default().fg(name_fg).bg(bg);
        if selected {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(vec![
            Span::styled(
                if selected { " ❯ " } else { "   " },
                Style::default().fg(theme.accent_blue).bg(bg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{} ", dot), Style::default().fg(dot_col).bg(bg)),
            Span::styled(pad_right(p.display_name(), name_w), name_style),
            Span::styled(count, Style::default().fg(theme.dark_gray).bg(bg)),
            Span::styled(" ", Style::default().bg(bg)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" ● ", Style::default().fg(theme.accent_green)),
        Span::styled("active ", Style::default().fg(theme.dark_gray)),
        Span::styled("○ ", Style::default().fg(theme.gray)),
        Span::styled("ready", Style::default().fg(theme.dark_gray)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" ◌ ", Style::default().fg(theme.accent_yellow)),
        Span::styled("needs API key", Style::default().fg(theme.dark_gray)),
    ]));

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_detail_pane(frame: &mut Frame, area: Rect, state: &ModelDialogState, theme: &Theme) {
    let Some(provider) = state.selected_provider() else { return };
    let w = area.width as usize;
    let ready = state.ready.get(state.prov_idx).copied().unwrap_or(false);
    let is_active_provider = state.prov_idx == state.active_provider;
    let mut lines: Vec<Line> = Vec::new();

    // Header: provider + status + endpoint
    let (status, status_col) = if is_active_provider {
        ("● active", theme.accent_green)
    } else if ready {
        ("○ ready", theme.gray)
    } else {
        ("◌ needs API key", theme.accent_yellow)
    };
    let endpoint = if provider.base_url.is_empty() { String::new() } else { provider.base_url.clone() };
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {}", provider.display_name()),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(status, Style::default().fg(status_col).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {}", endpoint), Style::default().fg(theme.dark_gray)),
    ]));
    lines.push(Line::from(Span::styled(
        "─".repeat(w.saturating_sub(1)),
        Style::default().fg(RULE),
    )));

    // Models
    let models_focused = state.pane == Pane::Models;
    lines.push(pane_title("MODELS", models_focused, theme));
    for (i, m) in provider.models.iter().enumerate() {
        let selected = i == state.model_idx;
        let is_active = is_active_provider && i == state.active_model;
        let bg = if selected && models_focused { SELECTED_BG } else { Color::Reset };

        let radio = if is_active { "(●) " } else { "( ) " };
        let radio_col = if is_active { theme.accent_green } else { theme.dark_gray };
        let tags = m.tags.join(" · ");
        let name_w = 26usize;
        let tags_w = w.saturating_sub(3 + 4 + name_w + 1);

        let mut name_style = Style::default().fg(if selected { Color::White } else { theme.foreground }).bg(bg);
        if selected {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(vec![
            Span::styled(
                if selected { " ❯ " } else { "   " },
                Style::default().fg(theme.accent_blue).bg(bg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(radio, Style::default().fg(radio_col).bg(bg).add_modifier(Modifier::BOLD)),
            Span::styled(pad_right(m.display_name(), name_w), name_style),
            Span::styled(" ", Style::default().bg(bg)),
            Span::styled(
                pad_right(&corex_core::truncate_ellipsis(&tags, tags_w), tags_w),
                Style::default().fg(theme.dark_gray).bg(bg),
            ),
        ]));
    }
    if provider.models.is_empty() {
        lines.push(Line::from(Span::styled(
            "   (no models defined — add them under `providers` in settings.json)",
            Style::default().fg(theme.dark_gray),
        )));
    }

    // Settings
    lines.push(Line::from(""));
    let settings_focused = state.pane == Pane::Settings;
    let model_name = state.selected_model().map(|m| m.display_name().to_string()).unwrap_or_default();
    lines.push(Line::from(vec![
        Span::styled(
            " SETTINGS",
            if settings_focused {
                Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.dark_gray).add_modifier(Modifier::BOLD)
            },
        ),
        Span::styled(format!(" · {}", model_name), Style::default().fg(theme.dark_gray)),
    ]));

    let family = state.selected_model().map(|m| m.family).unwrap_or_default();
    for (i, row) in state.settings_rows().iter().enumerate() {
        let selected = i == state.row_idx;
        let bg = if selected && settings_focused { SELECTED_BG } else { Color::Reset };
        let (label, value, val_col, desc) = setting_display(*row, family, state);

        let arrow_col = if selected && settings_focused { theme.accent_cyan } else { theme.dark_gray };
        let label_style = if selected {
            Style::default().fg(Color::White).bg(bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray)
        };
        let desc_w = w.saturating_sub(3 + 20 + 2 + 11 + 2 + 2);
        lines.push(Line::from(vec![
            Span::styled(
                if selected { " ❯ " } else { "   " },
                Style::default().fg(theme.accent_blue).bg(bg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(pad_right(label, 20), label_style),
            Span::styled("◄ ", Style::default().fg(arrow_col).bg(bg)),
            Span::styled(
                format!("{:^9}", value),
                Style::default().fg(val_col).bg(bg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ►", Style::default().fg(arrow_col).bg(bg)),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(
                pad_right(&corex_core::truncate_ellipsis(&desc, desc_w), desc_w),
                Style::default().fg(if selected { theme.foreground } else { theme.dark_gray }).bg(bg),
            ),
        ]));
    }

    // Credentials / endpoint hint
    lines.push(Line::from(""));
    let hint = state.hints.get(state.prov_idx).cloned().unwrap_or_default();
    lines.push(Line::from(Span::styled(
        format!(" {}", corex_core::truncate_ellipsis(&hint, w.saturating_sub(2))),
        Style::default().fg(if ready { theme.dark_gray } else { theme.accent_yellow }),
    )));

    frame.render_widget(Paragraph::new(lines), area);
}

fn setting_display(
    row: SettingRow,
    family: ModelFamily,
    state: &ModelDialogState,
) -> (&'static str, String, Color, String) {
    match row {
        SettingRow::Temperature => {
            let t = if family == ModelFamily::Flash { state.temperature } else { state.generic_temperature };
            let (tag, desc, col) = get_temp_info(t);
            ("Temperature", format!("{:.1}", t), col, format!("{} · {}", tag, desc))
        }
        SettingRow::Reasoning => {
            let r = match family {
                ModelFamily::Flash => &state.flash_reasoning,
                ModelFamily::Pro => &state.pro_reasoning,
                ModelFamily::Generic => &state.generic_reasoning,
            };
            let desc = match family {
                ModelFamily::Flash => "Adaptive · ~200ms tools, deep CoT for complex code",
                ModelFamily::Pro => "Exhaustive reasoning for novel architecture",
                ModelFamily::Generic => "Model reasoning level",
            };
            let shown = if r.is_empty() { "default".to_string() } else { r.to_uppercase() };
            ("Reasoning", shown, get_reasoning_color(r), desc.to_string())
        }
        SettingRow::CommandCot => (
            "Command & Tool CoT",
            state.flash_command_reasoning.to_uppercase(),
            get_reasoning_color(&state.flash_command_reasoning),
            "Quick checks for shell tools".to_string(),
        ),
        SettingRow::CodeCot => (
            "Coding & Dev CoT",
            state.flash_code_reasoning.to_uppercase(),
            get_reasoning_color(&state.flash_code_reasoning),
            "Verification for architecture & code".to_string(),
        ),
        SettingRow::SearchCot => {
            let r = if family == ModelFamily::Pro { &state.pro_search_reasoning } else { &state.flash_search_reasoning };
            (
                "Web Search CoT",
                r.to_uppercase(),
                get_reasoning_color(r),
                "Quick snippets & direct links".to_string(),
            )
        }
        SettingRow::Persist => (
            "Save changes",
            if state.persist { "PERMANENT".to_string() } else { "SESSION".to_string() },
            if state.persist { Color::Rgb(105, 240, 174) } else { Color::Rgb(255, 152, 0) },
            if state.persist { "Saved to ~/.corex/settings.json".to_string() } else { "Active in this session only".to_string() },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_default() -> ModelDialogState {
        let mut s = ModelDialogState::new();
        s.open(&Config::default());
        s
    }

    #[test]
    fn opens_on_active_deepseek_model() {
        let s = open_default();
        assert!(s.providers[s.active_provider].is_deepseek());
        assert!(s.selection_is_active());
    }

    #[test]
    fn enter_in_models_pane_activates_selection() {
        let mut s = open_default();
        s.pane = Pane::Models;
        s.handle_key(KeyCode::Down);
        let action = s.handle_key(KeyCode::Enter);
        assert_eq!(action, DialogAction::Activate { provider: s.prov_idx, model: 1 });
    }

    #[test]
    fn settings_rows_follow_model_family() {
        let mut s = open_default();
        assert_eq!(s.settings_rows().len(), 6); // flash
        s.model_idx = 1; // pro
        assert_eq!(s.settings_rows(), vec![SettingRow::Reasoning, SettingRow::SearchCot, SettingRow::Persist]);
        let local = s.providers.iter().position(|p| p.is_local()).unwrap();
        s.select_provider(local);
        assert_eq!(s.settings_rows(), vec![SettingRow::Temperature, SettingRow::Persist]);
    }

    #[test]
    fn right_in_settings_pane_adjusts_value() {
        let mut s = open_default();
        s.pane = Pane::Settings;
        let before = s.temperature;
        assert_eq!(s.handle_key(KeyCode::Right), DialogAction::SettingsChanged);
        assert_ne!(s.temperature, before);
    }
}
