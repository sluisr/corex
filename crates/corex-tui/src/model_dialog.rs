use corex_core::config::{Config, FlashSettings, ProSettings};
use corex_core::providers::{locate_active, ModelFamily, ModelProfile, ProviderConfig};
use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};
use ratatui::Frame;

use crate::overlay::{begin_modal, key_hints, panel_rules};
use crate::text::{fit_text, pad_right};
use crate::theme::Theme;

pub const TEMPERATURE_PRESETS: &[f32] = &[0.0, 0.1, 0.2, 0.3, 0.5, 0.7, 1.0, 1.2, 1.5, 2.0];
/// DeepSeek rejects `medium` (the client turns it into `high`), so it is not offered.
pub const REASONING_LEVELS: &[&str] = &["dynamic", "low", "high", "max"];
pub const PRO_REASONING_LEVELS: &[&str] = &["max", "high", "low"];


pub fn get_temp_info(temp: f32) -> (&'static str, &'static str, Color) {
    if temp <= 0.25 {
        ("Precise", "Exact, reproducible code", Color::Rgb(79, 195, 247))
    } else if temp <= 0.65 {
        ("Balanced", "Balanced code synthesis", Color::Rgb(105, 240, 174))
    } else if temp <= 1.05 {
        ("Default", "Everyday coding & discovery", Color::Rgb(255, 213, 79))
    } else {
        ("Creative", "Exploratory brainstorming", Color::Rgb(255, 152, 0))
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

/// Widest the dialog grows on a big terminal: the extra room goes to the setting descriptions.
const MAX_DIALOG_WIDTH: u16 = 110;
/// Width of the providers column, sized for the longest built-in names ("GitHub Models (Free)").
const PROVIDERS_COL_WIDTH: u16 = 28;

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
    Persist,
}

/// What the app must do after a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogAction {
    None,
    Close,
    /// Make `provider`/`model` (indices into the dialog's provider list) the active model.
    Activate { provider: usize, model: usize },
    /// Replace the API key of `provider` (index into the dialog's provider list).
    EditKey { provider: usize },
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

    // Pro family
    pub pro_reasoning: String,

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

            pro_reasoning: "max".to_string(),

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
                let key = cfg.provider_api_keys.get(&p.name).cloned().filter(|k| !k.trim().is_empty()).unwrap_or_else(|| p.api_key());
                self.ready.push(!key.trim().is_empty());
                self.hints.push(if key.is_empty() {
                    format!("export {}=…  to enable", p.api_key_env)
                } else {
                    format!("key: {} (configured)", mask_key(&key))
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

        self.pro_reasoning = cfg.pro_settings.reasoning_effort.clone();

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
                SettingRow::Persist,
            ],
            ModelFamily::Pro => vec![SettingRow::Reasoning, SettingRow::Persist],
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
            SettingRow::Persist => self.persist = !self.persist,
        }
    }

    pub fn to_flash_settings(&self) -> FlashSettings {
        FlashSettings {
            temperature: self.temperature,
            reasoning_effort: self.flash_reasoning.clone(),
        }
    }

    pub fn to_pro_settings(&self) -> ProSettings {
        ProSettings {
            reasoning_effort: self.pro_reasoning.clone(),
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

    /// Provider indices in the order the list shows them: the active one, then the ones ready to
    /// use, then the ones still missing a key. Indices stay the real ones, so selection and
    /// activation never depend on the display.
    pub fn provider_order(&self) -> Vec<usize> {
        let rank = |i: usize| {
            if i == self.active_provider {
                0
            } else if self.ready.get(i).copied().unwrap_or(false) {
                1
            } else {
                2
            }
        };
        let mut order: Vec<usize> = (0..self.providers.len()).collect();
        order.sort_by_key(|&i| rank(i));
        order
    }

    /// Moves the provider selection one row up or down the displayed list.
    fn step_provider(&mut self, down: bool) {
        let order = self.provider_order();
        let Some(pos) = order.iter().position(|&i| i == self.prov_idx) else { return };
        let next = if down { order.get(pos + 1) } else { pos.checked_sub(1).and_then(|p| order.get(p)) };
        if let Some(&idx) = next {
            self.select_provider(idx);
        }
    }

    pub fn handle_key(&mut self, code: KeyCode) -> DialogAction {
        match code {
            KeyCode::Esc => return DialogAction::Close,
            KeyCode::Tab => self.pane = self.pane.next(),
            KeyCode::BackTab => self.pane = self.pane.prev(),
            KeyCode::Char('e') | KeyCode::Char('E') => {
                if self.selected_provider().is_some_and(|p| !p.is_local()) {
                    return DialogAction::EditKey { provider: self.prov_idx };
                }
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                self.persist = !self.persist;
                return DialogAction::SettingsChanged;
            }
            KeyCode::Up | KeyCode::Char('k') => match self.pane {
                Pane::Providers => self.step_provider(false),
                Pane::Models => {
                    self.model_idx = self.model_idx.saturating_sub(1);
                    self.row_idx = 0;
                }
                Pane::Settings => self.row_idx = self.row_idx.saturating_sub(1),
            },
            KeyCode::Down | KeyCode::Char('j') => match self.pane {
                Pane::Providers => self.step_provider(true),
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

/// A section label: quiet by default, brighter while its pane has the keyboard.
fn pane_title(label: &str, focused: bool, theme: &Theme) -> Line<'static> {
    let fg = if focused { theme.text_strong } else { theme.dark_gray };
    Line::from(vec![Span::styled(format!("   {}", label), Style::default().fg(fg).add_modifier(Modifier::BOLD))])
}

/// The selection marker: the same rail that marks blocks elsewhere in Corex.
fn marker(selected: bool, theme: &Theme) -> Span<'static> {
    Span::styled(if selected { " ▎ " } else { "   " }, Style::default().fg(theme.accent_blue))
}

pub fn render_model_dialog(frame: &mut Frame, area: Rect, state: &ModelDialogState, theme: &Theme) {
    if !state.is_open {
        return;
    }

    let is_mobile = crate::app::is_mobile_portrait(area.width, area.height);

    let dialog_width = if is_mobile {
        area.width.saturating_sub(2).max(20)
    } else {
        MAX_DIALOG_WIDTH.min(area.width.saturating_sub(4)).max(60.min(area.width))
    };
    // As tall as what it shows, so the shortcuts sit right under the content.
    let detail_rows = detail_row_count(state);
    let body_rows = if is_mobile {
        6 + 1 + detail_rows
    } else {
        detail_rows.max(state.providers.len() + 2)
    };
    let dialog_height = ((body_rows + 2 + 2) as u16).min(area.height.saturating_sub(2));
    let x = area.width.saturating_sub(dialog_width) / 2;
    let y = area.height.saturating_sub(dialog_height) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    begin_modal(frame, area, dialog_area, 2, 1);

    // No fill, so a transparent terminal shows through; thin rules mark the top and bottom.
    let panel = Block::default().padding(Padding::new(1, 1, 1, 1));
    let inner = panel.inner(dialog_area);
    frame.render_widget(panel, dialog_area);
    panel_rules(frame, dialog_area, theme);

    let vertical = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).split(inner);
    let (body, footer_area) = (vertical[0], vertical[1]);

    if is_mobile {
        let vert_body = Layout::vertical([
            Constraint::Length(6),
            Constraint::Length(1),
            Constraint::Min(4),
        ])
        .split(body);

        render_providers_pane(frame, vert_body[0], state, theme);
        render_detail_pane(frame, vert_body[2], state, theme);
    } else {
        let horizontal = Layout::horizontal([
            Constraint::Length(PROVIDERS_COL_WIDTH),
            Constraint::Length(1),
            Constraint::Min(20),
        ])
        .split(body);

        render_providers_pane(frame, horizontal[0], state, theme);
        render_detail_pane(frame, horizontal[2], state, theme);
    }

    let next: (&'static str, &'static str) = match state.pane {
        Pane::Providers => ("→", "models"),
        Pane::Models => ("Enter", "use model"),
        Pane::Settings => ("‹ ›", "adjust"),
    };
    let pairs: Vec<(&'static str, &'static str)> = if is_mobile {
        vec![next, ("Esc", "close")]
    } else {
        vec![("↑↓", "select"), ("Tab", "pane"), next, ("E", "key"), ("T", "persist"), ("Esc", "close")]
    };
    let mut footer = vec![Span::raw("  ")];
    footer.extend(key_hints(&pairs, theme));
    frame.render_widget(Paragraph::new(vec![Line::from(""), Line::from(footer)]), footer_area);
}

/// Most settings rows any model shows (temperature, reasoning, persist).
const MAX_SETTINGS_ROWS: usize = 3;

/// Rows the detail pane needs for the largest provider and model: header, models, settings and
/// the key line, with their gaps. Sized for the worst case so the panel keeps its height (and
/// stays put) while the selection moves.
fn detail_row_count(state: &ModelDialogState) -> usize {
    let models = state.providers.iter().map(|p| p.models.len()).max().unwrap_or(1).max(1);
    2 + 1 + models + 1 + 1 + MAX_SETTINGS_ROWS + 1 + 1
}

/// The provider's name without a trailing aside such as "(Free / Ultra-Fast)", once it gets long.
fn short_name(name: &str) -> &str {
    match name.find(" (") {
        Some(cut) if name.chars().count() > 16 => &name[..cut],
        _ => name,
    }
}

fn render_providers_pane(frame: &mut Frame, area: Rect, state: &ModelDialogState, theme: &Theme) {
    let focused = state.pane == Pane::Providers;
    let w = area.width as usize;
    let mut lines = vec![pane_title("Providers", focused, theme), Line::from("")];

    for i in state.provider_order() {
        let p = &state.providers[i];
        let selected = i == state.prov_idx;
        let is_active = i == state.active_provider;
        let ready = state.ready.get(i).copied().unwrap_or(false);

        // Usable providers carry a dot; the ones still missing a key simply recede.
        let (dot, dot_col) = if is_active {
            ("●", theme.accent_green)
        } else if ready {
            ("○", theme.gray)
        } else {
            (" ", theme.dark_gray)
        };
        let name_fg = if selected && focused {
            theme.text_on_selected
        } else if ready {
            theme.foreground
        } else {
            theme.dark_gray
        };
        let count = if ready { String::new() } else { "no key".to_string() };
        // Marker (3) + dot (2) + a gap before the count + the count + a trailing pad.
        let name_w = w.saturating_sub(5 + 1 + count.chars().count() + 1);

        let mut name_style = Style::default().fg(name_fg);
        if selected {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(vec![
            marker(selected && focused, theme),
            Span::styled(format!("{} ", dot), Style::default().fg(dot_col)),
            Span::styled(pad_right(short_name(p.display_name()), name_w), name_style),
            Span::styled(" ", Style::default()),
            Span::styled(count, Style::default().fg(theme.dark_gray)),
            Span::styled(" ", Style::default()),
        ]));
    }

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
        ("no key · E to add one", theme.dark_gray)
    };
    let endpoint = if provider.base_url.is_empty() { String::new() } else { provider.base_url.clone() };
    lines.push(Line::from(vec![
        Span::styled(
            format!("   {}", provider.display_name()),
            Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(status, Style::default().fg(status_col).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {}", endpoint), Style::default().fg(theme.dark_gray)),
    ]));
    lines.push(Line::from(""));

    // Models
    let models_focused = state.pane == Pane::Models;
    lines.push(pane_title("Models", models_focused, theme));
    for (i, m) in provider.models.iter().enumerate() {
        let selected = i == state.model_idx;
        let is_active = is_active_provider && i == state.active_model;

        let radio = if is_active { "● " } else { "○ " };
        let radio_col = if is_active { theme.accent_green } else { theme.dark_gray };
        let tags = m.tags.join(" · ");
        let name_w = 26usize;
        let tags_w = w.saturating_sub(3 + 2 + name_w + 1);

        let mut name_style = Style::default().fg(if selected { theme.text_on_selected } else { theme.foreground });
        if selected {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(vec![
            marker(selected && models_focused, theme),
            Span::styled(radio, Style::default().fg(radio_col)),
            Span::styled(pad_right(m.display_name(), name_w), name_style),
            Span::styled(" ", Style::default()),
            Span::styled(
                pad_right(&tags, tags_w),
                Style::default().fg(theme.dark_gray),
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
    lines.push(Line::from(vec![
        Span::styled(
            "   Settings",
            Style::default().fg(if settings_focused { theme.text_strong } else { theme.dark_gray }).add_modifier(Modifier::BOLD),
        ),
    ]));

    let family = state.selected_model().map(|m| m.family).unwrap_or_default();
    for (i, row) in state.settings_rows().iter().enumerate() {
        let selected = i == state.row_idx;
        let (label, value, val_col, desc) = setting_display(*row, family, state);

        let arrow_col = if selected && settings_focused { theme.accent_cyan } else { theme.dark_gray };
        let label_style = if selected {
            Style::default().fg(theme.text_on_selected).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray)
        };
        // Every row but the on/off one is a value picker, so only those advertise ‹ ›.
        let (left_arrow, right_arrow) = if *row == SettingRow::Persist { ("  ", "  ") } else { ("‹ ", " ›") };
        let desc_w = w.saturating_sub(3 + 16 + 2 + 7 + 2 + 2);
        lines.push(Line::from(vec![
            marker(selected && settings_focused, theme),
            Span::styled(pad_right(label, 16), label_style),
            Span::styled(left_arrow, Style::default().fg(arrow_col)),
            Span::styled(
                format!("{:^7}", value),
                Style::default().fg(val_col).add_modifier(Modifier::BOLD),
            ),
            Span::styled(right_arrow, Style::default().fg(arrow_col)),
            Span::styled("  ", Style::default()),
            Span::styled(
                pad_right(&desc, desc_w),
                Style::default().fg(if selected { theme.foreground } else { theme.dark_gray }),
            ),
        ]));
    }

    // Credentials / endpoint hint
    lines.push(Line::from(""));
    let hint = state.hints.get(state.prov_idx).cloned().unwrap_or_default();
    lines.push(Line::from(Span::styled(
        format!("   {}", fit_text(&hint, w.saturating_sub(4))),
        Style::default().fg(theme.dark_gray),
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
                ModelFamily::Flash if r == "dynamic" => "Adaptive: light on tool results, deep otherwise",
                ModelFamily::Flash => "Same depth on every turn",
                ModelFamily::Pro => "Exhaustive, for novel architecture",
                ModelFamily::Generic => "Model reasoning level",
            };
            let shown = if r.is_empty() { "default".to_string() } else { r.to_uppercase() };
            ("Reasoning", shown, get_reasoning_color(r), desc.to_string())
        }
        SettingRow::Persist => (
            "Save permanently",
            if state.persist { "[x]".to_string() } else { "[ ]".to_string() },
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
    fn e_edits_the_key_of_any_non_local_provider() {
        let mut s = open_default();
        assert_eq!(s.handle_key(KeyCode::Char('e')), DialogAction::EditKey { provider: s.prov_idx });
        let local = s.providers.iter().position(|p| p.is_local()).expect("local provider");
        s.prov_idx = local;
        assert_eq!(s.handle_key(KeyCode::Char('e')), DialogAction::None);
    }

    #[test]
    fn dialog_is_a_transparent_panel_without_clipped_names() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let theme = Theme::default();
        let mut s = open_default();
        s.is_open = true;
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|f| render_model_dialog(f, f.area(), &s, &theme)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect();
        let screen = rows.join("\n");

        assert!(!screen.contains(['┌', '┐', '└', '┘', '┬', '┴', '├', '┤', '│']), "no box, only rules:\n{screen}");
        assert_eq!(rows.iter().filter(|r| r.contains("────")).count(), 2, "a thin rule above and below:\n{screen}");
        assert!(screen.contains("Providers") && screen.contains("Models") && screen.contains("Settings"));
        assert!(screen.contains("▎ ● DeepSeek"), "the active provider is selected with the rail:\n{screen}");
        assert_eq!(screen.matches('▎').count(), 1, "only the focused pane shows the rail:\n{screen}");
        let height = rows.iter().rposition(|r| r.contains("────")).unwrap() - rows.iter().position(|r| r.contains("────")).unwrap();
        assert!(height < 22, "the panel is as tall as its content, not a fixed 22 rows: {height}");
        assert!(screen.contains("Enter use model") || screen.contains("→ models"), "{screen}");
        assert!(screen.contains("Esc close"));
        assert!(screen.contains("DeepSeek-V4.1-Flash"));
        assert!(!screen.contains("Flash-1017"), "Flash is listed once");
        // Long names drop their aside ("(Free)") and never run into the "no key" column.
        for line in screen.lines().filter(|l| l.contains("GitHub Models")) {
            assert!(line.contains("GitHub Models  ") && !line.contains("(Free)"), "{line}");
        }

        let y = rows.iter().position(|r| r.contains("Providers")).unwrap() as u16;
        let x = rows[y as usize].find("Providers").unwrap() as u16;
        assert_eq!(buf[(x, y)].bg, Color::Reset, "no fill: a transparent terminal shows through");
    }

    #[test]
    fn arrows_follow_the_displayed_order_of_providers() {
        let mut s = open_default();
        s.pane = Pane::Providers;
        // Only the last provider has a key besides the active one: it is listed right after it.
        let last = s.providers.len() - 1;
        s.ready = (0..s.providers.len()).map(|i| i == s.active_provider || i == last).collect();
        let order = s.provider_order();
        assert_eq!(order[0], s.active_provider);
        assert_eq!(order[1], last);

        s.prov_idx = s.active_provider;
        s.handle_key(KeyCode::Down);
        assert_eq!(s.prov_idx, last, "Down goes to the row shown below, not to the next index");
        s.handle_key(KeyCode::Up);
        assert_eq!(s.prov_idx, s.active_provider);
    }

    #[test]
    fn the_panel_keeps_its_place_while_the_selection_moves() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let theme = Theme::default();
        let mut s = open_default();
        s.is_open = true;
        s.pane = Pane::Models;
        let top_rule = |s: &ModelDialogState| {
            let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
            terminal.draw(|f| render_model_dialog(f, f.area(), s, &theme)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..buf.area.height).find(|&y| (0..buf.area.width).any(|x| buf[(x, y)].symbol() == "─")).unwrap()
        };
        // Flash has three settings rows, Pro two: the panel must not shrink and recentre.
        let flash = top_rule(&s);
        s.handle_key(KeyCode::Down);
        assert_eq!(s.settings_rows().len(), 2, "the second model has fewer settings");
        assert_eq!(top_rule(&s), flash);
        s.pane = Pane::Providers;
        for _ in 0..s.providers.len() {
            s.handle_key(KeyCode::Down);
            assert_eq!(top_rule(&s), flash, "provider {}", s.prov_idx);
        }
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
        assert_eq!(
            s.settings_rows(),
            vec![SettingRow::Temperature, SettingRow::Reasoning, SettingRow::Persist]
        ); // flash
        s.model_idx = 1; // pro
        assert_eq!(s.settings_rows(), vec![SettingRow::Reasoning, SettingRow::Persist]);
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
