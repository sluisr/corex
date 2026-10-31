//! Anthropic Messages API client and protocol adapter.
//!
//! Provides conversion from Corex's canonical message/tool types to Anthropic's
//! `/v1/messages` JSON payload, and converts Anthropic SSE events back to `StreamEvent`.

use serde::{Deserialize, Serialize};
use crate::types::{Message, MessageContent, ToolDefinition, Usage};
use crate::client::StreamEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicTool>>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<AnthropicOutputConfig>,
    /// Top-level automatic prompt caching (`{"type":"ephemeral"}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicThinkingConfig {
    #[serde(rename = "type")]
    pub thinking_type: String, // "adaptive" | "enabled"
    /// Only for models that still take a fixed budget (`"enabled"`); rejected by 4.7+ models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicOutputConfig {
    pub effort: String,
}

/// How a given Claude model accepts the `thinking` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    /// Claude 3.x without thinking support.
    Unsupported,
    /// `{"type":"enabled","budget_tokens":N}` (Claude 3.7, 4.0–4.5, Haiku 4.5).
    Budget,
    /// `{"type":"adaptive"}` (Claude 4.6+, 5.x, Fable, Mythos).
    Adaptive,
}

/// Request parameters that depend on the Claude model generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelProfile {
    pub thinking: ThinkingMode,
    /// Whether `temperature` may be sent (removed on Claude 4.7+ / 5.x).
    pub accepts_temperature: bool,
    /// Whether `output_config.effort` is understood.
    pub accepts_effort: bool,
    /// Whether the `xhigh` effort level exists (4.7+).
    pub accepts_xhigh: bool,
    pub max_output_tokens: u32,
}

/// Classifies a Claude model id (`claude-opus-5-5`, `claude-sonnet-4-5-20250929`, …).
pub fn model_profile(model: &str) -> ModelProfile {
    let m = model.to_ascii_lowercase();
    if m.contains("claude-3") {
        return ModelProfile {
            thinking: if m.contains("claude-3-7") { ThinkingMode::Budget } else { ThinkingMode::Unsupported },
            accepts_temperature: true,
            accepts_effort: false,
            accepts_xhigh: false,
            max_output_tokens: 8192,
        };
    }
    // Claude 4.0–4.5 generation (dated ids such as `claude-sonnet-4-20250514` included).
    let gen4_early = ["-4-0", "-4-1", "-4-5", "-4-2025"]
        .iter()
        .any(|p| m.contains(p))
        && !["-4-6", "-4-7", "-4-8"].iter().any(|p| m.contains(p));
    if gen4_early {
        return ModelProfile {
            thinking: ThinkingMode::Budget,
            accepts_temperature: true,
            accepts_effort: m.contains("opus-4-5"),
            accepts_xhigh: false,
            max_output_tokens: 32_000,
        };
    }
    if m.contains("-4-6") {
        return ModelProfile {
            thinking: ThinkingMode::Adaptive,
            accepts_temperature: true,
            accepts_effort: true,
            accepts_xhigh: false,
            max_output_tokens: 64_000,
        };
    }
    // 4.7, 4.8, 5.x, Fable, Mythos and anything newer.
    ModelProfile {
        thinking: ThinkingMode::Adaptive,
        accepts_temperature: false,
        accepts_effort: true,
        accepts_xhigh: true,
        max_output_tokens: 64_000,
    }
}

/// Maps Corex's `reasoning_effort` setting onto an Anthropic effort level.
/// `"dynamic"` (and unknown values) leave the model default in place.
pub fn effort_for(profile: &ModelProfile, reasoning_effort: &str) -> Option<String> {
    if !profile.accepts_effort {
        return None;
    }
    let level = match reasoning_effort {
        "low" | "medium" | "high" | "max" => reasoning_effort,
        "xhigh" if profile.accepts_xhigh => "xhigh",
        "xhigh" => "high",
        _ => return None,
    };
    Some(level.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    pub role: String, // "user" | "assistant"
    pub content: Vec<AnthropicBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicBlock {
    /// Signed reasoning block. It must be replayed unchanged before the `tool_use` blocks it
    /// preceded, otherwise thinking-enabled requests with tool results are rejected.
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// Where an Anthropic image block takes its pixels from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicImageSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },
    #[serde(rename = "url")]
    Url { url: String },
}

/// Maps a user message's content to Anthropic blocks. Plain text stays one text block; multipart
/// content keeps its images (`data:` URIs inline, `http(s)` URLs by reference) instead of dropping them.
fn user_blocks(content: Option<&MessageContent>) -> Vec<AnthropicBlock> {
    match content {
        Some(MessageContent::Parts(parts)) => {
            let blocks: Vec<AnthropicBlock> = parts
                .iter()
                .filter_map(|part| {
                    if let Some(img) = &part.image_url {
                        let source = match img.url.strip_prefix("data:").and_then(|r| r.split_once(";base64,")) {
                            Some((media_type, data)) => AnthropicImageSource::Base64 {
                                media_type: media_type.to_string(),
                                data: data.to_string(),
                            },
                            None => AnthropicImageSource::Url { url: img.url.clone() },
                        };
                        Some(AnthropicBlock::Image { source })
                    } else {
                        part.text.clone().map(|text| AnthropicBlock::Text { text })
                    }
                })
                .collect();
            if blocks.is_empty() {
                vec![AnthropicBlock::Text { text: String::new() }]
            } else {
                blocks
            }
        }
        Some(MessageContent::Text(text)) => vec![AnthropicBlock::Text { text: text.clone() }],
        None => vec![AnthropicBlock::Text { text: String::new() }],
    }
}

/// Key under a tool call's `extra_content` holding the turn's signed thinking blocks.
pub const THINKING_EXTRA_KEY: &str = "anthropic_thinking";

fn is_task_report(m: &Message) -> bool {
    m.text_content()
        .is_some_and(|t| t.starts_with("TASK_DONE|") || t.starts_with("SUBAGENT_DONE|"))
}

/// Converts Corex messages and tool definitions into Anthropic's Messages API format.
pub fn convert_messages_and_tools(
    messages: &[Message],
    tools: Option<&[ToolDefinition]>,
) -> (Option<String>, Vec<AnthropicMessage>, Option<Vec<AnthropicTool>>) {
    // Anthropic takes the system prompt as a top-level `system` string, not as a message.
    let mut system_parts = Vec::new();
    let mut non_system = Vec::new();
    for m in messages {
        if m.role == "system" && !is_task_report(m) {
            if let Some(txt) = m.text_content() {
                if !txt.trim().is_empty() {
                    system_parts.push(txt.trim());
                }
            }
        } else {
            non_system.push(m);
        }
    }
    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    let mut anthropic_messages: Vec<AnthropicMessage> = Vec::new();

    for m in non_system {
        match m.role.as_str() {
            "system" => {
                let block = AnthropicBlock::Text {
                    text: format!("<system-notification>\n{}\n</system-notification>", m.text_content().unwrap_or("").trim_end()),
                };
                if let Some(last) = anthropic_messages.last_mut() {
                    if last.role == "user" {
                        last.content.push(block);
                        continue;
                    }
                }
                anthropic_messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: vec![block],
                });
            }
            "user" => {
                let blocks = user_blocks(m.content.as_ref());
                // Group consecutive user turns into a single user message
                if let Some(last) = anthropic_messages.last_mut() {
                    if last.role == "user" {
                        last.content.extend(blocks);
                        continue;
                    }
                }
                anthropic_messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: blocks,
                });
            }
            "tool" => {
                let call_id = m.tool_call_id.clone().unwrap_or_default();
                let content = m.text_content().unwrap_or("").to_string();
                let is_error = if content.starts_with("Error") || content.starts_with("error:") {
                    Some(true)
                } else {
                    None
                };
                let block = AnthropicBlock::ToolResult {
                    tool_use_id: call_id,
                    content,
                    is_error,
                };
                // Anthropic requires tool results in a `user` message. Group consecutive tool results together.
                if let Some(last) = anthropic_messages.last_mut() {
                    if last.role == "user" {
                        let pos = last
                            .content
                            .iter()
                            .position(|b| !matches!(b, AnthropicBlock::ToolResult { .. }))
                            .unwrap_or(last.content.len());
                        last.content.insert(pos, block);
                        continue;
                    }
                }
                anthropic_messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: vec![block],
                });
            }
            "assistant" => {
                let mut blocks = Vec::new();
                // Replay the signed thinking blocks captured with this turn's tool calls. Every
                // call of the turn carries the same copy, so only the first one is used.
                if let Some(saved) = m
                    .tool_calls
                    .as_ref()
                    .and_then(|calls| {
                        calls.iter().find_map(|c| {
                            c.extra_content
                                .as_ref()
                                .and_then(|ec| ec.get(THINKING_EXTRA_KEY))
                        })
                    })
                    .and_then(|v| v.as_array())
                {
                    blocks.extend(
                        saved
                            .iter()
                            .filter_map(|b| serde_json::from_value::<AnthropicBlock>(b.clone()).ok())
                            .filter(|b| matches!(b, AnthropicBlock::Thinking { .. } | AnthropicBlock::RedactedThinking { .. })),
                    );
                }
                if let Some(text) = m.text_content() {
                    if !text.is_empty() {
                        blocks.push(AnthropicBlock::Text { text: text.to_string() });
                    }
                }
                if let Some(ref calls) = m.tool_calls {
                    for c in calls {
                        let input_val: serde_json::Value = crate::parse_lenient_json(&c.function.arguments)
                            .unwrap_or_else(|_| serde_json::json!({}));
                        blocks.push(AnthropicBlock::ToolUse {
                            id: c.id.clone(),
                            name: c.function.name.clone(),
                            input: input_val,
                        });
                    }
                }
                if blocks.is_empty() {
                    blocks.push(AnthropicBlock::Text { text: "...".to_string() });
                }
                if let Some(last) = anthropic_messages.last_mut() {
                    if last.role == "assistant" {
                        last.content.extend(blocks);
                        continue;
                    }
                }
                anthropic_messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: blocks,
                });
            }
            _ => {}
        }
    }

    // Anthropic API requirement: Conversation MUST start with a user message.
    if anthropic_messages.is_empty() {
        anthropic_messages.push(AnthropicMessage {
            role: "user".to_string(),
            content: vec![AnthropicBlock::Text { text: "Hello".to_string() }],
        });
    } else if anthropic_messages[0].role != "user" {
        anthropic_messages.insert(0, AnthropicMessage {
            role: "user".to_string(),
            content: vec![AnthropicBlock::Text { text: "...".to_string() }],
        });
    }

    // Convert tool definitions to Anthropic format
    let anthropic_tools = tools.and_then(|t_list| {
        let valid_tools: Vec<AnthropicTool> = t_list
            .iter()
            .map(|t| AnthropicTool {
                name: t.function.name.clone(),
                description: t.function.description.clone(),
                input_schema: t.function.parameters.clone(),
            })
            .collect();
        if valid_tools.is_empty() { None } else { Some(valid_tools) }
    });

    (system, anthropic_messages, anthropic_tools)
}

// ─── SSE Event Structures ───────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicSseData {
    #[serde(rename = "message_start")]
    MessageStart {
        message: AnthropicMessageStartInfo,
    },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: AnthropicBlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        index: usize,
        delta: AnthropicDelta,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {
        index: usize,
    },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: AnthropicMessageDeltaInfo,
        usage: Option<AnthropicUsageInfo>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error {
        error: AnthropicErrorInfo,
    },
    /// Event types added to the API after this adapter was written.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageStartInfo {
    pub id: String,
    pub model: String,
    pub usage: Option<AnthropicUsageInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicUsageInfo {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// Prompt tokens served from the cache (not included in `input_tokens`).
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Prompt tokens written to the cache (not included in `input_tokens`).
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

impl AnthropicUsageInfo {
    /// Total prompt size: Anthropic reports cached and uncached input separately.
    fn total_prompt_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_input_tokens + self.cache_creation_input_tokens
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicBlockStart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageDeltaInfo {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicErrorInfo {
    pub message: String,
}

/// Per-response parsing state for an Anthropic SSE stream.
///
/// Besides the running token usage it reassembles the signed `thinking` blocks (text plus
/// `signature_delta`) so they can be attached to the turn's tool calls and replayed later.
#[derive(Debug, Default)]
pub struct AnthropicStreamState {
    pub usage: Usage,
    open_thinking: std::collections::BTreeMap<usize, AnthropicBlock>,
    finished_thinking: Vec<serde_json::Value>,
    completed: bool,
}

impl AnthropicStreamState {
    pub fn new() -> Self {
        Self::default()
    }

    fn apply_usage(&mut self, u: &AnthropicUsageInfo, from_message_start: bool) {
        // `message_start` carries the prompt side; `message_delta` carries cumulative output
        // (and, on newer API versions, may repeat the prompt side).
        let prompt = u.total_prompt_tokens();
        if from_message_start || prompt > 0 {
            self.usage.prompt_tokens = prompt;
            self.usage.prompt_cache_hit_tokens = u.cache_read_input_tokens;
            self.usage.prompt_cache_miss_tokens = u.input_tokens + u.cache_creation_input_tokens;
        }
        if !from_message_start || u.output_tokens > 0 {
            self.usage.completion_tokens = u.output_tokens;
        }
        self.usage.total_tokens = self.usage.prompt_tokens + self.usage.completion_tokens;
    }
}

/// Parses one Anthropic SSE event string into Corex `StreamEvent`s.
pub fn parse_anthropic_sse_data(
    data: &str,
    state: &mut AnthropicStreamState,
) -> Option<Vec<StreamEvent>> {
    let parsed: AnthropicSseData = serde_json::from_str(data).ok()?;
    let mut events = Vec::new();
    match parsed {
        AnthropicSseData::MessageStart { message } => {
            if let Some(u) = message.usage {
                state.apply_usage(&u, true);
                events.push(StreamEvent::UsageUpdate(state.usage.clone()));
            }
        }
        AnthropicSseData::ContentBlockStart { index, content_block } => {
            match content_block {
                AnthropicBlockStart::ToolUse { id, name } => {
                    // Thinking blocks always precede the tool_use blocks they justify.
                    // Take rather than clone so multiple tool_use blocks in the same turn do not
                    // duplicate the signed thinking payload across every ToolCall.
                    let extra_content = if state.finished_thinking.is_empty() {
                        None
                    } else {
                        Some(serde_json::json!({
                            THINKING_EXTRA_KEY: std::mem::take(&mut state.finished_thinking)
                        }))
                    };
                    events.push(StreamEvent::ToolCallDelta {
                        index,
                        id: Some(id),
                        name: Some(name),
                        arguments: None,
                        extra_content,
                    });
                }
                AnthropicBlockStart::Text { text } => {
                    if !text.is_empty() {
                        events.push(StreamEvent::ContentDelta(text));
                    }
                }
                AnthropicBlockStart::Thinking { thinking, signature } => {
                    if !thinking.is_empty() {
                        events.push(StreamEvent::ReasoningDelta(thinking.clone()));
                    }
                    state
                        .open_thinking
                        .insert(index, AnthropicBlock::Thinking { thinking, signature });
                }
                AnthropicBlockStart::RedactedThinking { data } => {
                    state
                        .open_thinking
                        .insert(index, AnthropicBlock::RedactedThinking { data });
                }
                AnthropicBlockStart::Unknown => {}
            }
        }
        AnthropicSseData::ContentBlockDelta { index, delta } => {
            match delta {
                AnthropicDelta::TextDelta { text } => {
                    if !text.is_empty() {
                        events.push(StreamEvent::ContentDelta(text));
                    }
                }
                AnthropicDelta::ThinkingDelta { thinking } => {
                    if let Some(AnthropicBlock::Thinking { thinking: acc, .. }) =
                        state.open_thinking.get_mut(&index)
                    {
                        acc.push_str(&thinking);
                    }
                    if !thinking.is_empty() {
                        events.push(StreamEvent::ReasoningDelta(thinking));
                    }
                }
                AnthropicDelta::SignatureDelta { signature } => {
                    if let Some(AnthropicBlock::Thinking { signature: acc, .. }) =
                        state.open_thinking.get_mut(&index)
                    {
                        acc.push_str(&signature);
                    }
                }
                AnthropicDelta::InputJsonDelta { partial_json } => {
                    events.push(StreamEvent::ToolCallDelta {
                        index,
                        id: None,
                        name: None,
                        arguments: Some(partial_json),
                        extra_content: None,
                    });
                }
                AnthropicDelta::Unknown => {}
            }
        }
        AnthropicSseData::ContentBlockStop { index } => {
            if let Some(block) = state.open_thinking.remove(&index) {
                if let Ok(v) = serde_json::to_value(&block) {
                    state.finished_thinking.push(v);
                }
            }
        }
        AnthropicSseData::MessageDelta { delta, usage } => {
            if let Some(u) = usage {
                state.apply_usage(&u, false);
                events.push(StreamEvent::UsageUpdate(state.usage.clone()));
            }
            if let Some(reason) = delta.stop_reason {
                let normalized = match reason.as_str() {
                    "end_turn" | "stop_sequence" => "stop",
                    "tool_use" => "tool_calls",
                    // Same meaning as OpenAI's "length": output was cut by max_tokens.
                    "max_tokens" | "model_context_window_exceeded" => "length",
                    other => other,
                };
                state.completed = true;
                events.push(StreamEvent::Completed { finish_reason: Some(normalized.to_string()) });
            }
        }
        AnthropicSseData::MessageStop => {
            if !state.completed {
                state.completed = true;
                events.push(StreamEvent::Completed { finish_reason: None });
            }
        }
        AnthropicSseData::Error { error } => {
            events.push(StreamEvent::Error(error.message));
        }
        AnthropicSseData::Ping | AnthropicSseData::Unknown => {}
    }
    Some(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    #[test]
    fn test_user_images_are_sent_as_image_blocks_after_tool_results() {
        let messages = vec![
            Message::user("look"),
            Message::assistant_with_tools(
                None,
                None,
                vec![ToolCall::new("c1", "read_image", r#"{"file_path":"a.png"}"#)],
            ),
            Message::tool_response("c1", "Loaded a.png"),
            Message::user_parts(vec![
                ContentPart::text("[Image(s) loaded]"),
                ContentPart::image_url("data:image/png;base64,QUJD"),
            ]),
        ];
        let (_, msgs, _) = convert_messages_and_tools(&messages, None);
        let last = msgs.last().unwrap();
        assert_eq!(last.role, "user");
        assert!(matches!(last.content[0], AnthropicBlock::ToolResult { .. }));
        match &last.content[2] {
            AnthropicBlock::Image { source: AnthropicImageSource::Base64 { media_type, data } } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "QUJD");
            }
            other => panic!("expected an image block, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_messages_groups_tool_results_into_user_turn() {
        let messages = vec![
            Message::system("You are a helpful assistant."),
            Message::user("Please list files."),
            Message::assistant_with_tools(
                Some("Running command".to_string()),
                None,
                vec![ToolCall::new("call_abc123", "run_shell_command", r#"{"command":"ls"}"#)],
            ),
            Message::tool_response("call_abc123", "Cargo.toml\nsrc"),
        ];

        let tools = vec![ToolDefinition::function(
            "run_shell_command",
            "Runs a shell command",
            serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"]
            }),
        )];

        let (system, anthropic_msgs, anthropic_tools) = convert_messages_and_tools(&messages, Some(&tools));

        assert_eq!(system.as_deref(), Some("You are a helpful assistant."));
        assert_eq!(anthropic_msgs.len(), 3); // user, assistant, user (tool result)

        // Turn 1: user
        assert_eq!(anthropic_msgs[0].role, "user");
        // Turn 2: assistant with text and tool_use block
        assert_eq!(anthropic_msgs[1].role, "assistant");
        assert_eq!(anthropic_msgs[1].content.len(), 2);
        // Turn 3: user with tool_result block
        assert_eq!(anthropic_msgs[2].role, "user");
        match &anthropic_msgs[2].content[0] {
            AnthropicBlock::ToolResult { tool_use_id, content, .. } => {
                assert_eq!(tool_use_id, "call_abc123");
                assert_eq!(content, "Cargo.toml\nsrc");
            }
            _ => panic!("Expected ToolResult block"),
        }

        // Tools converted
        assert!(anthropic_tools.is_some());
        let t_list = anthropic_tools.unwrap();
        assert_eq!(t_list.len(), 1);
        assert_eq!(t_list[0].name, "run_shell_command");
    }

    #[test]
    fn test_task_report_stays_in_place_as_a_user_turn() {
        let messages = vec![
            Message::system("You are a helpful assistant."),
            Message::user("Build it."),
            Message::assistant("Building in the background.".to_string(), None),
            Message::system("TASK_DONE|42|0|31.2s\nFinished release"),
        ];
        let (system, msgs, _) = convert_messages_and_tools(&messages, None);
        assert_eq!(system.as_deref(), Some("You are a helpful assistant."));
        let last = msgs.last().unwrap();
        assert_eq!(last.role, "user");
        match &last.content[0] {
            AnthropicBlock::Text { text } => assert!(text.contains("TASK_DONE|42|0|31.2s")),
            other => panic!("expected a text block, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_result_goes_before_a_task_report_added_while_it_ran() {
        let messages = vec![
            Message::user("Check."),
            Message::assistant_with_tools(None, None, vec![ToolCall::new("c1", "run_shell_command", r#"{"command":"ls"}"#)]),
            Message::system("TASK_DONE|42|1|3s\nerror"),
            Message::tool_response("c1", "src"),
        ];
        let (_, msgs, _) = convert_messages_and_tools(&messages, None);
        let last = msgs.last().unwrap();
        assert!(matches!(last.content[0], AnthropicBlock::ToolResult { .. }));
        assert!(matches!(last.content[1], AnthropicBlock::Text { .. }));
    }

    #[test]
    fn test_parse_anthropic_sse_stream() {
        let mut usage = AnthropicStreamState::new();

        // 1. message_start
        let start_json = r#"{"type":"message_start","message":{"id":"msg_01","type":"message","role":"assistant","model":"claude-3-5-sonnet","usage":{"input_tokens":120,"output_tokens":0}}}"#;
        let evs = parse_anthropic_sse_data(start_json, &mut usage).unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            StreamEvent::UsageUpdate(u) => assert_eq!(u.prompt_tokens, 120),
            _ => panic!("Expected UsageUpdate"),
        }

        // 2. content_block_delta (text)
        let text_json = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello world!"}}"#;
        let evs = parse_anthropic_sse_data(text_json, &mut usage).unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            StreamEvent::ContentDelta(t) => assert_eq!(t, "Hello world!"),
            _ => panic!("Expected ContentDelta"),
        }

        // 3. content_block_start (tool_use)
        let tool_start_json = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_tool_1","name":"read_file"}}"#;
        let evs = parse_anthropic_sse_data(tool_start_json, &mut usage).unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            StreamEvent::ToolCallDelta { id, name, .. } => {
                assert_eq!(id.as_deref(), Some("call_tool_1"));
                assert_eq!(name.as_deref(), Some("read_file"));
            }
            _ => panic!("Expected ToolCallDelta"),
        }

        // 4. content_block_delta (tool input chunk)
        let tool_chunk_json = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"Cargo.toml\"}"}}"#;
        let evs = parse_anthropic_sse_data(tool_chunk_json, &mut usage).unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            StreamEvent::ToolCallDelta { arguments, .. } => {
                assert_eq!(arguments.as_deref(), Some("{\"path\":\"Cargo.toml\"}"));
            }
            _ => panic!("Expected ToolCallDelta arguments"),
        }

        // 5. message_delta
        let delta_json = r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":0,"output_tokens":45}}"#;
        let evs = parse_anthropic_sse_data(delta_json, &mut usage).unwrap();
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            StreamEvent::UsageUpdate(u) => assert_eq!(u.completion_tokens, 45),
            _ => panic!("Expected UsageUpdate"),
        }
        match &evs[1] {
            StreamEvent::Completed { finish_reason } => assert_eq!(finish_reason.as_deref(), Some("tool_calls")),
            _ => panic!("Expected Completed"),
        }
    }

    fn feed(state: &mut AnthropicStreamState, chunks: &[&str]) -> Vec<StreamEvent> {
        chunks
            .iter()
            .flat_map(|c| parse_anthropic_sse_data(c, state).unwrap_or_default())
            .collect()
    }

    #[test]
    fn test_signed_thinking_is_attached_to_tool_calls_and_replayed() {
        let mut state = AnthropicStreamState::new();
        let events = feed(&mut state, &[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Need the file."}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-abc"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file"}}"#,
        ]);
        let extra = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::ToolCallDelta { extra_content: Some(ec), .. } => Some(ec.clone()),
                _ => None,
            })
            .expect("tool call carries the thinking block");

        let mut call = ToolCall::new("toolu_1", "read_file", r#"{"file_path":"a.rs"}"#);
        call.extra_content = Some(extra);
        let messages = vec![
            Message::user("read a.rs"),
            Message::assistant_with_tools(None, None, vec![call]),
            Message::tool_response("toolu_1", "fn main() {}"),
        ];
        let (_, converted, _) = convert_messages_and_tools(&messages, None);
        match &converted[1].content[0] {
            AnthropicBlock::Thinking { thinking, signature } => {
                assert_eq!(thinking, "Need the file.");
                assert_eq!(signature, "sig-abc");
            }
            other => panic!("thinking must come first, got {:?}", other),
        }
        assert!(matches!(converted[1].content[1], AnthropicBlock::ToolUse { .. }));
    }

    #[test]
    fn test_cached_prompt_tokens_are_counted() {
        let mut state = AnthropicStreamState::new();
        feed(&mut state, &[
            r#"{"type":"message_start","message":{"id":"m","model":"claude-opus-5-5","usage":{"input_tokens":50,"cache_read_input_tokens":9000,"cache_creation_input_tokens":200,"output_tokens":1}}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":40}}"#,
        ]);
        assert_eq!(state.usage.prompt_tokens, 9250);
        assert_eq!(state.usage.prompt_cache_hit_tokens, 9000);
        assert_eq!(state.usage.completion_tokens, 40);
        assert_eq!(state.usage.total_tokens, 9290);
    }

    #[test]
    fn test_max_tokens_stop_is_reported_as_length() {
        let mut state = AnthropicStreamState::new();
        let events = feed(&mut state, &[
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":64000}}"#,
        ]);
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::Completed { finish_reason: Some(r) } if r == "length"
        )));
    }

    #[test]
    fn test_unknown_event_types_do_not_break_parsing() {
        let mut state = AnthropicStreamState::new();
        assert!(parse_anthropic_sse_data(r#"{"type":"some_future_event","x":1}"#, &mut state).is_some());
        let evs = feed(&mut state, &[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"future_block"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"future_delta"}}"#,
        ]);
        assert!(evs.is_empty());
    }

    #[test]
    fn test_model_profiles_match_api_generations() {
        let opus55 = model_profile("claude-opus-5-5");
        assert_eq!(opus55.thinking, ThinkingMode::Adaptive);
        assert!(!opus55.accepts_temperature);
        assert_eq!(effort_for(&opus55, "xhigh").as_deref(), Some("xhigh"));
        assert_eq!(effort_for(&opus55, "dynamic"), None);

        let sonnet45 = model_profile("claude-sonnet-4-5-20250929");
        assert_eq!(sonnet45.thinking, ThinkingMode::Budget);
        assert!(sonnet45.accepts_temperature);
        assert_eq!(effort_for(&sonnet45, "high"), None);

        let opus46 = model_profile("claude-opus-4-6");
        assert_eq!(opus46.thinking, ThinkingMode::Adaptive);
        assert_eq!(effort_for(&opus46, "xhigh").as_deref(), Some("high"));

        let haiku35 = model_profile("claude-3-5-haiku-20241022");
        assert_eq!(haiku35.thinking, ThinkingMode::Unsupported);
        assert_eq!(haiku35.max_output_tokens, 8192);
    }

    #[test]
    fn tool_arguments_are_repaired_and_completed_is_sent_once() {
        let mut call1 = ToolCall::new("c1", "read_file", r#"{"file_path": "a.rs",}"#);
        call1.extra_content = Some(serde_json::json!({"other": true}));
        let mut call2 = ToolCall::new("c2", "read_file", r#"{"file_path": "b.rs"}"#);
        call2.extra_content = Some(serde_json::json!({
            THINKING_EXTRA_KEY: [{"type": "thinking", "thinking": "step", "signature": "sig"}]
        }));

        let messages = vec![
            Message::user("check files"),
            Message::assistant_with_tools(None, None, vec![call1, call2]),
            Message::tool_response("c1", "ok1"),
            Message::tool_response("c2", "ok2"),
        ];
        let (_, converted, _) = convert_messages_and_tools(&messages, None);
        assert!(matches!(converted[1].content[0], AnthropicBlock::Thinking { .. }));
        match &converted[1].content[1] {
            AnthropicBlock::ToolUse { input, .. } => {
                assert_eq!(input.get("file_path").and_then(|v| v.as_str()), Some("a.rs"));
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }

        let mut state = AnthropicStreamState::new();
        let events = feed(
            &mut state,
            &[
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":10}}"#,
                r#"{"type":"message_stop"}"#,
            ],
        );
        let completed_count = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Completed { .. }))
            .count();
        assert_eq!(completed_count, 1, "must emit Completed only once per stream");
    }
}
