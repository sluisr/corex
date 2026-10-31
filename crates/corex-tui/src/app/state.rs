use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use corex_core::client::LlmClient;
use corex_core::session::Session;
use corex_core::types::ToolCall;
use corex_tools::registry::ToolRegistry;

use crate::app::mobile::{TapKeyboardGate, TouchArrowClassifier};
use crate::auth_dialog::AuthDialogState;
use crate::fleet_dialog::FleetDialogState;
use crate::model_dialog::ModelDialogState;
use crate::session_dialog::SessionDialogState;
use crate::sudo_dialog::SudoDialogState;
use crate::theme::Theme;
use crate::thinking_view::ThinkingState;
use crate::user_dialog::UserDialogState;

/// Plain text of every rendered chat line, split into segments so the (large) history segment
/// can be maintained incrementally instead of being re-allocated on every frame.
#[derive(Debug, Clone, Default)]
pub struct SelectionLines {
    /// Header banner lines (rebuilt each frame, small).
    pub head: Vec<String>,
    /// Plain text of `App::cached_message_lines`, extended incrementally.
    pub cached: Vec<String>,
    /// Live content after the cached history (pending message, streaming text, queue).
    pub tail: Vec<String>,
}

impl SelectionLines {
    pub fn get(&self, idx: usize) -> Option<&String> {
        if idx < self.head.len() {
            return self.head.get(idx);
        }
        let idx = idx - self.head.len();
        if idx < self.cached.len() {
            return self.cached.get(idx);
        }
        self.tail.get(idx - self.cached.len())
    }

    pub fn len(&self) -> usize {
        self.head.len() + self.cached.len() + self.tail.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

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

impl Default for StatusTransition {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PendingToolBatch {
    pub calls: Vec<ToolCall>,
    pub total_count: usize,
    pub current_idx: usize,
    pub diff_preview: Option<String>,
    pub selected_option: usize,
    pub diff_expanded: bool,
    pub input_mode: bool,
    pub feedback_text: String,
}

/// A mouse text selection over the rendered chat feed.
///
/// Positions are `(rendered line index, display column)`. The line index is absolute, so the
/// highlight survives scrolling, and display columns (rather than char indices) map 1:1 onto
/// screen cells, which keeps both the highlight and the extracted text correct for wide glyphs
/// where a single char occupies two cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Where the drag started.
    pub anchor: (usize, u16),
    /// Where the pointer currently is.
    pub cursor: (usize, u16),
}

impl Selection {
    /// Anchor and cursor in reading order, so `start` is never after `end`.
    pub fn ordered(&self) -> ((usize, u16), (usize, u16)) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// True when the drag covered no cells at all (i.e. a plain click).
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }
}

pub struct App {
    pub session: Session,
    pub llm_client: LlmClient,
    pub tool_registry: ToolRegistry,
    pub theme: Theme,
    pub workspace_dir: PathBuf,
    /// Read-only roots added with `/directory add`, for this session.
    pub extra_dirs: Vec<PathBuf>,

    pub input_buffer: String,
    pub input_history: Vec<String>,
    pub history_idx: Option<usize>,
    pub saved_draft: String,
    pub slash_selected_idx: usize,
    pub pastes: HashMap<usize, String>,
    pub next_paste_id: usize,
    /// Images pasted from the clipboard, by the id in their `[Image #N]` tag.
    pub images: HashMap<usize, crate::clipboard::ClipboardImage>,
    pub next_image_id: usize,
    pub cursor_idx: usize,

    pub is_streaming: bool,
    pub thinking_state: ThinkingState,
    pub streaming_text: String,
    pub streaming_tool_calls: Vec<ToolCall>,
    pub streaming_extra_content: Option<serde_json::Value>,

    pub pending_confirmation: Option<PendingToolBatch>,
    pub user_dialog: UserDialogState,
    pub rewind_dialog: crate::rewind_dialog::RewindDialog,
    pub model_dialog: ModelDialogState,
    pub session_dialog: SessionDialogState,
    pub auth_dialog: AuthDialogState,
    pub sudo_dialog: SudoDialogState,
    pub fleet_dialog: FleetDialogState,
    pub fleet_session: Option<Session>,
    pub normal_session_stash: Option<Session>,
    pub cancel_token: Option<CancellationToken>,
    pub in_flight_tool_calls: usize,
    pub response_prefix: Option<String>,

    pub scroll_offset: usize,
    pub auto_scroll: bool,
    pub total_rendered_items: usize,
    pub plan_mode: bool,
    pub always_allow_tools: bool,
    pub mouse_capture: bool,
    /// Pauses mouse reporting around a tap so Termux raises the soft keyboard.
    pub tap_keyboard: TapKeyboardGate,
    /// Separates Termux finger swipes from arrow key presses (both arrive as bare Up/Down).
    pub touch_arrows: TouchArrowClassifier,
    pub selection: Option<Selection>,
    pub selection_lines: SelectionLines,
    /// Last cell the pointer was seen at while the left button is down.
    pub drag_point: Option<(u16, u16)>,
    /// Shown in the empty prompt for a moment after a selection is copied.
    pub copy_notice: Option<(Instant, &'static str)>,
    /// Under tmux a selection ends on release, as tmux's own does.
    pub inside_tmux: bool,
    pub app_start_time: Instant,
    pub last_focus_gained: Option<Instant>,
    pub mouse_down_time: Option<Instant>,
    pub mouse_down_pos: Option<(u16, u16)>,
    pub last_url_open: Option<(Instant, String)>,
    pub session_allowed_commands: Vec<String>,
    pub logs_expanded: bool,
    pub git_branch: String,
    pub last_turn_start: Option<Instant>,
    pub last_esc_press: Option<Instant>,
    pub last_ctrl_c_press: Option<Instant>,
    pub slash_popup_height_current: f32,
    /// Time of the last easing step; `None` while the popup is at rest.
    pub slash_popup_clock: Option<Instant>,
    pub last_slash_filter: String,
    /// Workspace paths offered while an `@path` is typed; rebuilt when stale.
    pub file_index: Option<crate::file_completion::FileIndex>,
    /// The `@path` query the popup last showed, kept so it can close smoothly.
    pub last_file_query: Option<String>,
    pub local_llm_online: Arc<AtomicBool>,
    pub last_local_check: Option<Instant>,
    pub cached_message_lines: Vec<Line<'static>>,
    pub cached_message_line_offsets: Vec<usize>,
    pub cached_message_count: usize,
    pub cached_render_width: usize,
    pub cached_session_id: String,
    pub cached_logs_expanded: bool,
    /// Whether any cached message hid lines behind a collapsed log, so the footer shows the
    /// Ctrl+O hint once instead of every block repeating it.
    pub cached_has_collapsed_logs: bool,
    /// Parsed markdown of `streaming_text`, reused across frames until the text grows.
    pub streaming_md_cache: Vec<Line<'static>>,
    pub streaming_md_cache_len: usize,
    pub streaming_md_cache_width: usize,
    pub active_background_pids: HashSet<u32>,
    pub pending_task_wakeup: bool,
    pub current_generation_id: u64,
    pub update_available: Arc<std::sync::Mutex<Option<String>>>,
    pub active_status: Option<String>,
    pub status_transition: StatusTransition,
    pub last_chat_rect: Option<Rect>,
    pub balance_tx: mpsc::Sender<Result<corex_core::types::BalanceResponse, String>>,
    pub balance_rx: mpsc::Receiver<Result<corex_core::types::BalanceResponse, String>>,
    pub is_checking_balance: bool,
    pub pending_balance_msg_index: Option<usize>,
    pub message_queue: VecDeque<String>,
    /// Messages sent with "send now" while the agent works. They skip the queue: the model reads
    /// them at the next safe point (between tool rounds) instead of after the whole task.
    pub steer_messages: Vec<String>,
    pub start_time: Instant,
    pub last_interaction: Instant,
}
