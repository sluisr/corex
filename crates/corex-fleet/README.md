# Corex Fleet (`corex-fleet`)

High-performance multi-server orchestrator, SSH connection pool, and SysAdmin fleet operations engine for Corex.

## Capabilities

- **SSH Connection Multiplexing**: Ultra-low latency remote execution via persistent SSH control sockets or native connections.
- **Multi-Node Fan-Out**: Parallel execution across clusters or tags with live streaming output.
- **Security Guardrails**: Command risk assessment, safe command whitelisting, and approval prompts for mutating/dangerous actions.
- **Automatic Inventory Discovery**: Discovers hosts from `~/.ssh/config` and manages declarative fleet definitions in `.corex/fleet.toml`.
- **Node Telemetry**: Live ping, CPU, memory, uptime, and disk monitoring.
