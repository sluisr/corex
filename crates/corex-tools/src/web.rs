use std::sync::OnceLock;
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use corex_core::safe_truncate_str;

use crate::types::{Tool, ToolContext, ToolOutput};

fn get_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .unwrap_or_default()
    })
}

fn tag_strip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<[^>]+>"#).unwrap())
}

fn script_strip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)<script[^>]*>.*?</script>"#).unwrap())
}

fn style_strip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)<style[^>]*>.*?</style>"#).unwrap())
}

fn urlencoding(input: &str) -> String {
    let mut encoded = String::new();
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{:02X}", byte)),
        }
    }
    encoded
}

fn resolve_api_key(context: &ToolContext) -> Option<String> {
    // Only resolve keys designated for DeepSeek / Corex API.
    // OPENAI_API_KEY is deliberately excluded to prevent credential leakage to DeepSeek's endpoint.
    std::env::var("COREX_API_KEY")
        .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
        .ok()
        .filter(|k| !k.trim().is_empty())
        .or_else(|| {
            let cfg = corex_core::config::Config::load_with_workspace(Some(&context.workspace_dir));
            if !cfg.api_key.trim().is_empty() {
                Some(cfg.api_key)
            } else {
                None
            }
        })
}

async fn search_with_deepseek(client: &reqwest::Client, api_key: &str, query: &str) -> Result<String> {
    let url = "https://api.deepseek.com/anthropic/v1/messages";
    let payload = json!({
        "model": "claude-3-5-sonnet-20241022",
        "max_tokens": 1500,
        "messages": [
            {
                "role": "user",
                "content": format!("Search the web for: {}. Return factual data points, direct answers, and relevant excerpts only. Do not include conversational filler or meta commentary. List URLs and sources.", query)
            }
        ],
        "tools": [
            {
                "type": "web_search_20250305",
                "name": "web_search"
            }
        ]
    });

    let res = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;

    if !res.status().is_success() {
        let status = res.status();
        let err_text = res.text().await.unwrap_or_default();
        anyhow::bail!("DeepSeek Web Search HTTP {}: {}", status, err_text);
    }

    let val: serde_json::Value = res.json().await?;
    let content = val.get("content").and_then(|c| c.as_array());

    let mut text_parts = Vec::new();
    let mut sources = Vec::new();

    if let Some(blocks) = content {
        for block in blocks {
            let b_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if b_type == "text" {
                if let Some(t) = block.get("text").and_then(|s| s.as_str()) {
                    let trimmed = t.trim();
                    if !trimmed.is_empty() {
                        let lines: Vec<&str> = trimmed
                            .lines()
                            .filter(|line| {
                                let l = line.trim().to_lowercase();
                                !l.starts_with("i'll search")
                                    && !l.starts_with("let me search")
                                    && !l.starts_with("let me dig deeper")
                                    && !l.starts_with("let me try")
                                    && !l.starts_with("i will search")
                                    && !l.starts_with("searching for ")
                            })
                            .collect();
                        let clean = lines.join("\n").trim().to_string();
                        if !clean.is_empty() {
                            text_parts.push(clean);
                        }
                    }
                }
            } else if b_type == "web_search_tool_result" {
                if let Some(items) = block.get("content").and_then(|c| c.as_array()) {
                    for item in items {
                        let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("Result");
                        let link = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
                        if !link.is_empty() {
                            sources.push(format!("- [{}]({})", title, link));
                        }
                    }
                }
            }
        }
    }

    let mut output = String::new();
    if !text_parts.is_empty() {
        output.push_str(&text_parts.join("\n\n"));
        output.push_str("\n\n");
    }
    if !sources.is_empty() {
        sources.sort();
        sources.dedup();
        output.push_str("Sources:\n");
        for s in sources.into_iter().take(8) {
            output.push_str(&s);
            output.push('\n');
        }
    }

    Ok(output)
}

async fn search_fallback(client: &reqwest::Client, query: &str) -> Result<String> {
    // 1. DuckDuckGo Instant Answer API
    let ddg_url = format!("https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1", urlencoding(query));
    if let Ok(resp) = client.get(&ddg_url).timeout(std::time::Duration::from_secs(8)).send().await {
        if let Ok(data) = resp.json::<serde_json::Value>().await {
            let mut parts = Vec::new();
            if let Some(abstract_text) = data.get("AbstractText").and_then(|v| v.as_str()) {
                if !abstract_text.is_empty() {
                    let source = data.get("AbstractURL").and_then(|v| v.as_str()).unwrap_or("");
                    parts.push(format!("{}\nSource: {}", abstract_text, source));
                }
            }
            if let Some(related) = data.get("RelatedTopics").and_then(|v| v.as_array()) {
                for item in related.iter().take(4) {
                    if let Some(txt) = item.get("Text").and_then(|v| v.as_str()) {
                        let url = item.get("FirstURL").and_then(|v| v.as_str()).unwrap_or("");
                        parts.push(format!("- {} ({})", txt, url));
                    }
                }
            }
            if !parts.is_empty() {
                return Ok(parts.join("\n\n"));
            }
        }
    }

    // 2. Wikipedia search API as backup
    let wiki_url = format!("https://en.wikipedia.org/w/api.php?action=opensearch&search={}&limit=4&namespace=0&format=json", urlencoding(query));
    if let Ok(resp) = client.get(&wiki_url).timeout(std::time::Duration::from_secs(8)).send().await {
        if let Ok(data) = resp.json::<serde_json::Value>().await {
            if let Some(arr) = data.as_array() {
                if arr.len() >= 4 {
                    let titles = arr[1].as_array();
                    let snippets = arr[2].as_array();
                    let urls = arr[3].as_array();
                    let mut results = Vec::new();
                    if let (Some(t), Some(s), Some(u)) = (titles, snippets, urls) {
                        for i in 0..t.len() {
                            let title = t.get(i).and_then(|v| v.as_str()).unwrap_or("");
                            let snippet = s.get(i).and_then(|v| v.as_str()).unwrap_or("");
                            let url = u.get(i).and_then(|v| v.as_str()).unwrap_or("");
                            if !title.is_empty() {
                                results.push(format!("- **{}**: {}\n  {}", title, snippet, url));
                            }
                        }
                    }
                    if !results.is_empty() {
                        return Ok(results.join("\n\n"));
                    }
                }
            }
        }
    }

    Ok(format!("Search completed for '{}'. Found 0 direct instant matches. Try checking specific URLs with web_fetch.", query))
}

// --- WebSearchTool (web_search) ---
pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Performs real-time web search using DeepSeek's native search engine to retrieve up-to-date facts, breaking news, market data, and documentation. [PREFERRED for finding real-time information, current news, and documentation]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to find information on the web."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return Ok(ToolOutput::error("Missing 'query' argument.")),
        };

        let client = get_http_client();

        if let Some(api_key) = resolve_api_key(context) {
            match search_with_deepseek(client, &api_key, query).await {
                Ok(res) if !res.trim().is_empty() => return Ok(ToolOutput::success(res)),
                Err(e) => {
                    tracing::warn!("DeepSeek native web search returned error, trying fallback: {}", e);
                }
                _ => {}
            }
        }

        let fallback_res = search_fallback(client, query).await?;
        Ok(ToolOutput::success(fallback_res))
    }
}

fn is_blocked_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ipv4) => {
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_unspecified()
                || ipv4.is_broadcast()
                || (ipv4.octets()[0] == 169 && ipv4.octets()[1] == 254) // Cloud metadata (AWS, GCP, Azure)
                || (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0xC0) == 64) // Carrier-grade NAT
        }
        std::net::IpAddr::V6(ipv6) => {
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                || (ipv6.segments()[0] & 0xfe00) == 0xfc00 // Unique local address (fc00::/7)
                || (ipv6.segments()[0] & 0xffc0) == 0xfe80 // Link-local (fe80::/10)
        }
    }
}

async fn validate_url_for_ssrf(url_str: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url_str).map_err(|e| format!("Invalid URL: {}", e))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("Unsupported protocol scheme '{}': only http and https are permitted", parsed.scheme()));
    }
    let host = parsed.host_str().ok_or_else(|| "URL has no host".to_string())?;

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(format!("Access to internal/private IP address '{}' is forbidden", ip));
        }
    } else {
        let port = parsed.port_or_known_default().unwrap_or(80);
        match tokio::net::lookup_host((host, port)).await {
            Ok(addrs) => {
                for addr in addrs {
                    if is_blocked_ip(addr.ip()) {
                        return Err(format!("Host '{}' resolves to private/internal IP '{}'; access is forbidden", host, addr.ip()));
                    }
                }
            }
            Err(e) => {
                return Err(format!("Failed to resolve host '{}': {}", host, e));
            }
        }
    }

    Ok(parsed)
}

// --- WebFetchTool ---
pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "Processes content from URL(s) embedded in a prompt."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "A comprehensive prompt that includes the URL(s) to fetch and specific instructions on how to process their content."
                },
                "url": {
                    "type": "string",
                    "description": "Optional direct URL to fetch (if not extracted from prompt)."
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        let direct_url = args.get("url").and_then(|v| v.as_str());

        let target_url = if let Some(u) = direct_url {
            Some(u.to_string())
        } else {
            prompt
                .split_whitespace()
                .find(|word| word.starts_with("http://") || word.starts_with("https://"))
                .map(|s| s.to_string())
        };

        let raw_url = match target_url {
            Some(u) => u,
            None => return Ok(ToolOutput::error("No valid http:// or https:// URL found in prompt.")),
        };

        let validated_url = match validate_url_for_ssrf(&raw_url).await {
            Ok(u) => u,
            Err(err) => return Ok(ToolOutput::error(format!("Security restriction: {}", err))),
        };
        let url = validated_url.to_string();

        let client = get_http_client();

        let mut resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to fetch {}: {}", url, e))),
        };

        if let Some(len) = resp.content_length() {
            if len > 10 * 1024 * 1024 {
                return Ok(ToolOutput::error(format!(
                    "Resource at {} is too large ({} bytes). Maximum allowed fetch limit is 10 MB.",
                    url, len
                )));
            }
        }

        let mut bytes = Vec::new();
        const MAX_DOWNLOAD_BYTES: usize = 2 * 1024 * 1024;
        while let Ok(Some(chunk)) = resp.chunk().await {
            let to_take = chunk.len().min(MAX_DOWNLOAD_BYTES.saturating_sub(bytes.len()));
            bytes.extend_from_slice(&chunk[..to_take]);
            if bytes.len() >= MAX_DOWNLOAD_BYTES {
                break;
            }
        }

        let body = String::from_utf8_lossy(&bytes);

        // Basic HTML stripping and newline normalization
        let clean_html = script_strip_re().replace_all(&body, "");
        let clean_html = style_strip_re().replace_all(&clean_html, "");
        let text = tag_strip_re().replace_all(&clean_html, " ");
        let normalized = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        let truncated = if normalized.len() > 15000 {
            let cut = safe_truncate_str(&normalized, 15000);
            format!("{}\n\n[Content truncated after 15,000 characters]", cut)
        } else {
            normalized
        };

        Ok(ToolOutput::success(format!("Content from {}:\n\n{}", url, truncated)))
    }
}
