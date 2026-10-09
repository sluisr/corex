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

## 🧠 Model & Mode Control

| Command | Description |
| :--- | :--- |
| `/model` | Open the interactive model selector (Cloud / Local) |

Available models inside `/model`:
- **`deepseek-flash`** — DeepSeek V4.1 Flash (522B MoE, 1M context, native vision, default)
- **`deepseek-v4-pro`** — DeepSeek V4 Pro (deep CoT reasoning / Thinking mode)

Switch model from CLI:
```bash
cx -m deepseek-v4-pro -p "Refactor this function..."
cx --model deepseek-flash
```

---

## ⚡ Context & Memory

| Command | Description |
| :--- | :--- |
| `/compress` | Manually compress conversation history to save tokens |
| `/compact` | Intelligently compact history into a structured memory block |

Corex also auto-compacts at **95,000 tokens** by default (configurable via `compact_threshold_tokens` in `~/.corex/settings.json`).

---

## 💰 Cost & Stats

| Command | Description |
| :--- | :--- |
| `/stats` | Session token metrics, KV cache hit rate, and USD cost summary |
| `/balance` | Live DeepSeek API account balance lookup (async, non-blocking) |
| `/wallet` | Alias for `/balance` |

---

## 🌐 Web & Search

| Command | Description |
| :--- | :--- |
| `/web <query>` | Live internet search via DeepSeek native search |
| `/search <query>` | Alias for `/web` |

Also available directly from CLI (no TUI needed):
```bash
cx -w "latest Rust async news"
cx web what is the current Bitcoin price
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
| `/mcp` | List all configured MCP servers and their tools |
| `/mcp status` | Show connection status of each MCP server |
| `/mcp reload` | Reconnect all MCP servers |

---

## ⚙️ Tools & Execution

| Command | Description |
| :--- | :--- |
| `/yolo` | Toggle YOLO mode — auto-approves ALL tool executions |
| `/yolo on` | Enable YOLO mode |
| `/yolo off` | Disable YOLO mode |
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
| `/prefix <text>` | Force the model to start its response with exact text |

---

## ℹ️ Info & Misc

| Command | Description |
| :--- | :--- |
| `/info` | Version, author credits, official links, and session telemetry |
| `/update` | Check for a newer Corex version and see upgrade instructions |
| `/help` | Show keyboard shortcuts and command list |
| `/clear` | Clear the terminal conversation view |
| `/quit` | Exit the session (saves state for later resumption) |
| `/exit` | Alias for `/quit` |

---

## ⌨️ Keyboard Shortcuts

| Key | Action |
| :--- | :--- |
| `Enter` | Send message / confirm |
| `Ctrl+C` | Cancel current generation |
| `Ctrl+L` | Clear screen |
| `↑ / ↓` | Navigate command autocomplete / history |
| `Tab` | Select highlighted autocomplete suggestion |
| `Esc` | Close popup / cancel |
