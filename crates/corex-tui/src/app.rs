use std::collections::VecDeque;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, ListState, Paragraph};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use corex_core::client::{get_sudo_password, set_sudo_password, LlmClient, StreamEvent};
use corex_core::config::Config;
use corex_core::session::Session;
use corex_core::types::{Message, ToolCall};
use corex_prompt::PromptBuilder;
use corex_tools::registry::ToolRegistry;
use corex_tools::types::ToolContext;
use corex_tools::{command_requires_sudo, extract_first_sudo_command};

use crate::ascii::render_gradient_logo;
use crate::auth_dialog::{render_auth_dialog, AuthDialogState};
use crate::diff_view::{build_streaming_tool_preview_lines, build_tool_confirmation_lines};
use crate::markdown::render_markdown;
use crate::model_dialog::{render_model_dialog, ModelDialogState, ModelTab};
use crate::session_dialog::{render_session_dialog, SessionDialogState};
use crate::slash_commands::{render_command_popup, ALL_COMMANDS};
use crate::sudo_dialog::{render_sudo_dialog, SudoDialogState};
use crate::theme::Theme;
use crate::thinking_view::ThinkingState;
use crate::user_dialog::{parse_questions, render_user_dialog, UserDialogState};

#[derive(Debug, Clone)]
pub struct StatusTransition {
    pub current_text: String,
    pub target_text: String,
    pub start_time: Instant,
    pub duration: Duration,
}

impl StatusTransition {
    pub fn new() -> Self {
        Self {
            current_text: String::new(),
            target_text: String::new(),
            start_time: Instant::now(),
            duration: Duration::from_millis(220),
        }
    }

    pub fn set_target(&mut self, new_text: &str) {
        if self.target_text == new_text {
            return;
        }
        if self.target_text.is_empty() {
            self.current_text = new_text.to_string();
            self.target_text = new_text.to_string();
            self.start_time = Instant::now();
            return;
        }
        let snapshot = if self.start_time.elapsed() < self.duration {
            if self.start_time.elapsed().as_secs_f32() / self.duration.as_secs_f32() > 0.4 {
                self.target_text.clone()
            } else {
                self.current_text.clone()
            }
        } else {
            self.target_text.clone()
        };
        self.current_text = snapshot;
        self.target_text = new_text.to_string();
        self.start_time = Instant::now();
    }

    pub fn clear(&mut self) {
        self.current_text.clear();
        self.target_text.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.target_text.is_empty()
    }

    pub fn is_animating(&self) -> bool {
        self.start_time.elapsed() < self.duration && self.current_text != self.target_text
    }

    pub fn render_spans(&self, _theme: &Theme) -> Vec<Span<'static>> {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        let total = self.duration.as_secs_f32();
        let normal_color = Color::Rgb(150, 155, 170);

        if elapsed >= total || self.current_text == self.target_text || self.current_text.is_empty() {
            return vec![Span::styled(self.target_text.clone(), Style::default().fg(normal_color))];
        }

        let progress = (elapsed / total).clamp(0.0, 1.0);
        let target_chars: Vec<char> = self.target_text.chars().collect();
        let current_chars: Vec<char> = self.current_text.chars().collect();

        let max_len = target_chars.len().max(current_chars.len());
        if max_len == 0 {
            return Vec::new();
        }

        // Smooth wave front advancing left-to-right
        let wave_pos = progress * (max_len as f32 + 2.5);

        let mut spans = Vec::new();
        let mut text_buf = String::new();
        let mut last_color = None;

        for i in 0..max_len {
            let fi = i as f32;
            let dist = wave_pos - fi;

            let (ch, col) = if dist >= 1.5 {
                // Wave has fully passed this character: render target_text in settled color
                if i < target_chars.len() {
                    (target_chars[i], normal_color)
                } else {
                    continue; // Old character has dissolved behind the wave
                }
            } else if dist >= 0.0 {
                // Crest of the wave: gentle satin sheen as character emerges
                if i < target_chars.len() {
                    let intensity = (dist / 1.5).clamp(0.0, 1.0);
                    let r = (180.0 - (180.0 - 150.0) * intensity) as u8;
                    let g = (186.0 - (186.0 - 155.0) * intensity) as u8;
                    let b = (202.0 - (202.0 - 170.0) * intensity) as u8;
                    (target_chars[i], Color::Rgb(r, g, b))
                } else if i < current_chars.len() {
                    // Dissolving old tail character
                    (current_chars[i], Color::Rgb(118, 124, 138))
                } else {
                    continue;
                }
            } else {
                // Ahead of the wave: smoothly display previous character awaiting replacement
                if i < current_chars.len() {
                    (current_chars[i], Color::Rgb(128, 134, 148))
                } else {
                    continue;
                }
            };

            if Some(col) == last_color {
                text_buf.push(ch);
            } else {
                if let Some(c) = last_color {
                    if !text_buf.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut text_buf), Style::default().fg(c)));
                    }
                }
                text_buf.push(ch);
                last_color = Some(col);
            }
        }

        if let Some(c) = last_color {
            if !text_buf.is_empty() {
                spans.push(Span::styled(text_buf, Style::default().fg(c)));
            }
        }

        spans
    }
}

pub struct PendingToolBatch {
    pub calls: Vec<ToolCall>,
    pub diff_preview: Option<String>,
    pub selected_option: usize,
    pub diff_expanded: bool,
}

pub struct App {
    pub session: Session,
    pub llm_client: LlmClient,
    pub tool_registry: ToolRegistry,
    pub theme: Theme,
    pub workspace_dir: PathBuf,

    pub input_buffer: String,
    pub input_history: Vec<String>,
    pub history_idx: Option<usize>,
    pub saved_draft: String,
    pub slash_selected_idx: usize,
    pub pastes: std::collections::HashMap<usize, String>,
    pub next_paste_id: usize,
    pub cursor_idx: usize,

    pub is_streaming: bool,
    pub thinking_state: ThinkingState,
    pub streaming_text: String,
    pub streaming_tool_calls: Vec<ToolCall>,

    pub pending_confirmation: Option<PendingToolBatch>,
    pub user_dialog: UserDialogState,
    pub model_dialog: ModelDialogState,
    pub session_dialog: SessionDialogState,
    pub auth_dialog: AuthDialogState,
    pub sudo_dialog: SudoDialogState,
    pub cancel_token: Option<CancellationToken>,

    pub list_state: ListState,
    pub scroll_offset: u16,
    pub auto_scroll: bool,
    pub total_rendered_items: usize,
    pub plan_mode: bool,
    pub always_allow_tools: bool,
    pub logs_expanded: bool,
    pub git_branch: String,
    pub last_turn_start: Option<Instant>,
    pub last_esc_press: Option<Instant>,
    pub last_ctrl_c_press: Option<Instant>,
    pub slash_popup_height_current: f32,
    pub last_slash_filter: String,
    pub local_llm_online: Arc<AtomicBool>,
    pub last_local_check: Option<Instant>,
    pub cached_message_lines: Vec<Line<'static>>,
    pub cached_message_count: usize,
    pub cached_render_width: usize,
    pub cached_session_id: String,
    pub cached_logs_expanded: bool,
    pub active_background_pids: std::collections::HashSet<u32>,
    pub current_generation_id: u64,
    pub update_available: Arc<std::sync::Mutex<Option<String>>>,
    pub active_status: Option<String>,
    pub status_transition: StatusTransition,
    pub last_chat_rect: Option<Rect>,
    pub last_rendered_text_lines: Vec<String>,
    pub balance_tx: mpsc::Sender<Result<corex_core::types::BalanceResponse, String>>,
    pub balance_rx: mpsc::Receiver<Result<corex_core::types::BalanceResponse, String>>,
    pub is_checking_balance: bool,
    pub pending_balance_msg_index: Option<usize>,
    pub message_queue: VecDeque<String>,
}

impl App {
    pub fn new(llm_client: LlmClient, workspace_dir: PathBuf, yolo: bool) -> Self {
        let branch = Self::detect_git_branch(&workspace_dir);
        let cfg = llm_client.get_config();
        let persistent_history = corex_core::HistoryStore::load();

        let mut auth_dialog = AuthDialogState::new();
        if cfg.api_key.trim().is_empty() && !cfg.local_llm_enabled {
            auth_dialog.open();
        }

        let is_online = Arc::new(AtomicBool::new(false));
        let (balance_tx, balance_rx) = mpsc::channel(10);
        let app = Self {
            session: Session::new_with_params(&cfg.model, cfg.temperature, &cfg.reasoning_effort, Some(&workspace_dir)),
            llm_client: llm_client.clone(),
            tool_registry: ToolRegistry::new(),
            theme: Theme::default(),
            workspace_dir,

            input_buffer: String::new(),
            input_history: persistent_history,
            history_idx: None,
            saved_draft: String::new(),
            slash_selected_idx: 0,
            pastes: std::collections::HashMap::new(),
            next_paste_id: 1,
            cursor_idx: 0,

            is_streaming: false,
            thinking_state: ThinkingState::new(),
            streaming_text: String::new(),
            streaming_tool_calls: Vec::new(),

            pending_confirmation: None,
            user_dialog: UserDialogState::new(),
            model_dialog: ModelDialogState::new(),
            session_dialog: SessionDialogState::new(),
            auth_dialog,
            sudo_dialog: SudoDialogState::new(),
            cancel_token: None,

            list_state: ListState::default(),
            scroll_offset: 0,
            auto_scroll: true,
            total_rendered_items: 0,
            plan_mode: false,
            always_allow_tools: yolo,
            logs_expanded: false,
            git_branch: branch,
            last_turn_start: None,
            last_esc_press: None,
            last_ctrl_c_press: None,
            slash_popup_height_current: 0.0,
            last_slash_filter: String::new(),
            local_llm_online: is_online,
            last_local_check: None,
            cached_message_lines: Vec::new(),
            cached_message_count: 0,
            cached_render_width: 0,
            cached_session_id: String::new(),
            cached_logs_expanded: false,
            active_background_pids: std::collections::HashSet::new(),
            current_generation_id: 0,
            update_available: Arc::new(std::sync::Mutex::new(corex_core::update::check_cached_update(env!("CARGO_PKG_VERSION")))),
            active_status: None,
            status_transition: StatusTransition::new(),
            last_chat_rect: None,
            last_rendered_text_lines: Vec::new(),
            balance_tx,
            balance_rx,
            is_checking_balance: false,
            pending_balance_msg_index: None,
            message_queue: VecDeque::new(),
        };
        app.trigger_local_health_check();
        app.trigger_update_check();
        app
    }

    pub fn set_status(&mut self, text: &str) {
        self.status_transition.set_target(text);
        self.active_status = Some(text.to_string());
    }

    pub fn clear_status(&mut self) {
        self.status_transition.clear();
        self.active_status = None;
    }

    pub fn trigger_update_check(&self) {
        let update_arc = self.update_available.clone();
        tokio::spawn(async move {
            if let Some(newer) = corex_core::update::check_for_update_online(env!("CARGO_PKG_VERSION")).await {
                if let Ok(mut lock) = update_arc.lock() {
                    *lock = Some(newer);
                }
            }
        });
    }

    pub fn slash_popup_target_height(&self) -> f32 {
        let is_modal_open = self.sudo_dialog.is_open
            || self.model_dialog.is_open
            || self.auth_dialog.is_open
            || self.session_dialog.is_open
            || self.user_dialog.is_open;

        if !is_modal_open && self.input_buffer.starts_with('/') {
            let filter = self.input_buffer.to_lowercase();
            let count = ALL_COMMANDS
                .iter()
                .filter(|c| c.name.starts_with(&filter))
                .count();
            if count > 0 {
                (count as f32 + 2.0).min(10.0)
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    pub fn is_slash_animating(&self) -> bool {
        let target = self.slash_popup_target_height();
        (target - self.slash_popup_height_current).abs() > 0.05
    }

    pub fn invalidate_message_cache(&mut self) {
        self.cached_message_lines.clear();
        self.cached_message_count = 0;
        self.cached_render_width = 0;
        self.cached_session_id.clear();
        self.cached_logs_expanded = !self.logs_expanded;
    }

    pub fn trigger_local_health_check(&self) {
        let flag = self.local_llm_online.clone();
        let local_client = self.llm_client.local_client();
        tokio::spawn(async move {
            let online = local_client.health_check().await;
            flag.store(online, Ordering::Relaxed);
        });
    }

    pub async fn reload_mcp_servers(&mut self) -> Vec<corex_tools::McpServerStatus> {
        let cfg = self.llm_client.get_config();
        let (tools, statuses) = corex_tools::load_mcp_servers(&cfg.mcp_servers).await;
        for t in tools {
            self.tool_registry.register(t);
        }
        statuses
    }

    pub fn poll_local_health_check(&mut self) {
        let cfg = self.llm_client.get_config();
        if !cfg.local_llm_enabled && !cfg.model.starts_with("local") {
            return;
        }
        let should_check = match self.last_local_check {
            None => true,
            Some(t) => t.elapsed() >= Duration::from_secs(3),
        };
        if should_check {
            self.last_local_check = Some(Instant::now());
            self.trigger_local_health_check();
        }
    }

    fn detect_git_branch(dir: &Path) -> String {
        let head_file = dir.join(".git").join("HEAD");
        if head_file.exists() {
            if let Ok(content) = std::fs::read_to_string(head_file) {
                if let Some(branch) = content.trim().strip_prefix("ref: refs/heads/") {
                    return branch.to_string();
                }
            }
        }
        "main".to_string()
    }

    pub fn shorten_path(&self) -> String {
        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        if let Some(h) = home {
            if let Ok(rel) = self.workspace_dir.strip_prefix(&h) {
                return format!("~/{}", rel.display());
            }
        }
        self.workspace_dir.display().to_string()
    }

    pub fn clamp_cursor(&mut self) {
        if self.cursor_idx > self.input_buffer.len() {
            self.cursor_idx = self.input_buffer.len();
        }
        while !self.input_buffer.is_char_boundary(self.cursor_idx) {
            self.cursor_idx = self.cursor_idx.saturating_sub(1);
        }
    }

    pub fn handle_paste(&mut self, pasted: String) {
        if self.sudo_dialog.is_open {
            self.sudo_dialog.password_input.push_str(pasted.trim());
        } else if self.auth_dialog.is_open {
            self.auth_dialog.input_buffer.push_str(pasted.trim());
        } else if self.user_dialog.is_open {
            let cur = self.user_dialog.current;
            if cur < self.user_dialog.questions.len() && !self.user_dialog.questions[cur].has_options {
                self.user_dialog.text_input[cur].push_str(&pasted);
            }
        } else if self.pending_confirmation.is_none() {
            self.clamp_cursor();
            let line_count = pasted.lines().count();
            if line_count > 1 || pasted.len() > 120 {
                let id = self.next_paste_id;
                self.next_paste_id += 1;
                let extra_lines = line_count.saturating_sub(1);
                let tag = format!("[Pasted text #{} +{} lines]", id, extra_lines);
                self.pastes.insert(id, pasted);
                self.input_buffer.insert_str(self.cursor_idx, &tag);
                self.cursor_idx += tag.len();
            } else {
                self.input_buffer.insert_str(self.cursor_idx, &pasted);
                self.cursor_idx += pasted.len();
            }
            self.slash_selected_idx = 0;
            self.last_esc_press = None;
        }
    }

    pub fn start_stream_turn(&mut self, tx: mpsc::Sender<(u64, StreamEvent)>) {
        // Cancel any previous in-flight generation immediately
        if let Some(token) = self.cancel_token.take() {
            token.cancel();
        }

        self.current_generation_id = self.current_generation_id.wrapping_add(1);
        let turn_gen = self.current_generation_id;

        self.streaming_text.clear();
        self.streaming_tool_calls.clear();
        self.thinking_state.reset();
        self.thinking_state.is_streaming = true;
        self.is_streaming = true;
        self.active_status = Some("Generando...".to_string());
        self.auto_scroll = true;
        self.last_turn_start = Some(Instant::now());

        let prompt_builder = PromptBuilder::new(&self.workspace_dir)
            .with_sudo_password(get_sudo_password().is_some())
            .with_plan_mode(self.plan_mode);

        let llm_cfg = self.llm_client.get_config();
        let is_local = llm_cfg.local_llm_enabled || llm_cfg.model.starts_with("local");
        let system_prompt = if is_local && llm_cfg.local_prompt_lite {
            prompt_builder.build_lite()
        } else {
            prompt_builder.build()
        };

        let mut messages = vec![Message::system(system_prompt)];
        messages.extend(self.session.messages.clone());

        let tools = self.tool_registry.list_definitions();
        let cancel_token = CancellationToken::new();
        self.cancel_token = Some(cancel_token.clone());

        let client = self.llm_client.clone();
        tokio::spawn(async move {
            match client.stream_chat(messages, Some(tools), cancel_token.clone()).await {
                Ok(mut rx) => {
                    while let Some(evt) = rx.recv().await {
                        if cancel_token.is_cancelled() {
                            break;
                        }
                        let _ = tx.send((turn_gen, evt)).await;
                    }
                }
                Err(e) => {
                    if !cancel_token.is_cancelled() {
                        let _ = tx.send((turn_gen, StreamEvent::Error(e.to_string()))).await;
                    }
                }
            }
        });
    }

    pub async fn handle_slash_command(&mut self, cmd: &str) -> bool {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        let name = parts.first().copied().unwrap_or("");

        match name {
            "/clear" => {
                self.session.messages.clear();
                self.invalidate_message_cache();
                self.thinking_state.reset();
                self.streaming_text.clear();
                true
            }
            "/plan" => {
                self.plan_mode = !self.plan_mode;
                let status = if self.plan_mode { "ENABLED" } else { "DISABLED" };
                self.session.add_message(Message::system(format!("Architectural Plan Mode {}", status)));
                true
            }
            "/stats" => {
                let u = &self.session.total_usage;
                let ratio = u.cache_hit_percentage();
                let msg = format!(
                    "Session Usage Metrics:\n- Prompt Tokens: {}\n- Cached Prompt Tokens: {} ({:.1}% KV Cache Hit)\n- Completion Tokens: {}\n- Total Tokens: {}",
                    u.prompt_tokens, u.prompt_cache_hit_tokens, ratio, u.completion_tokens, u.total_tokens
                );
                self.session.add_message(Message::system(msg));
                true
            }
            "/local" => {
                if parts.len() > 1 && parts[1] == "status" {
                    let cfg = self.llm_client.get_config();
                    let local = self.llm_client.local_client();
                    let is_up = local.health_check().await;
                    let msg = format!(
                        "Local LLM Status:\n- Enabled: {}\n- Endpoint: {}\n- Model: {}\n- Server Reachable: {}\n- Hybrid Compression: {}",
                        cfg.local_llm_enabled,
                        cfg.local_llm_url,
                        cfg.local_llm_model,
                        if is_up { "YES (Active)" } else { "NO (Offline / Fallback to Cloud)" },
                        if cfg.hybrid_compression { "ON" } else { "OFF" }
                    );
                    self.session.add_message(Message::system(msg));
                    return true;
                }

                if parts.len() > 1 {
                    let prompt = parts[1..].join(" ");
                    let local = self.llm_client.local_client();
                    self.session.add_message(Message::user(format!("/local {}", prompt)));
                    match local.quick_chat(&prompt).await {
                        Ok(answer) => {
                            self.session.add_message(Message::assistant(
                                format!("{}\n\n*(Answered by Local LLM @ $0.00)*", answer.trim()),
                                None,
                            ));
                        }
                        Err(e) => {
                            self.session.add_message(Message::system(format!(
                                "Local LLM Error (is llama-server running on {}?): {}",
                                self.llm_client.get_config().local_llm_url,
                                e
                            )));
                        }
                    }
                } else {
                    self.session.add_message(Message::system("Usage: /local <prompt> (ask local LLM directly) or /local status"));
                }
                true
            }
            "/hybrid" => {
                let mut cfg = self.llm_client.get_config();
                if parts.len() > 1 {
                    match parts[1].to_lowercase().as_str() {
                        "mode" => {
                            if parts.len() > 2 {
                                if let Some(m) = corex_core::config::HybridMode::from_str_loose(parts[2]) {
                                    cfg.hybrid_settings.mode = m;
                                    cfg.local_llm_enabled = true;
                                    self.llm_client.update_config(cfg.clone());
                                    let _ = Config::save_hybrid_settings_with_workspace(&cfg.hybrid_settings, Some(&self.workspace_dir));
                                    let _ = cfg.save_with_workspace(Some(&self.workspace_dir));
                                    self.session.add_message(Message::system(format!(
                                        "Hybrid Strategy set to: {}\n{}",
                                        m.display_name(), m.description()
                                    )));
                                } else {
                                    self.session.add_message(Message::system(
                                        "Available hybrid modes: triage | scout | review | compress"
                                    ));
                                }
                            } else {
                                self.session.add_message(Message::system(format!(
                                    "Current Hybrid Strategy: {}\nUsage: /hybrid mode <triage|scout|review|compress>",
                                    cfg.hybrid_settings.mode.display_name()
                                )) );
                            }
                        }
                        "on" | "true" | "1" => {
                            cfg.hybrid_compression = true;
                            cfg.local_llm_enabled = true;
                            self.llm_client.update_config(cfg.clone());
                            let _ = Config::save_hybrid_settings_with_workspace(&cfg.hybrid_settings, Some(&self.workspace_dir));
                            let _ = cfg.save_with_workspace(Some(&self.workspace_dir));
                            self.session.add_message(Message::system(format!(
                                "Hybrid Mode ENABLED!\n- Strategy: {}\n- Primary Model: {}\n- Local Server: {}",
                                cfg.hybrid_settings.mode.display_name(),
                                cfg.model,
                                cfg.local_llm_url
                            )));
                        }
                        "off" | "false" | "0" => {
                            cfg.hybrid_compression = false;
                            cfg.local_llm_enabled = false;
                            self.llm_client.update_config(cfg.clone());
                            let _ = Config::save_hybrid_settings_with_workspace(&cfg.hybrid_settings, Some(&self.workspace_dir));
                            let _ = cfg.save_with_workspace(Some(&self.workspace_dir));
                            self.session.add_message(Message::system("Hybrid Mode DISABLED (Pure Cloud active)."));
                        }
                        "status" => {
                            let is_up = self.llm_client.local_client().health_check().await;
                            self.session.add_message(Message::system(format!(
                                "Hybrid Configuration Status:\n- Enabled: {}\n- Strategy: {}\n- Local Server: {} ({})\n- Auto Output Compression: {}",
                                cfg.local_llm_enabled,
                                cfg.hybrid_settings.mode.display_name(),
                                cfg.local_llm_url,
                                if is_up { "ONLINE" } else { "OFFLINE" },
                                if cfg.hybrid_settings.auto_compression { "ON" } else { "OFF" }
                            )));
                        }
                        _ => {
                            self.session.add_message(Message::system("Usage: /hybrid on | off | mode <triage|scout|review|compress> | status"));
                        }
                    }
                } else {
                    cfg.local_llm_enabled = !cfg.local_llm_enabled;
                    let state = if cfg.local_llm_enabled { "ENABLED" } else { "DISABLED" };
                    self.llm_client.update_config(cfg.clone());
                    let _ = Config::save_hybrid_settings_with_workspace(&cfg.hybrid_settings, Some(&self.workspace_dir));
                    let _ = cfg.save_with_workspace(Some(&self.workspace_dir));
                    self.session.add_message(Message::system(format!("Hybrid Mode {}", state)));
                }
                true
            }
            "/balance" | "/wallet" => {
                if self.is_checking_balance {
                    self.set_status("Checking account balance...");
                    return true;
                }
                let msg_idx = self.session.messages.len();
                self.session.add_message(Message::system("Checking account balance..."));
                self.pending_balance_msg_index = Some(msg_idx);
                self.invalidate_message_cache();
                self.set_status("Checking account balance...");
                self.is_checking_balance = true;

                let client = self.llm_client.clone();
                let tx = self.balance_tx.clone();
                tokio::spawn(async move {
                    let res = client.check_balance().await.map_err(|e| e.to_string());
                    let _ = tx.send(res).await;
                });
                true
            }
            "/chat" | "/sessions" => {
                let subcmd = parts.get(1).copied().unwrap_or("list");
                match subcmd {
                    "list" => {
                        self.session_dialog.open(&self.workspace_dir.display().to_string());
                    }
                    "save" => {
                        if parts.len() > 2 {
                            let tag = parts[2..].join(" ");
                            if let Err(e) = self.session.save_checkpoint(&tag) {
                                self.session.add_message(Message::system(format!("Failed to save checkpoint: {}", e)));
                            } else {
                                self.session.add_message(Message::system(format!("Conversation checkpoint saved with tag: {}.", tag)));
                            }
                        } else {
                            self.session.add_message(Message::system("Missing tag. Usage: /chat save <tag> or /save <tag>"));
                        }
                    }
                    "resume" | "load" => {
                        if parts.len() > 2 {
                            let target = parts[2..].join(" ");
                            match Session::load_by_id_or_tag(&target) {
                                Ok(loaded) => {
                                    let mut cfg = self.llm_client.get_config();
                                    cfg.model = loaded.model.clone();
                                    if let Some(t) = loaded.temperature {
                                        cfg.temperature = t;
                                    }
                                    if let Some(ref r) = loaded.reasoning_effort {
                                        cfg.reasoning_effort = r.clone();
                                    }
                                    self.llm_client.update_config(cfg);
                                    let title = loaded.title.clone();
                                    let model = loaded.model.clone();
                                    self.session = loaded;
                                    self.session.add_message(Message::system(format!("Resumed session '{}' (Model: {}).", title, model)));
                                }
                                Err(e) => {
                                    self.session.add_message(Message::system(format!("Error: {}", e)));
                                }
                            }
                        } else {
                            self.session.add_message(Message::system("Missing session tag or number. Usage: /chat resume <tag/id>"));
                        }
                    }
                    "delete" | "rm" => {
                        if parts.len() > 2 {
                            let target = parts[2..].join(" ");
                            match Session::delete_by_id_or_tag(&target) {
                                Ok(id) => {
                                    self.session.add_message(Message::system(format!("Deleted session {}.", id)));
                                }
                                Err(e) => {
                                    self.session.add_message(Message::system(format!("Error deleting session: {}", e)));
                                }
                            }
                        } else {
                            self.session.add_message(Message::system("Missing session tag or number. Usage: /chat delete <tag/id>"));
                        }
                    }
                    "new" => {
                        let _ = self.session.save();
                        let cfg = self.llm_client.get_config();
                        self.session = Session::new_with_params(&cfg.model, cfg.temperature, &cfg.reasoning_effort, Some(&self.workspace_dir));
                        self.streaming_text.clear();
                        self.thinking_state.reset();
                        self.session.add_message(Message::system(format!("Started new chat session (Model: {}).", cfg.model)));
                    }
                    _ => {
                        self.session.add_message(Message::system("Usage: /chat list | /chat save <tag> | /chat resume <tag/id> | /chat delete <tag/id> | /chat new"));
                    }
                }
                true
            }
            "/resume" => {
                if parts.len() > 1 {
                    let target = parts[1..].join(" ");
                    match Session::load_by_id_or_tag(&target) {
                        Ok(loaded) => {
                            let mut cfg = self.llm_client.get_config();
                            cfg.model = loaded.model.clone();
                            if let Some(t) = loaded.temperature {
                                cfg.temperature = t;
                            }
                            if let Some(ref r) = loaded.reasoning_effort {
                                cfg.reasoning_effort = r.clone();
                            }
                            self.llm_client.update_config(cfg);
                            let title = loaded.title.clone();
                            let model = loaded.model.clone();
                            self.session = loaded;
                            self.session.add_message(Message::system(format!("Resumed session '{}' (Model: {}).", title, model)));
                        }
                        Err(e) => {
                            self.session.add_message(Message::system(format!("Error: {}", e)));
                        }
                    }
                } else {
                    self.slash_popup_height_current = 0.0;
                    self.session_dialog.open(&self.workspace_dir.display().to_string());
                }
                true
            }
            "/save" => {
                if parts.len() > 1 {
                    let tag = parts[1..].join(" ");
                    if let Err(e) = self.session.save_checkpoint(&tag) {
                        self.session.add_message(Message::system(format!("Failed to save checkpoint: {}", e)));
                    } else {
                        self.session.add_message(Message::system(format!("Conversation checkpoint saved with tag: {}.", tag)));
                    }
                } else {
                    self.session.add_message(Message::system("Missing tag. Usage: /save <tag>"));
                }
                true
            }
            "/new" => {
                let _ = self.session.save();
                let cfg = self.llm_client.get_config();
                self.session = Session::new_with_params(&cfg.model, cfg.temperature, &cfg.reasoning_effort, Some(&self.workspace_dir));
                self.streaming_text.clear();
                self.thinking_state.reset();
                self.session.add_message(Message::system(format!("Started new chat session (Model: {}).", cfg.model)));
                true
            }
            "/model" => {
                if parts.len() > 1 {
                    let new_model = parts[1];
                    let mut cfg = self.llm_client.get_config();
                    cfg.model = new_model.to_string();
                    if !new_model.contains("local") {
                        cfg.local_llm_enabled = false;
                        cfg.hybrid_compression = false;
                    }
                    let _ = cfg.save_with_workspace(Some(&self.workspace_dir));
                    self.llm_client.update_config(cfg.clone());
                    self.session.model = new_model.to_string();
                    self.session.temperature = Some(cfg.temperature);
                    self.session.reasoning_effort = Some(cfg.reasoning_effort);
                    let _ = self.session.save();
                    self.session.add_message(Message::system(format!("Switched and saved active model to '{}'.", new_model)));
                } else {
                    let cfg = self.llm_client.get_config();
                    self.slash_popup_height_current = 0.0;
                    self.model_dialog.open(
                        &cfg.model,
                        &cfg.flash_settings,
                        &cfg.pro_settings,
                        &cfg.hybrid_settings,
                        cfg.local_llm_enabled && cfg.hybrid_compression,
                        cfg.local_prompt_lite,
                    );
                }
                true
            }
            "/sudo" => {
                if parts.len() > 1 {
                    let pwd = parts[1..].join(" ");
                    set_sudo_password(Some(pwd));
                    self.session.add_message(Message::system("Sudo password stored in session RAM (silent AskPass enabled)."));
                } else {
                    set_sudo_password(None);
                    self.session.add_message(Message::system("Sudo password cleared from session RAM."));
                }
                true
            }
            "/key" | "/auth" => {
                if parts.len() > 1 {
                    let key_str = parts[1..].join(" ").trim().to_string();
                    let mut cfg = self.llm_client.get_config();
                    cfg.api_key = key_str.clone();
                    let _ = cfg.save_with_workspace(Some(&self.workspace_dir));
                    self.llm_client.update_config(cfg);
                    self.session.add_message(Message::system("DeepSeek API key updated and saved to ~/.corex/settings.json."));
                } else {
                    self.auth_dialog.open();
                }
                true
            }
            "/rewind" => {
                if self.session.messages.is_empty() {
                    self.session.add_message(Message::system("Conversation history is already empty."));
                } else {
                    let mut found_user = false;
                    while let Some(msg) = self.session.messages.pop() {
                        if msg.role == "user" {
                            found_user = true;
                            break;
                        }
                    }
                    self.invalidate_message_cache();
                    let _ = self.session.save();
                    if found_user {
                        self.session.add_message(Message::system("Rewound last conversation turn."));
                    } else {
                        self.session.add_message(Message::system("Cleared remaining messages."));
                    }
                }
                true
            }
            "/compact" | "/compress" => {
                match self.llm_client.compact_messages(&mut self.session.messages, true).await {
                    Ok(Some(notice)) => {
                        self.invalidate_message_cache();
                        let _ = self.session.save();
                        self.session.add_message(Message::system(notice));
                    }
                    Ok(None) => {
                        self.session.add_message(Message::system(
                            "Conversation history is too short to require compaction."
                        ));
                    }
                    Err(e) => {
                        self.session.add_message(Message::system(format!(
                            "Failed to compact conversation: {}", e
                        )));
                    }
                }
                true
            }
            "/info" | "/author" | "/credits" | "/about" => {
                let cfg = self.llm_client.get_config();
                let info = format!(
                    "COREX_INFO_CARD|{version}|{model}|{base_url}|{local_engine}|{local_enabled}|{session_id}|{workspace}|{branch}",
                    version = env!("CARGO_PKG_VERSION"),
                    model = cfg.model,
                    base_url = cfg.base_url,
                    local_engine = cfg.local_llm_url,
                    local_enabled = cfg.local_llm_enabled,
                    session_id = self.session.id,
                    workspace = self.workspace_dir.display(),
                    branch = self.git_branch
                );
                self.session.add_message(Message::system(info));
                true
            }
            "/update" => {
                let current = env!("CARGO_PKG_VERSION");
                let maybe_newer = self.update_available.lock().ok().and_then(|l| l.clone());
                let msg = if let Some(newer) = maybe_newer {
                    format!(
                        "⚡ A new version of Corex is available: v{} → v{}\n\n\
                        To update your installation, run in your terminal:\n\
                        • Via npm:       npm install -g corex-cli\n\
                        • From source:   cargo install --git https://github.com/sluisr/corex.git --force\n\
                        • Or download precompiled binaries from:\n\
                          https://github.com/sluisr/corex/releases/latest",
                        current, newer
                    )
                } else {
                    format!(
                        "✓ Corex is up to date (v{}).\n\n\
                        If you wish to reinstall or update manually:\n\
                        • npm install -g corex-cli\n\
                        • cargo install --git https://github.com/sluisr/corex.git --force\n\
                        • https://github.com/sluisr/corex/releases",
                        current
                    )
                };
                self.session.add_message(Message::system(msg));
                true
            }
            "/prefix" => {
                if parts.len() > 1 {
                    let prefix_text = parts[1..].join(" ");
                    self.session.add_message(Message::system(format!(
                        "Response formatting prefix configured: '{}'. DeepSeek will direct its next output with this prefix.",
                        prefix_text
                    )));
                } else {
                    self.session.add_message(Message::system("Usage: /prefix <text> (e.g. /prefix Output: or /prefix ```json)"));
                }
                true
            }
            "/fim" => {
                if parts.len() > 1 {
                    let file_arg = parts[1];
                    let file_path = if file_arg.starts_with('/') {
                        PathBuf::from(file_arg)
                    } else {
                        self.workspace_dir.join(file_arg)
                    };

                    if !file_path.exists() {
                        self.session.add_message(Message::system(format!("File '{}' not found.", file_arg)));
                    } else {
                        match std::fs::read_to_string(&file_path) {
                            Ok(content) => {
                                if content.contains("<FIM_HOLE>") {
                                    let prompt = format!(
                                        "Please perform Fill-in-the-Middle (FIM) code completion for `{}`. Complete the exact code that belongs strictly inside `<FIM_HOLE>`:\n\n```\n{}\n```",
                                        file_arg,
                                        content
                                    );
                                    self.session.add_message(Message::user(prompt));
                                    self.session.add_message(Message::system("FIM prompt loaded. Ready to stream completion."));
                                } else {
                                    self.session.add_message(Message::system(format!(
                                        "FIM for '{}': Insert `<FIM_HOLE>` at the exact position in `{}` where code should be filled, then run `/fim {}` again.",
                                        file_arg, file_arg, file_arg
                                    )));
                                }
                            }
                            Err(e) => {
                                self.session.add_message(Message::system(format!("Error reading {}: {}", file_arg, e)));
                            }
                        }
                    }
                } else {
                    self.session.add_message(Message::system("Usage: /fim <path/to/file> (Fill-in-the-Middle code completion using <FIM_HOLE>)"));
                }
                true
            }
            "/mcp" => {
                let sub = parts.get(1).copied().unwrap_or("status");
                match sub {
                    "reload" => {
                        let statuses = self.reload_mcp_servers().await;
                        let mut msg = format!("Reloaded MCP Servers ({} configured):\n", statuses.len());
                        if statuses.is_empty() {
                            msg.push_str("  No servers in config. Add them to ~/.corex/settings.json under 'mcp_servers'.\n");
                        }
                        for s in &statuses {
                            let icon = if s.is_connected { "[OK]" } else { "[ERR]" };
                            msg.push_str(&format!("{} {} ({}): {} tools discovered\n", icon, s.name, s.command, s.tools_count));
                            if let Some(ref e) = s.error {
                                msg.push_str(&format!("   Error: {}\n", e));
                            }
                        }
                        self.session.add_message(Message::system(msg));
                    }
                    _ => {
                        let cfg = self.llm_client.get_config();
                        if cfg.mcp_servers.is_empty() {
                            let help = "No MCP (Model Context Protocol) servers configured.\n\
                                To add MCP servers, configure ~/.corex/settings.json or .corex/settings.json:\n\
                                {\n\
                                  \"mcp_servers\": {\n\
                                    \"github\": {\n\
                                      \"command\": \"npx\",\n\
                                      \"args\": [\"-y\", \"@modelcontextprotocol/server-github\"],\n\
                                      \"env\": { \"GITHUB_PERSONAL_ACCESS_TOKEN\": \"ghp_...\" }\n\
                                    }\n\
                                  }\n\
                                }\n\
                                Then run '/mcp reload' to connect.";
                            self.session.add_message(Message::system(help));
                        } else {
                            let mut msg = format!("Configured MCP Servers ({}):\n", cfg.mcp_servers.len());
                            for (name, scfg) in &cfg.mcp_servers {
                                msg.push_str(&format!("- {}: {} {}\n", name, scfg.command, scfg.args.join(" ")));
                            }
                            msg.push_str("\nRun '/mcp reload' to spawn and register tools dynamically.");
                            self.session.add_message(Message::system(msg));
                        }
                    }
                }
                true
            }
            "/tasks" | "/background" => {
                let mgr = corex_tools::background::get_task_manager();
                if parts.len() == 1 {
                    let guard = mgr.lock().unwrap();
                    let list = guard.list();
                    self.session.add_message(Message::system(format!("Background Tasks:\n\n{}", list)));
                } else {
                    let sub = parts[1].to_lowercase();
                    match sub.as_str() {
                        "list" => {
                            let guard = mgr.lock().unwrap();
                            let list = guard.list();
                            self.session.add_message(Message::system(format!("Background Tasks:\n\n{}", list)));
                        }
                        "status" | "log" | "output" => {
                            if parts.len() > 2 {
                                if let Ok(pid) = parts[2].parse::<u32>() {
                                    let guard = mgr.lock().unwrap();
                                    match guard.get_status(pid) {
                                        Some(st) => self.session.add_message(Message::system(st)),
                                        None => self.session.add_message(Message::system(format!("Task {} not found.", pid))),
                                    }
                                } else {
                                    self.session.add_message(Message::system("Invalid Task ID / PID. Usage: /tasks status <pid>"));
                                }
                            } else {
                                self.session.add_message(Message::system("Usage: /tasks status <pid>"));
                            }
                        }
                        "kill" | "stop" => {
                            if parts.len() > 2 {
                                if let Ok(pid) = parts[2].parse::<u32>() {
                                    let mut guard = mgr.lock().unwrap();
                                    if guard.kill(pid) {
                                        self.session.add_message(Message::system(format!("Terminated background task {}.", pid)));
                                    } else {
                                        self.session.add_message(Message::system(format!("Task {} not found.", pid)));
                                    }
                                } else {
                                    self.session.add_message(Message::system("Invalid Task ID / PID. Usage: /tasks kill <pid>"));
                                }
                            } else {
                                self.session.add_message(Message::system("Usage: /tasks kill <pid>"));
                            }
                        }
                        "send" | "input" => {
                            if parts.len() > 3 {
                                if let Ok(pid) = parts[2].parse::<u32>() {
                                    let input = parts[3..].join(" ");
                                    let guard = mgr.lock().unwrap();
                                    match guard.send_input(pid, input) {
                                        Ok(true) => self.session.add_message(Message::system(format!("Sent input to task {}.", pid))),
                                        _ => self.session.add_message(Message::system(format!("Failed to send input to task {}. (Task might be inactive)", pid))),
                                    }
                                } else {
                                    self.session.add_message(Message::system("Invalid Task ID / PID. Usage: /tasks send <pid> <input>"));
                                }
                            } else {
                                self.session.add_message(Message::system("Usage: /tasks send <pid> <input text>"));
                            }
                        }
                        _ => {
                            if let Ok(pid) = parts[1].parse::<u32>() {
                                let guard = mgr.lock().unwrap();
                                match guard.get_status(pid) {
                                    Some(st) => self.session.add_message(Message::system(st)),
                                    None => self.session.add_message(Message::system(format!("Task {} not found.", pid))),
                                }
                            } else {
                                self.session.add_message(Message::system("Usage: /tasks [list] | /tasks status <pid> | /tasks kill <pid> | /tasks send <pid> <input>"));
                            }
                        }
                    }
                }
                true
            }
            "/yolo" => {
                if parts.len() > 1 {
                    let sub = parts[1].to_lowercase();
                    match sub.as_str() {
                        "on" | "enable" | "true" => {
                            self.always_allow_tools = true;
                            self.session.add_message(Message::system("YOLO mode enabled: all tool executions will be auto-approved without confirmation prompts."));
                        }
                        "off" | "disable" | "false" => {
                            self.always_allow_tools = false;
                            self.session.add_message(Message::system("YOLO mode disabled: confirmation prompts will appear for mutating or dangerous tools."));
                        }
                        _ => {
                            self.session.add_message(Message::system("Usage: /yolo on | /yolo off"));
                        }
                    }
                } else {
                    self.always_allow_tools = !self.always_allow_tools;
                    if self.always_allow_tools {
                        self.session.add_message(Message::system("YOLO mode enabled: all tool executions will be auto-approved without confirmation prompts."));
                    } else {
                        self.session.add_message(Message::system("YOLO mode disabled: confirmation prompts will appear for mutating or dangerous tools."));
                    }
                }
                true
            }
            "/help" => {
                let mut help = "Available Commands:\n".to_string();
                for c in ALL_COMMANDS {
                    help.push_str(&format!("  {:<12} {}\n", c.name, c.description));
                }
                help.push_str("  /key <sk..>  Set or update your DeepSeek API key\n");
                help.push_str("  /sudo <pwd>  Store sudo password in RAM for silent privilege escalation\n");
                help.push_str("\nKeyboard Shortcuts:\n  Enter: Submit | Ctrl+T: Toggle Thought Box | Ctrl+C: Cancel/Exit | Up/Down: History / Command Nav");
                self.session.add_message(Message::system(help));
                true
            }
            _ => false,
        }
    }
}

/// Restores the terminal to its standard state by disabling raw mode,
/// leaving alternate screen, disabling mouse capture/bracketed paste, and showing the cursor.
pub fn restore_terminal() {
    let _ = disable_raw_mode();
    let mut stdout = stdout();
    let _ = execute!(
        stdout,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        crossterm::cursor::Show
    );
}

/// RAII guard ensuring the terminal is always cleanly restored on drop (including panics and early returns).
pub struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

pub async fn run_tui(mut app: App) -> Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        restore_terminal();
        default_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let _guard = TerminalGuard;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (event_tx, mut event_rx) = mpsc::channel::<(u64, StreamEvent)>(100);

    let mut last_tick = Instant::now();
    let mut needs_redraw = true;

    loop {
        app.poll_local_health_check();

        // Check for finished background tasks and notify the user/session
        let newly_finished: Vec<(u32, Option<i32>, String, String)> = {
            if let Ok(mgr) = corex_tools::background::get_task_manager().lock() {
                let mut finished = Vec::new();
                let mut still_active = std::collections::HashSet::new();

                for &pid in &app.active_background_pids {
                    if let Some(proc) = mgr.get_process(pid) {
                        if proc.is_active() && proc.is_backgrounded() {
                            still_active.insert(pid);
                        } else if proc.is_backgrounded() {
                            let dur = proc.duration_str();
                            let output = proc.output_buffer.lock().map(|b| b.clone()).unwrap_or_default();
                            finished.push((pid, proc.get_exit_code(), dur, output));
                        }
                    } else {
                        finished.push((pid, None, String::new(), String::new()));
                    }
                }

                for (pid, proc) in mgr.all_processes() {
                    if proc.is_active() && proc.is_backgrounded() {
                        still_active.insert(*pid);
                    }
                }

                app.active_background_pids = still_active;
                finished
            } else {
                Vec::new()
            }
        };

        for (pid, exit_code, dur, output) in newly_finished {
            let code_str = match exit_code {
                Some(c) => c.to_string(),
                None => "stopped".to_string(),
            };
            let mut msg_text = format!("TASK_DONE|{}|{}|{}\n", pid, code_str, dur);
            msg_text.push_str(output.trim_end());
            app.session.add_message(Message::system(msg_text));
            app.invalidate_message_cache();
            needs_redraw = true;
        }

        while let Ok(res) = app.balance_rx.try_recv() {
            app.clear_status();
            app.is_checking_balance = false;

            let msg = match res {
                Ok(bal) => {
                    let mut msg = format!("DeepSeek Account Balance (Available: {}):\n", bal.is_available);
                    for info in bal.balance_infos {
                        msg.push_str(&format!(
                            "- Total: {} {}\n  (Topped up: {} {}, Granted: {} {})\n",
                            info.total_balance, info.currency,
                            info.topped_up_balance, info.currency,
                            info.granted_balance, info.currency
                        ));
                    }
                    msg
                }
                Err(e) => {
                    format!("Failed to retrieve balance: {}", e)
                }
            };

            if let Some(idx) = app.pending_balance_msg_index.take() {
                if idx < app.session.messages.len() {
                    app.session.messages[idx] = Message::system(msg);
                } else {
                    app.session.add_message(Message::system(msg));
                }
            } else {
                app.session.add_message(Message::system(msg));
            }
            app.invalidate_message_cache();
            needs_redraw = true;
        }

        if !app.is_streaming
            && !app.message_queue.is_empty()
            && app.pending_confirmation.is_none()
            && !app.user_dialog.is_open
            && !app.sudo_dialog.is_open
            && !app.is_checking_balance
        {
            if let Some(next_prompt) = app.message_queue.pop_front() {
                app.session.add_message(Message::user(next_prompt));
                app.start_stream_turn(event_tx.clone());
                needs_redraw = true;
            }
        }

        let has_active_tasks = !app.active_background_pids.is_empty();
        let is_animating = app.is_slash_animating() || app.status_transition.is_animating();

        if needs_redraw || app.is_streaming || app.is_checking_balance || has_active_tasks || is_animating {
            terminal.draw(|f| {
                render_ui(f, &mut app);
            })?;
            needs_redraw = false;
        }

        while let Ok((event_gen, event)) = event_rx.try_recv() {
            if event_gen != app.current_generation_id {
                // Drop stale/cancelled turn event to prevent concurrent interleaving
                continue;
            }
            needs_redraw = true;
            match event {
                StreamEvent::ReasoningDelta(delta) => {
                    app.thinking_state.content.push_str(&delta);
                    app.set_status("Thinking...");
                }
                StreamEvent::ContentDelta(delta) => {
                    app.streaming_text.push_str(&delta);
                    app.set_status("Generating response...");
                }
                StreamEvent::ToolCallDelta { index, id, name, arguments } => {
                    while app.streaming_tool_calls.len() <= index {
                        app.streaming_tool_calls.push(ToolCall {
                            id: String::new(),
                            call_type: "function".to_string(),
                            function: corex_core::types::FunctionCall {
                                name: String::new(),
                                arguments: String::new(),
                            },
                        });
                    }
                    if let Some(i) = id {
                        app.streaming_tool_calls[index].id.push_str(&i);
                    }
                    if let Some(n) = name {
                        app.streaming_tool_calls[index].function.name.push_str(&n);
                    }
                    if let Some(a) = arguments {
                        app.streaming_tool_calls[index].function.arguments.push_str(&a);
                    }
                }
                StreamEvent::UsageUpdate(usage) => {
                    app.session.update_usage(&usage);
                }
                StreamEvent::ToolExecutionStarting { summary, .. } => {
                    app.set_status(&summary);
                }
                StreamEvent::ToolExecutionDone { call_id, output } => {
                    if output.contains("incorrect password attempt")
                        || output.contains("sudo: a password is required")
                        || output.contains("sudo: PAM authentication")
                    {
                        set_sudo_password(None);
                    }
                    app.session.add_message(Message::tool_response(call_id, output));
                }
                StreamEvent::AllToolsDone => {
                    // ⚡ TRIGGER NEXT RECURSIVE TURN OF AGENT LOOP!
                    app.start_stream_turn(event_tx.clone());
                }
                StreamEvent::Completed { .. } => {
                    let assistant_text = if app.streaming_text.is_empty() {
                        None
                    } else {
                        Some(app.streaming_text.clone())
                    };

                    let reasoning = if app.thinking_state.content.is_empty() {
                        None
                    } else {
                        Some(app.thinking_state.content.clone())
                    };

                    if let (Some(ref txt), Some(ref cot)) = (&assistant_text, &reasoning) {
                        let key = corex_core::reasoning_cache::ReasoningCache::compute_key(
                            txt,
                            Some(&app.streaming_tool_calls),
                        );
                        app.llm_client.reasoning_cache().insert(key, cot.clone());
                    }

                    let valid_calls: Vec<ToolCall> = app.streaming_tool_calls
                        .drain(..)
                        .filter(|c| !c.function.name.trim().is_empty())
                        .collect();

                    if !valid_calls.is_empty() {
                        let calls = valid_calls;
                        app.session.add_message(Message::assistant_with_tools(
                            assistant_text,
                            reasoning,
                            calls.clone(),
                        ));

                        app.streaming_text.clear();
                        let _ = app.session.save();

                        // 1. Check if ANY tool in the batch needs user confirmation
                        let mut requires_confirmation = false;
                        let mut previews = Vec::new();

                        let confirmation_context = ToolContext {
                            workspace_dir: app.workspace_dir.clone(),
                            yolo_mode: app.always_allow_tools,
                            sudo_password: get_sudo_password(),
                            allowed_commands: app.llm_client.get_config().allowed_commands.clone(),
                        };

                        for call in &calls {
                            let tool = app.tool_registry.get(&call.function.name);
                            let args_json = serde_json::from_str(&call.function.arguments)
                                .unwrap_or(serde_json::Value::Null);

                            if let Some(t) = tool {
                                if t.needs_confirmation(&args_json, &confirmation_context)
                                    && !app.always_allow_tools
                                {
                                    requires_confirmation = true;
                                }
                                if call.function.name == "run_shell_command" {
                                    if let Some(cmd) = args_json.get("command").and_then(|c| c.as_str()) {
                                        previews.push(cmd.to_string());
                                    }
                                } else if let Some(diff) = t.format_diff(&args_json, &app.workspace_dir) {
                                    previews.push(diff);
                                }
                            }
                        }

                        // The interactive ask_user dialog takes priority over the
                        // confirmation modal: it must never run as a plain tool.
                        let ask_questions = calls
                            .iter()
                            .find(|c| c.function.name == "ask_user")
                            .and_then(|c| parse_questions(&c.function.arguments));

                        // Detect if any shell command requires sudo escalation
                        let sudo_needed_cmd = calls.iter().find_map(|c| {
                            if c.function.name == "run_shell_command" {
                                let args_json: serde_json::Value = serde_json::from_str(&c.function.arguments).ok()?;
                                let cmd = args_json.get("command")?.as_str()?;
                                if command_requires_sudo(cmd) {
                                    let extracted = extract_first_sudo_command(cmd).unwrap_or_else(|| cmd.to_string());
                                    return Some(extracted);
                                }
                            }
                            None
                        });

                        if let Some((questions, ask_call)) = ask_questions.and_then(|q| {
                            calls.iter().find(|c| c.function.name == "ask_user").map(|c| (q, c))
                        }) {
                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.clear_status();
                            app.user_dialog.open(questions, calls.clone(), ask_call.id.clone());
                        } else if let Some(sudo_cmd) = sudo_needed_cmd {
                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.clear_status();
                            app.sudo_dialog.open_batch(calls.clone(), sudo_cmd);
                        } else if requires_confirmation {
                            let combined_preview = if previews.is_empty() {
                                None
                            } else {
                                Some(previews.join("\n"))
                            };

                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.clear_status();
                            app.pending_confirmation = Some(PendingToolBatch {
                                calls: calls.clone(),
                                diff_preview: combined_preview,
                                selected_option: 0,
                                diff_expanded: false,
                            });
                        } else {
                            // Execute all tools concurrently in parallel without blocking UI!
                            let tx = event_tx.clone();
                            let registry = app.tool_registry.clone();
                            let context = confirmation_context;
                            let gen = app.current_generation_id;

                            tokio::spawn(async move {
                                let calls_count = calls.len();
                                if calls_count > 1 {
                                    let summary = if calls.iter().all(|c| c.function.name == "run_shell_command" || c.function.name == "execute_command") {
                                        format!("Running {} commands in parallel...", calls_count)
                                    } else {
                                        format!("Running {} tasks in parallel...", calls_count)
                                    };
                                    let _ = tx.send((gen, StreamEvent::ToolExecutionStarting {
                                        call_id: "batch".to_string(),
                                        name: "batch".to_string(),
                                        summary,
                                    })).await;
                                }

                                let mut handles = Vec::new();
                                for call in calls {
                                    let reg = registry.clone();
                                    let ctx = context.clone();
                                    let tx_call = tx.clone();
                                    let call_id = call.id.clone();
                                    let tool_name = call.function.name.clone();
                                    let args_str = call.function.arguments.clone();

                                    handles.push(tokio::spawn(async move {
                                        if calls_count == 1 {
                                            let summary = format_tool_call_summary(&tool_name, &args_str);
                                            let _ = tx_call.send((gen, StreamEvent::ToolExecutionStarting {
                                                call_id: call_id.clone(),
                                                name: tool_name.clone(),
                                                summary,
                                            })).await;
                                        }
                                        let start_tool = std::time::Instant::now();
                                        let args_json = serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
                                        let result = reg.execute(&tool_name, args_json, &ctx).await;
                                        let duration_ms = start_tool.elapsed().as_millis();
                                        let (output, success) = match result {
                                            Ok(o) => (o.output, true),
                                            Err(e) => (format!("Error executing {}: {}", tool_name, e), false),
                                        };
                                        corex_core::ForensicLogger::log_tool_call(
                                            &tool_name,
                                            &call_id,
                                            &args_str,
                                            &output,
                                            duration_ms,
                                            success
                                        );
                                        let _ = tx_call.send((gen, StreamEvent::ToolExecutionDone { call_id, output })).await;
                                    }));
                                }
                                for h in handles {
                                    let _ = h.await;
                                }
                                let _ = tx.send((gen, StreamEvent::AllToolsDone)).await;
                            });
                        }
                    } else {
                        // No tool calls — final model answer received!
                        app.is_streaming = false;
                        app.thinking_state.reset();
                        app.clear_status();

                        if let Some(txt) = assistant_text {
                            app.session.add_message(Message::assistant(txt, reasoning));
                        }

                        app.streaming_text.clear();
                        app.streaming_tool_calls.clear();
                        let _ = app.session.save();

                        // ⚡ Automatically dequeue and run next queued user prompt!
                        if let Some(next_prompt) = app.message_queue.pop_front() {
                            app.session.add_message(Message::user(next_prompt));
                            app.start_stream_turn(event_tx.clone());
                        }
                    }
                }
                StreamEvent::ContextCompacted { compacted_messages, notice } => {
                    app.session.messages = compacted_messages;
                    app.session.add_message(Message::system(notice));
                    app.invalidate_message_cache();
                    let _ = app.session.save();
                }
                StreamEvent::Notice(notice) => {
                    app.session.add_message(Message::system(notice));
                    app.invalidate_message_cache();
                }
                StreamEvent::Error(err) => {
                    app.is_streaming = false;
                    app.thinking_state.reset();
                    app.clear_status();
                    app.session.add_message(Message::system(format!("Error: {}", err)));
                    app.streaming_text.clear();
                    app.streaming_tool_calls.clear();
                    if let Some(next_prompt) = app.message_queue.pop_front() {
                        if app.input_buffer.is_empty() {
                            app.input_buffer = next_prompt;
                            app.cursor_idx = app.input_buffer.len();
                        }
                    }
                }
            }
        }

        if app.is_streaming && app.thinking_state.is_streaming {
            if let Some(start) = app.last_turn_start {
                app.thinking_state.elapsed_secs = start.elapsed().as_secs_f32();
            }
        }

        let tick_rate = if app.is_streaming || is_animating {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(80)
        };
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            needs_redraw = true;
            match event::read()? {
                Event::Paste(pasted) => {
                    app.handle_paste(pasted);
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        app.auto_scroll = false;
                        app.scroll_offset = app.scroll_offset.saturating_sub(3);
                    }
                    MouseEventKind::ScrollDown => {
                        let max_s = app.total_rendered_items.saturating_sub(10) as u16;
                        app.scroll_offset = (app.scroll_offset + 3).min(max_s);
                        if app.scroll_offset >= max_s {
                            app.auto_scroll = true;
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        if let Some(rect) = app.last_chat_rect {
                            if mouse.column >= rect.x
                                && mouse.column < rect.x + rect.width
                                && mouse.row >= rect.y
                                && mouse.row < rect.y + rect.height
                            {
                                let rel_y = (mouse.row - rect.y) as usize;
                                let line_idx = app.scroll_offset as usize + rel_y;
                                if line_idx < app.last_rendered_text_lines.len() {
                                    let line_text = &app.last_rendered_text_lines[line_idx];
                                    let click_col = mouse.column.saturating_sub(rect.x) as usize;
                                    if let Some(url) = find_url_in_line(line_text, Some(click_col)) {
                                        let _ = open::that(&url);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                },
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // --- 0. Cancel active streaming / generation immediately on Esc or Ctrl+C ---
                    if app.is_streaming
                        && (key.code == KeyCode::Esc
                            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)))
                        {
                            if key.code == KeyCode::Esc && !app.message_queue.is_empty() {
                                if let Some(queued) = app.message_queue.pop_back() {
                                    if app.input_buffer.is_empty() {
                                        app.input_buffer = queued;
                                        app.cursor_idx = app.input_buffer.len();
                                    }
                                }
                                needs_redraw = true;
                                continue;
                            }

                            if let Some(token) = app.cancel_token.take() {
                                token.cancel();
                            }
                            app.current_generation_id = app.current_generation_id.wrapping_add(1);
                            app.is_streaming = false;
                            app.thinking_state.is_streaming = false;
                            app.clear_status();
                            app.message_queue.clear();

                            let partial_text = if app.streaming_text.is_empty() {
                                None
                            } else {
                                Some(format!("{} *(interrupted)*", app.streaming_text.trim_end()))
                            };

                            let reasoning = if app.thinking_state.content.is_empty() {
                                None
                            } else {
                                Some(app.thinking_state.content.clone())
                            };

                            if let Some(txt) = partial_text {
                                app.session.add_message(Message::assistant(txt, reasoning));
                            } else {
                                app.session.add_message(Message::system("Generation cancelled by user (Esc)."));
                            }

                            app.streaming_text.clear();
                            app.streaming_tool_calls.clear();
                            let _ = app.session.save();
                            continue;
                        }

                    if app.is_checking_balance && key.code == KeyCode::Esc {
                        app.is_checking_balance = false;
                        app.clear_status();
                        if let Some(idx) = app.pending_balance_msg_index.take() {
                            if idx < app.session.messages.len() {
                                app.session.messages[idx] = Message::system("Balance check cancelled.");
                                app.invalidate_message_cache();
                            }
                        }
                        needs_redraw = true;
                        continue;
                    }

                    // --- 1. Sudo Password Dialog Active ---
                    if app.sudo_dialog.is_open {
                        match key.code {
                            KeyCode::Esc => {
                                let calls = if !app.sudo_dialog.pending_calls.is_empty() {
                                    std::mem::take(&mut app.sudo_dialog.pending_calls)
                                } else if let Some(call) = app.sudo_dialog.pending_call.take() {
                                    vec![call]
                                } else {
                                    Vec::new()
                                };
                                for call in calls {
                                    app.session.add_message(Message::tool_response(
                                        call.id,
                                        "Sudo authentication cancelled by user.",
                                    ));
                                }
                                app.start_stream_turn(event_tx.clone());
                                app.sudo_dialog.close();
                            }
                            KeyCode::Backspace => {
                                app.sudo_dialog.password_input.pop();
                            }
                            KeyCode::Char(c) => {
                                app.sudo_dialog.password_input.push(c);
                            }
                            KeyCode::Enter => {
                                let pwd = app.sudo_dialog.password_input.trim().to_string();
                                if pwd.is_empty() {
                                    app.sudo_dialog.error_msg = Some("Password cannot be empty.".to_string());
                                } else {
                                    // Sudo password is not persisted across commands
                                    set_sudo_password(None);
                                    let calls = if !app.sudo_dialog.pending_calls.is_empty() {
                                        std::mem::take(&mut app.sudo_dialog.pending_calls)
                                    } else if let Some(call) = app.sudo_dialog.pending_call.take() {
                                        vec![call]
                                    } else {
                                        Vec::new()
                                    };
                                    if !calls.is_empty() {
                                        let tx = event_tx.clone();
                                        let registry = app.tool_registry.clone();
                                        let gen = app.current_generation_id;
                                        let context = ToolContext {
                                            workspace_dir: app.workspace_dir.clone(),
                                            yolo_mode: app.always_allow_tools,
                                            sudo_password: Some(pwd),
                                            allowed_commands: app
                                                .llm_client
                                                .get_config()
                                                .allowed_commands
                                                .clone(),
                                        };
                                        app.is_streaming = true;
                                        tokio::spawn(async move {
                                            let calls_count = calls.len();
                                            if calls_count > 1 {
                                                let summary = if calls.iter().all(|c| c.function.name == "run_shell_command" || c.function.name == "execute_command") {
                                                    format!("Running {} commands in parallel...", calls_count)
                                                } else {
                                                    format!("Running {} tasks in parallel...", calls_count)
                                                };
                                                let _ = tx.send((gen, StreamEvent::ToolExecutionStarting {
                                                    call_id: "batch".to_string(),
                                                    name: "batch".to_string(),
                                                    summary,
                                                })).await;
                                            }

                                            let mut handles = Vec::new();
                                            for call in calls {
                                                let reg = registry.clone();
                                                let ctx = context.clone();
                                                let tx_call = tx.clone();
                                                let call_id = call.id.clone();
                                                let tool_name = call.function.name.clone();
                                                let args_str = call.function.arguments.clone();

                                                handles.push(tokio::spawn(async move {
                                                    if calls_count == 1 {
                                                        let summary = format_tool_call_summary(&tool_name, &args_str);
                                                        let _ = tx_call.send((gen, StreamEvent::ToolExecutionStarting {
                                                            call_id: call_id.clone(),
                                                            name: tool_name.clone(),
                                                            summary,
                                                        })).await;
                                                    }
                                                    let args_json = serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
                                                    let output = match reg.execute(&tool_name, args_json, &ctx).await {
                                                        Ok(o) => o.output,
                                                        Err(e) => format!("Execution error: {}", e),
                                                    };
                                                    let _ = tx_call.send((gen, StreamEvent::ToolExecutionDone { call_id, output })).await;
                                                }));
                                            }
                                            for h in handles {
                                                let _ = h.await;
                                            }
                                            let _ = tx.send((gen, StreamEvent::AllToolsDone)).await;
                                        });
                                    }
                                    app.sudo_dialog.close();
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- 1. First-Run API Key Auth Dialog Active ---
                    if app.auth_dialog.is_open {
                        match key.code {
                            KeyCode::Esc => {
                                app.auth_dialog.close();
                                app.session.add_message(Message::system(
                                    "Auth dialog skipped. Set your API key anytime via /key <sk-...> or select offline models via /model."
                                ));
                            }
                            KeyCode::Backspace => {
                                app.auth_dialog.input_buffer.pop();
                            }
                            KeyCode::Char(c) => {
                                app.auth_dialog.input_buffer.push(c);
                            }
                            KeyCode::Enter => {
                                let key_str = app.auth_dialog.input_buffer.trim().to_string();
                                if key_str.is_empty() {
                                    app.auth_dialog.error_msg = Some("API key cannot be empty.".to_string());
                                } else {
                                    let mut cfg = app.llm_client.get_config();
                                    cfg.api_key = key_str.clone();
                                    let _ = cfg.save_with_workspace(Some(&app.workspace_dir));
                                    app.llm_client.update_config(cfg);
                                    app.auth_dialog.close();
                                    app.session.add_message(Message::system("API key saved successfully to ~/.corex/settings.json. Ready to assist!"));
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- 1.5. Session Dialog Active ---
                    if app.session_dialog.is_open {
                        match key.code {
                            KeyCode::Esc => {
                                app.session_dialog.close();
                            }
                            KeyCode::Up => {
                                if app.session_dialog.selected_idx > 0 {
                                    app.session_dialog.selected_idx -= 1;
                                }
                            }
                            KeyCode::Down => {
                                if app.session_dialog.selected_idx + 1 < app.session_dialog.sessions.len() {
                                    app.session_dialog.selected_idx += 1;
                                }
                            }
                            KeyCode::Char('x') => {
                                if !app.session_dialog.sessions.is_empty() {
                                    let session_id = app.session_dialog.sessions[app.session_dialog.selected_idx].id.clone();
                                    let _ = Session::delete_by_id_or_tag(&session_id);
                                    // Refresh list
                                    app.session_dialog.open(&app.workspace_dir.display().to_string());
                                }
                            }
                            KeyCode::Enter => {
                                if !app.session_dialog.sessions.is_empty() {
                                    let session_id = app.session_dialog.sessions[app.session_dialog.selected_idx].id.clone();
                                    match Session::load_by_id_or_tag(&session_id) {
                                        Ok(loaded) => {
                                            let mut cfg = app.llm_client.get_config();
                                            cfg.model = loaded.model.clone();
                                            if let Some(t) = loaded.temperature {
                                                cfg.temperature = t;
                                            }
                                            if let Some(ref r) = loaded.reasoning_effort {
                                                cfg.reasoning_effort = r.clone();
                                            }
                                            app.llm_client.update_config(cfg);
                                            app.session = loaded;
                                            app.invalidate_message_cache();
                                            app.session.add_message(Message::system(format!(
                                                "Resumed session '{}' (Model: {}).",
                                                app.session.title, app.session.model
                                            )));
                                        }
                                        Err(e) => {
                                            app.session.add_message(Message::system(format!("Failed to load session: {}", e)));
                                        }
                                    }
                                }
                                app.session_dialog.close();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- 2. Model Dialog Active ---
                    if app.model_dialog.is_open {
                        match key.code {
                            KeyCode::Esc => {
                                app.model_dialog.close();
                            }
                            KeyCode::Tab => {
                                app.model_dialog.next_tab();
                            }
                            KeyCode::BackTab => {
                                app.model_dialog.prev_tab();
                            }
                            KeyCode::Char('1') => {
                                app.model_dialog.current_tab = ModelTab::Models;
                            }
                            KeyCode::Char('2') => {
                                app.model_dialog.current_tab = ModelTab::Flash;
                            }
                            KeyCode::Char('3') => {
                                app.model_dialog.current_tab = ModelTab::Pro;
                            }
                            KeyCode::Char('4') => {
                                app.model_dialog.current_tab = ModelTab::Hybrid;
                            }
                            KeyCode::Char('t') | KeyCode::Char('T') => {
                                match app.model_dialog.current_tab {
                                    ModelTab::Models => {
                                        app.model_dialog.persist_model = !app.model_dialog.persist_model;
                                    }
                                    ModelTab::Flash => {
                                        app.model_dialog.flash_persist_permanent =
                                            !app.model_dialog.flash_persist_permanent;
                                        if app.model_dialog.flash_persist_permanent {
                                            let flash_settings = app.model_dialog.to_flash_settings();
                                            let _ = Config::save_flash_settings(&flash_settings);
                                        }
                                    }
                                    ModelTab::Pro => {
                                        app.model_dialog.pro_persist_permanent =
                                            !app.model_dialog.pro_persist_permanent;
                                        if app.model_dialog.pro_persist_permanent {
                                            let pro_settings = app.model_dialog.to_pro_settings();
                                            let _ = Config::save_pro_settings(&pro_settings);
                                        }
                                    }
                                    ModelTab::Hybrid => {
                                        app.model_dialog.hybrid_persist_permanent =
                                            !app.model_dialog.hybrid_persist_permanent;
                                        if app.model_dialog.hybrid_persist_permanent {
                                            let hybrid_settings = app.model_dialog.to_hybrid_settings();
                                            let _ = Config::save_hybrid_settings(&hybrid_settings);
                                        }
                                    }
                                }
                            }
                            _ => {
                                match app.model_dialog.current_tab {
                                    ModelTab::Models => {
                                        match key.code {
                                            KeyCode::Up | KeyCode::Char('k') => {
                                                app.model_dialog.selected_model_idx =
                                                    app.model_dialog.selected_model_idx.saturating_sub(1);
                                            }
                                            KeyCode::Down | KeyCode::Char('j') => {
                                                app.model_dialog.selected_model_idx =
                                                    (app.model_dialog.selected_model_idx + 1).min(3);
                                            }
                                            KeyCode::Enter => {
                                                match app.model_dialog.selected_model_idx {
                                                    0 => {
                                                        app.model_dialog.active_engine = 0;
                                                        let mut cfg = app.llm_client.get_config();
                                                        cfg.model = "deepseek-flash".to_string();
                                                        cfg.local_llm_enabled = false;
                                                        cfg.hybrid_compression = false;
                                                        cfg.flash_settings = app.model_dialog.to_flash_settings();
                                                        cfg.temperature = cfg.flash_settings.temperature;
                                                        cfg.reasoning_effort = cfg.flash_settings.reasoning_effort.clone();
                                                        if app.model_dialog.persist_model {
                                                            let _ = cfg.save();
                                                        }
                                                        app.llm_client.update_config(cfg.clone());
                                                        app.session.model = cfg.model.clone();
                                                        app.session.temperature = Some(cfg.temperature);
                                                        app.session.reasoning_effort = Some(cfg.reasoning_effort.clone());
                                                        let _ = app.session.save();
                                                        app.session.add_message(Message::system(format!(
                                                            "Activated DeepSeek-V4.1-Flash (Fast MoE Engine)\n- Temperature: {:.1}\n- General Reasoning: {}\n- Command CoT: {}\n- Code CoT: {}",
                                                            cfg.temperature,
                                                            cfg.reasoning_effort,
                                                            cfg.flash_settings.command_reasoning_effort,
                                                            cfg.flash_settings.code_reasoning_effort
                                                        )));
                                                        app.model_dialog.close();
                                                    }
                                                    1 => {
                                                        app.model_dialog.active_engine = 1;
                                                        let mut cfg = app.llm_client.get_config();
                                                        cfg.model = "deepseek-pro".to_string();
                                                        cfg.local_llm_enabled = false;
                                                        cfg.hybrid_compression = false;
                                                        cfg.pro_settings = app.model_dialog.to_pro_settings();
                                                        cfg.reasoning_effort = cfg.pro_settings.reasoning_effort.clone();
                                                        if app.model_dialog.persist_model {
                                                            let _ = cfg.save();
                                                        }
                                                        app.llm_client.update_config(cfg.clone());
                                                        app.session.model = cfg.model.clone();
                                                        app.session.reasoning_effort = Some(cfg.reasoning_effort.clone());
                                                        let _ = app.session.save();
                                                        app.session.add_message(Message::system(format!(
                                                            "Activated DeepSeek-V4-Pro (Deep Reasoning Engine)\n- Reasoning Depth: {}\n- Search CoT: {}",
                                                            cfg.pro_settings.reasoning_effort,
                                                            cfg.pro_settings.search_reasoning_effort
                                                        )));
                                                        app.model_dialog.close();
                                                    }
                                                    2 => {
                                                        app.model_dialog.active_engine = 2;
                                                        let mut cfg = app.llm_client.get_config();
                                                        cfg.model = "local-assistant".to_string();
                                                        cfg.local_llm_enabled = true;
                                                        cfg.hybrid_compression = false;
                                                        cfg.local_llm_model = app.model_dialog.hybrid_secondary_local_model.clone();
                                                        cfg.local_llm_url = app.model_dialog.hybrid_local_url.clone();
                                                        if app.model_dialog.persist_model {
                                                            let _ = cfg.save();
                                                        }
                                                        app.llm_client.update_config(cfg.clone());
                                                        app.session.model = cfg.model.clone();
                                                        let _ = app.session.save();
                                                        app.session.add_message(Message::system(format!(
                                                            "Activated 100% Standalone Offline Local Assistant\n- Engine: {}\n- Endpoint: {}\n- Cost: $0.00 (Zero cloud telemetry)",
                                                            cfg.local_llm_model, cfg.local_llm_url
                                                        )));
                                                        app.model_dialog.close();
                                                    }
                                                    3 => {
                                                        app.model_dialog.active_engine = 3;
                                                        let mut cfg = app.llm_client.get_config();
                                                        cfg.model = "deepseek-flash".to_string();
                                                        cfg.local_llm_enabled = true;
                                                        cfg.hybrid_compression = true;
                                                        cfg.hybrid_settings = app.model_dialog.to_hybrid_settings();
                                                        cfg.local_llm_model = app.model_dialog.hybrid_secondary_local_model.clone();
                                                        cfg.local_llm_url = app.model_dialog.hybrid_local_url.clone();
                                                        if app.model_dialog.persist_model {
                                                            let _ = cfg.save();
                                                            let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                                                        }
                                                        app.llm_client.update_config(cfg.clone());
                                                        app.session.model = cfg.model.clone();
                                                        let _ = app.session.save();
                                                        app.session.add_message(Message::system(format!(
                                                            "Activated Smart Hybrid Dual-Engine Mode\n- Strategy: {}\n- Local SLM: {}\n- Endpoint: {}\n- Auto Compression: {}",
                                                            cfg.hybrid_settings.mode.display_name(),
                                                            cfg.local_llm_model,
                                                            cfg.local_llm_url,
                                                            if cfg.hybrid_settings.auto_compression { "Enabled (~70% token savings)" } else { "Disabled" }
                                                        )));
                                                        app.model_dialog.close();
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    ModelTab::Flash => {
                                        match key.code {
                                            KeyCode::Up | KeyCode::Char('k') => {
                                                app.model_dialog.flash_row_idx =
                                                    app.model_dialog.flash_row_idx.saturating_sub(1);
                                            }
                                            KeyCode::Down | KeyCode::Char('j') => {
                                                app.model_dialog.flash_row_idx =
                                                    (app.model_dialog.flash_row_idx + 1).min(5);
                                            }
                                            KeyCode::Left | KeyCode::Char('h') => {
                                                app.model_dialog.cycle_flash_row(false);
                                                let flash_settings = app.model_dialog.to_flash_settings();
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.flash_settings = flash_settings.clone();
                                                cfg.temperature = flash_settings.temperature;
                                                cfg.reasoning_effort = flash_settings.reasoning_effort.clone();
                                                if app.model_dialog.flash_persist_permanent {
                                                    let _ = Config::save_flash_settings(&flash_settings);
                                                    let _ = cfg.save();
                                                }
                                                app.llm_client.update_config(cfg);
                                            }
                                            KeyCode::Right | KeyCode::Char('l') => {
                                                app.model_dialog.cycle_flash_row(true);
                                                let flash_settings = app.model_dialog.to_flash_settings();
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.flash_settings = flash_settings.clone();
                                                cfg.temperature = flash_settings.temperature;
                                                cfg.reasoning_effort = flash_settings.reasoning_effort.clone();
                                                if app.model_dialog.flash_persist_permanent {
                                                    let _ = Config::save_flash_settings(&flash_settings);
                                                    let _ = cfg.save();
                                                }
                                                app.llm_client.update_config(cfg);
                                            }
                                            _ => {}
                                        }
                                    }
                                    ModelTab::Pro => {
                                        match key.code {
                                            KeyCode::Up | KeyCode::Char('k') => {
                                                app.model_dialog.pro_row_idx =
                                                    app.model_dialog.pro_row_idx.saturating_sub(1);
                                            }
                                            KeyCode::Down | KeyCode::Char('j') => {
                                                app.model_dialog.pro_row_idx =
                                                    (app.model_dialog.pro_row_idx + 1).min(2);
                                            }
                                            KeyCode::Left | KeyCode::Char('h') => {
                                                app.model_dialog.cycle_pro_row(false);
                                                let pro_settings = app.model_dialog.to_pro_settings();
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.pro_settings = pro_settings.clone();
                                                cfg.reasoning_effort = pro_settings.reasoning_effort.clone();
                                                if app.model_dialog.pro_persist_permanent {
                                                    let _ = Config::save_pro_settings(&pro_settings);
                                                    let _ = cfg.save();
                                                }
                                                app.llm_client.update_config(cfg);
                                            }
                                            KeyCode::Right | KeyCode::Char('l') => {
                                                app.model_dialog.cycle_pro_row(true);
                                                let pro_settings = app.model_dialog.to_pro_settings();
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.pro_settings = pro_settings.clone();
                                                cfg.reasoning_effort = pro_settings.reasoning_effort.clone();
                                                if app.model_dialog.pro_persist_permanent {
                                                    let _ = Config::save_pro_settings(&pro_settings);
                                                    let _ = cfg.save();
                                                }
                                                app.llm_client.update_config(cfg);
                                            }
                                            _ => {}
                                        }
                                    }
                                    ModelTab::Hybrid => {
                                        match key.code {
                                            KeyCode::Up | KeyCode::Char('k') => {
                                                app.model_dialog.hybrid_row_idx =
                                                    app.model_dialog.hybrid_row_idx.saturating_sub(1);
                                            }
                                            KeyCode::Down | KeyCode::Char('j') => {
                                                app.model_dialog.hybrid_row_idx =
                                                    (app.model_dialog.hybrid_row_idx + 1).min(4);
                                            }
                                            KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
                                                let forward = matches!(key.code, KeyCode::Right | KeyCode::Char('l'));
                                                app.model_dialog.cycle_hybrid_row(forward);
                                                let hybrid_settings = app.model_dialog.to_hybrid_settings();
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.hybrid_settings = hybrid_settings.clone();
                                                cfg.local_llm_url = hybrid_settings.local_url.clone();
                                                cfg.local_llm_model = hybrid_settings.secondary_local_model.clone();
                                                cfg.hybrid_compression = hybrid_settings.auto_compression;
                                                cfg.local_prompt_lite = app.model_dialog.local_prompt_lite;
                                                if app.model_dialog.hybrid_persist_permanent {
                                                    let _ = Config::save_hybrid_settings(&hybrid_settings);
                                                    let _ = cfg.save();
                                                }
                                                app.llm_client.update_config(cfg);
                                            }
                                            KeyCode::Char('c') | KeyCode::Char('C') => {
                                                let is_up = app.llm_client.local_client().health_check().await;
                                                if is_up {
                                                    app.model_dialog.server_status_msg = Some((
                                                        format!("ONLINE (Reachable at {})", app.model_dialog.hybrid_local_url),
                                                        ratatui::style::Color::Rgb(105, 240, 174),
                                                    ));
                                                } else {
                                                    app.model_dialog.server_status_msg = Some((
                                                        format!("OFFLINE (Cannot connect to {})", app.model_dialog.hybrid_local_url),
                                                        ratatui::style::Color::Rgb(244, 67, 54),
                                                    ));
                                                }
                                            }
                                            KeyCode::Enter => {
                                                // Activate pure Local mode (no cloud hybrid)
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.model = "local-assistant".to_string();
                                                cfg.local_llm_enabled = true;
                                                cfg.hybrid_compression = false;
                                                cfg.local_llm_model = app.model_dialog.hybrid_secondary_local_model.clone();
                                                cfg.local_llm_url = app.model_dialog.hybrid_local_url.clone();
                                                cfg.local_prompt_lite = app.model_dialog.local_prompt_lite;
                                                if app.model_dialog.hybrid_persist_permanent {
                                                    let _ = cfg.save();
                                                }
                                                app.model_dialog.active_engine = 2;
                                                app.llm_client.update_config(cfg.clone());
                                                app.session.model = cfg.model.clone();
                                                let _ = app.session.save();
                                                let prompt_mode = if cfg.local_prompt_lite { "Lite (SLM-optimized)" } else { "Full (Corex standard)" };
                                                app.session.add_message(Message::system(format!(
                                                    "Activated 100% Offline Local Mode\n- Model: {}\n- Endpoint: {}\n- Prompt Mode: {}\n- Cost: $0.00",
                                                    cfg.local_llm_model,
                                                    cfg.local_llm_url,
                                                    prompt_mode
                                                )));
                                                app.model_dialog.close();
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    // --- 3. Tool Confirmation Modal Active (1:1 DeepSeek Radio Selection) ---
                    if let Some(ref mut pending) = app.pending_confirmation {
                        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
                            pending.diff_expanded = !pending.diff_expanded;
                            continue;
                        }
                        match key.code {
                            KeyCode::Up => {
                                pending.selected_option = pending.selected_option.saturating_sub(1);
                                continue;
                            }
                            KeyCode::Down => {
                                pending.selected_option = (pending.selected_option + 1).min(2);
                                continue;
                            }
                            _ => {}
                        }

                        let should_allow_once = key.code == KeyCode::Char('1')
                            || key.code == KeyCode::Char('y')
                            || key.code == KeyCode::Char('Y')
                            || (key.code == KeyCode::Enter && pending.selected_option == 0);

                        let should_allow_session = key.code == KeyCode::Char('2')
                            || key.code == KeyCode::Char('a')
                            || key.code == KeyCode::Char('A')
                            || (key.code == KeyCode::Enter && pending.selected_option == 1);

                        let should_deny = key.code == KeyCode::Char('3')
                            || key.code == KeyCode::Char('n')
                            || key.code == KeyCode::Char('N')
                            || key.code == KeyCode::Esc
                            || (key.code == KeyCode::Enter && pending.selected_option == 2);

                        if should_allow_once || should_allow_session {
                            if should_allow_session {
                                app.always_allow_tools = true;
                            }

                            let pending_batch = app.pending_confirmation.take().unwrap();
                            let calls = pending_batch.calls;

                            // Check if sudo password is required before running
                            let sudo_needed = calls.iter().find_map(|c| {
                                if c.function.name == "run_shell_command" {
                                    let args_json: serde_json::Value = serde_json::from_str(&c.function.arguments).ok()?;
                                    let cmd = args_json.get("command")?.as_str()?;
                                    if command_requires_sudo(cmd) {
                                        let extracted = extract_first_sudo_command(cmd).unwrap_or_else(|| cmd.to_string());
                                        return Some(extracted);
                                    }
                                }
                                None
                            });

                            if let Some(sudo_cmd) = sudo_needed {
                                app.sudo_dialog.open_batch(calls, sudo_cmd);
                                continue;
                            }

                            let context = ToolContext {
                                workspace_dir: app.workspace_dir.clone(),
                                yolo_mode: app.always_allow_tools,
                                sudo_password: get_sudo_password(),
                                allowed_commands: app.llm_client.get_config().allowed_commands.clone(),
                            };

                            let tx = event_tx.clone();
                            let registry = app.tool_registry.clone();
                            let gen = app.current_generation_id;
                            app.is_streaming = true;
                            app.last_turn_start = Some(Instant::now());

                            tokio::spawn(async move {
                                let calls_count = calls.len();
                                if calls_count > 1 {
                                    let summary = if calls.iter().all(|c| c.function.name == "run_shell_command" || c.function.name == "execute_command") {
                                        format!("Running {} commands in parallel...", calls_count)
                                    } else {
                                        format!("Running {} tasks in parallel...", calls_count)
                                    };
                                    let _ = tx.send((gen, StreamEvent::ToolExecutionStarting {
                                        call_id: "batch".to_string(),
                                        name: "batch".to_string(),
                                        summary,
                                    })).await;
                                }

                                let mut handles = Vec::new();
                                for call in calls {
                                    let reg = registry.clone();
                                    let ctx = context.clone();
                                    let tx_call = tx.clone();
                                    let call_id = call.id.clone();
                                    let tool_name = call.function.name.clone();
                                    let args_str = call.function.arguments.clone();

                                    handles.push(tokio::spawn(async move {
                                        if calls_count == 1 {
                                            let summary = format_tool_call_summary(&tool_name, &args_str);
                                            let _ = tx_call.send((gen, StreamEvent::ToolExecutionStarting {
                                                call_id: call_id.clone(),
                                                name: tool_name.clone(),
                                                summary,
                                            })).await;
                                        }
                                        let args_json = serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
                                        let output = match reg.execute(&tool_name, args_json, &ctx).await {
                                            Ok(o) => o.output,
                                            Err(e) => format!("Execution error: {}", e),
                                        };
                                        let _ = tx_call.send((gen, StreamEvent::ToolExecutionDone { call_id, output })).await;
                                    }));
                                }
                                for h in handles {
                                    let _ = h.await;
                                }
                                let _ = tx.send((gen, StreamEvent::AllToolsDone)).await;
                            });
                            continue;
                        } else if should_deny {
                            let pending_batch = app.pending_confirmation.take().unwrap();
                            for call in pending_batch.calls {
                                app.session.add_message(Message::tool_response(
                                    call.id,
                                    "Tool execution denied by user.",
                                ));
                            }

                            // ⚡ TRIGGER NEXT RECURSIVE TURN OF AGENT LOOP!
                            app.start_stream_turn(event_tx.clone());
                            continue;
                        }
                    }

                    // --- 3.5. Interactive Ask-User Dialog Active ---
                    if app.user_dialog.is_open {
                        match key.code {
                            KeyCode::Esc => {
                                app.user_dialog.cancelled = true;
                                finish_ask_user(&mut app, event_tx.clone());
                            }
                            KeyCode::Up => {
                                let cur = app.user_dialog.current;
                                if let Some(q) = app.user_dialog.current_question() {
                                    if q.has_options && app.user_dialog.selected[cur] > 0 {
                                        app.user_dialog.selected[cur] -= 1;
                                    }
                                }
                            }
                            KeyCode::Down => {
                                let cur = app.user_dialog.current;
                                if let Some(q) = app.user_dialog.current_question() {
                                    if q.has_options {
                                        let max = q.options.len().saturating_sub(1);
                                        if app.user_dialog.selected[cur] < max {
                                            app.user_dialog.selected[cur] += 1;
                                        }
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                let cur = app.user_dialog.current;
                                if !app.user_dialog.questions[cur].has_options {
                                    app.user_dialog.text_input[cur].pop();
                                }
                            }
                            KeyCode::Char(c) => {
                                let cur = app.user_dialog.current;
                                if !app.user_dialog.questions[cur].has_options {
                                    app.user_dialog.text_input[cur].push(c);
                                }
                            }
                            KeyCode::Enter => {
                                let cur = app.user_dialog.current;
                                let total = app.user_dialog.questions.len();
                                if cur + 1 < total {
                                    app.user_dialog.current += 1;
                                } else {
                                    finish_ask_user(&mut app, event_tx.clone());
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- 4. Global Control Hotkeys ---
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        match key.code {
                            KeyCode::Char('c') => {
                                let is_recent = app.last_ctrl_c_press
                                    .map(|i| i.elapsed() <= std::time::Duration::from_secs(2))
                                    .unwrap_or(false);

                                if is_recent {
                                    break;
                                } else {
                                    app.last_ctrl_c_press = Some(std::time::Instant::now());
                                }
                            }
                            KeyCode::Char('t') => {
                                app.thinking_state.is_expanded = !app.thinking_state.is_expanded;
                            }
                            KeyCode::Char('l') => {
                                terminal.clear()?;
                                needs_redraw = true;
                            }
                            KeyCode::Char('o') => {
                                app.logs_expanded = !app.logs_expanded;
                                app.invalidate_message_cache();
                                needs_redraw = true;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // --- 5. Input Prompt & Slash Autocomplete Navigation ---
                    let is_slash_open = app.input_buffer.starts_with('/');
                    let filter = app.input_buffer.to_lowercase();
                    let matching_cmds: Vec<&'static str> = ALL_COMMANDS
                        .iter()
                        .filter(|c| c.name.starts_with(&filter))
                        .map(|c| c.name)
                        .collect();

                    match key.code {
                        KeyCode::Left => {
                            if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
                                let before = &app.input_buffer[..app.cursor_idx];
                                let trimmed = before.trim_end();
                                let non_space = match trimmed.rfind(' ') {
                                    Some(pos) => pos + 1,
                                    None => 0,
                                };
                                app.cursor_idx = non_space;
                            } else if app.cursor_idx > 0 {
                                let before = &app.input_buffer[..app.cursor_idx];
                                if before.ends_with(']') {
                                    if let Some(open_idx) = before.rfind("[Pasted text #") {
                                        app.cursor_idx = open_idx;
                                    } else {
                                        let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                        app.cursor_idx = prev;
                                    }
                                } else {
                                    let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                    app.cursor_idx = prev;
                                }
                            }
                            app.clamp_cursor();
                        }
                        KeyCode::Right => {
                            if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
                                let after = &app.input_buffer[app.cursor_idx..];
                                let trimmed = after.trim_start();
                                let skip_spaces = after.len() - trimmed.len();
                                let word_len = trimmed.find(' ').unwrap_or(trimmed.len());
                                app.cursor_idx = (app.cursor_idx + skip_spaces + word_len).min(app.input_buffer.len());
                            } else if app.cursor_idx < app.input_buffer.len() {
                                let after = &app.input_buffer[app.cursor_idx..];
                                if after.starts_with("[Pasted text #") {
                                    if let Some(end_bracket) = after.find(']') {
                                        app.cursor_idx = (app.cursor_idx + end_bracket + 1).min(app.input_buffer.len());
                                    } else {
                                        let next = after.chars().next().map(|c| app.cursor_idx + c.len_utf8()).unwrap_or(app.input_buffer.len());
                                        app.cursor_idx = next;
                                    }
                                } else {
                                    let next = after.chars().next().map(|c| app.cursor_idx + c.len_utf8()).unwrap_or(app.input_buffer.len());
                                    app.cursor_idx = next;
                                }
                            }
                            app.clamp_cursor();
                        }
                        KeyCode::Home => {
                            app.cursor_idx = 0;
                        }
                        KeyCode::End => {
                            app.cursor_idx = app.input_buffer.len();
                        }
                        KeyCode::Delete => {
                            app.clamp_cursor();
                            if app.cursor_idx < app.input_buffer.len() {
                                let after = &app.input_buffer[app.cursor_idx..];
                                if after.starts_with("[Pasted text #") {
                                    if let Some(end_bracket) = after.find(']') {
                                        let tag_slice = &after[..=end_bracket];
                                        if let Some(num_str) = tag_slice.strip_prefix("[Pasted text #") {
                                            if let Some(space_pos) = num_str.find(' ') {
                                                if let Ok(id) = num_str[..space_pos].parse::<usize>() {
                                                    app.pastes.remove(&id);
                                                }
                                            }
                                        }
                                        app.input_buffer.drain(app.cursor_idx..app.cursor_idx + end_bracket + 1);
                                    } else {
                                        let ch_len = after.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                                        app.input_buffer.drain(app.cursor_idx..app.cursor_idx + ch_len);
                                    }
                                } else {
                                    let ch_len = after.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                                    app.input_buffer.drain(app.cursor_idx..app.cursor_idx + ch_len);
                                }
                                app.clamp_cursor();
                            }
                        }
                        KeyCode::Char(c) => {
                            if key.modifiers.contains(KeyModifiers::CONTROL) {
                                match c {
                                    'a' => app.cursor_idx = 0,
                                    'e' => app.cursor_idx = app.input_buffer.len(),
                                    'u' => {
                                        app.input_buffer.drain(..app.cursor_idx);
                                        app.cursor_idx = 0;
                                    }
                                    'k' => {
                                        app.input_buffer.truncate(app.cursor_idx);
                                    }
                                    'w' => {
                                        let before = &app.input_buffer[..app.cursor_idx];
                                        let trimmed = before.trim_end();
                                        let non_space = match trimmed.rfind(' ') {
                                            Some(pos) => pos + 1,
                                            None => 0,
                                        };
                                        app.input_buffer.drain(non_space..app.cursor_idx);
                                        app.cursor_idx = non_space;
                                    }
                                    _ => {}
                                }
                            } else {
                                app.clamp_cursor();
                                app.input_buffer.insert(app.cursor_idx, c);
                                app.cursor_idx += c.len_utf8();
                                app.slash_selected_idx = 0;
                                app.last_esc_press = None;
                            }
                        }
                        KeyCode::Backspace => {
                            app.clamp_cursor();
                            if app.cursor_idx > 0 {
                                let before = &app.input_buffer[..app.cursor_idx];
                                if before.ends_with(']') {
                                    if let Some(open_idx) = before.rfind("[Pasted text #") {
                                        let tag_slice = &before[open_idx..];
                                        if tag_slice.ends_with(']') {
                                            if let Some(num_str) = tag_slice.strip_prefix("[Pasted text #") {
                                                if let Some(space_pos) = num_str.find(' ') {
                                                    if let Ok(id) = num_str[..space_pos].parse::<usize>() {
                                                        app.pastes.remove(&id);
                                                    }
                                                }
                                            }
                                            app.input_buffer.drain(open_idx..app.cursor_idx);
                                            app.cursor_idx = open_idx;
                                        } else {
                                            let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                            app.input_buffer.drain(prev..app.cursor_idx);
                                            app.cursor_idx = prev;
                                        }
                                    } else {
                                        let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                        app.input_buffer.drain(prev..app.cursor_idx);
                                        app.cursor_idx = prev;
                                    }
                                } else {
                                    let prev = before.char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
                                    app.input_buffer.drain(prev..app.cursor_idx);
                                    app.cursor_idx = prev;
                                }
                            }
                            app.clamp_cursor();
                            app.slash_selected_idx = 0;
                            app.last_esc_press = None;
                        }
                        KeyCode::Esc => {
                            if is_slash_open && !matching_cmds.is_empty() {
                                app.input_buffer.clear();
                                app.cursor_idx = 0;
                                app.pastes.clear();
                                app.slash_selected_idx = 0;
                                app.last_esc_press = None;
                            } else {
                                let is_recent = app.last_esc_press
                                    .map(|i| i.elapsed() <= Duration::from_millis(500))
                                    .unwrap_or(false);

                                if is_recent {
                                    // 2nd ESC within 500ms -> Clear prompt & reset history navigation!
                                    app.input_buffer.clear();
                                    app.cursor_idx = 0;
                                    app.pastes.clear();
                                    app.history_idx = None;
                                    app.saved_draft.clear();
                                    app.slash_selected_idx = 0;
                                    app.slash_popup_height_current = 0.0;
                                    app.last_esc_press = None;
                                } else {
                                    // 1st ESC -> Start 500ms window to show toast
                                    app.last_esc_press = Some(Instant::now());
                                }
                            }
                        }
                        KeyCode::Tab => {
                            if is_slash_open && !matching_cmds.is_empty() {
                                let selected = matching_cmds[app.slash_selected_idx % matching_cmds.len()];
                                app.input_buffer = format!("{} ", selected);
                                app.cursor_idx = app.input_buffer.len();
                            }
                        }
                        KeyCode::Up => {
                            if is_slash_open && !matching_cmds.is_empty() {
                                if app.slash_selected_idx > 0 {
                                    app.slash_selected_idx -= 1;
                                } else {
                                    app.slash_selected_idx = matching_cmds.len().saturating_sub(1);
                                }
                            } else if !app.input_history.is_empty() {
                                match app.history_idx {
                                    None => {
                                        app.saved_draft = app.input_buffer.clone();
                                        let last_idx = app.input_history.len() - 1;
                                        app.history_idx = Some(last_idx);
                                        app.input_buffer = app.input_history[last_idx].clone();
                                        app.cursor_idx = app.input_buffer.len();
                                    }
                                    Some(i) => {
                                        if i > 0 {
                                            let next_i = i - 1;
                                            app.history_idx = Some(next_i);
                                            app.input_buffer = app.input_history[next_i].clone();
                                            app.cursor_idx = app.input_buffer.len();
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Down => {
                            if is_slash_open && !matching_cmds.is_empty() {
                                app.slash_selected_idx = (app.slash_selected_idx + 1) % matching_cmds.len();
                            } else if let Some(i) = app.history_idx {
                                if i + 1 < app.input_history.len() {
                                    let next_i = i + 1;
                                    app.history_idx = Some(next_i);
                                    app.input_buffer = app.input_history[next_i].clone();
                                    app.cursor_idx = app.input_buffer.len();
                                } else {
                                    app.history_idx = None;
                                    app.input_buffer = std::mem::take(&mut app.saved_draft);
                                    app.cursor_idx = app.input_buffer.len();
                                }
                            }
                        }
                        KeyCode::PageUp => {
                            app.auto_scroll = false;
                            app.scroll_offset = app.scroll_offset.saturating_sub(10);
                        }
                        KeyCode::PageDown => {
                            let max_s = app.total_rendered_items.saturating_sub(10) as u16;
                            app.scroll_offset = (app.scroll_offset + 10).min(max_s);
                            if app.scroll_offset >= max_s {
                                app.auto_scroll = true;
                            }
                        }
                        KeyCode::Enter => {
                            let text = app.input_buffer.trim().to_string();
                            if !text.is_empty() {
                                let cmd_to_run = if is_slash_open && !matching_cmds.is_empty() && !text.contains(' ') {
                                    matching_cmds[app.slash_selected_idx % matching_cmds.len()].to_string()
                                } else {
                                    text.clone()
                                };

                                if app.input_history.last().map(|s| s.as_str()) != Some(&cmd_to_run) {
                                    app.input_history.push(cmd_to_run.clone());
                                    corex_core::HistoryStore::append(&cmd_to_run);
                                }
                                app.history_idx = None;
                                app.saved_draft.clear();
                                app.input_buffer.clear();
                                app.cursor_idx = 0;
                                app.slash_selected_idx = 0;
                                app.slash_popup_height_current = 0.0;

                                if cmd_to_run == "/quit" || cmd_to_run == "/exit" {
                                    break;
                                }

                                if cmd_to_run.starts_with('/')
                                    && app.handle_slash_command(&cmd_to_run).await {
                                        continue;
                                    }

                                let mut user_prompt_clean = cmd_to_run.clone();

                                // Strip $sudo: if present
                                if let Some(pos) = user_prompt_clean.find("$sudo:") {
                                    let after = &user_prompt_clean[pos + 6..];
                                    let end = after.find(' ').unwrap_or(after.len());
                                    let before = &user_prompt_clean[..pos];
                                    let rest = &after[end..];
                                    user_prompt_clean = format!("{} {}", before.trim(), rest.trim()).trim().to_string();
                                }

                                // Clean up $auto / $yolo prefix if present without mutating global session mode
                                if user_prompt_clean.starts_with("$auto") || user_prompt_clean.starts_with("$yolo") {
                                    user_prompt_clean = user_prompt_clean
                                        .trim_start_matches("$auto")
                                        .trim_start_matches("$yolo")
                                        .trim()
                                        .to_string();
                                }

                                if user_prompt_clean.is_empty() {
                                    continue;
                                }

                                for (id, pasted_content) in &app.pastes {
                                    let prefix = format!("[Pasted text #{}", id);
                                    while let Some(start_idx) = user_prompt_clean.find(&prefix) {
                                        let rest = &user_prompt_clean[start_idx..];
                                        if let Some(end_bracket) = rest.find(']') {
                                            user_prompt_clean.replace_range(start_idx..start_idx + end_bracket + 1, pasted_content);
                                        } else {
                                            break;
                                        }
                                    }
                                }
                                app.pastes.clear();

                                if app.is_streaming {
                                    app.message_queue.push_back(user_prompt_clean);
                                    needs_redraw = true;
                                    continue;
                                }

                                app.session.add_message(Message::user(user_prompt_clean));

                                // ⚡ START TURN!
                                app.start_stream_turn(event_tx.clone());
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        } else if is_animating || app.is_streaming {
            needs_redraw = true;
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    drop(_guard);

    print_session_summary(&app);
    Ok(())
}

fn print_session_summary(app: &App) {
    let cfg = app.llm_client.get_config();
    let is_local = cfg.local_llm_enabled;

    let model_display = if is_local {
        if !cfg.local_llm_model.is_empty() {
            format!("Local Offline Assistant ({})", cfg.local_llm_model)
        } else {
            "Local Offline Assistant (Air-gapped SLM)".to_string()
        }
    } else if cfg.model == "deepseek-flash" || cfg.model == "deepseek-v4.1-flash" || cfg.model == "deepseek-v4-flash" || cfg.model == "deepseek-chat" {
        "DeepSeek-V4.1-Flash".to_string()
    } else if cfg.model == "deepseek-v4-pro" || cfg.model == "deepseek-reasoner" {
        "DeepSeek-V4-Pro".to_string()
    } else {
        cfg.model.clone()
    };

    let elapsed = chrono::Utc::now().signed_duration_since(app.session.created_at).num_seconds().max(0);
    let duration_str = if elapsed < 60 {
        format!("{}s", elapsed)
    } else {
        format!("{}m {}s", elapsed / 60, elapsed % 60)
    };

    let msg_count = app.session.messages.len();
    let u = &app.session.total_usage;

    let (cost_str, tokens_str, cache_str) = if is_local {
        (
            "\x1b[38;2;105;240;174m$0.000000 USD\x1b[0m \x1b[90m(100% Free Local Hardware)\x1b[0m".to_string(),
            format!("{} in · {} out ({} total)", u.prompt_tokens, u.completion_tokens, u.total_tokens),
            None,
        )
    } else if u.total_tokens > 0 {
        let is_pro = cfg.model.contains("pro") || cfg.model.contains("reasoner");
        let (cached_rate, miss_rate, comp_rate) = if is_pro {
            (0.14 / 1_000_000.0, 0.55 / 1_000_000.0, 2.19 / 1_000_000.0)
        } else {
            (0.014 / 1_000_000.0, 0.14 / 1_000_000.0, 0.28 / 1_000_000.0)
        };
        let miss = u.prompt_tokens.saturating_sub(u.prompt_cache_hit_tokens);
        let actual_cost = (u.prompt_cache_hit_tokens as f64 * cached_rate)
            + (miss as f64 * miss_rate)
            + (u.completion_tokens as f64 * comp_rate);
        let un_cached = (u.prompt_tokens as f64 * miss_rate) + (u.completion_tokens as f64 * comp_rate);
        let saved = (un_cached - actual_cost).max(0.0);

        let hit_rate = u.cache_hit_percentage();
        let cost_text = if saved > 0.00001 {
            format!("\x1b[38;2;105;240;174m${:.6} USD\x1b[0m \x1b[90m(Saved ${:.6} with KV Cache)\x1b[0m", actual_cost, saved)
        } else {
            format!("\x1b[38;2;105;240;174m${:.6} USD\x1b[0m", actual_cost)
        };

        let cache_info = if u.prompt_tokens > 0 {
            Some(format!("{:.1}% Hit Rate ({} cached / {} in)", hit_rate, u.prompt_cache_hit_tokens, u.prompt_tokens))
        } else {
            None
        };

        (
            cost_text,
            format!("{} in · {} out ({} total)", u.prompt_tokens, u.completion_tokens, u.total_tokens),
            cache_info,
        )
    } else {
        (
            "\x1b[90m$0.000000 USD (No LLM turns)\x1b[0m".to_string(),
            "0 total".to_string(),
            None,
        )
    };

    println!();
    println!("\x1b[1;38;2;56;189;248m✦ UTI Session Summary\x1b[0m");
    println!("  \x1b[90mModel:\x1b[0m     \x1b[1m{}\x1b[0m", model_display);
    println!("  \x1b[90mDuration:\x1b[0m  {} \x1b[90m·\x1b[0m {} messages", duration_str, msg_count);
    println!("  \x1b[90mTokens:\x1b[0m    {}", tokens_str);
    if let Some(c_info) = cache_str {
        println!("  \x1b[90mKV Cache:\x1b[0m  {}", c_info);
    }
    println!("  \x1b[90mCost:\x1b[0m      {}", cost_str);
    println!();
    println!("  \x1b[90mResume:\x1b[0m    \x1b[38;2;135;215;215muti --resume {}\x1b[0m", app.session.id);
    println!();
}

/// Finishes the interactive ask_user dialog: executes the pending tool batch
/// (feeding the user's answers to the `ask_user` call), then triggers the next
/// agent turn via `AllToolsDone`.
fn finish_ask_user(app: &mut App, event_tx: mpsc::Sender<(u64, StreamEvent)>) {
    let tx = event_tx.clone();
    let registry = app.tool_registry.clone();
    let gen = app.current_generation_id;
    let context = ToolContext {
        workspace_dir: app.workspace_dir.clone(),
        yolo_mode: app.always_allow_tools,
        sudo_password: get_sudo_password(),
        allowed_commands: app.llm_client.get_config().allowed_commands.clone(),
    };
    let calls = app.user_dialog.calls.clone();
    let output = app.user_dialog.format_output();
    let ask_call_id = app.user_dialog.call_id.clone();
    let cancelled = app.user_dialog.cancelled;
    app.user_dialog.close();
    app.is_streaming = true;

    tokio::spawn(async move {
        for call in calls {
            if call.id == ask_call_id {
                let _ = tx
                    .send((gen, StreamEvent::ToolExecutionDone {
                        call_id: call.id,
                        output: output.clone(),
                    }))
                    .await;
            } else if cancelled {
                // Batch aborted: report every remaining call as skipped.
                let _ = tx
                    .send((gen, StreamEvent::ToolExecutionDone {
                        call_id: call.id,
                        output: "Skipped: user cancelled the pending questions.".to_string(),
                    }))
                    .await;
            } else {
                let args_json = serde_json::from_str(&call.function.arguments)
                    .unwrap_or(serde_json::Value::Null);
                let out = match registry.execute(&call.function.name, args_json, &context).await {
                    Ok(o) => o.output,
                    Err(e) => format!("Error executing {}: {}", call.function.name, e),
                };
                let _ = tx
                    .send((gen, StreamEvent::ToolExecutionDone { call_id: call.id, output: out }))
                    .await;
            }
        }
        let _ = tx.send((gen, StreamEvent::AllToolsDone)).await;
    });
}

pub fn find_url_in_line(line_text: &str, click_col: Option<usize>) -> Option<String> {
    let mut urls = Vec::new();
    let mut start = 0;
    while start < line_text.len() {
        let remainder = &line_text[start..];
        let pos = remainder.find("https://").or_else(|| remainder.find("http://"));
        let p = match pos {
            Some(idx) => idx,
            None => break,
        };
        let abs_start = start + p;
        let url_rem = &line_text[abs_start..];
        let end = url_rem
            .find(|c: char| c.is_whitespace() || c == ')' || c == ']' || c == '>' || c == '"' || c == '\'' || c == '│' || c == '|' || c == '}' || c == '<')
            .unwrap_or(url_rem.len());
        let mut url = url_rem[..end].to_string();
        while url.ends_with('.') || url.ends_with(',') || url.ends_with(';') {
            url.pop();
        }
        let abs_end = abs_start + url.len();
        if !url.is_empty() {
            urls.push((abs_start, abs_end, url));
        }
        start = abs_start + end.max(1);
    }

    if urls.is_empty() {
        return None;
    }

    if let Some(col) = click_col {
        for (s, e, u) in &urls {
            if col + 3 >= *s && col <= *e + 3 {
                return Some(u.clone());
            }
        }
    }

    if urls.len() == 1 {
        return Some(urls[0].2.clone());
    }

    None
}

pub fn format_tool_call_summary(name: &str, raw_args: &str) -> String {
    let args_val = serde_json::from_str::<serde_json::Value>(raw_args).ok();
    match name {
        "run_shell_command" | "shell" | "run_command" => {
            let cmd = args_val.as_ref()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()))
                .unwrap_or(raw_args)
                .trim();
            if cmd.is_empty() {
                "Running command...".to_string()
            } else {
                format!("Running command: {}", corex_core::truncate_ellipsis(cmd, 50))
            }
        }
        "read_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            format!("Reading file: {}", path)
        }
        "write_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            format!("Writing file: {}", path)
        }
        "edit" | "apply_patch" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            format!("Editing file: {}", path)
        }
        "list_directory" | "list_dir" | "glob" | "ls" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("dir_path").or_else(|| v.get("path")).or_else(|| v.get("dir")).and_then(|p| p.as_str()))
                .unwrap_or(".");
            format!("Exploring directory: {}", path)
        }
        _ => format!("Executing: {}", name),
    }
}

fn format_tool_call_spans(name: &str, raw_args: &str, theme: &Theme) -> Vec<Span<'static>> {
    let args_val = serde_json::from_str::<serde_json::Value>(raw_args).ok();
    match name {
        "run_shell_command" | "shell" => {
            let cmd = args_val.as_ref()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()))
                .unwrap_or(raw_args);
            let single_line_cmd = cmd.replace('\n', " ").replace("  ", " ");
            let display_cmd = corex_core::truncate_ellipsis(&single_line_cmd, 80);
            vec![
                Span::styled("  $ ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
                Span::styled(display_cmd, Style::default().fg(theme.accent_cyan)),
            ]
        }
        "list_directory" | "list_dir" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("dir_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("  [DIR] list_directory ", Style::default().fg(theme.accent_yellow)),
                Span::styled(path.to_string(), Style::default().fg(theme.accent_cyan)),
            ]
        }
        "read_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("  [FILE] read_file ", Style::default().fg(theme.accent_yellow)),
                Span::styled(path.to_string(), Style::default().fg(theme.accent_cyan)),
            ]
        }
        "grep_search" | "grep" => {
            let query = args_val.as_ref()
                .and_then(|v| v.get("query").or_else(|| v.get("pattern")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("  [GREP] grep ", Style::default().fg(theme.accent_yellow)),
                Span::styled(format!("\"{}\"", query), Style::default().fg(theme.accent_cyan)),
            ]
        }
        "glob_find" | "glob" => {
            let pat = args_val.as_ref()
                .and_then(|v| v.get("pattern").and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("  [GLOB] glob ", Style::default().fg(theme.accent_yellow)),
                Span::styled(pat.to_string(), Style::default().fg(theme.accent_cyan)),
            ]
        }
        "apply_patch" | "patch" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("  [PATCH] apply_patch ", Style::default().fg(theme.accent_yellow)),
                Span::styled(path.to_string(), Style::default().fg(theme.accent_cyan)),
            ]
        }
        "edit" | "replace" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("  [EDIT] edit ", Style::default().fg(theme.accent_yellow)),
                Span::styled(path.to_string(), Style::default().fg(theme.accent_cyan)),
            ]
        }
        "write_file" => {
            let path = args_val.as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")).and_then(|p| p.as_str()))
                .unwrap_or(raw_args);
            vec![
                Span::styled("  [WRITE] write_file ", Style::default().fg(theme.accent_yellow)),
                Span::styled(path.to_string(), Style::default().fg(theme.accent_cyan)),
            ]
        }
        _ => {
            let display_args = corex_core::truncate_ellipsis(raw_args, 60);
            vec![
                Span::styled("  [TOOL] ", Style::default().fg(theme.accent_yellow)),
                Span::styled(name.to_string(), Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({})", display_args), Style::default().fg(theme.dark_gray)),
            ]
        }
    }
}

fn format_input_spans<'a>(text: &'a str, theme: &'a Theme) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(start_idx) = rest.find("[Pasted text #") {
        if start_idx > 0 {
            spans.push(Span::raw(&rest[..start_idx]));
        }
        let after_start = &rest[start_idx..];
        if let Some(end_bracket) = after_start.find(']') {
            let tag = &after_start[..=end_bracket];
            spans.push(Span::styled(
                tag,
                Style::default()
                    .fg(theme.accent_cyan)
                    .bg(Color::Rgb(25, 45, 65))
                    .add_modifier(Modifier::BOLD),
            ));
            rest = &after_start[end_bracket + 1..];
        } else {
            spans.push(Span::raw(after_start));
            rest = "";
            break;
        }
    }
    if !rest.is_empty() {
        spans.push(Span::raw(rest));
    }
    spans
}

fn format_input_with_cursor<'a>(
    text: &'a str,
    cursor_idx: usize,
    theme: &'a Theme,
) -> (Vec<Span<'a>>, usize) {
    let mut clamped = cursor_idx.min(text.len());
    while !text.is_char_boundary(clamped) {
        clamped = clamped.saturating_sub(1);
    }
    let mut spans = Vec::new();

    if text.is_empty() {
        spans.push(Span::styled("█", Style::default().fg(theme.accent_blue)));
        return (spans, 0);
    }

    if clamped >= text.len() {
        spans.extend(format_input_spans(text, theme));
        let cursor_col: usize = spans.iter().map(|s| s.width()).sum();
        spans.push(Span::styled("█", Style::default().fg(theme.accent_blue)));
        return (spans, cursor_col);
    }

    let before = &text[..clamped];
    let mut before_spans = format_input_spans(before, theme);
    let cursor_col: usize = before_spans.iter().map(|s| s.width()).sum();

    if text[clamped..].starts_with("[Pasted text #") {
        if let Some(end_bracket) = text[clamped..].find(']') {
            let tag = &text[clamped..=clamped + end_bracket];
            let after = &text[clamped + end_bracket + 1..];
            spans.append(&mut before_spans);
            spans.push(Span::styled(
                tag,
                Style::default()
                    .fg(Color::Rgb(20, 20, 20))
                    .bg(theme.accent_blue)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.extend(format_input_spans(after, theme));
            return (spans, cursor_col);
        }
    }

    let c = text[clamped..].chars().next().unwrap_or(' ');
    let c_len = c.len_utf8();
    let char_str = &text[clamped..clamped + c_len];
    let after = &text[clamped + c_len..];

    spans.append(&mut before_spans);
    spans.push(Span::styled(
        char_str,
        Style::default()
            .fg(Color::Rgb(20, 20, 20))
            .bg(theme.accent_blue)
            .add_modifier(Modifier::BOLD),
    ));
    spans.extend(format_input_spans(after, theme));

    (spans, cursor_col)
}

fn render_about_card(
    version: &str,
    model: &str,
    base_url: &str,
    local_engine: &str,
    local_enabled: bool,
    session_id: &str,
    workspace: &str,
    branch: &str,
    theme: &Theme,
    content_max_width: usize,
) -> Vec<Line<'static>> {
    // Total inner width between left border and right border
    let inner_width = (content_max_width.saturating_sub(6)).clamp(44, 72);
    let label_w = 23;
    let usable_w = inner_width.saturating_sub(4); // 3 spaces left, 1 space right

    let make_row = |label: &str, val_spans: Vec<Span<'static>>| -> Line<'static> {
        let mut line_spans = vec![
            Span::styled("  │   ", Style::default().fg(theme.dark_gray)),
            Span::styled(format!("{:<width$}", label, width = label_w), Style::default().fg(Color::Rgb(160, 172, 190)).add_modifier(Modifier::BOLD)),
        ];
        let val_used: usize = val_spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
        line_spans.extend(val_spans);
        let rem = usable_w.saturating_sub(label_w + val_used);
        if rem > 0 {
            line_spans.push(Span::raw(" ".repeat(rem)));
        }
        line_spans.push(Span::styled(" │", Style::default().fg(theme.dark_gray)));
        Line::from(line_spans)
    };

    let empty_row = || -> Line<'static> {
        Line::from(vec![
            Span::styled("  │", Style::default().fg(theme.dark_gray)),
            Span::raw(" ".repeat(inner_width)),
            Span::styled("│", Style::default().fg(theme.dark_gray)),
        ])
    };

    // Top line: ╭─ Corex v0.2.0 ───────╮
    let title = format!(" Corex v{} ", version);
    let title_w = unicode_width::UnicodeWidthStr::width(title.as_str());
    let top_fill = inner_width.saturating_sub(1 + title_w);
    let top_line = Line::from(vec![
        Span::styled("  ╭─", Style::default().fg(theme.dark_gray)),
        Span::styled(title, Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}╮", "─".repeat(top_fill)), Style::default().fg(theme.dark_gray)),
    ]);

    // Section divider: ├─ Session Telemetry ──────┤
    let div_title = "─ Session Telemetry ";
    let div_w = unicode_width::UnicodeWidthStr::width(div_title);
    let div_fill = inner_width.saturating_sub(div_w);
    let div_line = Line::from(vec![
        Span::styled("  ├", Style::default().fg(theme.dark_gray)),
        Span::styled(div_title, Style::default().fg(Color::Rgb(95, 115, 140)).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}┤", "─".repeat(div_fill)), Style::default().fg(theme.dark_gray)),
    ]);

    // Bottom line: ╰────────────────────────╯
    let bot_line = Line::from(Span::styled(
        format!("  ╰{}╯", "─".repeat(inner_width)),
        Style::default().fg(theme.dark_gray),
    ));

    let local_status = if local_enabled { "(Active)" } else { "(Disabled)" };
    let local_color = if local_enabled { Color::Rgb(115, 185, 140) } else { Color::Rgb(110, 120, 135) };

    let val_max = usable_w.saturating_sub(label_w + 1);

    vec![
        top_line,
        empty_row(),
        make_row("Creator & Maintainer", vec![
            Span::styled("sluisr", Style::default().fg(Color::Rgb(225, 230, 242)).add_modifier(Modifier::BOLD)),
            Span::styled(" (https://sluisr.com)", Style::default().fg(Color::Rgb(115, 160, 220))),
        ]),
        make_row("Official Website", vec![
            Span::styled(corex_core::truncate_ellipsis("https://corex.sluisr.com", val_max), Style::default().fg(Color::Rgb(105, 185, 235))),
        ]),
        make_row("Changelog & Releases", vec![
            Span::styled(corex_core::truncate_ellipsis("https://corex.sluisr.com/changelog", val_max), Style::default().fg(Color::Rgb(105, 185, 235))),
        ]),
        make_row("Report Issues & Bugs", vec![
            Span::styled(corex_core::truncate_ellipsis("https://github.com/sluisr/corex/issues", val_max), Style::default().fg(Color::Rgb(105, 185, 235))),
        ]),
        make_row("GitHub Repository", vec![
            Span::styled(corex_core::truncate_ellipsis("https://github.com/sluisr/corex", val_max), Style::default().fg(Color::Rgb(105, 185, 235))),
        ]),
        empty_row(),
        div_line,
        empty_row(),
        make_row("Active Model", vec![
            Span::styled(corex_core::truncate_ellipsis(model, val_max), Style::default().fg(theme.accent_purple).add_modifier(Modifier::BOLD)),
        ]),
        make_row("API Base URL", vec![
            Span::styled(corex_core::truncate_ellipsis(base_url, val_max), Style::default().fg(Color::Rgb(140, 150, 170))),
        ]),
        make_row("Local SLM Engine", vec![
            Span::styled(format!("{} ", corex_core::truncate_ellipsis(local_engine, val_max.saturating_sub(12))), Style::default().fg(Color::Rgb(140, 150, 170))),
            Span::styled(local_status, Style::default().fg(local_color)),
        ]),
        make_row("Session ID", vec![
            Span::styled(corex_core::truncate_ellipsis(session_id, val_max), Style::default().fg(Color::Rgb(125, 135, 150))),
        ]),
        make_row("Workspace", vec![
            Span::styled(corex_core::truncate_ellipsis(workspace, val_max), Style::default().fg(Color::Rgb(150, 180, 135))),
        ]),
        make_row("Git Branch", vec![
            Span::styled(corex_core::truncate_ellipsis(branch, val_max), Style::default().fg(Color::Rgb(210, 165, 105))),
        ]),
        empty_row(),
        bot_line,
    ]
}

fn format_system_message(text: &str, theme: &Theme, content_max_width: usize, logs_expanded: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let raw_lines: Vec<&str> = text.lines().collect();

    if raw_lines.is_empty() {
        return lines;
    }

    let first_line = raw_lines[0].trim();

    // 0. Dedicated Corex Info Card
    if first_line.starts_with("COREX_INFO_CARD|") || first_line.starts_with("UTI_INFO_CARD|") {
        let parts: Vec<&str> = first_line.split('|').collect();
        if parts.len() >= 9 {
            return render_about_card(
                parts[1],
                parts[2],
                parts[3],
                parts[4],
                parts[5] == "true",
                parts[6],
                parts[7],
                parts[8],
                theme,
                content_max_width,
            );
        }
    }

    // Soft, muted color palette for discreet visual presence (opaco / disimulado)
    let muted_dim = Color::Rgb(70, 82, 98);
    let muted_label = Color::Rgb(105, 118, 135);
    let muted_text = Color::Rgb(155, 168, 185);
    let muted_tag = Color::Rgb(95, 115, 138);

    // 1. Task Finished / Background Task Done (One-Liner Badge + tail or full log)
    if first_line.starts_with("TASK_DONE|") || first_line.starts_with("[TASK FINISHED]") {
        let (pid, exit_code_str, duration) = if first_line.starts_with("TASK_DONE|") {
            let parts: Vec<&str> = first_line.split('|').collect();
            let pid = parts.get(1).copied().unwrap_or("?");
            let code = parts.get(2).copied().unwrap_or("0");
            let dur = parts.get(3).copied().unwrap_or("");
            (pid, code, dur)
        } else {
            ("task", "0", "")
        };

        let is_ok = exit_code_str == "0";
        let is_stopped = exit_code_str == "stopped";

        let (icon, icon_color, status_text, status_color) = if is_stopped {
            ("  ■ ", Color::Rgb(215, 175, 100), "stopped".to_string(), Color::Rgb(215, 175, 100))
        } else if is_ok {
            ("  ✓ ", Color::Rgb(105, 185, 135), "finished (exit 0)".to_string(), Color::Rgb(125, 160, 145))
        } else {
            let label = format!("finished (exit {})", exit_code_str);
            ("  ✕ ", Color::Rgb(220, 100, 100), label, Color::Rgb(220, 110, 110))
        };

        let mut badge_spans = vec![
            Span::styled(icon, Style::default().fg(icon_color)),
            Span::styled(format!("[bg:{}] ", pid), Style::default().fg(muted_tag)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ];
        if !duration.trim().is_empty() {
            badge_spans.push(Span::styled(format!(" ({})", duration.trim()), Style::default().fg(muted_dim)));
        }
        lines.push(Line::from(badge_spans));

        // Output lines
        let output_lines: Vec<&str> = raw_lines[1..]
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();

        if !output_lines.is_empty() {
            let total = output_lines.len();
            let max_lines = 5;
            if logs_expanded {
                for line_str in &output_lines {
                    let clean = line_str.replace('\t', "    ");
                    let spans = vec![
                        Span::styled(clean, Style::default().fg(muted_text)),
                    ];
                    lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    │ "));
                }
            } else {
                if total > max_lines {
                    let omitted = total - max_lines;
                    let om_spans = vec![
                        Span::styled("    │ ", Style::default().fg(muted_dim)),
                        Span::styled(format!("… (+{} lines above, Ctrl+O to expand)", omitted), Style::default().fg(muted_dim)),
                    ];
                    lines.push(Line::from(om_spans));
                }

                let start_idx = total.saturating_sub(max_lines);
                for line_str in &output_lines[start_idx..] {
                    let clean = line_str.replace('\t', "    ");
                    let spans = vec![
                        Span::styled(clean, Style::default().fg(muted_text)),
                    ];
                    lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    │ "));
                }
            }
        }

        return lines;
    }

    // 2. Model Activation Message
    if first_line.starts_with("Activated ") || first_line.starts_with("Switched ") {
        let title = first_line.trim_start_matches("Activated ").trim_start_matches("Switched ");
        let header_spans = vec![
            Span::styled("  · ", Style::default().fg(muted_dim)),
            Span::styled("[model] ", Style::default().fg(muted_tag)),
            Span::styled(title.to_string(), Style::default().fg(muted_text)),
        ];
        lines.push(Line::from(header_spans));

        for sub_line in &raw_lines[1..] {
            let trimmed = sub_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(param) = trimmed.strip_prefix("- ") {
                let mut spans = vec![
                    Span::styled("    · ", Style::default().fg(muted_dim)),
                ];
                if let Some((k, v)) = param.split_once(':') {
                    spans.push(Span::styled(format!("{}: ", k.trim()), Style::default().fg(muted_label)));
                    let val = v.trim();
                    let val_color = match val.to_lowercase().as_str() {
                        "dynamic" | "low" | "enabled" => Color::Rgb(115, 150, 135),
                        "medium" => Color::Rgb(165, 150, 120),
                        "high" | "max" => Color::Rgb(150, 130, 165),
                        _ => Color::Rgb(140, 155, 175),
                    };
                    spans.push(Span::styled(val.to_string(), Style::default().fg(val_color)));
                } else {
                    spans.push(Span::styled(param.to_string(), Style::default().fg(muted_text)));
                }
                lines.push(Line::from(spans));
            } else {
                let spans = vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(trimmed.to_string(), Style::default().fg(muted_label)),
                ];
                lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    "));
            }
        }
        return lines;
    }

    // 2. Error / Failure Message
    if first_line.starts_with("Error") || first_line.starts_with("Failed") {
        let err_spans = vec![
            Span::styled("  ✕ ", Style::default().fg(Color::Rgb(190, 95, 95))),
            Span::styled("[error] ", Style::default().fg(Color::Rgb(190, 95, 95))),
            Span::styled(first_line.to_string(), Style::default().fg(muted_text)),
        ];
        lines.extend(crate::markdown::wrap_spans(err_spans, content_max_width, "    "));
        for sub_line in &raw_lines[1..] {
            let spans = vec![
                Span::styled("    ", Style::default()),
                Span::styled(sub_line.to_string(), Style::default().fg(muted_label)),
            ];
            lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "    "));
        }
        return lines;
    }

    // 3. Checkpoint / Saved Message
    if first_line.contains("checkpoint saved") || first_line.contains("saved with tag") {
        let save_spans = vec![
            Span::styled("  ✓ ", Style::default().fg(Color::Rgb(105, 145, 125))),
            Span::styled("[checkpoint] ", Style::default().fg(Color::Rgb(105, 145, 125))),
            Span::styled(first_line.to_string(), Style::default().fg(muted_text)),
        ];
        lines.extend(crate::markdown::wrap_spans(save_spans, content_max_width, "    "));
        return lines;
    }

    // 4. Session Event
    if first_line.starts_with("Resumed session") || first_line.starts_with("Started new chat") {
        let session_spans = vec![
            Span::styled("  · ", Style::default().fg(muted_dim)),
            Span::styled("[session] ", Style::default().fg(muted_tag)),
            Span::styled(first_line.to_string(), Style::default().fg(muted_text)),
        ];
        lines.extend(crate::markdown::wrap_spans(session_spans, content_max_width, "    "));
        return lines;
    }

    // 5. Rich Markdown Formatted System Messages (e.g. headers, rules, code blocks)
    if text.contains('#') || text.contains("```") || text.contains("---") {
        return crate::markdown::render_markdown(text, theme, content_max_width);
    }

    // 6. Default General Multi-line or Single-line System Message
    let mut is_first = true;
    for line in raw_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let prefix = if is_first { "  · " } else { "    " };
        let sys_spans = vec![
            Span::styled(prefix, Style::default().fg(muted_dim)),
            Span::styled(trimmed.to_string(), Style::default().fg(muted_label)),
        ];
        lines.extend(crate::markdown::wrap_spans(sys_spans, content_max_width, "    "));
        is_first = false;
    }

    lines
}

fn render_single_message_with_pending(
    _msg_idx: usize,
    msg: &Message,
    next_msg: Option<&Message>,
    theme: &Theme,
    content_max_width: usize,
    pending_conf: Option<&PendingToolBatch>,
    logs_expanded: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match msg.role.as_str() {
        "user" => {
            let content = msg.text_content().unwrap_or("");
            let user_spans = vec![
                Span::styled("❯ ", Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)),
                Span::styled(content.to_string(), Style::default().add_modifier(Modifier::BOLD)),
            ];
            let mut wrapped = crate::markdown::wrap_spans(user_spans, content_max_width, "  ");
            wrapped.push(Line::from(""));
            lines.extend(wrapped);
        }
        "assistant" => {
            if let Some(text) = msg.text_content() {
                let mut md_lines = render_markdown(text, theme, content_max_width);
                md_lines.push(Line::from(""));
                lines.extend(md_lines);
            }

            if let Some(ref calls) = msg.tool_calls {
                for call in calls {
                    if let Some(pending) = pending_conf {
                        if pending.calls.iter().any(|c| c.id == call.id) {
                            continue;
                        }
                    }

                    let spans = format_tool_call_spans(&call.function.name, &call.function.arguments, theme);
                    let wrapped = crate::markdown::wrap_spans(spans, content_max_width, "  ");
                    lines.extend(wrapped);
                }
            }
        }
        "tool" => {
            let content = msg.text_content().unwrap_or("");
            let is_last_tool = next_msg.map(|m| m.role.as_str() != "tool").unwrap_or(true);

            let check_slice = corex_core::safe_truncate_str(content, 2048);
            if check_slice.contains("denied by user") || check_slice.contains("declined") {
                let line_spans = vec![
                    Span::styled("    ✕ ", Style::default().fg(Color::Red)),
                    Span::styled("Execution declined by user", Style::default().fg(theme.gray)),
                ];
                let wrapped = crate::markdown::wrap_spans(line_spans, content_max_width, "    ");
                lines.extend(wrapped);
            } else if content.starts_with("Successfully edited") && content.contains('\n') {
                let mut content_lines = content.lines();
                let header = content_lines.next().unwrap_or("Successfully edited");
                let header_spans = vec![
                    Span::styled("    ✓ ", Style::default().fg(Color::Green)),
                    Span::styled(header.to_string(), Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
                ];
                let wrapped_header = crate::markdown::wrap_spans(header_spans, content_max_width, "    ");
                lines.extend(wrapped_header);

                for prev_line in content_lines {
                    if prev_line.trim().is_empty() {
                        continue;
                    }
                    if let Some(body) = prev_line.strip_prefix('>') {
                        let mut spans = vec![
                            Span::styled("      > ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                        ];
                        if let Some((num, code)) = body.split_once('|') {
                            spans.push(Span::styled(num.to_string(), Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)));
                            spans.push(Span::styled("│", Style::default().fg(theme.dark_gray)));
                            spans.push(Span::styled(code.to_string(), Style::default().fg(theme.diff_added_fg).add_modifier(Modifier::BOLD)));
                        } else {
                            spans.push(Span::styled(body.to_string(), Style::default().fg(theme.diff_added_fg).add_modifier(Modifier::BOLD)));
                        }
                        lines.push(Line::from(spans));
                    } else {
                        let clean_line = prev_line.strip_prefix(' ').unwrap_or(prev_line);
                        let mut spans = vec![
                            Span::styled("        ", Style::default()),
                        ];
                        if let Some((num, code)) = clean_line.split_once('|') {
                            spans.push(Span::styled(num.to_string(), Style::default().fg(theme.dark_gray)));
                            spans.push(Span::styled("│", Style::default().fg(theme.dark_gray)));
                            spans.push(Span::styled(code.to_string(), Style::default().fg(theme.gray)));
                        } else {
                            spans.push(Span::styled(clean_line.to_string(), Style::default().fg(theme.gray)));
                        }
                        lines.push(Line::from(spans));
                    }
                }
            } else if content.starts_with("[COMMAND SENT TO BACKGROUND]") || content.starts_with("[BACKGROUND TASK LAUNCHED]") {
                let pid_str = content.lines().find(|l| l.contains("Task ID (PID):")).and_then(|l| l.split(':').nth(1)).map(|p| p.trim()).unwrap_or("?");
                let line_spans = vec![
                    Span::styled("    · ", Style::default().fg(theme.accent_cyan)),
                    Span::styled(format!("[bg:{}] ", pid_str), Style::default().fg(theme.accent_cyan)),
                    Span::styled("running in background...", Style::default().fg(theme.gray)),
                ];
                let wrapped = crate::markdown::wrap_spans(line_spans, content_max_width, "    ");
                lines.extend(wrapped);
            } else {
                let raw_output_lines: Vec<&str> = content
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .collect();

                let is_err = content.starts_with("Error")
                    || content.starts_with("Failed")
                    || content.starts_with("Command exited with status code");

                let icon = if is_err { "    ✕ " } else { "    ✓ " };
                let icon_color = if is_err { Color::Red } else { Color::Green };

                if raw_output_lines.len() <= 1 {
                    let first_line_raw = raw_output_lines.first().copied().unwrap_or("Done").replace('\t', "    ");
                    let display_first_line = if first_line_raw.len() > 140 {
                        let mut end = 137;
                        while end > 0 && !first_line_raw.is_char_boundary(end) {
                            end -= 1;
                        }
                        format!("{}...", &first_line_raw[..end])
                    } else {
                        first_line_raw
                    };

                    let line_spans = vec![
                        Span::styled(icon, Style::default().fg(icon_color)),
                        Span::styled(display_first_line, Style::default().fg(theme.gray)),
                    ];
                    let wrapped = crate::markdown::wrap_spans(line_spans, content_max_width, "    ");
                    lines.extend(wrapped);
                } else {
                    let total = raw_output_lines.len();
                    let header_label = if is_err {
                        format!("Command failed ({} lines)", total)
                    } else {
                        format!("Output ({} lines)", total)
                    };

                    let mut header_spans = vec![
                        Span::styled(icon, Style::default().fg(icon_color)),
                        Span::styled(header_label, Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
                    ];
                    if !logs_expanded {
                        header_spans.push(Span::styled(" (Ctrl+O to expand)", Style::default().fg(theme.dark_gray)));
                    }
                    lines.push(Line::from(header_spans));

                    let max_lines = 5;
                    if logs_expanded {
                        for line_str in &raw_output_lines {
                            let clean = line_str.replace('\t', "    ");
                            let spans = vec![
                                Span::styled(clean, Style::default().fg(theme.gray)),
                            ];
                            lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "        │ "));
                        }
                    } else {
                        if total > max_lines {
                            let omitted = total - max_lines;
                            let om_spans = vec![
                                Span::styled("        │ ", Style::default().fg(theme.dark_gray)),
                                Span::styled(format!("… (+{} lines above)", omitted), Style::default().fg(theme.dark_gray)),
                            ];
                            lines.push(Line::from(om_spans));
                        }

                        let start_idx = total.saturating_sub(max_lines);
                        for line_str in &raw_output_lines[start_idx..] {
                            let clean = line_str.replace('\t', "    ");
                            let spans = vec![
                                Span::styled(clean, Style::default().fg(theme.gray)),
                            ];
                            lines.extend(crate::markdown::wrap_spans(spans, content_max_width, "        │ "));
                        }
                    }
                }
            }

            if is_last_tool {
                lines.push(Line::from(""));
            }
        }
        "system" => {
            if let Some(text) = msg.text_content() {
                lines.extend(format_system_message(text, theme, content_max_width, logs_expanded));
                let is_task_done = text.starts_with("TASK_DONE|") || text.starts_with("[TASK FINISHED]");
                let next_is_task_done = next_msg
                    .and_then(|m| if m.role.as_str() == "system" { m.text_content() } else { None })
                    .map(|t| t.starts_with("TASK_DONE|") || t.starts_with("[TASK FINISHED]"))
                    .unwrap_or(false);

                if !is_task_done || !next_is_task_done {
                    lines.push(Line::from(""));
                }
            }
        }
        _ => {}
    }
    lines
}

fn render_ui(frame: &mut Frame, app: &mut App) {
    let size = frame.area();

    if app.input_buffer.starts_with('/') {
        app.last_slash_filter = app.input_buffer.clone();
    }

    let target_height = app.slash_popup_target_height();
    let speed = 0.35;
    let diff = target_height - app.slash_popup_height_current;
    if diff.abs() > 0.05 {
        app.slash_popup_height_current += diff * speed;
    } else {
        app.slash_popup_height_current = target_height;
    }

    // Bordered popup needs at least 3 lines (top border, 1 item, bottom border).
    // When closing, any height < 2.5 snaps to 0 so the single top-border line is never rendered.
    let popup_height = if app.slash_popup_height_current >= 2.5 {
        app.slash_popup_height_current.round() as u16
    } else {
        0
    };

    let show_activity = (app.is_streaming || !app.status_transition.is_empty())
        && app.pending_confirmation.is_none()
        && !app.user_dialog.is_open
        && !app.sudo_dialog.is_open;
    let activity_height = if show_activity { 1 } else { 0 };

    let chunks = if popup_height > 0 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),               // Chat Feed
                Constraint::Length(popup_height), // Autocomplete Popup Slot
                Constraint::Length(activity_height), // Fixed Activity / Status Bar
                Constraint::Length(3),            // Input Composer
                Constraint::Length(1),            // Status Footer Bar
            ])
            .split(size)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),               // Chat Feed
                Constraint::Length(activity_height), // Fixed Activity / Status Bar
                Constraint::Length(3),            // Input Composer
                Constraint::Length(1),            // Status Footer Bar
            ])
            .split(size)
    };

    let chat_chunk = chunks[0];
    let (popup_chunk, activity_chunk, composer_chunk, status_chunk) = if popup_height > 0 {
        (Some(chunks[1]), chunks[2], chunks[3], chunks[4])
    } else {
        (None, chunks[1], chunks[2], chunks[3])
    };

    let content_max_width = (chat_chunk.width as usize).saturating_sub(4).max(20);
    let mut all_lines = Vec::new();

    // 1. Header Banner (First item in scrollable feed!)
    let cfg = app.llm_client.get_config();
    let is_auth = !cfg.api_key.trim().is_empty();
    let is_local = cfg.local_llm_enabled;
    let update_notice = app.update_available.lock().ok().and_then(|l| l.clone());
    let header_lines = render_gradient_logo(
        env!("CARGO_PKG_VERSION"),
        is_auth,
        is_local,
        update_notice.as_deref(),
    );
    all_lines.extend(header_lines);

    // 2. Cached Messages & Tool Executions List
    if app.cached_render_width != content_max_width
        || app.cached_session_id != app.session.id
        || app.cached_logs_expanded != app.logs_expanded
    {
        app.cached_message_lines.clear();
        app.cached_message_count = 0;
        app.cached_render_width = content_max_width;
        app.cached_session_id = app.session.id.clone();
        app.cached_logs_expanded = app.logs_expanded;
    }

    let target_cache_count = if app.pending_confirmation.is_some() {
        app.session.messages.len().saturating_sub(1)
    } else {
        app.session.messages.len()
    };

    if app.cached_message_count > target_cache_count {
        app.cached_message_lines.clear();
        app.cached_message_count = 0;
    }

    if app.cached_message_count != target_cache_count {
        app.cached_message_lines.clear();
        for msg_idx in 0..target_cache_count {
            let msg = &app.session.messages[msg_idx];
            let next_msg = app.session.messages[..target_cache_count].get(msg_idx + 1);
            let rendered = render_single_message_with_pending(
                msg_idx,
                msg,
                next_msg,
                &app.theme,
                content_max_width,
                None,
                app.logs_expanded,
            );
            app.cached_message_lines.extend(rendered);
        }
        app.cached_message_count = target_cache_count;
    }

    all_lines.extend(app.cached_message_lines.iter().cloned());

    if target_cache_count < app.session.messages.len() {
        let msg_idx = target_cache_count;
        let msg = &app.session.messages[msg_idx];
        let next_msg = app.session.messages.get(msg_idx + 1);
        let rendered = render_single_message_with_pending(
            msg_idx,
            msg,
            next_msg,
            &app.theme,
            content_max_width,
            app.pending_confirmation.as_ref(),
            app.logs_expanded,
        );
        all_lines.extend(rendered);
    }

    // Render Tool Confirmation Dialog inline directly in the chat stream!
    if let Some(ref pending) = app.pending_confirmation {
        let diff_str = pending.diff_preview.as_deref().unwrap_or("");
        let tool_name = pending.calls.first().map(|c| c.function.name.as_str()).unwrap_or("tool");
        let conf_lines = build_tool_confirmation_lines(
            tool_name,
            diff_str,
            pending.selected_option,
            pending.calls.len(),
            content_max_width,
            &app.theme,
            pending.diff_expanded,
        );
        all_lines.extend(conf_lines);
    }

    if app.is_streaming {
        let elapsed = app.last_turn_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);

        if !app.streaming_text.is_empty() {
            let mut stream_lines = render_markdown(&app.streaming_text, &app.theme, content_max_width);
            stream_lines.push(Line::from(""));
            all_lines.extend(stream_lines);
        }

        if !app.streaming_tool_calls.is_empty() {
            let preview_lines = build_streaming_tool_preview_lines(
                &app.streaming_tool_calls,
                elapsed,
                content_max_width,
                &app.theme,
            );
            all_lines.extend(preview_lines);
        }
    }

    if !app.message_queue.is_empty() {
        all_lines.push(Line::from(""));
        for (i, queued_msg) in app.message_queue.iter().enumerate() {
            let label = if app.message_queue.len() == 1 {
                "[queued] ".to_string()
            } else {
                format!("[queued #{}] ", i + 1)
            };
            all_lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(label, Style::default().fg(app.theme.accent_yellow).add_modifier(Modifier::BOLD)),
                Span::styled(queued_msg.clone(), Style::default().fg(app.theme.foreground)),
                Span::styled(" (pending)", Style::default().fg(app.theme.dark_gray)),
            ]));
        }
        all_lines.push(Line::from(""));
    }

    let total_lines = all_lines.len();
    app.total_rendered_items = total_lines;

    let visible_height = (chat_chunk.height as usize).max(1);
    let max_scroll = total_lines.saturating_sub(visible_height) as u16;

    if app.auto_scroll {
        app.scroll_offset = max_scroll;
    } else {
        app.scroll_offset = app.scroll_offset.min(max_scroll);
    }

    app.last_chat_rect = Some(chat_chunk);
    app.last_rendered_text_lines = all_lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect();

    let message_paragraph = Paragraph::new(all_lines)
        .block(Block::default().borders(Borders::NONE))
        .scroll((app.scroll_offset, 0));
    frame.render_widget(message_paragraph, chat_chunk);

    // 2.5. Fixed Activity / Status Bar (Directly above the Composer!)
    if show_activity {
        let spinner_frames = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
        let elapsed = app.last_turn_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
        let idx = ((elapsed * 10.0) as usize) % spinner_frames.len();
        let spinner_char = spinner_frames[idx];

        if app.status_transition.is_empty() {
            if app.thinking_state.is_streaming {
                app.set_status("Thinking...");
            } else if !app.streaming_text.is_empty() {
                app.set_status("Generating response...");
            } else {
                app.set_status("Generating...");
            }
        }

        let text_spans = app.status_transition.render_spans(&app.theme);

        let mut left_spans = vec![
            Span::raw(" "),
            Span::styled(spinner_char, Style::default().fg(app.theme.accent_purple)),
            Span::raw("  "),
        ];
        left_spans.extend(text_spans);

        let queue_count = app.message_queue.len();
        if queue_count > 0 {
            left_spans.push(Span::styled(
                format!(" · [{} queued]", queue_count),
                Style::default().fg(app.theme.accent_yellow).add_modifier(Modifier::BOLD),
            ));
        }

        let right_span = if queue_count > 0 {
            Span::styled("(Esc para descolar) ", Style::default().fg(app.theme.dark_gray))
        } else {
            Span::styled("(Esc para cancelar) ", Style::default().fg(app.theme.dark_gray))
        };

        let left_len: usize = left_spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
        let right_len = unicode_width::UnicodeWidthStr::width(right_span.content.as_ref());
        let bar_width = activity_chunk.width as usize;

        let mut spans = left_spans;
        if bar_width > left_len + right_len {
            let pad = bar_width - (left_len + right_len);
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(right_span);
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), activity_chunk);
    }

    // 3. Input Prompt Composer
    let prompt_prefix = if app.always_allow_tools {
        Span::styled("* ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else if app.plan_mode {
        Span::styled("? ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("> ", Style::default().fg(app.theme.accent_blue).add_modifier(Modifier::BOLD))
    };
    let prefix_width = prompt_prefix.width();

    let show_esc_hint = app.last_esc_press
        .map(|i| i.elapsed() <= Duration::from_millis(500))
        .unwrap_or(false);

    let (prompt_content, cursor_col) = if app.pending_confirmation.is_some() {
        (
            Line::from(vec![
                prompt_prefix,
                Span::styled("(Press 1-3 or Enter to select, Esc to decline)", Style::default().fg(app.theme.dark_gray)),
            ]),
            0,
        )
    } else if show_esc_hint {
        let msg = if app.input_buffer.is_empty() {
            "Press Esc again to rewind."
        } else {
            "Press Esc again to clear prompt."
        };
        let (mut spans, col) = format_input_with_cursor(&app.input_buffer, app.cursor_idx, &app.theme);
        spans.insert(0, prompt_prefix);
        spans.push(Span::styled(format!(" ({})", msg), Style::default().fg(app.theme.gray)));
        (Line::from(spans), prefix_width + col)
    } else if app.input_buffer.is_empty() {
        (
            Line::from(vec![
                prompt_prefix,
                Span::styled("█ ", Style::default().fg(app.theme.accent_blue)),
                Span::styled("Type your message or @path/to/file", Style::default().fg(app.theme.dark_gray)),
            ]),
            prefix_width,
        )
    } else {
        let (mut spans, col) = format_input_with_cursor(&app.input_buffer, app.cursor_idx, &app.theme);
        spans.insert(0, prompt_prefix);
        (Line::from(spans), prefix_width + col)
    };

    let composer_block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(app.theme.dark_gray));

    let visible = (composer_chunk.width as usize).saturating_sub(1).max(1);
    let margin = if visible > 10 { 5 } else { 0 };
    let scroll_x = if cursor_col < visible {
        0
    } else {
        (cursor_col + margin).saturating_sub(visible) as u16
    };

    frame.render_widget(
        Paragraph::new(prompt_content)
            .block(composer_block)
            .scroll((0, scroll_x)),
        composer_chunk,
    );

    // Render slash command popup if active
    if let Some(p_chunk) = popup_chunk {
        let filter_text = if app.input_buffer.starts_with('/') {
            &app.input_buffer
        } else {
            &app.last_slash_filter
        };
        render_command_popup(frame, filter_text, app.slash_selected_idx, p_chunk, &app.theme);
    }

    // 4. Status Footer Bar (Left: Workspace Path | Right: Model Info)
    let cfg = app.llm_client.get_config();

    // Left spans: ONLY the workspace directory (and git branch)
    let mut left_spans = vec![
        Span::styled(format!(" {} ", app.shorten_path()), Style::default().fg(app.theme.foreground)),
    ];
    if !app.git_branch.is_empty() {
        left_spans.push(Span::styled(format!("({}) ", app.git_branch), Style::default().fg(app.theme.accent_cyan)));
    }
    if app.plan_mode {
        left_spans.push(Span::styled("[PLAN MODE] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    }

    let show_exit_warning = app.last_ctrl_c_press
        .map(|i| i.elapsed() <= std::time::Duration::from_secs(2))
        .unwrap_or(false);

    if show_exit_warning {
        left_spans.push(Span::styled("Press Ctrl+C again to exit. ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)));
    }

    // Right spans: Model info, temperature, reasoning
    let mut right_spans = Vec::new();
    let is_flash = cfg.model.contains("flash") || cfg.model == "deepseek-chat" || cfg.model == "DeepSeek-V4.1-Flash" || cfg.model == "DeepSeek-V4-Flash";
    let is_pro = cfg.model.contains("pro") || cfg.model == "deepseek-reasoner" || cfg.model == "DeepSeek-V4-Pro";

    if is_flash {
        let temp = cfg.temperature;
        let temp_color = if temp <= 0.2 {
            Color::Rgb(79, 195, 247)
        } else if temp <= 0.5 {
            Color::Rgb(105, 240, 174)
        } else if temp <= 1.0 {
            Color::Rgb(255, 213, 79)
        } else if temp <= 1.5 {
            Color::Rgb(255, 152, 0)
        } else {
            Color::Rgb(244, 67, 54)
        };

        let effort = if (cfg.local_llm_enabled || cfg.reasoning_effort == "dynamic") && !cfg.flash_settings.code_reasoning_effort.is_empty() {
            &cfg.flash_settings.code_reasoning_effort
        } else {
            &cfg.reasoning_effort
        };

        let r_color = match effort.as_str() {
            "none" => Color::Rgb(158, 158, 158),
            "low" => Color::Rgb(79, 195, 247),
            "high" => Color::Rgb(255, 213, 79),
            "xhigh" => Color::Rgb(255, 110, 64),
            "max" => Color::Rgb(224, 64, 251),
            _ => app.theme.accent_cyan,
        };

        right_spans.push(Span::styled("DeepSeek-V4.1-Flash", Style::default().fg(app.theme.accent_purple).add_modifier(Modifier::BOLD)));
        right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
        right_spans.push(Span::styled(format!("{:.1}", temp), Style::default().fg(temp_color)));
        right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
        right_spans.push(Span::styled(format!("{} ", effort), Style::default().fg(r_color)));
    } else if is_pro {
        let effort = &cfg.pro_settings.reasoning_effort;
        let r_color = match effort.as_str() {
            "low" => Color::Rgb(105, 240, 174),
            "medium" => Color::Rgb(255, 152, 0),
            "high" => Color::Rgb(244, 67, 54),
            "max" => Color::Rgb(224, 64, 251),
            _ => app.theme.accent_cyan,
        };

        right_spans.push(Span::styled("DeepSeek-V4-Pro", Style::default().fg(app.theme.accent_purple).add_modifier(Modifier::BOLD)));
        right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
        right_spans.push(Span::styled(format!("{} ", effort), Style::default().fg(r_color)));
    } else if cfg.model.starts_with("local") {
        right_spans.push(Span::styled(format!("Local: {} ", cfg.local_llm_model), Style::default().fg(Color::Rgb(105, 240, 174)).add_modifier(Modifier::BOLD)));
        right_spans.push(Span::styled("· Offline @ $0.00 ", Style::default().fg(app.theme.gray)));
    } else {
        right_spans.push(Span::styled(format!("{} ", cfg.model), Style::default().fg(app.theme.accent_purple).add_modifier(Modifier::BOLD)));
    }

    if cfg.local_llm_enabled && !cfg.model.starts_with("local") {
        right_spans.push(Span::styled("Hybrid ", Style::default().fg(app.theme.accent_cyan).add_modifier(Modifier::BOLD)));
    }

    // Center spans: Local LLM status indicator and Active Background Tasks badge
    let mut center_spans = Vec::new();
    if cfg.local_llm_enabled || cfg.model.starts_with("local") {
        let is_local_online = app.local_llm_online.load(Ordering::Relaxed);
        let dot_color = if is_local_online {
            Color::Rgb(105, 240, 174) // Green (#69f0ae)
        } else {
            Color::Rgb(244, 67, 54)   // Red (#f44336)
        };
        center_spans.push(Span::styled("● ", Style::default().fg(dot_color)));
        center_spans.push(Span::styled(
            "Local LLM",
            Style::default().fg(if is_local_online { app.theme.foreground } else { app.theme.gray })
        ));
    }

    let active_tasks = if !app.active_background_pids.is_empty() {
        app.active_background_pids.len()
    } else {
        corex_tools::background::get_task_manager()
            .lock()
            .map(|m| m.active_count())
            .unwrap_or(0)
    };

    if active_tasks > 0 {
        if !center_spans.is_empty() {
            center_spans.push(Span::styled("  ·  ", Style::default().fg(app.theme.gray)));
        }
        let task_badge = if active_tasks == 1 {
            "[1 Task Running]".to_string()
        } else {
            format!("[{} Tasks Running]", active_tasks)
        };
        center_spans.push(Span::styled(
            task_badge,
            Style::default()
                .fg(Color::Rgb(255, 213, 79))
                .add_modifier(Modifier::BOLD),
        ));
    }

    if app.logs_expanded {
        if !center_spans.is_empty() {
            center_spans.push(Span::styled("  ·  ", Style::default().fg(app.theme.gray)));
        }
        center_spans.push(Span::styled(
            "[Logs: Full (Ctrl+O)]",
            Style::default()
                .fg(app.theme.accent_cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let footer_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(status_chunk);

    frame.render_widget(Paragraph::new(Line::from(left_spans)).alignment(Alignment::Left), footer_cols[0]);
    frame.render_widget(Paragraph::new(Line::from(center_spans)).alignment(Alignment::Center), footer_cols[1]);
    frame.render_widget(Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right), footer_cols[2]);

    // 5. Sudo Authentication Dialog Modal if open
    if app.sudo_dialog.is_open {
        render_sudo_dialog(frame, size, &app.sudo_dialog, &app.theme);
    }

    // 6. Model Dialog Modal if open
    if app.model_dialog.is_open {
        render_model_dialog(frame, size, &app.model_dialog, &app.theme);
    }

    // 7. Auth Dialog Modal if open
    if app.auth_dialog.is_open {
        render_auth_dialog(frame, size, &app.auth_dialog, &app.theme);
    }

    // 8. Session Dialog Modal if open
    if app.session_dialog.is_open {
        render_session_dialog(frame, size, &app.session_dialog, &app.theme);
    }

    // 9. Interactive Ask-User Dialog Modal if open (rendered on top)
    if app.user_dialog.is_open {
        render_user_dialog(frame, size, &app.user_dialog, &app.theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_large_output_is_truncated_in_render() {
        let theme = Theme::default();
        // Create huge 100KB output on a single line
        let huge_line = "A".repeat(50_000);
        let msg = Message::tool_response("call_1".to_string(), huge_line);

        let lines = render_single_message_with_pending(0, &msg, None, &theme, 80, None, false);
        assert!(!lines.is_empty(), "expected at least one line rendered for tool");

        // Verify the entire output spans contain "✓" and "..." and was bounded to ~140 chars
        let all_text: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all_text.contains('✓'));
        assert!(all_text.contains("..."));
        assert!(all_text.len() < 300, "tool summary should be truncated: length was {}", all_text.len());
        assert!(lines.len() <= 3, "expected at most 3 wrapped lines, got {}", lines.len());
    }

    #[test]
    fn test_tool_multiline_output_expand() {
        let theme = Theme::default();
        let multiline_output = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8";
        let msg = Message::tool_response("call_2".to_string(), multiline_output.to_string());

        // 1. Compact mode (logs_expanded = false) -> header + omission line + last 5 lines
        let lines_compact = render_single_message_with_pending(0, &msg, None, &theme, 80, None, false);
        let all_compact: String = lines_compact.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all_compact.contains("Output (8 lines)"));
        assert!(all_compact.contains("… (+3 lines above)"));
        assert!(all_compact.contains("line 8"));

        // 2. Expanded mode (logs_expanded = true) -> header + all 8 lines
        let lines_expanded = render_single_message_with_pending(0, &msg, None, &theme, 80, None, true);
        let all_expanded: String = lines_expanded.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all_expanded.contains("Output (8 lines)"));
        assert!(!all_expanded.contains("lines above"));
        assert!(all_expanded.contains("line 1"));
        assert!(all_expanded.contains("line 8"));
    }

    #[tokio::test]
    async fn test_app_cache_invalidation() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);

        app.cached_message_count = 10;
        app.cached_render_width = 80;
        app.cached_session_id = "test_sess".to_string();
        app.cached_message_lines.push(Line::from("cached line"));

        app.invalidate_message_cache();

        assert_eq!(app.cached_message_count, 0);
        assert_eq!(app.cached_render_width, 0);
        assert!(app.cached_session_id.is_empty());
        assert!(app.cached_message_lines.is_empty());
    }

    #[test]
    fn test_format_input_spans_with_paste_tag() {
        let theme = Theme::default();
        let text = "Look at this: [Pasted text #1 +17 lines] and continue";
        let spans = format_input_spans(text, &theme);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content, "Look at this: ");
        assert_eq!(spans[1].content, "[Pasted text #1 +17 lines]");
        assert_eq!(spans[2].content, " and continue");
    }

    #[test]
    fn test_paste_expansion_logic() {
        let mut user_prompt_clean = "Explain this: [Pasted text #1 +2 lines] and fix it.".to_string();
        let mut pastes = std::collections::HashMap::new();
        pastes.insert(1, "error line 1\nerror line 2\nerror line 3".to_string());

        for (id, pasted_content) in &pastes {
            let prefix = format!("[Pasted text #{}", id);
            while let Some(start_idx) = user_prompt_clean.find(&prefix) {
                let rest = &user_prompt_clean[start_idx..];
                if let Some(end_bracket) = rest.find(']') {
                    user_prompt_clean.replace_range(start_idx..start_idx + end_bracket + 1, pasted_content);
                } else {
                    break;
                }
            }
        }

        assert_eq!(
            user_prompt_clean,
            "Explain this: error line 1\nerror line 2\nerror line 3 and fix it."
        );
    }

    #[test]
    fn test_format_input_with_cursor_empty() {
        let theme = Theme::default();
        let (spans, col) = format_input_with_cursor("", 0, &theme);
        assert_eq!(col, 0);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "█");
    }

    #[test]
    fn test_format_input_with_cursor_end() {
        let theme = Theme::default();
        let (spans, col) = format_input_with_cursor("cargo check", 11, &theme);
        assert_eq!(col, 11);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content, "cargo check");
        assert_eq!(spans[1].content, "█");
    }

    #[test]
    fn test_format_input_with_cursor_middle() {
        let theme = Theme::default();
        let (spans, col) = format_input_with_cursor("cargo check", 5, &theme);
        assert_eq!(col, 5);
        // Spans: "cargo" (before), " " (cursor highlighted), "check" (after)
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content, "cargo");
        assert_eq!(spans[1].content, " ");
        assert_eq!(spans[2].content, "check");
    }

    #[test]
    fn test_format_input_with_cursor_paste_tag() {
        let theme = Theme::default();
        let text = "run [Pasted text #1 +5 lines] now";
        let (spans, col) = format_input_with_cursor(text, 4, &theme);
        assert_eq!(col, 4);
        assert_eq!(spans[0].content, "run ");
        assert_eq!(spans[1].content, "[Pasted text #1 +5 lines]");
        assert_eq!(spans[2].content, " now");
    }

    #[tokio::test]
    async fn test_generation_id_increment() {
        let mut app = App::new(
            LlmClient::new(Config::default()),
            PathBuf::from("/tmp"),
            false,
        );
        assert_eq!(app.current_generation_id, 0);
        app.current_generation_id = app.current_generation_id.wrapping_add(1);
        assert_eq!(app.current_generation_id, 1);
    }

    #[tokio::test]
    async fn test_slash_popup_animation_targets() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.auth_dialog.close();

        // 1. Empty buffer -> target is 0.0, no animation
        assert_eq!(app.slash_popup_target_height(), 0.0);
        assert!(!app.is_slash_animating());

        // 2. Buffer starts with '/' -> target is > 0, animation starts
        app.input_buffer = "/".to_string();
        assert!(app.slash_popup_target_height() >= 3.0);
        assert!(app.is_slash_animating());

        // 3. Buffer has /model -> target is 3.0 (1 item + 2 borders)
        app.input_buffer = "/model".to_string();
        assert_eq!(app.slash_popup_target_height(), 3.0);

        // 4. Modal is open -> target must be 0.0 even if buffer has '/'
        app.model_dialog.is_open = true;
        assert_eq!(app.slash_popup_target_height(), 0.0);
    }

    #[test]
    fn test_format_system_message_model_activation() {
        let theme = Theme::default();
        let msg = "Activated DeepSeek-V4-Pro (Deep Reasoning Engine)\n- Reasoning Depth: max\n- Search CoT: low";
        let lines = format_system_message(msg, &theme, 80, false);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].spans.iter().any(|s| s.content.contains("[model]")));
        assert!(lines[1].spans.iter().any(|s| s.content.contains("Reasoning Depth:")));
        assert!(lines[2].spans.iter().any(|s| s.content.contains("Search CoT:")));
    }

    #[test]
    fn test_find_url_in_line() {
        assert_eq!(
            find_url_in_line("Creator: sluisr (https://sluisr.com)", None),
            Some("https://sluisr.com".to_string())
        );
        assert_eq!(
            find_url_in_line("│   Official Website       https://uti.sluisr.com                            │", None),
            Some("https://uti.sluisr.com".to_string())
        );
        assert_eq!(
            find_url_in_line("Visit https://github.com/sluisr/uti-cli/issues.", None),
            Some("https://github.com/sluisr/uti-cli/issues".to_string())
        );
        assert_eq!(
            find_url_in_line("Active Model: deepseek-flash", None),
            None
        );
    }

    #[test]
    fn test_format_system_message_task_done() {
        let theme = Theme::default();
        
        // 1. Success task with empty output -> single badge line
        let msg = "TASK_DONE|201653|0|0.2s\n";
        let lines = format_system_message(msg, &theme, 80, false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans.iter().any(|s| s.content.contains("[bg:201653]")));
        assert!(lines[0].spans.iter().any(|s| s.content.contains("finished (exit 0)")));

        // 2. Failed task with > 5 output lines -> badge + omission line + last 5 lines (tail)
        let msg_err = "TASK_DONE|201654|2|0.5s\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7";
        let lines_err = format_system_message(msg_err, &theme, 80, false);
        // Line 0: badge, Line 1: … (+2 lines above, Ctrl+O to expand), Lines 2..7: last 5 lines
        assert_eq!(lines_err.len(), 7);
        assert!(lines_err[0].spans.iter().any(|s| s.content.contains("[bg:201654]")));
        assert!(lines_err[0].spans.iter().any(|s| s.content.contains("finished (exit 2)")));
        assert!(lines_err[1].spans.iter().any(|s| s.content.contains("… (+2 lines above")));
        let line2_text: String = lines_err[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line2_text.contains("line 3"));
        let line6_text: String = lines_err[6].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line6_text.contains("line 7"));

        // 3. Expanded logs -> shows all 7 lines (badge + 7 lines = 8 lines)
        let lines_expanded = format_system_message(msg_err, &theme, 80, true);
        assert_eq!(lines_expanded.len(), 8);
        let exp_line1_text: String = lines_expanded[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(exp_line1_text.contains("line 1"));
    }

    #[tokio::test]
    async fn test_balance_slash_command_non_blocking() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);

        assert!(!app.is_checking_balance);
        let handled = app.handle_slash_command("/balance").await;
        assert!(handled);
        assert!(app.is_checking_balance);
        assert_eq!(app.active_status.as_deref(), Some("Checking account balance..."));
        assert_eq!(app.session.messages.len(), 1);
        assert_eq!(app.session.messages[0].text_content(), Some("Checking account balance..."));
    }

    #[tokio::test]
    async fn test_paste_while_streaming() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.auth_dialog.close();
        app.is_streaming = true;

        app.handle_paste("hello world".to_string());
        assert_eq!(app.input_buffer, "hello world");
        assert_eq!(app.cursor_idx, 11);

        // Multiline paste while streaming creates paste tag
        app.handle_paste("\nsecond line\nthird line".to_string());
        assert!(app.input_buffer.contains("[Pasted text #1 +2 lines]"));
        assert_eq!(app.pastes.len(), 1);
    }

    #[tokio::test]
    async fn test_message_queuing_while_streaming() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.auth_dialog.close();
        app.is_streaming = true;

        // Queue two messages
        app.message_queue.push_back("First queued task".to_string());
        app.message_queue.push_back("Second queued task".to_string());
        assert_eq!(app.message_queue.len(), 2);

        // Dequeue first message
        let first = app.message_queue.pop_front().unwrap();
        assert_eq!(first, "First queued task");
        app.session.add_message(Message::user(first));
        assert_eq!(app.session.messages.len(), 1);
        assert_eq!(app.session.messages[0].text_content(), Some("First queued task"));

        // Dequeue second message
        let second = app.message_queue.pop_front().unwrap();
        assert_eq!(second, "Second queued task");
        app.session.add_message(Message::user(second));
        assert_eq!(app.session.messages.len(), 2);
        assert_eq!(app.session.messages[1].text_content(), Some("Second queued task"));
        assert!(app.message_queue.is_empty());
    }
}
