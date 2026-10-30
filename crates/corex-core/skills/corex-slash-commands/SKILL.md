---
name: corex-slash-commands
description: >-
  Use this skill when the user asks about Corex slash commands, what commands
  are available inside the TUI, how to use a specific command like /model,
  /chat, /resume, /stats, /yolo, /fim, /web, /local, /plan, /compress,
  /compact, /rewind, /prefix, /tasks, /mcp, /balance, /wallet,
  /save, /update, /info, /clear, /help, /quit. Also activate for questions
  like 'what can I type with /', 'list commands', 'how do I switch model',
  'how do I check balance', 'how do I resume a session', 'how to compress context'.
---

# Corex Slash Commands Reference

Inside the interactive TUI (`cx`), type `/` to trigger the command autocomplete popup. Use arrow keys to navigate and `Enter` to select.

---

## 💬 Session Management

| Command | Description |
| :--- | :--- |
| `/chat list` | List all saved sessions for this project |
| `/chat save <tag>` | Save current conversation with a tag |
| `/chat resume <tag\|id>` | Resume a previously saved session |
| `/chat new` | Start a fresh conversation |
| `/resume [tag\|id]` | Shortcut to resume the last or a specific session |
| `/save <tag>` | Save a checkpoint of the current conversation |
| `/rewind` | Undo the last turn (removes last user + assistant message) |

**Resume from CLI (outside TUI):**
```bash
cx --resume <index|tag|id>     # Resume by session index, tag, or UUID
cx --list-sessions             # List all available sessions
cx -l                          # Short alias for --list-sessions
```

---

## 🧠 Model, Auth & Mode Control

| Command | Description |
| :--- | :--- |
| `/model` | Open the interactive model selector (Cloud / Local across 15 providers) |
| `/key [api_key]` | Configure or update API key for the active provider (saved to `~/.corex/settings.json`) |
| `/auth [api_key]` | Alias for `/key` |

Available flagship DeepSeek models inside `/model`:
- **`deepseek-flash`** — DeepSeek V4.1 Flash (522B MoE, 1M context, native vision, default)
- **`deepseek-v4.1-flash-1017`** — DeepSeek V4.1 Flash-1017 (wish-driven programming, faster task completion)
- **`deepseek-v4-pro`** — DeepSeek V4 Pro (deep CoT reasoning / Thinking mode)

Switch model from CLI:
```bash
cx -m deepseek-v4-pro -p "Refactor this function..."
cx --model deepseek-v4.1-flash-1017
cx --model deepseek-flash
```

---

## 🖥️ Fleet & Remote SSH Control Plane

| Command | Description |
| :--- | :--- |
| `/fleet` | Toggle the interactive SSH Fleet & VPS terminal pane (aliases: `/sysadmin`, `/vps`) |
| `/fleet wide` | Open the 3-column wide Fleet layout (Nodes + Output/PTY + Chat) |
| `/fleet list` | List all configured SSH fleet servers |
| `/fleet connect <id>` | Connect directly to a fleet server and open an interactive SSH PTY |

Servers are added by editing `fleet.toml` (`cxs init` writes a template) or `~/.ssh/config`.

You can also launch directly into Fleet mode from the CLI via `cxs` (or `cx fleet`).

---

## ⚡ Context & Memory

| Command | Description |
| :--- | :--- |
| `/compress` | Manually compress conversation history to save tokens |
| `/compact` | Intelligently compact history into a structured memory block |

Corex also auto-compacts at **80% of the model's context window** by default (configurable via `compact_threshold_tokens` in `~/.corex/settings.json`).

---

## 💰 Cost & Stats

| Command | Description |
| :--- | :--- |
| `/stats` (alias `/context`) | Session token metrics, KV cache hit rate, USD cost summary, and the context in use against the auto-compact and history limits |
| `/balance` | Live DeepSeek API account balance lookup (async, non-blocking) |
| `/wallet` | Alias for `/balance` |

---

## 🌐 Web & Search

| Command | Description |
| :--- | :--- |
| `/web <query>` | Live internet search (`web_search` via DuckDuckGo HTML scraping & instant answers) |
| `/search <query>` | Alias for `/web` |
| `/fetch <url>` | Fetch web page content and convert HTML to clean Markdown (`web_fetch`) |

Also available directly from CLI (no TUI needed):
```bash
cx -w "latest Rust async news"
cx web what is the current Bitcoin price
cx --fetch "https://api-docs.deepseek.com/guides/search"
cx fetch "https://news.ycombinator.com"
```

---

## 🤖 Local LLM

| Command | Description |
| :--- | :--- |
| `/local <prompt>` | Send a prompt directly to your local LLM at $0.00 cost |
| `/local status` | Check if local LLM server is reachable |

---

## 🔌 MCP Servers

| Command | Description |
| :--- | :--- |
| `/mcp` | List configured MCP servers, status, and exposed tools |
| `/mcp catalog` | Browse vetted community templates (GitHub, SQLite, Postgres, Memory, etc.) |
| `/mcp add <id>` | Add a server from catalog (e.g. `/mcp add github`, `/mcp add sqlite app.db`) |
| `/mcp add <name> <cmd> [args]` | Add a custom MCP server executable directly |
| `/mcp remove <name>` | Remove an MCP server and reload configuration |
| `/mcp reload` | Reconnect and rediscover tools across all configured MCP servers |

---

## ⚙️ Tools & Execution

| Command | Description |
| :--- | :--- |
| `/yolo` | Toggle YOLO mode — auto-approves ALL tool executions |
| `/yolo on` | Enable YOLO mode |
| `/yolo off` | Disable YOLO mode |
| `/sudo [clear]` | Configure or clear ephemeral in-memory sudo password for privileged commands |
| `/tasks` | List all running background tasks |
| `/tasks status <pid>` | Show status of a specific background task |
| `/tasks kill <pid>` | Kill a background task |
| `/tasks send <pid> <input>` | Send stdin input to a running background task |

Enable YOLO from CLI:
```bash
cx -y -p "Run all tests and fix any failures"
cx --yolo
```

> ⚠️ YOLO mode skips all confirmation prompts. Use carefully — commands run instantly without asking.

---

## 📝 Code & Planning

| Command | Description |
| :--- | :--- |
| `/fim <file>` | Fill-in-the-Middle code autocompletion for a specific file |
| `/plan` | Toggle architectural plan mode (read-only discovery, no file writes) |
| `/prefix <text\|off>` | Force the model to start its response with exact text (or `off` to clear) |

---

## ℹ️ Info & Misc

| Command | Description |
| :--- | :--- |
| `/info` | Version, author credits, official links, and session telemetry |
| `/update` | Check for updates, or apply in-place atomically (`/update apply` / `/update force`) |
| `/mouse` | Toggle terminal mouse capture (enable/disable native terminal text selection) |
| `/keyboard` | Termux / Android: pause mouse reporting for a few seconds so the next tap raises the soft keyboard |
| `/help` | Show keyboard shortcuts and command list |
| `/clear` | Clear the terminal conversation view |
| `/quit` | Exit the session (saves state for later resumption) |
| `/exit` | Alias for `/quit` |

---

## ⌨️ Keyboard Shortcuts

| Key | Action |
| :--- | :--- |
| `Enter` | Send message / confirm (queued while the agent works) |
| `Alt+Enter` | While the agent works: deliver the message at the next safe point |
| `Esc` | Cancel the generation / close popup / snap scroll to bottom; twice within 0.5 s clears the prompt |
| `Ctrl+C` | Cancel the generation; twice within 2 s quits |
| `Ctrl+Z` | Quit immediately |
| `Ctrl+T` | Expand / collapse Chain-of-Thought reasoning blocks |
| `Ctrl+O` | Expand / collapse full tool output & diff previews |
| `Ctrl+L` | Repaint the screen |
| `F2` | Toggle mouse reporting (wheel and selection in Corex vs. the terminal) |
| `↑ / ↓` | Navigate command autocomplete / prompt history |
| `Ctrl+P / Ctrl+N`, `Alt+↑ / Alt+↓` | Previous / next prompt in history |
| `PageUp / PageDown`, `Shift+↑ / Shift+↓` | Scroll the conversation |
| `Tab` | Select highlighted autocomplete suggestion / switch Fleet focus |
