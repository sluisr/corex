---
name: corex-config
description: >-
  Use this skill when the user asks about Corex configuration files, how to customize
  Corex personality or system prompt, where to add allowed or safe commands,
  how to configure remote servers in fleet.toml, or where configuration files
  are stored. Trigger phrases: 'config', 'configuracion', 'archivos de configuracion',
  'donde configurar', 'personalidad', 'comandos permitidos', 'safe_commands',
  'fleet.toml', 'system.md', 'corex.md', 'agents.md', 'deepseek.md', 'settings',
  'como personalizar', 'memory', 'memoria', '/memory', 'instrucciones ignoradas'.
---

# Corex Configuration & Customization Guide

Corex uses a clear, human-readable hierarchy of configuration files in **TOML** and **Markdown** (`.md`).

---

## Configuration Files Overview

|File | Scope | Format | Purpose |
|:--- | :--- | :--- | :--- |
|**`~/.corex/SYSTEM.md`** *(+ `~/.corex/COREX.md`, `~/.deepseek/DEEPSEEK.md`)* | **Global** | Markdown | **User Personality & Global Rules**: custom tone, role instructions, language, conventions applied to all projects and sessions. |
|**`.corex/SYSTEM.md`** *(+ `./COREX.md`, `./AGENTS.md`, `./DEEPSEEK.md`)* | **Project Local** | Markdown | **Project Instructions**: repository-specific architecture, build commands, and rules. |
|**`~/.corex/fleet.toml`** *(or `.corex/fleet.toml`)* | **Global or Local** | TOML | **Remote Fleet & Allowed Commands**: SSH server inventory, `safe_commands` (auto-approved commands), and `dangerous_commands`. A workspace `.corex/fleet.toml` only contributes servers: its command lists are ignored and its servers always ask before Corex reaches them. |
|**`~/.corex/settings.json`** | **Global** | JSON | **Corex Engine Settings**: active model and provider, reasoning effort, temperature, YOLO mode default, API keys, allowed commands. |
|**`~/.ssh/config`** | **System** | OpenSSH | Fallback server discovery for Fleet mode. |

Every instruction file found is loaded, global first and then project, in the order listed. Use `/memory list` to see which are in use and `/memory reload` after editing one.

---

## 1. Custom Personality & Global Rules (`~/.corex/SYSTEM.md`)

Create `~/.corex/SYSTEM.md` to customize how Corex responds and behaves across all tasks.
Corex automatically loads this file into its system prompt on every execution.

### Example `~/.corex/SYSTEM.md`:
```markdown
# Personalidad y Tono
- Responde de forma técnica, directa y concisa.
- Idioma predeterminado: Español profesional pero cercano.
- Evita introducciones largas o conclusiones redundantes.

# Reglas de SysAdmin e Infraestructura
- Prioriza diagnósticos no destructivos.
- Al revisar memoria o disco, usa flags legibles por humanos (`-h`).
- Prefiere Docker Compose v2 (`docker compose`).

# Preferencias de Código
- En Rust, prioriza código idiomático y tipado seguro.
- No agregues comentarios decorativos innecesarios.
```

### Instruction files ("memory") and `/memory`

Corex's memory is the set of instruction files you write, plus the notes saved with `/memory add` or by the model's `save_memory` tool (which always asks for your approval first). Every file found is appended to the system prompt in this order, so later (more specific) files come last:

1. `~/.corex/SYSTEM.md`
2. `~/.corex/COREX.md`
3. `~/.deepseek/DEEPSEEK.md`
4. `./.corex/SYSTEM.md`
5. `./COREX.md`
6. `./AGENTS.md`
7. `./DEEPSEEK.md`
8. `~/.corex/projects/<workspace path>/MEMORY.md` (private project notes, never inside the repository)

Project files are read from the directory where `cx` was started, not from parent directories. Each file is capped at 20,000 bytes.

|Command | Use it to |
|:--- | :--- |
|`/memory` or `/memory list` | See which files are loaded, their size, and whether one was truncated |
|`/memory show` | See the exact memory block the model receives |
|`/memory add <text>` | Save `- <text>` to `~/.corex/COREX.md` (personal, applies in every project) and reload |
|`/memory add --project <text>` | Save it to the private notes of this project (file 8), outside the repository |
|`/memory reload` (alias `refresh`) | Pick up an edit right away instead of waiting up to 15 s for the cache |

`/memory add` and `save_memory` never write into the repository: personal notes go to `~/.corex/COREX.md` and project notes to the private file, as in deepseek-cli. Rules the team should share belong in `AGENTS.md` / `COREX.md`, edited by hand and committed. Keep `~/.corex/SYSTEM.md` for what you write by hand. The files stay plain Markdown, so you can always open them to review or delete what was added.

---

## 2. Allowed & Safe Commands (`~/.corex/fleet.toml`)

In Fleet & SysAdmin mode, Corex categorizes commands into:
1. **Safe**: Run automatically without confirmation prompts.
2. **Mutating**: Require user confirmation unless in YOLO mode.
3. **Dangerous**: Require strict confirmation.

You can customize which commands are auto-approved by adding them to `safe_commands` in your global `~/.corex/fleet.toml` (or `~/.config/corex/fleet.toml`). A workspace `.corex/fleet.toml` cannot change these lists, because it comes with the repository: servers it defines ask for confirmation on every command, even safe ones, until you approve it for the session (`s`) or permanently (`p`).

### Example `~/.corex/fleet.toml`:
```toml
# General settings
timeout_secs = 10
auto_sync_cwd = true

# Whitelist of commands that will execute automatically without confirmation:
safe_commands = [
    "uptime",
    "free",
    "df",
    "ps",
    "docker ps",
    "systemctl status",
    "systemctl --failed",
    "journalctl",
    "ss",
    "netstat",
    "lscpu",
    "lsblk",
    "hostnamectl",
    "echo",
    "nginx -t",
    "git status",
    "git branch",
    "git log",
]

# Blacklist of high-risk commands requiring explicit confirmation:
dangerous_commands = [
    "rm -rf",
    "reboot",
    "shutdown",
    "poweroff",
    "mkfs",
    "dd",
    "iptables -F",
    "killall",
]

# Remote Server Inventory
[[nodes]]
id = "web-prod"
name = "Production Web Server"
host = "203.0.113.10"
port = 22
user = "deploy"
key_path = "~/.ssh/id_ed25519"
tags = ["prod", "web"]

[[nodes]]
id = "db-node"
name = "Primary PostgreSQL"
host = "203.0.113.11"
port = 22
user = "ubuntu"
sudo = true
tags = ["prod", "database"]
```

---

## 3. Engine Settings (`~/.corex/settings.json`)

Stored automatically when switching models via `/model`, saving a key with `/key` or running `/yolo`. A project `.corex/settings.json` may override the model, temperature, reasoning effort and compaction settings; keys, YOLO mode, the local LLM switch and MCP servers are only read from the global file.

### Example `~/.corex/settings.json`:
```json
{
  "model": "deepseek-v4-pro",
  "active_provider": "deepseek",
  "temperature": 0.7,
  "reasoning_effort": "high",
  "yolo_mode": false,
  "local_llm_enabled": false,
  "local_llm_url": "http://127.0.0.1:8080/v1",
  "allowed_commands": ["cargo test"]
}
```

---

## 4. Quick Commands
* `/yolo on` / `/yolo off`: Enable or disable interactive confirmations on the fly.
* `/model`: Open interactive model selector.
* `/fleet`: Open the dual-pane Fleet console.
* `/info`: Show active environment, workspace, and config paths.
