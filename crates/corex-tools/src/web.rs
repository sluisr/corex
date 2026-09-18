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

fn snippet_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)<a class="result__snippet[^"]*"[^>]*>(.*?)</a>"#).unwrap())
}

fn title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)<a class="result__url"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#).unwrap())
}

// --- WebSearchTool (google_web_search) ---
pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "google_web_search"
    }

    fn description(&self) -> &'static str {
        "Performs a web search using Google Search and returns the results. [PREFERRED for finding real-time information, documentation, and current web content]"
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

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return Ok(ToolOutput::error("Missing 'query' argument.")),
        };

        let client = get_http_client();
        let encoded_query = query.replace(' ', "+");
        let search_url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);

        let resp = match client.get(&search_url).send().await {
            Ok(r) => r.text().await.unwrap_or_default(),
            Err(e) => return Ok(ToolOutput::error(format!("Search request failed: {}", e))),
        };

        let mut results = Vec::new();
        let snippets: Vec<String> = snippet_re()
            .captures_iter(&resp)
            .take(5)
            .map(|c| tag_strip_re().replace_all(&c[1], "").trim().to_string())
            .collect();

        let titles: Vec<(String, String)> = title_re()
            .captures_iter(&resp)
            .take(5)
            .map(|c| {
                let url = c[1].trim().to_string();
                let title = tag_strip_re().replace_all(&c[2], "").trim().to_string();
                (title, url)
            })
            .collect();

        for (i, (title, url)) in titles.iter().enumerate() {
            let snippet = snippets.get(i).map(|s| s.as_str()).unwrap_or("");
            results.push(format!("### {}\nURL: {}\n{}\n", title, url, snippet));
        }

        if results.is_empty() {
            Ok(ToolOutput::success(format!("Search completed for '{}'. Found 0 direct instant matches.", query)))
        } else {
            Ok(ToolOutput::success(results.join("\n---\n")))
        }
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

        let url = match target_url {
            Some(u) => u,
            None => return Ok(ToolOutput::error("No valid http:// or https:// URL found in prompt.")),
        };

        let client = get_http_client();

        let body = match client.get(&url).send().await {
            Ok(resp) => resp.text().await.unwrap_or_default(),
            Err(e) => return Ok(ToolOutput::error(format!("Failed to fetch {}: {}", url, e))),
        };

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
