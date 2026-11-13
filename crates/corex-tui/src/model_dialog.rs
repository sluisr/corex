use corex_core::config::{Config, FlashSettings, ProSettings};
use corex_core::providers::{locate_active, ModelFamily, ModelProfile, ProviderConfig};
use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::overlay::{docked_height, docked_panel, highlight_cursor, key_hints, rail, row_text, Row};
use crate::text::{fit_text, pad_right};
use crate::theme::Theme;

pub const TEMPERATURE_PRESETS: &[f32] = &[0.0, 0.1, 0.2, 0.3, 0.5, 0.7, 1.0, 1.2, 1.5, 2.0];
/// DeepSeek rejects `medium` (the client turns it into `high`), so it is not offered.
pub const REASONING_LEVELS: &[&str] = &["dynamic", "low", "high", "max"];
pub const PRO_REASONING_LEVELS: &[&str] = &["max", "high", "low"];

const PROVIDERS_COL_WIDTH: u16 = 24;
const COLUMN_GAP: u16 = 2;
const MODEL_NAME_WIDTH: usize = 26;
const SETTING_LABEL_WIDTH: usize = 14;
const SETTING_VALUE_WIDTH: usize = 10;
const COMPACT_BELOW: u16 = 80;
const MAX_SETTINGS_ROWS: usize = 3;
const IN_USE: &str = " ✓";

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

    /// Drawn as one column (phone, narrow terminal): the keys then walk it as a single list.
    /// Set by the renderer, which is the one that knows the width.
    pub compact: std::cell::Cell<bool>,
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

fn key_hint(key: &str, env_var: &str) -> String {
    if key.trim().is_empty() {
        format!("no key · press E or set ${env_var}")
    } else {
        format!("key {}", mask_key(key))
    }
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
            compact: std::cell::Cell::new(false),
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
                self.hints.push(cfg.local_llm_url.clone());
            } else if p.is_deepseek() {
                let key = if cfg.api_key.is_empty() { p.api_key() } else { cfg.api_key.clone() };
                self.ready.push(!key.trim().is_empty());
                self.hints.push(key_hint(&key, &p.api_key_env));
            } else {
                let key = cfg.provider_api_keys.get(&p.name).cloned().filter(|k| !k.trim().is_empty()).unwrap_or_else(|| p.api_key());
                self.ready.push(!key.trim().is_empty());
                self.hints.push(key_hint(&key, &p.api_key_env));
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
        self.normalize_generic_reasoning();
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

    /// Keeps the generic Reasoning row on a level the highlighted model actually supports, so it
    /// never displays "default" for a model that exposes a level.
    fn normalize_generic_reasoning(&mut self) {
        let Some(model) = self.selected_model() else { return };
        if model.family != ModelFamily::Generic || model.reasoning.is_empty() {
            return;
        }
        if model.reasoning.iter().any(|level| level == &self.generic_reasoning) {
            return;
        }
        let preferred = match self.generic_reasoning.trim().to_ascii_lowercase().as_str() {
            "max" | "xhigh" => "high",
            _ => "medium",
        };
        self.generic_reasoning = if model.reasoning.iter().any(|level| level.as_str() == preferred) {
            preferred.to_string()
        } else {
            model.reasoning.first().cloned().unwrap_or_default()
        };
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
        self.normalize_generic_reasoning();
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

    /// Keys of the one-column layout: ↑↓ walk provider, models and settings as one list; ←→ change
    /// the provider on its row and adjust a setting on its row. `None` leaves the key to
    /// [`Self::handle_key`]'s common handling.
    fn handle_compact_key(&mut self, code: KeyCode) -> Option<DialogAction> {
        let models = self.selected_provider().map(|p| p.models.len()).unwrap_or(0);
        match (code, self.pane) {
            (KeyCode::Up | KeyCode::Char('k'), Pane::Providers) => {}
            (KeyCode::Up | KeyCode::Char('k'), Pane::Models) if self.model_idx == 0 => self.pane = Pane::Providers,
            (KeyCode::Up | KeyCode::Char('k'), Pane::Settings) if self.row_idx == 0 => self.pane = Pane::Models,
            (KeyCode::Down | KeyCode::Char('j'), Pane::Providers) => self.pane = Pane::Models,
            (KeyCode::Down | KeyCode::Char('j'), Pane::Models) if self.model_idx + 1 >= models => {
                self.pane = Pane::Settings;
                self.row_idx = 0;
            }
            (KeyCode::Left | KeyCode::Char('h'), Pane::Providers) => self.step_provider(false),
            (KeyCode::Right | KeyCode::Char('l'), Pane::Providers) => self.step_provider(true),
            (KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l'), Pane::Models) => {}
            _ => return None,
        }
        Some(DialogAction::None)
    }

    pub fn handle_key(&mut self, code: KeyCode) -> DialogAction {
        if self.compact.get() {
            if let Some(action) = self.handle_compact_key(code) {
                return action;
            }
        }
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
                    self.normalize_generic_reasoning();
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
                        self.normalize_generic_reasoning();
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

fn is_compact(width: u16) -> bool {
    width < COMPACT_BELOW || crate::app::is_mobile_portrait(width, 0)
}

fn detail_row_count(state: &ModelDialogState) -> usize {
    let models = state.providers.iter().map(|p| p.models.len()).max().unwrap_or(1).max(1);
    models + 1 + MAX_SETTINGS_ROWS
}

fn is_keyless(state: &ModelDialogState, provider: usize) -> bool {
    provider != state.active_provider && !state.ready.get(provider).copied().unwrap_or(false)
}

fn provider_row_count(state: &ModelDialogState) -> usize {
    let group_label = if (0..state.providers.len()).any(|i| is_keyless(state, i)) { 2 } else { 0 };
    state.providers.len() + group_label
}

impl ModelDialogState {
    pub fn height(&self, width: u16, screen_height: u16) -> u16 {
        let body = if is_compact(width) {
            2 + detail_row_count(self)
        } else {
            detail_row_count(self).max(provider_row_count(self))
        };
        docked_height(body, screen_height)
    }
}

pub fn render_model_dialog(frame: &mut Frame, area: Rect, state: &ModelDialogState, theme: &Theme) {
    if !state.is_open {
        return;
    }

    let compact = is_compact(area.width);
    state.compact.set(compact);

    let inner = docked_panel(frame, area, theme);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let (title_area, body, footer_area) = (rows[0], rows[2], rows[4]);

    let title = " Model";
    let lead = if compact { title.len() + 2 } else { (PROVIDERS_COL_WIDTH + COLUMN_GAP) as usize + 3 };
    let meta_w = (title_area.width as usize).saturating_sub(lead + 1);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(pad_right(title, lead), Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD)),
            Span::styled(fit_text(&provider_meta(state), meta_w), Style::default().fg(theme.dark_gray)),
        ])),
        title_area,
    );

    if compact {
        let w = body.width as usize;
        let mut lines = vec![compact_provider_row(state, w, theme), Line::default()];
        lines.extend(detail_lines(state, w, theme));
        frame.render_widget(Paragraph::new(lines), body);
    } else {
        let detail_w = body.width.saturating_sub(PROVIDERS_COL_WIDTH + COLUMN_GAP).min(DETAIL_MAX_WIDTH);
        let columns = Layout::horizontal([
            Constraint::Length(PROVIDERS_COL_WIDTH),
            Constraint::Length(COLUMN_GAP),
            Constraint::Length(detail_w),
            Constraint::Min(0),
        ])
        .split(body);
        render_providers_pane(frame, columns[0], state, theme);
        frame.render_widget(Paragraph::new(detail_lines(state, columns[2].width as usize, theme)), columns[2]);
    }

    let next = match (state.pane, compact) {
        (Pane::Providers, true) => ("←→", "provider"),
        (Pane::Providers, false) => ("→", "models"),
        (Pane::Models, _) => ("Enter", "use"),
        (Pane::Settings, _) => ("←→", "adjust"),
    };
    let mut pairs = vec![("↑↓", "select")];
    if !compact {
        pairs.push(("Tab", "pane"));
    }
    pairs.push(next);
    if !compact && state.selected_provider().is_some_and(|p| !p.is_local()) {
        pairs.push(("E", "key"));
    }
    pairs.push(("Esc", "close"));
    let mut footer = vec![Span::raw(" ")];
    footer.extend(key_hints(&pairs, theme));
    frame.render_widget(Paragraph::new(Line::from(footer)), footer_area);
}

const DETAIL_MAX_WIDTH: u16 = 84;

fn provider_meta(state: &ModelDialogState) -> String {
    let Some(provider) = state.selected_provider() else { return String::new() };
    let endpoint = if provider.is_local() {
        ""
    } else {
        provider.base_url.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/')
    };
    let hint = state.hints.get(state.prov_idx).map(String::as_str).unwrap_or_default();
    [endpoint, hint].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// The provider's name without a trailing aside such as "(Free / Ultra-Fast)", once it gets long.
fn short_name(name: &str) -> &str {
    match name.find(" (") {
        Some(cut) if name.chars().count() > 16 => &name[..cut],
        _ => name,
    }
}

fn name_cell(name: &str, width: usize, in_use: bool, style: Style, theme: &Theme) -> Vec<Span<'static>> {
    let name = Span::styled(fit_text(name, width), style);
    let mark = Span::styled(if in_use { IN_USE } else { "" }, Style::default().fg(theme.success));
    let pad = (width + 2).saturating_sub(name.width() + mark.width());
    vec![name, mark, Span::raw(" ".repeat(pad))]
}

fn render_providers_pane(frame: &mut Frame, area: Rect, state: &ModelDialogState, theme: &Theme) {
    let focused = state.pane == Pane::Providers;
    let w = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_line = 0;
    let mut keyless_labeled = false;

    for i in state.provider_order() {
        let keyless = is_keyless(state, i);
        if keyless && !keyless_labeled {
            keyless_labeled = true;
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(" no key", Style::default().fg(theme.dark_gray))));
        }
        if i == state.prov_idx {
            cursor_line = lines.len();
        }

        let row = Row::of(i == state.prov_idx, focused);
        let idle = if keyless { theme.dark_gray } else { theme.foreground };
        let mut spans = vec![rail(row, theme)];
        spans.extend(name_cell(
            short_name(state.providers[i].display_name()),
            w.saturating_sub(3 + 2),
            i == state.active_provider,
            row_text(row, idle, theme),
            theme,
        ));
        lines.push(highlight_cursor(row, Line::from(spans), w, theme));
    }

    let scroll = (cursor_line + 1).saturating_sub(area.height as usize) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
}

fn compact_provider_row(state: &ModelDialogState, w: usize, theme: &Theme) -> Line<'static> {
    let Some(provider) = state.selected_provider() else { return Line::default() };
    let row = Row::of(state.pane == Pane::Providers, true);
    let arrows = Style::default().fg(theme.dark_gray);
    let in_use = state.prov_idx == state.active_provider;
    let name_w = w.saturating_sub(3 + 2 + 2 + IN_USE.chars().count());
    let line = Line::from(vec![
        rail(row, theme),
        Span::styled("‹ ", arrows),
        Span::styled(fit_text(short_name(provider.display_name()), name_w), row_text(row, theme.foreground, theme)),
        Span::styled(if in_use { IN_USE } else { "" }, Style::default().fg(theme.success)),
        Span::styled(" ›", arrows),
    ]);
    highlight_cursor(row, line, w, theme)
}

fn detail_lines(state: &ModelDialogState, w: usize, theme: &Theme) -> Vec<Line<'static>> {
    let Some(provider) = state.selected_provider() else { return Vec::new() };
    let dim = Style::default().fg(theme.dark_gray);
    let viewing_active = state.prov_idx == state.active_provider;
    let models_focused = state.pane == Pane::Models;
    let name_w = MODEL_NAME_WIDTH.min(w.saturating_sub(3 + 2));
    let tags_w = w.saturating_sub(3 + name_w + 2 + 2 + 1);
    let mut lines = Vec::new();

    for (i, m) in provider.models.iter().enumerate() {
        let row = Row::of(i == state.model_idx, models_focused);
        let mut spans = vec![rail(row, theme)];
        spans.extend(name_cell(
            m.display_name(),
            name_w,
            viewing_active && i == state.active_model,
            row_text(row, theme.foreground, theme),
            theme,
        ));
        if tags_w >= 8 {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(fit_text(&m.tags.join(" · "), tags_w), dim));
        }
        lines.push(highlight_cursor(row, Line::from(spans), w, theme));
    }
    if provider.models.is_empty() {
        let note = "no models · add them under `providers` in settings.json";
        lines.push(Line::from(Span::styled(format!("   {}", fit_text(note, w.saturating_sub(4))), dim)));
    }
    lines.push(Line::default());

    let family = state.selected_model().map(|m| m.family).unwrap_or_default();
    let settings_focused = state.pane == Pane::Settings;
    for (i, setting) in state.settings_rows().iter().enumerate() {
        let row = Row::of(settings_focused && i == state.row_idx, true);
        let (label, value, note) = setting_display(*setting, family, state);
        let (left, right) = if row == Row::Cursor { ("‹ ", " ›") } else { ("  ", "  ") };
        let value_pad = SETTING_VALUE_WIDTH.saturating_sub(value.chars().count());
        let note_w = w.saturating_sub(3 + SETTING_LABEL_WIDTH + 2 + value.chars().count() + 2 + value_pad + 2 + 1);
        let line = Line::from(vec![
            rail(row, theme),
            Span::styled(pad_right(label, SETTING_LABEL_WIDTH), row_text(row, theme.gray, theme)),
            Span::styled(left, dim),
            Span::styled(value, row_text(row, theme.foreground, theme)),
            Span::styled(right, dim),
            Span::styled(format!("{}  {}", " ".repeat(value_pad), fit_text(note, note_w)), dim),
        ]);
        lines.push(highlight_cursor(row, line, w, theme));
    }
    lines
}

fn setting_display(row: SettingRow, family: ModelFamily, state: &ModelDialogState) -> (&'static str, String, &'static str) {
    match row {
        SettingRow::Temperature => {
            let t = if family == ModelFamily::Flash { state.temperature } else { state.generic_temperature };
            ("temperature", format!("{t:.1}"), "")
        }
        SettingRow::Reasoning => {
            let level = match family {
                ModelFamily::Flash => &state.flash_reasoning,
                ModelFamily::Pro => &state.pro_reasoning,
                ModelFamily::Generic => &state.generic_reasoning,
            };
            let note = if family == ModelFamily::Flash && level == "dynamic" {
                "low on tool-result turns, high otherwise"
            } else {
                ""
            };
            let shown = if level.is_empty() { "default".to_string() } else { level.to_lowercase() };
            ("reasoning", shown, note)
        }
        SettingRow::Persist if state.persist => ("save", "on".to_string(), "kept across sessions"),
        SettingRow::Persist => ("save", "off".to_string(), "this session only"),
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

    fn draw(state: &ModelDialogState, width: u16) -> (ratatui::buffer::Buffer, Vec<String>) {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let screen_height = 40;
        let height = state.height(width, screen_height);
        let mut terminal = Terminal::new(TestBackend::new(width, screen_height)).unwrap();
        terminal
            .draw(|f| render_model_dialog(f, Rect::new(0, screen_height - height, width, height), state, &Theme::default()))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let rows = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect();
        (buf, rows)
    }

    fn highlighted_rows(buf: &ratatui::buffer::Buffer) -> Vec<u16> {
        let bg = Theme::default().selected_bg;
        (0..buf.area.height).filter(|&y| (0..buf.area.width).any(|x| buf[(x, y)].bg == bg)).collect()
    }

    #[test]
    fn panel_is_frameless_and_highlights_only_the_cursor_row() {
        let s = open_default();
        let (buf, rows) = draw(&s, 140);
        let screen = rows.join("\n");

        assert!(!screen.contains(['┌', '┐', '└', '┘', '┬', '┴', '├', '┤', '│', '●', '○']), "{screen}");
        assert_eq!(rows.iter().filter(|r| r.contains("────")).count(), 2, "{screen}");
        assert_eq!(screen.matches('✓').count(), 2, "active provider and active model:\n{screen}");
        assert!(screen.contains("Enter use") && screen.contains("Esc close"), "{screen}");
        assert!(!screen.contains("Flash-1017"), "Flash is listed once");
        for line in screen.lines().filter(|l| l.contains("GitHub Models")) {
            assert!(!line.contains("(Free)"), "{line}");
        }

        let highlighted = highlighted_rows(&buf);
        assert_eq!(highlighted.len(), 1, "{screen}");
        assert!(rows[highlighted[0] as usize].contains("DeepSeek-V4.1-Flash"), "{screen}");

        let y = rows.iter().position(|r| r.contains(" Model")).unwrap() as u16;
        assert_eq!(buf[(1, y)].bg, ratatui::style::Color::Reset, "no fill");
    }

    #[test]
    fn the_highlight_follows_the_focused_pane() {
        let mut s = open_default();
        s.pane = Pane::Settings;
        let (buf, rows) = draw(&s, 140);
        let highlighted = highlighted_rows(&buf);
        assert_eq!(highlighted.len(), 1);
        let row = &rows[highlighted[0] as usize];
        assert!(row.contains("temperature") && row.contains('‹') && row.contains('›'), "{row}");
        assert_eq!(rows.join("\n").matches('‹').count(), 1, "only the cursor row offers ‹ ›");
    }

    #[test]
    fn credentials_sit_on_the_title_row_and_keyless_providers_are_grouped() {
        let mut s = open_default();
        s.ready = (0..s.providers.len()).map(|i| i == s.active_provider).collect();
        s.hints[s.prov_idx] = "credentials-hint".to_string();
        let (_, rows) = draw(&s, 140);

        let title = rows.iter().find(|r| r.contains(" Model")).expect("title");
        assert!(title.contains("credentials-hint"), "{}", rows.join("\n"));
        assert_eq!(rows.iter().filter(|r| r.contains("no key")).count(), 1, "{}", rows.join("\n"));
    }

    #[test]
    fn narrow_terminals_get_one_column() {
        let s = open_default();
        let (buf, rows) = draw(&s, 60);
        assert!(s.compact.get());
        assert!(rows.iter().any(|r| r.contains("‹ DeepSeek")), "{}", rows.join("\n"));
        assert_eq!(highlighted_rows(&buf).len(), 1);
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
    fn the_panel_keeps_its_height_while_the_selection_moves() {
        let mut s = open_default();
        s.pane = Pane::Models;
        let height = s.height(140, 40);
        s.handle_key(KeyCode::Down);
        assert_eq!(s.settings_rows().len(), 2, "the second model has fewer settings");
        assert_eq!(s.height(140, 40), height);
        s.pane = Pane::Providers;
        for _ in 0..s.providers.len() {
            s.handle_key(KeyCode::Down);
            assert_eq!(s.height(140, 40), height, "provider {}", s.prov_idx);
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
