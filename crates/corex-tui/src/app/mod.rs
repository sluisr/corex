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
use corex_core::types::{ContentPart, Message};
use corex_core::memory::{Memory, MemoryScope};
use corex_prompt::PromptBuilder;
use corex_tools::registry::ToolRegistry;

use crate::auth_dialog::AuthDialogState;
use crate::clipboard;
use crate::fleet_dialog::FleetDialogState;
use crate::model_dialog::{DialogAction, ModelDialogState};
use crate::session_dialog::SessionDialogState;
use crate::slash_commands::ALL_COMMANDS;
use crate::sudo_dialog::SudoDialogState;
use crate::text::group_thousands;
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
pub mod summary;
pub use summary::*;
pub mod render;
pub use render::*;
pub mod event_loop;
mod commands;
pub use event_loop::*;

const YOLO_ON_MESSAGE: &str = "YOLO on: tools run without asking, except shell commands that touch credentials, git hooks or shell startup files, and notes for the global memory.";
const YOLO_OFF_MESSAGE: &str = "YOLO off: anything that changes files or runs a command asks first.";

impl App {
    pub fn new(llm_client: LlmClient, workspace_dir: PathBuf, yolo: bool) -> Self {
        // Tests build many Apps and some save sessions: keep them out of the real ~/.corex.
        #[cfg(test)]
        crate::test_support::isolate_home();
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
            theme: Theme::detect(),
            workspace_dir,
            extra_dirs: Vec::new(),

            input_buffer: String::new(),
            input_history: persistent_history,
            history_idx: None,
            saved_draft: String::new(),
            slash_selected_idx: 0,
            pastes: std::collections::HashMap::new(),
            next_paste_id: 1,
            images: std::collections::HashMap::new(),
            next_image_id: 1,
            cursor_idx: 0,

            is_streaming: false,
            thinking_state: ThinkingState::new(),
            streaming_text: String::new(),
            streaming_tool_calls: Vec::new(),
            streaming_extra_content: None,

            pending_confirmation: None,
            user_dialog: UserDialogState::new(),
            rewind_dialog: Default::default(),
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
            tap_keyboard: TapKeyboardGate::default(),
            touch_arrows: TouchArrowClassifier::default(),
            selection: None,
            selection_lines: SelectionLines::default(),
            drag_point: None,
            copy_notice: None,
            inside_tmux: std::env::var_os("TMUX").is_some(),
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
            slash_popup_clock: None,
            last_slash_filter: String::new(),
            file_index: None,
            last_file_query: None,
            local_llm_online: is_online,
            last_local_check: None,
            cached_message_lines: Vec::new(),
            cached_message_line_offsets: Vec::new(),
            cached_message_count: 0,
            cached_render_width: 0,
            cached_session_id: String::new(),
            cached_logs_expanded: false,
            cached_has_collapsed_logs: false,
            streaming_md_cache: Vec::new(),
            streaming_md_cache_len: 0,
            streaming_md_cache_width: 0,
            active_background_pids: std::collections::HashSet::new(),
            pending_task_wakeup: false,
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
            steer_messages: Vec::new(),
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
        self.tap_keyboard.cancel();
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

    /// Responds to terminal resize events by clearing the rendering caches.
    pub fn handle_resize(&mut self, _width: u16, _height: u16) {
        self.cached_message_lines.clear();
        self.cached_message_line_offsets.clear();
        self.cached_render_width = 0;
        self.last_chat_rect = None;
    }

    // --- Chat text selection (driven by mouse drags, see `Selection`) ---

    /// Routes a mouse event: the wheel scrolls the feed, the left button selects text or opens links.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_selection(-wheel_lines()),
            MouseEventKind::ScrollDown => self.scroll_selection(wheel_lines()),
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
        let applied = self.scroll_offset as i64 - before as i64;
        if applied == 0 {
            return;
        }
        if let Some(selection) = self.selection.as_mut() {
            selection.cursor.0 = (selection.cursor.0 as i64 + applied).max(0) as usize;
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

    /// Copies the active selection to the system clipboard; a short notice in the prompt confirms
    /// it. The highlight stays until the next click or Esc, as in a plain terminal, except inside
    /// tmux, whose own selection ends on release.
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

        // Point click: duration between 30ms and 500ms, max 1 cell jitter.
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

        // Ignore clicks within 400ms of FocusGained (window manager focus click-through).
        let is_focus_activation = self.last_focus_gained
            .map(|t| t.elapsed() < Duration::from_millis(400))
            .unwrap_or(false);

        let is_startup_grace = self.app_start_time.elapsed() < Duration::from_millis(800);

        if is_point_click && !is_focus_activation && !is_startup_grace {
            let (line_idx, char_col) = selection.anchor;
            if let Some(line) = self.selection_lines.get(line_idx) {
                if let Some(url) = formatting::find_url_in_line(line, Some(char_col as usize)) {
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

        if selection.is_empty() || text.trim().is_empty() || self.inside_tmux {
            self.clear_selection();
        } else {
            self.drag_point = None;
        }
        if selection.is_empty() || text.trim().is_empty() {
            return;
        }

        match clipboard::copy(&text) {
            Ok(clipboard::Route::Osc52) => {
                // Many terminals (GNOME Terminal among them) ignore OSC 52 without saying so.
                self.copy_notice = Some((
                    Instant::now(),
                    "Copied via the terminal (OSC 52). Paste empty? Install xclip or wl-clipboard, or use /mouse",
                ));
            }
            Ok(_) => self.copy_notice = Some((Instant::now(), "Copied to clipboard")),
            Err(err) => self.session.add_message(Message::system(format!("Clipboard unavailable: {err}"))),
        }
    }

    /// Drops a selection still being dragged. A finished one survives losing focus, which with
    /// focus-follows-mouse happens as soon as the pointer leaves the window to paste elsewhere.
    pub fn cancel_drag(&mut self) {
        if self.drag_point.is_some() {
            self.clear_selection();
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
        Some((self.scroll_offset + row_in_view as usize, col_in_view))
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

    /// Slash-command (or subcommand) completions for the current prompt.
    ///
    /// Returns an empty list unless the menu is genuinely active: the prompt must
    /// start with `/` AND the user must not be browsing prompt history (Up/Down).
    /// Recalling an entry such as `/resume` from history fills the buffer with an
    /// exact command name, which would otherwise hijack the arrows for autocomplete
    /// navigation and trap the user on a single-item list.
    pub fn matching_slash_commands(&self) -> Vec<String> {
        if self.history_idx.is_some() || !self.input_buffer.starts_with('/') || self.file_query().is_some() {
            return Vec::new();
        }

        crate::slash_commands::match_commands(&self.input_buffer)
            .into_iter()
            .map(|c| crate::slash_commands::completion(&self.input_buffer, c))
            .collect()
    }

    /// The `@path` being typed at the cursor, unless the user is browsing history.
    pub fn file_query(&self) -> Option<String> {
        if self.history_idx.is_some() {
            return None;
        }
        crate::file_completion::at_token(&self.input_buffer, self.cursor_idx).map(|(_, q)| q.to_string())
    }

    /// Paths matching the `@path` being typed; `None` outside of one.
    pub fn file_completions(&self) -> Option<Vec<String>> {
        let query = self.file_query()?;
        Some(self.file_index.as_ref().map(|index| index.matches(&query)).unwrap_or_default())
    }

    /// Builds the workspace index the first time an `@path` is typed, and again once stale.
    pub fn refresh_file_index(&mut self) {
        let fresh = self.file_index.as_ref().is_some_and(|i| i.is_fresh_for(&self.workspace_dir));
        if self.file_query().is_some() && !fresh {
            self.file_index = Some(crate::file_completion::FileIndex::build(&self.workspace_dir));
        }
    }

    /// Replaces the `@path` being typed with `path`. A file ends the token with a space; a
    /// directory keeps it open so its contents are listed next.
    pub fn accept_file_completion(&mut self, path: &str) {
        let Some((start, _)) = crate::file_completion::at_token(&self.input_buffer, self.cursor_idx) else {
            return;
        };
        let replacement = if path.ends_with('/') { format!("@{}", path) } else { format!("@{} ", path) };
        self.input_buffer.replace_range(start..self.cursor_idx, &replacement);
        self.cursor_idx = start + replacement.len();
        self.slash_selected_idx = 0;
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

        if let Some(files) = self.file_completions() {
            return (files.len().max(1) as f32 + 2.0).min(10.0);
        }

        let count = self.matching_slash_commands().len();
        if count > 0 {
            (count as f32 + 2.0).min(10.0)
        } else {
            0.0
        }
    }

    /// Eases the popup height toward its target. The step follows the real time between frames,
    /// so the motion lasts the same ~200 ms whatever the tick rate (a fixed fraction per frame
    /// made it twice as slow and choppy when the tick was lengthened).
    pub fn advance_slash_popup(&mut self) {
        // Equivalent to the original 35% per 16 ms frame.
        const TAU_MS: f32 = 37.0;
        let target = self.slash_popup_target_height();
        let diff = target - self.slash_popup_height_current;
        if diff.abs() <= 0.05 {
            self.slash_popup_height_current = target;
            self.slash_popup_clock = None;
            return;
        }
        let now = Instant::now();
        let dt_ms = self
            .slash_popup_clock
            .map_or(16.0, |t| now.duration_since(t).as_secs_f32() * 1000.0)
            .clamp(1.0, 50.0);
        self.slash_popup_clock = Some(now);
        self.slash_popup_height_current += diff * (1.0 - (-dt_ms / TAU_MS).exp());
    }

    pub fn is_slash_animating(&self) -> bool {
        let target = self.slash_popup_target_height();
        (target - self.slash_popup_height_current).abs() > 0.05
    }

    /// Messages waiting to reach the model: the steer ones first, then the queue.
    pub fn has_pending_messages(&self) -> bool {
        !self.steer_messages.is_empty() || !self.message_queue.is_empty()
    }

    /// Takes back the newest pending message (what the user did last), steer before queue.
    pub fn undo_last_pending(&mut self) -> Option<String> {
        self.steer_messages.pop().or_else(|| self.message_queue.pop_back())
    }

    /// Empties both lists in delivery order, so nothing the user typed is thrown away.
    pub fn take_all_pending(&mut self) -> Vec<String> {
        let mut all = std::mem::take(&mut self.steer_messages);
        all.extend(self.message_queue.drain(..));
        all
    }

    /// Puts messages that were never delivered back in the composer, in front of whatever is
    /// already typed there.
    pub fn restore_to_input(&mut self, messages: Vec<String>) {
        if messages.is_empty() {
            return;
        }
        let mut text = messages.join("\n");
        if !self.input_buffer.trim().is_empty() {
            text.push('\n');
            text.push_str(&self.input_buffer);
        }
        self.input_buffer = text;
        self.cursor_idx = self.input_buffer.len();
    }

    /// "Send now" for a message already waiting in the queue: the oldest one moves to the steer
    /// list. Returns false when there was nothing to promote.
    pub fn promote_queued_to_steer(&mut self) -> bool {
        match self.message_queue.pop_front() {
            Some(msg) => {
                self.steer_messages.push(msg);
                true
            }
            None => false,
        }
    }

    /// Adds the steer messages to the conversation as one user message. Call it at a point where
    /// every tool call has its response, right before the next request to the model.
    pub fn deliver_steer(&mut self) -> bool {
        if self.steer_messages.is_empty() {
            return false;
        }
        let text = std::mem::take(&mut self.steer_messages).join("\n\n");
        corex_core::reset_repeat_guard();
        let message = self.user_message(text);
        self.session.add_message(message);
        true
    }

    /// Stops the current generation, keeping whatever the model had already written.
    ///
    /// Anything still running is orphaned: its late events carry the old generation id and are
    /// dropped. `reason` is the note left in the conversation when there was no partial text.
    pub fn cancel_generation(&mut self, reason: &str) {
        if let Some(token) = self.cancel_token.take() {
            token.cancel();
        }
        self.current_generation_id = self.current_generation_id.wrapping_add(1);
        self.in_flight_tool_calls = 0;
        self.is_streaming = false;
        self.thinking_state.is_streaming = false;
        self.clear_status();

        let partial_text = if self.streaming_text.is_empty() {
            None
        } else {
            Some(format!("{} *(interrupted)*", self.streaming_text.trim_end()))
        };
        let reasoning = if self.thinking_state.content.is_empty() {
            None
        } else {
            Some(self.thinking_state.content.clone())
        };

        if let Some(txt) = partial_text {
            self.session.add_message(Message::assistant(txt, reasoning));
        } else {
            corex_core::LlmClient::sanitize_tool_call_sequences(&mut self.session.messages);
            self.session.add_message(Message::system(reason));
        }

        self.streaming_text.clear();
        self.streaming_tool_calls.clear();
        let _ = self.session.save();
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
    pub fn max_scroll_offset(&self) -> usize {
        self.total_rendered_items
            .saturating_sub(self.chat_visible_height())
    }

    /// Scrolls the chat feed by `delta` rendered lines (negative scrolls up).
    ///
    /// Any scroll up disengages auto-scroll so new streamed content no longer drags the
    /// viewport down; landing on the last line re-engages it. This is the single place
    /// that mutates `scroll_offset`/`auto_scroll`, so wheel, PageUp/PageDown and keyboard
    /// scrolling always agree on the clamp.
    pub fn scroll_chat(&mut self, delta: i32) {
        let max = self.max_scroll_offset();
        let current = self.scroll_offset.min(max) as i64;
        let next = (current + delta as i64).clamp(0, max as i64) as usize;
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

    /// A dialog has the keyboard, so the composer behind it shows no caret of its own.
    pub fn dialog_has_focus(&self) -> bool {
        self.sudo_dialog.is_open
            || self.model_dialog.is_open
            || self.session_dialog.is_open
            || self.auth_dialog.is_open
            || self.user_dialog.is_open
            || self.rewind_dialog.is_open
    }

    pub fn active_tool_definitions(&self) -> Vec<corex_core::types::ToolDefinition> {
        let mut defs = self.tool_registry.list_definitions();
        if self.plan_mode {
            defs.retain(|d| !corex_tools::policy::hidden_in_plan_mode(&d.function.name));
        }
        defs
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

    /// The user message for `text`. When it mentions pasted images (`[Image #N]`) they travel with
    /// it as image parts after the text, in the order their tags appear.
    pub fn user_message(&mut self, text: String) -> Message {
        use base64::Engine;

        let images: Vec<_> = image_tag_ids(&text)
            .into_iter()
            .filter_map(|id| self.images.remove(&id))
            .collect();
        if images.is_empty() {
            return Message::user(text);
        }
        let mut parts = vec![ContentPart::text(text)];
        parts.extend(images.iter().map(|image| {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&image.data);
            ContentPart::image_url(format!("data:{};base64,{}", image.media_type, encoded))
        }));
        Message::user_parts(parts)
    }

    /// Forgets what a paste tag stood for once the tag is deleted from the prompt.
    pub fn forget_paste_tag(&mut self, tag: &str) {
        if let Some(rest) = tag.strip_prefix("[Pasted text #") {
            if let Some(id) = rest.split(' ').next().and_then(|n| n.parse().ok()) {
                self.pastes.remove(&id);
            }
        } else if let Some(rest) = tag.strip_prefix("[Image #") {
            if let Ok(id) = rest.trim_end_matches(']').parse() {
                self.images.remove(&id);
            }
        }
    }

    /// Whether a paste lands in the prompt (no dialog is asking for something else).
    fn composer_takes_paste(&self) -> bool {
        !self.sudo_dialog.is_open
            && !self.auth_dialog.is_open
            && !self.user_dialog.is_open
            && !(self.fleet_dialog.is_open && self.fleet_dialog.command_mode)
            && self.pending_confirmation.is_none()
    }

    /// Puts the clipboard image in the prompt as an `[Image #N]` tag; the pixels travel with the
    /// message when it is sent. Returns false when there is nothing to attach (no image on the
    /// clipboard, or a dialog has the focus), so the caller can paste text instead.
    pub fn paste_clipboard_image(&mut self) -> bool {
        if !self.composer_takes_paste() {
            return false;
        }
        match clipboard::read_image() {
            Ok(Some(image)) => {
                self.clamp_cursor();
                let id = self.next_image_id;
                self.next_image_id += 1;
                self.images.insert(id, image);
                let tag = format!("[Image #{}] ", id);
                self.input_buffer.insert_str(self.cursor_idx, &tag);
                self.cursor_idx += tag.len();
                self.slash_selected_idx = 0;
                self.last_esc_press = None;
                let cfg = self.llm_client.get_config();
                if !cfg.can_see_images() {
                    self.session.add_message(Message::system(format!(
                        "{} cannot see images: it will only read a note where [Image #{}] is. \
                         Switch with /model to Claude, GPT-4o or Gemini to have it look at the picture.",
                        cfg.model,
                        id
                    )));
                    self.invalidate_message_cache();
                }
                true
            }
            Ok(None) => false,
            Err(err) => {
                self.session.add_message(Message::system(format!("Could not paste the image: {}", err)));
                self.invalidate_message_cache();
                true
            }
        }
    }

    /// Ctrl+V: the clipboard image if there is one, otherwise its text.
    pub fn paste_from_clipboard(&mut self) {
        if self.paste_clipboard_image() {
            return;
        }
        if let Some(text) = clipboard::read_text().filter(|t| !t.is_empty()) {
            self.handle_paste(text);
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
        self.pending_task_wakeup = false;
        self.active_status = Some("Generating...".to_string());
        // Preserve user scroll: only follow stream if user has not scrolled up to read
        if self.auto_scroll {
            self.auto_scroll = true;
        }
        self.last_turn_start = Some(Instant::now());

        let prompt_builder = PromptBuilder::new(&self.workspace_dir)
            .with_sudo_password(get_sudo_password().is_some())
            .with_plan_mode(self.plan_mode)
            .with_extra_dirs(&self.extra_dirs);

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

        let tools = self.active_tool_definitions();
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

}


impl App {
    /// Applies what the model dialog asked for after a key press.
    pub fn apply_model_dialog_action(&mut self, action: DialogAction) {
        match action {
            DialogAction::None => {}
            DialogAction::Close => self.model_dialog.close(),
            DialogAction::SettingsChanged => self.apply_dialog_settings(),
            DialogAction::Activate { provider, model } => self.activate_model(provider, model),
            DialogAction::EditKey { provider } => {
                let Some(p) = self.model_dialog.providers.get(provider).cloned() else {
                    return;
                };
                self.model_dialog.close();
                self.auth_dialog.open_for_provider(
                    None,
                    &p.name,
                    p.display_name(),
                    &p.api_key_env,
                    p.portal_url(),
                );
            }
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
            format!("- Endpoint: {}\n- Mode: Local", cfg.local_llm_url)
        } else {
            match profile.family {
                ModelFamily::Flash => {
                    cfg.flash_settings = flash.clone();
                    cfg.temperature = flash.temperature;
                    cfg.reasoning_effort = flash.reasoning_effort.clone();
                    format!(
                        "- Temperature: {:.1}\n- Reasoning: {}",
                        cfg.temperature,
                        cfg.reasoning_effort
                    )
                }
                ModelFamily::Pro => {
                    cfg.pro_settings = pro.clone();
                    cfg.reasoning_effort = pro.reasoning_effort.clone();
                    format!(
                        "- Reasoning Depth: {}",
                        pro.reasoning_effort
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
impl App {
    /// Applies a choice from the `/rewind` dialog for user turn `turn`.
    pub fn apply_rewind(&mut self, action: crate::rewind_dialog::RewindAction, turn: usize) {
        use crate::rewind_dialog::{edits_since, RewindAction};

        self.rewind_dialog.close();
        if action == RewindAction::Cancel {
            return;
        }
        let edits = edits_since(&self.session.file_edits, turn);
        // Edits of rewound turns are dropped either way: those turns are gone or already undone.
        self.session.file_edits.retain(|e| e.turn < turn);

        let mut report = Vec::new();
        if matches!(action, RewindAction::RewindAndRevert | RewindAction::RevertOnly) {
            let warnings = corex_core::file_history::revert(&edits);
            let mut files: Vec<_> = edits.iter().map(|e| &e.path).collect();
            files.sort();
            files.dedup();
            report.push(format!("Reverted code changes in {} file{}.", files.len(), if files.len() == 1 { "" } else { "s" }));
            report.extend(warnings.into_iter().map(|w| format!("- {w}")));
        }
        if matches!(action, RewindAction::RewindAndRevert | RewindAction::RewindOnly) {
            let messages = self.session.display_messages();
            let text = self
                .session
                .user_turns()
                .get(turn - 1)
                .and_then(|&i| messages[i].display_text())
                .map(|t| t.to_string())
                .unwrap_or_default();
            self.session.rewind_to_turn(turn);
            self.invalidate_message_cache();
            self.clear_selection();
            self.input_buffer = text;
            self.cursor_idx = self.input_buffer.len();
            report.push("Rewound the conversation; your message is back in the prompt to edit.".to_string());
        }
        let _ = self.session.save();
        self.session.add_message(Message::system(report.join("\n")));
        self.scroll_to_bottom();
    }

}

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

        // `termux-open-url` ships with Termux (termux-tools) and hands the link to Android.
        #[cfg(target_os = "android")]
        let child = std::process::Command::new("termux-open-url")
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

        #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos", target_os = "windows"))]
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
