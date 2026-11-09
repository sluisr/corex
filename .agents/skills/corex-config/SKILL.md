---
name: corex-config
description: >-
  Use this skill when the user asks about Corex configuration files, how to customize
  Corex personality or system prompt, where to add allowed or safe commands,
  how to configure remote servers in fleet.toml, or where configuration files
  are stored. Trigger phrases: 'config', 'configuracion', 'archivos de configuracion',
  'donde configurar', 'personalidad', 'comandos permitidos', 'safe_commands',
  'fleet.toml', 'system.md', 'corex.md', 'agents.md', 'deepseek.md', 'settings',
  'como personalizar', 'memory', 'memoria', '/memory', 'instrucciones ignoradas',
  'exportar', 'exportaciones', 'exports', 'exports_dir', 'carpeta de exportaciones',
  'donde se guardan los chats exportados', 'cambiar ruta de exportacion'.
---

# Corex Configuration & Customization Guide

Corex uses a clear, human-readable hierarchy of configuration files in **TOML** and **Markdown** (`.md`).

---

## Configuration Files Overview

|File | Scope | Format | Purpose |
|:--- | :--- | :--- | :--- |
|**`~/.corex/SYSTEM.md`** *(+ `~/.corex/COREX.md`)* | **Global** | Markdown | **User Personality & Global Rules**: custom tone, role instructions, language, conventions applied to all projects and sessions. |
|**`.corex/SYSTEM.md`** *(+ `./COREX.md`, `./AGENTS.md`, `./DEEPSEEK.md`)* | **Project Local** | Markdown | **Project Instructions**: repository-specific architecture, build commands, and rules. |
|**`~/.corex/fleet.toml`** *(or `.corex/fleet.toml`)* | **Global or Local** | TOML | **Remote Fleet & Allowed Commands**: SSH server inventory, `safe_commands` (auto-approved commands), and `dangerous_commands`. A workspace `.corex/fleet.toml` only contributes servers: its command lists are ignored and its servers always ask before Corex reaches them. |
|**`~/.corex/settings.json`** | **Global** | JSON | **Corex Engine Settings**: active model and provider, reasoning effort, temperature, YOLO mode default, allowed commands. Only values that differ from the defaults are written. |
|**System keyring** (`corex` / `credentials`) | **Global** | JSON secret | **API keys**: kept in GNOME Keyring/KWallet (Secret Service), macOS Keychain or Windows Credential Manager. Where no keyring answers, or with `COREX_KEYRING=0`, they go to `~/.corex/credentials.json` (mode 600). Never edit or print them; to change a key the user runs `/key`. |
|**`~/.corex/exports/`** | **Global** | Folder | **Exported chats**: what `/export` writes, one folder per project. Moved elsewhere with `exports_dir` in `settings.json` (see section 3). |
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
3. `./.corex/SYSTEM.md`
4. `./COREX.md`
5. `./AGENTS.md`
6. `./DEEPSEEK.md`
7. `~/.corex/projects/<workspace path>/MEMORY.md` (private project notes, never inside the repository)

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

Stored automatically when switching models via `/model` or saving a key with `/key` (the key itself goes to the system keyring, not to this file). `/yolo` only changes the current session. A project `.corex/settings.json` may override the model, temperature, reasoning effort and compaction settings; keys, YOLO mode, the local LLM switch and MCP servers are only read from the global file.

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

### Your name and colour in the conversation
Your messages are labelled with the login of the machine (`$USER`) and drawn in a soft blue-grey. Both can be changed in the global `~/.corex/settings.json`:
```json
{
  "user_name": "Luis",
  "user_color": "#82d7a0"
}
```
* `user_name`: the label in `[Luis] your message`. Spaces are removed and it is cut to 16 characters. Remove the key to go back to the machine's login.
* `user_color`: the colour of your messages, as `#rrggbb`. An invalid value keeps the default and Corex says so at startup. Pick a light colour for dark terminals and a dark one for light terminals, so it stays readable.

When the user asks to change their name or the colour of their messages, edit these keys in `~/.corex/settings.json` (keep the other keys as they are) and tell them to restart `cx`: settings are read at startup.

### Where `/export` saves chats (`exports_dir`)
By default `/export` writes to `~/.corex/exports/<project>/`, where `<project>` is the name of the folder `cx` was started in (`home` when started in the home folder). Files are named by date and chat title, such as `2026-11-09_22-50_check-the-disk.md`, and an export never overwrites an earlier one (`-2`, `-3`… is added).

To keep them somewhere else, set `exports_dir` in the global `~/.corex/settings.json`:
```json
{
  "exports_dir": "~/Documents/corex"
}
```
* The value must be a full path: absolute (`/data/chats`) or starting with `~/`. A relative path such as `chats` is rejected.
* It must be a folder. If it does not exist yet, Corex creates it on the first export.
* The per-project folders and file names stay the same inside it.
* It is read on every `/export`, so a change applies at once, without restarting.
* If the value is invalid (relative, or pointing at a file), Corex says so at startup and in the export message, and saves to `~/.corex/exports` meanwhile. Nothing is lost.
* `/export <path>` still saves one export to a given folder or file, without changing the setting.

When the user asks to change where exports go:
1. Work out the folder they mean and write it as a full path (`~/Documents/corex`, not `Documents/corex`). Ask only if it is really unclear.
2. Check it is not an existing file. Create it with `mkdir -p` if it does not exist, so a typo shows up now rather than on the next export.
3. Set `exports_dir` in `~/.corex/settings.json`, keeping every other key as it is. To go back to the default, remove the key.
4. Tell them where the next export will land, for example `~/Documents/corex/cx/`. No restart is needed.

Do not move earlier exports unless the user asks; if they do, move the per-project folders from the old location into the new one.

---

## 4. Quick Commands
* `/yolo on` / `/yolo off`: Enable or disable interactive confirmations on the fly.
* `/model`: Open interactive model selector.
* `/fleet`: Open the dual-pane Fleet console.
* `/export [md|txt|html] [full] [path]`: Save the chat to the exports folder (see section 3).
* `/info`: Show active environment, workspace, and config paths.
