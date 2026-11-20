use crate::client::StreamEvent;
use crate::types::{Message, MessageContent, ToolDefinition, Usage};
use serde_json::{json, Value};

pub const REPLAY_EXTRA_KEY: &str = "responses_reasoning";

// DeepSeek on Azure only reasons through Chat Completions, so it stays there.
pub fn uses_responses_api(kind: crate::providers::ProviderKind, model: &str) -> bool {
    kind == crate::providers::ProviderKind::Azure && is_openai_reasoning_model(model)
}

// Other models served through Responses (DeepSeek) reject the `reasoning` parameter.
fn is_openai_reasoning_model(model: &str) -> bool {
    let m = model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase();
    ["gpt-6", "gpt-5", "o1", "o3", "o4"].iter().any(|prefix| m.starts_with(prefix))
}

pub fn responses_url(endpoint: &str) -> String {
    let base = endpoint.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/responses")
    } else {
        format!("{base}/v1/responses")
    }
}

// DeepSeek on Azure must stay on Chat Completions to reason, so its searches go through this.
pub fn web_search_request(model: &str, queries: &[String]) -> Value {
    let list: Vec<String> = queries.iter().map(|q| format!("- {q}")).collect();
    json!({
        "model": model,
        "input": format!(
            "Search the web for each of these queries. Reply only with the facts found, each with its source URL: no greeting, no preamble, no description of the search, nothing from memory.\n{}",
            list.join("\n")
        ),
        "tools": [{ "type": "web_search" }],
        "include": ["web_search_call.action.sources"],
        "store": false,
    })
}

pub fn web_search_results(response: &Value) -> Option<String> {
    let output = response.get("output")?.as_array()?;
    let mut text = String::new();
    let mut sources: Vec<String> = Vec::new();
    let mut add = |line: String| {
        if !sources.contains(&line) {
            sources.push(line);
        }
    };
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for part in item["content"].as_array().into_iter().flatten() {
                    text.push_str(part["text"].as_str().unwrap_or_default());
                    for a in part["annotations"].as_array().into_iter().flatten() {
                        if let Some(url) = a["url"].as_str() {
                            add(match a["title"].as_str().filter(|t| !t.is_empty()) {
                                Some(title) => format!("- {title} — {url}"),
                                None => format!("- {url}"),
                            });
                        }
                    }
                }
            }
            Some("web_search_call") => {
                for source in item["action"]["sources"].as_array().into_iter().flatten() {
                    if let Some(url) = source["url"].as_str() {
                        add(format!("- {url}"));
                    }
                }
            }
            _ => {}
        }
    }
    if text.trim().is_empty() && sources.is_empty() {
        return None;
    }
    if sources.is_empty() {
        return Some(text.trim().to_string());
    }
    Some(format!("{}\n\nSources:\n{}", text.trim(), sources.join("\n")))
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
                input.extend(replay_items(msg, model));
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

    let mut body = json!({
        "model": model,
        "input": input,
        "stream": true,
        "store": false,
    });
    let mut include = vec!["web_search_call.action.sources"];
    if is_openai_reasoning_model(model) {
        let effort = match reasoning_effort.to_ascii_lowercase().as_str() {
            "none" | "off" | "false" | "minimal" | "low" => "low",
            "high" => "high",
            "xhigh" | "max" => "xhigh",
            _ => "medium",
        };
        body["reasoning"] = json!({ "effort": effort, "summary": "auto" });
        include.insert(0, "reasoning.encrypted_content");
    }
    body["include"] = json!(include);

    if !system_parts.is_empty() {
        body["instructions"] = json!(system_parts.join("\n\n"));
    }

    let mut mapped: Vec<Value> = tools
        .unwrap_or_default()
        .iter()
        .filter(|t| t.function.name != "web_search")
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
    mapped.push(json!({ "type": "web_search" }));
    body["tools"] = json!(mapped);

    body
}

// Encrypted reasoning only decodes for the model that wrote it.
fn replay_items(msg: &Message, model: &str) -> Vec<Value> {
    let saved = msg
        .extra_content
        .as_ref()
        .and_then(|ec| ec.get(REPLAY_EXTRA_KEY))
        .or_else(|| {
            msg.tool_calls
                .as_ref()?
                .iter()
                .find_map(|tc| tc.extra_content.as_ref()?.get(REPLAY_EXTRA_KEY))
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
    replay: Vec<Value>,
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
        "response.web_search_call.searching" => {
            let id = v.get("item_id").and_then(Value::as_str).unwrap_or_default().to_string();
            events.push(StreamEvent::ServerTool { id, done: false, summary: "Web search".to_string() });
        }
        "response.output_item.done" => {
            let item = v.get("item").cloned().unwrap_or_default();
            match item.get("type").and_then(Value::as_str) {
                Some("reasoning") if item.get("encrypted_content").and_then(Value::as_str).is_some_and(|c| !c.is_empty()) => {
                    let mut item = item;
                    if let Some(obj) = item.as_object_mut() {
                        obj.remove("status");
                    }
                    state.replay.push(item);
                }
                Some("web_search_call") => {
                    let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                    events.push(StreamEvent::ServerTool { id, done: true, summary: describe_web_search(&item) });
                    state.replay.push(item);
                }
                _ => {}
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
                if !state.replay.is_empty() {
                    events.push(StreamEvent::ExtraContentDelta(json!({
                        REPLAY_EXTRA_KEY: { "model": state.model, "items": std::mem::take(&mut state.replay) }
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

fn describe_web_search(item: &Value) -> String {
    let action = &item["action"];
    let url = action["url"].as_str().unwrap_or_default();
    let what = match action["type"].as_str() {
        Some("open_page") => format!("Opened {url}"),
        Some("find_in_page") => format!("Searched \"{}\" in {url}", action["pattern"].as_str().unwrap_or_default()),
        _ => {
            let mut queries: Vec<&str> = action["queries"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            if queries.is_empty() {
                queries.extend(action["query"].as_str());
            }
            format!("Web search: {}", queries.join(" · "))
        }
    };
    let failed = item["status"].as_str() == Some("failed");
    match action["sources"].as_array().map(Vec::len) {
        _ if failed => format!("{what} — failed"),
        Some(n) if n > 0 => format!("{what} — {n} sources"),
        _ => what,
    }
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

    #[test]
    fn web_search_replaces_corex_search_and_skips_reasoning_for_deepseek() {
        let tool = |name: &str| ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition { name: name.into(), description: String::new(), parameters: json!({"type": "object"}) },
        };
        let tools = vec![tool("web_search"), tool("read_file")];
        let messages = vec![Message::user("hi")];

        let req = build_responses_request("DeepSeek-V4.1-Flash-1017", &messages, Some(&tools), "high");
        let names: Vec<&str> = req["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap_or(t["type"].as_str().unwrap())).collect();
        assert_eq!(names, ["read_file", "web_search"]);
        assert_eq!(req["tools"][1]["type"], "web_search");
        assert!(req.get("reasoning").is_none());
        assert_eq!(req["include"], json!(["web_search_call.action.sources"]));

        let gpt = build_responses_request("gpt-6.2-astra", &messages, Some(&tools), "high");
        assert_eq!(gpt["reasoning"]["effort"], "high");
        assert_eq!(gpt["include"], json!(["reasoning.encrypted_content", "web_search_call.action.sources"]));

        assert!(uses_responses_api(crate::providers::ProviderKind::Azure, "gpt-6.2-astra"));
        assert!(!uses_responses_api(crate::providers::ProviderKind::Azure, "DeepSeek-V4.1-Flash-1017"));
    }

    #[test]
    fn a_web_search_shows_as_a_line_and_is_kept_for_the_next_turn() {
        let mut state = ResponsesStreamState::new("gpt-6.2-astra");
        let searching = r#"{"type":"response.web_search_call.searching","item_id":"ws_1"}"#;
        assert!(matches!(&parse_responses_sse_data(searching, &mut state)[0], StreamEvent::ServerTool { done: false, .. }));
        let done = r#"{"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","queries":["rust 1.95"],"sources":[{"type":"url","url":"https://blog.rust-lang.org"}]}}}"#;
        let evs = parse_responses_sse_data(done, &mut state);
        assert!(matches!(&evs[0], StreamEvent::ServerTool { done: true, summary, .. } if summary == "Web search: rust 1.95 — 1 sources"), "{evs:?}");
        let completed = r#"{"type":"response.completed","response":{"output":[]}}"#;
        let evs = parse_responses_sse_data(completed, &mut state);
        let StreamEvent::ExtraContentDelta(extra) = &evs[0] else { panic!("{evs:?}") };
        assert_eq!(extra[REPLAY_EXTRA_KEY]["items"][0]["type"], "web_search_call");
    }

    #[test]
    fn a_search_only_request_comes_back_as_text_with_its_sources() {
        let req = web_search_request("DeepSeek-V4.1-Flash-1017", &["rust 1.100".to_string()]);
        assert_eq!(req["tools"][0]["type"], "web_search");
        assert!(req["input"].as_str().unwrap().contains("- rust 1.100"));
        assert!(req.get("reasoning").is_none());

        let response = json!({"output": [
            {"type": "web_search_call", "action": {"type": "search", "sources": [{"type": "url", "url": "https://blog.rust-lang.org/a"}, {"type": "url", "url": "https://x.dev"}]}},
            {"type": "message", "content": [{"type": "output_text", "text": "Rust 1.100 shipped.", "annotations": [{"type": "url_citation", "url": "https://blog.rust-lang.org/a", "title": "Announcing"}]}]}
        ]});
        let text = web_search_results(&response).unwrap();
        assert!(text.starts_with("Rust 1.100 shipped.\n\nSources:\n"), "{text}");
        assert!(text.contains("- https://blog.rust-lang.org/a") && text.contains("- Announcing — https://blog.rust-lang.org/a") && text.contains("- https://x.dev"));
        assert_eq!(responses_url("https://r.services.ai.azure.com/openai/v1/"), "https://r.services.ai.azure.com/openai/v1/responses");
        assert!(web_search_results(&json!({"output": []})).is_none());
    }
}
