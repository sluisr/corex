use corex_core::types::Message;
use ratatui::text::Line;

use crate::markdown::{code_blocks, is_copy_header, CodeBlock};

use super::App;

// Header rows show no number, so which block each one copies is kept beside the rendered lines.
#[derive(Debug, Default)]
pub struct CopyHeaders {
    pub cached: Vec<(usize, usize)>,
    pub tail: Vec<(usize, usize)>,
}

pub(crate) fn number_headers(lines: &[Line<'static>], base: usize, first_block: usize) -> Vec<(usize, usize)> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_copy_header(&super::formatting::line_plain_text(line)))
        .enumerate()
        .map(|(k, (idx, _))| (base + idx, first_block + k))
        .collect()
}

pub(crate) fn block_count(msg: &Message) -> usize {
    assistant_blocks(msg).len()
}

pub(crate) fn blocks_before(messages: &[Message], idx: usize) -> usize {
    messages[..idx.min(messages.len())].iter().map(block_count).sum()
}

fn assistant_blocks(msg: &Message) -> Vec<CodeBlock> {
    match (msg.role.as_str(), msg.text_content()) {
        ("assistant", Some(text)) => code_blocks(text),
        _ => Vec::new(),
    }
}

pub(crate) fn chat_blocks(messages: &[Message]) -> Vec<CodeBlock> {
    messages.iter().flat_map(assistant_blocks).collect()
}

pub(crate) const COPIED_FLASH: std::time::Duration = std::time::Duration::from_secs(2);

impl App {
    pub(crate) fn copy_number_at(&self, line_idx: usize) -> Option<usize> {
        let lines = &self.selection_lines;
        let idx = line_idx.checked_sub(lines.head.len())?;
        let (map, idx) = match idx.checked_sub(lines.cached.len()) {
            None => (&self.copy_headers.cached, idx),
            Some(tail_idx) => (&self.copy_headers.tail, tail_idx),
        };
        map.iter().find(|(line, _)| *line == idx).map(|(_, number)| *number)
    }

    pub(crate) fn copy_header_line(&self, number: usize) -> Option<usize> {
        let lines = &self.selection_lines;
        let find = |map: &[(usize, usize)]| map.iter().find(|(_, n)| *n == number).map(|(line, _)| *line);
        find(&self.copy_headers.cached)
            .map(|line| lines.head.len() + line)
            .or_else(|| find(&self.copy_headers.tail).map(|line| lines.head.len() + lines.cached.len() + line))
    }

    /// Copies block `number` (1-based), or the last one when it is `None`.
    pub(crate) fn copy_block(&mut self, number: Option<usize>) {
        let mut blocks = chat_blocks(self.session.display_messages());
        if self.is_streaming && self.streaming_md_committed_len > 0 {
            if let Some(committed) = self.streaming_text.get(..self.streaming_md_committed_len) {
                blocks.extend(code_blocks(committed));
            }
        }
        let picked = match number {
            None => blocks.last().map(|b| (blocks.len(), b.text.clone())),
            Some(n) => n.checked_sub(1).and_then(|i| blocks.get(i)).map(|b| (n, b.text.clone())),
        };
        match picked {
            Some((n, text)) => {
                self.copy_to_clipboard(&text);
                self.copied_block = Some((n, std::time::Instant::now()));
            }
            None if number.is_none() => {
                self.session.add_message(Message::system("No code blocks in this chat yet."));
                self.invalidate_message_cache();
                self.scroll_to_bottom();
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(text: &str) -> Message {
        Message { role: "assistant".to_string(), ..Message::user(text) }
    }

    #[test]
    fn blocks_are_numbered_across_the_chat_and_only_from_replies() {
        let messages = vec![
            Message::user("```\nnot mine\n```"),
            reply("```bash\nls\n```\nand\n```\npwd\n```"),
            Message::system("```\nsystem\n```"),
            reply("```\nwhoami\n```"),
        ];
        let texts: Vec<String> = chat_blocks(&messages).into_iter().map(|b| b.text).collect();
        assert_eq!(texts, vec!["ls", "pwd", "whoami"]);
        assert_eq!(blocks_before(&messages, 3), 2);
        assert_eq!(blocks_before(&messages, 99), 3);
    }

    #[test]
    fn closed_streaming_blocks_get_copy_button_while_open_block_waits() {
        let streamed = "```bash\nnmap -sS target\n```\n\n```bash\nffuf -u http://target";
        let split = crate::markdown::safe_markdown_split_point(streamed);
        let committed = &streamed[..split];
        let tail = &streamed[split..];

        let committed_lines = crate::markdown::render_markdown_with_copy(committed, &crate::theme::Theme::default(), 80, true);
        let tail_lines = crate::markdown::render_markdown(tail, &crate::theme::Theme::default(), 80);

        let committed_text: Vec<String> = committed_lines.iter().map(super::super::formatting::line_plain_text).collect();
        let tail_text: Vec<String> = tail_lines.iter().map(super::super::formatting::line_plain_text).collect();

        assert!(committed_text.iter().any(|l| l.contains("bash  ⧉ copy")), "{committed_text:#?}");
        assert!(!tail_text.iter().any(|l| l.contains("⧉ copy")), "{tail_text:#?}");
        assert_eq!(code_blocks(committed).len(), 1);
        assert_eq!(code_blocks(committed)[0].text, "nmap -sS target");
    }
}
