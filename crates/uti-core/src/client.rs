use std::sync::{Arc, Mutex};
use std::time::Duration;
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use reqwest_eventsource::{Event, RequestBuilderExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use crate::config::Config;
use crate::reasoning_cache::ReasoningCache;
use crate::types::*;

static GLOBAL_SUDO_PASSWORD: Mutex<Option<String>> = Mutex::new(None);

pub fn get_sudo_password() -> Option<String> {
    GLOBAL_SUDO_PASSWORD.lock().ok().and_then(|g| g.clone())
}

pub fn set_sudo_password(pwd: Option<String>) {
    if let Ok(mut g) = GLOBAL_SUDO_PASSWORD.lock() {
        *g = pwd;
    }
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    ReasoningDelta(String),
    ContentDelta(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    UsageUpdate(Usage),
    Completed {
        finish_reason: Option<String>,
    },
    ToolExecutionDone {
        call_id: String,
        output: String,
    },
    AllToolsDone,
    Notice(String),
    ContextCompacted {
        compacted_messages: Vec<Message>,
        notice: String,
    },
    Error(String),
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    config: Arc<Mutex<Config>>,
    reasoning_cache: ReasoningCache,
    local_client: Arc<Mutex<crate::local_client::LocalLlmClient>>,
}

impl LlmClient {
    pub fn new(config: Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(10))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .expect("Failed to build HTTP client");

        let local_client = crate::local_client::LocalLlmClient::new(
            &config.local_llm_url,
            &config.local_llm_model,
        );

        Self {
            http,
            config: Arc::new(Mutex::new(config)),
            reasoning_cache: ReasoningCache::new(),
            local_client: Arc::new(Mutex::new(local_client)),
        }
    }

    pub fn get_config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    pub fn update_config(&self, new_config: Config) {
        let local = crate::local_client::LocalLlmClient::new(
            &new_config.local_llm_url,
            &new_config.local_llm_model,
        );
        if let Ok(mut l) = self.local_client.lock() {
            *l = local;
        }
        if let Ok(mut g) = self.config.lock() {
            *g = new_config;
        }
    }

    pub fn local_client(&self) -> crate::local_client::LocalLlmClient {
        self.local_client.lock().unwrap().clone()
    }

    pub fn reasoning_cache(&self) -> &ReasoningCache {
        &self.reasoning_cache
    }

    /// Sanitizes message sequences to satisfy DeepSeek/OpenAI schema requirements:
    /// 1. Removes orphaned 'tool' messages not preceded by an assistant message with matching tool_calls id.
    /// 2. Ensures every assistant tool_call has a following 'tool' response.
    pub fn sanitize_tool_call_sequences(messages: &mut Vec<Message>) {
        let mut i = 0;
        while i < messages.len() {
            if messages[i].role == "tool" {
                let tool_id = messages[i].tool_call_id.as_deref().unwrap_or("");
                let mut k = i as isize - 1;
                while k >= 0 && (messages[k as usize].role == "tool" || messages[k as usize].role == "system") {
                    k -= 1;
                }

                let has_preceding_call = if k >= 0 && messages[k as usize].role == "assistant" {
                    messages[k as usize]
                        .tool_calls
                        .as_ref()
                        .map(|calls| calls.iter().any(|c| c.id == tool_id))
                        .unwrap_or(false)
                } else {
                    false
                };

                if !has_preceding_call {
                    debug!("[SANITIZER] Removing orphan tool message at index {}", i);
                    messages.remove(i);
                    continue;
                }
            }
            i += 1;
        }

        // Fill missing tool responses for any tool_calls and ensure tool messages directly follow assistant
        let mut idx = 0;
        while idx < messages.len() {
            if messages[idx].role == "assistant" {
                if let Some(ref calls) = messages[idx].tool_calls.clone() {
                    let mut following_ids = Vec::new();
                    let mut next_idx = idx + 1;
                    // Scan forward to find all tool responses belonging to this assistant turn
                    while next_idx < messages.len()
                        && messages[next_idx].role != "assistant"
                        && messages[next_idx].role != "user"
                    {
                        if messages[next_idx].role == "tool" {
                            if let Some(ref tid) = messages[next_idx].tool_call_id {
                                following_ids.push(tid.clone());
                            }
                        }
                        next_idx += 1;
                    }

                    for call in calls {
                        if !following_ids.contains(&call.id) {
                            messages.insert(
                                next_idx,
                                Message::tool_response(
                                    call.id.clone(),
                                    "Tool call was cancelled by the user.",
                                ),
                            );
                            following_ids.push(call.id.clone());
                            next_idx += 1;
                        }
                    }

                    // Strict API ordering: Ensure all tool responses for this assistant message
                    // immediately follow it, with no intervening system messages.
                    let mut insert_pos = idx + 1;
                    let mut check_pos = idx + 1;
                    while check_pos < messages.len()
                        && messages[check_pos].role != "assistant"
                        && messages[check_pos].role != "user"
                    {
                        if messages[check_pos].role == "tool" {
                            if check_pos != insert_pos {
                                let tool_msg = messages.remove(check_pos);
                                messages.insert(insert_pos, tool_msg);
                            }
                            insert_pos += 1;
                        }
                        check_pos += 1;
                    }
                    idx = check_pos - 1;
                }
            }
            idx += 1;
        }
    }

    /// Normalizes tools by sorting them alphabetically to guarantee 100% prefix matching
    /// for DeepSeek's KV cache pricing discount.
    pub fn sort_tools(tools: &mut [ToolDefinition]) {
        tools.sort_by(|a, b| a.function.name.cmp(&b.function.name));
    }

    /// Truncates tool response messages that are excessively long.
    /// Large shell outputs (recursive ls, grep dumps, logs) get re-sent every turn
    /// and are the single biggest source of unexpected token growth in agentic loops.
    /// Cap at 8 000 chars (~2 400 tokens) — enough for the model to understand the
    /// result, but not enough to break the bank.
    const TOOL_OUTPUT_MAX_CHARS: usize = 8_000;

    pub async fn compress_or_truncate_tool_outputs(&self, messages: &mut [Message]) {
        let cfg = self.get_config();
        let local_available = cfg.local_llm_enabled && (cfg.hybrid_compression || cfg.hybrid_settings.auto_compression);

        let local_client = if local_available {
            let client = self.local_client();
            if client.health_check().await {
                Some(client)
            } else {
                None
            }
        } else {
            None
        };

        // ⚡ KV CACHE IMMUTABILITY GUARANTEE:
        // Find the boundary of the current turn's tool outputs (trailing block of tool messages).
        // Historical messages prior to the latest assistant message MUST remain strictly immutable
        // to guarantee a 100% KV Cache hit rate on DeepSeek / OpenAI prefix caching.
        let first_active_tool_idx = match messages.iter().rposition(|m| m.role == "assistant") {
            Some(pos) => pos + 1,
            None => 0,
        };

        // Collect indices of active tool messages that need local compression
        let mut compression_candidates = Vec::new();
        for (idx, msg) in messages.iter().enumerate().skip(first_active_tool_idx) {
            if msg.role == "tool" {
                if let Some(content) = msg.text_content() {
                    // Only compress if not already compressed (preserves Prefix KV Cache for past turns!)
                    if content.len() > 1_500
                        && !content.contains("[⚡ Synthesized & compressed")
                        && !content.contains("chars truncated to save tokens")
                    {
                        compression_candidates.push((idx, content.to_string()));
                    }
                }
            }
        }

        if let Some(ref local) = local_client {
            if !compression_candidates.is_empty() {
                let mut futures = Vec::new();
                for (idx, content) in compression_candidates {
                    let local = local.clone();
                    futures.push(tokio::spawn(async move {
                        let tool_name = "tool_output";
                        let summary = local.summarize_tool_output(tool_name, &content).await;
                        (idx, content, summary)
                    }));
                }

                let results = futures_util::future::join_all(futures).await;
                for res in results {
                    if let Ok((idx, orig_content, Ok(summary))) = res {
                        if !summary.trim().is_empty() && summary.len() < orig_content.len() {
                            debug!("[HYBRID] Compressed tool output at index {} from {} to {} chars", idx, orig_content.len(), summary.len());
                            messages[idx].content = Some(format!(
                                "{}\n\n[⚡ Synthesized & compressed by Local LLM (saved ~{} tokens)]",
                                summary.trim(),
                                (orig_content.len().saturating_sub(summary.len())) / 4
                            ).into());
                        }
                    }
                }
            }
        }

        // Final safety truncation for any newly generated active tool output still exceeding TOOL_OUTPUT_MAX_CHARS
        for msg in messages[first_active_tool_idx..].iter_mut() {
            if msg.role == "tool" {
                if let Some(content) = msg.text_content().map(|s| s.to_string()) {
                    if content.len() > Self::TOOL_OUTPUT_MAX_CHARS && !content.contains("chars truncated to save tokens") {
                        let truncated = crate::types::safe_truncate_str(&content, Self::TOOL_OUTPUT_MAX_CHARS);
                        let omitted = content.len() - truncated.len();
                        msg.content = Some(format!(
                            "{}\n\n[... {} chars truncated to save tokens ...]",
                            truncated, omitted
                        ).into());
                    }
                }
            }
        }
    }

    /// Estimates token count of a string using a conservative 4 chars/token ratio.
    fn estimate_tokens(s: &str) -> usize {
        s.len() / 4
    }

    /// Rough total token estimate for a message list (prompt side only).
    fn estimate_messages_tokens(messages: &[Message]) -> usize {
        messages.iter().map(|m| {
            let content_toks = match &m.content {
                Some(MessageContent::Text(s)) => Self::estimate_tokens(s),
                Some(MessageContent::Parts(parts)) => parts.iter().map(|p| {
                    let text_toks = p.text.as_deref().map(Self::estimate_tokens).unwrap_or(0);
                    let img_toks = if p.image_url.is_some() { 1024 } else { 0 };
                    text_toks + img_toks
                }).sum(),
                None => 0,
            };
            let reasoning_toks = m.reasoning_content.as_deref().map(Self::estimate_tokens).unwrap_or(0);
            content_toks + reasoning_toks + 4 // per-message overhead
        }).sum()
    }

    /// Automatically or manually compacts older conversation turns into a dense, structured memory block.
    /// Preserves system prompt and root user prompt (anchor) and latest active turns.
    pub async fn compact_messages(
        &self,
        messages: &mut Vec<Message>,
        force: bool,
    ) -> Result<Option<String>> {
        let cfg = self.get_config();
        let current_tokens = Self::estimate_messages_tokens(messages);

        let effective_threshold = if cfg.model.starts_with("local") {
            cfg.compact_threshold_tokens.min(24_000)
        } else {
            cfg.compact_threshold_tokens
        };

        if !force && (!cfg.auto_compact || current_tokens <= effective_threshold) {
            return Ok(None);
        }

        let total_msgs = messages.len();
        if total_msgs < 8 {
            return Ok(None);
        }

        // Find system prompt end index (first non-system message = Root User Anchor)
        let first_non_system = messages.iter().position(|m| m.role != "system").unwrap_or(0);

        // Keep the latest 8-10 messages (representing current active turns / tools / reasoning)
        let keep_recent = 10.min(total_msgs.saturating_sub(first_non_system + 4)).max(4);
        let split_at = total_msgs.saturating_sub(keep_recent);

        // The root user prompt (anchor) is preserved at first_non_system.
        // We only compact the middle turns between the anchor and recent active turns.
        let middle_start = first_non_system + 1;
        if middle_start >= split_at || split_at.saturating_sub(middle_start) < 2 {
            return Ok(None);
        }

        let older_slice = &messages[middle_start..split_at];
        let older_count = older_slice.len();

        // Extract intermediate user requests, tool actions, and errors
        let mut user_requests = Vec::new();
        let mut tool_actions = Vec::new();
        let mut prev_summary: Option<String> = None;

        for m in older_slice {
            let text = m.text_content().unwrap_or("").trim();
            if m.role == "system" && text.contains("<CONTEXT_SUMMARY>") {
                prev_summary = Some(text.to_string());
                continue;
            }

            if m.role == "user" {
                if !text.is_empty() {
                    user_requests.push(crate::types::truncate_ellipsis(text, 250));
                }
            } else if m.role == "assistant" {
                if let Some(ref calls) = m.tool_calls {
                    for c in calls {
                        let short_args = crate::types::truncate_ellipsis(&c.function.arguments, 120);
                        tool_actions.push(format!("{}({})", c.function.name, short_args));
                    }
                }
            } else if m.role == "tool" && (text.starts_with("Error") || text.contains("failed") || text.contains("error:")) {
                let err_snip = crate::types::truncate_ellipsis(text, 120);
                tool_actions.push(format!("Tool result error: {}", err_snip));
            }
        }

        let mut summary_opt: Option<String> = None;

        // 1. Try local SLM first (100% free @ $0.00)
        let local_client = self.local_client();
        if local_client.health_check().await {
            let digest = format!(
                "User Requests in Compacted Turns:\n{}\n\nTool Actions:\n{}",
                user_requests.join("\n"),
                tool_actions.join("\n")
            );
            if let Ok(local_sum) = local_client.compact_conversation(&digest).await {
                if !local_sum.trim().is_empty() {
                    summary_opt = Some(local_sum.trim().to_string());
                }
            }
        }

        // 2. If no local SLM, try calling Cloud API for semantic summary if API key is present
        if summary_opt.is_none() && !cfg.api_key.trim().is_empty() {
            let summary_system = "You are the Context Compactor for UTI CLI. Condense the intermediate conversation turns into a dense technical summary. Include: 1) User Directives & Goals, 2) Files touched/modified, 3) Key decisions & pending tasks.";
            let digest = format!(
                "User Requests:\n{}\n\nTool Actions:\n{}",
                user_requests.join("\n"),
                tool_actions.join("\n")
            );
            let request_body = serde_json::json!({
                "model": if cfg.base_url.contains("deepseek.com") { "deepseek-chat" } else { "deepseek-flash" },
                "messages": [
                    {"role": "system", "content": summary_system},
                    {"role": "user", "content": digest}
                ],
                "max_tokens": 600,
                "temperature": 0.3
            });

            let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
            if let Ok(resp) = self.http
                .post(&url)
                .header("Authorization", format!("Bearer {}", cfg.api_key))
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
            {
                if let Ok(json_resp) = resp.json::<serde_json::Value>().await {
                    if let Some(text) = json_resp
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("message"))
                        .and_then(|m| m.get("content"))
                        .and_then(|t| t.as_str())
                    {
                        summary_opt = Some(text.trim().to_string());
                    }
                }
            }
        }

        // 3. Fallback: structured deterministic extractive summary
        let summary_text = summary_opt.unwrap_or_else(|| {
            let mut parts = Vec::new();
            if let Some(ref prev) = prev_summary {
                parts.push(format!("## Previous Compacted History:\n{}", prev));
            }
            if !user_requests.is_empty() {
                parts.push(format!(
                    "## User Directives in Compacted Turns:\n{}",
                    user_requests.iter().map(|u| format!("- {}", u)).collect::<Vec<_>>().join("\n")
                ));
            }
            if !tool_actions.is_empty() {
                parts.push(format!(
                    "## Touched Files & Actions:\n{}",
                    tool_actions.iter().take(30).map(|a| format!("- {}", a)).collect::<Vec<_>>().join("\n")
                ));
            }
            if parts.is_empty() {
                parts.push("Intermediate conversation history compacted to conserve context window.".to_string());
            }
            parts.join("\n\n")
        });

        let compacted_content = format!(
            "<CONTEXT_SUMMARY>\nThe following is a structured memory compaction of {} intermediate turns to fit within the context window:\n\n{}\n</CONTEXT_SUMMARY>",
            older_count,
            summary_text.trim()
        );

        // Drain ONLY the middle slice and insert the compacted block
        messages.drain(middle_start..split_at);
        let compacted_message = Message::system(compacted_content);
        messages.insert(middle_start, compacted_message);

        let new_tokens = Self::estimate_messages_tokens(messages);
        let saved_tokens = current_tokens.saturating_sub(new_tokens);
        let notice = format!(
            "Context compacted: preserved initial user prompt + active window (saved ~{} tokens).",
            saved_tokens
        );

        Ok(Some(notice))
    }

    /// If the estimated prompt token count is above the threshold, drop the oldest
    /// non-system messages (in message pairs to preserve tool call integrity) until
    /// we are back under the limit or only the system + last turn remains.
    /// DeepSeek-V4.1-Flash features a 1M token context window (1,048,576 tokens).
    const HISTORY_TOKEN_LIMIT: usize = 256_000;

    pub fn cap_history_if_needed(messages: &mut Vec<Message>) {
        if Self::estimate_messages_tokens(messages) <= Self::HISTORY_TOKEN_LIMIT {
            return;
        }

        // Find the first non-system message index
        let first_non_system = messages.iter().position(|m| m.role != "system").unwrap_or(messages.len());

        // Drop oldest messages one at a time (skip the system prompt at index 0)
        while messages.len() > first_non_system + 2
            && Self::estimate_messages_tokens(messages) > Self::HISTORY_TOKEN_LIMIT
        {
            messages.remove(first_non_system);
        }

        debug!(
            "[HISTORY] Capped history. Estimated tokens now: {}",
            Self::estimate_messages_tokens(messages)
        );
    }

    pub async fn stream_chat(
        &self,
        mut messages: Vec<Message>,
        mut tools: Option<Vec<ToolDefinition>>,
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let cfg = self.get_config();

        // ⚡ 100% STANDALONE OFFLINE LOCAL MODEL (Zero Cloud, 100% Local):
        if cfg.model.starts_with("local") {
            let local_client = self.local_client();
            if local_client.health_check().await {
                crate::forensic::ForensicLogger::log_hybrid_decision(
                    messages.last().and_then(|m| m.text_content()).unwrap_or(""),
                    false,
                    "LOCAL_LLM (STANDALONE_OFFLINE)",
                    "Running 100% offline on Local LLM (zero Cloud dependency, $0.00)."
                );
                return local_client.stream_chat(messages, tools, cancel_token).await;
            } else {
                let err_msg = format!("Local LLM not reachable at {}. Start your llama-server/Ollama or switch model with /model.", cfg.local_llm_url);
                let (tx, rx) = mpsc::channel::<StreamEvent>(1);
                tokio::spawn(async move {
                    let _ = tx.send(StreamEvent::Error(err_msg)).await;
                });
                return Ok(rx);
            }
        }

        let mut offline_fallback_notice: Option<String> = None;

        // ⚡ MULTI-MODE DYNAMIC HYBRID ROUTING:
        if cfg.local_llm_enabled {
            let local_client = self.local_client();
            let is_local_online = local_client.health_check().await;

            if !is_local_online {
                let is_tool_call_in_progress = messages.last().map(|m| m.role == "tool").unwrap_or(false);
                if !is_tool_call_in_progress {
                    offline_fallback_notice = Some(format!(
                        "Local LLM is offline at {}. Using Cloud API.",
                        cfg.local_llm_url
                    ));
                }
                // Graceful fallback to DeepSeek Cloud without crashing or blocking the session!
                crate::forensic::ForensicLogger::log_event(
                    "HYBRID_FALLBACK",
                    "LOCAL_OFFLINE -> CLOUD_FALLBACK",
                    &format!("Local server unreachable at {}. Seamlessly falling back to DeepSeek Cloud.", cfg.local_llm_url)
                );
                debug!("[HYBRID_ROUTER] Local server offline at {}; fallback to DeepSeek Cloud", cfg.local_llm_url);
            } else {
                // Hybrid Mode: All agent turns run on DeepSeek Cloud with full tools,
                // while the Local LLM handles parallel output compression & token reduction.
                debug!("[HYBRID_ROUTER] Hybrid active; delegating agent turn to DeepSeek Cloud");
            }
        }

        let api_key = &cfg.api_key;
        if api_key.is_empty() {
            crate::forensic::ForensicLogger::log_error("stream_chat", "Missing API key in Config");
            bail!("API key is missing. Please set UTI_API_KEY / DEEPSEEK_API_KEY or configure ~/.uti/settings.json");
        }

        // ⚡ ORDER OF INTEGRITY:
        // 1. Auto-compact conversation turns if threshold is reached (Gemini CLI / Antigravity spec)
        let auto_compact_event = match self.compact_messages(&mut messages, false).await {
            Ok(Some(notice)) => {
                let compacted_session_messages = if messages.len() > 1 {
                    messages[1..].to_vec()
                } else {
                    Vec::new()
                };
                Some((compacted_session_messages, notice))
            }
            _ => None,
        };
        // 2. Cap history limit as emergency fallback
        Self::cap_history_if_needed(&mut messages);
        // 3. Sanitize tool call sequences so no orphaned messages exist
        Self::sanitize_tool_call_sequences(&mut messages);
        // 4. Compress tool outputs in parallel (preserving past turns for KV Cache hit)
        self.compress_or_truncate_tool_outputs(&mut messages).await;

        if let Some(ref mut t_list) = tools {
            Self::sort_tools(t_list);
        }

        let has_tools = tools.is_some();

        // DeepSeek V4 Reasoning spec:
        for msg in &mut messages {
            if msg.role == "assistant" {
                if msg.content.is_none() {
                    msg.content = Some(MessageContent::Text(String::new()));
                }

                if has_tools {
                    if msg.reasoning_content.is_none() {
                        if let Some(text) = msg.text_content() {
                            let key = ReasoningCache::compute_key(text, msg.tool_calls.as_deref());
                            if let Some(cached) = self.reasoning_cache.get(&key) {
                                msg.reasoning_content = Some(cached);
                            }
                        }
                    }
                    if msg.reasoning_content.is_none() {
                        msg.reasoning_content = Some(String::new());
                    }
                } else {
                    msg.reasoning_content = None;
                }
            }
        }

        let is_official_deepseek = cfg.base_url.contains("deepseek.com");
        let api_model = if cfg.model == "deepseek-chat" {
            "deepseek-chat".to_string()
        } else if cfg.model == "deepseek-reasoner" {
            "deepseek-reasoner".to_string()
        } else if cfg.model.contains("pro") || cfg.model.contains("reasoner") || cfg.model.contains("Pro") {
            if is_official_deepseek {
                "deepseek-reasoner".to_string()
            } else {
                "deepseek-v4-pro".to_string()
            }
        } else if cfg.model == "deepseek-flash"
            || cfg.model == "deepseek-v4.1-flash"
            || cfg.model == "deepseek-v4-flash"
            || cfg.model == "deepseek-v4-flash-vision-exp"
            || cfg.model.contains("flash")
            || cfg.model.contains("Flash")
            || cfg.model.contains("vision")
            || cfg.model.contains("Vision")
        {
            if is_official_deepseek {
                "deepseek-chat".to_string()
            } else {
                "deepseek-flash".to_string()
            }
        } else {
            cfg.model.clone()
        };

        let base_url = cfg.base_url.trim_end_matches('/');
        let url = format!("{}/chat/completions", base_url);

        let tools_count = tools.as_ref().map(|t| t.len()).unwrap_or(0);
        let messages_count = messages.len();
        // ⚡ DYNAMIC ADAPTIVE REASONING EFFORT:
        // When configured as "dynamic" (default for flash), adapt depth per turn:
        // - Tool Call Rounds / System Inspection / Command Execution / Quick Queries -> command_reasoning_effort (default "low" ~200ms)
        // - Heavy Code Generation / Patch Creation / Architecture -> code_reasoning_effort (default "high" Deep CoT)
        let raw_reasoning_effort = if cfg.local_llm_enabled || cfg.reasoning_effort == "dynamic" || cfg.flash_settings.reasoning_effort == "dynamic" {
            let is_tool_response_turn = messages.last().map(|m| m.role == "tool").unwrap_or(false);
            if is_tool_response_turn {
                cfg.flash_settings.command_reasoning_effort.clone()
            } else {
                cfg.flash_settings.code_reasoning_effort.clone()
            }
        } else {
            cfg.reasoning_effort.clone()
        };

        // DeepSeek-V4.1-Flash reasoning_effort normalization:
        // 'minimal' and 'medium' are rejected by DeepSeek-V4.1.
        // Allowed: 'none', 'low' (25), 'high' (50), 'xhigh' (75), 'max' (100)
        let normalized_reasoning_effort = match raw_reasoning_effort.to_lowercase().as_str() {
            "medium" => "high".to_string(),
            "minimal" => "low".to_string(),
            other => other.to_string(),
        };

        let is_thinking_disabled = normalized_reasoning_effort == "none"
            || normalized_reasoning_effort == "off"
            || normalized_reasoning_effort == "false";

        let thinking_config = if is_thinking_disabled {
            Some(ThinkingConfig {
                thinking_type: "disabled".to_string(),
            })
        } else {
            Some(ThinkingConfig {
                thinking_type: "enabled".to_string(),
            })
        };

        let effective_reasoning_effort = if is_thinking_disabled {
            Some("none".to_string())
        } else {
            Some(normalized_reasoning_effort)
        };

        let effective_temperature = if api_model.contains("flash") {
            Some(cfg.flash_settings.temperature)
        } else {
            None
        };

        let request_body = ChatCompletionRequest {
            model: api_model.clone(),
            messages,
            tools,
            stream: true,
            thinking: thinking_config,
            reasoning_effort: effective_reasoning_effort,
            temperature: effective_temperature,
            top_p: None,
            max_tokens: None,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
        };

        let body_serialized = serde_json::to_string_pretty(&request_body).unwrap_or_default();
        crate::forensic::ForensicLogger::log_llm_request(
            "DeepSeek Cloud",
            &api_model,
            &url,
            messages_count,
            tools_count,
            &body_serialized
        );

        debug!("[REQUEST] POST {} (api_model: {})", url, api_model);

        let req_builder = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&request_body);

        let mut event_source = req_builder.eventsource()?;
        let (tx, rx) = mpsc::channel::<StreamEvent>(100);

        let api_model_clone = api_model.clone();
        tokio::spawn(async move {
            if let Some((compacted_messages, notice)) = auto_compact_event {
                let _ = tx.send(StreamEvent::ContextCompacted { compacted_messages, notice }).await;
            }
            if let Some(notice) = offline_fallback_notice {
                let _ = tx.send(StreamEvent::Notice(notice)).await;
            }
            let start_time = std::time::Instant::now();
            let mut first_token_time = None;
            let timeout_duration = std::time::Duration::from_secs(180);
            let mut total_content_chars = 0;
            let mut total_reasoning_chars = 0;
            let mut final_usage: Option<Usage> = None;
            let final_reason: Option<String> = None;

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        crate::forensic::ForensicLogger::log_error("stream_chat", "Request cancelled by user (CancellationToken fired)");
                        let _ = tx.send(StreamEvent::Error("Request cancelled by user".to_string())).await;
                        break;
                    }
                    _ = tokio::time::sleep(timeout_duration) => {
                        crate::forensic::ForensicLogger::log_error("stream_chat", "Stream idle timeout: DeepSeek API did not respond for 180 seconds.");
                        let _ = tx.send(StreamEvent::Error("Stream idle timeout: DeepSeek API did not respond for 180 seconds.".to_string())).await;
                        break;
                    }
                    event = event_source.next() => {
                        match event {
                            Some(Ok(Event::Open)) => {
                                debug!("[SSE] Stream opened");
                            }
                            Some(Ok(Event::Message(msg))) => {
                                if msg.data.trim() == "[DONE]" {
                                    let duration_ms = start_time.elapsed().as_millis();
                                    crate::forensic::ForensicLogger::log_llm_response(
                                        "DeepSeek Cloud",
                                        &api_model_clone,
                                        total_content_chars,
                                        total_reasoning_chars,
                                        duration_ms,
                                        first_token_time,
                                        final_reason.as_deref().or(Some("stop")),
                                        final_usage.as_ref(),
                                    );
                                    let _ = tx.send(StreamEvent::Completed { finish_reason: Some("stop".to_string()) }).await;
                                    break;
                                }

                                if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(&msg.data) {
                                    if let Some(usage) = chunk.usage {
                                        final_usage = Some(usage.clone());
                                        let _ = tx.send(StreamEvent::UsageUpdate(usage)).await;
                                    }

                                    if let Some(choice) = chunk.choices.first() {
                                        if let Some(ref cot) = choice.delta.reasoning_content {
                                            if !cot.is_empty() {
                                                if first_token_time.is_none() {
                                                    first_token_time = Some(start_time.elapsed().as_millis());
                                                }
                                                total_reasoning_chars += cot.len();
                                                let _ = tx.send(StreamEvent::ReasoningDelta(cot.clone())).await;
                                            }
                                        }

                                        if let Some(ref text) = choice.delta.content {
                                            if !text.is_empty() {
                                                if first_token_time.is_none() {
                                                    first_token_time = Some(start_time.elapsed().as_millis());
                                                }
                                                total_content_chars += text.len();
                                                let _ = tx.send(StreamEvent::ContentDelta(text.clone())).await;
                                            }
                                        }

                                        if let Some(ref tc_deltas) = choice.delta.tool_calls {
                                            if !tc_deltas.is_empty() && first_token_time.is_none() {
                                                first_token_time = Some(start_time.elapsed().as_millis());
                                            }
                                            for tc in tc_deltas {
                                                let _ = tx.send(StreamEvent::ToolCallDelta {
                                                    index: tc.index,
                                                    id: tc.id.clone(),
                                                    name: tc.function.as_ref().and_then(|f| f.name.clone()),
                                                    arguments: tc.function.as_ref().and_then(|f| f.arguments.clone()),
                                                }).await;
                                            }
                                        }

                                        if let Some(ref reason) = choice.finish_reason {
                                            let duration_ms = start_time.elapsed().as_millis();
                                            crate::forensic::ForensicLogger::log_llm_response(
                                                "DeepSeek Cloud",
                                                &api_model_clone,
                                                total_content_chars,
                                                total_reasoning_chars,
                                                duration_ms,
                                                first_token_time,
                                                Some(reason),
                                                final_usage.as_ref(),
                                            );
                                            let _ = tx.send(StreamEvent::Completed { finish_reason: Some(reason.clone()) }).await;
                                            break;
                                        }
                                    }
                                }
                            }
                            Some(Err(err)) => {
                                error!("[SSE] Error: {}", err);
                                crate::forensic::ForensicLogger::log_error("stream_chat_sse", &err.to_string());
                                let _ = tx.send(StreamEvent::Error(format!("Stream error: {}", err))).await;
                                break;
                            }
                            None => {
                                let duration_ms = start_time.elapsed().as_millis();
                                crate::forensic::ForensicLogger::log_llm_response(
                                    "DeepSeek Cloud",
                                    &api_model_clone,
                                    total_content_chars,
                                    total_reasoning_chars,
                                    duration_ms,
                                    first_token_time,
                                    final_reason.as_deref(),
                                    final_usage.as_ref(),
                                );
                                let _ = tx.send(StreamEvent::Completed { finish_reason: None }).await;
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    pub async fn check_balance(&self) -> Result<BalanceResponse> {
        let cfg = self.get_config();
        let api_key = &cfg.api_key;
        if api_key.is_empty() {
            bail!("API key is missing.");
        }

        let base_url = cfg.base_url.trim_end_matches('/');
        let url = format!("{}/user/balance", base_url);

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
            .context("Failed to contact DeepSeek user/balance endpoint")?;

        if !resp.status().is_success() {
            bail!("Failed to get balance: HTTP {}", resp.status());
        }

        let bal = resp.json::<BalanceResponse>().await?;
        Ok(bal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_tool_call_sequences() {
        let mut messages = vec![
            Message::system("system prompt"),
            Message::user("do something"),
            Message::assistant_with_tools(
                None,
                None,
                vec![ToolCall {
                    id: "call_123".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "run_shell_command".to_string(),
                        arguments: "{\"command\":\"ls\"}".to_string(),
                    },
                }],
            ),
            Message::tool_response("call_123", "file1.txt\nfile2.txt"),
        ];

        // Should maintain intact sequence
        LlmClient::sanitize_tool_call_sequences(&mut messages);
        assert_eq!(messages.len(), 4);

        // Add an orphan tool response
        messages.push(Message::tool_response("orphan_call", "some output"));
        LlmClient::sanitize_tool_call_sequences(&mut messages);
        // Orphan should be stripped
        assert_eq!(messages.len(), 4);

        // Test intervening system message between assistant tool call and tool response
        let mut messages_with_intervening = vec![
            Message::assistant_with_tools(
                None,
                None,
                vec![ToolCall {
                    id: "call_sudo".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "run_shell_command".to_string(),
                        arguments: "{\"command\":\"sudo whoami\"}".to_string(),
                    },
                }],
            ),
            Message::system("Sudo password saved in session RAM"),
            Message::tool_response("call_sudo", "root\n"),
        ];

        LlmClient::sanitize_tool_call_sequences(&mut messages_with_intervening);
        // Should not insert "cancelled", should put tool response right after assistant!
        assert_eq!(messages_with_intervening.len(), 3);
        assert_eq!(messages_with_intervening[0].role, "assistant");
        assert_eq!(messages_with_intervening[1].role, "tool");
        assert_eq!(messages_with_intervening[1].tool_call_id.as_deref(), Some("call_sudo"));
        assert_eq!(messages_with_intervening[1].text_content(), Some("root\n"));
        assert_eq!(messages_with_intervening[2].role, "system");
    }

    #[tokio::test]
    async fn test_compact_messages_force() {
        let client = LlmClient::new(Config::default());
        let mut messages = vec![
            Message::system("System prompt instructions"),
            Message::user("Hello 1"),
            Message::assistant("Answer 1", None),
            Message::user("Hello 2"),
            Message::assistant("Answer 2", None),
            Message::user("Hello 3"),
            Message::assistant("Answer 3", None),
            Message::user("Hello 4"),
            Message::assistant("Answer 4", None),
            Message::user("Hello 5"),
            Message::assistant("Answer 5", None),
            Message::user("Hello 6"),
            Message::assistant("Answer 6", None),
        ];

        let initial_count = messages.len();
        let result = client.compact_messages(&mut messages, true).await;
        assert!(result.is_ok());
        let notice = result.unwrap();
        assert!(notice.is_some());
        // Should have compacted middle turns, keeping system, root user prompt (anchor), and active window
        assert!(messages.len() < initial_count);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].text_content().unwrap(), "Hello 1");
        assert!(messages[2].text_content().unwrap().contains("<CONTEXT_SUMMARY>"));
    }
}

