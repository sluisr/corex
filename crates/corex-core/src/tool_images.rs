//! Images produced by tools (e.g. `read_image`) waiting to reach the model.
//!
//! A `tool` message carries text only, and OpenAI-style APIs reject any other message between an
//! assistant turn's tool calls and their responses. So a tool parks its image here and the agent
//! loop, once *every* tool response of the turn is recorded, drains the queue into one `user`
//! message with image parts. That shape is accepted by every provider.

use std::sync::{Mutex, OnceLock};

use crate::types::{ContentPart, Message};

struct PendingImage {
    label: String,
    data_uri: String,
}

fn queue() -> &'static Mutex<Vec<PendingImage>> {
    static QUEUE: OnceLock<Mutex<Vec<PendingImage>>> = OnceLock::new();
    QUEUE.get_or_init(Default::default)
}

/// Parks an image (`data:image/...;base64,...`) to be shown to the model on its next turn.
pub fn push_tool_image(label: impl Into<String>, data_uri: impl Into<String>) {
    if let Ok(mut q) = queue().lock() {
        q.push(PendingImage {
            label: label.into(),
            data_uri: data_uri.into(),
        });
    }
}

/// Start of the text that labels the `user` message carrying tool images.
const NOTICE_PREFIX: &str = "[Image(s) loaded by the read_image tool:";

/// True for the bookkeeping `user` message built by [`drain_tool_images_message`]: the user did not
/// write it, so it is not shown in the feed nor counted as one of their requests.
pub fn is_tool_image_message(message: &Message) -> bool {
    message.role == "user"
        && message
            .display_text()
            .is_some_and(|text| text.starts_with(NOTICE_PREFIX))
}

/// Empties the queue into a single user message, or `None` when no tool produced an image.
/// Call it after all tool responses of the turn were appended and before the next model request.
pub fn drain_tool_images_message() -> Option<Message> {
    let pending = std::mem::take(&mut *queue().lock().ok()?);
    if pending.is_empty() {
        return None;
    }
    let names: Vec<&str> = pending.iter().map(|p| p.label.as_str()).collect();
    let mut parts = vec![ContentPart::text(format!("{} {}]", NOTICE_PREFIX, names.join(", ")))];
    parts.extend(pending.iter().map(|p| ContentPart::image_url(p.data_uri.clone())));
    Some(Message::user_parts(parts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_images_become_one_user_message_and_the_queue_empties() {
        // Drain anything left by parallel tests first; this test owns the assertions below.
        let _ = drain_tool_images_message();
        push_tool_image("a.png", "data:image/png;base64,AAAA");
        push_tool_image("b.png", "data:image/png;base64,BBBB");
        let msg = drain_tool_images_message().expect("a message");
        assert_eq!(msg.role, "user");
        assert!(is_tool_image_message(&msg));
        assert!(!is_tool_image_message(&Message::user("hello")));
        match msg.content {
            Some(crate::types::MessageContent::Parts(parts)) => assert_eq!(parts.len(), 3),
            _ => panic!("expected multipart content"),
        }
        assert!(drain_tool_images_message().is_none());
    }
}
