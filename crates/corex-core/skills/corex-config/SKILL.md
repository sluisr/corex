---
name: corex-config
description: >-
  Use this skill when the user asks about Corex configuration files, how to customize
  Corex personality or system prompt, where to add allowed or safe commands,
  how to configure remote servers in fleet.toml, or where configuration files
  are stored. Trigger phrases: 'config', 'configuracion', 'archivos de configuracion',
  'donde configurar', 'personalidad', 'comandos permitidos', 'safe_commands',
  'fleet.toml', 'system.md', 'corex.md', 'settings', 'como personalizar'.
---

# Corex Configuration & Customization Guide

Corex uses a clear, human-readable hierarchy of configuration files in **TOML** and **Markdown** (`.md`).

---

## 📂 Configuration Files Overview

| File | Scope | Format | Purpose |
| :--- | :--- | :--- | :--- |
| **`~/.corex/SYSTEM.md`** *(or `COREX.md`)* | **Global** | Markdown | **User Personality & Global Rules**: custom tone, role instructions, language, conventions applied to all projects and sessions. |
| **`.corex/SYSTEM.md`** *(or `./COREX.md` / `./AGENTS.md`)* | **Project Local** | Markdown | **Project Instructions**: repository-specific architecture, build commands, and rules. |
| **`~/.corex/fleet.toml`** *(or `.corex/fleet.toml`)* | **Global or Local** | TOML | **Remote Fleet & Allowed Commands**: SSH server inventory, `safe_commands` (auto-approved commands), and `dangerous_commands`. |
| **`~/.corex/config.toml`** | **Global** | TOML | **Corex Engine Settings**: active model, reasoning effort, temperature, YOLO mode default, API keys. |
| **`~/.ssh/config`** | **System** | OpenSSH | Fallback server discovery for Fleet mode. |

---

## 1. 🧠 Custom Personality & Global Rules (`~/.corex/SYSTEM.md`)

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

---

## 2. 🛡️ Allowed & Safe Commands (`~/.corex/fleet.toml` or `.corex/fleet.toml`)

In Fleet & SysAdmin mode, Corex categorizes commands into:
1. **Safe**: Run automatically without confirmation prompts.
2. **Mutating**: Require user confirmation unless in YOLO mode.
3. **Dangerous**: Require strict confirmation.

You can customize which commands are auto-approved by adding them to `safe_commands`.

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

## 3. ⚙️ Engine Settings (`~/.corex/config.toml`)

Stored automatically when switching models via `/model` or running `/yolo`.

### Example `~/.corex/config.toml`:
```toml
model = "deepseek-flash"
temperature = 0.7
reasoning_effort = "medium"
yolo_mode = false
local_llm_enabled = false
local_llm_url = "http://127.0.0.1:8080/v1"
```

---

## 4. 🚀 Quick Commands
* `/yolo on` / `/yolo off`: Enable or disable interactive confirmations on the fly.
* `/model`: Open interactive model selector.
* `/fleet`: Open the dual-pane Fleet console.
* `/info`: Show active environment, workspace, and config paths.
