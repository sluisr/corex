---
name: corex-fleet
description: >-
  Use this skill when the user asks about managing remote servers, VPS, clusters,
  SysAdmin mode, /fleet, /sysadmin, /vps, running commands across multiple machines,
  checking remote server RAM/CPU/Disk metrics, or automating infrastructure.
  Trigger phrases: 'modo servidor', 'cambia a modo servidor', 'fleet', 'sysadmin',
  'vps', 'servidores', 'administrar servidores', 'cluster', 'remote command',
  and Fleet keyboard shortcuts ('atajos de fleet', 'how do I hide the terminal').
---

# Corex Fleet & SysAdmin Operations

Corex includes a high-performance multi-server orchestrator (`corex-fleet`) to connect to remote VPS and bare-metal nodes via SSH.

## Available Remote Fleet Tools:
All four are actions of the single `fleet` tool (`action` argument).

1. `list_nodes`: Discovers and lists all configured servers in `.corex/fleet.toml` and `~/.ssh/config`. Supports `check_status: true` for live latency ping.
2. `telemetry`: Fetches real-time CPU, RAM, Disk, Uptime, OS, and Load Average for one or all nodes.
3. `run_command`: Executes a command on a specific remote server (e.g. `node_id: "prod-node-1"`, `command: "docker ps"`).
4. `broadcast_command`: Concurrently runs a command across all or tagged servers (e.g. `targets: "all"` or `targets: "tag:web"`).

## Safety & Approval Guardrails:
- **Read-Only / Safe (`uptime`, `free -m`, `df -h`, `docker ps`, `cat`, `ls`)**: Run automatically.
- **Mutating (`apt update`, `systemctl restart`, `git pull`)**: Prompt the user for approval.
- **Dangerous (`rm -rf`, `reboot`, `shutdown`, `iptables -F`)**: Strict high-risk confirmation.

## How to interact:
When the user asks to switch to "modo servidor", check servers, or inspect remote infrastructure:
- Immediately call `fleet` with `telemetry` or `list_nodes`.
- Report server health clearly with latency, CPU, and RAM metrics.
- Ask the user what actions or deployments they want to perform on those servers.

## Split view keyboard (`/fleet`)
The chat sits on the left and the live SSH terminal on the right (full screen one at a time below 110 columns).

|Key | Action |
|:--- | :--- |
|`F6` (or `Ctrl+]`) | Swap focus between chat and terminal (also shows a hidden terminal) |
|`F7`, `/fleet hide` (or `Ctrl+\`) | Hide / show the terminal; the chat fills the screen and the SSH session stays alive |
|`Alt+1..9`, `F1`–`F9` (not F2/F6/F7) | Pick a server |
|`Alt+S` | Open the server selector |
|`Ctrl+R` (chat focus) or `/fleet probe` | Refresh telemetry for every server |
|`Shift+Tab` | Chat: cycle modes, as outside Fleet. Terminal: sent to the shell |
|`Tab` | Chat: autocomplete. Terminal: shell completion |
|`PageUp / PageDown`, wheel | Scroll the terminal's scrollback |
|`exit` in the terminal | Disconnect from the server |
|`/fleet off` | Leave Fleet (closes the SSH session) |

With the terminal focused every other key, including `Ctrl+R`, `Ctrl+W`, `Alt+←/→` and `Ctrl+C`, goes to the remote shell. In the server selector: `↑↓` or `1-9` select, `Enter` connects, typing filters, `Ctrl+S` switches the source (all · corex · ssh), `Esc` clears the filter or returns to the chat.

Subcommands: `/fleet list`, `/fleet connect <node>`, `/fleet wide` (3 columns: chat · terminal · telemetry), `/fleet hide`, `/fleet probe`, `/fleet resume`, `/fleet off`.
