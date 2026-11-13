use ratatui::text::Line;

/// Reasoning stream of the current turn; the feed renders it as the collapsible "Thought" block.
pub struct ThinkingState {
    pub content: String,
    pub is_streaming: bool,
    pub is_expanded: bool,
    pub cached_lines: Vec<Line<'static>>,
    pub cached_len: usize,
    pub cached_width: usize,
}

impl Default for ThinkingState {
    fn default() -> Self {
        Self::new()
    }
}

impl ThinkingState {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            is_streaming: false,
            is_expanded: false,
            cached_lines: Vec::new(),
            cached_len: 0,
            cached_width: 0,
        }
    }

    pub fn reset(&mut self) {
        self.content.clear();
        self.is_streaming = false;
        self.cached_lines.clear();
        self.cached_len = 0;
        self.cached_width = 0;
    }
}
