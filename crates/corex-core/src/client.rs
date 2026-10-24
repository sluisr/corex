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
        extra_content: Option<serde_json::Value>,
    },
    ExtraContentDelta(serde_json::Value),
    UsageUpdate(Usage),
    Completed {
        finish_reason: Option<String>,
    },
    ToolExecutionStarting {
        call_id: String,
        name: String,
        summary: String,
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

/// True when the provider stopped because the output hit its token limit. Tool calls from such a
/// turn may have truncated arguments (half-written files, cut commands) and must not run.
pub fn is_truncated_finish(finish_reason: Option<&str>) -> bool {
    matches!(finish_reason, Some("length" | "max_tokens" | "model_context_window_exceeded"))
}

/// True when an API error message says the request did not fit the model's context window.
pub fn is_context_overflow_error(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "maximum context length",
        "context_length_exceeded",
        "context length exceeded",
        "exceeds the context window",
        "exceed the context window",
        "prompt is too long",
        "input is too long",
        "reduce the length of the messages",
    ]
    .iter()
    .any(|needle| m.contains(needle))
}

/// What a text-only model reads where the user attached an image.
const IMAGE_OMITTED_NOTE: &str = "[The user attached an image here, but the active model cannot see images. \
It is not saved as a file, so do not search the disk for it. Tell the user to switch to a vision-capable model \
with /model (Claude, GPT-4o or Gemini) and send it again.]";

/// True when an API error message says the model does not accept image input.
pub fn is_image_rejection_error(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    ["image_url", "image input", "images are not supported", "does not support image", "unsupported image", "not support vision"]
        .iter()
        .any(|needle| m.contains(needle))
}

/// Tool result returned for every call of a truncated turn so the model re-issues it.
pub const TRUNCATED_TOOL_CALL_ERROR: &str = "Error: this tool call was NOT executed because the response hit the output token limit and its arguments may be truncated. Re-issue it, splitting large content into several smaller calls (e.g. write the file in parts or use apply_patch).";

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    config: Arc<Mutex<Config>>,
    reasoning_cache: ReasoningCache,
    local_client: Arc<Mutex<crate::local_client::LocalLlmClient>>,
}

impl LlmClient {
    pub fn new(config: Config) -> Self {
        let _ = crate::engine_signature();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(10))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

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
        self.config.lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
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
        self.local_client.lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    pub fn reasoning_cache(&self) -> &ReasoningCache {
        &self.reasoning_cache
    }

    /// Heals accidental Chinese drift by asking the LLM to re-express the text in the user's conversation language dynamically.
    pub async fn heal_cjk_drift(&self, text: &str) -> anyhow::Result<String> {
        let cfg = self.get_config();
        let (base_url, api_key) = cfg.endpoint();
        if api_key.trim().is_empty() {
            anyhow::bail!("No API key configured for translation");
        }
        let prompt = format!(
            "The following response unintentionally drifted into Chinese. \
            Re-express and translate it accurately into the exact language of the user's conversation. \
            Preserve all markdown formatting, tables, bullet points, source links, and code blocks exactly. \
            Output ONLY the translation with no preamble or commentary:\n\n{}",
            text
        );
        if cfg.provider_kind() == crate::providers::ProviderKind::Anthropic {
            let trimmed_base = base_url.trim_end_matches('/');
            let url = if trimmed_base.ends_with("/v1") {
                format!("{}/messages", trimmed_base)
            } else {
                format!("{}/v1/messages", trimmed_base)
            };
            let request_body = serde_json::json!({
                "model": cfg.model,
                "max_tokens": 2048,
                "temperature": 0.1,
                "messages": [
                    {"role": "user", "content": prompt}
                ]
            });
            let resp = self.http
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json")
                .json(&request_body)
                .timeout(Duration::from_secs(20))
                .send()
                .await?;
            let val: serde_json::Value = resp.json().await?;
            let translated = val.get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.iter().find_map(|item| item.get("text").and_then(|t| t.as_str())))
                .map(|s| s.trim().to_string())
                .ok_or_else(|| anyhow::anyhow!("Empty Anthropic translation response"))?;
            return Ok(translated);
        }
        let request_body = serde_json::json!({
            "model": if cfg.is_deepseek_endpoint() { "deepseek-flash".to_string() } else { cfg.model.clone() },
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "temperature": 0.1,
            "max_tokens": 2048
        });
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let resp = self.http
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .timeout(Duration::from_secs(20))
            .send()
            .await?;

        let val: serde_json::Value = resp.json().await?;
        let translated = val.get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|t| t.as_str())
            .map(|s| s.trim().to_string())
            .ok_or_else(|| anyhow::anyhow!("Empty translation response"))?;

        Ok(translated)
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

    /// True when the message carries an image.
    fn has_images(message: &Message) -> bool {
        matches!(&message.content, Some(MessageContent::Parts(parts)) if parts.iter().any(|p| p.image_url.is_some()))
    }

    /// Replaces the images of multipart messages with a short note, for models that only read text.
    ///
    /// The note says what to do, because left alone the model tends to go looking for the image on
    /// disk: the image only ever existed in memory, so there is no file to find.
    ///
    /// Such an endpoint rejects image parts outright, and since the message stays in the conversation
    /// every later request would be rejected too.
    pub fn replace_images_with_notes(messages: &mut [Message]) {
        for message in messages.iter_mut() {
            let Some(MessageContent::Parts(parts)) = &message.content else {
                continue;
            };
            if !parts.iter().any(|p| p.image_url.is_some()) {
                continue;
            }
            let text = parts
                .iter()
                .map(|p| match (&p.text, &p.image_url) {
                    (Some(text), _) => text.as_str(),
                    (None, Some(_)) => IMAGE_OMITTED_NOTE,
                    (None, None) => "",
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            message.content = Some(MessageContent::Text(text));
        }
    }

    /// Removes Anthropic thinking blocks stored in tool calls' `extra_content`.
    pub fn strip_anthropic_thinking(messages: &mut [Message]) {
        for call in messages
            .iter_mut()
            .filter_map(|m| m.tool_calls.as_mut())
            .flat_map(|calls| calls.iter_mut())
        {
            let now_empty = match call.extra_content.as_mut().and_then(|ec| ec.as_object_mut()) {
                Some(obj) => {
                    obj.remove(crate::anthropic::THINKING_EXTRA_KEY);
                    obj.is_empty()
                }
                None => false,
            };
            if now_empty {
                call.extra_content = None;
            }
        }
    }

    /// Sanitizes tool call history for Google Gemini models:
    /// Google requires a valid cryptographic `thought_signature` on every tool_call in functionCall parts.
    /// If older turns (e.g. from previous sessions or other models like DeepSeek) lack `thought_signature`,
    /// Google rejects the entire request with 400 Bad Request ("Function call is missing a thought_signature").
    /// This method safely converts unsigned tool calls and their responses into readable text assistant turns.
    pub fn sanitize_for_google_tool_calls(messages: &mut Vec<Message>) {
        let mut i = 0;
        while i < messages.len() {
            if messages[i].role == "assistant" {
                let has_unsigned_tool_call = if let Some(ref calls) = messages[i].tool_calls {
                    calls.iter().any(|c| {
                        c.extra_content.as_ref()
                            .and_then(|ec| ec.get("google"))
                            .and_then(|g| g.get("thought_signature"))
                            .and_then(|s| s.as_str())
                            .map(|s| s.is_empty())
                            .unwrap_or(true)
                    })
                } else {
                    false
                };

                if has_unsigned_tool_call {
                    let calls = messages[i].tool_calls.take().unwrap_or_default();
                    let mut converted_text = messages[i].text_content().unwrap_or("").to_string();

                    let j = i + 1;
                    while j < messages.len() && messages[j].role == "tool" {
                        let tool_msg = messages.remove(j);
                        let tool_output = tool_msg.text_content().unwrap_or("");
                        let tool_id = tool_msg.tool_call_id.as_deref().unwrap_or("");
                        let call_name = calls.iter().find(|c| c.id == tool_id).map(|c| c.function.name.as_str()).unwrap_or("tool");
                        if !converted_text.is_empty() {
                            converted_text.push_str("\n\n");
                        }
                        converted_text.push_str(&format!("[Executed {}: {}]", call_name, tool_output.trim()));
                    }

                    if converted_text.trim().is_empty() {
                        converted_text = "[Tool calls completed]".to_string();
                    }
                    messages[i].content = Some(MessageContent::Text(converted_text));
                }
            }
            i += 1;
        }

        // Merge consecutive plain-text assistant messages created by collapsing unsigned tool turns
        // so Gemini never sees adjacent model turns without an intervening user/tool turn.
        let mut idx = 0;
        while idx + 1 < messages.len() {
            if messages[idx].role == "assistant"
                && messages[idx + 1].role == "assistant"
                && messages[idx].tool_calls.is_none()
            {
                let next_msg = messages.remove(idx + 1);
                let prev_text = messages[idx].text_content().unwrap_or("").to_string();
                let next_text = next_msg.text_content().unwrap_or("").to_string();
                let merged = match (prev_text.is_empty(), next_text.is_empty()) {
                    (false, false) => format!("{}\n\n{}", prev_text, next_text),
                    (false, true) => prev_text,
                    (true, false) => next_text,
                    (true, true) => String::new(),
                };
                messages[idx].content = Some(MessageContent::Text(merged));
                if messages[idx].tool_calls.is_none() {
                    messages[idx].tool_calls = next_msg.tool_calls;
                }
                continue;
            }
            idx += 1;
        }
    }

    /// Normalizes tools by sorting them alphabetically to guarantee 100% prefix matching
    /// for DeepSeek's KV cache pricing discount.
    pub fn sort_tools(tools: &mut [ToolDefinition]) {
        tools.sort_by(|a, b| a.function.name.cmp(&b.function.name));
    }

    /// Truncates tool response messages and background task / subagent dumps that exceed the
    /// spill threshold (`SPILL_THRESHOLD_CHARS = 30_000`), preserving both head and tail and
    /// saving the full output to `/tmp/corex-spill-...` so the model never loses the tail or path.
    const TOOL_OUTPUT_MAX_CHARS: usize = crate::spill::SPILL_THRESHOLD_CHARS;

    pub fn truncate_tool_outputs(messages: &mut [Message]) {
        for msg in messages.iter_mut() {
            let is_truncatable = if msg.role == "tool" {
                true
            } else if msg.role == "system" {
                msg.text_content()
                    .map(|t| t.starts_with("TASK_DONE|") || t.starts_with("SUBAGENT_DONE|"))
                    .unwrap_or(false)
            } else {
                false
            };

            if is_truncatable {
                if let Some(content) = msg.text_content().map(|s| s.to_string()) {
                    if content.chars().count() > Self::TOOL_OUTPUT_MAX_CHARS
                        && !content.contains(crate::spill::TRIM_MARKER)
                        && !content.contains("chars truncated to save tokens")
                    {
                        msg.content = Some(
                            crate::spill::spill_large_output("tool_output", content).into(),
                        );
                    }
                }
            }
        }
    }

    /// Estimates token count of a string using a conservative ~3 bytes/token ratio
    /// that accurately reflects source code, JSON, diffs, and multi-byte UTF-8 text.
    fn estimate_tokens(s: &str) -> usize {
        s.len().div_ceil(3)
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
            let tool_call_toks = m.tool_calls.as_ref().map(|calls| {
                calls.iter().map(|c| {
                    let extra_toks = c.extra_content.as_ref().map(|ec| Self::estimate_tokens(&ec.to_string())).unwrap_or(0);
                    Self::estimate_tokens(&c.function.name) + Self::estimate_tokens(&c.function.arguments) + extra_toks + 8
                }).sum::<usize>()
            }).unwrap_or(0);
            content_toks + reasoning_toks + tool_call_toks + 4 // per-message overhead
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

        // Cheapest relief first: cut oversized tool results down to head + tail. It needs no model
        // call, and if it brings the prompt back under the threshold no summary is needed.
        let prune = crate::context_prune::prune_tool_results(messages);
        let prune_note = (prune.pruned > 0).then(|| {
            let freed = current_tokens.saturating_sub(Self::estimate_messages_tokens(messages));
            format!(
                "Trimmed {} oversized tool result(s), freeing ~{} tokens.",
                prune.pruned, freed
            )
        });
        if !force && prune_note.is_some() && Self::estimate_messages_tokens(messages) <= effective_threshold {
            return Ok(prune_note);
        }

        let total_msgs = messages.len();
        if total_msgs < 8 {
            return Ok(prune_note);
        }

        // Root user prompt anchor (first non-system message).
        let first_non_system = messages.iter().position(|m| m.role != "system").unwrap_or(0);

        // Locate the last user message (active directive that initiated the current work).
        let last_user_idx = messages.iter().rposition(|m| m.role == "user");

        // Determine the middle slice to compact while preserving both `first_non_system`
        // (session anchor) and `last_user_idx` (active directive).
        let (middle_start, mut split_at) = if let Some(last_u_idx) = last_user_idx {
            let pre_user_start = first_non_system + 1;
            let pre_user_count = last_u_idx.saturating_sub(pre_user_start);
            let only_summary_before = pre_user_count == 1
                && messages[pre_user_start].role == "system"
                && messages[pre_user_start]
                    .text_content()
                    .map(|t| t.contains("<CONTEXT_SUMMARY>"))
                    .unwrap_or(false);
            let post_user_count = total_msgs.saturating_sub(last_u_idx + 1);

            if pre_user_count >= 2 && (!only_summary_before && post_user_count < 8) {
                (pre_user_start, last_u_idx)
            } else if post_user_count >= 6 {
                let anchor = last_u_idx;
                let keep_recent = 6.min(total_msgs.saturating_sub(anchor + 3)).max(4);
                let start = anchor + 1;
                let end = total_msgs.saturating_sub(keep_recent);
                (start, end)
            } else if pre_user_count >= 2 {
                (pre_user_start, last_u_idx)
            } else {
                let keep_recent = 8.min(total_msgs.saturating_sub(first_non_system + 4)).max(4);
                let start = first_non_system + 1;
                let end = total_msgs.saturating_sub(keep_recent);
                (start, end)
            }
        } else {
            let keep_recent = 8.min(total_msgs.saturating_sub(first_non_system + 4)).max(4);
            let start = first_non_system + 1;
            let end = total_msgs.saturating_sub(keep_recent);
            (start, end)
        };

        // Never split an atomic `assistant(tool_calls) -> tool` group at `split_at`.
        while split_at > middle_start && split_at < messages.len() && messages[split_at].role == "tool" {
            split_at -= 1;
        }
        // If stepping back landed on the assistant message that owns those tool responses, include
        // that assistant + its tool responses in the kept tail rather than orphaning them.
        if split_at > middle_start
            && split_at < messages.len()
            && messages[split_at].role == "assistant"
            && messages[split_at].tool_calls.is_some()
        {
            // `split_at` is exclusive in `middle_start..split_at`, so `messages[split_at]` is the
            // first message KEPT after compaction, keeping the assistant and its tool outputs together.
        }

        if middle_start >= split_at || split_at.saturating_sub(middle_start) < 2 {
            return Ok(prune_note);
        }

        let older_slice = &messages[middle_start..split_at];
        let older_count = older_slice.len();

        // Extract intermediate user requests, tool actions, and errors
        let mut user_requests = Vec::new();
        let mut tool_actions = Vec::new();
        let mut prev_summary: Option<String> = None;

        for m in older_slice {
            // A prompt that carries images is multipart: `display_text` keeps the user's words.
            let full_text = m.display_text();
            let text = full_text.as_deref().unwrap_or("").trim();
            if m.role == "system" && text.contains("<CONTEXT_SUMMARY>") {
                prev_summary = Some(text.to_string());
                continue;
            }

            if m.role == "user" {
                if !text.is_empty() && !crate::tool_images::is_tool_image_message(m) {
                    user_requests.push(crate::types::truncate_ellipsis(text, 500));
                }
            } else if m.role == "assistant" {
                if let Some(ref calls) = m.tool_calls {
                    for c in calls {
                        let short_args = crate::types::truncate_ellipsis(&c.function.arguments, 160);
                        tool_actions.push(format!("{}({})", c.function.name, short_args));
                    }
                }
            } else if m.role == "tool" && (text.starts_with("Error") || text.contains("failed") || text.contains("error:")) {
                let err_snip = crate::types::truncate_ellipsis(text, 160);
                tool_actions.push(format!("Tool result error: {}", err_snip));
            }
        }

        let mut summary_opt: Option<String> = None;
        let mut digest_parts = Vec::new();
        if let Some(ref prev) = prev_summary {
            digest_parts.push(format!("Prior Context Summary:\n{}", prev));
        }
        if !user_requests.is_empty() {
            digest_parts.push(format!("User Directives:\n{}", user_requests.join("\n")));
        }
        if !tool_actions.is_empty() {
            digest_parts.push(format!("Tool Actions:\n{}", tool_actions.join("\n")));
        }
        let digest = digest_parts.join("\n\n");

        // 1. If local SLM is enabled, try local compaction first
        if cfg.local_llm_enabled {
            if let Ok(local_summary) = self.local_client().compact_conversation(&digest).await {
                if !local_summary.trim().is_empty() {
                    summary_opt = Some(local_summary.trim().to_string());
                }
            }
        }

        // 2. If no local SLM, try calling Cloud API for semantic summary if API key is present
        let (ep_base_url, ep_api_key) = cfg.endpoint();
        if summary_opt.is_none() && !ep_api_key.trim().is_empty() {
            let summary_system = "You are the Context Compactor for Corex. Condense the intermediate conversation turns into a dense technical summary. Include: 1) User Directives & Goals, 2) Files touched/modified, 3) Key decisions & pending tasks.";
            if cfg.provider_kind() == crate::providers::ProviderKind::Anthropic {
                let trimmed_base = ep_base_url.trim_end_matches('/');
                let url = if trimmed_base.ends_with("/v1") {
                    format!("{}/messages", trimmed_base)
                } else {
                    format!("{}/v1/messages", trimmed_base)
                };
                let request_body = serde_json::json!({
                    "model": cfg.model,
                    "max_tokens": 600,
                    "temperature": 0.3,
                    "system": summary_system,
                    "messages": [
                        {"role": "user", "content": digest}
                    ]
                });
                if let Ok(resp) = self.http
                    .post(&url)
                    .header("x-api-key", &ep_api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("Content-Type", "application/json")
                    .json(&request_body)
                    .send()
                    .await
                {
                    if let Ok(json_resp) = resp.json::<serde_json::Value>().await {
                        if let Some(text) = json_resp
                            .get("content")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| arr.iter().find_map(|item| item.get("text").and_then(|t| t.as_str())))
                        {
                            summary_opt = Some(text.trim().to_string());
                        }
                    }
                }
            } else {
                let request_body = serde_json::json!({
                    "model": if cfg.is_deepseek_endpoint() { "deepseek-flash".to_string() } else { cfg.model.clone() },
                    "messages": [
                        {"role": "system", "content": summary_system},
                        {"role": "user", "content": digest}
                    ],
                    "max_tokens": 600,
                    "temperature": 0.3
                });

                let url = format!("{}/chat/completions", ep_base_url.trim_end_matches('/'));
                if let Ok(resp) = self.http
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", ep_api_key))
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
    /// intermediate messages (preserving system prompt, root user prompt when possible,
    /// and atomic tool-call groups) until we are back under the limit.
    const HISTORY_TOKEN_LIMIT: usize = 120_000;
    const LOCAL_HISTORY_TOKEN_LIMIT: usize = 24_000;

    pub fn cap_history_if_needed(messages: &mut Vec<Message>) {
        Self::cap_history_with_limit(messages, Self::HISTORY_TOKEN_LIMIT);
    }

    pub fn cap_history_with_limit(messages: &mut Vec<Message>, token_limit: usize) {
        if Self::estimate_messages_tokens(messages) <= token_limit {
            return;
        }

        let first_non_system = messages
            .iter()
            .position(|m| m.role != "system")
            .unwrap_or(messages.len());

        // Prefer dropping after the root user anchor (first_non_system + 1) while there are enough turns;
        // fall back to first_non_system only if the anchor itself makes history exceed the limit.
        while messages.len() > first_non_system + 2
            && Self::estimate_messages_tokens(messages) > token_limit
        {
            let last_user_idx = messages.iter().rposition(|m| m.role == "user");
            let mut drop_idx = if messages.len() > first_non_system + 3 {
                first_non_system + 1
            } else {
                first_non_system
            };

            // Never drop the latest active user prompt if there are alternative turns to trim
            if let Some(last_u) = last_user_idx {
                if drop_idx == last_u {
                    if drop_idx + 1 < messages.len() {
                        drop_idx += 1;
                    } else {
                        break;
                    }
                }
            }

            if drop_idx >= messages.len() {
                break;
            }

            let removed = messages.remove(drop_idx);
            // If we removed an assistant message with tool calls, also remove its immediate tool responses
            if removed.role == "assistant" && removed.tool_calls.is_some() {
                while drop_idx < messages.len() && messages[drop_idx].role == "tool" {
                    messages.remove(drop_idx);
                }
            }
        }

        debug!(
            "[HISTORY] Capped history to limit {}. Estimated tokens now: {}",
            token_limit,
            Self::estimate_messages_tokens(messages)
        );
    }

    pub async fn stream_chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let mut rx = self
            .stream_chat_once(messages.clone(), tools.clone(), cancel_token.clone())
            .await?;

        // Look at the first real event. A context-overflow rejection arrives before any content, so
        // this is the one moment where compacting and retrying is invisible to the consumer.
        let mut prefix = Vec::new();
        let first = loop {
            match rx.recv().await {
                Some(ev @ StreamEvent::ContextCompacted { .. }) => prefix.push(ev),
                other => break other,
            }
        };
        let overflow = matches!(&first, Some(StreamEvent::Error(m)) if is_context_overflow_error(m));
        let image_rejected = matches!(&first, Some(StreamEvent::Error(m)) if is_image_rejection_error(m))
            && messages.iter().any(Self::has_images);

        let (tx, out) = mpsc::channel::<StreamEvent>(100);
        if overflow {
            let mut retry_messages = messages;
            if let Ok(Some(notice)) = self.compact_messages(&mut retry_messages, true).await {
                let compacted_session = if retry_messages.len() > 1 {
                    retry_messages[1..].to_vec()
                } else {
                    Vec::new()
                };
                let mut retry_rx = self
                    .stream_chat_once(retry_messages, tools, cancel_token)
                    .await?;
                let notice = format!("The request exceeded the model's context window. {} Retrying.", notice);
                tokio::spawn(async move {
                    let _ = tx
                        .send(StreamEvent::ContextCompacted { compacted_messages: compacted_session, notice })
                        .await;
                    while let Some(ev) = retry_rx.recv().await {
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                });
                return Ok(out);
            }
        } else if image_rejected {
            // The model does not take images after all. Send the same conversation with a note in
            // place of each image; the session keeps the images for a model that can see them.
            let mut retry_messages = messages;
            Self::replace_images_with_notes(&mut retry_messages);
            let mut retry_rx = self.stream_chat_once(retry_messages, tools, cancel_token).await?;
            let notice = "The active model rejected the image, so it was sent as a note. \
                          Switch with /model to one that can see images."
                .to_string();
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::Notice(notice)).await;
                while let Some(ev) = retry_rx.recv().await {
                    if tx.send(ev).await.is_err() {
                        break;
                    }
                }
            });
            return Ok(out);
        }

        tokio::spawn(async move {
            for ev in prefix {
                if tx.send(ev).await.is_err() {
                    return;
                }
            }
            if let Some(ev) = first {
                if tx.send(ev).await.is_err() {
                    return;
                }
            }
            while let Some(ev) = rx.recv().await {
                if tx.send(ev).await.is_err() {
                    break;
                }
            }
        });
        Ok(out)
    }

    async fn stream_chat_once(
        &self,
        mut messages: Vec<Message>,
        mut tools: Option<Vec<ToolDefinition>>,
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let cfg = self.get_config();
        let is_local_model = cfg.model.starts_with("local");

        if is_local_model {
            let local_client = self.local_client();
            if local_client.health_check().await {
                let compact_notice = self.compact_messages(&mut messages, false).await.ok().flatten();
                Self::sanitize_tool_call_sequences(&mut messages);
                let auto_compact_event = compact_notice.map(|notice| {
                    let compacted_session_messages = if messages.len() > 1 {
                        messages[1..].to_vec()
                    } else {
                        Vec::new()
                    };
                    (compacted_session_messages, notice)
                });
                Self::cap_history_with_limit(&mut messages, Self::LOCAL_HISTORY_TOKEN_LIMIT);
                Self::sanitize_tool_call_sequences(&mut messages);
                Self::truncate_tool_outputs(&mut messages);
                crate::language::sanitize_messages_cjk_drift(&mut messages);

                crate::forensic::ForensicLogger::log_event(
                    "LOCAL_LLM",
                    "STANDALONE_OFFLINE",
                    "Routing request to local LLM endpoint.",
                );
                let mut inner_rx = local_client.stream_chat(messages, tools, cancel_token).await?;
                if let Some((compacted_messages, notice)) = auto_compact_event {
                    let (tx, rx) = mpsc::channel::<StreamEvent>(100);
                    tokio::spawn(async move {
                        let _ = tx
                            .send(StreamEvent::ContextCompacted {
                                compacted_messages,
                                notice,
                            })
                            .await;
                        while let Some(evt) = inner_rx.recv().await {
                            if tx.send(evt).await.is_err() {
                                break;
                            }
                        }
                    });
                    return Ok(rx);
                }
                return Ok(inner_rx);
            } else {
                let err_msg = format!(
                    "Local LLM not reachable at {}. Start your llama-server/Ollama or switch model with /model.",
                    cfg.local_llm_url
                );
                let (tx, rx) = mpsc::channel::<StreamEvent>(1);
                tokio::spawn(async move {
                    let _ = tx.send(StreamEvent::Error(err_msg)).await;
                });
                return Ok(rx);
            }
        }

        let (endpoint_url, api_key_owned) = cfg.endpoint();
        let api_key = &api_key_owned;
        let is_deepseek = cfg.is_deepseek_endpoint();
        if api_key.is_empty() {
            crate::forensic::ForensicLogger::log_error("stream_chat", "Missing API key in Config");
            if is_deepseek {
                bail!("API key is missing. Please set COREX_API_KEY / DEEPSEEK_API_KEY or configure ~/.corex/settings.json");
            } else {
                bail!(
                    "API key for provider '{}' is missing. Export its API key environment variable (see /model) or switch provider.",
                    cfg.active_provider.as_deref().unwrap_or("?")
                );
            }
        }

        let compact_notice = self.compact_messages(&mut messages, false).await.ok().flatten();
        Self::sanitize_tool_call_sequences(&mut messages);
        // Capture compacted session messages before wire-only mutations
        // (cap_history_if_needed, sanitize_for_google_tool_calls, truncate_tool_outputs).
        let auto_compact_event = compact_notice.map(|notice| {
            let compacted_session_messages = if messages.len() > 1 {
                messages[1..].to_vec()
            } else {
                Vec::new()
            };
            (compacted_session_messages, notice)
        });
        Self::cap_history_if_needed(&mut messages);
        Self::sanitize_tool_call_sequences(&mut messages);
        if cfg.provider_kind() == crate::providers::ProviderKind::Google {
            Self::sanitize_for_google_tool_calls(&mut messages);
        }
        Self::truncate_tool_outputs(&mut messages);
        crate::language::sanitize_messages_cjk_drift(&mut messages);

        // Strip legacy recency anchors spliced onto trailing tool outputs by older builds
        // before applying a fresh standalone anchor.
        crate::language::sanitize_messages_recency_anchor(&mut messages);

        // Re-anchor the conversation language at the tail of the context window as a
        // standalone message (never spliced into untrusted tool payloads) so long tool
        // outputs do not cause DeepSeek to drift into Chinese.
        if is_deepseek {
            crate::language::apply_recency_anchor(&mut messages);
        }

        if let Some(ref mut t_list) = tools {
            Self::sort_tools(t_list);
        }

        let provider_kind = cfg.provider_kind();
        if provider_kind != crate::providers::ProviderKind::Anthropic {
            // Signed Claude thinking blocks are only meaningful to Anthropic; never forward
            // them to OpenAI-compatible endpoints after a mid-session provider switch.
            Self::strip_anthropic_thinking(&mut messages);
        }
        if !cfg.can_see_images() {
            Self::replace_images_with_notes(&mut messages);
        }
        if provider_kind == crate::providers::ProviderKind::Anthropic {
            return self.stream_chat_anthropic(
                &endpoint_url,
                api_key,
                &cfg.model,
                messages,
                tools,
                cancel_token,
                auto_compact_event,
            ).await;
        }

        let has_tools = tools.is_some();

        // DeepSeek V4 Reasoning spec:
        for msg in &mut messages {
            if msg.role == "assistant" {
                if msg.content.is_none() {
                    msg.content = Some(MessageContent::Text(String::new()));
                }

                if has_tools && is_deepseek {
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

        let api_model = if !is_deepseek {
            if provider_kind == crate::providers::ProviderKind::Google {
                if cfg.model == "gemini-2.5-flash" || cfg.model == "gemini-2.0-flash" || cfg.model == "gemini-1.5-flash" {
                    "gemini-3.5-flash-lite".to_string()
                } else if cfg.model == "gemini-2.5-pro" {
                    "gemini-3.8-flash".to_string()
                } else {
                    cfg.model.clone()
                }
            } else {
                cfg.model.clone()
            }
        } else if cfg.model == "deepseek-v4-pro"
            || cfg.model == "deepseek-reasoner"
            || cfg.model.contains("pro")
            || cfg.model.contains("reasoner")
            || cfg.model.contains("Pro")
        {
            "deepseek-v4-pro".to_string()
        } else if cfg.model == "deepseek-flash"
            || cfg.model == "deepseek-v4.1-flash"
            || cfg.model == "deepseek-v4.1-flash-1017"
            || cfg.model == "deepseek-v4-flash"
            || cfg.model == "deepseek-v4-flash-vision-exp"
            || cfg.model == "deepseek-chat"
            || cfg.model.contains("flash")
            || cfg.model.contains("Flash")
            || cfg.model.contains("vision")
            || cfg.model.contains("Vision")
        {
            if cfg.model == "deepseek-v4.1-flash-1017" {
                "deepseek-v4.1-flash-1017".to_string()
            } else {
                "deepseek-flash".to_string()
            }
        } else {
            cfg.model.clone()
        };

        let base_url = endpoint_url.trim_end_matches('/');
        let url = format!("{}/chat/completions", base_url);

        let tools_count = tools.as_ref().map(|t| t.len()).unwrap_or(0);
        let messages_count = messages.len();
        // When configured as "dynamic" (default for flash), adapt reasoning depth per turn:
        // tool-response turns use DYNAMIC_TOOL_TURN_EFFORT ("low"), others use DYNAMIC_DEFAULT_EFFORT ("high").
        let is_flash = !crate::config::is_pro_model(&cfg.model)
            && (cfg.model.to_ascii_lowercase().contains("flash") || cfg.model == "deepseek-chat");
        let raw_reasoning_effort = if cfg.local_llm_enabled
            || cfg.reasoning_effort == "dynamic"
            || (is_flash && cfg.flash_settings.reasoning_effort == "dynamic")
        {
            let is_tool_response_turn = messages.last().map(|m| m.role == "tool").unwrap_or(false);
            if is_tool_response_turn {
                crate::config::DYNAMIC_TOOL_TURN_EFFORT.to_string()
            } else {
                crate::config::DYNAMIC_DEFAULT_EFFORT.to_string()
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

        let is_openai_reasoning = api_model.starts_with("o1") || api_model.starts_with("o3");

        // `thinking` / `reasoning_effort` are DeepSeek extensions; OpenAI reasoning models (o1/o3)
        // support standard `reasoning_effort`. Other OpenAI-compatible providers reject unknown fields.
        let thinking_config = if !is_deepseek {
            None
        } else if is_thinking_disabled {
            Some(ThinkingConfig {
                thinking_type: "disabled".to_string(),
            })
        } else {
            Some(ThinkingConfig {
                thinking_type: "enabled".to_string(),
            })
        };

        let effective_reasoning_effort = if is_deepseek {
            if is_thinking_disabled {
                Some("none".to_string())
            } else {
                Some(normalized_reasoning_effort)
            }
        } else if is_openai_reasoning {
            match normalized_reasoning_effort.to_lowercase().as_str() {
                "low" => Some("low".to_string()),
                "high" | "max" | "xhigh" => Some("high".to_string()),
                _ => Some("medium".to_string()),
            }
        } else {
            None
        };

        let effective_temperature = if is_openai_reasoning {
            // OpenAI o1/o3 reject the temperature parameter with 400 Bad Request
            None
        } else if !is_deepseek {
            Some(cfg.temperature)
        } else if is_thinking_disabled {
            if api_model.contains("flash") {
                Some(cfg.flash_settings.temperature)
            } else {
                Some(cfg.temperature)
            }
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

        let provider_label = match provider_kind {
            crate::providers::ProviderKind::DeepSeek => "DeepSeek Cloud",
            crate::providers::ProviderKind::OpenAi => "OpenAI Cloud",
            crate::providers::ProviderKind::Google => "Google Gemini",
            crate::providers::ProviderKind::GitHub => "GitHub Models",
            crate::providers::ProviderKind::Groq => "Groq Cloud",
            crate::providers::ProviderKind::OpenRouter => "OpenRouter",
            crate::providers::ProviderKind::Mistral => "Mistral AI",
            crate::providers::ProviderKind::Local => "Local LLM",
            crate::providers::ProviderKind::Anthropic => "Anthropic Claude",
            crate::providers::ProviderKind::GenericOpenAi => "OpenAI Compatible",
        };

        if crate::forensic::ForensicLogger::is_enabled() {
            let body_serialized = serde_json::to_string(&request_body).unwrap_or_default();
            crate::forensic::ForensicLogger::log_llm_request(
                provider_label,
                &api_model,
                &url,
                messages_count,
                tools_count,
                &body_serialized,
            );
        }

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
        let provider_label_clone = provider_label.to_string();
        tokio::spawn(async move {
            if let Some((compacted_messages, notice)) = auto_compact_event {
                let _ = tx.send(StreamEvent::ContextCompacted { compacted_messages, notice }).await;
            }
            let start_time = std::time::Instant::now();
            let mut first_token_time = None;
            let timeout_duration = std::time::Duration::from_secs(180);
            let mut total_content_chars = 0;
            let mut total_reasoning_chars = 0;
            let mut final_usage: Option<Usage> = None;
            let mut final_reason: Option<String> = None;

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        crate::forensic::ForensicLogger::log_error("stream_chat", "Request cancelled by user (CancellationToken fired)");
                        let _ = tx.send(StreamEvent::Error("Request cancelled by user".to_string())).await;
                        break;
                    }
                    _ = tokio::time::sleep(timeout_duration) => {
                        crate::forensic::ForensicLogger::log_error("stream_chat", "Stream idle timeout: API did not respond for 180 seconds.");
                        let _ = tx.send(StreamEvent::Error("Stream idle timeout: API did not respond for 180 seconds.".to_string())).await;
                        break;
                    }
                    event = event_source.next() => {
                        match event {
                            Some(Ok(Event::Open)) => {
                                debug!("[SSE] Stream opened");
                            }
                            Some(Ok(Event::Message(msg))) => {
                                if let Some(events) = parse_openai_sse_data(&msg.data) {
                                    let mut should_break = false;
                                    for ev in events {
                                        match ev {
                                            StreamEvent::UsageUpdate(ref u) => {
                                                final_usage = Some(u.clone());
                                                if tx.send(ev).await.is_err() { return; }
                                                if final_reason.is_some() {
                                                    let duration_ms = start_time.elapsed().as_millis();
                                                    crate::forensic::ForensicLogger::log_llm_response(
                                                        &provider_label_clone,
                                                        &api_model_clone,
                                                        total_content_chars,
                                                        total_reasoning_chars,
                                                        duration_ms,
                                                        first_token_time,
                                                        final_reason.as_deref(),
                                                        final_usage.as_ref(),
                                                    );
                                                    let _ = tx.send(StreamEvent::Completed { finish_reason: final_reason.clone() }).await;
                                                    should_break = true;
                                                }
                                            }
                                            StreamEvent::ReasoningDelta(ref cot) => {
                                                if first_token_time.is_none() {
                                                    first_token_time = Some(start_time.elapsed().as_millis());
                                                }
                                                total_reasoning_chars += cot.len();
                                                if tx.send(ev).await.is_err() { return; }
                                            }
                                            StreamEvent::ContentDelta(ref text) => {
                                                if first_token_time.is_none() {
                                                    first_token_time = Some(start_time.elapsed().as_millis());
                                                }
                                                total_content_chars += text.len();
                                                if tx.send(ev).await.is_err() { return; }
                                            }
                                            StreamEvent::ToolCallDelta { .. } => {
                                                if first_token_time.is_none() {
                                                    first_token_time = Some(start_time.elapsed().as_millis());
                                                }
                                                if tx.send(ev).await.is_err() { return; }
                                            }
                                            StreamEvent::ExtraContentDelta(_) => {
                                                if tx.send(ev).await.is_err() { return; }
                                            }
                                            StreamEvent::Completed { ref finish_reason } => {
                                                if finish_reason.is_some() {
                                                    final_reason = finish_reason.clone();
                                                }
                                                // If usage has already arrived or this is the terminal [DONE] marker
                                                // (finish_reason is None), complete the stream immediately.
                                                // Otherwise wait for the trailing usage chunk / [DONE] sent by
                                                // OpenAI-compatible servers when `stream_options.include_usage` is true.
                                                if final_usage.is_some() || finish_reason.is_none() {
                                                    let duration_ms = start_time.elapsed().as_millis();
                                                    crate::forensic::ForensicLogger::log_llm_response(
                                                        &provider_label_clone,
                                                        &api_model_clone,
                                                        total_content_chars,
                                                        total_reasoning_chars,
                                                        duration_ms,
                                                        first_token_time,
                                                        final_reason.as_deref(),
                                                        final_usage.as_ref(),
                                                    );
                                                    let _ = tx.send(StreamEvent::Completed { finish_reason: final_reason.clone() }).await;
                                                    should_break = true;
                                                }
                                            }
                                            other => {
                                                let _ = tx.send(other).await;
                                            }
                                        }
                                    }
                                    if should_break {
                                        break;
                                    }
                                }
                            }
                            Some(Err(reqwest_eventsource::Error::StreamEnded)) if final_reason.is_some() => {
                                let duration_ms = start_time.elapsed().as_millis();
                                crate::forensic::ForensicLogger::log_llm_response(
                                    &provider_label_clone,
                                    &api_model_clone,
                                    total_content_chars,
                                    total_reasoning_chars,
                                    duration_ms,
                                    first_token_time,
                                    final_reason.as_deref(),
                                    final_usage.as_ref(),
                                );
                                let _ = tx.send(StreamEvent::Completed { finish_reason: final_reason }).await;
                                break;
                            }
                            Some(Err(err)) => {
                                error!("[SSE] Error: {}", err);
                                crate::forensic::ForensicLogger::log_error("stream_chat_sse", &err.to_string());
                                let friendly_err = match err {
                                    reqwest_eventsource::Error::InvalidStatusCode(status, resp) => {
                                        let body_text = resp.text().await.unwrap_or_default();
                                        crate::forensic::ForensicLogger::log_error("stream_chat_sse_body", &body_text);
                                        if status == reqwest::StatusCode::UNAUTHORIZED {
                                            format!("Authentication error (401 Unauthorized): Invalid or missing API key for {}. Please check your API key in ~/.corex/settings.json or export the environment variable.", provider_label_clone)
                                        } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                                            format!("Rate limit or quota exceeded (429 Too Many Requests) from {}.", provider_label_clone)
                                        } else if !body_text.is_empty() {
                                            format!("API error ({}): {}", status, body_text)
                                        } else {
                                            format!("Stream error: Invalid status code {}", status)
                                        }
                                    }
                                    other => format!("Stream error: {}", other),
                                };
                                let _ = tx.send(StreamEvent::Error(friendly_err)).await;
                                break;
                            }
                            None => {
                                let duration_ms = start_time.elapsed().as_millis();
                                crate::forensic::ForensicLogger::log_llm_response(
                                    &provider_label_clone,
                                    &api_model_clone,
                                    total_content_chars,
                                    total_reasoning_chars,
                                    duration_ms,
                                    first_token_time,
                                    final_reason.as_deref(),
                                    final_usage.as_ref(),
                                );
                                let _ = tx.send(StreamEvent::Completed { finish_reason: final_reason }).await;
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Handles streaming requests to Anthropic's official `/v1/messages` endpoint.
    #[allow(clippy::too_many_arguments)]
    pub async fn stream_chat_anthropic(
        &self,
        endpoint_url: &str,
        api_key: &str,
        model: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        cancel_token: CancellationToken,
        auto_compact_event: Option<(Vec<Message>, String)>,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let base_url = endpoint_url.trim_end_matches('/');
        let url = if base_url.ends_with("/v1") {
            format!("{}/messages", base_url)
        } else {
            format!("{}/v1/messages", base_url)
        };

        let (system, anthropic_messages, anthropic_tools) =
            crate::anthropic::convert_messages_and_tools(&messages, tools.as_deref());

        let cfg = self.get_config();
        let profile = crate::anthropic::model_profile(model);
        let max_tokens = profile.max_output_tokens;

        let effort_setting = cfg.reasoning_effort.as_str();
        let thinking_off = matches!(effort_setting, "none" | "off" | "false");
        let thinking_config = match profile.thinking {
            crate::anthropic::ThinkingMode::Adaptive => Some(crate::anthropic::AnthropicThinkingConfig {
                thinking_type: "adaptive".to_string(),
                budget_tokens: None,
            }),
            crate::anthropic::ThinkingMode::Budget if !thinking_off && effort_setting != "low" => {
                Some(crate::anthropic::AnthropicThinkingConfig {
                    thinking_type: "enabled".to_string(),
                    budget_tokens: Some((max_tokens / 4).clamp(1024, 16_000)),
                })
            }
            _ => None,
        };
        let output_config = crate::anthropic::effort_for(&profile, effort_setting)
            .map(|effort| crate::anthropic::AnthropicOutputConfig { effort });

        // Thinking requires the default sampling, and 4.7+/5.x models reject `temperature`.
        let temperature = if thinking_config.is_some()
            || !profile.accepts_temperature
            || !(0.0..=1.0).contains(&cfg.temperature)
        {
            None
        } else {
            Some(cfg.temperature)
        };

        let request_body = crate::anthropic::AnthropicRequest {
            model: model.to_string(),
            max_tokens,
            system,
            messages: anthropic_messages,
            tools: anthropic_tools,
            stream: true,
            temperature,
            thinking: thinking_config,
            output_config,
            cache_control: Some(serde_json::json!({ "type": "ephemeral" })),
        };

        if crate::forensic::ForensicLogger::is_enabled() {
            let body_serialized = serde_json::to_string(&request_body).unwrap_or_default();
            crate::forensic::ForensicLogger::log_llm_request(
                "Anthropic Claude",
                model,
                &url,
                messages.len(),
                tools.as_ref().map(|t| t.len()).unwrap_or(0),
                &body_serialized,
            );
        }

        let req_builder = self
            .http
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&request_body);

        let mut event_source = req_builder.eventsource()?;
        let (tx, rx) = mpsc::channel::<StreamEvent>(100);
        let model_clone = model.to_string();

        tokio::spawn(async move {
            if let Some((compacted_messages, notice)) = auto_compact_event {
                let _ = tx.send(StreamEvent::ContextCompacted { compacted_messages, notice }).await;
            }
            let start_time = std::time::Instant::now();
            let mut first_token_time = None;
            let timeout_duration = std::time::Duration::from_secs(180);
            let mut total_content_chars = 0;
            let mut total_reasoning_chars = 0;
            let mut stream_state = crate::anthropic::AnthropicStreamState::new();
            let mut final_reason: Option<String> = None;

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        crate::forensic::ForensicLogger::log_error("stream_chat_anthropic", "Request cancelled by user");
                        let _ = tx.send(StreamEvent::Error("Request cancelled by user".to_string())).await;
                        break;
                    }
                    _ = tokio::time::sleep(timeout_duration) => {
                        crate::forensic::ForensicLogger::log_error("stream_chat_anthropic", "Stream idle timeout: Anthropic API did not respond for 180 seconds.");
                        let _ = tx.send(StreamEvent::Error("Stream idle timeout: Anthropic API did not respond for 180 seconds.".to_string())).await;
                        break;
                    }
                    event = event_source.next() => {
                        match event {
                            Some(Ok(Event::Open)) => {
                                debug!("[ANTHROPIC SSE] Stream opened");
                            }
                            Some(Ok(Event::Message(msg))) => {
                                if let Some(evs) = crate::anthropic::parse_anthropic_sse_data(&msg.data, &mut stream_state) {
                                    for ev in evs {
                                        match &ev {
                                            StreamEvent::ContentDelta(text) => {
                                                if first_token_time.is_none() {
                                                    first_token_time = Some(start_time.elapsed().as_millis());
                                                }
                                                total_content_chars += text.len();
                                            }
                                            StreamEvent::ReasoningDelta(cot) => {
                                                if first_token_time.is_none() {
                                                    first_token_time = Some(start_time.elapsed().as_millis());
                                                }
                                                total_reasoning_chars += cot.len();
                                            }
                                            StreamEvent::Completed { finish_reason } => {
                                                final_reason = finish_reason.clone();
                                                let duration_ms = start_time.elapsed().as_millis();
                                                crate::forensic::ForensicLogger::log_llm_response(
                                                    "Anthropic Claude",
                                                    &model_clone,
                                                    total_content_chars,
                                                    total_reasoning_chars,
                                                    duration_ms,
                                                    first_token_time,
                                                    final_reason.as_deref().or(Some("stop")),
                                                    Some(&stream_state.usage),
                                                );
                                                let _ = tx.send(ev).await;
                                                return;
                                            }
                                            _ => {}
                                        }
                                        if tx.send(ev).await.is_err() { return; }
                                    }
                                }
                            }
                            Some(Err(err)) => {
                                error!("[ANTHROPIC SSE] Error: {}", err);
                                crate::forensic::ForensicLogger::log_error("stream_chat_anthropic_sse", &err.to_string());
                                let friendly_err = match err {
                                    reqwest_eventsource::Error::InvalidStatusCode(status, resp) => {
                                        let body_text = resp.text().await.unwrap_or_default();
                                        crate::forensic::ForensicLogger::log_error("stream_chat_anthropic_sse_body", &body_text);
                                        if status == reqwest::StatusCode::UNAUTHORIZED {
                                            "Authentication error (401 Unauthorized): Invalid or missing Anthropic API key. Export ANTHROPIC_API_KEY or configure ~/.corex/settings.json".to_string()
                                        } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                                            "Rate limit or quota exceeded (429 Too Many Requests) from Anthropic API.".to_string()
                                        } else if !body_text.is_empty() {
                                            format!("Anthropic API error ({}): {}", status, body_text)
                                        } else {
                                            format!("Anthropic Stream error: Invalid status code {}", status)
                                        }
                                    }
                                    other => format!("Anthropic Stream error: {}", other),
                                };
                                let _ = tx.send(StreamEvent::Error(friendly_err)).await;
                                break;
                            }
                            None => {
                                let duration_ms = start_time.elapsed().as_millis();
                                crate::forensic::ForensicLogger::log_llm_response(
                                    "Anthropic Claude",
                                    &model_clone,
                                    total_content_chars,
                                    total_reasoning_chars,
                                    duration_ms,
                                    first_token_time,
                                    final_reason.as_deref(),
                                    Some(&stream_state.usage),
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
        if !cfg.is_deepseek_endpoint() {
            bail!("Balance lookup is only available for the DeepSeek provider.");
        }
        let api_key = &cfg.api_key;
        if api_key.is_empty() {
            bail!("API key is missing.");
        }

        let base_url = cfg.base_url.trim_end_matches('/').trim_end_matches("/v1");
        let url = format!("{}/user/balance", base_url);

        let resp = self
            .http
            .get(&url)
            .timeout(Duration::from_secs(10))
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

/// Parses a single raw OpenAI-compatible SSE data payload (e.g. from DeepSeek, OpenAI, Gemini, Groq, Mistral, Ollama)
/// into a list of `StreamEvent`s.
pub fn parse_openai_sse_data(data: &str) -> Option<Vec<StreamEvent>> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "[DONE]" {
        return Some(vec![StreamEvent::Completed { finish_reason: None }]);
    }

    let chunk = serde_json::from_str::<ChatCompletionChunk>(trimmed).ok()?;
    let mut events = Vec::new();

    if let Some(usage) = chunk.usage {
        events.push(StreamEvent::UsageUpdate(usage));
    }

    if let Some(choice) = chunk.choices.first() {
        if let Some(ref cot) = choice.delta.reasoning_content {
            if !cot.is_empty() {
                events.push(StreamEvent::ReasoningDelta(cot.clone()));
            }
        }

        if let Some(ref text) = choice.delta.content {
            if !text.is_empty() {
                events.push(StreamEvent::ContentDelta(text.clone()));
            }
        }

        if let Some(ref tc_deltas) = choice.delta.tool_calls {
            for (pos, tc) in tc_deltas.iter().enumerate() {
                let effective_index = if tc.index == 0 && pos > 0 { pos } else { tc.index };
                events.push(StreamEvent::ToolCallDelta {
                    index: effective_index,
                    id: tc.id.clone(),
                    name: tc.function.as_ref().and_then(|f| f.name.clone()),
                    arguments: tc.function.as_ref().and_then(|f| f.arguments.clone()),
                    extra_content: tc.extra_content.clone(),
                });
            }
        }

        if let Some(ref ec) = choice.delta.extra_content {
            events.push(StreamEvent::ExtraContentDelta(ec.clone()));
        }

        if let Some(ref reason) = choice.finish_reason {
            events.push(StreamEvent::Completed { finish_reason: Some(reason.clone()) });
        }
    }

    Some(events)
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
                vec![ToolCall::new("call_123", "run_shell_command", "{\"command\":\"ls\"}")],
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
                vec![ToolCall::new("call_sudo", "run_shell_command", "{\"command\":\"sudo whoami\"}")],
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
        // The LATEST user prompt MUST be preserved immediately after context summary!
        assert_eq!(messages[3].role, "user");
        assert_eq!(messages[3].text_content().unwrap(), "Hello 6");
    }

    #[tokio::test]
    async fn test_compact_preserves_latest_user_prompt_during_heavy_tool_task() {
        let client = LlmClient::new(Config::default());
        let mut messages = vec![
            Message::system("System prompt instructions"),
            Message::user("First ancient request from hours ago"),
            Message::assistant("Ancient response", None),
            Message::user("ACTIVE DIRECTIVE: Fix the database connection pool leak!"),
        ];

        // Simulate 8 tool turns executed under this active directive (16 tool messages)
        for i in 1..=8 {
            let call_id = format!("call_{}", i);
            messages.push(Message::assistant_with_tools(
                None,
                None,
                vec![ToolCall::new(&call_id, "run_shell_command", "{\"command\":\"test\"}")],
            ));
            messages.push(Message::tool_response(&call_id, format!("Output from tool step {}", i)));
        }

        let initial_count = messages.len();
        assert_eq!(initial_count, 20); // 1 sys + 1 old user + 1 old asst + 1 active user + 16 tool msgs

        let result = client.compact_messages(&mut messages, true).await;
        assert!(result.is_ok());
        let notice = result.unwrap();
        assert!(notice.is_some(), "Expected compaction to shrink heavy tool turns after active user prompt");
        assert!(messages.len() < initial_count, "Expected message count to shrink after compaction");

        // Verify that the active user prompt was NEVER deleted or forgotten!
        let has_active_directive = messages.iter().any(|m| {
            m.role == "user" && m.text_content().unwrap_or("").contains("Fix the database connection pool leak!")
        });
        assert!(has_active_directive, "CRITICAL: The active user prompt was deleted during compaction!");

        // The latest user prompt should still be in messages
        let last_user = messages.iter().filter(|m| m.role == "user").last().unwrap();
        assert_eq!(last_user.text_content().unwrap(), "ACTIVE DIRECTIVE: Fix the database connection pool leak!");
    }

    #[test]
    fn test_provider_kind_classification() {
        let mut cfg = Config::default();
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::DeepSeek);

        cfg.active_provider = Some("openai".to_string());
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::OpenAi);

        cfg.active_provider = Some("anthropic".to_string());
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::Anthropic);

        cfg.active_provider = Some("google".to_string());
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::Google);

        cfg.active_provider = Some("github".to_string());
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::GitHub);

        cfg.local_llm_enabled = true;
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::Local);
    }

    #[test]
    fn test_google_and_openai_endpoint_resolution() {
        let mut cfg = Config::default();
        cfg.active_provider = Some("google".to_string());
        let (url, _) = cfg.endpoint();
        assert!(url.contains("generativelanguage.googleapis.com"));
        assert!(!cfg.is_deepseek_endpoint());

        cfg.active_provider = Some("openai".to_string());
        let (url_oai, _) = cfg.endpoint();
        assert!(url_oai.contains("api.openai.com"));
        assert!(!cfg.is_deepseek_endpoint());

        cfg.active_provider = Some("anthropic".to_string());
        let (url_ant, _) = cfg.endpoint();
        assert!(url_ant.contains("api.anthropic.com"));
        assert!(!cfg.is_deepseek_endpoint());

        cfg.active_provider = Some("groq".to_string());
        let (url_groq, _) = cfg.endpoint();
        assert!(url_groq.contains("api.groq.com"));
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::Groq);

        cfg.active_provider = Some("openrouter".to_string());
        let (url_or, _) = cfg.endpoint();
        assert!(url_or.contains("openrouter.ai"));
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::OpenRouter);

        cfg.active_provider = Some("mistral".to_string());
        let (url_mis, _) = cfg.endpoint();
        assert!(url_mis.contains("api.mistral.ai"));
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::Mistral);
    }

    #[test]
    fn test_model_inference_without_explicit_provider() {
        let mut cfg = Config::default();
        cfg.active_provider = None;
        cfg.model = "gemini-3.5-flash-lite".to_string();
        let (url, _) = cfg.endpoint();
        assert!(url.contains("generativelanguage.googleapis.com"));
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::Google);

        cfg.model = "claude-3-7-sonnet-20250219".to_string();
        let (url_ant, _) = cfg.endpoint();
        assert!(url_ant.contains("api.anthropic.com"));
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::Anthropic);

        cfg.model = "gpt-4o".to_string();
        let (url_oai, _) = cfg.endpoint();
        assert!(url_oai.contains("api.openai.com"));
        assert_eq!(cfg.provider_kind(), crate::providers::ProviderKind::OpenAi);
    }
}

#[cfg(test)]
mod overflow_tests {
    use super::is_context_overflow_error;

    #[test]
    fn recognises_context_overflow_but_not_rate_limits() {
        assert!(is_context_overflow_error("API error (400): This model's maximum context length is 131072 tokens"));
        assert!(is_context_overflow_error("prompt is too long: 250000 tokens > 200000 maximum"));
        assert!(is_context_overflow_error("code: context_length_exceeded"));
        assert!(!is_context_overflow_error("Rate limit or quota exceeded (429 Too Many Requests) from DeepSeek."));
        assert!(!is_context_overflow_error("Too many tokens per minute"));
    }
}

#[cfg(test)]
mod image_note_tests {
    use super::LlmClient;
    use crate::types::{ContentPart, Message, MessageContent};

    #[test]
    fn text_only_models_get_a_note_instead_of_the_image() {
        let mut messages = vec![
            Message::user("plain"),
            Message::user_parts(vec![
                ContentPart::text("what is this? [Image #1]"),
                ContentPart::image_url("data:image/png;base64,AAAA"),
            ]),
        ];
        LlmClient::replace_images_with_notes(&mut messages);
        assert_eq!(messages[0].text_content(), Some("plain"));
        let converted = messages[1].text_content().expect("now plain text");
        assert!(converted.starts_with("what is this? [Image #1]"));
        assert!(converted.contains("cannot see images"));
        assert!(converted.contains("do not search the disk"));
        assert!(!converted.contains("base64"));
    }

    #[test]
    fn rejections_of_image_input_are_recognised() {
        use super::is_image_rejection_error;
        assert!(is_image_rejection_error("API error (400): unknown variant `image_url`, expected `text`"));
        assert!(is_image_rejection_error("This model does not support image input"));
        assert!(!is_image_rejection_error("Rate limit or quota exceeded (429 Too Many Requests)"));
        assert!(!is_image_rejection_error("maximum context length is 131072 tokens"));
    }

    #[test]
    fn a_prompt_with_images_keeps_its_words_for_titles_and_digests() {
        let msg = Message::user_parts(vec![
            ContentPart::text("look at [Image #1]"),
            ContentPart::image_url("data:image/png;base64,AAAA"),
        ]);
        assert_eq!(msg.text_content(), None);
        assert_eq!(msg.display_text().as_deref(), Some("look at [Image #1]"));
        assert!(matches!(msg.content, Some(MessageContent::Parts(_))));
    }
}

#[cfg(test)]
mod image_fallback_tests {
    use super::*;
    use crate::types::ContentPart;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serves two requests: a 400 that rejects image input, then a normal streamed answer. Returns
    /// the address and the bodies of the requests it received.
    async fn mock_server() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = bodies.clone();
        tokio::spawn(async move {
            for attempt in 0..2 {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 256 * 1024];
                let mut total = 0;
                loop {
                    let n = sock.read(&mut buf[total..]).await.unwrap_or(0);
                    total += n;
                    let text = String::from_utf8_lossy(&buf[..total]).to_string();
                    let done = text.split_once("\r\n\r\n").is_some_and(|(head, body)| {
                        let len = head
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        body.len() >= len
                    });
                    if n == 0 || done {
                        seen.lock().unwrap().push(text);
                        break;
                    }
                }
                let response = if attempt == 0 {
                    let body = r#"{"error":{"message":"Failed to deserialize the JSON body: unknown variant `image_url`, expected `text`"}}"#;
                    format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body)
                } else {
                    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body)
                };
                let _ = sock.write_all(response.as_bytes()).await;
            }
        });
        (addr, bodies)
    }

    #[tokio::test]
    async fn a_model_that_rejects_images_gets_the_conversation_with_a_note_instead() {
        let (addr, bodies) = mock_server().await;
        let cfg = Config {
            base_url: addr,
            api_key: "test-key".to_string(),
            model: "some-text-model".to_string(),
            ..Config::default()
        };
        let client = LlmClient::new(cfg);
        let messages = vec![
            Message::system("sys"),
            Message::user_parts(vec![
                ContentPart::text("what is this? [Image #1]"),
                ContentPart::image_url("data:image/png;base64,AAAA"),
            ]),
        ];

        let mut rx = client.stream_chat(messages, None, CancellationToken::new()).await.unwrap();
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }

        assert!(events.iter().any(|e| matches!(e, StreamEvent::Notice(n) if n.contains("rejected the image"))), "{events:?}");
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ContentDelta(t) if t == "ok")), "{events:?}");
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::Error(_))), "{events:?}");

        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].contains("image_url") && bodies[0].contains("AAAA"));
        assert!(!bodies[1].contains("AAAA") && bodies[1].contains("do not search the disk"));
    }
}
