use crate::client::StreamEvent;
use crate::types::{Message, MessageContent, ToolDefinition, Usage};
use serde_json::{json, Value};

/// Encrypted reasoning items kept in `extra_content`, sent back so a tool loop keeps its reasoning.
pub const REASONING_EXTRA_KEY: &str = "responses_reasoning";

pub fn uses_responses_api(kind: crate::providers::ProviderKind, model: &str) -> bool {
    if kind != crate::providers::ProviderKind::Azure {
        return false;
    }
    let m = model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase();
    m.starts_with("gpt-6")
        || m.starts_with("gpt-5")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
}

pub fn build_responses_request(
    model: &str,
    messages: &[Message],
    tools: Option<&[ToolDefinition]>,
    reasoning_effort: &str,
) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut input: Vec<Value> = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => {
                if let Some(text) = msg.text_content() {
                    if !text.trim().is_empty() {
                        system_parts.push(text.to_string());
                    }
                }
            }
            "user" => {
                let mut parts: Vec<Value> = Vec::new();
                match &msg.content {
                    Some(MessageContent::Text(t)) => {
                        if !t.is_empty() {
                            parts.push(json!({ "type": "input_text", "text": t }));
                        }
                    }
                    Some(MessageContent::Parts(list)) => {
                        for p in list {
                            match p.part_type.as_str() {
                                "text" => {
                                    if let Some(text) = &p.text {
                                        if !text.is_empty() {
                                            parts.push(json!({ "type": "input_text", "text": text }));
                                        }
                                    }
                                }
                                "image_url" => {
                                    if let Some(image_url) = &p.image_url {
                                        parts.push(json!({
                                            "type": "input_image",
                                            "image_url": image_url.url
                                        }));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    None => {}
                }
                if parts.is_empty() {
                    parts.push(json!({ "type": "input_text", "text": "" }));
                }
                input.push(json!({ "role": "user", "content": parts }));
            }
            "assistant" => {
                input.extend(reasoning_items(msg, model));
                if let Some(text) = msg.text_content() {
                    if !text.is_empty() {
                        input.push(json!({
                            "role": "assistant",
                            "content": [{ "type": "output_text", "text": text }]
                        }));
                    }
                }
                if let Some(tcs) = &msg.tool_calls {
                    for tc in tcs {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.function.name,
                            "arguments": tc.function.arguments
                        }));
                    }
                }
            }
            "tool" => {
                if let Some(call_id) = &msg.tool_call_id {
                    let output = msg.text_content().unwrap_or("");
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output
                    }));
                }
            }
            _ => {}
        }
    }

    let effort = match reasoning_effort.to_ascii_lowercase().as_str() {
        "none" | "off" | "false" | "minimal" | "low" => "low",
        "high" => "high",
        "xhigh" | "max" => "xhigh",
        _ => "medium",
    };

    let mut body = json!({
        "model": model,
        "input": input,
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
        "reasoning": {
            "effort": effort,
            "summary": "auto"
        }
    });

    if !system_parts.is_empty() {
        body["instructions"] = json!(system_parts.join("\n\n"));
    }

    if let Some(tool_list) = tools {
        if !tool_list.is_empty() {
            let mapped: Vec<Value> = tool_list
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.function.name,
                        "description": t.function.description,
                        "parameters": t.function.parameters,
                        "strict": false
                    })
                })
                .collect();
            body["tools"] = json!(mapped);
        }
    }

    body
}

/// Items saved under another model are dropped: their encrypted content only decodes for the model that wrote it.
fn reasoning_items(msg: &Message, model: &str) -> Vec<Value> {
    let saved = msg
        .extra_content
        .as_ref()
        .and_then(|ec| ec.get(REASONING_EXTRA_KEY))
        .or_else(|| {
            msg.tool_calls
                .as_ref()?
                .iter()
                .find_map(|tc| tc.extra_content.as_ref()?.get(REASONING_EXTRA_KEY))
        });
    match saved {
        Some(saved) if saved.get("model").and_then(Value::as_str) == Some(model) => saved
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

use std::collections::HashMap;

#[derive(Default)]
pub struct ResponsesStreamState {
    tool_indices: HashMap<usize, usize>,
    next_tool_index: usize,
    reasoning: Vec<Value>,
    model: String,
}

impl ResponsesStreamState {
    pub fn new(model: &str) -> Self {
        Self { model: model.to_string(), ..Self::default() }
    }

    fn index_for(&mut self, output_index: usize) -> usize {
        if let Some(&idx) = self.tool_indices.get(&output_index) {
            idx
        } else {
            let idx = self.next_tool_index;
            self.next_tool_index += 1;
            self.tool_indices.insert(output_index, idx);
            idx
        }
    }
}

pub fn parse_responses_sse_data(data: &str, state: &mut ResponsesStreamState) -> Vec<StreamEvent> {
    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
    let ev_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let mut events = Vec::new();

    match ev_type {
        "response.output_text.delta" => {
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                if !delta.is_empty() {
                    events.push(StreamEvent::ContentDelta(delta.to_string()));
                }
            }
        }
        "response.reasoning_summary_text.delta" => {
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                if !delta.is_empty() {
                    events.push(StreamEvent::ReasoningDelta(delta.to_string()));
                }
            }
        }
        "response.output_item.added" => {
            if let Some(item) = v.get("item") {
                if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                    let output_index = v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    let index = state.index_for(output_index);
                    let call_id = item.get("call_id").and_then(|s| s.as_str()).map(str::to_string);
                    let name = item.get("name").and_then(|s| s.as_str()).map(str::to_string);
                    events.push(StreamEvent::ToolCallDelta {
                        index,
                        id: call_id,
                        name,
                        arguments: None,
                        extra_content: None,
                    });
                }
            }
        }
        "response.output_item.done" => {
            if let Some(item) = v.get("item").filter(|i| i.get("type").and_then(Value::as_str) == Some("reasoning")) {
                if item.get("encrypted_content").and_then(Value::as_str).is_some_and(|c| !c.is_empty()) {
                    let mut item = item.clone();
                    if let Some(obj) = item.as_object_mut() {
                        obj.remove("status");
                    }
                    state.reasoning.push(item);
                }
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                let output_index = v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let index = state.index_for(output_index);
                events.push(StreamEvent::ToolCallDelta {
                    index,
                    id: None,
                    name: None,
                    arguments: Some(delta.to_string()),
                    extra_content: None,
                });
            }
        }
        "response.completed" | "response.incomplete" => {
            if let Some(resp) = v.get("response") {
                if let Some(u) = resp.get("usage") {
                    events.push(StreamEvent::UsageUpdate(usage_from(u)));
                }
                if !state.reasoning.is_empty() {
                    events.push(StreamEvent::ExtraContentDelta(json!({
                        REASONING_EXTRA_KEY: { "model": state.model, "items": std::mem::take(&mut state.reasoning) }
                    })));
                }
                events.push(StreamEvent::Completed { finish_reason: Some(finish_reason(ev_type, resp).to_string()) });
            }
        }
        "response.failed" | "error" => {
            let msg = v
                .get("response")
                .and_then(|r| r.get("error"))
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .or_else(|| v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()))
                .or_else(|| v.get("message").and_then(|m| m.as_str()))
                .unwrap_or("Responses API error");
            events.push(StreamEvent::Error(msg.to_string()));
        }
        _ => {}
    }

    events
}

fn usage_from(u: &Value) -> Usage {
    let prompt_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let completion_tokens = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cached = u
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Usage {
        prompt_tokens,
        prompt_cache_hit_tokens: cached,
        prompt_cache_miss_tokens: prompt_tokens.saturating_sub(cached),
        completion_tokens,
        total_tokens: u.get("total_tokens").and_then(Value::as_u64).unwrap_or(prompt_tokens + completion_tokens),
    }
}

fn finish_reason(ev_type: &str, resp: &Value) -> &'static str {
    if ev_type == "response.incomplete" {
        let reason = resp.get("incomplete_details").and_then(|d| d.get("reason")).and_then(Value::as_str);
        return match reason {
            Some("content_filter") => "content_filter",
            _ => "length",
        };
    }
    let calls_tool = resp
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|i| i.get("type").and_then(Value::as_str) == Some("function_call")));
    if calls_tool { "tool_calls" } else { "stop" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionDefinition, ToolCall};

    #[test]
    fn converts_messages_and_tools_to_responses_shape() {
        let mut asst = Message::assistant("checking", None);
        asst.tool_calls = Some(vec![ToolCall::new("call_1", "ping", "{}")]);
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("hi"),
            asst,
            Message::tool_response("call_1", "pong"),
        ];
        let tools = vec![ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: "ping".into(),
                description: "ping".into(),
                parameters: json!({"type": "object", "properties": {}}),
            },
        }];

        let req = build_responses_request("gpt-6.2-astra", &messages, Some(&tools), "medium");
        assert_eq!(req["model"], "gpt-6.2-astra");
        assert_eq!(req["instructions"], "You are helpful");
        assert_eq!(req["reasoning"]["effort"], "medium");
        assert_eq!(req["tools"][0]["name"], "ping");
        assert_eq!(req["tools"][0]["strict"], false);
        let input = req["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[3]["type"], "function_call_output");
    }

    #[test]
    fn parses_tool_call_and_completed_events() {
        let mut state = ResponsesStreamState::new("gpt-6.2-astra");
        let added = r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_abc","name":"ping"},"output_index":1}"#;
        let evs = parse_responses_sse_data(added, &mut state);
        assert!(matches!(
            &evs[0],
            StreamEvent::ToolCallDelta { index: 0, id: Some(id), name: Some(n), arguments: None, .. }
            if id == "call_abc" && n == "ping"
        ));

        let delta = r#"{"type":"response.function_call_arguments.delta","delta":"{\"msg\":\"hola\"}","output_index":1}"#;
        let evs = parse_responses_sse_data(delta, &mut state);
        assert!(matches!(
            &evs[0],
            StreamEvent::ToolCallDelta { index: 0, arguments: Some(a), .. }
            if a == "{\"msg\":\"hola\"}"
        ));

        let completed = r#"{"type":"response.completed","response":{"output":[{"type":"function_call"}],"usage":{"input_tokens":47,"output_tokens":18,"total_tokens":65}}}"#;
        let evs = parse_responses_sse_data(completed, &mut state);
        assert!(matches!(&evs[0], StreamEvent::UsageUpdate(u) if u.prompt_tokens == 47 && u.completion_tokens == 18));
        assert!(matches!(&evs[1], StreamEvent::Completed { finish_reason: Some(r) } if r == "tool_calls"));
    }

    #[test]
    fn encrypted_reasoning_goes_back_only_to_the_model_that_wrote_it() {
        let mut state = ResponsesStreamState::new("gpt-6.2-astra");
        let done = r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"enc","status":"completed"}}"#;
        assert!(parse_responses_sse_data(done, &mut state).is_empty());
        let completed = r#"{"type":"response.completed","response":{"output":[{"type":"function_call"}]}}"#;
        let evs = parse_responses_sse_data(completed, &mut state);
        let StreamEvent::ExtraContentDelta(extra) = &evs[0] else { panic!("{evs:?}") };

        let mut call = ToolCall::new("call_1", "ping", "{}");
        call.extra_content = Some(extra.clone());
        let mut asst = Message::assistant("", None);
        asst.tool_calls = Some(vec![call]);
        let messages = vec![Message::user("hi"), asst];

        let req = build_responses_request("gpt-6.2-astra", &messages, None, "high");
        assert_eq!(req["include"][0], "reasoning.encrypted_content");
        assert_eq!(req["input"][1]["type"], "reasoning");
        assert_eq!(req["input"][1]["encrypted_content"], "enc");
        assert!(req["input"][1].get("status").is_none());
        assert_eq!(req["input"][2]["type"], "function_call");

        let other = build_responses_request("gpt-5", &messages, None, "high");
        assert_eq!(other["input"][1]["type"], "function_call");
    }

    #[test]
    fn an_incomplete_response_says_why_it_stopped() {
        let mut state = ResponsesStreamState::new("gpt-6.2-astra");
        let cut = r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":5,"output_tokens":9}}}"#;
        let evs = parse_responses_sse_data(cut, &mut state);
        assert!(matches!(evs.last(), Some(StreamEvent::Completed { finish_reason: Some(r) }) if r == "length"));
        let filtered = r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"content_filter"}}}"#;
        let evs = parse_responses_sse_data(filtered, &mut state);
        assert!(matches!(evs.last(), Some(StreamEvent::Completed { finish_reason: Some(r) }) if r == "content_filter"));
    }
}
