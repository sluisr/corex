/// Reasoning stream of the current turn; the feed renders it as the collapsible "Thought" block.
pub struct ThinkingState {
    pub content: String,
    pub is_streaming: bool,
    pub is_expanded: bool,
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
        }
    }

    pub fn reset(&mut self) {
        self.content.clear();
        self.is_streaming = false;
    }
}
