//! Anthropic Messages API client and protocol adapter.
//!
//! Provides conversion from Corex's canonical message/tool types to Anthropic's
//! `/v1/messages` JSON payload, and converts Anthropic SSE events back to `StreamEvent`.

use serde::{Deserialize, Serialize};
use crate::types::{Message, ToolDefinition, Usage};
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicThinkingConfig {
    #[serde(rename = "type")]
    pub thinking_type: String, // "enabled"
    pub budget_tokens: u32,
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
    #[serde(rename = "text")]
    Text { text: String },
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

/// Converts Corex messages and tool definitions into Anthropic's Messages API format.
pub fn convert_messages_and_tools(
    messages: &[Message],
    tools: Option<&[ToolDefinition]>,
) -> (Option<String>, Vec<AnthropicMessage>, Option<Vec<AnthropicTool>>) {
    // 1. Extract system prompt (Anthropic accepts top-level `system` string)
    let mut system_parts = Vec::new();
    let mut non_system = Vec::new();
    for m in messages {
        if m.role == "system" {
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

    // 2. Convert conversation turns
    let mut anthropic_messages: Vec<AnthropicMessage> = Vec::new();

    for m in non_system {
        match m.role.as_str() {
            "user" => {
                let text = m.text_content().unwrap_or("").to_string();
                let block = AnthropicBlock::Text { text };
                // Group consecutive user turns into a single user message
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
                        last.content.push(block);
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
                if let Some(text) = m.text_content() {
                    if !text.is_empty() {
                        blocks.push(AnthropicBlock::Text { text: text.to_string() });
                    }
                }
                if let Some(ref calls) = m.tool_calls {
                    for c in calls {
                        let input_val: serde_json::Value = serde_json::from_str(&c.function.arguments)
                            .unwrap_or_else(|_| serde_json::json!({}));
                        blocks.push(AnthropicBlock::ToolUse {
                            id: c.id.clone(),
                            name: c.function.name.clone(),
                            input: input_val,
                        });
                    }
                }
                if blocks.is_empty() {
                    blocks.push(AnthropicBlock::Text { text: String::new() });
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicBlockStart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageDeltaInfo {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicErrorInfo {
    pub message: String,
}

/// Parses an Anthropic SSE event string into Corex `StreamEvent`s.
pub fn parse_anthropic_sse_data(
    data: &str,
    current_usage: &mut Usage,
) -> Option<Vec<StreamEvent>> {
    let parsed: AnthropicSseData = serde_json::from_str(data).ok()?;
    let mut events = Vec::new();
    match parsed {
        AnthropicSseData::MessageStart { message } => {
            if let Some(u) = message.usage {
                current_usage.prompt_tokens = u.input_tokens;
                current_usage.total_tokens = current_usage.prompt_tokens + current_usage.completion_tokens;
                events.push(StreamEvent::UsageUpdate(current_usage.clone()));
            }
        }
        AnthropicSseData::ContentBlockStart { index, content_block } => {
            match content_block {
                AnthropicBlockStart::ToolUse { id, name } => {
                    events.push(StreamEvent::ToolCallDelta {
                        index,
                        id: Some(id),
                        name: Some(name),
                        arguments: None,
                        extra_content: None,
                    });
                }
                AnthropicBlockStart::Text { text } => {
                    if !text.is_empty() {
                        events.push(StreamEvent::ContentDelta(text));
                    }
                }
                AnthropicBlockStart::Thinking { thinking } => {
                    if !thinking.is_empty() {
                        events.push(StreamEvent::ReasoningDelta(thinking));
                    }
                }
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
                    if !thinking.is_empty() {
                        events.push(StreamEvent::ReasoningDelta(thinking));
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
            }
        }
        AnthropicSseData::MessageDelta { delta, usage } => {
            if let Some(u) = usage {
                current_usage.completion_tokens = u.output_tokens;
                current_usage.total_tokens = current_usage.prompt_tokens + current_usage.completion_tokens;
                events.push(StreamEvent::UsageUpdate(current_usage.clone()));
            }
            if let Some(reason) = delta.stop_reason {
                let normalized = match reason.as_str() {
                    "end_turn" => "stop",
                    "tool_use" => "tool_calls",
                    other => other,
                };
                events.push(StreamEvent::Completed { finish_reason: Some(normalized.to_string()) });
            }
        }
        AnthropicSseData::MessageStop => {
            events.push(StreamEvent::Completed { finish_reason: None });
        }
        AnthropicSseData::Error { error } => {
            events.push(StreamEvent::Error(error.message));
        }
        AnthropicSseData::ContentBlockStop { .. } | AnthropicSseData::Ping => {}
    }
    Some(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

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
    fn test_parse_anthropic_sse_stream() {
        let mut usage = Usage::default();

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
}
