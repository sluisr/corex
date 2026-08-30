use std::time::Duration;
use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use futures_util::StreamExt;
use reqwest_eventsource::{Event, RequestBuilderExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};
use crate::types::{ChatCompletionChunk, Message, ToolDefinition};
use crate::StreamEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalChatChoice {
    pub message: LocalChatMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalChatResponse {
    pub choices: Vec<LocalChatChoice>,
}

#[derive(Clone)]
pub struct LocalLlmClient {
    http: Client,
    base_url: String,
    model_name: String,
}

impl LocalLlmClient {
    pub fn new(base_url: impl Into<String>, model_name: impl Into<String>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_millis(600))
            .build()
            .unwrap_or_default();

        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model_name: model_name.into(),
        }
    }

    /// Fast health check to verify if llama-server / local OpenAI-compatible endpoint is reachable.
    /// Uses a tight 600ms connect timeout so it never blocks normal execution.
    pub async fn health_check(&self) -> bool {
        let url = format!("{}/models", self.base_url);
        match self.http.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Streaming chat completion against the local LLM with optional tools support.
    pub async fn stream_chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let url = format!("{}/chat/completions", self.base_url);
        // Forward the exact messages list containing the global system prompt and conversation history:
        let request_body = LocalChatRequest {
            model: self.model_name.clone(),
            messages,
            tools,
            temperature: Some(0.7),
            max_tokens: Some(2048),
            stream: true,
        };

        let msg_count = request_body.messages.len();
        let body_serialized = serde_json::to_string_pretty(&request_body).unwrap_or_default();
        crate::forensic::ForensicLogger::log_llm_request(
            "Local LLM (llama-server)",
            &self.model_name,
            &url,
            msg_count,
            0,
            &body_serialized
        );

        debug!("[LOCAL_LLM] POST {} (streaming, model: {})", url, self.model_name);

        let req_builder = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request_body);

        let mut event_source = req_builder.eventsource()?;
        let (tx, rx) = mpsc::channel::<StreamEvent>(100);

        let model_name_clone = self.model_name.clone();
        tokio::spawn(async move {
            let start_time = std::time::Instant::now();
            let mut first_token_time = None;
            let timeout_duration = Duration::from_secs(45);
            let mut total_chars = 0;
            let mut final_usage: Option<crate::types::Usage> = None;
            let final_reason: Option<String> = None;

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        crate::forensic::ForensicLogger::log_error("local_stream_chat", "Request cancelled by user (CancellationToken fired)");
                        let _ = tx.send(StreamEvent::Error("Request cancelled by user".to_string())).await;
                        break;
                    }
                    _ = tokio::time::sleep(timeout_duration) => {
                        crate::forensic::ForensicLogger::log_error("local_stream_chat", "Stream idle timeout: Local LLM did not respond.");
                        let _ = tx.send(StreamEvent::Error("Stream idle timeout: Local LLM did not respond.".to_string())).await;
                        break;
                    }
                    event = event_source.next() => {
                        match event {
                            Some(Ok(Event::Open)) => {
                                debug!("[LOCAL_SSE] Stream opened");
                            }
                            Some(Ok(Event::Message(msg))) => {
                                if msg.data.trim() == "[DONE]" {
                                    let duration_ms = start_time.elapsed().as_millis();
                                    crate::forensic::ForensicLogger::log_llm_response(
                                        "Local LLM (llama-server)",
                                        &model_name_clone,
                                        total_chars,
                                        0,
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
                                        if let Some(ref text) = choice.delta.content {
                                            if !text.is_empty() {
                                                if first_token_time.is_none() {
                                                    first_token_time = Some(start_time.elapsed().as_millis());
                                                }
                                                total_chars += text.len();
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
                                                "Local LLM (llama-server)",
                                                &model_name_clone,
                                                total_chars,
                                                0,
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
                                error!("[LOCAL_SSE] Error: {}", err);
                                crate::forensic::ForensicLogger::log_error("local_stream_chat_sse", &err.to_string());
                                let _ = tx.send(StreamEvent::Error(format!("Local stream error: {}", err))).await;
                                break;
                            }
                            None => {
                                let duration_ms = start_time.elapsed().as_millis();
                                crate::forensic::ForensicLogger::log_llm_response(
                                    "Local LLM (llama-server)",
                                    &model_name_clone,
                                    total_chars,
                                    0,
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

    /// Single-turn completion against the local LLM.
    pub async fn complete(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let request_body = LocalChatRequest {
            model: self.model_name.clone(),
            messages: vec![
                Message::system(system_prompt),
                Message::user(user_prompt),
            ],
            tools: None,
            temperature: Some(0.2), // Low temperature for high fidelity and determinism
            max_tokens: Some(1024),
            stream: false,
        };

        debug!("[LOCAL_LLM] POST {} (model: {})", url, self.model_name);

        let resp = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to local LLM server")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            bail!("Local LLM error (HTTP {}): {}", status, err_text);
        }

        let body = resp.json::<LocalChatResponse>().await?;
        let text = body
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(text)
    }

    /// Compresses and extracts critical errors / context from massive tool outputs
    /// (e.g. cargo builds, test dumps, git logs) to reduce prompt tokens sent to DeepSeek Cloud.
    pub async fn summarize_tool_output(&self, tool_name: &str, raw_output: &str) -> Result<String> {
        let system_prompt = r#"You are an ultra-fast, high-precision technical summarizer for a CLI coding agent.
Your task is to extract the essential facts, compiler errors, failing tests, or key command outcomes from the output.
RULES:
1. Retain exact file paths, line numbers, error codes, and failure messages.
2. Remove repetitive noise, progress bars, and successful unchanged steps.
3. Be concise and direct. Do not add conversational intro/outro. Output ONLY the technical summary."#;

        let user_prompt = format!(
            "Tool executed: {}\n\nRaw Output:\n{}\n\nPlease provide a concise summary retaining all errors and key findings:",
            tool_name, raw_output
        );

        self.complete(system_prompt, &user_prompt).await
    }

    /// Fast answering for simple shell / terminal questions without contacting cloud API.
    pub async fn quick_chat(&self, prompt: &str) -> Result<String> {
        let system_prompt = r#"You are UTI Local Assistant, a fast, knowledgeable Linux terminal & developer helper.
Give direct, accurate answers for shell commands, system utilities, git workflows, and quick explanations.
Keep answers concise and practical with code snippets where helpful."#;

        self.complete(system_prompt, prompt).await
    }

    /// Evaluates user prompt intent using fast heuristics (0ms) and local LLM zero-shot classification:
    /// - `LocalChat` -> Greetings, theory, general Q&A ($0.00)
    /// - `LocalInspection` -> Hardware stats, reading files, directory navigation ($0.00)
    /// - `HeavyCoding` -> Code modifications, refactorings, patch generation (DeepSeek Cloud)
    pub async fn classify_intent(&self, user_prompt: &str, context: Option<&str>) -> IntentDecision {
        if let Some(dec) = fast_heuristic_intent(user_prompt) {
            debug!("[INTENT_ROUTER] Fast heuristic (0ms) classified {:?} as {:?}", user_prompt, dec);
            return dec;
        }

        let system_prompt = r#"You are a strict routing classifier for a Linux CLI developer assistant.
Classify the user's intent into EXACTLY one of three categories:

[CHAT] (Reply 'CHAT'):
- Greetings, farewells, conversational remarks ('hola', 'cómo estás', 'gracias').
- Personal assistant questions, meta-questions ('quién eres?', 'qué sabes hacer?').
- Pure theoretical/conceptual questions without requiring file modifications ('qué es async?', 'explica REST').

[INSPECT] (Reply 'INSPECT'):
- Inquiries about hardware, PC metrics, CPU, RAM, disk, network ('revisa mi pc', 'cuánta RAM tengo', 'lscpu').
- Read-only workspace inspection, directory listing, searching ('qué archivos hay aquí?', 'busca dónde está x').

[CODE] (Reply 'CODE'):
- Code generation, bug fixing, refactoring, modifying files, executing builds, test suites, or implementing features.

Rule: Reply with ONLY the single word 'CHAT', 'INSPECT', or 'CODE'."#;

        let prompt_payload = if let Some(ctx) = context {
            format!("Recent conversation context:\n{}\n\nCurrent user request:\n{}", ctx, user_prompt)
        } else {
            user_prompt.to_string()
        };

        let url = format!("{}/chat/completions", self.base_url);
        let request_body = LocalChatRequest {
            model: self.model_name.clone(),
            messages: vec![
                Message::system(system_prompt),
                Message::user(prompt_payload),
            ],
            tools: None,
            temperature: Some(0.0),
            max_tokens: Some(4),
            stream: false,
        };

        if let Ok(resp) = self.http.post(&url).json(&request_body).send().await {
            if let Ok(body) = resp.json::<LocalChatResponse>().await {
                if let Some(choice) = body.choices.first() {
                    let out = choice.message.content.trim().to_uppercase();
                    debug!("[INTENT_ROUTER] LLM classified {:?} as {}", user_prompt, out);
                    if out.contains("CODE") {
                        return IntentDecision::HeavyCoding;
                    } else if out.contains("INSPECT") {
                        return IntentDecision::LocalInspection;
                    } else {
                        return IntentDecision::LocalChat;
                    }
                }
            }
        }

        IntentDecision::HeavyCoding
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentDecision {
    LocalChat,
    LocalInspection,
    HeavyCoding,
}

impl IntentDecision {
    pub fn is_heavy_coding(&self) -> bool {
        matches!(self, IntentDecision::HeavyCoding)
    }

    pub fn is_local_chat(&self) -> bool {
        matches!(self, IntentDecision::LocalChat)
    }

    pub fn is_local_inspection(&self) -> bool {
        matches!(self, IntentDecision::LocalInspection)
    }

    pub fn can_handle_locally(&self) -> bool {
        matches!(self, IntentDecision::LocalChat | IntentDecision::LocalInspection)
    }
}

/// Instant in-memory 0ms heuristic classifier for common patterns
pub fn fast_heuristic_intent(user_prompt: &str) -> Option<IntentDecision> {
    let lower = user_prompt.trim().to_lowercase();
    if lower.is_empty() {
        return Some(IntentDecision::LocalChat);
    }

    // Clean leading/trailing punctuation (including Spanish inverted ¿, ¡)
    let clean = lower.trim_matches(|c: char| c.is_ascii_punctuation() || c == '¿' || c == '¡' || c == '…').trim();

    // 1. Definite heavy coding indicators -> DeepSeek Cloud
    let code_keywords = [
        "refactor", "refactoriza", "corrige", "arregla", "fix ", "fix:", "bug",
        "patch", "aplica el parche", "apply_patch", "implementa", "implement ",
        "escribe una funcion", "escribe una función", "write a function", "write code",
        "crea un archivo", "create file", "modifica el archivo", "edit file",
        "crea una app", "haz una app", "build an app", "desarrolla",
        "fn ", "pub fn ", "def ", "class ", "impl ", "struct ", "enum ",
        "<!doctype html", "<html", "import react", "use std::",
    ];
    if code_keywords.iter().any(|k| lower.contains(k) || clean.contains(k)) {
        return Some(IntentDecision::HeavyCoding);
    }

    // 2. Hardware / OS / System diagnostics -> Local Inspection
    let inspect_exact = [
        "revisa mi pc", "revisa el pc", "que pc tengo", "qué pc tengo", "mi pc",
        "specs", "hardware", "cpu", "gpu", "ram", "memoria", "cuanta ram", "cuánta ram",
        "disco", "espacio en disco", "temperatura", "temperaturas", "neofetch", "fastfetch",
        "lscpu", "free -h", "df -h", "uname -a", "hostnamectl", "uptime", "top", "htop",
        "lista los archivos", "muestra los archivos", "que archivos hay", "qué archivos hay",
        "ls", "tree", "pwd", "ip a", "ifconfig", "ping",
    ];
    if inspect_exact.iter().any(|&k| clean == k || clean.starts_with(k) || clean.ends_with(k)) {
        return Some(IntentDecision::LocalInspection);
    }

    // 3. Conversational greetings / meta questions / basic explanations -> Local Chat
    let chat_exact = [
        "hola", "buenos dias", "buenos días", "buenas tardes", "buenas noches",
        "hello", "hi", "hey", "que tal", "qué tal", "como estas", "cómo estás",
        "quien eres", "quién eres", "who are you", "que puedes hacer", "qué puedes hacer",
        "como te llamas", "cómo te llamas", "ayuda", "help", "gracias", "muchas gracias",
        "thanks", "thx", "ok", "vale", "listo", "adios", "adiós", "bye", "chau", "hasta luego",
    ];
    if chat_exact.iter().any(|&k| clean == k || clean.starts_with(k)) {
        return Some(IntentDecision::LocalChat);
    }

    // Explanations of theoretical concepts without code modification
    if (clean.starts_with("que es ") || clean.starts_with("qué es ") || clean.starts_with("what is ")
        || clean.starts_with("explica ") || clean.starts_with("explicame ") || clean.starts_with("explícame ")
        || clean.starts_with("explain ") || clean.starts_with("diferencia entre ") || clean.starts_with("como funciona ")
        || clean.starts_with("cómo funciona "))
        && !clean.contains("crea") && !clean.contains("haz") && !clean.contains("escribe") && !clean.contains("modifica")
    {
        return Some(IntentDecision::LocalChat);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_heuristic_chat() {
        assert_eq!(fast_heuristic_intent("hola"), Some(IntentDecision::LocalChat));
        assert_eq!(fast_heuristic_intent("Buenos días!"), Some(IntentDecision::LocalChat));
        assert_eq!(fast_heuristic_intent("¿Quién eres?"), Some(IntentDecision::LocalChat));
        assert_eq!(fast_heuristic_intent("qué es un trait en Rust?"), Some(IntentDecision::LocalChat));
        assert_eq!(fast_heuristic_intent("explica cómo funciona async"), Some(IntentDecision::LocalChat));
    }

    #[test]
    fn test_fast_heuristic_inspection() {
        assert_eq!(fast_heuristic_intent("revisa mi pc"), Some(IntentDecision::LocalInspection));
        assert_eq!(fast_heuristic_intent("cuánta ram tengo"), Some(IntentDecision::LocalInspection));
        assert_eq!(fast_heuristic_intent("lscpu"), Some(IntentDecision::LocalInspection));
        assert_eq!(fast_heuristic_intent("lista los archivos"), Some(IntentDecision::LocalInspection));
    }

    #[test]
    fn test_fast_heuristic_coding() {
        assert_eq!(fast_heuristic_intent("refactoriza esta función"), Some(IntentDecision::HeavyCoding));
        assert_eq!(fast_heuristic_intent("corrige el bug en el parser"), Some(IntentDecision::HeavyCoding));
        assert_eq!(fast_heuristic_intent("aplica el parche al archivo main.rs"), Some(IntentDecision::HeavyCoding));
        assert_eq!(fast_heuristic_intent("pub fn execute() -> Result<()>"), Some(IntentDecision::HeavyCoding));
    }
}
