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
            .redirect(reqwest::redirect::Policy::none())
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

pub fn format_search_results(query: &str, results: &[serde_json::Value]) -> String {
    if results.is_empty() {
        return format!("Search completed for '{}'. Found 0 direct matches.", query);
    }

    let mut output = String::new();
    let mut sources = Vec::new();

    for (i, item) in results.iter().enumerate() {
        let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("Untitled").trim();
        let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("").trim();
        let snippet = item.get("snippet").and_then(|s| s.as_str()).unwrap_or("").trim();
        let summary = item.get("summary").and_then(|s| s.as_str()).unwrap_or("").trim();
        let site_name = item.get("site_name").and_then(|s| s.as_str()).unwrap_or("").trim();
        let date = item.get("date").and_then(|d| d.as_str()).unwrap_or("").trim();

        let description = if !summary.is_empty() {
            summary
        } else {
            snippet
        };

        let mut meta = Vec::new();
        if !site_name.is_empty() {
            meta.push(site_name);
        }
        if !date.is_empty() {
            let clean_date = date.split('T').next().unwrap_or(date);
            meta.push(clean_date);
        }

        output.push_str(&format!("{}. **[{}]({})**\n", i + 1, title, url));
        if !meta.is_empty() {
            output.push_str(&format!("   *{}*\n", meta.join(" · ")));
        }
        if !description.is_empty() {
            output.push_str(&format!("   {}\n", description));
        }
        if let Some(content) = item.get("content").and_then(|c| c.as_str()) {
            let trimmed_content = content.trim();
            if !trimmed_content.is_empty() {
                let cut = if trimmed_content.len() > 1000 {
                    format!("{}\n   [...]", safe_truncate_str(trimmed_content, 1000))
                } else {
                    trimmed_content.to_string()
                };
                output.push_str(&format!("   *Extracted Content:*\n   {}\n", cut.replace('\n', "\n   ")));
            }
        }
        output.push('\n');

        if !url.is_empty() {
            sources.push(format!("- [{}]({})", title, url));
        }
    }

    if !sources.is_empty() {
        output.push_str("Sources:\n");
        for s in sources {
            output.push_str(&s);
            output.push('\n');
        }
    }

    output
}

pub fn format_fetch_result(url: &str, title: &str, content: &str) -> String {
    let mut md = String::new();
    if !title.is_empty() {
        md.push_str(&format!("# {}\n\n", title));
    }
    md.push_str(content);

    let truncated = if md.len() > 30000 {
        let cut = safe_truncate_str(&md, 30000);
        format!("{}\n\n[Content truncated after 30,000 characters]", cut)
    } else {
        md
    };

    format!("Content from {} (Markdown via DeepSeek Fetch):\n\n{}", url, truncated)
}

async fn search_with_deepseek(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
    count: Option<u32>,
    ori_question: Option<&str>,
    need_content: bool,
) -> Result<String> {
    let url = "https://api.deepseek.com/v1/tools/search";

    let trimmed = query.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Search query cannot be empty");
    }
    let safe_query = safe_truncate_str(trimmed, 128);

    let effective_count = count.unwrap_or(8).clamp(1, 50);
    let mut payload = json!({
        "query": safe_query,
        "count": effective_count,
        "need_content": need_content
    });

    if let Some(oq) = ori_question {
        let trimmed_oq = oq.trim();
        if !trimmed_oq.is_empty() {
            payload["ori_question"] = json!(safe_truncate_str(trimmed_oq, 128));
        }
    }

    let res = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&payload)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await?;

    if !res.status().is_success() {
        let status = res.status();
        let err_text = res.text().await.unwrap_or_default();
        if status.as_u16() == 402 {
            anyhow::bail!("DeepSeek API balance insufficient. Run /balance to check your credits.");
        } else if status.as_u16() == 429 {
            anyhow::bail!("DeepSeek Search QPS limit exceeded (5 req/sec). Please retry shortly.");
        }
        anyhow::bail!("DeepSeek Web Search HTTP {}: {}", status, err_text);
    }

    let val: serde_json::Value = res.json().await?;
    let results = val.get("results").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    Ok(format_search_results(query, &results))
}

async fn fetch_with_deepseek(client: &reqwest::Client, api_key: &str, url: &str) -> Result<String> {
    let endpoint = "https://api.deepseek.com/v1/tools/fetch";
    let payload = json!({
        "url": url
    });

    let res = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&payload)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;

    if !res.status().is_success() {
        let status = res.status();
        let err_text = res.text().await.unwrap_or_default();
        if status.as_u16() == 402 {
            anyhow::bail!("DeepSeek API balance insufficient. Run /balance to check your credits.");
        } else if status.as_u16() == 403 {
            anyhow::bail!("Access rejected by DeepSeek Fetch security policy for target URL.");
        } else if status.as_u16() == 404 {
            anyhow::bail!("Page content could not be retrieved by DeepSeek Fetch (unsupported type, blocked, or missing).");
        } else if status.as_u16() == 429 {
            anyhow::bail!("DeepSeek Fetch QPS limit exceeded (5 req/sec). Please retry shortly.");
        }
        anyhow::bail!("DeepSeek Web Fetch HTTP {}: {}", status, err_text);
    }

    let val: serde_json::Value = res.json().await?;
    let title = val.get("title").and_then(|t| t.as_str()).unwrap_or("").trim();
    let content = val.get("content").and_then(|c| c.as_str()).unwrap_or("").trim();

    if content.is_empty() {
        anyhow::bail!("DeepSeek Fetch returned empty content");
    }

    Ok(format_fetch_result(url, title, content))
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
        "Performs real-time web search using DeepSeek's native search engine to retrieve up-to-date facts, breaking news, market data, and documentation. Supports search operators (site:, -site:, \"exact phrase\", lang:, after:, before:). [PREFERRED for finding real-time information, current news, and documentation]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to find information on the web (1-128 chars). Supports operators like site:github.com, -advertisement, \"exact phrase\", lang:en, after:2025."
                },
                "count": {
                    "type": "integer",
                    "description": "Optional number of results to return (between 1 and 50, default 8)."
                },
                "ori_question": {
                    "type": "string",
                    "description": "Optional user's original question to improve search query understanding (1-128 chars)."
                },
                "need_content": {
                    "type": "boolean",
                    "description": "Optional flag: if true, returns page content excerpts directly in each result."
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
        let count = args.get("count").and_then(|v| v.as_u64()).map(|n| n as u32);
        let ori_question = args.get("ori_question").and_then(|v| v.as_str());
        let need_content = args.get("need_content").and_then(|v| v.as_bool()).unwrap_or(false);

        let client = get_http_client();

        if let Some(api_key) = resolve_api_key(context) {
            match search_with_deepseek(client, &api_key, query, count, ori_question, need_content).await {
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
                || ipv4.is_multicast()
                || (ipv4.octets()[0] == 169 && ipv4.octets()[1] == 254) // Cloud metadata (AWS, GCP, Azure)
                || (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0xC0) == 64) // Carrier-grade NAT
                || (ipv4.octets()[0] == 198 && (ipv4.octets()[1] & 0xFE) == 18) // Benchmarking (198.18.0.0/15)
        }
        std::net::IpAddr::V6(ipv6) => {
            if let Some(mapped_v4) = ipv6.to_ipv4_mapped() {
                if is_blocked_ip(std::net::IpAddr::V4(mapped_v4)) {
                    return true;
                }
            }
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                || ipv6.is_multicast()
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
    let clean_host = host.trim_start_matches('[').trim_end_matches(']');
    let lower_host = clean_host.to_ascii_lowercase();

    if lower_host == "localhost"
        || lower_host.ends_with(".localhost")
        || lower_host.ends_with(".internal")
    {
        return Err(format!("Access to localhost ('{}') is forbidden", host));
    }

    if lower_host == "nip.io"
        || lower_host.ends_with(".nip.io")
        || lower_host == "sslip.io"
        || lower_host.ends_with(".sslip.io")
        || lower_host == "xip.io"
        || lower_host.ends_with(".xip.io")
    {
        return Err(format!(
            "Access to DNS-rebinding wildcard domain ('{}') is forbidden",
            host
        ));
    }

    if let Ok(ip) = clean_host.parse::<std::net::IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(format!("Access to internal/private IP address '{}' is forbidden", ip));
        }
    } else {
        let port = parsed.port_or_known_default().unwrap_or(80);
        match tokio::net::lookup_host((clean_host, port)).await {
            Ok(addrs) => {
                let mut resolved_any = false;
                for addr in addrs {
                    resolved_any = true;
                    if is_blocked_ip(addr.ip()) {
                        return Err(format!("Host '{}' resolves to private/internal IP '{}'; access is forbidden", host, addr.ip()));
                    }
                }
                if !resolved_any {
                    return Err(format!("Host '{}' did not resolve to any IP address", host));
                }
            }
            Err(e) => {
                return Err(format!("Failed to resolve host '{}': {}", host, e));
            }
        }
    }

    Ok(parsed)
}

async fn fetch_url_direct(
    client: &reqwest::Client,
    start_url: &str,
) -> Result<(String, reqwest::Response), String> {
    let mut current_url = validate_url_for_ssrf(start_url)
        .await
        .map_err(|e| format!("Security restriction: {}", e))?;

    for hop in 0..=5 {
        let resp = client
            .get(current_url.clone())
            .send()
            .await
            .map_err(|e| format!("Failed to fetch {}: {}", current_url, e))?;

        if matches!(resp.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            if hop == 5 {
                return Err(format!("Too many redirects when fetching {}", start_url));
            }
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "Redirect response missing Location header".to_string())?;
            let next_url = current_url
                .join(loc)
                .map_err(|e| format!("Invalid redirect Location '{}': {}", loc, e))?;
            current_url = validate_url_for_ssrf(next_url.as_str())
                .await
                .map_err(|e| format!("Security restriction: {}", e))?;
            continue;
        }

        return Ok((current_url.to_string(), resp));
    }

    Err(format!("Too many redirects when fetching {}", start_url))
}

fn alternative_www_url(url: &str) -> Option<String> {
    let mut parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_string();
    if let Some(clean) = host.strip_prefix("www.") {
        let clean_owned = clean.to_string();
        parsed.set_host(Some(&clean_owned)).ok()?;
        Some(parsed.to_string())
    } else if !host.eq_ignore_ascii_case("localhost") && host.contains('.') {
        let with_www = format!("www.{}", host);
        parsed.set_host(Some(&with_www)).ok()?;
        Some(parsed.to_string())
    } else {
        None
    }
}

// --- WebFetchTool ---
pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "Fetches the content of a web page URL and converts it into clean Markdown text using DeepSeek's native fetch engine, with safe local HTML fallback."
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

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
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

        // 1. Try DeepSeek's native Fetch API first if an API key is available
        if let Some(api_key) = resolve_api_key(context) {
            match fetch_with_deepseek(client, &api_key, &url).await {
                Ok(content) if !content.trim().is_empty() => {
                    return Ok(ToolOutput::success(content));
                }
                Ok(_) | Err(_) => {
                    // Try alternative www/non-www domain if available
                    if let Some(alt_url) = alternative_www_url(&url) {
                        if let Ok(alt_valid) = validate_url_for_ssrf(&alt_url).await {
                            let alt_str = alt_valid.to_string();
                            if let Ok(alt_content) = fetch_with_deepseek(client, &api_key, &alt_str).await {
                                if !alt_content.trim().is_empty() {
                                    return Ok(ToolOutput::success(alt_content));
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Safe local scraper fallback with per-hop SSRF redirect checking and www/apex fallback
        let (target_fetch_url, mut resp) = match fetch_url_direct(client, &url).await {
            Ok((final_url, r)) if r.status().is_success() => (final_url, r),
            Ok((_, r)) => {
                if let Some(alt_url) = alternative_www_url(&url) {
                    if let Ok((alt_final, alt_resp)) = fetch_url_direct(client, &alt_url).await {
                        if alt_resp.status().is_success() {
                            (alt_final, alt_resp)
                        } else {
                            return Ok(ToolOutput::error(format!("HTTP {} error when fetching {}", r.status(), url)));
                        }
                    } else {
                        return Ok(ToolOutput::error(format!("HTTP {} error when fetching {}", r.status(), url)));
                    }
                } else {
                    return Ok(ToolOutput::error(format!("HTTP {} error when fetching {}", r.status(), url)));
                }
            }
            Err(e) => return Ok(ToolOutput::error(e)),
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

        Ok(ToolOutput::success(format!("Content from {}:\n\n{}", target_fetch_url, truncated)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_search_results_with_metadata() {
        let raw_json = serde_json::json!([
            {
                "rerank_score": 2.4,
                "title": "DeepSeek V4 model released",
                "url": "https://example.com/deepseek-v4",
                "snippet": "DeepSeek V4 is now available.",
                "summary": "DeepSeek has released the V4 model with tool calling and long-context support.",
                "site_name": "Example News",
                "date": "2026-06-01T00:00:00+00:00",
                "authority": 3
            },
            {
                "rerank_score": 1.8,
                "title": "Search API Guide",
                "url": "https://api-docs.deepseek.com/guides/search",
                "snippet": "The Search API finds web pages for a query.",
                "summary": "",
                "site_name": "DeepSeek Docs",
                "date": null,
                "authority": 4
            }
        ]);

        let results = raw_json.as_array().unwrap();
        let formatted = format_search_results("DeepSeek release", results);

        assert!(formatted.contains("1. **[DeepSeek V4 model released](https://example.com/deepseek-v4)**"));
        assert!(formatted.contains("Example News · 2026-06-01"));
        assert!(formatted.contains("DeepSeek has released the V4 model"));
        assert!(formatted.contains("2. **[Search API Guide](https://api-docs.deepseek.com/guides/search)**"));
        assert!(formatted.contains("The Search API finds web pages for a query."));
        assert!(formatted.contains("Sources:\n- [DeepSeek V4 model released](https://example.com/deepseek-v4)\n- [Search API Guide](https://api-docs.deepseek.com/guides/search)"));
    }

    #[test]
    fn test_format_search_results_empty() {
        let results = vec![];
        let formatted = format_search_results("nonexistent-topic-query", &results);
        assert_eq!(formatted, "Search completed for 'nonexistent-topic-query'. Found 0 direct matches.");
    }

    #[test]
    fn test_format_fetch_result() {
        let url = "https://example.com/doc";
        let title = "API Reference";
        let content = "## Endpoints\n\n- `POST /v1/tools/search`\n- `POST /v1/tools/fetch`";

        let formatted = format_fetch_result(url, title, content);
        assert!(formatted.contains("Content from https://example.com/doc (Markdown via DeepSeek Fetch):"));
        assert!(formatted.contains("# API Reference\n\n## Endpoints"));
        assert!(formatted.contains("POST /v1/tools/fetch"));
    }

    #[test]
    fn test_alternative_www_url() {
        assert_eq!(alternative_www_url("https://www.sluisr.com/path"), Some("https://sluisr.com/path".to_string()));
        assert_eq!(alternative_www_url("https://sluisr.com/path"), Some("https://www.sluisr.com/path".to_string()));
        assert_eq!(alternative_www_url("http://localhost:8080"), None);
    }
}
