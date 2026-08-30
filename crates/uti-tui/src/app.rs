use std::io::stdout;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, ListState, Paragraph};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use uti_core::client::{get_sudo_password, set_sudo_password, LlmClient, StreamEvent};
use uti_core::config::Config;
use uti_core::session::Session;
use uti_core::types::{Message, ToolCall};
use uti_prompt::PromptBuilder;
use uti_tools::registry::ToolRegistry;
use uti_tools::types::ToolContext;

use crate::ascii::render_gradient_logo;
use crate::auth_dialog::{render_auth_dialog, AuthDialogState};
use crate::diff_view::{build_streaming_tool_preview_lines, build_tool_confirmation_lines};
use crate::markdown::render_markdown;
use crate::model_dialog::{
    render_model_dialog, FlashConfigRow, HybridConfigRow, ModelDialogState, ModelDialogView,
    ProConfigRow, HYBRID_LOCAL_MODELS, HYBRID_PRIMARY_MODELS, PRO_REASONING_LEVELS,
    REASONING_LEVELS, SEARCH_REASONING_LEVELS, TEMPERATURE_PRESETS,
};
use crate::session_dialog::{render_session_dialog, SessionDialogState};
use crate::slash_commands::{render_command_popup, ALL_COMMANDS};
use crate::sudo_dialog::{render_sudo_dialog, SudoDialogState};
use crate::theme::Theme;
use crate::thinking_view::ThinkingState;
use crate::user_dialog::{parse_questions, render_user_dialog, UserDialogState};

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
    pub git_branch: String,
    pub last_turn_start: Option<Instant>,
    pub last_esc_press: Option<Instant>,
    pub last_ctrl_c_press: Option<Instant>,
    pub slash_popup_height_current: f32,
}

impl App {
    pub fn new(llm_client: LlmClient, workspace_dir: PathBuf, yolo: bool) -> Self {
        let branch = Self::detect_git_branch(&workspace_dir);
        let cfg = llm_client.get_config();
        let persistent_history = uti_core::HistoryStore::load();

        let mut auth_dialog = AuthDialogState::new();
        if cfg.api_key.trim().is_empty() {
            auth_dialog.open();
        }

        Self {
            session: Session::new_with_params(&cfg.model, cfg.temperature, &cfg.reasoning_effort, Some(&workspace_dir)),
            llm_client,
            tool_registry: ToolRegistry::new(),
            theme: Theme::default(),
            workspace_dir,

            input_buffer: String::new(),
            input_history: persistent_history,
            history_idx: None,
            saved_draft: String::new(),
            slash_selected_idx: 0,

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
            git_branch: branch,
            last_turn_start: None,
            last_esc_press: None,
            last_ctrl_c_press: None,
            slash_popup_height_current: 0.0,
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

    pub fn start_stream_turn(&mut self, tx: mpsc::Sender<StreamEvent>) {
        let prompt_builder = PromptBuilder::new(&self.workspace_dir)
            .with_sudo_password(get_sudo_password().is_some())
            .with_plan_mode(self.plan_mode);

        let mut messages = vec![Message::system(prompt_builder.build())];
        messages.extend(self.session.messages.clone());

        let tools = self.tool_registry.list_definitions();
        let cancel_token = CancellationToken::new();
        self.cancel_token = Some(cancel_token.clone());

        self.is_streaming = true;
        self.auto_scroll = true;
        self.thinking_state.reset();
        self.thinking_state.is_streaming = true;
        self.last_turn_start = Some(Instant::now());

        let client = self.llm_client.clone();
        tokio::spawn(async move {
            match client.stream_chat(messages, Some(tools), cancel_token).await {
                Ok(mut rx) => {
                    while let Some(evt) = rx.recv().await {
                        let _ = tx.send(evt).await;
                    }
                }
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(e.to_string())).await;
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
                self.thinking_state.reset();
                self.streaming_text.clear();
                return true;
            }
            "/plan" => {
                self.plan_mode = !self.plan_mode;
                let status = if self.plan_mode { "ENABLED" } else { "DISABLED" };
                self.session.add_message(Message::system(format!("Architectural Plan Mode {}", status)));
                return true;
            }
            "/stats" => {
                let u = &self.session.total_usage;
                let ratio = u.cache_hit_percentage();
                let msg = format!(
                    "Session Usage Metrics:\n- Prompt Tokens: {}\n- Cached Prompt Tokens: {} ({:.1}% KV Cache Hit)\n- Completion Tokens: {}\n- Total Tokens: {}",
                    u.prompt_tokens, u.prompt_cache_hit_tokens, ratio, u.completion_tokens, u.total_tokens
                );
                self.session.add_message(Message::system(msg));
                return true;
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
                return true;
            }
            "/hybrid" => {
                let mut cfg = self.llm_client.get_config();
                if parts.len() > 1 {
                    match parts[1].to_lowercase().as_str() {
                        "mode" => {
                            if parts.len() > 2 {
                                if let Some(m) = uti_core::config::HybridMode::from_str_loose(parts[2]) {
                                    cfg.hybrid_settings.mode = m;
                                    cfg.local_llm_enabled = true;
                                    self.llm_client.update_config(cfg.clone());
                                    let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                                    let _ = cfg.save();
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
                                )));
                            }
                        }
                        "on" | "true" | "1" => {
                            cfg.hybrid_compression = true;
                            cfg.local_llm_enabled = true;
                            self.llm_client.update_config(cfg.clone());
                            let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                            let _ = cfg.save();
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
                            let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                            let _ = cfg.save();
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
                    let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                    let _ = cfg.save();
                    self.session.add_message(Message::system(format!("Hybrid Mode {}", state)));
                }
                return true;
            }
            "/balance" | "/wallet" => {
                match self.llm_client.check_balance().await {
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
                        self.session.add_message(Message::system(msg));
                    }
                    Err(e) => {
                        self.session.add_message(Message::system(format!("Failed to retrieve balance: {}", e)));
                    }
                }
                return true;
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
                return true;
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
                    self.session_dialog.open(&self.workspace_dir.display().to_string());
                }
                return true;
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
                return true;
            }
            "/new" => {
                let _ = self.session.save();
                let cfg = self.llm_client.get_config();
                self.session = Session::new_with_params(&cfg.model, cfg.temperature, &cfg.reasoning_effort, Some(&self.workspace_dir));
                self.streaming_text.clear();
                self.thinking_state.reset();
                self.session.add_message(Message::system(format!("Started new chat session (Model: {}).", cfg.model)));
                return true;
            }
            "/model" => {
                if parts.len() > 1 {
                    let new_model = parts[1];
                    let mut cfg = self.llm_client.get_config();
                    cfg.model = new_model.to_string();
                    self.llm_client.update_config(cfg.clone());
                    self.session.model = new_model.to_string();
                    self.session.temperature = Some(cfg.temperature);
                    self.session.reasoning_effort = Some(cfg.reasoning_effort);
                    let _ = self.session.save();
                    self.session.add_message(Message::system(format!("Switched active model to '{}'.", new_model)));
                } else {
                    let cfg = self.llm_client.get_config();
                    self.model_dialog.open(
                        &cfg.model,
                        &cfg.flash_settings,
                        &cfg.pro_settings,
                        &cfg.hybrid_settings,
                        cfg.local_llm_enabled && cfg.hybrid_compression,
                    );
                }
                return true;
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
                return true;
            }
            "/help" => {
                let mut help = "Available Commands:\n".to_string();
                for c in ALL_COMMANDS {
                    help.push_str(&format!("  {:<12} {}\n", c.name, c.description));
                }
                help.push_str("  /sudo <pwd>  Store sudo password in RAM for silent privilege escalation\n");
                help.push_str("\nKeyboard Shortcuts:\n  Enter: Submit | Ctrl+T: Toggle Thought Box | Ctrl+C: Cancel/Exit | Up/Down: History / Command Nav");
                self.session.add_message(Message::system(help));
                return true;
            }
            _ => false,
        }
    }
}

pub async fn run_tui(mut app: App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (event_tx, mut event_rx) = mpsc::channel::<StreamEvent>(100);

    let tick_rate = Duration::from_millis(16);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| {
            render_ui(f, &mut app);
        })?;

        while let Ok(event) = event_rx.try_recv() {
            match event {
                StreamEvent::ReasoningDelta(delta) => {
                    app.thinking_state.content.push_str(&delta);
                }
                StreamEvent::ContentDelta(delta) => {
                    app.streaming_text.push_str(&delta);
                }
                StreamEvent::ToolCallDelta { index, id, name, arguments } => {
                    while app.streaming_tool_calls.len() <= index {
                        app.streaming_tool_calls.push(ToolCall {
                            id: String::new(),
                            call_type: "function".to_string(),
                            function: uti_core::types::FunctionCall {
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
                StreamEvent::ToolExecutionDone { call_id, output } => {
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
                        let key = uti_core::reasoning_cache::ReasoningCache::compute_key(
                            txt,
                            Some(&app.streaming_tool_calls),
                        );
                        app.llm_client.reasoning_cache().insert(key, cot.clone());
                    }

                    if !app.streaming_tool_calls.is_empty() {
                        let calls = app.streaming_tool_calls.clone();
                        app.session.add_message(Message::assistant_with_tools(
                            assistant_text,
                            reasoning,
                            calls.clone(),
                        ));

                        app.streaming_text.clear();
                        app.streaming_tool_calls.clear();
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

                        if let Some(questions) = ask_questions {
                            let ask_call = calls.iter().find(|c| c.function.name == "ask_user").unwrap();
                            app.is_streaming = false;
                            app.thinking_state.reset();
                            app.user_dialog.open(questions, calls.clone(), ask_call.id.clone());
                        } else if requires_confirmation {
                            let combined_preview = if previews.is_empty() {
                                None
                            } else {
                                Some(previews.join("\n"))
                            };

                            app.is_streaming = false;
                            app.thinking_state.reset();
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

                            tokio::spawn(async move {
                                let mut handles = Vec::new();
                                for call in calls {
                                    let reg = registry.clone();
                                    let ctx = context.clone();
                                    let tx_call = tx.clone();
                                    let call_id = call.id.clone();
                                    let tool_name = call.function.name.clone();
                                    let args_str = call.function.arguments.clone();

                                    handles.push(tokio::spawn(async move {
                                        let start_tool = std::time::Instant::now();
                                        let args_json = serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
                                        let result = reg.execute(&tool_name, args_json, &ctx).await;
                                        let duration_ms = start_tool.elapsed().as_millis();
                                        let (output, success) = match result {
                                            Ok(o) => (o.output, true),
                                            Err(e) => (format!("Error executing {}: {}", tool_name, e), false),
                                        };
                                        uti_core::ForensicLogger::log_tool_call(
                                            &tool_name,
                                            &call_id,
                                            &args_str,
                                            &output,
                                            duration_ms,
                                            success
                                        );
                                        let _ = tx_call.send(StreamEvent::ToolExecutionDone { call_id, output }).await;
                                    }));
                                }
                                for h in handles {
                                    let _ = h.await;
                                }
                                let _ = tx.send(StreamEvent::AllToolsDone).await;
                            });
                        }
                    } else {
                        // No tool calls — final model answer received!
                        app.is_streaming = false;
                        app.thinking_state.reset();

                        if let Some(txt) = assistant_text {
                            app.session.add_message(Message::assistant(txt, reasoning));
                        }

                        app.streaming_text.clear();
                        app.streaming_tool_calls.clear();
                        let _ = app.session.save();
                    }
                }
                StreamEvent::Error(err) => {
                    app.is_streaming = false;
                    app.thinking_state.reset();
                    app.session.add_message(Message::system(format!("Error: {}", err)));
                    app.streaming_text.clear();
                    app.streaming_tool_calls.clear();
                }
            }
        }

        if app.is_streaming && app.thinking_state.is_streaming {
            if let Some(start) = app.last_turn_start {
                app.thinking_state.elapsed_secs = start.elapsed().as_secs_f32();
            }
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
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
                    _ => {}
                },
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // --- 0. Cancel active streaming / generation immediately on Esc or Ctrl+C ---
                    if app.is_streaming {
                        if key.code == KeyCode::Esc
                            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
                        {
                            if let Some(token) = app.cancel_token.take() {
                                token.cancel();
                            }
                            app.is_streaming = false;
                            app.thinking_state.is_streaming = false;

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
                    }

                    // --- 1. Sudo Password Dialog Active ---
                    if app.sudo_dialog.is_open {
                        match key.code {
                            KeyCode::Esc => {
                                if let Some(call) = app.sudo_dialog.pending_call.take() {
                                    app.session.add_message(Message::tool_response(
                                        call.id,
                                        "Sudo authentication cancelled by user.",
                                    ));
                                    app.start_stream_turn(event_tx.clone());
                                }
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
                                    set_sudo_password(Some(pwd));
                                    app.session.add_message(Message::system("Sudo password saved in session RAM (silent AskPass enabled)."));
                                    if let Some(call) = app.sudo_dialog.pending_call.take() {
                                        let tx = event_tx.clone();
                                        let registry = app.tool_registry.clone();
                                        let args_json = serde_json::from_str(&call.function.arguments).unwrap_or(serde_json::Value::Null);
                                        let context = ToolContext {
                                            workspace_dir: app.workspace_dir.clone(),
                                            yolo_mode: app.always_allow_tools,
                                            sudo_password: get_sudo_password(),
                                            allowed_commands: app
                                                .llm_client
                                                .get_config()
                                                .allowed_commands
                                                .clone(),
                                        };
                                        app.is_streaming = true;
                                        tokio::spawn(async move {
                                            let output = match registry.execute(&call.function.name, args_json, &context).await {
                                                Ok(o) => o.output,
                                                Err(e) => format!("Execution error: {}", e),
                                            };
                                            let _ = tx.send(StreamEvent::ToolExecutionDone { call_id: call.id, output }).await;
                                            let _ = tx.send(StreamEvent::AllToolsDone).await;
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
                                break;
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
                                    let _ = cfg.save();
                                    app.llm_client.update_config(cfg);
                                    app.auth_dialog.close();
                                    app.session.add_message(Message::system("API key saved successfully to ~/.uti/settings.json. Ready to assist!"));
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
                        match app.model_dialog.view {
                            ModelDialogView::Main => {
                                match key.code {
                                    KeyCode::Esc => {
                                        app.model_dialog.close();
                                    }
                                    KeyCode::Up => {
                                        app.model_dialog.selected_main_idx =
                                            app.model_dialog.selected_main_idx.saturating_sub(1);
                                    }
                                    KeyCode::Down => {
                                        app.model_dialog.selected_main_idx =
                                            (app.model_dialog.selected_main_idx + 1).min(2);
                                    }
                                    KeyCode::Char('1') => {
                                        app.model_dialog.view = ModelDialogView::CloudMenu;
                                    }
                                    KeyCode::Char('2') => {
                                        app.model_dialog.view = ModelDialogView::LocalMenu;
                                    }
                                    KeyCode::Char('3') => {
                                        app.model_dialog.view = ModelDialogView::HybridConfig;
                                    }
                                    KeyCode::Enter => {
                                        match app.model_dialog.selected_main_idx {
                                            0 => {
                                                app.model_dialog.view = ModelDialogView::CloudMenu;
                                            }
                                            1 => {
                                                app.model_dialog.view = ModelDialogView::LocalMenu;
                                            }
                                            2 => {
                                                app.model_dialog.view = ModelDialogView::HybridConfig;
                                            }
                                            _ => {}
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            ModelDialogView::CloudMenu => {
                                match key.code {
                                    KeyCode::Esc => {
                                        app.model_dialog.view = ModelDialogView::Main;
                                    }
                                    KeyCode::Up => {
                                        app.model_dialog.selected_cloud_idx =
                                            app.model_dialog.selected_cloud_idx.saturating_sub(1);
                                    }
                                    KeyCode::Down => {
                                        app.model_dialog.selected_cloud_idx =
                                            (app.model_dialog.selected_cloud_idx + 1).min(3);
                                    }
                                    KeyCode::Tab => {
                                        app.model_dialog.persist_model = !app.model_dialog.persist_model;
                                    }
                                    KeyCode::Char('1') => {
                                        let mut cfg = app.llm_client.get_config();
                                        cfg.model = "deepseek-v4-flash".to_string();
                                        cfg.local_llm_enabled = false;
                                        cfg.hybrid_compression = false;
                                        cfg.temperature = app.model_dialog.temperature;
                                        cfg.reasoning_effort = app.model_dialog.flash_reasoning.clone();
                                        if app.model_dialog.persist_model {
                                            let _ = cfg.save();
                                        }
                                        app.llm_client.update_config(cfg.clone());
                                        app.session.model = cfg.model;
                                        app.session.temperature = Some(cfg.temperature);
                                        app.session.reasoning_effort = Some(cfg.reasoning_effort);
                                        let _ = app.session.save();
                                        app.session.add_message(Message::system("Selected Model: DeepSeek-V4-Flash (Standard Cloud Mode)"));
                                        app.model_dialog.close();
                                    }
                                    KeyCode::Char('2') => {
                                        let mut cfg = app.llm_client.get_config();
                                        cfg.model = "deepseek-v4-pro".to_string();
                                        cfg.local_llm_enabled = false;
                                        cfg.hybrid_compression = false;
                                        cfg.reasoning_effort = app.model_dialog.pro_reasoning.clone();
                                        if app.model_dialog.persist_model {
                                            let _ = cfg.save();
                                        }
                                        app.llm_client.update_config(cfg.clone());
                                        app.session.model = cfg.model;
                                        app.session.temperature = Some(cfg.temperature);
                                        app.session.reasoning_effort = Some(cfg.reasoning_effort);
                                        let _ = app.session.save();
                                        app.session.add_message(Message::system("Selected Model: DeepSeek-V4-Pro (Thinking Cloud Mode)"));
                                        app.model_dialog.close();
                                    }
                                    KeyCode::Char('3') => {
                                        app.model_dialog.view = ModelDialogView::FlashConfig;
                                    }
                                    KeyCode::Char('4') => {
                                        app.model_dialog.view = ModelDialogView::ProConfig;
                                    }
                                    KeyCode::Enter => {
                                        match app.model_dialog.selected_cloud_idx {
                                            0 => {
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.model = "deepseek-v4-flash".to_string();
                                                cfg.local_llm_enabled = false;
                                                cfg.hybrid_compression = false;
                                                cfg.temperature = app.model_dialog.temperature;
                                                cfg.reasoning_effort = app.model_dialog.flash_reasoning.clone();
                                                if app.model_dialog.persist_model {
                                                    let _ = cfg.save();
                                                }
                                                app.llm_client.update_config(cfg.clone());
                                                app.session.model = cfg.model;
                                                app.session.temperature = Some(cfg.temperature);
                                                app.session.reasoning_effort = Some(cfg.reasoning_effort);
                                                let _ = app.session.save();
                                                app.session.add_message(Message::system("Selected Model: DeepSeek-V4-Flash (Standard Cloud Mode)"));
                                                app.model_dialog.close();
                                            }
                                            1 => {
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.model = "deepseek-v4-pro".to_string();
                                                cfg.local_llm_enabled = false;
                                                cfg.hybrid_compression = false;
                                                cfg.reasoning_effort = app.model_dialog.pro_reasoning.clone();
                                                if app.model_dialog.persist_model {
                                                    let _ = cfg.save();
                                                }
                                                app.llm_client.update_config(cfg.clone());
                                                app.session.model = cfg.model;
                                                app.session.temperature = Some(cfg.temperature);
                                                app.session.reasoning_effort = Some(cfg.reasoning_effort);
                                                let _ = app.session.save();
                                                app.session.add_message(Message::system("Selected Model: DeepSeek-V4-Pro (Thinking Cloud Mode)"));
                                                app.model_dialog.close();
                                            }
                                            2 => {
                                                app.model_dialog.view = ModelDialogView::FlashConfig;
                                            }
                                            3 => {
                                                app.model_dialog.view = ModelDialogView::ProConfig;
                                            }
                                            _ => {}
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            ModelDialogView::LocalMenu => {
                                match key.code {
                                    KeyCode::Esc => {
                                        app.model_dialog.view = ModelDialogView::Main;
                                    }
                                    KeyCode::Up => {
                                        app.model_dialog.selected_local_idx =
                                            app.model_dialog.selected_local_idx.saturating_sub(1);
                                    }
                                    KeyCode::Down => {
                                        app.model_dialog.selected_local_idx =
                                            (app.model_dialog.selected_local_idx + 1).min(3);
                                    }
                                    KeyCode::Char('1') => {
                                        let mut cfg = app.llm_client.get_config();
                                        cfg.model = "local-assistant".to_string();
                                        cfg.local_llm_enabled = true;
                                        cfg.local_llm_model = app.model_dialog.hybrid_secondary_local_model.clone();
                                        cfg.local_llm_url = app.model_dialog.hybrid_local_url.clone();
                                        let _ = cfg.save();
                                        app.llm_client.update_config(cfg.clone());
                                        app.session.model = cfg.model.clone();
                                        let _ = app.session.save();
                                        app.session.add_message(Message::system(format!(
                                            "Activated 100% Standalone Offline Local Assistant!\n- Engine: Local LLM ({})\n- Endpoint: {}\n- Cost: $0.00 (Zero Cloud Calls)",
                                            cfg.local_llm_model, cfg.local_llm_url
                                        )));
                                        app.model_dialog.close();
                                    }
                                    KeyCode::Char('2') => {
                                        let cfg = app.llm_client.get_config();
                                        let is_up = app.llm_client.local_client().health_check().await;
                                        app.session.add_message(Message::system(format!(
                                            "Local LLM Server Status:\n- Reachable: {}\n- Endpoint: {}\n- Model: {}",
                                            if is_up { "ONLINE (Active)" } else { "OFFLINE (Unreachable)" },
                                            cfg.local_llm_url,
                                            cfg.local_llm_model
                                        )));
                                        app.model_dialog.close();
                                    }
                                    KeyCode::Char('3') => {
                                        let curr_idx = HYBRID_LOCAL_MODELS
                                            .iter()
                                            .position(|&m| m == app.model_dialog.hybrid_secondary_local_model)
                                            .unwrap_or(0);
                                        let next_idx = (curr_idx + 1) % HYBRID_LOCAL_MODELS.len();
                                        app.model_dialog.hybrid_secondary_local_model =
                                            HYBRID_LOCAL_MODELS[next_idx].to_string();
                                        let mut cfg = app.llm_client.get_config();
                                        cfg.local_llm_model = app.model_dialog.hybrid_secondary_local_model.clone();
                                        cfg.hybrid_settings.secondary_local_model = cfg.local_llm_model.clone();
                                        let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                                        app.llm_client.update_config(cfg);
                                    }
                                    KeyCode::Char('4') => {
                                        if app.model_dialog.hybrid_local_url == "http://127.0.0.1:8080/v1" {
                                            app.model_dialog.hybrid_local_url = "http://localhost:11434/v1".to_string();
                                        } else {
                                            app.model_dialog.hybrid_local_url = "http://127.0.0.1:8080/v1".to_string();
                                        }
                                        let mut cfg = app.llm_client.get_config();
                                        cfg.local_llm_url = app.model_dialog.hybrid_local_url.clone();
                                        cfg.hybrid_settings.local_url = cfg.local_llm_url.clone();
                                        let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                                        app.llm_client.update_config(cfg);
                                    }
                                    KeyCode::Enter => {
                                        match app.model_dialog.selected_local_idx {
                                            0 => {
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.model = "local-assistant".to_string();
                                                cfg.local_llm_enabled = true;
                                                cfg.local_llm_model = app.model_dialog.hybrid_secondary_local_model.clone();
                                                cfg.local_llm_url = app.model_dialog.hybrid_local_url.clone();
                                                let _ = cfg.save();
                                                app.llm_client.update_config(cfg.clone());
                                                app.session.model = cfg.model.clone();
                                                let _ = app.session.save();
                                                app.session.add_message(Message::system(format!(
                                                    "Activated 100% Standalone Offline Local Assistant!\n- Engine: Local LLM ({})\n- Endpoint: {}\n- Cost: $0.00 (Zero Cloud Calls)",
                                                    cfg.local_llm_model, cfg.local_llm_url
                                                )));
                                                app.model_dialog.close();
                                            }
                                            1 => {
                                                let cfg = app.llm_client.get_config();
                                                let is_up = app.llm_client.local_client().health_check().await;
                                                app.session.add_message(Message::system(format!(
                                                    "Local LLM Server Status:\n- Reachable: {}\n- Endpoint: {}\n- Model: {}",
                                                    if is_up { "ONLINE (Active)" } else { "OFFLINE (Unreachable)" },
                                                    cfg.local_llm_url,
                                                    cfg.local_llm_model
                                                )));
                                                app.model_dialog.close();
                                            }
                                            2 => {
                                                let curr_idx = HYBRID_LOCAL_MODELS
                                                    .iter()
                                                    .position(|&m| m == app.model_dialog.hybrid_secondary_local_model)
                                                    .unwrap_or(0);
                                                let next_idx = (curr_idx + 1) % HYBRID_LOCAL_MODELS.len();
                                                app.model_dialog.hybrid_secondary_local_model =
                                                    HYBRID_LOCAL_MODELS[next_idx].to_string();
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.local_llm_model = app.model_dialog.hybrid_secondary_local_model.clone();
                                                cfg.hybrid_settings.secondary_local_model = cfg.local_llm_model.clone();
                                                let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                                                app.llm_client.update_config(cfg);
                                            }
                                            3 => {
                                                if app.model_dialog.hybrid_local_url == "http://127.0.0.1:8080/v1" {
                                                    app.model_dialog.hybrid_local_url = "http://localhost:11434/v1".to_string();
                                                } else {
                                                    app.model_dialog.hybrid_local_url = "http://127.0.0.1:8080/v1".to_string();
                                                }
                                                let mut cfg = app.llm_client.get_config();
                                                cfg.local_llm_url = app.model_dialog.hybrid_local_url.clone();
                                                cfg.hybrid_settings.local_url = cfg.local_llm_url.clone();
                                                let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                                                app.llm_client.update_config(cfg);
                                            }
                                            _ => {}
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            ModelDialogView::FlashConfig => {
                                match key.code {
                                    KeyCode::Esc => {
                                        app.model_dialog.view = ModelDialogView::CloudMenu;
                                    }
                                    KeyCode::Up => {
                                        app.model_dialog.flash_row = match app.model_dialog.flash_row {
                                            FlashConfigRow::Temperature => FlashConfigRow::Temperature,
                                            FlashConfigRow::ModelReasoning => FlashConfigRow::Temperature,
                                            FlashConfigRow::CommandReasoning => FlashConfigRow::ModelReasoning,
                                            FlashConfigRow::CodeReasoning => FlashConfigRow::CommandReasoning,
                                            FlashConfigRow::SearchReasoning => FlashConfigRow::CodeReasoning,
                                            FlashConfigRow::Persistence => FlashConfigRow::SearchReasoning,
                                        };
                                    }
                                    KeyCode::Down => {
                                        app.model_dialog.flash_row = match app.model_dialog.flash_row {
                                            FlashConfigRow::Temperature => FlashConfigRow::ModelReasoning,
                                            FlashConfigRow::ModelReasoning => FlashConfigRow::CommandReasoning,
                                            FlashConfigRow::CommandReasoning => FlashConfigRow::CodeReasoning,
                                            FlashConfigRow::CodeReasoning => FlashConfigRow::SearchReasoning,
                                            FlashConfigRow::SearchReasoning => FlashConfigRow::Persistence,
                                            FlashConfigRow::Persistence => FlashConfigRow::Persistence,
                                        };
                                    }
                                    KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                                        let is_right = key.code == KeyCode::Right;
                                        match app.model_dialog.flash_row {
                                            FlashConfigRow::Temperature => {
                                                let curr_idx = TEMPERATURE_PRESETS.iter().position(|&t| (t - app.model_dialog.temperature).abs() < 0.05).unwrap_or(6);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(TEMPERATURE_PRESETS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.temperature = TEMPERATURE_PRESETS[next_idx];
                                            }
                                            FlashConfigRow::ModelReasoning => {
                                                let curr_idx = REASONING_LEVELS.iter().position(|&r| r == app.model_dialog.flash_reasoning).unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(REASONING_LEVELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.flash_reasoning = REASONING_LEVELS[next_idx].to_string();
                                            }
                                            FlashConfigRow::CommandReasoning => {
                                                let curr_idx = crate::model_dialog::COMMAND_REASONING_LEVELS
                                                    .iter()
                                                    .position(|&r| r == app.model_dialog.flash_command_reasoning)
                                                    .unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(crate::model_dialog::COMMAND_REASONING_LEVELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.flash_command_reasoning =
                                                    crate::model_dialog::COMMAND_REASONING_LEVELS[next_idx].to_string();
                                            }
                                            FlashConfigRow::CodeReasoning => {
                                                let curr_idx = crate::model_dialog::CODE_REASONING_LEVELS
                                                    .iter()
                                                    .position(|&r| r == app.model_dialog.flash_code_reasoning)
                                                    .unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(crate::model_dialog::CODE_REASONING_LEVELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.flash_code_reasoning =
                                                    crate::model_dialog::CODE_REASONING_LEVELS[next_idx].to_string();
                                            }
                                            FlashConfigRow::SearchReasoning => {
                                                let curr_idx = SEARCH_REASONING_LEVELS.iter().position(|&s| s == app.model_dialog.flash_search_reasoning).unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(SEARCH_REASONING_LEVELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.flash_search_reasoning = SEARCH_REASONING_LEVELS[next_idx].to_string();
                                            }
                                            FlashConfigRow::Persistence => {
                                                app.model_dialog.flash_persist_permanent = !app.model_dialog.flash_persist_permanent;
                                            }
                                        }

                                        let flash_settings = app.model_dialog.to_flash_settings();
                                        let mut cfg = app.llm_client.get_config();
                                        cfg.flash_settings = flash_settings.clone();
                                        if cfg.model == "deepseek-v4-flash" || cfg.model == "deepseek-chat" {
                                            cfg.temperature = flash_settings.temperature;
                                            cfg.reasoning_effort = flash_settings.reasoning_effort.clone();
                                        }
                                        if app.model_dialog.flash_persist_permanent {
                                            let _ = Config::save_flash_settings(&flash_settings);
                                        }
                                        app.llm_client.update_config(cfg);
                                    }
                                    _ => {}
                                }
                            }
                            ModelDialogView::ProConfig => {
                                match key.code {
                                    KeyCode::Esc => {
                                        app.model_dialog.view = ModelDialogView::CloudMenu;
                                    }
                                    KeyCode::Up => {
                                        app.model_dialog.pro_row = match app.model_dialog.pro_row {
                                            ProConfigRow::ModelReasoning => ProConfigRow::ModelReasoning,
                                            ProConfigRow::SearchReasoning => ProConfigRow::ModelReasoning,
                                            ProConfigRow::Persistence => ProConfigRow::SearchReasoning,
                                        };
                                    }
                                    KeyCode::Down => {
                                        app.model_dialog.pro_row = match app.model_dialog.pro_row {
                                            ProConfigRow::ModelReasoning => ProConfigRow::SearchReasoning,
                                            ProConfigRow::SearchReasoning => ProConfigRow::Persistence,
                                            ProConfigRow::Persistence => ProConfigRow::Persistence,
                                        };
                                    }
                                    KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                                        let is_right = key.code == KeyCode::Right;
                                        match app.model_dialog.pro_row {
                                            ProConfigRow::ModelReasoning => {
                                                let curr_idx = PRO_REASONING_LEVELS.iter().position(|&r| r == app.model_dialog.pro_reasoning).unwrap_or(3);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(PRO_REASONING_LEVELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.pro_reasoning = PRO_REASONING_LEVELS[next_idx].to_string();
                                            }
                                            ProConfigRow::SearchReasoning => {
                                                let curr_idx = SEARCH_REASONING_LEVELS.iter().position(|&s| s == app.model_dialog.pro_search_reasoning).unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(SEARCH_REASONING_LEVELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.pro_search_reasoning = SEARCH_REASONING_LEVELS[next_idx].to_string();
                                            }
                                            ProConfigRow::Persistence => {
                                                app.model_dialog.pro_persist_permanent = !app.model_dialog.pro_persist_permanent;
                                            }
                                        }

                                        let pro_settings = app.model_dialog.to_pro_settings();
                                        let mut cfg = app.llm_client.get_config();
                                        cfg.pro_settings = pro_settings.clone();
                                        if cfg.model == "deepseek-v4-pro" || cfg.model == "deepseek-reasoner" {
                                            cfg.reasoning_effort = pro_settings.reasoning_effort.clone();
                                        }
                                        if app.model_dialog.pro_persist_permanent {
                                            let _ = Config::save_pro_settings(&pro_settings);
                                        }
                                        app.llm_client.update_config(cfg);
                                    }
                                    _ => {}
                                }
                            }
                            ModelDialogView::HybridConfig => {
                                match key.code {
                                    KeyCode::Esc => {
                                        app.model_dialog.view = ModelDialogView::Main;
                                    }
                                    KeyCode::Up => {
                                        app.model_dialog.hybrid_row = match app.model_dialog.hybrid_row {
                                            HybridConfigRow::Mode => HybridConfigRow::Mode,
                                            HybridConfigRow::PrimaryModel => HybridConfigRow::Mode,
                                            HybridConfigRow::CommandReasoning => HybridConfigRow::PrimaryModel,
                                            HybridConfigRow::CodeReasoning => HybridConfigRow::CommandReasoning,
                                            HybridConfigRow::SecondaryLocalModel => HybridConfigRow::CodeReasoning,
                                            HybridConfigRow::LocalUrl => HybridConfigRow::SecondaryLocalModel,
                                            HybridConfigRow::AutoCompression => HybridConfigRow::LocalUrl,
                                            HybridConfigRow::Persistence => HybridConfigRow::AutoCompression,
                                        };
                                    }
                                    KeyCode::Down => {
                                        app.model_dialog.hybrid_row = match app.model_dialog.hybrid_row {
                                            HybridConfigRow::Mode => HybridConfigRow::PrimaryModel,
                                            HybridConfigRow::PrimaryModel => HybridConfigRow::CommandReasoning,
                                            HybridConfigRow::CommandReasoning => HybridConfigRow::CodeReasoning,
                                            HybridConfigRow::CodeReasoning => HybridConfigRow::SecondaryLocalModel,
                                            HybridConfigRow::SecondaryLocalModel => HybridConfigRow::LocalUrl,
                                            HybridConfigRow::LocalUrl => HybridConfigRow::AutoCompression,
                                            HybridConfigRow::AutoCompression => HybridConfigRow::Persistence,
                                            HybridConfigRow::Persistence => HybridConfigRow::Persistence,
                                        };
                                    }
                                    KeyCode::Enter => {
                                        let mut cfg = app.llm_client.get_config();
                                        cfg.model = app.model_dialog.hybrid_primary_model.clone();
                                        cfg.flash_settings.command_reasoning_effort = app.model_dialog.hybrid_command_reasoning.clone();
                                        cfg.flash_settings.code_reasoning_effort = app.model_dialog.hybrid_code_reasoning.clone();
                                        cfg.local_llm_enabled = true;
                                        cfg.hybrid_compression = app.model_dialog.hybrid_auto_compression;
                                        cfg.local_llm_url = app.model_dialog.hybrid_local_url.clone();
                                        cfg.local_llm_model = app.model_dialog.hybrid_secondary_local_model.clone();
                                        cfg.hybrid_settings = app.model_dialog.to_hybrid_settings();
                                        if app.model_dialog.hybrid_persist_permanent {
                                            let _ = cfg.save();
                                            let _ = Config::save_hybrid_settings(&cfg.hybrid_settings);
                                            let _ = Config::save_flash_settings(&cfg.flash_settings);
                                        }
                                        app.llm_client.update_config(cfg.clone());
                                        app.session.model = cfg.model.clone();
                                        let _ = app.session.save();
                                        app.session.add_message(Message::system(format!(
                                            "Activated Hybrid Mode!\n- Strategy: {}\n- Primary Cloud Engine: {}\n- Command CoT: {} | Code CoT: {}\n- Secondary Local Assistant: {}\n- Local Server: {}",
                                            cfg.hybrid_settings.mode.display_name(),
                                            cfg.model,
                                            cfg.flash_settings.command_reasoning_effort,
                                            cfg.flash_settings.code_reasoning_effort,
                                            cfg.local_llm_model,
                                            cfg.local_llm_url
                                        )));
                                        app.model_dialog.close();
                                    }
                                    KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                                        let is_right = key.code == KeyCode::Right;
                                        match app.model_dialog.hybrid_row {
                                            HybridConfigRow::Mode => {
                                                let curr_idx = crate::model_dialog::HYBRID_MODES
                                                    .iter()
                                                    .position(|&m| m == app.model_dialog.hybrid_mode)
                                                    .unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1) % crate::model_dialog::HYBRID_MODES.len()
                                                } else if curr_idx == 0 {
                                                    crate::model_dialog::HYBRID_MODES.len() - 1
                                                } else {
                                                    curr_idx - 1
                                                };
                                                app.model_dialog.hybrid_mode = crate::model_dialog::HYBRID_MODES[next_idx];
                                            }
                                            HybridConfigRow::PrimaryModel => {
                                                let curr_idx = HYBRID_PRIMARY_MODELS
                                                    .iter()
                                                    .position(|&m| m == app.model_dialog.hybrid_primary_model)
                                                    .unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(HYBRID_PRIMARY_MODELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.hybrid_primary_model =
                                                    HYBRID_PRIMARY_MODELS[next_idx].to_string();
                                            }
                                            HybridConfigRow::CommandReasoning => {
                                                let curr_idx = crate::model_dialog::COMMAND_REASONING_LEVELS
                                                    .iter()
                                                    .position(|&r| r == app.model_dialog.hybrid_command_reasoning)
                                                    .unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(crate::model_dialog::COMMAND_REASONING_LEVELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.hybrid_command_reasoning =
                                                    crate::model_dialog::COMMAND_REASONING_LEVELS[next_idx].to_string();
                                            }
                                            HybridConfigRow::CodeReasoning => {
                                                let curr_idx = crate::model_dialog::CODE_REASONING_LEVELS
                                                    .iter()
                                                    .position(|&r| r == app.model_dialog.hybrid_code_reasoning)
                                                    .unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(crate::model_dialog::CODE_REASONING_LEVELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.hybrid_code_reasoning =
                                                    crate::model_dialog::CODE_REASONING_LEVELS[next_idx].to_string();
                                            }
                                            HybridConfigRow::SecondaryLocalModel => {
                                                let curr_idx = HYBRID_LOCAL_MODELS
                                                    .iter()
                                                    .position(|&m| m == app.model_dialog.hybrid_secondary_local_model)
                                                    .unwrap_or(0);
                                                let next_idx = if is_right {
                                                    (curr_idx + 1).min(HYBRID_LOCAL_MODELS.len() - 1)
                                                } else {
                                                    curr_idx.saturating_sub(1)
                                                };
                                                app.model_dialog.hybrid_secondary_local_model =
                                                    HYBRID_LOCAL_MODELS[next_idx].to_string();
                                            }
                                            HybridConfigRow::LocalUrl => {
                                                if app.model_dialog.hybrid_local_url == "http://127.0.0.1:8080/v1" {
                                                    app.model_dialog.hybrid_local_url = "http://localhost:11434/v1".to_string();
                                                } else {
                                                    app.model_dialog.hybrid_local_url = "http://127.0.0.1:8080/v1".to_string();
                                                }
                                            }
                                            HybridConfigRow::AutoCompression => {
                                                app.model_dialog.hybrid_auto_compression =
                                                    !app.model_dialog.hybrid_auto_compression;
                                            }
                                            HybridConfigRow::Persistence => {
                                                app.model_dialog.hybrid_persist_permanent =
                                                    !app.model_dialog.hybrid_persist_permanent;
                                            }
                                        }

                                        let hybrid_settings = app.model_dialog.to_hybrid_settings();
                                        let mut cfg = app.llm_client.get_config();
                                        cfg.hybrid_settings = hybrid_settings.clone();
                                        cfg.flash_settings.command_reasoning_effort = app.model_dialog.hybrid_command_reasoning.clone();
                                        cfg.flash_settings.code_reasoning_effort = app.model_dialog.hybrid_code_reasoning.clone();
                                        cfg.local_llm_url = hybrid_settings.local_url.clone();
                                        cfg.local_llm_model = hybrid_settings.secondary_local_model.clone();
                                        cfg.hybrid_compression = hybrid_settings.auto_compression;
                                        if app.model_dialog.hybrid_persist_permanent {
                                            let _ = Config::save_hybrid_settings(&hybrid_settings);
                                            let _ = Config::save_flash_settings(&cfg.flash_settings);
                                        }
                                        app.llm_client.update_config(cfg);
                                    }
                                    _ => {}
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
                            let context = ToolContext {
                                workspace_dir: app.workspace_dir.clone(),
                                yolo_mode: app.always_allow_tools,
                                sudo_password: get_sudo_password(),
                                allowed_commands: app.llm_client.get_config().allowed_commands.clone(),
                            };

                            let tx = event_tx.clone();
                            let registry = app.tool_registry.clone();
                            app.is_streaming = true;

                            tokio::spawn(async move {
                                let mut handles = Vec::new();
                                for call in calls {
                                    let reg = registry.clone();
                                    let ctx = context.clone();
                                    let tx_call = tx.clone();
                                    let call_id = call.id.clone();
                                    let tool_name = call.function.name.clone();
                                    let args_str = call.function.arguments.clone();

                                    handles.push(tokio::spawn(async move {
                                        let args_json = serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
                                        let output = match reg.execute(&tool_name, args_json, &ctx).await {
                                            Ok(o) => o.output,
                                            Err(e) => format!("Execution error: {}", e),
                                        };
                                        let _ = tx_call.send(StreamEvent::ToolExecutionDone { call_id, output }).await;
                                    }));
                                }
                                for h in handles {
                                    let _ = h.await;
                                }
                                let _ = tx.send(StreamEvent::AllToolsDone).await;
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
                                app.session.messages.clear();
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
                        KeyCode::Char(c) => {
                            app.input_buffer.push(c);
                            app.slash_selected_idx = 0;
                            app.last_esc_press = None;
                        }
                        KeyCode::Backspace => {
                            app.input_buffer.pop();
                            app.slash_selected_idx = 0;
                            app.last_esc_press = None;
                        }
                        KeyCode::Esc => {
                            if is_slash_open && !matching_cmds.is_empty() {
                                app.input_buffer.clear();
                                app.slash_selected_idx = 0;
                                app.last_esc_press = None;
                            } else {
                                let is_recent = app.last_esc_press
                                    .map(|i| i.elapsed() <= Duration::from_millis(500))
                                    .unwrap_or(false);

                                if is_recent {
                                    // 2nd ESC within 500ms -> Clear prompt & reset history navigation!
                                    app.input_buffer.clear();
                                    app.history_idx = None;
                                    app.saved_draft.clear();
                                    app.slash_selected_idx = 0;
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
                                    }
                                    Some(i) => {
                                        if i > 0 {
                                            let next_i = i - 1;
                                            app.history_idx = Some(next_i);
                                            app.input_buffer = app.input_history[next_i].clone();
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
                                } else {
                                    app.history_idx = None;
                                    app.input_buffer = std::mem::take(&mut app.saved_draft);
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
                            if !text.is_empty() && !app.is_streaming {
                                let cmd_to_run = if is_slash_open && !matching_cmds.is_empty() && !text.contains(' ') {
                                    matching_cmds[app.slash_selected_idx % matching_cmds.len()].to_string()
                                } else {
                                    text.clone()
                                };

                                if app.input_history.last().map(|s| s.as_str()) != Some(&cmd_to_run) {
                                    app.input_history.push(cmd_to_run.clone());
                                    uti_core::HistoryStore::append(&cmd_to_run);
                                }
                                app.history_idx = None;
                                app.saved_draft.clear();
                                app.input_buffer.clear();
                                app.slash_selected_idx = 0;

                                if cmd_to_run == "/quit" || cmd_to_run == "/exit" {
                                    break;
                                }

                                if cmd_to_run.starts_with('/') {
                                    if app.handle_slash_command(&cmd_to_run).await {
                                        continue;
                                    }
                                }

                                let mut user_prompt_clean = cmd_to_run.clone();

                                // Extract and strip $sudo:<password> securely into session RAM
                                if let Some(pos) = user_prompt_clean.find("$sudo:") {
                                    let after = &user_prompt_clean[pos + 6..];
                                    let end = after.find(' ').unwrap_or(after.len());
                                    let pwd = &after[..end];
                                    if !pwd.is_empty() {
                                        set_sudo_password(Some(pwd.to_string()));
                                        app.session.add_message(Message::system("Sudo password stored in session RAM (silent AskPass enabled)."));
                                    }
                                    let before = &user_prompt_clean[..pos];
                                    let rest = &after[end..];
                                    user_prompt_clean = format!("{} {}", before.trim(), rest.trim()).trim().to_string();
                                }

                                // Extract $auto / $yolo
                                if user_prompt_clean.starts_with("$auto") || user_prompt_clean.starts_with("$yolo") {
                                    app.always_allow_tools = true;
                                    user_prompt_clean = user_prompt_clean
                                        .trim_start_matches("$auto")
                                        .trim_start_matches("$yolo")
                                        .trim()
                                        .to_string();
                                }

                                if user_prompt_clean.is_empty() {
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
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    println!("Session saved. Resume anytime with `uti --resume {}`", app.session.id);
    Ok(())
}

/// Finishes the interactive ask_user dialog: executes the pending tool batch
/// (feeding the user's answers to the `ask_user` call), then triggers the next
/// agent turn via `AllToolsDone`.
fn finish_ask_user(app: &mut App, event_tx: mpsc::Sender<StreamEvent>) {
    let tx = event_tx.clone();
    let registry = app.tool_registry.clone();
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
                    .send(StreamEvent::ToolExecutionDone {
                        call_id: call.id,
                        output: output.clone(),
                    })
                    .await;
            } else if cancelled {
                // Batch aborted: report every remaining call as skipped.
                let _ = tx
                    .send(StreamEvent::ToolExecutionDone {
                        call_id: call.id,
                        output: "Skipped: user cancelled the pending questions.".to_string(),
                    })
                    .await;
            } else {
                let args_json = serde_json::from_str(&call.function.arguments)
                    .unwrap_or(serde_json::Value::Null);
                let out = match registry.execute(&call.function.name, args_json, &context).await {
                    Ok(o) => o.output,
                    Err(e) => format!("Error executing {}: {}", call.function.name, e),
                };
                let _ = tx
                    .send(StreamEvent::ToolExecutionDone { call_id: call.id, output: out })
                    .await;
            }
        }
        let _ = tx.send(StreamEvent::AllToolsDone).await;
    });
}

fn format_tool_call_spans(name: &str, raw_args: &str, theme: &Theme) -> Vec<Span<'static>> {
    let args_val = serde_json::from_str::<serde_json::Value>(raw_args).ok();
    match name {
        "run_shell_command" | "shell" => {
            let cmd = args_val.as_ref()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()))
                .unwrap_or(raw_args);
            let single_line_cmd = cmd.replace('\n', " ").replace("  ", " ");
            let display_cmd = if single_line_cmd.len() > 80 {
                format!("{}...", &single_line_cmd[..77])
            } else {
                single_line_cmd
            };
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
        _ => {
            let display_args = if raw_args.len() > 60 {
                format!("{}...", &raw_args[..57])
            } else {
                raw_args.to_string()
            };
            vec![
                Span::styled("  [TOOL] ", Style::default().fg(theme.accent_yellow)),
                Span::styled(name.to_string(), Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({})", display_args), Style::default().fg(theme.dark_gray)),
            ]
        }
    }
}

fn render_ui(frame: &mut Frame, app: &mut App) {
    let size = frame.area();

    let show_popup = app.input_buffer.starts_with('/');
    let target_height = if show_popup {
        let filter = app.input_buffer.to_lowercase();
        let matching_count = ALL_COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(&filter))
            .count();
        if matching_count > 0 {
            (matching_count as f32 + 2.0).min(10.0)
        } else {
            0.0
        }
    } else {
        0.0
    };

    let speed = 0.35;
    let diff = target_height - app.slash_popup_height_current;
    if diff.abs() > 0.05 {
        app.slash_popup_height_current += diff * speed;
    } else {
        app.slash_popup_height_current = target_height;
    }

    let popup_height = app.slash_popup_height_current.round() as u16;

    let chunks = if popup_height > 0 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),               // Chat Feed
                Constraint::Length(popup_height), // Autocomplete Popup Slot
                Constraint::Length(3),            // Input Composer
                Constraint::Length(1),            // Status Footer Bar
            ])
            .split(size)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),               // Chat Feed
                Constraint::Length(3),            // Input Composer
                Constraint::Length(1),            // Status Footer Bar
            ])
            .split(size)
    };

    let chat_chunk = chunks[0];
    let (popup_chunk, composer_chunk, status_chunk) = if popup_height > 0 {
        (Some(chunks[1]), chunks[2], chunks[3])
    } else {
        (None, chunks[1], chunks[2])
    };

    let content_max_width = (chat_chunk.width as usize).saturating_sub(4).max(20);
    let mut all_lines = Vec::new();

    // 1. Header Banner (First item in scrollable feed!)
    let header_lines = render_gradient_logo("0.1.0");
    all_lines.extend(header_lines);

    // 2. Messages & Tool Executions List
    for (msg_idx, msg) in app.session.messages.iter().enumerate() {
        match msg.role.as_str() {
            "user" => {
                let content = msg.text_content().unwrap_or("");
                let user_spans = vec![
                    Span::styled("❯ ", Style::default().fg(app.theme.accent_blue).add_modifier(Modifier::BOLD)),
                    Span::styled(content.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                ];
                let mut lines = crate::markdown::wrap_spans(user_spans, content_max_width, "  ");
                lines.push(Line::from(""));
                all_lines.extend(lines);
            }
            "assistant" => {
                if let Some(text) = msg.text_content() {
                    let mut lines = render_markdown(text, &app.theme, content_max_width);
                    lines.push(Line::from(""));
                    all_lines.extend(lines);
                }

                if let Some(ref calls) = msg.tool_calls {
                    for call in calls {
                        // If this tool call is currently pending confirmation, do not render duplicate raw JSON line!
                        if let Some(ref pending) = app.pending_confirmation {
                            if pending.calls.iter().any(|c| c.id == call.id) {
                                continue;
                            }
                        }

                        let spans = format_tool_call_spans(&call.function.name, &call.function.arguments, &app.theme);
                        let wrapped = crate::markdown::wrap_spans(spans, content_max_width, "  ");
                        all_lines.extend(wrapped);
                    }
                }
            }
            "tool" => {
                let content = msg.text_content().unwrap_or("");
                let is_last_tool = msg_idx + 1 >= app.session.messages.len()
                    || app.session.messages[msg_idx + 1].role != "tool";

                let line_spans = if content.contains("denied by user") || content.contains("declined") {
                    vec![
                        Span::styled("    ✕ ", Style::default().fg(Color::Red)),
                        Span::styled("Execution declined by user", Style::default().fg(app.theme.gray)),
                    ]
                } else {
                    let first_line = content
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("Done")
                        .replace('\t', "    ");
                    vec![
                        Span::styled("    ✓ ", Style::default().fg(Color::Green)),
                        Span::styled(first_line, Style::default().fg(app.theme.gray)),
                    ]
                };

                let wrapped = crate::markdown::wrap_spans(line_spans, content_max_width, "    ");
                all_lines.extend(wrapped);
                if is_last_tool {
                    all_lines.push(Line::from(""));
                }
            }
            "system" => {
                if let Some(text) = msg.text_content() {
                    let sys_spans = vec![
                        Span::styled("  • ", Style::default().fg(app.theme.accent_cyan)),
                        Span::styled(text.to_string(), Style::default().fg(app.theme.gray)),
                    ];
                    let wrapped = crate::markdown::wrap_spans(sys_spans, content_max_width, "    ");
                    all_lines.extend(wrapped);
                    all_lines.push(Line::from(""));
                }
            }
            _ => {}
        }
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
        let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let elapsed = app.last_turn_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
        let idx = ((elapsed * 10.0) as usize) % spinner_frames.len();
        let spinner_char = spinner_frames[idx];

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
        } else if app.streaming_text.is_empty() {
            all_lines.push(Line::from(vec![
                Span::styled(format!("  {} ", spinner_char), Style::default().fg(app.theme.accent_purple).add_modifier(Modifier::BOLD)),
                Span::styled("Generating...", Style::default().fg(app.theme.gray)),
            ]));
            all_lines.push(Line::from(""));
        }
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

    let message_paragraph = Paragraph::new(all_lines)
        .block(Block::default().borders(Borders::NONE))
        .scroll((app.scroll_offset, 0));
    frame.render_widget(message_paragraph, chat_chunk);

    // 3. Input Prompt Composer
    let prompt_prefix = if app.always_allow_tools {
        Span::styled("* ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else if app.plan_mode {
        Span::styled("? ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("> ", Style::default().fg(app.theme.accent_blue).add_modifier(Modifier::BOLD))
    };

    let show_esc_hint = app.last_esc_press
        .map(|i| i.elapsed() <= Duration::from_millis(500))
        .unwrap_or(false);

    let prompt_content = if app.pending_confirmation.is_some() {
        Line::from(vec![
            prompt_prefix,
            Span::styled("(Press 1-3 or Enter to select, Esc to decline)", Style::default().fg(app.theme.dark_gray)),
        ])
    } else if show_esc_hint {
        let msg = if app.input_buffer.is_empty() {
            "Press Esc again to rewind."
        } else {
            "Press Esc again to clear prompt."
        };
        Line::from(vec![
            prompt_prefix,
            Span::raw(&app.input_buffer),
            Span::styled("█ ", Style::default().fg(app.theme.accent_blue)),
            Span::styled(format!("({})", msg), Style::default().fg(app.theme.gray)),
        ])
    } else if app.input_buffer.is_empty() {
        Line::from(vec![
            prompt_prefix,
            Span::styled("█ ", Style::default().fg(app.theme.accent_blue)),
            Span::styled("Type your message or @path/to/file", Style::default().fg(app.theme.dark_gray)),
        ])
    } else {
        Line::from(vec![
            prompt_prefix,
            Span::raw(&app.input_buffer),
            Span::styled("█", Style::default().fg(app.theme.accent_blue)),
        ])
    };

    let composer_block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(app.theme.dark_gray));

    // Horizontal scroll: keep the end of the input (and the █ cursor) visible
    // once the text grows beyond the composer width.
    let line_width = prompt_content.width() as u16;
    let visible = composer_chunk.width.saturating_sub(1);
    let scroll_x = line_width.saturating_sub(visible);

    frame.render_widget(
        Paragraph::new(prompt_content)
            .block(composer_block)
            .scroll((0, scroll_x)),
        composer_chunk,
    );

    // Render slash command popup if active
    if let Some(p_chunk) = popup_chunk {
        render_command_popup(frame, &app.input_buffer, app.slash_selected_idx, p_chunk, &app.theme);
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
    let is_flash = cfg.model.contains("flash") || cfg.model == "deepseek-chat" || cfg.model == "DeepSeek-V4-Flash";
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

        let r_color = match cfg.reasoning_effort.as_str() {
            "low" => Color::Rgb(105, 240, 174),
            "medium" => Color::Rgb(255, 152, 0),
            "high" => Color::Rgb(244, 67, 54),
            "max" => Color::Rgb(224, 64, 251),
            _ => app.theme.accent_cyan,
        };

        right_spans.push(Span::styled("DeepSeek-V4-Flash", Style::default().fg(app.theme.accent_purple).add_modifier(Modifier::BOLD)));
        right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
        right_spans.push(Span::styled(format!("{:.1}", temp), Style::default().fg(temp_color)));
        right_spans.push(Span::styled(" · ", Style::default().fg(app.theme.gray)));
        right_spans.push(Span::styled(format!("{} ", cfg.reasoning_effort), Style::default().fg(r_color)));
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

    let footer_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
        ])
        .split(status_chunk);

    frame.render_widget(Paragraph::new(Line::from(left_spans)).alignment(Alignment::Left), footer_cols[0]);
    frame.render_widget(Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right), footer_cols[1]);

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
