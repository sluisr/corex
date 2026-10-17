---
name: corex-fleet
description: >-
  Use this skill when the user asks about managing remote servers, VPS, clusters,
  SysAdmin mode, /fleet, /sysadmin, /vps, running commands across multiple machines,
  checking remote server RAM/CPU/Disk metrics, or automating infrastructure.
  Trigger phrases: 'modo servidor', 'cambia a modo servidor', 'fleet', 'sysadmin',
  'vps', 'servidores', 'administrar servidores', 'cluster', 'remote command'.
---

# Corex Fleet & SysAdmin Operations

Corex includes a high-performance multi-server orchestrator (`corex-fleet`) to connect to remote VPS and bare-metal nodes via SSH.

## Available Remote Fleet Tools:
1. `fleet_list_nodes`: Discovers and lists all configured servers in `.corex/fleet.toml` and `~/.ssh/config`. Supports `check_status: true` for live latency ping.
2. `fleet_get_telemetry`: Fetches real-time CPU, RAM, Disk, Uptime, OS, and Load Average for one or all nodes.
3. `fleet_run_command`: Executes a command on a specific remote server (e.g. `node_id: "triade-ipv4"`, `command: "docker ps"`).
4. `fleet_broadcast_command`: Concurrently runs a command across all or tagged servers (e.g. `targets: "all"` or `targets: "tag:web"`).

## Safety & Approval Guardrails:
- **Read-Only / Safe (`uptime`, `free -m`, `df -h`, `docker ps`, `cat`, `ls`)**: Run automatically.
- **Mutating (`apt update`, `systemctl restart`, `git pull`)**: Prompt the user for approval.
- **Dangerous (`rm -rf`, `reboot`, `shutdown`, `iptables -F`)**: Strict high-risk confirmation.

## How to interact:
When the user asks to switch to "modo servidor", check servers, or inspect remote infrastructure:
- Immediately query `fleet_get_telemetry` or `fleet_list_nodes`.
- Report server health clearly with latency, CPU, and RAM metrics.
- Ask the user what actions or deployments they want to perform on those servers.
