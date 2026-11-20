use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use corex_core::config::{Config, WheelAcceleration};

use crate::overlay::{docked_height, docked_panel_open_bottom, highlight_cursor, key_hints, rail, row_text, Row};
use crate::text::{fit_text, pad_right};
use crate::theme::Theme;

const CONTEXT_LIMITS: [Option<usize>; 5] = [None, Some(128_000), Some(256_000), Some(512_000), Some(1_000_000)];
const RECENT_IMAGES: [usize; 5] = [1, 3, 5, 10, 0];
const LABEL_WIDTH: usize = 22;
const VALUE_WIDTH: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    ContextLimit,
    AutoCompact,
    RecentImages,
    WheelAcceleration,
    DeepseekWebTools,
    LocalPromptLite,
    Temperature,
}

pub const SETTINGS: [Setting; 7] = [
    Setting::ContextLimit,
    Setting::AutoCompact,
    Setting::Temperature,
    Setting::RecentImages,
    Setting::WheelAcceleration,
    Setting::DeepseekWebTools,
    Setting::LocalPromptLite,
];

#[derive(Debug, Default)]
pub struct SettingsDialogState {
    pub is_open: bool,
    pub selected: usize,
    pub notice: Option<String>,
    rows: Vec<Setting>,
}

pub enum SettingsAction {
    None,
    Close,
    Change(Setting, bool),
}

pub fn visible_settings(cfg: &Config) -> Vec<Setting> {
    let deepseek = cfg.provider_kind() == corex_core::providers::ProviderKind::DeepSeek;
    SETTINGS
        .into_iter()
        .filter(|setting| deepseek || *setting != Setting::DeepseekWebTools)
        .collect()
}

impl SettingsDialogState {
    pub fn open(&mut self, cfg: &Config) {
        *self = Self { is_open: true, rows: visible_settings(cfg), ..Self::default() };
    }

    pub fn dialog_height(&self, screen_height: u16) -> u16 {
        docked_height(self.rows.len(), screen_height).saturating_sub(2)
    }

    fn selected_setting(&self) -> Option<Setting> {
        self.rows.get(self.selected.min(self.rows.len().saturating_sub(1))).copied()
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }

    pub fn handle_key(&mut self, code: crossterm::event::KeyCode) -> SettingsAction {
        use crossterm::event::KeyCode;
        let Some(setting) = self.selected_setting() else {
            return if code == KeyCode::Esc { SettingsAction::Close } else { SettingsAction::None };
        };
        match code {
            KeyCode::Esc => SettingsAction::Close,
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                SettingsAction::None
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.rows.len() - 1);
                SettingsAction::None
            }
            KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => SettingsAction::Change(setting, true),
            KeyCode::Left => SettingsAction::Change(setting, false),
            _ => SettingsAction::None,
        }
    }
}

fn step<T: PartialEq + Copy>(options: &[T], current: T, forward: bool) -> T {
    let len = options.len();
    match options.iter().position(|o| *o == current) {
        Some(i) if forward => options[(i + 1) % len],
        Some(i) => options[(i + len - 1) % len],
        None => options[0],
    }
}

pub fn apply(cfg: &mut Config, setting: Setting, forward: bool) {
    match setting {
        Setting::ContextLimit => cfg.context_limit_tokens = step(&CONTEXT_LIMITS, cfg.context_limit_tokens, forward),
        Setting::AutoCompact => cfg.auto_compact = !cfg.auto_compact,
        Setting::Temperature => cfg.temperature_enabled = !cfg.temperature_enabled,
        Setting::RecentImages => cfg.recent_images = step(&RECENT_IMAGES, cfg.recent_images, forward),
        Setting::WheelAcceleration => cfg.wheel_acceleration = step(&WheelAcceleration::ALL, cfg.wheel_acceleration, forward),
        Setting::DeepseekWebTools => cfg.deepseek_web_tools = !cfg.deepseek_web_tools,
        Setting::LocalPromptLite => cfg.local_prompt_lite = !cfg.local_prompt_lite,
    }
}

fn tokens(n: usize) -> String {
    if n >= 1_000_000 && n % 1_000_000 == 0 {
        format!("{}M", n / 1_000_000)
    } else {
        format!("{}k", n / 1_000)
    }
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

fn toggle(cfg: &Config, setting: Setting) -> Option<bool> {
    match setting {
        Setting::AutoCompact => Some(cfg.auto_compact),
        Setting::Temperature => Some(cfg.temperature_enabled),
        Setting::DeepseekWebTools => Some(cfg.deepseek_web_tools),
        Setting::LocalPromptLite => Some(cfg.local_prompt_lite),
        _ => None,
    }
}

fn value_style(cfg: &Config, setting: Setting, theme: &Theme) -> Style {
    match toggle(cfg, setting) {
        Some(true) => Style::default().fg(theme.success),
        Some(false) => Style::default().fg(theme.dark_gray),
        None => Style::default().fg(theme.accent_cyan),
    }
}

pub fn label(setting: Setting) -> &'static str {
    match setting {
        Setting::ContextLimit => "Context limit",
        Setting::AutoCompact => "Auto-compact",
        Setting::Temperature => "Temperature",
        Setting::RecentImages => "Images per request",
        Setting::WheelAcceleration => "Scroll acceleration",
        Setting::DeepseekWebTools => "DeepSeek web tools",
        Setting::LocalPromptLite => "Lite prompt (local)",
    }
}

pub fn value(cfg: &Config, setting: Setting) -> String {
    match setting {
        Setting::ContextLimit => cfg.context_limit_tokens.map(tokens).unwrap_or_else(|| "auto".to_string()),
        Setting::AutoCompact => on_off(cfg.auto_compact),
        Setting::Temperature => on_off(cfg.temperature_enabled),
        Setting::RecentImages => match cfg.recent_images {
            0 => "all".to_string(),
            n => n.to_string(),
        },
        Setting::WheelAcceleration => cfg.wheel_acceleration.label().to_string(),
        Setting::DeepseekWebTools => on_off(cfg.deepseek_web_tools),
        Setting::LocalPromptLite => on_off(cfg.local_prompt_lite),
    }
}

fn hint(cfg: &Config, setting: Setting) -> String {
    match setting {
        Setting::ContextLimit => format!(
            "sends up to {}, compacts at {} (model: {})",
            tokens(cfg.effective_history_limit()),
            tokens(cfg.effective_compact_threshold()),
            tokens(cfg.model_context_window_with_source().0),
        ),
        Setting::AutoCompact => "summarize old turns when the context fills".to_string(),
        Setting::Temperature => "send and show it, only where the provider accepts one".to_string(),
        Setting::RecentImages => "newest images uploaded; older ones become a path".to_string(),
        Setting::WheelAcceleration => match cfg.wheel_acceleration.thresholds() {
            None => "always 1 line per wheel event".to_string(),
            Some((two, three)) => format!("fast wheel: 2 lines at {two}+ events/120ms, 3 at {three}+"),
        },
        Setting::DeepseekWebTools => "deepseek-flash + thinking search on DeepSeek's side".to_string(),
        Setting::LocalPromptLite => "shorter system prompt for small local models".to_string(),
    }
}

pub fn render_settings_dialog(frame: &mut Frame, area: Rect, state: &SettingsDialogState, cfg: &Config, theme: &Theme) {
    if !state.is_open {
        return;
    }
    let inner = docked_panel_open_bottom(frame, area, theme);
    let width = inner.width as usize;
    let compact = width < 80;
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(u16::from(compact)), Constraint::Length(1)]).split(inner);
    let dim = Style::default().fg(theme.dark_gray);
    let title = " Settings   ";

    let meta = state.notice.clone().unwrap_or_else(|| "changes are saved to settings.json".to_string());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(title, Style::default().fg(theme.text_strong).add_modifier(Modifier::BOLD)),
            Span::styled(fit_text(&meta, width.saturating_sub(title.len() + 1)), dim),
        ])),
        rows[0],
    );

    let selected = state.selected.min(state.rows.len().saturating_sub(1));
    let visible = (rows[1].height as usize).max(1);
    let offset = (selected + 1).saturating_sub(visible);
    let lines: Vec<Line<'static>> = state
        .rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, &setting)| {
            let kind = Row::of(i == selected, true);
            let mut spans = vec![
                rail(kind, theme),
                Span::styled(pad_right(label(setting), LABEL_WIDTH), row_text(kind, theme.foreground, theme)),
                Span::styled(pad_right(&format!("‹ {} ›", value(cfg, setting)), VALUE_WIDTH), value_style(cfg, setting, theme)),
            ];
            if !compact {
                let room = width.saturating_sub(3 + LABEL_WIDTH + VALUE_WIDTH + 1);
                spans.push(Span::styled(fit_text(&hint(cfg, setting), room), dim));
            }
            highlight_cursor(kind, Line::from(spans), width, theme)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows[1]);

    if let Some(setting) = state.selected_setting().filter(|_| compact) {
        let text = fit_text(&hint(cfg, setting), width.saturating_sub(4));
        frame.render_widget(Paragraph::new(Span::styled(format!("   {text}"), dim)), rows[2]);
    }

    let mut footer = vec![Span::raw(" ")];
    footer.extend(key_hints(&[("↑↓", "select"), ("←→", "change"), ("esc", "close")], theme));
    frame.render_widget(Paragraph::new(Line::from(footer)), rows[3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn context_limit_cycles_both_ways() {
        let mut cfg = Config::default();
        cfg.context_limit_tokens = None;
        apply(&mut cfg, Setting::ContextLimit, true);
        assert_eq!(cfg.context_limit_tokens, Some(128_000));
        apply(&mut cfg, Setting::ContextLimit, false);
        apply(&mut cfg, Setting::ContextLimit, false);
        assert_eq!(cfg.context_limit_tokens, Some(1_000_000));
        assert_eq!(value(&cfg, Setting::ContextLimit), "1M");
    }

    #[test]
    fn custom_value_jumps_to_first_option() {
        let mut cfg = Config::default();
        cfg.context_limit_tokens = Some(300_000);
        apply(&mut cfg, Setting::ContextLimit, true);
        assert_eq!(cfg.context_limit_tokens, None);
    }

    #[test]
    fn keys_move_and_change() {
        let mut state = SettingsDialogState::default();
        state.open(&Config::default());
        state.handle_key(KeyCode::Down);
        assert!(matches!(state.handle_key(KeyCode::Right), SettingsAction::Change(Setting::AutoCompact, true)));
        assert!(matches!(state.handle_key(KeyCode::Left), SettingsAction::Change(Setting::AutoCompact, false)));
        for _ in 0..10 {
            state.handle_key(KeyCode::Down);
        }
        assert_eq!(state.selected, state.rows.len() - 1);
        assert!(matches!(state.handle_key(KeyCode::Esc), SettingsAction::Close));
    }

    #[test]
    fn panel_lists_every_setting_of_the_active_provider() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let draw = |cfg: &Config| {
            let mut state = SettingsDialogState::default();
            state.open(cfg);
            let h = state.dialog_height(20);
            let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
            terminal.draw(|f| render_settings_dialog(f, Rect::new(0, 20 - h, 100, h), &state, cfg, &Theme::default())).unwrap();
            terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect::<String>()
        };
        let deepseek = Config { active_provider: Some("deepseek".into()), ..Config::default() };
        let screen = draw(&deepseek);
        for setting in visible_settings(&deepseek) {
            assert!(screen.contains(label(setting)), "{}", label(setting));
        }
        assert!(screen.contains(label(Setting::DeepseekWebTools)));

        let azure = Config { active_provider: Some("azure".into()), ..Config::default() };
        assert!(!draw(&azure).contains(label(Setting::DeepseekWebTools)));
    }

    #[test]
    fn short_screen_scrolls_to_selection() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let cfg = Config::default();
        let mut state = SettingsDialogState::default();
        state.open(&cfg);
        state.selected = state.rows.len() - 1;
        let h = 14;
        let height = state.dialog_height(h);
        let mut terminal = Terminal::new(TestBackend::new(100, h)).unwrap();
        terminal.draw(|f| render_settings_dialog(f, Rect::new(0, h - height, 100, height), &state, &cfg, &Theme::default())).unwrap();
        let screen: String = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(screen.contains(label(Setting::LocalPromptLite)));
    }
}
