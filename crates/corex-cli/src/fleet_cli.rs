use std::path::Path;
use std::sync::Arc;
use anyhow::{bail, Result};
use colored::Colorize;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use corex_fleet::{
    assess_command_risk, CommandRiskLevel, FleetConfig, SshExecutor, TelemetryCollector,
};

pub async fn handle_fleet_cli(workspace: &Path, args: &[String]) -> Result<()> {
    let sub = args.get(0).map(|s| s.as_str()).unwrap_or("status");

    match sub {
        "status" | "list" | "ls" => show_status(workspace, true).await,
        "nodes" => show_status(workspace, false).await,
        "ping" => ping_all(workspace).await,
        "top" | "telemetry" | "metrics" => show_telemetry(workspace).await,
        "init" => init_fleet_config(workspace).await,
        "run" => {
            let mut sudo = false;
            let mut rest = &args[1..];
            while let Some(first) = rest.first() {
                if first == "--sudo" || first == "-s" {
                    sudo = true;
                    rest = &rest[1..];
                } else {
                    break;
                }
            }
            if rest.len() < 2 {
                println!("{}", "Usage: cx fleet run [--sudo] <node_id> <command...>".yellow());
                return Ok(());
            }
            let node_id = &rest[0];
            let command = rest[1..].join(" ");
            run_on_node(workspace, node_id, &command, sudo).await
        }
        "broadcast" | "all" => {
            let mut sudo = false;
            let mut rest = &args[1..];
            while let Some(first) = rest.first() {
                if first == "--sudo" || first == "-s" {
                    sudo = true;
                    rest = &rest[1..];
                } else {
                    break;
                }
            }
            if rest.len() < 2 {
                println!("{}", "Usage: cx fleet broadcast [--sudo] <targets> <command...>".yellow());
                println!("Targets can be: 'all', 'tag:<name>', 'cluster:<name>', or 'node1,node2'");
                return Ok(());
            }
            let targets = &rest[0];
            let command = rest[1..].join(" ");
            broadcast_cmd(workspace, targets, &command, sudo).await
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        _ => {
            println!("{} Unknown fleet subcommand: '{}'\n", "Error:".red().bold(), sub);
            print_help();
            Ok(())
        }
    }
}

fn print_help() {
    println!("{}", "Corex Fleet Manager — High-Performance SysAdmin Operations".cyan().bold());
    println!("{}", "===========================================================".dimmed());
    println!("Manage multiple VPS and dedicated servers concurrently.\n");
    println!("{}", "Commands:".bold());
    println!("  {}     List servers and test SSH latency", "cx fleet status".green());
    println!("  {}      List configured servers without pinging", "cx fleet nodes".green());
    println!("  {}       Measure SSH handshake round-trip latency", "cx fleet ping".green());
    println!("  {}  Query real-time CPU, RAM, Disk and Uptime", "cx fleet telemetry".green());
    println!("  {}  Execute command on a single server", "cx fleet run [--sudo] <node> <cmd>".green());
    println!("  {}  Execute concurrently across servers", "cx fleet broadcast [--sudo] <targets> <cmd>".green());
    println!("  {}       Generate a sample .corex/fleet.toml in this project", "cx fleet init".green());
    println!();
    println!("{}", "Configuration:".bold());
    println!("  Project: .corex/fleet.toml");
    println!("  Global:  ~/.config/corex/fleet.toml or automatically from ~/.ssh/config\n");
}

async fn show_status(workspace: &Path, check_online: bool) -> Result<()> {
    let config = FleetConfig::load(Some(workspace))?;
    let all_nodes = config.all_nodes();

    if all_nodes.is_empty() {
        println!("{}", "No servers found in fleet inventory.".yellow());
        println!("Run '{}' to create a template or configure hosts in '~/.ssh/config'.", "cx fleet init".cyan());
        return Ok(());
    }

    println!("\n{} ({} nodes registered):", "🌐 Corex Fleet Inventory".cyan().bold(), all_nodes.len());
    println!("{}", "─────────────────────────────────────────────────────────────────────────────".dimmed());
    println!("{:<14} {:<24} {:<16} {:<12}", "NODE ID".bold(), "HOST / IP".bold(), "TAGS".bold(), "STATUS".bold());
    println!("{}", "─────────────────────────────────────────────────────────────────────────────".dimmed());

    if check_online {
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

        for (_, node, ping_res) in indexed {
            let status_badge = match ping_res {
                Ok(latency) => format!("{} ({}ms)", "🟢 ONLINE".green().bold(), latency),
                Err(_) => "🔴 OFFLINE".red().bold().to_string(),
            };
            let tags_str = if node.tags.is_empty() { "-".to_string() } else { node.tags.join(",") };
            println!(
                "{:<14} {:<24} {:<16} {}",
                node.id.cyan(),
                node.target_string(),
                tags_str.dimmed(),
                status_badge
            );
        }
    } else {
        for node in all_nodes {
            let tags_str = if node.tags.is_empty() { "-".to_string() } else { node.tags.join(",") };
            println!(
                "{:<14} {:<24} {:<16} {}",
                node.id.cyan(),
                node.target_string(),
                tags_str.dimmed(),
                "⚪ UNCHECKED".dimmed()
            );
        }
    }

    println!("{}\n", "─────────────────────────────────────────────────────────────────────────────".dimmed());
    Ok(())
}

async fn ping_all(workspace: &Path) -> Result<()> {
    show_status(workspace, true).await
}

async fn show_telemetry(workspace: &Path) -> Result<()> {
    let config = FleetConfig::load(Some(workspace))?;
    let all_nodes = config.all_nodes();

    if all_nodes.is_empty() {
        println!("{}", "No servers found in fleet inventory.".yellow());
        return Ok(());
    }

    println!("\n{} ({} nodes):", "📊 Live Fleet Telemetry & Health".cyan().bold(), all_nodes.len());
    println!("{}", "─────────────────────────────────────────────────────────────────────────────".dimmed());

    let semaphore = Arc::new(Semaphore::new(16));
    let mut set = JoinSet::new();
    for (idx, node) in all_nodes.iter().enumerate() {
        let n = (*node).clone();
        let timeout = config.timeout_secs;
        let sem = semaphore.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            let t = TelemetryCollector::fetch(&n, timeout).await;
            (idx, t)
        });
    }

    let mut indexed = Vec::with_capacity(all_nodes.len());
    while let Some(res) = set.join_next().await {
        if let Ok(item) = res {
            indexed.push(item);
        }
    }
    indexed.sort_by_key(|(idx, _)| *idx);

    for (_, telemetry) in indexed {
        if telemetry.is_online {
            let mem_str = match (telemetry.memory_used_mb, telemetry.memory_total_mb) {
                (Some(u), Some(t)) => format!("{} / {} MB ({}%)", u, t, telemetry.memory_percent().unwrap_or(0)),
                _ => "N/A".to_string(),
            };
            let disk_str = telemetry.disk_usage_pct.map(|d| format!("{}%", d)).unwrap_or_else(|| "N/A".to_string());
            let latency_str = telemetry.latency_ms.map(|l| format!("{}ms", l)).unwrap_or_else(|| "N/A".to_string());
            let cores_str = telemetry.cpu_cores.map(|c| format!(" · CPU: {} cores", c)).unwrap_or_default();

            let hostname = corex_core::sanitize_for_terminal(telemetry.hostname.as_deref().unwrap_or(""));
            let os_name = corex_core::sanitize_for_terminal(telemetry.os_name.as_deref().unwrap_or("Linux"));
            let uptime_str = corex_core::sanitize_for_terminal(telemetry.uptime_str.as_deref().unwrap_or("N/A"));
            let load_avg = corex_core::sanitize_for_terminal(telemetry.load_avg.as_deref().unwrap_or("N/A"));

            println!("{} [{}] {} (Latency: {})", "●".green(), telemetry.node_id.cyan().bold(), hostname, latency_str.dimmed());
            println!("  OS:      {}{}", os_name.bright_white(), cores_str);
            println!("  Uptime:  {}", uptime_str);
            println!("  RAM:     {} · Root Disk: {} · Load: {}", mem_str.yellow(), disk_str.yellow(), load_avg);
        } else {
            let err_msg = corex_core::sanitize_for_terminal(telemetry.error_message.as_deref().unwrap_or("Unreachable"));
            println!("{} [{}] OFFLINE: {}", "●".red(), telemetry.node_id.red().bold(), err_msg.dimmed());
        }
        println!("{}", "  ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄".dimmed());
    }

    println!();
    Ok(())
}

async fn run_on_node(workspace: &Path, node_id: &str, command: &str, sudo: bool) -> Result<()> {
    let config = FleetConfig::load(Some(workspace))?;
    let node = match config.find_node(node_id) {
        Some(n) => n,
        None => bail!("Node '{}' not found in fleet inventory. Use 'cx fleet nodes' to list.", node_id),
    };

    let risk = assess_command_risk(command, &config.safe_commands, &config.dangerous_commands);
    let effective_sudo = sudo || node.sudo;
    if risk != CommandRiskLevel::Safe || effective_sudo {
        let sudo_label = if effective_sudo { " [sudo]" } else { "" };
        println!("\n{} Remote action on [{}]{} requires approval:", risk.badge(), node.id.cyan(), sudo_label.yellow());
        println!("  Command: {}\n", corex_core::sanitize_for_terminal(command).bold());
        print!("Execute on server? [y/N]: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("{}", "Execution cancelled by user.".yellow());
            return Ok(());
        }
    }

    println!("\n{} [{}] $ {}", "› Executing on".cyan(), node.id, corex_core::sanitize_for_terminal(command));
    let out = SshExecutor::execute(node, command, sudo, config.timeout_secs).await?;

    if !out.stdout.is_empty() {
        print!("{}", corex_core::sanitize_for_terminal(&out.stdout));
    }
    if !out.stderr.is_empty() {
        eprint!("{}", corex_core::sanitize_for_terminal(&out.stderr).red());
    }

    println!("\n[Exit code: {} · Took {}ms]", out.exit_code, out.duration_ms);
    Ok(())
}

async fn broadcast_cmd(workspace: &Path, targets: &str, command: &str, sudo: bool) -> Result<()> {
    let config = FleetConfig::load(Some(workspace))?;

    let target_nodes: Vec<&corex_fleet::NodeConfig> = if targets.eq_ignore_ascii_case("all") {
        config.all_nodes()
    } else if let Some(tag) = targets.strip_prefix("tag:") {
        config.nodes_by_tag(tag.trim())
    } else if let Some(cluster_id) = targets.strip_prefix("cluster:") {
        config.clusters.get(cluster_id.trim())
            .map(|c| c.nodes.iter().collect())
            .unwrap_or_default()
    } else {
        targets.split(',').filter_map(|id| config.find_node(id.trim())).collect()
    };

    if target_nodes.is_empty() {
        bail!("No nodes matched target selector '{}'", targets);
    }

    let risk = assess_command_risk(command, &config.safe_commands, &config.dangerous_commands);
    let any_sudo = sudo || target_nodes.iter().any(|n| n.sudo);
    if risk != CommandRiskLevel::Safe || any_sudo {
        let sudo_label = if any_sudo { " [sudo]" } else { "" };
        println!("\n{} Broadcast to {} nodes{} requires approval:", risk.badge(), target_nodes.len(), sudo_label.yellow());
        println!("  Targets: {}", target_nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>().join(", ").cyan());
        println!("  Command: {}\n", corex_core::sanitize_for_terminal(command).bold());
        print!("Execute on ALL servers? [y/N]: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("{}", "Execution cancelled by user.".yellow());
            return Ok(());
        }
    }

    println!("\n{} {} nodes concurrently...", "🚀 Broadcasting to".cyan().bold(), target_nodes.len());
    let results = SshExecutor::broadcast(&target_nodes, command, sudo, config.timeout_secs).await;

    for (node_id, res) in results {
        println!("\n{} [{}]", "─── Result for".bold(), node_id.cyan());
        match res {
            Ok(out) => {
                if !out.stdout.is_empty() {
                    print!("{}", corex_core::sanitize_for_terminal(&out.stdout));
                }
                if !out.stderr.is_empty() {
                    eprint!("{}", corex_core::sanitize_for_terminal(&out.stderr).red());
                }
                println!("[Exit code: {} · Took {}ms]", out.exit_code, out.duration_ms);
            }
            Err(e) => {
                println!("{} {}", "ERROR:".red(), e);
            }
        }
    }

    Ok(())
}

async fn init_fleet_config(workspace: &Path) -> Result<()> {
    let dir = workspace.join(".corex");
    let target = dir.join("fleet.toml");
    if target.exists() {
        println!("{} Config already exists at: {}", "Notice:".yellow(), target.display());
        return Ok(());
    }

    corex_core::ensure_private_dir(&dir)?;
    let template = include_str!("../../../.corex/fleet.example.toml");
    corex_core::write_private_atomic(&target, template)?;
    println!("{} Created template configuration at: {}", "✓ Success:".green().bold(), target.display().to_string().cyan());
    println!("Edit this file to add your VPS IP addresses, SSH keys, and clusters.");
    Ok(())
}
