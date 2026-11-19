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
  'donde se guardan los chats exportados', 'cambiar ruta de exportacion',
  'proveedor', 'provider', 'api key', 'clave', '/key', 'añadir modelo', 'agregar modelo',
  'modelos personalizados', 'custom models', 'nvidia', 'nvidia nim', 'nim', 'build.nvidia.com',
  'openrouter', 'groq', 'mistral', 'providers en settings.json'.
---

# Corex Configuration & Customization Guide

Corex uses a clear, human-readable hierarchy of configuration files in **TOML** and **Markdown** (`.md`).

---

## Configuration Files Overview

|File | Scope | Format | Purpose |
|:--- | :--- | :--- | :--- |
|**`~/.corex/SYSTEM.md`** | **Global** | Markdown | **User Personality & Global Rules**, written by hand and never edited by Corex: custom tone, role instructions, language, conventions applied to all projects and sessions. |
|**`~/.corex/COREX.md`** | **Global** | Markdown | **Saved notes**: what `/memory add` and the agent's memory tool store when asked to remember something. Sent to the model under its own heading, after `SYSTEM.md`. Keep a rule in one of the two files, not both. |
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

**Changing settings yourself:** `read_file`, `edit` and the shell refuse everything under `~/.corex`, so use the `corex_settings` tool instead. `{"action": "get"}` shows the settings with API keys, tokens and passwords hidden; `{"action": "set", "key": "context_limit_tokens", "value": 256000}` changes one after the user approves it and applies it to the running session (`null` resets it). Everything can be set except the API keys, which the user changes with `/key`. `yolo_mode` applies to the running session only; `providers` takes the whole provider list (never stores API keys); `mcp_servers` takes the whole map (values shown as `[hidden]` keep their real value) and reconnects the servers. `yolo_mode`, `allowed_commands`, `providers` and `mcp_servers` always wait for the user's approval, even in YOLO mode. The user can also change the common ones in the `/settings` panel.

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

### Images sent to the model (`recent_images`)
Screenshots stay in the session (saved as files under `~/.corex/sessions/images/<session id>/`), but each request only carries the newest ones: 3 by default. An older image goes as a note with the path of its saved file, and the agent can look at it again with `read_file` on that path when the user asks about it. Set `"recent_images": 5` (any number) in `~/.corex/settings.json` or a project `.corex/settings.json` to change it, or `0` to send every image as before.

### How much context each request carries (`context_limit_tokens`)
Every agent step resends the conversation, and DeepSeek and Gemini allow 1M tokens, so by default a long session grows to ~800k before compaction summarizes it. `/context-limit 256k` (or `128k`, `512k`, any number of at least 32k, `auto` to remove it) caps that: each request carries at most the limit and compaction runs at 80% of it. It never raises a model's own window, and it is saved as `"context_limit_tokens"` in `~/.corex/settings.json`. `/context-limit` alone shows the current values; `/context` shows the window in use and where it comes from.

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

## 4. Providers and models (`/model`, API keys, NVIDIA NIM)

### Choosing a provider and setting its key
* `/model` lists every provider. Those with a key (or the local one) are on top; the rest sit under `no key`.
* On a provider row, press `E` (or run `/key`) to paste its API key. It is saved in the system keyring and also read from the provider's environment variable (`NVIDIA_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, `GROQ_API_KEY`, `MISTRAL_API_KEY`, `GEMINI_API_KEY`, `GITHUB_TOKEN`…). Keys never go in `settings.json`.
* Pick a model on the right and press `Enter` to use it.

### Adding models to a provider (`providers` in `settings.json`)
A provider's models can be listed, or extended, with an entry under `providers` in `~/.corex/settings.json`. The entry is matched by `name`; `label`, `base_url` and `api_key_env` are taken from the built-in provider when left out, so for a built-in one only `models` is needed. The entry's `models` list **replaces** the built-in list, so include every model you want to see.

Each model needs its `id` exactly as the provider names it. Optional: `label` (shown in `/model`), `reasoning` (e.g. `["low", "medium", "high"]` for models that take a reasoning effort) and `context_window` (tokens, when the provider does not report one).

### NVIDIA NIM (cloud API, `build.nvidia.com`)
NVIDIA's catalog changes too often to ship, so the provider comes **with no models**: you list the ones you want.
1. Get a free key at https://build.nvidia.com/models (it starts with `nvapi-`).
2. In `/model`, select **NVIDIA NIM** (under `no key`), press `E` and paste the key. It is stored for `$NVIDIA_API_KEY` and the keyring.
3. On build.nvidia.com, open the model card you want and copy its id from the code sample (`"model": "meta/llama-3.3-70b-instruct"`).
4. Add it to `~/.corex/settings.json` (keep the other keys as they are):
```json
{
  "providers": [
    {
      "name": "nvidia",
      "models": [
        { "id": "meta/llama-3.3-70b-instruct", "label": "Llama 3.3 70B" },
        { "id": "qwen/qwen2.5-coder-32b-instruct", "label": "Qwen2.5 Coder 32B" }
      ]
    }
  ]
}
```
5. Reopen `/model`: the models appear under NVIDIA NIM. Select one and press `Enter`.

Endpoint: `https://integrate.api.nvidia.com/v1` (OpenAI-compatible; set automatically). If a model does not appear, check that the JSON is valid and the `id` matches the model card character for character.

**Self-hosted NIM** (the NVIDIA container on your own GPU) is not this provider: it serves an OpenAI-compatible API on your machine (usually `http://localhost:8000/v1`), so use it as the **local** provider (see the `corex-local-llm` skill) or add a custom entry with that `base_url`.

When the user asks how to use NVIDIA NIM (or any provider) in Corex, explain these steps in Corex terms; do not answer with generic `curl` or Docker instructions unless they ask about the API itself. If they want, configure the models for them with `corex_settings` (`action: "set", key: "providers"`, preserving existing entries from `action: "get"`): API keys stay untouched and hidden.

---

## 5. Quick Commands
* `/yolo on` / `/yolo off`: Enable or disable interactive confirmations on the fly.
* `/model`: Open interactive model selector.
* `/fleet`: Open the dual-pane Fleet console.
* `/export [md|txt|html] [full] [path]`: Save the chat to the exports folder (see section 3).
* `/info`: Show active environment, workspace, and config paths.
