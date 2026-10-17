use std::path::Path;
use std::sync::Arc;
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use corex_tools::types::{Tool, ToolContext, ToolOutput};

use crate::config::{FleetConfig, NodeConfig};
use crate::ssh::{assess_command_risk, CommandRiskLevel, SshExecutor};
use crate::telemetry::TelemetryCollector;

fn resolve_target_nodes<'a>(config: &'a FleetConfig, targets_str: &str) -> Vec<&'a NodeConfig> {
    let targets_str = targets_str.trim();
    if targets_str.eq_ignore_ascii_case("all") {
        config.all_nodes()
    } else if let Some(tag) = targets_str.strip_prefix("tag:") {
        config.nodes_by_tag(tag.trim())
    } else if let Some(cluster_id) = targets_str.strip_prefix("cluster:") {
        config
            .clusters
            .get(cluster_id.trim())
            .map(|c| c.nodes.iter().collect())
            .unwrap_or_default()
    } else {
        targets_str
            .split(',')
            .filter_map(|id| config.find_node(id.trim()))
            .collect()
    }
}

/// Tool to list configured nodes and clusters.
pub struct FleetListNodesTool;

#[async_trait]
impl Tool for FleetListNodesTool {
    fn name(&self) -> &str {
        "fleet_list_nodes"
    }

    fn description(&self) -> &str {
        "List all remote servers (VPS), clusters, and tags configured in the Corex Fleet inventory."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "check_status": {
                    "type": "boolean",
                    "description": "If true, tests SSH connectivity and measures latency for each node."
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let config = FleetConfig::load(Some(&context.workspace_dir))?;
        let all_nodes = config.all_nodes();

        if all_nodes.is_empty() {
            return Ok(ToolOutput::success(
                "No fleet nodes configured. Create `.corex/fleet.toml` or configure hosts in `~/.ssh/config`."
            ));
        }

        let check_status = args.get("check_status").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut output = String::new();
        output.push_str(&format!("📦 Corex Fleet Inventory ({} nodes found):\n\n", all_nodes.len()));

        if check_status {
            let semaphore = Arc::new(Semaphore::new(16));
            let mut set = JoinSet::new();
            for (idx, node) in all_nodes.iter().enumerate() {
                let n = (*node).clone();
                let timeout = config.timeout_secs;
                let sem = semaphore.clone();
                set.spawn(async move {
                    let _permit = sem.acquire_owned().await;
                    let ping = SshExecutor::test_connection(&n, timeout).await;
                    (idx, n, ping)
                });
            }

            let mut indexed = Vec::with_capacity(all_nodes.len());
            while let Some(res) = set.join_next().await {
                if let Ok(item) = res {
                    indexed.push(item);
                }
            }
            indexed.sort_by_key(|(idx, _, _)| *idx);

            for (_, n, ping_res) in indexed {
                let status_badge = match ping_res {
                    Ok(latency) => format!("● ONLINE ({}ms)", latency),
                    Err(e) => format!("× UNREACHABLE ({})", e),
                };
                output.push_str(&format!(
                    "• [{}] {} -> {} | Tags: [{}] | {}\n",
                    n.id,
                    n.display_name(),
                    n.target_string(),
                    n.tags.join(", "),
                    status_badge
                ));
            }
        } else {
            for n in all_nodes {
                output.push_str(&format!(
                    "• [{}] {} -> {} | Tags: [{}]\n",
                    n.id,
                    n.display_name(),
                    n.target_string(),
                    n.tags.join(", ")
                ));
            }
        }

        if !config.clusters.is_empty() {
            output.push_str("\nConfigured Clusters:\n");
            let mut cluster_keys: Vec<&String> = config.clusters.keys().collect();
            cluster_keys.sort();
            for key in cluster_keys {
                if let Some(cluster) = config.clusters.get(key) {
                    let member_ids: Vec<&str> = cluster.nodes.iter().map(|n| n.id.as_str()).collect();
                    output.push_str(&format!(
                        "• cluster:{} ({}) -> Members: [{}]\n",
                        key,
                        cluster.name,
                        member_ids.join(", ")
                    ));
                }
            }
        }

        Ok(ToolOutput::success(output))
    }
}

/// Tool to execute a shell command on a single remote VPS node.
pub struct FleetRunCommandTool;

#[async_trait]
impl Tool for FleetRunCommandTool {
    fn name(&self) -> &str {
        "fleet_run_command"
    }

    fn description(&self) -> &str {
        "Execute a shell command on a specific remote server (VPS/Node) via SSH."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute remotely."
                },
                "node_id": {
                    "type": "string",
                    "description": "The unique ID or alias of the remote node. Optional if a server is currently connected in Corex."
                },
                "sudo": {
                    "type": "boolean",
                    "description": "Whether to execute with root privileges (sudo)."
                }
            }
        })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        if context.yolo_mode {
            return false;
        }

        if args.get("sudo").and_then(|v| v.as_bool()).unwrap_or(false) {
            return true;
        }

        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim();
        let config = FleetConfig::load_fast(Some(&context.workspace_dir));

        let resolved_node = args
            .get("node_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|id| config.find_node(id))
            .or_else(|| config.default_or_single_node());

        if let Some(node) = resolved_node {
            if node.sudo {
                return true;
            }
        }

        let risk = assess_command_risk(command, &config.safe_commands, &config.dangerous_commands);

        if context
            .allowed_commands
            .iter()
            .any(|c| c == self.name() || c == command)
            && risk != CommandRiskLevel::Dangerous
        {
            return false;
        }

        risk != CommandRiskLevel::Safe
    }

    fn format_diff(&self, args: &serde_json::Value, _workspace: &Path) -> Option<String> {
        let node_id = args.get("node_id").and_then(|v| v.as_str()).unwrap_or("active-server");
        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let sudo = args.get("sudo").and_then(|v| v.as_bool()).unwrap_or(false);

        let sudo_str = if sudo { " [sudo]" } else { "" };
        Some(format!("Target Node: {}{}\n$ {}", node_id, sudo_str, command))
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(cmd) if !cmd.trim().is_empty() => cmd.trim(),
            _ => bail!("Missing required parameter 'command'"),
        };

        let config = FleetConfig::load(Some(&context.workspace_dir))?;
        let all_nodes = config.all_nodes();

        let node_id = match args.get("node_id").and_then(|v| v.as_str()) {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => {
                if let Some(active) = crate::session::global_session_manager().active_node().await {
                    active
                } else if all_nodes.len() == 1 {
                    all_nodes[0].id.clone()
                } else if all_nodes.is_empty() {
                    bail!("No remote nodes configured in .corex/fleet.toml or ~/.ssh/config.");
                } else {
                    bail!(
                        "No remote server is currently connected or active. Please specify 'node_id' from available servers: [{}] or select/connect to a server in the Fleet panel.",
                        all_nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>().join(", ")
                    );
                }
            }
        };

        let sudo = args.get("sudo").and_then(|v| v.as_bool()).unwrap_or(false);

        let node = match config.find_node(&node_id) {
            Some(n) => n,
            None => bail!("Remote node '{}' not found in fleet inventory. Use 'fleet_list_nodes' to inspect.", node_id),
        };

        let mut session = match crate::session::global_session_manager().get_session(&node_id).await {
            Some(s) if s.is_connected => s,
            _ => {
                let mut s = crate::session::RemoteSession::new(node.clone());
                match s.connect(config.timeout_secs.min(6)).await {
                    Ok(_) => s,
                    Err(e) => {
                        bail!("Failed to connect to server '{}' ({}@{}): {}. Host is unreachable or down. Please choose an alternate server (e.g. 'triade-ipv4').", node.id, node.user, node.host, e);
                    }
                }
            }
        };

        let record = session.execute_command(command, crate::session::CommandInitiator::AI, sudo, config.timeout_secs).await?;
        crate::session::global_session_manager()
            .update_session_after_command(&node.id, node, record.cwd_after.clone(), record.clone())
            .await;

        let mut res_text = format!(
            "[{}] Exit code: {} (took {}ms) | CWD: {}\n",
            node.id, record.exit_code, record.duration_ms, record.cwd_after
        );

        if !record.stdout.is_empty() {
            res_text.push_str("--- STDOUT ---\n");
            res_text.push_str(&record.stdout);
            if !res_text.ends_with('\n') {
                res_text.push('\n');
            }
        }

        if !record.stderr.is_empty() {
            res_text.push_str("--- STDERR ---\n");
            res_text.push_str(&record.stderr);
            if !res_text.ends_with('\n') {
                res_text.push('\n');
            }
        }

        if record.success {
            Ok(ToolOutput::success(res_text))
        } else {
            Ok(ToolOutput::error(res_text))
        }
    }
}

/// Tool to broadcast a command concurrently across multiple remote nodes.
pub struct FleetBroadcastTool;

#[async_trait]
impl Tool for FleetBroadcastTool {
    fn name(&self) -> &str {
        "fleet_broadcast_command"
    }

    fn description(&self) -> &str {
        "Broadcast and execute a shell command concurrently across multiple remote servers (by tag, cluster, or 'all')."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["targets", "command"],
            "properties": {
                "targets": {
                    "type": "string",
                    "description": "Target selector: 'all', 'cluster:<name>', 'tag:<name>', or comma-separated node IDs (e.g. 'web-01,web-02')."
                },
                "command": {
                    "type": "string",
                    "description": "The shell command to execute across all targeted nodes."
                },
                "sudo": {
                    "type": "boolean",
                    "description": "Whether to execute with root privileges (sudo)."
                }
            }
        })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        if context.yolo_mode {
            return false;
        }

        if args.get("sudo").and_then(|v| v.as_bool()).unwrap_or(false) {
            return true;
        }

        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim();
        let targets_str = args.get("targets").and_then(|v| v.as_str()).unwrap_or("all");
        let config = FleetConfig::load_fast(Some(&context.workspace_dir));

        let target_nodes = resolve_target_nodes(&config, targets_str);
        if target_nodes.iter().any(|n| n.sudo) {
            return true;
        }

        let risk = assess_command_risk(command, &config.safe_commands, &config.dangerous_commands);

        if context
            .allowed_commands
            .iter()
            .any(|c| c == self.name() || c == command)
            && risk != CommandRiskLevel::Dangerous
        {
            return false;
        }

        risk != CommandRiskLevel::Safe
    }

    fn format_diff(&self, args: &serde_json::Value, _workspace: &Path) -> Option<String> {
        let targets = args.get("targets").and_then(|v| v.as_str()).unwrap_or("all");
        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let sudo = args.get("sudo").and_then(|v| v.as_bool()).unwrap_or(false);

        let sudo_str = if sudo { " [sudo]" } else { "" };
        Some(format!("BROADCAST to [{}]{}\n$ {}", targets, sudo_str, command))
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let targets_str = match args.get("targets").and_then(|v| v.as_str()) {
            Some(t) => t.trim(),
            None => bail!("Missing required parameter 'targets'"),
        };

        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(cmd) => cmd,
            None => bail!("Missing required parameter 'command'"),
        };

        let sudo = args.get("sudo").and_then(|v| v.as_bool()).unwrap_or(false);

        let config = FleetConfig::load(Some(&context.workspace_dir))?;

        let target_nodes = resolve_target_nodes(&config, targets_str);

        if target_nodes.is_empty() {
            bail!("No nodes matched target selector '{}'", targets_str);
        }

        let results = SshExecutor::broadcast(&target_nodes, command, sudo, config.timeout_secs).await;

        let mut output = format!(
            "🚀 Broadcast complete on {} nodes for command: `{}`\n\n",
            results.len(),
            command
        );

        let mut all_ok = true;
        for (node_id, res) in results {
            output.push_str(&format!("─── [{}] ───\n", node_id));
            match res {
                Ok(out) => {
                    output.push_str(&format!("Status: Exit code {} ({}ms)\n", out.exit_code, out.duration_ms));
                    if !out.stdout.is_empty() {
                        output.push_str(&out.stdout);
                        if !output.ends_with('\n') {
                            output.push('\n');
                        }
                    }
                    if !out.stderr.is_empty() {
                        output.push_str(&format!("[STDERR] {}", out.stderr));
                        if !output.ends_with('\n') {
                            output.push('\n');
                        }
                    }
                    if !out.success {
                        all_ok = false;
                    }
                }
                Err(e) => {
                    output.push_str(&format!("ERROR: {}\n", e));
                    all_ok = false;
                }
            }
            output.push('\n');
        }

        if all_ok {
            Ok(ToolOutput::success(output))
        } else {
            Ok(ToolOutput::error(output))
        }
    }
}

/// Tool to fetch real-time health telemetry across nodes.
pub struct FleetTelemetryTool;

#[async_trait]
impl Tool for FleetTelemetryTool {
    fn name(&self) -> &str {
        "fleet_get_telemetry"
    }

    fn description(&self) -> &str {
        "Fetch real-time health metrics (CPU, RAM, Disk, Uptime, OS) for one or all remote fleet nodes."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "node_id": {
                    "type": "string",
                    "description": "Optional ID of a specific node. If omitted, collects metrics from all nodes."
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let config = FleetConfig::load(Some(&context.workspace_dir))?;

        let nodes_to_check: Vec<&NodeConfig> = match args.get("node_id").and_then(|v| v.as_str()) {
            Some(id) => match config.find_node(id) {
                Some(n) => vec![n],
                None => bail!("Remote node '{}' not found in fleet inventory.", id),
            },
            None => config.all_nodes(),
        };

        if nodes_to_check.is_empty() {
            return Ok(ToolOutput::success("No nodes configured in fleet inventory."));
        }

        let semaphore = Arc::new(Semaphore::new(16));
        let mut set = JoinSet::new();
        for (idx, node) in nodes_to_check.iter().enumerate() {
            let n = (*node).clone();
            let timeout = config.timeout_secs;
            let sem = semaphore.clone();
            set.spawn(async move {
                let _permit = sem.acquire_owned().await;
                let t = TelemetryCollector::fetch(&n, timeout).await;
                (idx, t)
            });
        }

        let mut indexed = Vec::with_capacity(nodes_to_check.len());
        while let Some(res) = set.join_next().await {
            if let Ok(item) = res {
                indexed.push(item);
            }
        }
        indexed.sort_by_key(|(idx, _)| *idx);

        let mut output = String::new();
        output.push_str("📊 Fleet Health & Telemetry Report:\n\n");

        for (_, telemetry) in indexed {
            if telemetry.is_online {
                let mem_str = match (telemetry.memory_used_mb, telemetry.memory_total_mb) {
                    (Some(u), Some(t)) => format!("{} / {} MB ({}%)", u, t, telemetry.memory_percent().unwrap_or(0)),
                    _ => "N/A".to_string(),
                };
                let disk_str = telemetry.disk_usage_pct.map(|d| format!("{}%", d)).unwrap_or_else(|| "N/A".to_string());
                let latency_str = telemetry.latency_ms.map(|l| format!("{}ms", l)).unwrap_or_else(|| "N/A".to_string());
                let cores_str = telemetry
                    .cpu_cores
                    .map(|c| format!(" | CPU: {} cores", c))
                    .unwrap_or_default();

                output.push_str(&format!(
                    "🟢 [{}] {} | Latency: {}\n   OS: {} | Uptime: {}{}\n   RAM: {} | Root Disk: {} | Load: {}\n\n",
                    telemetry.node_id,
                    telemetry.hostname.as_deref().unwrap_or(""),
                    latency_str,
                    telemetry.os_name.as_deref().unwrap_or("Unknown"),
                    telemetry.uptime_str.as_deref().unwrap_or("N/A"),
                    cores_str,
                    mem_str,
                    disk_str,
                    telemetry.load_avg.as_deref().unwrap_or("N/A")
                ));
            } else {
                output.push_str(&format!(
                    "🔴 [{}] OFFLINE / UNREACHABLE: {}\n\n",
                    telemetry.node_id,
                    telemetry.error_message.as_deref().unwrap_or("Unknown error")
                ));
            }
        }

        Ok(ToolOutput::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::config::ClusterConfig;

    fn make_temp_workspace(name: &str, cfg: &FleetConfig) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corex_fleet_tools_test_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        cfg.save(Some(&dir)).unwrap();
        dir
    }

    #[test]
    fn test_run_and_broadcast_needs_confirmation_sudo_and_allowed() {
        let mut cfg = FleetConfig::default();
        let mut normal_node = NodeConfig::new("web-1", "10.0.0.1");
        normal_node.sudo = false;
        let mut sudo_node = NodeConfig::new("root-1", "10.0.0.2");
        sudo_node.sudo = true;
        cfg.nodes.push(normal_node);
        cfg.nodes.push(sudo_node);

        let ws = make_temp_workspace("confirm", &cfg);
        let run_tool = FleetRunCommandTool;
        let bcast_tool = FleetBroadcastTool;

        let base_ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        // Safe command on non-sudo node -> no confirmation
        assert!(!run_tool.needs_confirmation(
            &json!({"node_id": "web-1", "command": "uptime"}),
            &base_ctx
        ));

        // Safe command with sudo=true arg -> requires confirmation
        assert!(run_tool.needs_confirmation(
            &json!({"node_id": "web-1", "command": "uptime", "sudo": true}),
            &base_ctx
        ));

        // Safe command on node with node.sudo=true -> requires confirmation
        assert!(run_tool.needs_confirmation(
            &json!({"node_id": "root-1", "command": "uptime"}),
            &base_ctx
        ));

        // Broadcast to all includes root-1 (node.sudo=true) -> requires confirmation
        assert!(bcast_tool.needs_confirmation(
            &json!({"targets": "all", "command": "uptime"}),
            &base_ctx
        ));

        // Broadcast only to web-1 with safe command -> no confirmation
        assert!(!bcast_tool.needs_confirmation(
            &json!({"targets": "web-1", "command": "uptime"}),
            &base_ctx
        ));

        // Allowed command bypasses confirmation for Mutating, but NOT Dangerous
        let allowed_ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: vec!["systemctl restart nginx".to_string()],
        };
        assert!(!run_tool.needs_confirmation(
            &json!({"node_id": "web-1", "command": "systemctl restart nginx"}),
            &allowed_ctx
        ));
        assert!(run_tool.needs_confirmation(
            &json!({"node_id": "web-1", "command": "rm -rf /var/www"}),
            &allowed_ctx
        ));

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn test_list_nodes_displays_clusters_and_deterministic_order() {
        let mut cfg = FleetConfig::default();
        cfg.nodes.push(NodeConfig::new("standalone-1", "10.0.0.1"));
        let mut prod = ClusterConfig::new("production", "Prod Cluster");
        prod.nodes.push(NodeConfig::new("prod-web-1", "10.0.1.1"));
        prod.nodes.push(NodeConfig::new("prod-db-1", "10.0.1.2"));
        cfg.clusters.insert("production".to_string(), prod);

        let ws = make_temp_workspace("list_clusters", &cfg);
        let ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let out = FleetListNodesTool
            .execute(json!({"check_status": false}), &ctx)
            .await
            .unwrap();
        assert!(out.output.contains("cluster:production (Prod Cluster) -> Members: [prod-web-1, prod-db-1]"));

        let _ = std::fs::remove_dir_all(&ws);
    }
}
