use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use crossterm::event::{
    MouseButton, MouseEvent, MouseEventKind,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use corex_core::client::{get_sudo_password, set_sudo_password, LlmClient, StreamEvent};
use corex_core::config::Config;
use corex_core::session::Session;
use corex_core::types::Message;
use corex_prompt::PromptBuilder;
use corex_tools::registry::ToolRegistry;

use crate::auth_dialog::AuthDialogState;
use crate::clipboard;
use crate::fleet_dialog::FleetDialogState;
use crate::model_dialog::{DialogAction, ModelDialogState};
use crate::session_dialog::SessionDialogState;
use crate::slash_commands::ALL_COMMANDS;
use crate::sudo_dialog::SudoDialogState;
use crate::theme::Theme;
use crate::thinking_view::ThinkingState;
use crate::user_dialog::UserDialogState;

pub mod state;
pub use state::*;
pub mod formatting;
pub use formatting::*;
pub mod mobile;
pub use mobile::*;
pub mod terminal;
pub use terminal::*;
pub mod render;
pub use render::*;
pub mod event_loop;
pub use event_loop::*;

impl App {
    pub fn new(llm_client: LlmClient, workspace_dir: PathBuf, yolo: bool) -> Self {
        corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(llm_client.clone()));
        let branch = Self::detect_git_branch(&workspace_dir);
        let cfg = llm_client.get_config();
        let persistent_history = corex_core::HistoryStore::load();

        let auth_dialog = AuthDialogState::new();

        let is_online = Arc::new(AtomicBool::new(false));
        let (balance_tx, balance_rx) = mpsc::channel(10);
        let mut app = Self {
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
            streaming_extra_content: None,

            pending_confirmation: None,
            user_dialog: UserDialogState::new(),
            model_dialog: ModelDialogState::new(),
            session_dialog: SessionDialogState::new(),
            auth_dialog,
            sudo_dialog: SudoDialogState::new(),
            fleet_dialog: FleetDialogState::new(),
            fleet_session: None,
            normal_session_stash: None,
            cancel_token: None,
            in_flight_tool_calls: 0,
            response_prefix: None,

            scroll_offset: 0,
            auto_scroll: true,
            total_rendered_items: 0,
            plan_mode: false,
            always_allow_tools: yolo,
            mouse_capture: mouse_capture_enabled(),
            selection: None,
            selection_lines: SelectionLines::default(),
            drag_point: None,
            app_start_time: Instant::now(),
            last_focus_gained: None,
            mouse_down_time: None,
            mouse_down_pos: None,
            last_url_open: None,
            session_allowed_commands: Vec::new(),
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
            cached_message_line_offsets: Vec::new(),
            cached_message_count: 0,
            cached_render_width: 0,
            cached_session_id: String::new(),
            cached_logs_expanded: false,
            streaming_md_cache: Vec::new(),
            streaming_md_cache_len: 0,
            streaming_md_cache_width: 0,
            active_background_pids: std::collections::HashSet::new(),
            current_generation_id: 0,
            update_available: Arc::new(std::sync::Mutex::new(corex_core::update::check_cached_update(env!("CARGO_PKG_VERSION")))),
            active_status: None,
            status_transition: StatusTransition::new(),
            last_chat_rect: None,
            balance_tx,
            balance_rx,
            is_checking_balance: false,
            pending_balance_msg_index: None,
            message_queue: VecDeque::new(),
            start_time: Instant::now(),
            last_interaction: Instant::now(),
        };
        corex_fleet::register_fleet_tools(&mut app.tool_registry);
        app.trigger_local_health_check();
        app.trigger_update_check();
        if cfg.api_key.trim().is_empty() && cfg.endpoint().1.trim().is_empty() && !cfg.local_llm_enabled {
            app.set_status("No API key configured · Type /model to choose AI or /local");
        }
        app
    }

    pub fn set_status(&mut self, text: &str) {
        self.status_transition.set_target(text);
        self.active_status = Some(text.to_string());
    }

    /// Enables or disables terminal mouse reporting at runtime.
    ///
    /// The escape sequence itself is emitted by the main event loop, which reconciles
    /// `mouse_capture` against the real terminal state, so F2 and `COREX_MOUSE`
    /// all funnel through one code path.
    pub fn set_mouse_capture(&mut self, enabled: bool) {
        self.mouse_capture = enabled;
        let msg = if enabled {
            "Mouse reporting ENABLED: the wheel scrolls the feed, and dragging over it selects text, which is copied to the clipboard on release. F2 gives the mouse back to the terminal."
        } else {
            "Mouse reporting DISABLED: the terminal owns the mouse again and its native selection is back. Scroll with PageUp/PageDown or Shift+Up/Shift+Down."
        };
        self.session.add_message(Message::system(msg));
    }

    /// Flips mouse reporting on/off (bound to F2).
    pub fn toggle_mouse_capture(&mut self) {
        self.set_mouse_capture(!self.mouse_capture);
    }

    /// Responds to terminal resize events: clears rendering caches and dynamically adapts
    /// mouse reporting on mobile vertical screens unless COREX_MOUSE is pinned.
    pub fn handle_resize(&mut self, width: u16, height: u16) {
        self.cached_message_lines.clear();
        self.cached_message_line_offsets.clear();
        self.cached_render_width = 0;
        self.last_chat_rect = None;
        if std::env::var("COREX_MOUSE").is_err() {
            let should_capture = !is_mobile_portrait(width, height);
            if self.mouse_capture != should_capture {
                self.mouse_capture = should_capture;
            }
        }
    }

    // --- Chat text selection (driven by mouse drags, see `Selection`) ---

    /// Routes a mouse event: the wheel scrolls the feed, the left button selects text or opens links.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_selection(-3),
            MouseEventKind::ScrollDown => self.scroll_selection(3),
            MouseEventKind::Down(MouseButton::Left) => {
                self.mouse_down_time = Some(Instant::now());
                self.mouse_down_pos = Some((mouse.column, mouse.row));
                self.begin_selection(mouse.column, mouse.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.extend_selection(mouse.column, mouse.row);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let down_time = self.mouse_down_time.take();
                let down_pos = self.mouse_down_pos.take();
                self.commit_selection_with_click(mouse.column, mouse.row, down_time, down_pos);
            }
            _ => {}
        }
    }

    /// Scrolls the feed, taking a live selection endpoint along with the view.
    ///
    /// With the button held the pointer stays on the same screen row, so scrolling swaps the line
    /// underneath it. Moving the endpoint by the applied delta keeps the selection anchored to the
    /// text rather than to the screen, which is how a terminal behaves.
    fn scroll_selection(&mut self, delta: i32) {
        let before = self.scroll_offset;
        self.scroll_chat(delta);
        let applied = self.scroll_offset as i32 - before as i32;
        if applied == 0 {
            return;
        }
        if let Some(selection) = self.selection.as_mut() {
            selection.cursor.0 = (selection.cursor.0 as i32 + applied).max(0) as usize;
        }
    }

    /// Keeps the feed scrolling while a drag is held at the edge of the viewport.
    ///
    /// Drag events stop the moment the pointer stops moving, so on its own a selection could only
    /// ever reach one line past the edge. This is called on every tick, which is what lets a drag
    /// held at the top row climb the whole conversation. Returns true when the view moved.
    pub fn autoscroll_selection(&mut self) -> bool {
        let Some((column, row)) = self.drag_point else { return false };
        if self.selection.is_none() {
            return false;
        }
        let Some(direction) = self.selection_scroll_delta(row) else { return false };
        // Three lines a tick: fast enough to climb a long conversation, slow enough to aim.
        let delta = direction * 3;

        let before = self.scroll_offset;
        self.scroll_chat(delta);
        if self.scroll_offset == before {
            return false;
        }
        // The pointer has not moved, so the line under it has: keep the endpoint under it.
        if let Some(point) = self.chat_point(column, row) {
            if let Some(selection) = self.selection.as_mut() {
                selection.cursor = point;
            }
        }
        true
    }

    /// Starts a selection on a press inside the chat viewport; a press elsewhere clears it.
    pub fn begin_selection(&mut self, column: u16, row: u16) {
        if !self.is_over_chat(column, row) {
            self.clear_selection();
            return;
        }
        if let Some(point) = self.chat_point(column, row) {
            self.selection = Some(Selection { anchor: point, cursor: point });
            self.drag_point = Some((column, row));
        }
    }

    /// Extends the active selection, scrolling the feed when the drag reaches its edge.
    pub fn extend_selection(&mut self, column: u16, row: u16) {
        if self.selection.is_none() {
            return;
        }
        self.drag_point = Some((column, row));
        if let Some(delta) = self.selection_scroll_delta(row) {
            self.scroll_chat(delta);
        }
        if let Some(point) = self.chat_point(column, row) {
            if let Some(selection) = self.selection.as_mut() {
                selection.cursor = point;
            }
        }
    }

    /// Copies the active selection to the system clipboard and drops the highlight.
    ///
    /// Releasing the button ends the drag the way a terminal ends its own selection. The app has
    /// to do it because it is the one consuming the drag: with mouse reporting on, nothing above
    /// it ever sees the gesture, so nothing above it can clear a highlight it did not paint. The
    /// copy itself stays silent.
    pub fn commit_selection(&mut self) {
        self.commit_selection_with_click(0, 0, None, None);
    }

    /// Commits active selection. If the gesture is a clean, intentional point click directly
    /// on a URL (and not an accidental window-focus click or drag), opens the link in the background.
    /// Otherwise, if text was selected, copies it to the system clipboard silently.
    pub fn commit_selection_with_click(
        &mut self,
        up_col: u16,
        up_row: u16,
        down_time: Option<Instant>,
        down_pos: Option<(u16, u16)>,
    ) {
        let Some(selection) = self.selection else { return };

        // 1. Is this a deliberate point click? (Human tap: between 30ms and 500ms, max 1 cell jitter)
        let is_point_click = if let (Some(t), Some((dc, dr))) = (down_time, down_pos) {
            let elapsed = t.elapsed();
            let duration_ok = elapsed >= Duration::from_millis(30) && elapsed <= Duration::from_millis(500);
            let distance_ok = dc.abs_diff(up_col) <= 1 && dr == up_row;
            let selection_empty = selection.is_empty()
                || (selection.anchor.0 == selection.cursor.0 && selection.anchor.1.abs_diff(selection.cursor.1) <= 1);
            duration_ok && distance_ok && selection_empty
        } else {
            false
        };

        // 2. Window Focus Activation Guard:
        // Window managers (Hyprland, Wayland, X11) pass click-through when focusing an inactive window.
        // A click within 400ms of FocusGained is a window activation click, NOT a deliberate URL click!
        let is_focus_activation = self.last_focus_gained
            .map(|t| t.elapsed() < Duration::from_millis(400))
            .unwrap_or(false);

        // 3. App startup grace period (first 800ms of app lifecycle):
        let is_startup_grace = self.app_start_time.elapsed() < Duration::from_millis(800);

        if is_point_click && !is_focus_activation && !is_startup_grace {
            let (line_idx, char_col) = selection.anchor;
            if let Some(line) = self.selection_lines.get(line_idx) {
                if let Some(url) = formatting::find_url_in_line(line, Some(char_col as usize)) {
                    // 4. Strict Debounce:
                    // Minimum 1.5s between ANY URL open, 3.0s between opening the exact same URL.
                    let already_opened_recently = self.last_url_open
                        .as_ref()
                        .map(|(t, prev_url)| {
                            t.elapsed() < Duration::from_millis(1500)
                                || (prev_url == &url && t.elapsed() < Duration::from_millis(3000))
                        })
                        .unwrap_or(false);

                    if !already_opened_recently {
                        self.last_url_open = Some((Instant::now(), url.clone()));
                        open_url_detached(&url);
                    }

                    self.clear_selection();
                    return;
                }
            }
        }

        let text = self.selection_text();

        self.clear_selection();

        if selection.is_empty() || text.trim().is_empty() {
            return;
        }

        if let Err(err) = clipboard::copy(&text) {
            self.session.add_message(Message::system(format!("Clipboard unavailable: {err}")));
        }
    }

    /// Drops the current selection and drag point.
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.drag_point = None;
    }

    /// True when the cell sits inside the chat viewport.
    fn is_over_chat(&self, column: u16, row: u16) -> bool {
        self.last_chat_rect.is_some_and(|rect| {
            column >= rect.x
                && column < rect.x.saturating_add(rect.width)
                && row >= rect.y
                && row < rect.y.saturating_add(rect.height)
        })
    }

    /// Lines a drag held at the edge of the feed scrolls it: negative upwards, positive downwards,
    /// `None` while the pointer stays inside the viewport.
    ///
    /// The feed is the first area on screen, so it starts at row 0 and the pointer can never sit
    /// above it: reaching its top row is the only way to ask for more content upwards, which is
    /// what a terminal does when the pointer hits the top of the window. Downwards the composer
    /// sits below the feed, so leaving the viewport stays the signal.
    fn selection_scroll_delta(&self, row: u16) -> Option<i32> {
        let rect = self.last_chat_rect?;
        if row <= rect.y {
            return Some(-1);
        }
        if row >= rect.y.saturating_add(rect.height) {
            return Some(1);
        }
        None
    }

    /// Maps a screen cell onto `(rendered line index, display column)`.
    ///
    /// Cells outside the viewport clamp to its edge, which is what lets a drag continue past the
    /// top or bottom without jumping to another line.
    fn chat_point(&self, column: u16, row: u16) -> Option<(usize, u16)> {
        let rect = self.last_chat_rect?;
        if rect.width == 0 || rect.height == 0 {
            return None;
        }
        let row_in_view = row.saturating_sub(rect.y).min(rect.height - 1);
        let col_in_view = column.saturating_sub(rect.x).min(rect.width - 1);
        Some((self.scroll_offset as usize + row_in_view as usize, col_in_view))
    }

    /// Text covered by the active selection, one entry per rendered line.
    pub fn selection_text(&self) -> String {
        let Some(selection) = self.selection else {
            return String::new();
        };
        let ((start_line, start_col), (end_line, end_col)) = selection.ordered();
        let mut out = String::new();

        for line_idx in start_line..=end_line {
            let Some(line) = self.selection_lines.get(line_idx) else {
                continue;
            };
            if line_idx > start_line {
                out.push('\n');
            }
            let from = if line_idx == start_line { start_col } else { 0 };
            let to = if line_idx == end_line { end_col } else { u16::MAX };
            let (first_char, last_char) = char_range_for_display_columns(line, from, to);
            out.extend(line.chars().skip(first_char).take(last_char.saturating_sub(first_char)));
        }
        out
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

    /// Slash-command candidates for the current prompt.
    ///
    /// Returns an empty list unless the menu is genuinely active: the prompt must
    /// start with `/` AND the user must not be browsing prompt history (Up/Down).
    /// Recalling an entry such as `/resume` from history fills the buffer with an
    /// exact command name, which would otherwise hijack the arrows for autocomplete
    /// navigation and trap the user on a single-item list.
    pub fn matching_slash_commands(&self) -> Vec<&'static str> {
        if self.history_idx.is_some() || !self.input_buffer.starts_with('/') {
            return Vec::new();
        }

        crate::slash_commands::match_commands(&self.input_buffer)
            .into_iter()
            .map(|c| c.name)
            .collect()
    }

    /// True when the slash-completion menu should own the arrow keys.
    pub fn is_slash_menu_active(&self) -> bool {
        !self.matching_slash_commands().is_empty()
    }

    pub fn slash_popup_target_height(&self) -> f32 {
        let is_modal_open = self.sudo_dialog.is_open
            || self.model_dialog.is_open
            || self.auth_dialog.is_open
            || self.session_dialog.is_open
            || self.user_dialog.is_open;

        if is_modal_open {
            return 0.0;
        }

        let count = self.matching_slash_commands().len();
        if count > 0 {
            (count as f32 + 2.0).min(10.0)
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
        self.cached_message_line_offsets.clear();
        self.cached_message_count = 0;
        self.cached_render_width = 0;
        self.cached_session_id.clear();
        self.cached_logs_expanded = !self.logs_expanded;
    }

    /// Height (in rendered lines) of the chat feed as of the last drawn frame.
    fn chat_visible_height(&self) -> usize {
        self.last_chat_rect.map(|r| r.height as usize).unwrap_or(0)
    }

    /// Highest valid scroll offset for the chat feed, in rendered lines.
    pub fn max_scroll_offset(&self) -> u16 {
        self.total_rendered_items
            .saturating_sub(self.chat_visible_height()) as u16
    }

    /// Scrolls the chat feed by `delta` rendered lines (negative scrolls up).
    ///
    /// Any scroll up disengages auto-scroll so new streamed content no longer drags the
    /// viewport down; landing on the last line re-engages it. This is the single place
    /// that mutates `scroll_offset`/`auto_scroll`, so wheel, PageUp/PageDown and keyboard
    /// scrolling always agree on the clamp.
    pub fn scroll_chat(&mut self, delta: i32) {
        let max = self.max_scroll_offset();
        let current = self.scroll_offset.min(max) as i32;
        let next = (current + delta).clamp(0, max as i32) as u16;
        self.scroll_offset = next;
        self.auto_scroll = next >= max;
    }

    /// Jumps the chat feed straight to the latest message and re-engages auto-scrolling.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.max_scroll_offset();
        self.auto_scroll = true;
    }

    pub fn get_effective_allowed_commands(&self) -> Vec<String> {
        let mut all = self.llm_client.get_config().allowed_commands.clone();
        for cmd in &self.session_allowed_commands {
            if !all.contains(cmd) {
                all.push(cmd.clone());
            }
        }
        all
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
        self.tool_registry.unregister_prefix("mcp_");
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
        } else if self.fleet_dialog.is_open && self.fleet_dialog.command_mode {
            if let Some(ref pty) = self.fleet_dialog.pty_session {
                pty.write_bytes(pasted.as_bytes());
            } else {
                self.fleet_dialog.command_input.push_str(&pasted);
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
        self.streaming_extra_content = None;
        self.thinking_state.reset();
        self.thinking_state.is_streaming = true;
        self.is_streaming = true;
        self.active_status = Some("Generando...".to_string());
        // Preserve user scroll: only follow stream if user has not scrolled up to read
        if self.auto_scroll {
            self.auto_scroll = true;
        }
        self.last_turn_start = Some(Instant::now());

        let prompt_builder = PromptBuilder::new(&self.workspace_dir)
            .with_sudo_password(get_sudo_password().is_some())
            .with_plan_mode(self.plan_mode);

        let llm_cfg = self.llm_client.get_config();
        let is_local = llm_cfg.local_llm_enabled || llm_cfg.model.starts_with("local");
        let mut system_prompt = if self.fleet_dialog.is_open {
            let active_node = self.fleet_dialog.active_session.as_ref()
                .filter(|s| s.is_connected)
                .map(|s| (s.node.id.as_str(), s.node.user.as_str(), s.node.host.as_str()))
                .or_else(|| {
                    self.fleet_dialog.pty_session.as_ref()
                        .filter(|p| p.is_alive())
                        .and_then(|p| {
                            self.fleet_dialog.items.iter()
                                .find(|it| it.config.id == p.node_id)
                                .map(|it| (it.config.id.as_str(), it.config.user.as_str(), it.config.host.as_str()))
                        })
                });
            let available_nodes: Vec<(&str, &str, &str)> = self.fleet_dialog.items.iter()
                .map(|it| (it.config.id.as_str(), it.config.user.as_str(), it.config.host.as_str()))
                .collect();
            prompt_builder.build_fleet(active_node, &available_nodes)
        } else if is_local && llm_cfg.local_prompt_lite {
            prompt_builder.build_lite()
        } else {
            prompt_builder.build()
        };

        if let Some(ref pfx) = self.response_prefix {
            system_prompt.push_str(&format!(
                "\n\n[RESPONSE PREFIX REQUIREMENT]: You MUST begin your final text response immediately with the exact prefix: {}",
                pfx
            ));
        }

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
                self.clear_selection();
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
                let cfg = self.llm_client.get_config();
                let mut msg = format!(
                    "Session Usage Metrics:\n- Prompt Tokens: {}\n- Cached Prompt Tokens: {} ({:.1}% KV Cache Hit)\n- Completion Tokens: {}\n- Total Tokens: {}",
                    u.prompt_tokens, u.prompt_cache_hit_tokens, ratio, u.completion_tokens, u.total_tokens
                );
                if cfg.is_deepseek_endpoint() && !cfg.local_llm_enabled && u.total_tokens > 0 {
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
                    msg.push_str(&format!("\n- Estimated DeepSeek Cost: ${:.6} USD (Saved ${:.6} with KV Cache)", actual_cost, saved));
                }
                self.session.add_message(Message::system(msg));
                true
            }
            "/local" => {
                if parts.len() > 1 && parts[1] == "status" {
                    let cfg = self.llm_client.get_config();
                    let local = self.llm_client.local_client();
                    let is_up = local.health_check().await;
                    let msg = format!(
                        "Local LLM Status:\n- Enabled: {}\n- Endpoint: {}\n- Model: {}\n- Server Reachable: {}",
                        cfg.local_llm_enabled,
                        cfg.local_llm_url,
                        cfg.local_llm_model,
                        if is_up { "YES (Active)" } else { "NO (Offline)" }
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
            "/web" | "/search" => {
                if parts.len() > 1 {
                    // Handled by streaming chat loop so the AI invokes web_search and synthesizes the answer
                    false
                } else {
                    self.session.add_message(Message::system("Usage: /web <query> (Search the web with DeepSeek and synthesize the answer)"));
                    true
                }
            }
            "/fetch" => {
                if parts.len() > 1 {
                    // Handled by streaming chat loop so the AI invokes web_fetch and summarizes the page
                    false
                } else {
                    self.session.add_message(Message::system("Usage: /fetch <url> (Fetch a webpage into Markdown using DeepSeek Fetch)"));
                    true
                }
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
                let is_fleet = self.fleet_dialog.is_open || self.normal_session_stash.is_some() || self.session.is_fleet();
                let subcmd = parts.get(1).copied().unwrap_or("list");
                match subcmd {
                    "list" => {
                        self.session_dialog.open(&self.workspace_dir.display().to_string(), is_fleet);
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
                                    corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                                    let title = loaded.title.clone();
                                    let model = loaded.model.clone();
                                    self.session = loaded;
                                    self.cached_session_id.clear();
                                    self.invalidate_message_cache();
                                    self.clear_selection();
                                    self.scroll_to_bottom();
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
                        self.cached_session_id.clear();
                        self.invalidate_message_cache();
                        self.clear_selection();
                        self.scroll_to_bottom();
                        corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                        self.session.add_message(Message::system(format!("Started new chat session (Model: {}).", cfg.model)));
                    }
                    _ => {
                        self.session.add_message(Message::system("Usage: /chat list | /chat save <tag> | /chat resume <tag/id> | /chat delete <tag/id> | /chat new"));
                    }
                }
                true
            }
            "/resume" => {
                let is_fleet = self.fleet_dialog.is_open || self.normal_session_stash.is_some() || self.session.is_fleet();
                if parts.len() > 1 {
                    let target = parts[1..].join(" ");
                    match Session::load_by_id_or_tag(&target) {
                        Ok(mut loaded) => {
                            let mut cfg = self.llm_client.get_config();
                            cfg.model = loaded.model.clone();
                            if let Some(t) = loaded.temperature {
                                cfg.temperature = t;
                            }
                            if let Some(ref r) = loaded.reasoning_effort {
                                cfg.reasoning_effort = r.clone();
                            }
                            self.llm_client.update_config(cfg);
                            corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                            let title = loaded.title.clone();
                            let model = loaded.model.clone();
                            if is_fleet {
                                loaded.tag = Some("fleet".to_string());
                            }
                            self.session = loaded;
                            self.cached_session_id.clear();
                            self.invalidate_message_cache();
                            self.clear_selection();
                            self.scroll_to_bottom();
                            let label = if is_fleet { "Fleet session" } else { "session" };
                            self.session.add_message(Message::system(format!("Resumed {} '{}' (Model: {}).", label, title, model)));
                        }
                        Err(e) => {
                            self.session.add_message(Message::system(format!("Error: {}", e)));
                        }
                    }
                } else {
                    self.slash_popup_height_current = 0.0;
                    self.session_dialog.open(&self.workspace_dir.display().to_string(), is_fleet);
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
                self.cached_session_id.clear();
                self.invalidate_message_cache();
                self.clear_selection();
                self.scroll_to_bottom();
                corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
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
                    }
                    let _ = cfg.save_with_workspace(Some(&self.workspace_dir));
                    self.llm_client.update_config(cfg.clone());
                    corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                    self.session.model = new_model.to_string();
                    self.session.temperature = Some(cfg.temperature);
                    self.session.reasoning_effort = Some(cfg.reasoning_effort);
                    let _ = self.session.save();
                    self.session.add_message(Message::system(format!("Switched and saved active model to '{}'.", new_model)));
                } else {
                    let cfg = self.llm_client.get_config();
                    self.slash_popup_height_current = 0.0;
                    self.model_dialog.open(&cfg);
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
                let cfg = self.llm_client.get_config();
                let prov = cfg.active_provider.clone().unwrap_or_else(|| "deepseek".to_string());
                let p_opt = cfg.all_providers().into_iter().find(|p| p.name == prov);
                let label = p_opt.as_ref().map(|p| p.display_name()).unwrap_or("DeepSeek").to_string();
                let env_var = p_opt.as_ref().map(|p| p.api_key_env.as_str()).unwrap_or("DEEPSEEK_API_KEY").to_string();

                if parts.len() > 1 {
                    let key_str = parts[1..].join(" ").trim().to_string();
                    let mut new_cfg = cfg.clone();
                    if prov == "deepseek" {
                        new_cfg.api_key = key_str.clone();
                    } else {
                        new_cfg.provider_api_keys.insert(prov.clone(), key_str.clone());
                    }
                    let _ = new_cfg.save_with_workspace(Some(&self.workspace_dir));
                    self.llm_client.update_config(new_cfg);
                    corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                    if let Some(pos) = self.model_dialog.providers.iter().position(|p| p.name == prov) {
                        if pos < self.model_dialog.ready.len() {
                            self.model_dialog.ready[pos] = true;
                        }
                    }
                    self.session.add_message(Message::system(format!(
                        "API key updated and saved for {} (${})!",
                        label, env_var
                    )));
                } else {
                    let portal = self.model_dialog.providers.iter()
                        .find(|p| p.name == prov)
                        .map(|p| p.portal_url())
                        .unwrap_or("https://platform.deepseek.com");
                    self.auth_dialog.open_for_provider(None, &prov, &label, &env_var, portal);
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
                let apply_update = parts.get(1).map(|s| *s == "apply" || *s == "--apply" || *s == "force" || *s == "--force").unwrap_or(false);
                let force = parts.get(1).map(|s| *s == "force" || *s == "--force").unwrap_or(false);

                if apply_update {
                    self.session.add_message(Message::system("Checking release and applying in-place update..."));
                    match corex_core::update::perform_self_update(current, force, |_| {}).await {
                        Ok(summary) => {
                            self.session.add_message(Message::system(format!(
                                "✓ Corex successfully updated to v{} (previous: v{})!\nBinary: {}\n\nPlease restart Corex to run the updated version.",
                                summary.updated_version, summary.previous_version, summary.binary_path.display()
                            )));
                        }
                        Err(e) => {
                            self.session.add_message(Message::system(format!(
                                "✕ In-place update: {}\n\nManual update alternatives:\n• cx update (in terminal)\n• npm install -g @sluisr/corex\n• cargo install --git https://github.com/sluisr/corex.git --force",
                                e
                            )));
                        }
                    }
                    return true;
                }

                let maybe_newer = self.update_available.lock().ok().and_then(|l| l.clone());
                let msg = if let Some(newer) = maybe_newer {
                    format!(
                        "A new version of Corex is available: v{} → v{}\n\n\
                        To update automatically in-place, run:\n\
                        • /update apply (or cx update in your terminal)\n\n\
                        Manual alternatives:\n\
                        • Via npm:       npm install -g @sluisr/corex\n\
                        • From source:   cargo install --git https://github.com/sluisr/corex.git --force\n\
                        • Releases:      https://github.com/sluisr/corex/releases/latest",
                        current, newer
                    )
                } else {
                    format!(
                        "Corex is up to date (v{}).\n\n\
                        To force re-download the latest release in-place:\n\
                        • /update force\n\n\
                        Manual update alternatives:\n\
                        • npm install -g @sluisr/corex\n\
                        • cargo install --git https://github.com/sluisr/corex.git --force\n\
                        • https://github.com/sluisr/corex/releases",
                        current
                    )
                };
                self.session.add_message(Message::system(msg));
                true
            }
            "/mouse" => {
                self.toggle_mouse_capture();
                true
            }
            "/keyboard" => {
                try_show_termux_soft_keyboard();
                self.session.add_message(Message::system("Virtual keyboard requested (Termux tap-to-type active on mobile vertical screens)."));
                true
            }
            "/prefix" => {
                if parts.len() > 1 {
                    let prefix_text = parts[1..].join(" ");
                    let trimmed = prefix_text.trim();
                    if matches!(trimmed.to_ascii_lowercase().as_str(), "off" | "clear" | "none") {
                        self.response_prefix = None;
                        self.session.add_message(Message::system("Response formatting prefix cleared."));
                    } else {
                        self.response_prefix = Some(prefix_text.clone());
                        self.session.add_message(Message::system(format!(
                            "Response formatting prefix configured: '{}'. DeepSeek will direct its next output with this prefix.",
                            prefix_text
                        )));
                    }
                } else {
                    self.session.add_message(Message::system("Usage: /prefix <text> (e.g. /prefix Output: or /prefix ```json, or /prefix off to clear)"));
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

                    if !corex_tools::fs_tools::is_within_workspace(&file_path, &self.workspace_dir)
                        || corex_tools::fs_tools::is_sensitive_system_path(&file_path)
                    {
                        self.session.add_message(Message::system(format!(
                            "Refused to load '{}': path is outside the active workspace or points to a sensitive file.",
                            file_arg
                        )));
                        return true;
                    }

                    const FIM_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
                    const FIM_WINDOW_BYTES: usize = 24_000;

                    let file_size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
                    if !file_path.exists() {
                        self.session.add_message(Message::system(format!("File '{}' not found.", file_arg)));
                    } else if file_size > FIM_MAX_FILE_BYTES {
                        self.session.add_message(Message::system(format!(
                            "File '{}' is too large for FIM ({} bytes, max {}).",
                            file_arg, file_size, FIM_MAX_FILE_BYTES
                        )));
                    } else {
                        match std::fs::read_to_string(&file_path) {
                            Ok(content) => {
                                if let Some(hole_idx) = content.find("<FIM_HOLE>") {
                                    let snippet = fim_context_window(&content, hole_idx, FIM_WINDOW_BYTES);
                                    let prompt = format!(
                                        "Please perform Fill-in-the-Middle (FIM) code completion for `{}`. Complete the exact code that belongs strictly inside `<FIM_HOLE>`:\n\n```\n{}\n```",
                                        file_arg,
                                        snippet
                                    );
                                    self.session.add_message(Message::system("FIM prompt loaded. Streaming completion..."));
                                    self.session.add_message(Message::user(prompt));
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
                let sub = parts.get(1).map(|s| s.to_lowercase()).unwrap_or_else(|| "status".to_string());
                match sub.as_str() {
                    "catalog" | "available" => {
                        let catalog = corex_tools::get_mcp_catalog();
                        let mut msg = format!("MCP Server Catalog ({} vetted servers):\n\n", catalog.len());
                        for item in catalog {
                            msg.push_str(&format!("• **{}** (`{}`)\n", item.name, item.id));
                            msg.push_str(&format!("  *{}*\n", item.description));
                            msg.push_str(&format!("  Command: `{} {}`\n", item.command, item.default_args.join(" ")));
                            if !item.env_keys.is_empty() {
                                msg.push_str(&format!("  Required Env: `{}`\n", item.env_keys.join("`, `")));
                            }
                            msg.push_str(&format!("  Quick Add: `{}`\n\n", item.example));
                        }
                        msg.push_str("Tip: Add any server with `/mcp add <id>` (e.g. `/mcp add github` or `/mcp add sqlite app.db`)");
                        self.session.add_message(Message::system(msg));
                    }
                    "add" => {
                        if parts.len() < 3 && parts.get(1).map(|&p| p == "add").unwrap_or(false) && parts.len() < 3 {
                            let help = "Usage:\n  /mcp add <catalog-id> [args...]\n  /mcp add <custom-name> <command> [args...]\n\nExamples:\n  /mcp add github\n  /mcp add sqlite app.db\n  /mcp add filesystem .\n  /mcp add mytool python3 /path/server.py\n\nRun `/mcp catalog` to browse all vetted community templates.";
                            self.session.add_message(Message::system(help));
                        } else {
                            let target_name = parts[2];
                            let mut cfg = self.llm_client.get_config();

                            if let Some(cat) = corex_tools::find_catalog_item(target_name) {
                                let mut args: Vec<String> = cat.default_args.iter().map(|s| s.to_string()).collect();
                                if parts.len() > 3 {
                                    if cat.id == "sqlite" {
                                        args = vec!["-y".into(), "@modelcontextprotocol/server-sqlite".into(), "--db-path".into(), parts[3].to_string()];
                                    } else if cat.id == "filesystem" {
                                        args = vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into(), parts[3].to_string()];
                                    } else if cat.id == "postgres" {
                                        args = vec!["-y".into(), "@modelcontextprotocol/server-postgres".into(), parts[3].to_string()];
                                    } else {
                                        args.extend(parts[3..].iter().map(|s| s.to_string()));
                                    }
                                }
                                let mcp_cfg = corex_core::McpServerConfig {
                                    command: cat.command.to_string(),
                                    args,
                                    env: std::collections::HashMap::new(),
                                };
                                let server_name = cat.id.to_string();
                                if let Err(e) = cfg.add_mcp_server(server_name.clone(), mcp_cfg, Some(&self.workspace_dir)) {
                                    self.session.add_message(Message::system(format!("Failed to save MCP config: {}", e)));
                                } else {
                                    self.llm_client.update_config(cfg);
                                    let statuses = self.reload_mcp_servers().await;
                                    let mut msg = format!("✓ Added MCP server '{}' from catalog.\n", cat.name);
                                    if !cat.env_keys.is_empty() {
                                        msg.push_str(&format!("Environment variables to set if required:\n  {}\n\n", cat.env_keys.iter().map(|k| format!("export {}=\"...\"", k)).collect::<Vec<_>>().join("\n  ")));
                                    }
                                    if let Some(s) = statuses.iter().find(|s| s.name == server_name) {
                                        let icon = if s.is_connected { "[Connected]" } else { "[Offline/Pending]" };
                                        msg.push_str(&format!("Status: {} — {} tools discovered", icon, s.tools_count));
                                        if let Some(ref err) = s.error {
                                            msg.push_str(&format!("\nWarning: {}", err));
                                        }
                                    }
                                    self.session.add_message(Message::system(msg));
                                }
                            } else if parts.len() >= 4 {
                                let server_name = target_name.to_string();
                                let command = parts[3].to_string();
                                let args = parts[4..].iter().map(|s| s.to_string()).collect();
                                let mcp_cfg = corex_core::McpServerConfig {
                                    command,
                                    args,
                                    env: std::collections::HashMap::new(),
                                };
                                if let Err(e) = cfg.add_mcp_server(server_name.clone(), mcp_cfg, Some(&self.workspace_dir)) {
                                    self.session.add_message(Message::system(format!("Failed to save MCP config: {}", e)));
                                } else {
                                    self.llm_client.update_config(cfg);
                                    let statuses = self.reload_mcp_servers().await;
                                    let mut msg = format!("✓ Added custom MCP server '{}'.\n", server_name);
                                    if let Some(s) = statuses.iter().find(|s| s.name == server_name) {
                                        let icon = if s.is_connected { "[Connected]" } else { "[Offline/Pending]" };
                                        msg.push_str(&format!("Status: {} — {} tools discovered", icon, s.tools_count));
                                        if let Some(ref err) = s.error {
                                            msg.push_str(&format!("\nError: {}", err));
                                        }
                                    }
                                    self.session.add_message(Message::system(msg));
                                }
                            } else {
                                self.session.add_message(Message::system(format!(
                                    "Unknown catalog template '{}'.\nFor custom commands, provide command and args:\n  /mcp add {} <command> [args...]\nOr browse available templates with `/mcp catalog`.",
                                    target_name, target_name
                                )));
                            }
                        }
                    }
                    "remove" | "delete" => {
                        if parts.len() < 3 {
                            self.session.add_message(Message::system("Usage: /mcp remove <name>"));
                        } else {
                            let name = parts[2];
                            let mut cfg = self.llm_client.get_config();
                            match cfg.remove_mcp_server(name, Some(&self.workspace_dir)) {
                                Ok(true) => {
                                    self.llm_client.update_config(cfg);
                                    let _ = self.reload_mcp_servers().await;
                                    self.session.add_message(Message::system(format!("✓ Removed MCP server '{}' and reloaded tools.", name)));
                                }
                                Ok(false) => {
                                    self.session.add_message(Message::system(format!("MCP server '{}' was not found in config.", name)));
                                }
                                Err(e) => {
                                    self.session.add_message(Message::system(format!("Failed to update MCP config: {}", e)));
                                }
                            }
                        }
                    }
                    "reload" => {
                        let statuses = self.reload_mcp_servers().await;
                        let mut msg = format!("Reloaded MCP Servers ({} configured):\n", statuses.len());
                        if statuses.is_empty() {
                            msg.push_str("  No servers configured. Type `/mcp catalog` to see available servers or `/mcp add <id>`.\n");
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
                            let help = "No MCP (Model Context Protocol) servers configured.\n\n\
                                Quick start:\n\
                                • Browse templates:  /mcp catalog\n\
                                • Add GitHub MCP:    /mcp add github\n\
                                • Add SQLite MCP:    /mcp add sqlite app.db\n\
                                • Add Filesystem:    /mcp add filesystem .\n\
                                • Add custom:        /mcp add <name> <command> [args...]\n\n\
                                All servers are persisted in ~/.corex/settings.json or .corex/settings.json.";
                            self.session.add_message(Message::system(help));
                        } else {
                            let mut msg = format!("Configured MCP Servers ({}):\n", cfg.mcp_servers.len());
                            for (name, scfg) in &cfg.mcp_servers {
                                msg.push_str(&format!("• {}: {} {}\n", name, scfg.command, scfg.args.join(" ")));
                            }
                            msg.push_str("\nCommands:\n  /mcp catalog        - Browse available templates\n  /mcp add <name>     - Add new MCP server\n  /mcp remove <name>  - Remove an MCP server\n  /mcp reload         - Reload and reconnect servers");
                            self.session.add_message(Message::system(msg));
                        }
                    }
                }
                true
            }
            "/tasks" | "/background" => {
                let mgr = corex_tools::background::get_task_manager();
                if parts.len() == 1 {
                    let guard = match mgr.lock() {
                        Ok(g) => g,
                        Err(e) => e.into_inner(),
                    };
                    let list = guard.list();
                    self.session.add_message(Message::system(format!("Background Tasks:\n\n{}", list)));
                } else {
                    let sub = parts[1].to_lowercase();
                    match sub.as_str() {
                        "list" => {
                            let guard = match mgr.lock() {
                                Ok(g) => g,
                                Err(e) => e.into_inner(),
                            };
                            let list = guard.list();
                            self.session.add_message(Message::system(format!("Background Tasks:\n\n{}", list)));
                        }
                        "status" | "log" | "output" => {
                            if parts.len() > 2 {
                                if let Ok(pid) = parts[2].parse::<u32>() {
                                    let guard = match mgr.lock() {
                                        Ok(g) => g,
                                        Err(e) => e.into_inner(),
                                    };
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
                                    let mut guard = match mgr.lock() {
                                        Ok(g) => g,
                                        Err(e) => e.into_inner(),
                                    };
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
                                    let guard = match mgr.lock() {
                                        Ok(g) => g,
                                        Err(e) => e.into_inner(),
                                    };
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
                                let guard = match mgr.lock() {
                                    Ok(g) => g,
                                    Err(e) => e.into_inner(),
                                };
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

            "/fleet" | "/sysadmin" | "/vps" => {
                let cfg = corex_fleet::FleetConfig::load(Some(&self.workspace_dir)).unwrap_or_default();
                let nodes = cfg.all_nodes();
                if parts.len() > 1 && parts[1] == "wide" {
                    self.fleet_dialog.wide_layout = !self.fleet_dialog.wide_layout;
                    if !self.fleet_dialog.is_open {
                        self.enter_fleet_mode();
                        self.fleet_dialog.open_async(&self.workspace_dir).await;
                    }
                } else if parts.len() > 1 && matches!(parts[1], "off" | "close" | "exit" | "quit") {
                    self.exit_fleet_mode();
                } else if parts.len() > 1 && (parts[1] == "resume" || parts[1] == "sessions" || parts[1] == "history") {
                    self.enter_fleet_mode();
                    self.session_dialog.open(&self.workspace_dir.display().to_string(), true);
                } else if parts.len() == 1 || (parts.len() > 1 && (parts[1] == "nodes" || parts[1] == "list" || parts[1] == "dashboard" || parts[1] == "console")) {
                    self.enter_fleet_mode();
                    self.fleet_dialog.open_async(&self.workspace_dir).await;
                } else if parts.len() > 2 && (parts[1] == "connect" || parts[1] == "select") {
                    let target_id = parts[2];
                    self.enter_fleet_mode();
                    self.fleet_dialog.open_async(&self.workspace_dir).await;
                    if let Some(pos) = self.fleet_dialog.items.iter().position(|it| it.config.id == target_id) {
                        self.fleet_dialog.selected_idx = pos;
                        let node = self.fleet_dialog.items[pos].config.clone();
                        let mut session = corex_fleet::RemoteSession::new(node.clone());
                        match session.connect(8).await {
                            Ok(lat) => {
                                corex_fleet::global_session_manager().set_session(node.id.clone(), session.clone()).await;
                                corex_fleet::global_session_manager().set_active_node(Some(node.id.clone())).await;
                                self.fleet_dialog.active_session = Some(session);
                                if let Ok(pty) = crate::fleet_pty::FleetPtySession::spawn(&node, 24, 80) {
                                    self.fleet_dialog.pty_session = Some(pty);
                                }
                                self.fleet_dialog.status_msg = Some(format!("Connected to {} in {}ms", node.id, lat));
                                self.session.add_message(Message::system(format!(
                                    "Connected to remote server: {} ({}@{}) · Active and ready.",
                                    node.id, node.user, node.host
                                )));
                                self.scroll_to_bottom();
                            }
                            Err(e) => {
                                self.fleet_dialog.status_msg = Some(format!("Could not connect to {}: {}", node.id, e));
                            }
                        }
                    } else {
                        self.fleet_dialog.status_msg = Some(format!("Server '{}' not found in inventory", target_id));
                    }
                } else if parts.len() > 1 && parts[1] == "status" {
                    let mut msg = format!("🌐 Fleet Nodes ({} servers):\n", nodes.len());
                    for n in &nodes {
                        msg.push_str(&format!("• [{}] {}\n", n.id, n.target_string()));
                    }
                    msg.push_str("\n(For full real-time telemetry run `cx fleet telemetry` or ask the assistant: 'check server status')");
                    self.session.add_message(Message::system(msg));
                } else {
                    self.enter_fleet_mode();
                    self.fleet_dialog.open_async(&self.workspace_dir).await;
                }
                true
            }
            "/help" => {
                let mut help = "Available Commands:\n".to_string();
                let mut last_group = "";
                for c in ALL_COMMANDS {
                    if c.group != last_group {
                        last_group = c.group;
                        help.push_str(&format!("\n {}\n", c.group));
                    }
                    let usage = if c.args.is_empty() {
                        c.name.to_string()
                    } else {
                        format!("{} {}", c.name, c.args)
                    };
                    let alias = if c.aliases.is_empty() {
                        String::new()
                    } else {
                        format!(" (alias {})", c.aliases.join(", "))
                    };
                    help.push_str(&format!("  {:<30} {}{}\n", usage, c.description, alias));
                }
                help.push_str("  /key <sk..>  Set or update your DeepSeek API key\n");
                help.push_str("  /sudo <pwd>  Store sudo password in RAM for silent privilege escalation\n");
                help.push_str("\nKeyboard Shortcuts:\n  Enter: Submit | Ctrl+T: Toggle Thought Box | Ctrl+C: Cancel/Exit | Up/Down: History / Command Nav");
                help.push_str("\n  Scroll chat: Mouse wheel | PageUp / PageDown | Shift+Up / Shift+Down");
                help.push_str("\n  Copy text: drag to select, release to copy and clear the highlight");
                self.session.add_message(Message::system(help));
                true
            }
            _ => false,
        }
    }
}


impl App {
    /// Applies what the model dialog asked for after a key press.
    pub fn apply_model_dialog_action(&mut self, action: DialogAction) {
        match action {
            DialogAction::None => {}
            DialogAction::Close => self.model_dialog.close(),
            DialogAction::SettingsChanged => self.apply_dialog_settings(),
            DialogAction::Activate { provider, model } => self.activate_model(provider, model),
        }
    }

    /// Pushes the dialog's tunables into the running config (and disk when persistent).
    fn apply_dialog_settings(&mut self) {
        let d = &self.model_dialog;
        let flash = d.to_flash_settings();
        let pro = d.to_pro_settings();
        let persist = d.persist;
        let is_active = d.selection_is_active();
        let family = d.selected_model().map(|m| m.family);
        let (temp, reasoning) = d.effective_params();

        let mut cfg = self.llm_client.get_config();
        cfg.flash_settings = flash.clone();
        cfg.pro_settings = pro.clone();
        if is_active {
            match family {
                Some(corex_core::providers::ModelFamily::Flash) => {
                    cfg.temperature = flash.temperature;
                    cfg.reasoning_effort = flash.reasoning_effort.clone();
                }
                Some(corex_core::providers::ModelFamily::Pro) => {
                    cfg.reasoning_effort = pro.reasoning_effort.clone();
                }
                _ => {
                    cfg.temperature = temp;
                    if !reasoning.is_empty() {
                        cfg.reasoning_effort = reasoning;
                    }
                }
            }
        }
        if persist {
            let _ = Config::save_flash_settings(&flash);
            let _ = Config::save_pro_settings(&pro);
            let _ = cfg.save();
        }
        self.llm_client.update_config(cfg);
    }

    /// Makes `provider`/`model` (dialog indices) the active model.
    pub fn activate_model(&mut self, provider_idx: usize, model_idx: usize) {
        let Some((provider, profile)) = self
            .model_dialog
            .providers
            .get(provider_idx)
            .and_then(|p| p.models.get(model_idx).map(|m| (p.clone(), m.clone())))
        else {
            return;
        };

        let cfg = self.llm_client.get_config();
        let is_ready = self.model_dialog.ready.get(provider_idx).copied().unwrap_or(false)
            || cfg.provider_api_keys.get(&provider.name).map(|k| !k.trim().is_empty()).unwrap_or(false)
            || provider.is_ready();

        if !is_ready {
            let portal = provider.portal_url();
            self.model_dialog.close();
            self.auth_dialog.open_for_provider(
                Some((provider_idx, model_idx)),
                &provider.name,
                provider.display_name(),
                &provider.api_key_env,
                portal,
            );
            return;
        }

        let d = &self.model_dialog;
        let flash = d.to_flash_settings();
        let pro = d.to_pro_settings();
        let generic_temp = d.generic_temperature;
        let generic_reasoning = d.generic_reasoning.clone();
        let persist = d.persist;

        use corex_core::providers::ModelFamily;
        let mut cfg = self.llm_client.get_config();
        cfg.model = profile.id.clone();
        cfg.active_provider = Some(provider.name.clone());
        cfg.local_llm_enabled = provider.is_local();
        let detail = if provider.is_local() {
            format!("- Endpoint: {}\n- Cost: $0.00 (Zero cloud telemetry)", cfg.local_llm_url)
        } else {
            match profile.family {
                ModelFamily::Flash => {
                    cfg.flash_settings = flash.clone();
                    cfg.temperature = flash.temperature;
                    cfg.reasoning_effort = flash.reasoning_effort.clone();
                    format!(
                        "- Temperature: {:.1}\n- General Reasoning: {}\n- Command CoT: {}\n- Code CoT: {}",
                        cfg.temperature,
                        cfg.reasoning_effort,
                        flash.command_reasoning_effort,
                        flash.code_reasoning_effort
                    )
                }
                ModelFamily::Pro => {
                    cfg.pro_settings = pro.clone();
                    cfg.reasoning_effort = pro.reasoning_effort.clone();
                    format!(
                        "- Reasoning Depth: {}\n- Search CoT: {}",
                        pro.reasoning_effort, pro.search_reasoning_effort
                    )
                }
                ModelFamily::Generic => {
                    cfg.temperature = generic_temp;
                    if !generic_reasoning.is_empty() {
                        cfg.reasoning_effort = generic_reasoning.clone();
                    }
                    format!("- Temperature: {:.1}\n- Endpoint: {}", generic_temp, provider.base_url)
                }
            }
        };

        if persist {
            let _ = cfg.save();
        }
        self.llm_client.update_config(cfg.clone());
        self.session.model = cfg.model.clone();
        self.session.temperature = Some(cfg.temperature);
        self.session.reasoning_effort = Some(cfg.reasoning_effort.clone());
        let _ = self.session.save();
        self.session.add_message(Message::system(format!(
            "Activated {} · {}\n{}",
            provider.display_name(),
            profile.display_name(),
            detail
        )));
        self.model_dialog.mark_active(provider_idx, model_idx);
        self.model_dialog.close();
    }

    /// Clears rendered message line cache to force fresh render of messages.
    pub fn clear_render_cache(&mut self) {
        self.invalidate_message_cache();
        self.selection_lines = SelectionLines::default();
    }

    /// Transitions into Fleet / SysAdmin mode with its own isolated conversation session.
    pub fn enter_fleet_mode(&mut self) {
        if self.normal_session_stash.is_none() {
            let _ = self.session.save();
            self.normal_session_stash = Some(self.session.clone());

            if let Some(fs) = self.fleet_session.take() {
                self.session = fs;
            } else {
                let mut new_fs = Session::new_with_params(
                    &self.session.model,
                    self.session.temperature.unwrap_or(0.7),
                    self.session.reasoning_effort.as_deref().unwrap_or("medium"),
                    Some(&self.workspace_dir),
                );
                new_fs.title = "SysAdmin Fleet Console".to_string();
                new_fs.tag = Some("fleet".to_string());
                new_fs.add_message(Message::system(
                    "SysAdmin Fleet Mode active. You are supervising and orchestrating remote servers via SSH (fleet tools). All commands and telemetry are isolated to this session."
                ));
                self.session = new_fs;
            }
            self.clear_render_cache();
            self.scroll_to_bottom();
        }
    }

    /// Exits Fleet / SysAdmin mode, saving the fleet session and restoring the normal conversation.
    pub fn exit_fleet_mode(&mut self) {
        if let Some(normal) = self.normal_session_stash.take() {
            let _ = self.session.save();
            self.fleet_session = Some(self.session.clone());
            self.session = normal;
            self.clear_render_cache();
            self.scroll_to_bottom();
        }
        self.fleet_dialog.close();
    }
}

/// Opens a URL in the user's default browser completely detached in the background.
/// Never blocks the event loop, never inherits terminal stdio, never interferes with raw mode.
pub fn open_url_detached(url: &str) {
    if cfg!(test) {
        return;
    }
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://") || trimmed.starts_with("mailto:")) {
        return;
    }
    let url_owned = trimmed.to_string();
    std::thread::spawn(move || {
        #[cfg(target_os = "linux")]
        let child = std::process::Command::new("xdg-open")
            .arg(&url_owned)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        #[cfg(target_os = "macos")]
        let child = std::process::Command::new("open")
            .arg(&url_owned)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        #[cfg(target_os = "windows")]
        let child = std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", &url_owned])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        if let Ok(mut c) = child {
            let _ = c.wait();
        }
    });
}

/// Returns at most `window` bytes before and after `hole_idx`, snapped to UTF-8 char and line
/// boundaries, with an elision marker when the file was clipped. Keeps /fim prompts bounded.
pub(crate) fn fim_context_window(content: &str, hole_idx: usize, window: usize) -> String {
    if content.len() <= window * 2 {
        return content.to_string();
    }

    let mut start = hole_idx.saturating_sub(window);
    while start > 0 && !content.is_char_boundary(start) {
        start -= 1;
    }
    if start > 0 {
        if let Some(nl) = content[start..hole_idx].find('\n') {
            start += nl + 1;
        }
    }

    let mut end = (hole_idx + window).min(content.len());
    while end < content.len() && !content.is_char_boundary(end) {
        end += 1;
    }
    if end < content.len() {
        if let Some(nl) = content[hole_idx..end].rfind('\n') {
            end = hole_idx + nl;
        }
    }

    let mut out = String::with_capacity(end - start + 64);
    if start > 0 {
        out.push_str("// ... (earlier lines omitted) ...\n");
    }
    out.push_str(&content[start..end]);
    if end < content.len() {
        out.push_str("\n// ... (later lines omitted) ...");
    }
    out
}

#[cfg(test)]
mod tests;
