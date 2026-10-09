---
name: corex-mcp-setup
description: >-
  Use this skill when the user asks about MCP, Model Context Protocol,
  connecting external tools to Corex, configuring MCP servers, adding
  GitHub tools, Docker MCP, database MCP, how to set up an MCP server,
  how to list or reload MCP servers, or how to write an MCP config.
  Trigger phrases: 'mcp', 'model context protocol', 'external tools',
  'connect github', 'docker mcp', 'mcp server', 'mcp config'.
---

# Corex MCP (Model Context Protocol) Setup

MCP lets Corex connect to external tool servers — giving it capabilities beyond its built-in tools: GitHub, Docker, databases, custom APIs, and more.

---

## 📁 Configuration Location

MCP servers are configured inside **`~/.corex/settings.json`** under the `mcp_servers` key:

```json
{
  "api_key": "sk-...",
  "model": "deepseek-flash",
  "mcp_servers": {
    "my-server-name": {
      "command": "executable-or-script",
      "args": ["--arg1", "value"],
      "env": {
        "MY_API_KEY": "secret-value"
      }
    }
  }
}
```

Each entry under `mcp_servers` is a named server with:
- **`command`** — the executable to launch (e.g. `npx`, `uvx`, `python`, a binary path)
- **`args`** — list of arguments to pass
- **`env`** — optional environment variables for the server process

Corex spawns the server as a subprocess and communicates via **JSON-RPC over stdio** (MCP protocol version `2024-11-05`).

---

## 🔌 Common MCP Server Examples

### GitHub Tools
```json
"mcp_servers": {
  "github": {
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-github"],
    "env": {
      "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_yourtoken"
    }
  }
}
```

### Filesystem (extra paths)
```json
"mcp_servers": {
  "filesystem": {
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/projects"]
  }
}
```

### PostgreSQL Database
```json
"mcp_servers": {
  "postgres": {
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-postgres"],
    "env": {
      "DATABASE_URL": "postgresql://user:pass@localhost:5432/mydb"
    }
  }
}
```

### Brave Search
```json
"mcp_servers": {
  "brave-search": {
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-brave-search"],
    "env": {
      "BRAVE_API_KEY": "your-brave-key"
    }
  }
}
```

### Custom Python MCP server
```json
"mcp_servers": {
  "my-tool": {
    "command": "python",
    "args": ["/path/to/my_mcp_server.py"]
  }
}
```

---

## 🛠️ Managing MCP from Inside the TUI

| Command | What it does |
| :--- | :--- |
| `/mcp` | List all configured servers and the tools they expose |
| `/mcp status` | Show connection status for each server |
| `/mcp reload` | Disconnect and reconnect all MCP servers |

MCP tools are loaded at startup. If you edit `settings.json` to add a server, restart `cx` or run `/mcp reload`.

---

## 🏷️ How Corex Names MCP Tools

When Corex connects to a server named `github` and it exposes a tool called `create_issue`, Corex registers it internally as:

```
mcp_github_create_issue
```

Format: `mcp_<server-name>_<tool-name>`

The model can call these tools naturally — just describe what you want and Corex will use the appropriate MCP tool.

---

## ✅ Verification

After adding a server, verify it's connected:

```bash
# Inside TUI
/mcp status

# Or check the forensic log
cat ~/.corex/logs/corex-forensic-$(date +%Y-%m-%d).log | grep -i mcp
```

If a server fails to connect, Corex logs the error but continues running with just the built-in tools — it never crashes on MCP failures.

---

## 📦 Finding MCP Servers

- **Official:** [github.com/modelcontextprotocol/servers](https://github.com/modelcontextprotocol/servers)
- **Community:** [mcp.so](https://mcp.so) — MCP server registry
- Any server using the MCP stdio transport protocol is compatible with Corex.
