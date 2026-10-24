use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use corex_fleet::{FleetConfig, NodeConfig, NodeTelemetry, RemoteSession};
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct PendingRemoteApproval {
    pub call_id: String,
    pub node_id: String,
    pub command: String,
    pub sudo: bool,
    pub edit_mode: bool,
    pub edited_command: String,
    pub instructions_mode: bool,
    pub instructions_text: String,
}

#[derive(Debug, Clone)]
pub struct FleetItem {
    pub config: NodeConfig,
    pub telemetry: Option<NodeTelemetry>,
}

pub struct FleetDialogState {
    pub is_open: bool,
    pub selected_idx: usize,
    pub items: Vec<FleetItem>,
    pub search_query: String,
    pub is_refreshing: bool,
    pub status_msg: Option<String>,
    pub command_input: String,
    /// true = keyboard focus is on the VPS terminal pane, false = on the AI chat.
    pub command_mode: bool,
    pub active_session: Option<RemoteSession>,
    pub pending_approval: Option<PendingRemoteApproval>,
    pub terminal_scroll: usize,
    /// Set when the dialog opens: the event loop connects to the selected node
    /// and measures its telemetry automatically on the next iteration.
    pub auto_connect_pending: bool,
    /// Optional 3-column layout (chat | output | fleet telemetry). Off by default;
    /// toggled with `/fleet wide`. The default is the minimal 2-pane layout.
    pub wide_layout: bool,
    pub last_tab_input: Option<String>,
    pub pty_session: Option<crate::fleet_pty::FleetPtySession>,
    pub cmd_done_tx: UnboundedSender<(String, RemoteSession)>,
    pub cmd_done_rx: UnboundedReceiver<(String, RemoteSession)>,
    pub telemetry_tx: UnboundedSender<(String, NodeTelemetry)>,
    pub telemetry_rx: UnboundedReceiver<(String, NodeTelemetry)>,
    last_refresh: Option<Instant>,
    refresh_inflight: Arc<AtomicBool>,
}

impl Default for FleetDialogState {
    fn default() -> Self {
        Self::new()
    }
}

impl FleetDialogState {
    pub fn new() -> Self {
        let (telemetry_tx, telemetry_rx) = unbounded_channel();
        let (cmd_done_tx, cmd_done_rx) = unbounded_channel();
        Self {
            is_open: false,
            selected_idx: 0,
            items: Vec::new(),
            search_query: String::new(),
            is_refreshing: false,
            status_msg: None,
            command_input: String::new(),
            command_mode: false,
            active_session: None,
            pending_approval: None,
            terminal_scroll: 0,
            auto_connect_pending: false,
            wide_layout: false,
            last_tab_input: None,
            pty_session: None,
            cmd_done_tx,
            cmd_done_rx,
            telemetry_tx,
            telemetry_rx,
            last_refresh: None,
            refresh_inflight: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn open(&mut self, workspace: &Path) {
        self.init_open_items(workspace);

        // Restore active session from global manager if available on a multi-thread runtime
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
                if let Some(active_id) = tokio::task::block_in_place(|| {
                    handle.block_on(corex_fleet::global_session_manager().active_node())
                }) {
                    if let Some(pos) = self.items.iter().position(|it| it.config.id == active_id) {
                        self.selected_idx = pos;
                    }
                    if let Some(sess) = tokio::task::block_in_place(|| {
                        handle.block_on(corex_fleet::global_session_manager().get_session(&active_id))
                    }) {
                        self.active_session = Some(sess);
                    }
                }
            }
        }

        self.finalize_open();
    }

    pub async fn open_async(&mut self, workspace: &Path) {
        self.init_open_items(workspace);

        if let Some(active_id) = corex_fleet::global_session_manager().active_node().await {
            if let Some(pos) = self.items.iter().position(|it| it.config.id == active_id) {
                self.selected_idx = pos;
            }
            if let Some(sess) = corex_fleet::global_session_manager().get_session(&active_id).await {
                self.active_session = Some(sess);
            }
        }

        self.finalize_open();
    }

    fn init_open_items(&mut self, workspace: &Path) {
        self.is_open = true;
        self.command_input.clear();
        self.last_tab_input = None;
        self.search_query.clear();

        let cfg = FleetConfig::load(Some(workspace)).unwrap_or_default();
        let all_nodes = cfg.all_nodes();

        self.items = all_nodes
            .into_iter()
            .map(|n| FleetItem {
                config: n.clone(),
                telemetry: None,
            })
            .collect();
    }

    fn finalize_open(&mut self) {
        if let Some(ref sess) = self.active_session {
            if sess.is_connected {
                let need_spawn = self
                    .pty_session
                    .as_ref()
                    .map(|p| !p.is_alive() || p.node_id != sess.node.id)
                    .unwrap_or(true);
                if need_spawn {
                    match crate::fleet_pty::FleetPtySession::spawn(&sess.node, 24, 80) {
                        Ok(pty) => {
                            self.pty_session = Some(pty);
                        }
                        Err(err) => {
                            self.pty_session = None;
                            self.status_msg = Some(format!("Interactive PTY unavailable ({}); fallback batch SSH mode active", err));
                        }
                    }
                }
            } else {
                self.pty_session = None;
            }
        } else {
            self.pty_session = None;
        }

        // Focus the server selector if no active session yet, so Up/Down and Enter work immediately.
        self.command_mode = self.active_session.as_ref().map(|s| !s.is_connected).unwrap_or(true);
        self.last_refresh = None;
        self.auto_connect_pending = false;
        self.status_msg = if self.items.is_empty() {
            Some("No servers configured: add hosts to .corex/fleet.toml or ~/.ssh/config".to_string())
        } else {
            None
        };
    }

    /// Seconds between automatic fleet-wide telemetry refreshes.
    const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

    /// Applies finished telemetry results and, when due, launches a background
    /// probe of every node. Never blocks the UI. Returns true if anything changed
    /// (so the caller can redraw).
    pub fn poll_telemetry(&mut self) -> bool {
        let mut changed = false;
        while let Ok((id, tel)) = self.telemetry_rx.try_recv() {
            if let Some(item) = self.items.iter_mut().find(|it| it.config.id == id) {
                item.telemetry = Some(tel);
                changed = true;
            }
            if self.status_msg.as_deref() == Some("Probing telemetry...") {
                self.status_msg = None;
            }
        }
        // Only query telemetry when fleet is open AND there is an active connected session.
        // Before connecting, keep it completely quiet and fast.
        if !self.is_open || self.items.is_empty() || !self.is_selected_connected() {
            return changed;
        }
        let any_missing_in_wide = self.wide_layout && self.items.iter().take(8).any(|i| i.telemetry.is_none());
        let due = any_missing_in_wide
            || self
                .last_refresh
                .map(|t| t.elapsed() >= Self::REFRESH_INTERVAL)
                .unwrap_or(true);
        if due && !self.refresh_inflight.swap(true, Ordering::SeqCst) {
            self.last_refresh = Some(Instant::now());
            let nodes_to_probe: Vec<NodeConfig> = if self.wide_layout {
                self.items.iter().take(8).map(|i| i.config.clone()).collect()
            } else {
                let mut v = Vec::new();
                if let Some(n) = self
                    .active_session
                    .as_ref()
                    .map(|s| s.node.clone())
                    .or_else(|| self.selected_node().cloned())
                {
                    v.push(n);
                }
                for item in self.items.iter().take(8) {
                    if item.telemetry.is_none() && !v.iter().any(|n| n.id == item.config.id) {
                        v.push(item.config.clone());
                    }
                }
                v
            };
            let tx = self.telemetry_tx.clone();
            let inflight = self.refresh_inflight.clone();
            if !nodes_to_probe.is_empty() {
                tokio::spawn(async move {
                    let mut handles = Vec::new();
                    for n in nodes_to_probe {
                        let tx_clone = tx.clone();
                        handles.push(tokio::spawn(async move {
                            let t = corex_fleet::TelemetryCollector::fetch(&n, 4).await;
                            let _ = tx_clone.send((n.id.clone(), t));
                        }));
                    }
                    for h in handles {
                        let _ = h.await;
                    }
                    inflight.store(false, Ordering::SeqCst);
                });
            } else {
                inflight.store(false, Ordering::SeqCst);
            }
        }
        changed
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.pty_session = None;
        if let Some(ref mut pending) = self.pending_approval {
            pending.edit_mode = false;
            pending.instructions_mode = false;
        }
    }

    pub fn filtered_items(&self) -> Vec<(usize, &FleetItem)> {
        if self.search_query.trim().is_empty() {
            return self.items.iter().enumerate().collect();
        }
        let q = self.search_query.trim().to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                it.config.id.to_lowercase().contains(&q)
                    || it.config.host.to_lowercase().contains(&q)
                    || it.config.user.to_lowercase().contains(&q)
                    || it.config.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn select_prev(&mut self) {
        let filtered = self.filtered_items();
        if filtered.is_empty() {
            return;
        }
        if let Some(pos) = filtered.iter().position(|(idx, _)| *idx == self.selected_idx) {
            if pos > 0 {
                self.selected_idx = filtered[pos - 1].0;
            }
        } else if let Some(first) = filtered.first() {
            self.selected_idx = first.0;
        }
    }

    pub fn select_next(&mut self) {
        let filtered = self.filtered_items();
        if filtered.is_empty() {
            return;
        }
        if let Some(pos) = filtered.iter().position(|(idx, _)| *idx == self.selected_idx) {
            if pos + 1 < filtered.len() {
                self.selected_idx = filtered[pos + 1].0;
            }
        } else if let Some(first) = filtered.first() {
            self.selected_idx = first.0;
        }
    }

    pub fn selected_node(&self) -> Option<&NodeConfig> {
        self.items.get(self.selected_idx).map(|item| &item.config)
    }

    pub fn is_selected_connected(&self) -> bool {
        if let (Some(ref sess), Some(node)) = (&self.active_session, self.selected_node()) {
            sess.is_connected && sess.node.id == node.id
        } else {
            false
        }
    }

    pub fn total_output_lines(&self) -> usize {
        self.active_session
            .as_ref()
            .map(|s| s.live_output_lines(1000).len())
            .unwrap_or(0)
    }

    pub fn scroll_up(&mut self, delta: usize, total_lines: usize, view_height: usize) {
        let max_scroll = total_lines.saturating_sub(view_height);
        self.terminal_scroll = (self.terminal_scroll + delta).min(max_scroll);
    }

    pub fn scroll_down(&mut self, delta: usize) {
        self.terminal_scroll = self.terminal_scroll.saturating_sub(delta);
    }
}


fn rule(width: u16, theme: &Theme) -> Paragraph<'static> {
    Paragraph::new(Line::from(Span::styled("─".repeat(width as usize), Style::default().fg(theme.rule))))
}

/// Right-hand pane of the Fleet split view:
/// - When NO active session is connected to the selected server: displays an interactive Server Selector
///   where the user can navigate with Up/Down and press Enter to connect.
/// - When connected: displays the live VPS terminal, telemetry, output feed and command prompt.
pub fn render_fleet_dialog(
    frame: &mut Frame,
    area: Rect,
    state: &FleetDialogState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    if !state.is_selected_connected() {
        render_server_selector(frame, area, state, theme);
        return;
    }

    let approval_height: u16 = if state.pending_approval.is_some() { 5 } else { 0 };

    if let Some(ref pty) = state.pty_session {
        let v = Layout::vertical([
            Constraint::Length(1),                    // header: server tabs
            Constraint::Length(1),                    // rule
            Constraint::Length(metrics_height(state)), // telemetry card
            Constraint::Length(1),                    // titled rule: terminal
            Constraint::Min(4),                       // PTY terminal
            Constraint::Length(approval_height),      // approval strip
            Constraint::Length(1),                    // rule (aligns with chat composer top border)
            Constraint::Length(1),                    // footer (aligns with chat status bar)
        ])
        .split(area);

        render_header(frame, v[0], state, theme);
        frame.render_widget(rule(area.width, theme), v[1]);
        render_metrics(frame, v[2], state, theme);
        render_section_title(frame, v[3], "terminal", theme);

        pty.resize(v[4].height, v[4].width);
        pty.render(v[4], frame.buffer_mut());

        if state.command_mode {
            if let Some((row, col)) = pty.cursor_position() {
                let cur_x = (v[4].x + col).min(v[4].x + v[4].width.saturating_sub(1));
                let cur_y = (v[4].y + row).min(v[4].y + v[4].height.saturating_sub(1));
                frame.set_cursor_position(ratatui::layout::Position { x: cur_x, y: cur_y });
            }
        }

        if approval_height > 0 {
            render_approval(frame, v[5], state, theme);
        }
        frame.render_widget(rule(area.width, theme), v[6]);
        render_footer(frame, v[7], state, theme);
    } else {
        let v = Layout::vertical([
            Constraint::Length(1),                    // header: server tabs
            Constraint::Length(1),                    // rule
            Constraint::Length(metrics_height(state)), // telemetry card
            Constraint::Length(1),                    // titled rule: output
            Constraint::Min(3),                       // live feed
            Constraint::Length(approval_height),      // approval strip
            Constraint::Length(1),                    // rule (aligns with chat composer top border)
            Constraint::Length(1),                    // prompt (aligns with chat input line)
            Constraint::Length(1),                    // rule (aligns with chat composer bottom border)
            Constraint::Length(1),                    // footer (aligns with chat status bar)
        ])
        .split(area);

        render_header(frame, v[0], state, theme);
        frame.render_widget(rule(area.width, theme), v[1]);
        render_metrics(frame, v[2], state, theme);
        render_section_title(frame, v[3], "output [batch ssh]", theme);
        render_feed(frame, v[4], state, theme);
        if approval_height > 0 {
            render_approval(frame, v[5], state, theme);
        }
        frame.render_widget(rule(area.width, theme), v[6]);
        render_prompt(frame, v[7], state, theme);
        frame.render_widget(rule(area.width, theme), v[8]);
        render_footer(frame, v[9], state, theme);
    }
}

fn render_server_selector(
    frame: &mut Frame,
    area: Rect,
    state: &FleetDialogState,
    theme: &Theme,
) {
    let dim = Style::default().fg(theme.dark_gray);
    let val = Style::default().fg(theme.text_strong);
    let focused = state.command_mode;

    let v = Layout::vertical([
        Constraint::Length(1), // Title
        Constraint::Length(1), // rule
        Constraint::Min(3),    // clean server list
        Constraint::Length(1), // status message (errors if connection failed)
        Constraint::Length(1), // rule (aligns with chat composer top border)
        Constraint::Length(1), // action bar (aligns with chat input line)
        Constraint::Length(1), // rule (aligns with chat composer bottom border)
        Constraint::Length(1), // footer (aligns with chat status bar)
    ])
    .split(area);

    let filtered = state.filtered_items();
    let title_line = if state.search_query.is_empty() {
        Line::from(vec![
            Span::styled(" SSH SERVERS ", Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)),
            Span::styled(format!("({} available)", state.items.len()), dim),
        ])
    } else {
        Line::from(vec![
            Span::styled(" SSH SERVERS ", Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)),
            Span::styled(format!("({}/{} available) · Filter: ", filtered.len(), state.items.len()), dim),
            Span::styled(&state.search_query, Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_yellow)),
        ])
    };
    frame.render_widget(Paragraph::new(title_line), v[0]);
    frame.render_widget(rule(area.width, theme), v[1]);

    let mut list_lines: Vec<Line> = Vec::new();
    if state.items.is_empty() {
        list_lines.push(Line::from(""));
        list_lines.push(Line::from(vec![
            Span::styled("  No servers configured.", Style::default().fg(theme.dark_gray)),
        ]));
        list_lines.push(Line::from(vec![
            Span::styled("  Add hosts to ~/.ssh/config or create .corex/fleet.toml", dim),
        ]));
    } else if filtered.is_empty() {
        list_lines.push(Line::from(""));
        list_lines.push(Line::from(vec![
            Span::styled(format!("  No servers matching '{}'.", state.search_query), Style::default().fg(theme.accent_yellow)),
        ]));
        list_lines.push(Line::from(vec![
            Span::styled("  Press Esc or Backspace to clear filter.", dim),
        ]));
    } else {
        list_lines.push(Line::from("")); // subtle top padding
        for &(orig_idx, item) in &filtered {
            let is_selected = orig_idx == state.selected_idx;
            let bg = if is_selected { theme.selected_bg } else { Color::Reset };
            let prefix = if is_selected { " > " } else { "   " };
            let num_badge = if orig_idx < 9 && state.search_query.is_empty() {
                format!("[{}] ", orig_idx + 1)
            } else {
                format!("[{:>2}] ", orig_idx + 1)
            };

            let line = vec![
                Span::styled(prefix, Style::default().fg(if is_selected { theme.accent_cyan } else { Color::Reset }).bg(bg).add_modifier(Modifier::BOLD)),
                Span::styled(num_badge, Style::default().fg(if is_selected { theme.accent_yellow } else { theme.dark_gray }).bg(bg)),
                Span::styled(format!("{:<18}", item.config.id), Style::default().fg(if is_selected { theme.text_on_selected } else { theme.gray }).bg(bg).add_modifier(if is_selected { Modifier::BOLD } else { Modifier::empty() })),
                Span::styled(format!("{}@{}", item.config.user, item.config.host), Style::default().fg(if is_selected { theme.text_strong } else { theme.dark_gray }).bg(bg)),
            ];
            list_lines.push(Line::from(line));
        }
    }
    frame.render_widget(Paragraph::new(list_lines), v[2]);

    if let Some(ref msg) = state.status_msg {
        let msg_style = if msg.contains("failed") || msg.contains("Error") {
            Style::default().fg(theme.accent_red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accent_yellow)
        };
        frame.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(msg.clone(), msg_style)])), v[3]);
    } else {
        frame.render_widget(Paragraph::new(Line::from("")), v[3]);
    }

    frame.render_widget(rule(area.width, theme), v[4]);

    let sel_name = state.selected_node().map(|n| n.id.as_str()).unwrap_or("server");
    let action_line = Line::from(vec![
        Span::raw(" "),
        Span::styled("Enter", Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" Connect to {}", sel_name), val),
    ]);
    frame.render_widget(Paragraph::new(action_line), v[5]);
    frame.render_widget(rule(area.width, theme), v[6]);

    let mut footer_spans = vec![
        Span::raw(" "),
        Span::styled("↑↓ / Tab / 1-9", Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
        Span::styled(" Select VPS  ·  ", dim),
        Span::styled("Enter", Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
        Span::styled(" Connect  ·  ", dim),
        Span::styled("Esc", Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)),
        Span::styled(if focused { " Chat" } else { " Close" }, dim),
    ];
    if !state.search_query.is_empty() {
        footer_spans.push(Span::styled("  ·  Esc", Style::default().fg(theme.gray).add_modifier(Modifier::BOLD)));
        footer_spans.push(Span::styled(" Clear Filter", dim));
    }
    frame.render_widget(Paragraph::new(Line::from(footer_spans)), v[7]);
}

/// Minimum terminal width at which Fleet switches to the 3-column layout.
pub const FLEET_WIDE_MIN_WIDTH: u16 = 135;

/// Center column of the wide layout: connection title, live output/PTY, approvals,
/// shell prompt and footer. Telemetry lives in the fleet column instead.
pub fn render_fleet_output(frame: &mut Frame, area: Rect, state: &FleetDialogState, theme: &Theme) {
    if !state.is_open {
        return;
    }
    if !state.is_selected_connected() {
        render_server_selector(frame, area, state, theme);
        return;
    }
    let approval_height: u16 = if state.pending_approval.is_some() { 5 } else { 0 };
    if let Some(ref pty) = state.pty_session {
        let v = Layout::vertical([
            Constraint::Length(1),               // title
            Constraint::Min(4),                  // REAL LIVE PTY TERMINAL
            Constraint::Length(approval_height), // approval strip
            Constraint::Length(1),               // rule
            Constraint::Length(1),               // footer
        ])
        .split(area);

        let title = match state.active_session.as_ref().filter(|s| s.is_connected) {
            Some(s) => format!("terminal · {}@{} · {}", s.node.user, s.node.host, s.current_dir),
            None => "terminal".to_string(),
        };
        render_section_title(frame, v[0], &title, theme);
        pty.resize(v[1].height, v[1].width);
        pty.render(v[1], frame.buffer_mut());
        if state.command_mode {
            if let Some((row, col)) = pty.cursor_position() {
                let cur_x = (v[1].x + col).min(v[1].x + v[1].width.saturating_sub(1));
                let cur_y = (v[1].y + row).min(v[1].y + v[1].height.saturating_sub(1));
                frame.set_cursor_position(ratatui::layout::Position { x: cur_x, y: cur_y });
            }
        }
        if approval_height > 0 {
            render_approval(frame, v[2], state, theme);
        }
        frame.render_widget(rule(area.width, theme), v[3]);
        render_footer(frame, v[4], state, theme);
    } else {
        let v = Layout::vertical([
            Constraint::Length(1),               // title
            Constraint::Min(3),                  // live feed
            Constraint::Length(approval_height), // approval strip
            Constraint::Length(1),               // rule
            Constraint::Length(1),               // prompt
            Constraint::Length(1),               // footer
        ])
        .split(area);

        let title = match state.active_session.as_ref().filter(|s| s.is_connected) {
            Some(s) => format!("output [batch ssh] · {}@{} · {}", s.node.user, s.node.host, s.current_dir),
            None => "output [batch ssh]".to_string(),
        };
        render_section_title(frame, v[0], &title, theme);
        render_feed(frame, v[1], state, theme);
        if approval_height > 0 {
            render_approval(frame, v[2], state, theme);
        }
        frame.render_widget(rule(area.width, theme), v[3]);
        render_prompt(frame, v[4], state, theme);
        render_footer(frame, v[5], state, theme);
    }
}

fn truncate_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Right column of the wide layout: one telemetry card per server so the whole
/// fleet is visible at a glance. The selected server is highlighted.
pub fn render_fleet_column(frame: &mut Frame, area: Rect, state: &FleetDialogState, theme: &Theme) {
    if !state.is_open || area.height < 2 {
        return;
    }
    const CARD_H: u16 = 5; // 4 content lines + 1 spacer
    let dim = Style::default().fg(theme.dark_gray);
    let val = Style::default().fg(theme.text_strong);

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let online = state
        .items
        .iter()
        .filter(|i| i.telemetry.as_ref().map(|t| t.is_online).unwrap_or(false))
        .count();
    render_section_title(
        frame,
        rows[0],
        &format!("fleet · {}/{} online", online, state.items.len()),
        theme,
    );

    let body = rows[1];
    if state.items.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" no servers", dim))),
            body,
        );
        return;
    }

    let capacity = (body.height / CARD_H).max(1) as usize;
    // Keep the selected card in view.
    let first = state.selected_idx.saturating_sub(capacity.saturating_sub(1));
    let w = body.width as usize;
    let bar_w = w.saturating_sub(16).clamp(4, 14);

    for (slot, (i, item)) in state.items.iter().enumerate().skip(first).take(capacity).enumerate() {
        let y = body.y + (slot as u16) * CARD_H;
        if y + 4 > body.y + body.height {
            break;
        }
        let card = Rect { x: body.x, y, width: body.width, height: 4 };
        let selected = i == state.selected_idx;
        let bg = if selected { theme.selected_bg } else { Color::Reset };
        let live = state
            .active_session
            .as_ref()
            .map(|s| s.is_connected && s.node.id == item.config.id)
            .unwrap_or(false);
        let tel = item.telemetry.as_ref();
        let (dot, dot_col) = if live {
            ("●", theme.success)
        } else {
            match tel {
                Some(t) if t.is_online => ("●", theme.accent_green),
                Some(_) => ("×", theme.accent_red),
                None => ("○", theme.dark_gray),
            }
        };
        let name_style = Style::default()
            .fg(if selected { theme.text_on_selected } else { theme.gray })
            .bg(bg)
            .add_modifier(if selected { Modifier::BOLD } else { Modifier::empty() });

        let latency = tel
            .and_then(|t| t.latency_ms)
            .map(|ms| format!("{}ms", ms))
            .unwrap_or_default();
        let latency_w = unicode_width::UnicodeWidthStr::width(latency.as_str());
        let name_max = w.saturating_sub(latency_w + 7);
        let name = truncate_to(&item.config.id, name_max.max(4));
        let name_w = unicode_width::UnicodeWidthStr::width(name.as_str());
        let prefix_num = format!(" {} ", i + 1);
        let dot_str = format!("{} ", dot);
        let prefix_w = unicode_width::UnicodeWidthStr::width(prefix_num.as_str())
            + unicode_width::UnicodeWidthStr::width(dot_str.as_str());
        let used = prefix_w + name_w + latency_w + 1;
        let head = vec![
            Span::styled(prefix_num, Style::default().fg(theme.dark_gray).bg(bg)),
            Span::styled(dot_str, Style::default().fg(dot_col).bg(bg)),
            Span::styled(name, name_style),
            Span::styled(" ".repeat(w.saturating_sub(used)), Style::default().bg(bg)),
            Span::styled(latency, Style::default().fg(theme.dark_gray).bg(bg)),
            Span::styled(" ", Style::default().bg(bg)),
        ];

        let mut lines: Vec<Line> = vec![Line::from(head)];
        let gauge = |label: &str, pct: u8, extra: String| -> Line<'static> {
            let mut spans = vec![
                Span::styled(format!("   {:<4}", label), dim),
                Span::styled(make_progress_bar(pct, bar_w), Style::default().fg(gauge_color(pct, theme))),
                Span::styled(format!(" {:>3}%", pct), val),
            ];
            if pct >= 90 {
                spans.push(Span::styled(" !", Style::default().fg(theme.accent_red)));
            } else if !extra.is_empty() {
                spans.push(Span::styled(format!(" {}", extra), dim));
            }
            Line::from(spans)
        };

        match tel {
            Some(t) if t.is_online => {
                let ram_extra = match (t.memory_used_mb, t.memory_total_mb) {
                    (Some(u), Some(tot)) => format!("{}/{}M", u, tot),
                    _ => String::new(),
                };
                lines.push(gauge("ram", t.memory_percent().unwrap_or(0), ram_extra));
                lines.push(gauge("disk", t.disk_usage_pct.unwrap_or(0), String::new()));
                let mut tail = vec![Span::styled("   ", dim)];
                if let Some(ref load) = t.load_avg {
                    tail.push(Span::styled("load ", dim));
                    tail.push(Span::styled(truncate_to(load, 14), val));
                }
                if let Some(ref up) = t.uptime_str {
                    let clean_up = up.trim_start_matches("up ").trim();
                    tail.push(Span::styled("  up ", dim));
                    tail.push(Span::styled(truncate_to(clean_up, 12), val));
                }
                lines.push(Line::from(tail));
            }
            Some(t) => {
                let err = t.error_message.as_deref().unwrap_or("connection refused");
                lines.push(Line::from(Span::styled(
                    format!("   {}", truncate_to(err, w.saturating_sub(4))),
                    Style::default().fg(theme.accent_red),
                )));
            }
            None => lines.push(Line::from(Span::styled("   probing…", dim))),
        }
        frame.render_widget(Paragraph::new(lines), card);
    }

    let hidden = state.items.len().saturating_sub(first + capacity);
    if hidden > 0 && body.height > 0 {
        let more = Rect { x: body.x, y: body.y + body.height - 1, width: body.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!("  +{} more ↓", hidden), dim))),
            more,
        );
    }
}

fn render_section_title(frame: &mut Frame, area: Rect, title: &str, theme: &Theme) {
    let label = format!(" {} ", title);
    let label_w = unicode_width::UnicodeWidthStr::width(label.as_str());
    let fill = (area.width as usize).saturating_sub(label_w + 1);
    let line = Line::from(vec![
        Span::styled("─", Style::default().fg(theme.rule)),
        Span::styled(label, Style::default().fg(theme.dark_gray)),
        Span::styled("─".repeat(fill), Style::default().fg(theme.rule)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_header(frame: &mut Frame, area: Rect, state: &FleetDialogState, theme: &Theme) {
    let mut spans = vec![Span::styled(" vps ", Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD))];

    if state.items.is_empty() {
        spans.push(Span::styled("no servers · configure .corex/fleet.toml or ~/.ssh/config", Style::default().fg(theme.dark_gray)));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }

    let active_node_id = state.active_session.as_ref().filter(|s| s.is_connected).map(|s| s.node.id.as_str());

    for (i, item) in state.items.iter().enumerate() {
        let is_selected = i == state.selected_idx;
        let is_active = active_node_id == Some(&item.config.id);
        let bg = if is_selected { theme.selected_bg } else { Color::Reset };
        let num_fg = if is_selected { theme.accent_cyan } else { theme.dark_gray };
        let name_fg = if is_selected { theme.text_on_selected } else { theme.gray };
        let mut name_style = Style::default().fg(name_fg).bg(bg);
        if is_selected {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }

        spans.push(Span::styled(format!(" [{}] ", i + 1), Style::default().fg(num_fg).bg(bg)));
        spans.push(Span::styled(item.config.id.clone(), name_style));

        if is_active {
            spans.push(Span::styled(" ● ", Style::default().fg(theme.success).bg(bg)));
        } else if item.telemetry.is_some() {
            spans.push(Span::styled(" ○ ", Style::default().fg(theme.accent_cyan).bg(bg)));
        } else {
            spans.push(Span::styled(" · ", Style::default().fg(theme.dark_gray).bg(bg)));
        }
    }

    if let Some(s) = state.active_session.as_ref().filter(|s| s.is_connected) {
        let ping_str = s.last_ping_ms.map(|ms| format!(" · {}ms", ms)).unwrap_or_default();
        let right = format!("{}@{} · {}{} ", s.node.user, s.node.host, s.current_dir, ping_str);
        let used: usize = spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
        let right_w = unicode_width::UnicodeWidthStr::width(right.as_str());
        if (area.width as usize) > used + right_w + 2 {
            let pad = (area.width as usize) - (used + right_w);
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(right, Style::default().fg(theme.gray)));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Color a gauge by usage: green < 70, yellow < 90, red otherwise.
fn gauge_color(pct: u8, theme: &Theme) -> Color {
    if pct >= 90 {
        theme.accent_red
    } else if pct >= 70 {
        theme.accent_yellow
    } else {
        theme.accent_green
    }
}

fn telemetry_for<'a>(state: &'a FleetDialogState) -> Option<&'a NodeTelemetry> {
    state.selected_node()
        .and_then(|n| state.items.iter().find(|it| it.config.id == n.id))
        .and_then(|it| it.telemetry.as_ref())
}

/// Height of the telemetry strip: 2 lines when we have live metrics, 1 otherwise.
fn metrics_height(state: &FleetDialogState) -> u16 {
    match telemetry_for(state) {
        Some(t) if t.is_online => 2,
        _ => 1,
    }
}

fn render_metrics(frame: &mut Frame, area: Rect, state: &FleetDialogState, theme: &Theme) {
    let dim = Style::default().fg(theme.dark_gray);
    let val = Style::default().fg(theme.text_strong);
    let mut lines: Vec<Line> = Vec::new();

    match telemetry_for(state) {
        Some(t) if t.is_online => {
            // Line 1: identity · latency · load · uptime
            let mut l1 = vec![Span::raw(" ")];
            if let Some(ref h) = t.hostname {
                let short_h = h.split('.').next().unwrap_or(h);
                l1.push(Span::styled(short_h.to_string(), Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)));
            }
            if let Some(ref os) = t.os_name {
                let os_short = if os.starts_with("Linux") { "Linux" } else { os.split_whitespace().next().unwrap_or(os) };
                l1.push(Span::styled(format!("  {}", os_short), dim));
            }
            if let Some(ms) = t.latency_ms {
                l1.push(Span::styled(format!("  {}ms", ms), dim));
            }
            if let Some(ref load) = t.load_avg {
                l1.push(Span::styled("  ·  load ", dim));
                l1.push(Span::styled(load.clone(), val));
            }
            if let Some(ref up) = t.uptime_str {
                let clean_up = up.trim_start_matches("up ").trim();
                l1.push(Span::styled("  ·  up ", dim));
                l1.push(Span::styled(clean_up.to_string(), val));
            }
            lines.push(Line::from(l1));

            // Line 2: RAM and disk gauges side by side
            let bar_w = ((area.width as usize).saturating_sub(22) / 2).clamp(4, 16);
            let ram = t.memory_percent().unwrap_or(0);
            let disk = t.disk_usage_pct.unwrap_or(0);
            let mut l2 = vec![Span::styled(" ram ", dim)];
            l2.push(Span::styled(make_progress_bar(ram, bar_w), Style::default().fg(gauge_color(ram, theme))));
            l2.push(Span::styled(format!(" {:>3}%", ram), val));
            l2.push(Span::styled("   disk ", dim));
            l2.push(Span::styled(make_progress_bar(disk, bar_w), Style::default().fg(gauge_color(disk, theme))));
            l2.push(Span::styled(format!(" {:>3}%", disk), val));
            if ram >= 90 || disk >= 90 {
                l2.push(Span::styled(" !", Style::default().fg(theme.accent_red)));
            }
            lines.push(Line::from(l2));
        }
        Some(t) => {
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("unreachable · {}", t.error_message.as_deref().unwrap_or("connection refused")),
                    Style::default().fg(theme.accent_red),
                ),
            ]));
        }
        None => lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("no telemetry · Ctrl+R to probe", dim),
        ])),
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Sanitizes terminal output: strips cursor movement / ANSI sequences, resolves carriage returns,
/// and strictly clamps character width to prevent overflowing into adjacent panes.
pub fn clean_terminal_line(input: &str, max_width: usize) -> String {
    let mut cleaned = String::with_capacity(input.len());
    let mut in_escape = false;
    let mut in_osc = false;

    // Fastfetch / progress spinners emit '\r' to reposition cursor to line start.
    // In split-screen TUI, a literal '\r' moves physical cursor to column 0 (the chat pane!).
    // We isolate the latest text segment after the last '\r'.
    let effective = match input.rfind('\r') {
        Some(pos) if pos + 1 < input.len() => &input[pos + 1..],
        Some(_) => "",
        None => input,
    };

    let mut chars = effective.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if let Some(&next) = chars.peek() {
                if next == ']' {
                    chars.next();
                    in_osc = true;
                    continue;
                }
            }
            in_escape = true;
            continue;
        }

        if in_osc {
            if c == '\x07' || c == '\x1b' {
                in_osc = false;
            }
            continue;
        }

        if in_escape {
            // CSI sequences terminate on an ASCII letter, @, or ~
            if c.is_ascii_alphabetic() || c == '@' || c == '~' {
                in_escape = false;
            }
            continue;
        }

        // Drop non-printable control characters and stray carriage returns
        if c.is_control() && c != '\t' {
            continue;
        }

        if c == '\t' {
            cleaned.push_str("    ");
        } else {
            cleaned.push(c);
        }
    }

    if max_width > 0 && unicode_width::UnicodeWidthStr::width(cleaned.as_str()) > max_width {
        let mut w = 0;
        let mut truncated = String::new();
        for ch in cleaned.chars() {
            let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
            if w + ch_w > max_width {
                break;
            }
            w += ch_w;
            truncated.push(ch);
        }
        truncated
    } else {
        cleaned
    }
}

fn render_feed(frame: &mut Frame, area: Rect, state: &FleetDialogState, theme: &Theme) {
    let mut lines: Vec<Line> = Vec::new();
    match state.active_session.as_ref().filter(|s| s.is_connected) {
        Some(sess) => {
            let mut out = sess.live_output_lines(500);
            if let Some(ref msg) = state.status_msg {
                if msg.contains('$') || msg.starts_with("Running") || msg.starts_with("Executing") {
                    out.push(format!("  › {} …", msg));
                }
            }
            if out.is_empty() {
                lines.push(Line::from(Span::styled(
                    " Persistent SSH session ready. Remote command output will stream here.",
                    Style::default().fg(theme.dark_gray),
                )));
            } else {
                let cap = area.height as usize;
                let max_w = (area.width as usize).saturating_sub(1);
                let max_scroll = out.len().saturating_sub(cap);
                let scroll = state.terminal_scroll.min(max_scroll);
                let skip = out.len().saturating_sub(cap + scroll);
                for l in out.iter().skip(skip).take(cap) {
                    let cleaned = clean_terminal_line(l, max_w);
                    let style = if cleaned.contains("user") && cleaned.contains('$') {
                        Style::default().fg(theme.accent_blue).add_modifier(Modifier::BOLD)
                    } else if cleaned.contains("corex") && cleaned.contains('$') {
                        Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)
                    } else if cleaned.starts_with("  !") {
                        Style::default().fg(theme.accent_red)
                    } else if cleaned.starts_with("  ›") {
                        Style::default().fg(theme.accent_yellow).add_modifier(Modifier::ITALIC)
                    } else {
                        Style::default().fg(theme.text_strong)
                    };
                    lines.push(Line::from(Span::styled(cleaned, style)));
                }
            }
        }
        None => {
            let target = state.selected_node().map(|n| n.id.clone());
            lines.push(Line::from(""));
            match target {
                Some(id) => {
                    lines.push(Line::from(vec![
                        Span::styled(" No active session. Press ", Style::default().fg(theme.dark_gray)),
                        Span::styled("Enter", Style::default().fg(theme.accent_green).add_modifier(Modifier::BOLD)),
                        Span::styled(format!(" to connect to {}, or choose with F1-F9.", id), Style::default().fg(theme.dark_gray)),
                    ]));
                }
                None => {
                    lines.push(Line::from(Span::styled(" No servers configured.", Style::default().fg(theme.dark_gray))));
                }
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_approval(frame: &mut Frame, area: Rect, state: &FleetDialogState, theme: &Theme) {
    let Some(p) = state.pending_approval.as_ref() else { return };
    let block = Block::default()
        .borders(Borders::TOP)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent_yellow))
        .title(Span::styled(
            format!(" Corex wants to run on {}{} ", p.node_id, if p.sudo { " (sudo)" } else { "" }),
            Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD),
        ));

    let key = |k: &str, d: &str, c: Color| vec![
        Span::styled(format!(" {} ", k), Style::default().fg(c).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}  ", d), Style::default().fg(theme.gray)),
    ];
    let mut lines = Vec::new();
    if p.edit_mode {
        lines.push(Line::from(vec![
            Span::styled(" $ ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
            Span::styled(p.edited_command.clone(), Style::default().fg(theme.foreground)),
            Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_yellow)),
        ]));
        let mut l = key("Enter", "run edited", theme.accent_green);
        l.extend(key("Esc", "cancel", theme.gray));
        lines.push(Line::from(l));
    } else if p.instructions_mode {
        lines.push(Line::from(vec![
            Span::styled(" instruct › ", Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD)),
            Span::styled(p.instructions_text.clone(), Style::default().fg(theme.foreground)),
            Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_cyan)),
        ]));
        let mut l = key("Enter", "send", theme.accent_green);
        l.extend(key("Esc", "cancel", theme.gray));
        lines.push(Line::from(l));
    } else {
        lines.push(Line::from(vec![
            Span::styled(" $ ", Style::default().fg(theme.accent_yellow).add_modifier(Modifier::BOLD)),
            Span::styled(p.command.clone(), Style::default().fg(theme.foreground).add_modifier(Modifier::BOLD)),
        ]));
        let mut l = key("y", "allow", theme.accent_green);
        l.extend(key("e", "edit", theme.accent_yellow));
        l.extend(key("w", "instruct", theme.accent_cyan));
        l.extend(key("n", "deny", theme.accent_red));
        lines.push(Line::from(l));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_prompt(frame: &mut Frame, area: Rect, state: &FleetDialogState, theme: &Theme) {
    let prefix = match state.active_session.as_ref() {
        Some(s) => format!("{}:{}", s.node.id, s.current_dir),
        None => state.selected_node().map(|n| n.id.clone()).unwrap_or_else(|| "remote".to_string()),
    };
    let focused = state.command_mode;
    let mut spans = vec![
        Span::styled(format!(" {} $ ", prefix), Style::default().fg(if focused { theme.accent_cyan } else { theme.dark_gray }).add_modifier(Modifier::BOLD)),
        Span::styled(state.command_input.clone(), Style::default().fg(theme.foreground)),
    ];
    if focused {
        spans.push(Span::styled(crate::cursor::block(), Style::default().fg(theme.accent_yellow)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(frame: &mut Frame, area: Rect, state: &FleetDialogState, theme: &Theme) {
    let dim = Style::default().fg(theme.dark_gray);
    let key_style = Style::default().fg(theme.gray).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];

    let scroll_offset = state.pty_session.as_ref().map(|p| p.scrollback()).unwrap_or(state.terminal_scroll);
    if let Some(ref msg) = state.status_msg {
        spans.push(Span::styled(format!("{}  ", msg), Style::default().fg(theme.accent_yellow)));
    } else if scroll_offset > 0 {
        spans.push(Span::styled(
            format!("[↑ {}]  ", scroll_offset),
            Style::default().fg(theme.accent_cyan).add_modifier(Modifier::BOLD),
        ));
    }

    if state.pty_session.is_some() {
        if state.command_mode {
            spans.push(Span::styled("Shift+Tab", key_style));
            spans.push(Span::styled(" Chat  ·  ", dim));

            spans.push(Span::styled("Alt+1..9/n", key_style));
            spans.push(Span::styled(" Switch VPS  ·  ", dim));

            spans.push(Span::styled("Tab", key_style));
            spans.push(Span::styled(" AutoComplete  ·  ", dim));

            spans.push(Span::styled("exit", key_style));
            spans.push(Span::styled(" Disconnect", dim));
        } else {
            spans.push(Span::styled("Shift+Tab / Tab", key_style));
            spans.push(Span::styled(" VPS Terminal  ·  ", dim));

            spans.push(Span::styled("Esc", key_style));
            spans.push(Span::styled(" Close", dim));
        }
    } else if state.command_mode {
        spans.push(Span::styled("Tab", key_style));
        spans.push(Span::styled(" Complete/Chat  ·  ", dim));

        spans.push(Span::styled("Shift+Tab", key_style));
        spans.push(Span::styled(" Focus  ·  ", dim));

        spans.push(Span::styled("Esc", key_style));
        spans.push(Span::styled(" Servers  ·  ", dim));

        spans.push(Span::styled("PgUp/Dn", key_style));
        spans.push(Span::styled(" Scroll", dim));
    } else {
        spans.push(Span::styled("Tab", key_style));
        spans.push(Span::styled(" Terminal  ·  ", dim));

        spans.push(Span::styled("Esc", key_style));
        spans.push(Span::styled(" Close", dim));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn make_progress_bar(pct: u8, width: usize) -> String {
    let filled = ((pct as usize * width) / 100).min(width);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "■".repeat(filled), "·".repeat(empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_node(id: &str) -> NodeConfig {
        let mut node = NodeConfig::new(id, "10.0.0.1");
        node.name = Some(format!("Server {}", id));
        node.user = "root".to_string();
        node.tags = vec!["prod".to_string()];
        node
    }

    #[test]
    fn test_fleet_dialog_navigation() {
        let mut state = FleetDialogState::new();
        state.items = vec![
            FleetItem { config: dummy_node("srv1"), telemetry: None },
            FleetItem { config: dummy_node("srv2"), telemetry: None },
        ];

        assert_eq!(state.selected_idx, 0);
        assert_eq!(state.selected_node().unwrap().id, "srv1");

        state.select_next();
        assert_eq!(state.selected_idx, 1);
        assert_eq!(state.selected_node().unwrap().id, "srv2");

        // Clamp at end
        state.select_next();
        assert_eq!(state.selected_idx, 1);

        state.select_prev();
        assert_eq!(state.selected_idx, 0);
    }

    #[test]
    fn test_fleet_dialog_search_filter() {
        let mut state = FleetDialogState::new();
        let mut n1 = dummy_node("web-alpha");
        n1.tags = vec!["frontend".to_string()];
        let mut n2 = dummy_node("db-primary");
        n2.tags = vec!["database".to_string()];
        let mut n3 = dummy_node("web-beta");
        n3.tags = vec!["frontend".to_string()];

        state.items = vec![
            FleetItem { config: n1, telemetry: None },
            FleetItem { config: n2, telemetry: None },
            FleetItem { config: n3, telemetry: None },
        ];

        // All items match when search_query is empty
        assert_eq!(state.filtered_items().len(), 3);

        // Filter by id substring
        state.search_query = "web".to_string();
        let filtered = state.filtered_items();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].0, 0);
        assert_eq!(filtered[1].0, 2);

        // Navigation respects filtered items
        state.selected_idx = 0;
        state.select_next();
        assert_eq!(state.selected_idx, 2);
        state.select_next();
        assert_eq!(state.selected_idx, 2); // Clamps at end of filtered list
        state.select_prev();
        assert_eq!(state.selected_idx, 0);

        // Filter by tag
        state.search_query = "database".to_string();
        let db_filtered = state.filtered_items();
        assert_eq!(db_filtered.len(), 1);
        assert_eq!(db_filtered[0].0, 1);
    }

    #[test]
    fn test_fleet_dialog_pending_approval() {
        let mut state = FleetDialogState::new();
        assert!(state.pending_approval.is_none());

        state.pending_approval = Some(PendingRemoteApproval {
            call_id: "c1".to_string(),
            node_id: "srv1".to_string(),
            command: "systemctl restart nginx".to_string(),
            sudo: true,
            edit_mode: false,
            edited_command: "systemctl restart nginx".to_string(),
            instructions_mode: false,
            instructions_text: String::new(),
        });

        assert!(state.pending_approval.is_some());
        let p = state.pending_approval.as_ref().unwrap();
        assert_eq!(p.node_id, "srv1");
        assert_eq!(p.command, "systemctl restart nginx");
        assert!(p.sudo);
    }

    #[test]
    fn test_clean_terminal_line() {
        // 1. Carriage returns are resolved to the last part
        let cr_line = "old text\rnew text";
        assert_eq!(clean_terminal_line(cr_line, 50), "new text");

        // 2. ANSI color sequences and cursor movements are stripped
        let ansi_line = "\x1b[38;2;255;100;0mUbuntu 26.04\x1b[0m \x1b[10C(fastfetch)";
        assert_eq!(clean_terminal_line(ansi_line, 50), "Ubuntu 26.04 (fastfetch)");

        // 3. Max width clamping prevents overflowing adjacent panes
        let long_line = "a".repeat(100);
        assert_eq!(clean_terminal_line(&long_line, 40).len(), 40);
    }
}
