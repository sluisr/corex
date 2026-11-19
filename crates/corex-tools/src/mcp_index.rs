use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::types::{Tool, ToolContext, ToolOutput};

pub const SEARCH_TOOL: &str = "mcp_search_tools";
pub const CALL_TOOL: &str = "mcp_call_tool";

// Above this many MCP tools their schemas stay out of every request.
pub const INLINE_LIMIT: usize = 24;

const MAX_RESULTS: usize = 8;

pub type McpTools = Arc<HashMap<String, Arc<dyn Tool>>>;

pub fn is_meta_tool(name: &str) -> bool {
    name == SEARCH_TOOL || name == CALL_TOOL
}

pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp_") && !is_meta_tool(name)
}

fn server_counts(tools: &McpTools) -> String {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for tool in tools.values() {
        let server = tool
            .description()
            .strip_prefix("[MCP: ")
            .and_then(|rest| rest.split_once(']'))
            .map_or("?", |(server, _)| server);
        *counts.entry(server).or_default() += 1;
    }
    let mut list: Vec<_> = counts.into_iter().map(|(s, n)| format!("{s} ({n})")).collect();
    list.sort();
    list.join(", ")
}

fn score(query_terms: &[String], name: &str, description: &str) -> usize {
    let name = name.to_ascii_lowercase();
    let description = description.to_ascii_lowercase();
    query_terms
        .iter()
        .map(|term| {
            if name.contains(term.as_str()) {
                3
            } else if description.contains(term.as_str()) {
                1
            } else {
                0
            }
        })
        .sum()
}

pub struct McpSearchTool {
    tools: McpTools,
    description: String,
}

impl McpSearchTool {
    pub fn new(tools: McpTools) -> Self {
        let description = format!(
            "Find MCP tools by keyword. {} MCP tools are available but their schemas are not loaded: \
             search here, then run one with {CALL_TOOL}. Servers: {}.",
            tools.len(),
            server_counts(&tools)
        );
        Self { tools, description }
    }
}

#[async_trait]
impl Tool for McpSearchTool {
    fn name(&self) -> &str {
        SEARCH_TOOL
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Keywords describing the capability, or an exact tool name." }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolOutput> {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|t| t.len() > 1)
            .map(str::to_string)
            .collect();
        let mut ranked: Vec<(usize, &String, &Arc<dyn Tool>)> = self
            .tools
            .iter()
            .map(|(name, tool)| {
                let exact = if *name == query { 100 } else { 0 };
                (exact + score(&terms, name, tool.description()), name, tool)
            })
            .filter(|(score, ..)| *score > 0)
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        if ranked.is_empty() {
            return Ok(ToolOutput::success(format!(
                "No MCP tool matches \"{query}\". Servers: {}.",
                server_counts(&self.tools)
            )));
        }
        let matches: Vec<Value> = ranked
            .into_iter()
            .take(MAX_RESULTS)
            .map(|(_, name, tool)| json!({ "name": name, "description": tool.description(), "parameters": tool.parameters() }))
            .collect();
        let count = matches.len();
        Ok(ToolOutput::success_with_summary(
            serde_json::to_string_pretty(&json!({ "tools": matches, "call_with": CALL_TOOL })).unwrap_or_default(),
            format!("{count} MCP tools for \"{query}\""),
        ))
    }
}

pub struct McpCallTool {
    tools: McpTools,
}

impl McpCallTool {
    pub fn new(tools: McpTools) -> Self {
        Self { tools }
    }

    fn target(&self, args: &Value) -> Option<(&Arc<dyn Tool>, Value)> {
        let name = args.get("name").and_then(Value::as_str)?;
        let arguments = match args.get("arguments") {
            Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or(Value::Null),
            Some(value) => value.clone(),
            None => json!({}),
        };
        self.tools.get(name).map(|tool| (tool, arguments))
    }
}

#[async_trait]
impl Tool for McpCallTool {
    fn name(&self) -> &str {
        CALL_TOOL
    }

    fn description(&self) -> &str {
        "Run an MCP tool found with mcp_search_tools, passing its arguments as an object that follows the schema it returned."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Exact MCP tool name from mcp_search_tools." },
                "arguments": { "type": "object", "description": "Arguments for that tool." }
            },
            "required": ["name"]
        })
    }

    fn needs_confirmation(&self, args: &Value, context: &ToolContext) -> bool {
        self.target(args).map_or(true, |(tool, arguments)| tool.needs_confirmation(&arguments, context))
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolOutput> {
        match self.target(&args) {
            Some((tool, arguments)) => tool.execute(arguments, context).await,
            None => Ok(ToolOutput::error(format!(
                "Unknown MCP tool {}. Find it first with {SEARCH_TOOL}.",
                args.get("name").and_then(Value::as_str).unwrap_or("(missing name)")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(&'static str, &'static str, bool);

    #[async_trait]
    impl Tool for Fake {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            self.1
        }
        fn parameters(&self) -> Value {
            json!({"type": "object"})
        }
        fn needs_confirmation(&self, _args: &Value, _context: &ToolContext) -> bool {
            self.2
        }
        async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolOutput> {
            Ok(ToolOutput::success(format!("{} {}", self.0, args)))
        }
    }

    fn ctx() -> ToolContext {
        ToolContext {
            workspace_dir: std::env::temp_dir(),
            yolo_mode: false,
            yolo_all: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        }
    }

    fn tools() -> McpTools {
        let list: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Fake("mcp_rea_open_binary", "[MCP: rea] Open a native binary for analysis", true)),
            Arc::new(Fake("mcp_rea_list_strings", "[MCP: rea] List strings in the open binary", false)),
            Arc::new(Fake("mcp_stitch_get_project", "[MCP: stitch] Get a design project", false)),
        ];
        Arc::new(list.into_iter().map(|t| (t.name().to_string(), t)).collect())
    }

    #[tokio::test]
    async fn search_ranks_name_matches_and_returns_schemas() {
        let out = McpSearchTool::new(tools()).execute(json!({"query": "binary strings"}), &ctx()).await.unwrap();
        let first = out.output.find("mcp_rea_list_strings").unwrap();
        let second = out.output.find("mcp_rea_open_binary").unwrap();
        assert!(first < second);
        assert!(out.output.contains("\"parameters\""));
        assert!(!out.output.contains("stitch_get_project"));
    }

    #[tokio::test]
    async fn call_dispatches_and_inherits_confirmation() {
        let call = McpCallTool::new(tools());
        let args = json!({"name": "mcp_rea_open_binary", "arguments": {"path": "/bin/ls"}});
        assert!(call.needs_confirmation(&args, &ctx()));
        assert!(!call.needs_confirmation(&json!({"name": "mcp_rea_list_strings"}), &ctx()));
        let out = call.execute(args, &ctx()).await.unwrap();
        assert_eq!(out.output, r#"mcp_rea_open_binary {"path":"/bin/ls"}"#);
        assert!(call.execute(json!({"name": "nope"}), &ctx()).await.unwrap().is_error);
    }
}
