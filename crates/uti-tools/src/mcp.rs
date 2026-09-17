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
use uti_core::config::McpServerConfig;

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

pub struct McpClient {
    pub server_name: String,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout_reader: Arc<Mutex<BufReader<ChildStdout>>>,
    request_id: AtomicU64,
    _child: Arc<Mutex<tokio::process::Child>>,
}

impl McpClient {
    pub async fn spawn(server_name: &str, cfg: &McpServerConfig) -> Result<Self> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args);
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
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
            stdin: Arc::new(Mutex::new(stdin)),
            stdout_reader: Arc::new(Mutex::new(reader)),
            request_id: AtomicU64::new(1),
            _child: Arc::new(Mutex::new(child)),
        };

        // Initialize MCP handshake
        client.initialize().await?;
        Ok(client)
    }

    async fn send_raw(&self, request: &Value) -> Result<Value> {
        let payload = serde_json::to_string(request)? + "\n";
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(payload.as_bytes()).await?;
            stdin.flush().await?;
        }

        // If it's a notification without an "id", do not wait for a response
        if request.get("id").is_none() {
            return Ok(Value::Null);
        }

        let expected_id = request.get("id").and_then(|v| v.as_u64());

        // Read lines until matching response is found
        let mut reader = self.stdout_reader.lock().await;
        let mut line = String::new();
        while reader.read_line(&mut line).await? > 0 {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                line.clear();
                continue;
            }

            if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                if let Some(id) = parsed.get("id").and_then(|v| v.as_u64()) {
                    if Some(id) == expected_id {
                        return Ok(parsed);
                    }
                }
            }
            line.clear();
        }

        bail!("MCP server closed stdout stream unexpectedly")
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
                    "name": "uti-cli",
                    "version": "0.1.0"
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

        Ok(ToolOutput::success(output_str))
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
