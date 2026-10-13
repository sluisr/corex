use corex_core::{
    anthropic::parse_anthropic_sse_data, parse_lenient_json, parse_openai_sse_data, StreamEvent,
    Usage,
};

#[test]
fn test_deepseek_v4_sse_replay_cot_and_tool_calls() {
    let sse_chunks = vec![
        // 1. Thinking / reasoning delta
        r#"{"id":"chatcmpl-ds-1","object":"chat.completion.chunk","created":1740000000,"model":"deepseek-v4-pro","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"Let's check the Cargo.toml file to verify workspace dependencies."},"finish_reason":null}]}"#,
        // 2. Additional reasoning delta
        r#"{"id":"chatcmpl-ds-1","object":"chat.completion.chunk","created":1740000000,"model":"deepseek-v4-pro","choices":[{"index":0,"delta":{"reasoning_content":" I will use read_file for this."},"finish_reason":null}]}"#,
        // 3. Tool call start
        r#"{"id":"chatcmpl-ds-1","object":"chat.completion.chunk","created":1740000000,"model":"deepseek-v4-pro","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_ds_read_01","type":"function","function":{"name":"read_file","arguments":""}}]},"finish_reason":null}]}"#,
        // 4. Tool call arguments fragment 1
        r#"{"id":"chatcmpl-ds-1","object":"chat.completion.chunk","created":1740000000,"model":"deepseek-v4-pro","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":\"Cargo"}}]},"finish_reason":null}]}"#,
        // 5. Tool call arguments fragment 2
        r#"{"id":"chatcmpl-ds-1","object":"chat.completion.chunk","created":1740000000,"model":"deepseek-v4-pro","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":".toml\"}"}}]},"finish_reason":null}]}"#,
        // 6. Finish reason and usage
        r#"{"id":"chatcmpl-ds-1","object":"chat.completion.chunk","created":1740000000,"model":"deepseek-v4-pro","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":150,"completion_tokens":42,"total_tokens":192,"prompt_cache_hit_tokens":128,"prompt_cache_miss_tokens":22}}"#,
        // 7. [DONE]
        "[DONE]",
    ];

    let mut accumulated_reasoning = String::new();
    let mut tool_name = String::new();
    let mut tool_id = String::new();
    let mut tool_args = String::new();
    let mut completed_reason = None;
    let mut final_usage = None;

    for chunk in sse_chunks {
        if let Some(events) = parse_openai_sse_data(chunk) {
            for ev in events {
                match ev {
                    StreamEvent::ReasoningDelta(cot) => accumulated_reasoning.push_str(&cot),
                    StreamEvent::ToolCallDelta { id, name, arguments, .. } => {
                        if let Some(i) = id {
                            tool_id.push_str(&i);
                        }
                        if let Some(n) = name {
                            tool_name.push_str(&n);
                        }
                        if let Some(a) = arguments {
                            tool_args.push_str(&a);
                        }
                    }
                    StreamEvent::UsageUpdate(u) => final_usage = Some(u),
                    StreamEvent::Completed { finish_reason } => {
                        if let Some(r) = finish_reason {
                            completed_reason = Some(r);
                        } else if completed_reason.is_none() {
                            completed_reason = Some("stop".to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    assert_eq!(
        accumulated_reasoning,
        "Let's check the Cargo.toml file to verify workspace dependencies. I will use read_file for this."
    );
    assert_eq!(tool_name, "read_file");
    assert_eq!(tool_id, "call_ds_read_01");
    assert_eq!(tool_args, "{\"path\":\"Cargo.toml\"}");

    let parsed_args = parse_lenient_json(&tool_args).expect("valid args JSON");
    assert_eq!(parsed_args["path"], "Cargo.toml");

    let usage = final_usage.expect("usage recorded");
    assert_eq!(usage.prompt_tokens, 150);
    assert_eq!(usage.prompt_cache_hit_tokens, 128);
    assert_eq!(completed_reason.as_deref(), Some("tool_calls"));
}

#[test]
fn test_anthropic_sse_replay_claude_tool_calling() {
    let mut usage = Usage::default();

    let chunks = vec![
        r#"{"type":"message_start","message":{"id":"msg_claude_1","type":"message","role":"assistant","model":"claude-3-7-sonnet-20250219","usage":{"input_tokens":320,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"I should inspect the file using read_file."}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" Path is crates/corex-core/src/lib.rs."}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01Abc","name":"read_file"}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\": \"crates/corex-core/src/lib.rs\"}"}}"#,
        r#"{"type":"content_block_stop","index":1}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":0,"output_tokens":85}}"#,
        r#"{"type":"message_stop"}"#,
    ];

    let mut accumulated_thinking = String::new();
    let mut tool_id = String::new();
    let mut tool_name = String::new();
    let mut tool_args = String::new();
    let mut stop_reason = None;

    for chunk in chunks {
        if let Some(events) = parse_anthropic_sse_data(chunk, &mut usage) {
            for ev in events {
                match ev {
                    StreamEvent::ReasoningDelta(cot) => accumulated_thinking.push_str(&cot),
                    StreamEvent::ToolCallDelta { id, name, arguments, .. } => {
                        if let Some(i) = id {
                            tool_id.push_str(&i);
                        }
                        if let Some(n) = name {
                            tool_name.push_str(&n);
                        }
                        if let Some(a) = arguments {
                            tool_args.push_str(&a);
                        }
                    }
                    StreamEvent::Completed { finish_reason } => {
                        if let Some(r) = finish_reason {
                            stop_reason = Some(r);
                        } else if stop_reason.is_none() {
                            stop_reason = Some("stop".to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    assert_eq!(accumulated_thinking, "I should inspect the file using read_file. Path is crates/corex-core/src/lib.rs.");
    assert_eq!(tool_id, "toolu_01Abc");
    assert_eq!(tool_name, "read_file");
    assert_eq!(tool_args, "{\"path\": \"crates/corex-core/src/lib.rs\"}");
    assert_eq!(stop_reason.as_deref(), Some("tool_calls"));
    assert_eq!(usage.prompt_tokens, 320);
    assert_eq!(usage.completion_tokens, 85);
}

#[test]
fn test_google_gemini_sse_replay_with_thought_signature() {
    let gemini_chunks = vec![
        // Chunk 1: Tool call with extra_content (Gemini Thought Signature)
        r#"{"id":"chatcmpl-gemini-1","object":"chat.completion.chunk","created":1740000001,"model":"gemini-3.8-flash","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_gemini_read_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"README.md\"}"},"extra_content":{"thought_signature":"gemini_ts_token_alpha987654321"}}]},"finish_reason":null}]}"#,
        // Chunk 2: Extra content / thought signature update
        r#"{"id":"chatcmpl-gemini-1","object":"chat.completion.chunk","created":1740000001,"model":"gemini-3.8-flash","choices":[{"index":0,"delta":{"extra_content":{"thought_signature":"gemini_ts_token_alpha987654321"}},"finish_reason":"tool_calls"}]}"#,
        // Chunk 3: [DONE]
        "[DONE]",
    ];

    let mut tool_name = String::new();
    let mut tool_id = String::new();
    let mut tool_args = String::new();
    let mut tool_extra = None;
    let mut completed_reason = None;

    for chunk in gemini_chunks {
        if let Some(events) = parse_openai_sse_data(chunk) {
            for ev in events {
                match ev {
                    StreamEvent::ToolCallDelta { id, name, arguments, extra_content, .. } => {
                        if let Some(i) = id {
                            tool_id.push_str(&i);
                        }
                        if let Some(n) = name {
                            tool_name.push_str(&n);
                        }
                        if let Some(a) = arguments {
                            tool_args.push_str(&a);
                        }
                        if let Some(ec) = extra_content {
                            tool_extra = Some(ec);
                        }
                    }
                    StreamEvent::ExtraContentDelta(ec) => {
                        if tool_extra.is_none() {
                            tool_extra = Some(ec);
                        }
                    }
                    StreamEvent::Completed { finish_reason } => {
                        if let Some(r) = finish_reason {
                            completed_reason = Some(r);
                        } else if completed_reason.is_none() {
                            completed_reason = Some("stop".to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    assert_eq!(tool_name, "read_file");
    assert_eq!(tool_id, "call_gemini_read_1");
    assert_eq!(tool_args, "{\"path\":\"README.md\"}");
    assert_eq!(completed_reason.as_deref(), Some("tool_calls"));

    let extra = tool_extra.expect("thought signature preserved");
    assert_eq!(extra["thought_signature"], "gemini_ts_token_alpha987654321");
}

#[test]
fn test_openai_multi_tool_call_parallel_streaming() {
    let openai_chunks = vec![
        // Tool 0: run_shell_command
        r#"{"id":"chatcmpl-oai-1","object":"chat.completion.chunk","created":1740000002,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_01","type":"function","function":{"name":"run_shell_command","arguments":""}}]},"finish_reason":null}]}"#,
        // Tool 1: list_directory
        r#"{"id":"chatcmpl-oai-1","object":"chat.completion.chunk","created":1740000002,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_02","type":"function","function":{"name":"list_directory","arguments":""}}]},"finish_reason":null}]}"#,
        // Tool 0 arguments delta
        r#"{"id":"chatcmpl-oai-1","object":"chat.completion.chunk","created":1740000002,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":\"cargo check\"}"}}]},"finish_reason":null}]}"#,
        // Tool 1 arguments delta
        r#"{"id":"chatcmpl-oai-1","object":"chat.completion.chunk","created":1740000002,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"path\":\"crates\"}"}}]},"finish_reason":"tool_calls"}]}"#,
    ];

    let mut tools = [
        (String::new(), String::new(), String::new()),
        (String::new(), String::new(), String::new()),
    ];

    for chunk in openai_chunks {
        if let Some(events) = parse_openai_sse_data(chunk) {
            for ev in events {
                if let StreamEvent::ToolCallDelta { index, id, name, arguments, .. } = ev {
                    if let Some(i) = id {
                        tools[index].0.push_str(&i);
                    }
                    if let Some(n) = name {
                        tools[index].1.push_str(&n);
                    }
                    if let Some(a) = arguments {
                        tools[index].2.push_str(&a);
                    }
                }
            }
        }
    }

    assert_eq!(tools[0].0, "call_01");
    assert_eq!(tools[0].1, "run_shell_command");
    assert_eq!(tools[0].2, "{\"command\":\"cargo check\"}");

    assert_eq!(tools[1].0, "call_02");
    assert_eq!(tools[1].1, "list_directory");
    assert_eq!(tools[1].2, "{\"path\":\"crates\"}");
}

#[test]
fn test_ollama_local_llm_object_arguments_and_string_index() {
    // Local LLMs or Ollama proxies sometimes format index as string "0" and arguments directly as a JSON object
    let chunk = r#"{"id":"chatcmpl-ollama-1","object":"chat.completion.chunk","created":1740000003,"model":"qwen2.5-coder:7b","choices":[{"index":0,"delta":{"tool_calls":[{"index":"0","id":"call_loc_1","type":"function","function":{"name":"grep_search","arguments":{"query":"fn main","path":"."}}}]},"finish_reason":"stop"}]}"#;

    let events = parse_openai_sse_data(chunk).expect("parsed Ollama chunk");
    assert_eq!(events.len(), 2);

    match &events[0] {
        StreamEvent::ToolCallDelta { index, id, name, arguments, .. } => {
            assert_eq!(*index, 0);
            assert_eq!(id.as_deref(), Some("call_loc_1"));
            assert_eq!(name.as_deref(), Some("grep_search"));
            let raw_args = arguments.as_ref().expect("arguments present");
            let parsed = parse_lenient_json(raw_args).expect("parsed lenient json");
            assert_eq!(parsed["query"], "fn main");
            assert_eq!(parsed["path"], ".");
        }
        _ => panic!("Expected ToolCallDelta"),
    }
}
