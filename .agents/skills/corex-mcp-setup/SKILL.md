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

## Configuration Location

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
- **`command`** — the executable to launch (e.g. `npx`, `uvx`, `python`, a binary path). Used by local servers.
- **`args`** — list of arguments to pass
- **`env`** — optional environment variables for the server process
- **`url`** — for a **remote** server instead of `command` (e.g. `https://stitch.googleapis.com/mcp`)
- **`headers`** — HTTP headers sent to a remote server, where its API key or token goes

Corex spawns a local server as a subprocess and communicates via **JSON-RPC over stdio** (protocol version `2024-11-05`); a remote one is reached over **Streamable HTTP**. The two forms are mixed freely in the same `mcp_servers` map.

---

## Remote MCP Servers (Streamable HTTP)

Not every server runs on your machine. A **remote** MCP server is reached over HTTPS instead of being spawned, and it is configured with `url` + `headers` instead of `command`:

```json
"mcp_servers": {
  "stitch": {
    "url": "https://stitch.googleapis.com/mcp",
    "headers": { "X-Goog-Api-Key": "YOUR-API-KEY" }
  }
}
```

How Corex speaks to it: one JSON-RPC message per `POST`, `Accept: application/json, text/event-stream`, a reply that is either a JSON body or an SSE stream (read until the matching response arrives), the `Mcp-Session-Id` returned by the server echoed on every later request, and the negotiated `mcp-protocol-version` (`2025-06-18` is offered). Requests time out after 60s, and a `401`/`403` reports that the credentials expired instead of a generic failure.

### Giving the server its key: two ways

**A. In the config file — permanent, no shell involved.** The value goes straight into the `headers` map of `~/.corex/settings.json` (optionally `.corex/settings.json` in the project, which is private to you). Corex reads it at start-up; nothing depends on an `export` that dies with the terminal. This is the option to offer whenever the user wants the key to *stay*:

```json
"headers": { "X-Goog-Api-Key": "AIza...your-key" }
```

That file is written `-rw-------` inside `~/.corex` (`drwx------`), and Corex strips the whole `mcp_servers` map when it writes a project-level `.corex/settings.json`, so a key set this way cannot reach a repository or a commit.

**B. From the environment — a reference instead of the secret.** A header value may be written as `${NAME}`; it is resolved when connecting, so the secret never lands in Corex's files:

```json
"headers": { "X-Goog-Api-Key": "${STITCH_API_KEY}" }
```

```bash
export STITCH_API_KEY="AIza...your-key"   # add to ~/.bashrc or ~/.zshrc to persist
```

A missing or empty variable is reported as a connection error, never sent literally.

Which to recommend: **A** for the user who wants it configured once and forgotten (it is the simplest that actually persists). **B** for machines where a secret must stay out of JSON files — but be honest about the trade-off: an environment variable is inherited by the processes Corex spawns, so a shell command that prints the environment could show it. Corex does scrub names matching `TOKEN`/`SECRET`/`API_KEY` from the environment of MCP *subprocesses*, but that does not cover shell commands. The truly ideal store — the **system keyring**, which Corex already uses for `api_key` and `provider_api_keys` — is **not** wired to MCP headers yet, so do not promise it.

After editing the file: `/mcp reload` re-reads `mcp_servers` from `~/.corex/settings.json` and reconnects, so no restart is needed for a refreshed key or a server added by hand. Then `/mcp status` shows the result.

### Google Stitch

Stitch is a remote MCP server that generates and edits UI designs (projects, screens, design systems).

1. Create an API key at [stitch.google.com](https://stitch.google.com) → Settings → **API Keys**.
2. Add the server — from the catalog, or with the URL plus headers:

```
/mcp add stitch
/mcp add stitch https://stitch.googleapis.com/mcp "X-Goog-Api-Key=${STITCH_API_KEY}"
```

The catalog entry is saved with `X-Goog-Api-Key` set to `${STITCH_API_KEY}`, so either export that variable or replace the placeholder with the key itself in `~/.corex/settings.json` (option A above). With the key in the file, no shell setup is needed at all. A process that started before the key was pasted keeps using its old copy until `/mcp reload`; saves made from that process never write the old `mcp_servers` map back to the file.

3. `/mcp reload`, then `/mcp status`: it should read `Stitch (remote): https://stitch.googleapis.com/mcp — 18 tools discovered` and list `create_project`, `generate_screen_from_text`, `edit_screens`, `generate_variants`, `create_design_system` and the rest as `mcp_stitch_*` tools. **The tool count is not proof that the key works**: `tools/list` answers even with an invalid key, so only an actual tool call confirms the credentials.

The OAuth setup (`Authorization: Bearer <access token>` + `X-Goog-User-Project`) is also supported through the same `headers` map; its tokens last about an hour, so refresh the value (file or variable) and run `/mcp reload` — it re-reads the file and re-resolves `${...}` references, so the new token applies without restarting.

---

## Common MCP Server Examples

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

## Managing MCP from Inside the TUI

|Command | What it does |
|:--- | :--- |
|`/mcp` or `/mcp status` | List all configured servers, connection status, and discovered tools |
|`/mcp catalog` | Browse the built-in catalog of vetted community templates |
|`/mcp add <catalog-id>` | Instantly add a server from catalog (e.g. `/mcp add github`, `/mcp add sqlite app.db`) |
|`/mcp add <name> <cmd> [args]` | Add a custom MCP server executable directly |
|`/mcp add <name> <url> [Header=value ...]` | Add a remote server; an `http(s)` target is taken as its URL |
|`/mcp remove <name>` | Remove an MCP server from configuration and reload tools |
|`/mcp reload` | Re-read `mcp_servers` from `~/.corex/settings.json`, reconnect and rediscover every server |

MCP servers can be added in one command via `/mcp add <id>` or configured manually in `~/.corex/settings.json`. Dynamic tool discovery registers tools immediately without needing a full restart, and `/mcp reload` picks up hand edits to that file.

---

## How Corex Names MCP Tools

When Corex connects to a server named `github` and it exposes a tool called `create_issue`, Corex registers it internally as:

```
mcp_github_create_issue
```

Format: `mcp_<server-name>_<tool-name>`

The model can call these tools naturally — just describe what you want and Corex will use the appropriate MCP tool.

---

## Verification

After adding a server, verify it's connected:

```bash
# Inside TUI
/mcp status

# Or check the forensic log
cat ~/.corex/logs/corex-forensic-$(date +%Y-%m-%d).log | grep -i mcp
```

If a server fails to connect, Corex logs the error but continues running with just the built-in tools — it never crashes on MCP failures.

---

## Finding MCP Servers

- **Official:** [github.com/modelcontextprotocol/servers](https://github.com/modelcontextprotocol/servers)
- **Community:** [mcp.so](https://mcp.so) — MCP server registry
- Any server using the MCP **stdio** or **Streamable HTTP** transport is compatible with Corex.
