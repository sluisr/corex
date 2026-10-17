use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use corex_fleet::NodeConfig;

/// A live, interactive pseudo-terminal session connected to a remote VPS via SSH.
/// Renders a real vt100 virtual terminal directly inside Ratatui.
pub struct FleetPtySession {
    pub node_id: String,
    pair: Mutex<PtyPair>,
    writer: Mutex<Box<dyn Write + Send>>,
    parser: Arc<Mutex<vt100::Parser>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    dirty: Arc<AtomicBool>,
}

impl Drop for FleetPtySession {
    fn drop(&mut self) {
        let mut child = self.child.lock();
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl FleetPtySession {
    /// Spawns an interactive native SSH session allocated with a remote pseudo-terminal (`ssh -tt`).
    pub fn spawn(node: &NodeConfig, rows: u16, cols: u16) -> anyhow::Result<Self> {
        let pty_system = NativePtySystem::default();
        let size = PtySize {
            rows: rows.max(8),
            cols: cols.max(30),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system.openpty(size)?;

        let mut cmd = CommandBuilder::new("ssh");
        cmd.arg("-tt"); // Forces pseudo-terminal allocation on the remote host
        cmd.arg("-o");
        cmd.arg("BatchMode=no");
        cmd.arg("-o");
        cmd.arg("ConnectTimeout=10");
        cmd.arg("-o");
        cmd.arg("StrictHostKeyChecking=accept-new");

        // SSH Multiplexing for near-zero latency reusing ControlPath
        if let Some(base) = directories::BaseDirs::new() {
            let ssh_dir = base.home_dir().join(".corex").join("ssh");
            let _ = corex_core::ensure_private_dir(&ssh_dir);
            let control_path = ssh_dir.join("cx-%r@%h:%p");
            cmd.arg("-o");
            cmd.arg("ControlMaster=auto");
            cmd.arg("-o");
            cmd.arg(format!("ControlPath={}", control_path.display()));
            cmd.arg("-o");
            cmd.arg("ControlPersist=10m");
        }

        if node.port != 22 {
            cmd.arg("-p");
            cmd.arg(node.port.to_string());
        }
        if let Some(ref key) = node.key_path {
            let expanded = {
                let s = key.to_string_lossy();
                if let Some(stripped) = s.strip_prefix("~/") {
                    if let Some(base) = directories::BaseDirs::new() {
                        base.home_dir().join(stripped)
                    } else {
                        key.clone()
                    }
                } else {
                    key.clone()
                }
            };
            cmd.arg("-i");
            cmd.arg(expanded.to_string_lossy().to_string());
        }
        if let Some(ref jump) = node.proxy_jump {
            let safe_jump = jump.trim().trim_start_matches('-');
            if !safe_jump.is_empty() {
                cmd.arg("-J");
                cmd.arg(safe_jump);
            }
        }

        let safe_user = node.user.trim().trim_start_matches('-');
        let safe_host = node.host.trim().trim_start_matches('-');
        cmd.arg("--");
        cmd.arg(format!("{}@{}", safe_user, safe_host));

        let child = pair.slave.spawn_command(cmd)?;
        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 1000)));
        let parser_clone = parser.clone();
        let dirty = Arc::new(AtomicBool::new(true));
        let dirty_clone = dirty.clone();

        std::thread::Builder::new()
            .name(format!("pty-{}", node.id))
            .spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            parser_clone.lock().process(&buf[..n]);
                            dirty_clone.store(true, Ordering::Release);
                        }
                        Err(_) => break,
                    }
                }
            })?;

        Ok(Self {
            node_id: node.id.clone(),
            pair: Mutex::new(pair),
            writer: Mutex::new(writer),
            parser,
            child: Mutex::new(child),
            dirty,
        })
    }

    /// Atomically returns true and clears the dirty flag if new PTY output arrived since last check.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    /// Sends raw bytes into the remote shell's stdin.
    pub fn write_bytes(&self, bytes: &[u8]) {
        let mut w = self.writer.lock();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    /// Feeds arbitrary text (with ANSI escape sequences) directly into the virtual terminal screen parser.
    pub fn feed_text(&self, text: &str) {
        let mut parser = self.parser.lock();
        parser.process(text.as_bytes());
        self.dirty.store(true, Ordering::Release);
    }

    /// Feeds raw bytes directly into the virtual terminal screen parser.
    pub fn feed_bytes(&self, bytes: &[u8]) {
        let mut parser = self.parser.lock();
        parser.process(bytes);
        self.dirty.store(true, Ordering::Release);
    }

    /// Scrolls the virtual terminal screen up into history.
    pub fn scroll_up(&self, rows: usize) {
        let mut parser = self.parser.lock();
        let cur = parser.screen().scrollback();
        parser.set_scrollback(cur + rows);
        self.dirty.store(true, Ordering::Release);
    }

    /// Scrolls the virtual terminal screen down towards the bottom.
    pub fn scroll_down(&self, rows: usize) {
        let mut parser = self.parser.lock();
        let cur = parser.screen().scrollback();
        parser.set_scrollback(cur.saturating_sub(rows));
        self.dirty.store(true, Ordering::Release);
    }

    /// Resets scrolling to the live bottom view.
    pub fn scroll_reset(&self) {
        let mut parser = self.parser.lock();
        parser.set_scrollback(0);
        self.dirty.store(true, Ordering::Release);
    }

    /// Returns the current scrollback offset.
    pub fn scrollback(&self) -> usize {
        let parser = self.parser.lock();
        parser.screen().scrollback()
    }

    /// Resizes the pseudo-terminal window to match the current split area.
    pub fn resize(&self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 {
            return;
        }
        {
            let p = self.parser.lock();
            let (cur_rows, cur_cols) = p.screen().size();
            if cur_rows == rows && cur_cols == cols {
                return;
            }
        }
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let _ = self.pair.lock().master.resize(size);
        self.parser.lock().set_size(rows, cols);
        self.dirty.store(true, Ordering::Release);
    }

    /// Checks whether the SSH process is still alive.
    pub fn is_alive(&self) -> bool {
        match self.child.lock().try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }

    /// Renders the current virtual terminal screen into the specified ratatui buffer area.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let parser = self.parser.lock();
        let screen = parser.screen();
        render_vt100_screen(screen, area, buf);
    }

    /// Gets the current virtual cursor position (row, col) if cursor is visible.
    pub fn cursor_position(&self) -> Option<(u16, u16)> {
        let parser = self.parser.lock();
        let screen = parser.screen();
        if screen.hide_cursor() || screen.scrollback() > 0 {
            None
        } else {
            Some(screen.cursor_position())
        }
    }
}

/// Renders a `vt100::Screen` onto a ratatui `Buffer`.
pub fn render_vt100_screen(screen: &vt100::Screen, area: Rect, buf: &mut Buffer) {
    let (rows, cols) = screen.size();
    let max_y = (area.height as usize).min(rows as usize);
    let max_x = (area.width as usize).min(cols as usize);

    for y in 0..max_y {
        for x in 0..max_x {
            if let Some(cell) = screen.cell(y as u16, x as u16) {
                let cell_buf = &mut buf[(area.x + x as u16, area.y + y as u16)];
                let text = cell.contents();
                if text.is_empty() {
                    cell_buf.set_char(' ');
                } else if let Some(ch) = text.chars().next() {
                    cell_buf.set_char(ch);
                }

                let mut style = Style::default();
                if let Some(fg) = map_vt100_color(cell.fgcolor()) {
                    style = style.fg(fg);
                }
                if let Some(bg) = map_vt100_color(cell.bgcolor()) {
                    style = style.bg(bg);
                }
                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                cell_buf.set_style(style);
            }
        }
    }
}

fn map_vt100_color(col: vt100::Color) -> Option<Color> {
    match col {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

/// Converts crossterm KeyEvent into raw bytes for the pseudo-terminal.
pub fn key_event_to_bytes(key: crossterm::event::KeyEvent) -> Option<Vec<u8>> {
    use crossterm::event::{KeyCode, KeyModifiers};

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if ctrl {
        return match key.code {
            KeyCode::Char(c) => {
                let lower = c.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    Some(vec![(lower as u8) - b'a' + 1])
                } else if c == '@' || c == ' ' {
                    Some(vec![0])
                } else if c == '[' {
                    Some(vec![27])
                } else if c == '\\' {
                    Some(vec![28])
                } else if c == ']' {
                    Some(vec![29])
                } else if c == '^' {
                    Some(vec![30])
                } else if c == '_' {
                    Some(vec![31])
                } else {
                    None
                }
            }
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char(c) => {
            if alt {
                let mut v = vec![0x1b];
                v.extend(c.to_string().into_bytes());
                Some(v)
            } else {
                Some(c.to_string().into_bytes())
            }
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]), // Standard ASCII DEL for backspace
        KeyCode::Tab => Some(vec![b'\t']),      // Tab character sent directly to remote bash!
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        _ => None,
    }
}
