use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use corex_core::config::McpServerConfig;

use crate::types::{Tool, ToolContext, ToolOutput};

#[derive(Debug, Clone)]
pub struct McpServerStatus {
    pub name: String,
    pub command: String,
    pub is_connected: bool,
    pub tools_count: usize,
    pub tools: Vec<String>,
    pub error: Option<String>,
}

const MAX_MCP_LINE_BYTES: usize = 16 * 1024 * 1024;

struct McpTransport {
    stdin: ChildStdin,
    stdout_reader: BufReader<ChildStdout>,
    read_buf: Vec<u8>,
}

impl McpTransport {
    async fn read_next_line(&mut self) -> Result<Option<String>> {
        loop {
            let available = self.stdout_reader.fill_buf().await?;
            if available.is_empty() {
                if self.read_buf.is_empty() {
                    return Ok(None);
                }
                let line_bytes = std::mem::take(&mut self.read_buf);
                return Ok(Some(String::from_utf8_lossy(&line_bytes).into_owned()));
            }
            if let Some(nl_pos) = available.iter().position(|&b| b == b'\n') {
                let to_copy = &available[..nl_pos];
                if self.read_buf.len() + to_copy.len() > MAX_MCP_LINE_BYTES {
                    self.stdout_reader.consume(nl_pos + 1);
                    self.read_buf.clear();
                    bail!("MCP server response line exceeded maximum allowed size (16 MiB)");
                }
                self.read_buf.extend_from_slice(to_copy);
                self.stdout_reader.consume(nl_pos + 1);
                let line_bytes = std::mem::take(&mut self.read_buf);
                return Ok(Some(String::from_utf8_lossy(&line_bytes).into_owned()));
            } else {
                let chunk_len = available.len();
                if self.read_buf.len() + chunk_len > MAX_MCP_LINE_BYTES {
                    self.stdout_reader.consume(chunk_len);
                    self.read_buf.clear();
                    bail!("MCP server response line exceeded maximum allowed size (16 MiB)");
                }
                self.read_buf.extend_from_slice(available);
                self.stdout_reader.consume(chunk_len);
            }
        }
    }
}

pub struct McpClient {
    pub server_name: String,
    transport: Arc<Mutex<McpTransport>>,
    request_id: AtomicU64,
    _child: Arc<Mutex<tokio::process::Child>>,
}

impl McpClient {
    pub async fn spawn(server_name: &str, cfg: &McpServerConfig) -> Result<Self> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args);
        // Cleanse sensitive core process credentials and cloud LLM API keys from MCP child processes
        // unless explicitly provided in `cfg.env`.
        for sensitive_var in [
            "COREX_SUDO_PASSWORD",
            "DEEPSEEK_SUDO_PASSWORD",
            "SUDO_ASKPASS",
            "SSH_ASKPASS",
            "COREX_API_KEY",
            "DEEPSEEK_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GROQ_API_KEY",
            "OPENROUTER_API_KEY",
            "MISTRAL_API_KEY",
            "GITHUB_TOKEN",
            "COREX_GITHUB_TOKEN",
        ] {
            cmd.env_remove(sensitive_var);
        }
        for (var_name, _) in std::env::vars() {
            if var_name.starts_with("COREX_") && var_name.ends_with("_API_KEY") {
                cmd.env_remove(&var_name);
            }
        }
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        // Discard stderr so a verbose MCP server cannot fill the 64 KB OS pipe buffer and deadlock.
        cmd.stderr(Stdio::null());
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn().with_context(|| {
            format!("Failed to spawn MCP server '{}' with command '{}'", server_name, cfg.command)
        })?;

        let stdin = child.stdin.take().context("Failed to open stdin for MCP server")?;
        let stdout = child.stdout.take().context("Failed to open stdout for MCP server")?;
        let reader = BufReader::new(stdout);

        let client = Self {
            server_name: server_name.to_string(),
            transport: Arc::new(Mutex::new(McpTransport {
                stdin,
                stdout_reader: reader,
                read_buf: Vec::new(),
            })),
            request_id: AtomicU64::new(1),
            _child: Arc::new(Mutex::new(child)),
        };

        // Initialize MCP handshake
        client.initialize().await?;
        Ok(client)
    }

    async fn send_raw(&self, request: &Value) -> Result<Value> {
        let payload = serde_json::to_string(request)? + "\n";
        let mut transport = self.transport.lock().await;
        transport.stdin.write_all(payload.as_bytes()).await?;
        transport.stdin.flush().await?;

        // If it's a notification without an "id", do not wait for a response
        if request.get("id").is_none() {
            return Ok(Value::Null);
        }

        let expected_id = request.get("id").and_then(|v| v.as_u64());

        // Read lines until matching response is found (with 60s timeout to prevent hanging)
        let timeout_duration = std::time::Duration::from_secs(60);

        let read_future = async {
            while let Some(line) = transport.read_next_line().await? {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                    if let Some(id) = parsed.get("id").and_then(|v| v.as_u64()) {
                        if Some(id) == expected_id {
                            return Ok(parsed);
                        }
                    } else if parsed.get("error").is_some()
                        && parsed.get("id").map_or(true, |v| v.is_null())
                    {
                        return Ok(parsed);
                    }
                }
            }

            bail!("MCP server closed stdout stream unexpectedly")
        };

        match tokio::time::timeout(timeout_duration, read_future).await {
            Ok(res) => res,
            Err(_) => bail!("Timeout waiting for MCP server response (60s)"),
        }
    }

    async fn initialize(&self) -> Result<()> {
        let init_id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let init_req = json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "corex",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        });

        let resp = self.send_raw(&init_req).await?;
        if let Some(err) = resp.get("error") {
            bail!("MCP initialize error: {}", err);
        }

        // Send notifications/initialized
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let _ = self.send_raw(&notif).await;

        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<(String, String, Value)>> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/list",
            "params": {}
        });

        let resp = self.send_raw(&req).await?;
        if let Some(err) = resp.get("error") {
            bail!("MCP tools/list error: {}", err);
        }

        let mut tools = Vec::new();
        if let Some(tool_list) = resp.get("result").and_then(|r| r.get("tools")).and_then(|t| t.as_array()) {
            for t in tool_list {
                let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let schema = t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"}));
                if !name.is_empty() {
                    tools.push((name, desc, schema));
                }
            }
        }

        Ok(tools)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolOutput> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments
            }
        });

        let resp = self.send_raw(&req).await?;
        if let Some(err) = resp.get("error") {
            return Ok(ToolOutput::error(format!("MCP tool error: {}", err)));
        }

        let mut output_str = String::new();
        if let Some(content_arr) = resp.get("result").and_then(|r| r.get("content")).and_then(|c| c.as_array()) {
            for item in content_arr {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    output_str.push_str(text);
                }
            }
        } else if let Some(res) = resp.get("result") {
            output_str = serde_json::to_string_pretty(res).unwrap_or_default();
        }

        let is_error = resp
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_error {
            Ok(ToolOutput::error(output_str))
        } else {
            Ok(ToolOutput::success(output_str))
        }
    }
}

pub struct McpTool {
    pub display_name: String,
    pub original_name: String,
    pub description_str: String,
    pub parameters_schema: Value,
    pub client: Arc<McpClient>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn description(&self) -> &str {
        &self.description_str
    }

    fn parameters(&self) -> Value {
        self.parameters_schema.clone()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolOutput> {
        self.client.call_tool(&self.original_name, args).await
    }
}

pub async fn load_mcp_servers(
    configs: &HashMap<String, McpServerConfig>,
) -> (Vec<Arc<dyn Tool>>, Vec<McpServerStatus>) {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    let mut statuses: Vec<McpServerStatus> = Vec::new();

    for (name, cfg) in configs {
        let cmd_display = format!("{} {}", cfg.command, cfg.args.join(" "));
        match McpClient::spawn(name, cfg).await {
            Ok(client) => {
                let client_arc = Arc::new(client);
                match client_arc.list_tools().await {
                    Ok(tool_list) => {
                        let count = tool_list.len();
                        let mut tool_names = Vec::new();

                        for (orig_name, desc, schema) in tool_list {
                            let qualified_name = format!("mcp_{}_{}", name, orig_name);
                            tool_names.push(orig_name.clone());

                            let mcp_tool = Arc::new(McpTool {
                                display_name: qualified_name,
                                original_name: orig_name,
                                description_str: format!("[MCP: {}] {}", name, desc),
                                parameters_schema: schema,
                                client: client_arc.clone(),
                            });
                            tools.push(mcp_tool);
                        }

                        statuses.push(McpServerStatus {
                            name: name.clone(),
                            command: cmd_display,
                            is_connected: true,
                            tools_count: count,
                            tools: tool_names,
                            error: None,
                        });
                    }
                    Err(e) => {
                        statuses.push(McpServerStatus {
                            name: name.clone(),
                            command: cmd_display,
                            is_connected: false,
                            tools_count: 0,
                            tools: Vec::new(),
                            error: Some(format!("Failed to list tools: {}", e)),
                        });
                    }
                }
            }
            Err(e) => {
                statuses.push(McpServerStatus {
                    name: name.clone(),
                    command: cmd_display,
                    is_connected: false,
                    tools_count: 0,
                    tools: Vec::new(),
                    error: Some(e.to_string()),
                });
            }
        }
    }

    (tools, statuses)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct McpCatalogItem {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub command: &'static str,
    pub default_args: &'static [&'static str],
    pub env_keys: &'static [&'static str],
    pub example: &'static str,
}

pub static MCP_CATALOG: &[McpCatalogItem] = &[
    McpCatalogItem {
        id: "github",
        name: "GitHub",
        description: "Search repos, inspect code, manage issues and pull requests via official GitHub MCP",
        command: "npx",
        default_args: &["-y", "@modelcontextprotocol/server-github"],
        env_keys: &["GITHUB_PERSONAL_ACCESS_TOKEN"],
        example: "/mcp add github",
    },
    McpCatalogItem {
        id: "sqlite",
        name: "SQLite Database",
        description: "Inspect schema and run SQL queries against local SQLite database files",
        command: "npx",
        default_args: &["-y", "@modelcontextprotocol/server-sqlite", "--db-path", "app.db"],
        env_keys: &[],
        example: "/mcp add sqlite app.db",
    },
    McpCatalogItem {
        id: "postgres",
        name: "PostgreSQL Database",
        description: "Connect to PostgreSQL databases, inspect tables, and run read/write queries",
        command: "npx",
        default_args: &["-y", "@modelcontextprotocol/server-postgres", "postgresql://localhost/mydb"],
        env_keys: &[],
        example: "/mcp add postgres postgresql://localhost/mydb",
    },
    McpCatalogItem {
        id: "filesystem",
        name: "Secure Filesystem",
        description: "Sandboxed filesystem tools scoped to specific authorized directories",
        command: "npx",
        default_args: &["-y", "@modelcontextprotocol/server-filesystem", "."],
        env_keys: &[],
        example: "/mcp add filesystem .",
    },
    McpCatalogItem {
        id: "memory",
        name: "Knowledge Graph Memory",
        description: "Persistent graph memory across coding sessions and projects",
        command: "npx",
        default_args: &["-y", "@modelcontextprotocol/server-memory"],
        env_keys: &[],
        example: "/mcp add memory",
    },
    McpCatalogItem {
        id: "brave-search",
        name: "Brave Web Search",
        description: "Live web search and news indexing through Brave Search API",
        command: "npx",
        default_args: &["-y", "@modelcontextprotocol/server-brave-search"],
        env_keys: &["BRAVE_API_KEY"],
        example: "/mcp add brave-search",
    },
    McpCatalogItem {
        id: "fetch",
        name: "Web Fetch & HTML Reader",
        description: "Fetch web pages and convert HTML to readable Markdown",
        command: "uvx",
        default_args: &["mcp-server-fetch"],
        env_keys: &[],
        example: "/mcp add fetch",
    },
    McpCatalogItem {
        id: "puppeteer",
        name: "Puppeteer Web Automation",
        description: "Headless Chromium browser control, navigation, screenshots, and form filling",
        command: "npx",
        default_args: &["-y", "@modelcontextprotocol/server-puppeteer"],
        env_keys: &[],
        example: "/mcp add puppeteer",
    },
    McpCatalogItem {
        id: "docker",
        name: "Docker Container Manager",
        description: "Inspect containers, check container logs, start/stop services via Docker daemon",
        command: "npx",
        default_args: &["-y", "@modelcontextprotocol/server-docker"],
        env_keys: &[],
        example: "/mcp add docker",
    },
    McpCatalogItem {
        id: "slack",
        name: "Slack Channel Integration",
        description: "Post messages and inspect threads in Slack workspaces",
        command: "npx",
        default_args: &["-y", "@modelcontextprotocol/server-slack"],
        env_keys: &["SLACK_BOT_TOKEN", "SLACK_TEAM_ID"],
        example: "/mcp add slack",
    },
];

pub fn get_mcp_catalog() -> &'static [McpCatalogItem] {
    MCP_CATALOG
}

pub fn find_catalog_item(id: &str) -> Option<&'static McpCatalogItem> {
    MCP_CATALOG.iter().find(|item| item.id.eq_ignore_ascii_case(id))
}

