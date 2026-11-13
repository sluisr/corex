# Corex (`cx` & `cxs`)

> High-performance autonomous terminal agent for software engineering & server infrastructure automation (DeepSeek, Gemini, Claude, OpenAI & Local LLMs), written in native Rust.

[![Release](https://img.shields.io/github/v/release/sluisr/corex?style=flat-square&color=2563eb&label=release)](https://github.com/sluisr/corex/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/sluisr/corex/ci.yml?style=flat-square&label=ci)](https://github.com/sluisr/corex/actions)
[![Discord](https://img.shields.io/badge/Discord-Join%20Community-5865F2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/MGMt6M6Yx)
[![Rust](https://img.shields.io/badge/rust-2021-f97316?style=flat-square)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue?style=flat-square)](LICENSE)

---

## Contents

- [Overview](#overview)
- [Key Highlights](#key-highlights)
- [Performance & Resource Footprint](#performance--resource-footprint)
- [Operating Modes](#operating-modes)
- [Installation](#installation)
- [Authentication & Configuration](#authentication--configuration)
- [Usage](#usage)
- [Slash Commands](#slash-commands)
- [Core Developer Tools](#core-developer-tools)
- [Configuration Files](#configuration-files)
- [License & Commercial Licensing](#license--commercial-licensing)

---

## Overview

**Corex** (invoked as `cx` or `corex`; `cxs` is the same binary under another name, see [The `cxs` command](#the-cxs-command)) is a native terminal agent for software engineers and systems administrators who want low latency, tools that run on their own machine, and the option of fully local inference:

- **`cx` / `corex`**: Autonomous coding and full-stack software development assistant with multi-turn reasoning, subagents, and fuzzy patching.
- **`cxs` / `cx --fleet`**: Dedicated SysAdmin & Fleet automation console for remote VPS nodes and server clusters, featuring live SSH telemetries (CPU, RAM, Disk) and embedded interactive PTY terminal multiplexing.

Unlike traditional terminal assistants built on Node.js or Python runtimes, Corex is engineered from the ground up in pure Rust. It starts in about 1.6 ms (`cx --version`) and draws its first frame in about 10 ms, ships as one native binary per platform, and idles at about 13.4 MB of resident memory (see [Performance & Resource Footprint](#performance--resource-footprint) for the measurements).

> [!NOTE]
> **Release Status:** Active Development / Beta (`v0.11.0`). Corex is moving quickly towards `v1.0` stabilization. APIs, internal interfaces, and CLI flags are continuously refined. Community feedback and contributions are welcome!

---

## Key Highlights

- **Server & Fleet Automation (`cxs`):** Built-in remote VPS cluster management and SysAdmin console (`cxs` or `cx --fleet`). Inspect remote CPU/RAM/Disk metrics, broadcast commands across node clusters, and open interactive SSH sessions in an embedded terminal. Fleet mode drives the OpenSSH `ssh` client, which must be installed.
- **Native Systems Architecture:** One native Rust binary per platform (6.6 – 9.7 MB) that needs no Node.js, Python or other runtime to run. On Linux it links dynamically against `libc`, `libm`, `libpthread`, `libdl` and `libutil`. Startup in about 1.6 ms and about 13.4 MB of resident memory when idle, of which 2.3 MB is anonymous (heap and stacks).
- **Multi-Provider Wire Protocols:** Native streaming adapters for DeepSeek V4.1, Google Gemini (Gemini 3.x Flash with Thought Signatures), Anthropic Claude (Opus, Sonnet and Haiku 5.5, and Fable 5.1, via `/v1/messages` SSE), OpenAI (`/chat/completions`, o1/o3-mini reasoning effort), Groq, GitHub Models, OpenRouter, and Mistral AI.
- **Local Inference:** Direct integration with `llama-server` (llama.cpp, `http://127.0.0.1:8080/v1` by default) and `Ollama` (`http://127.0.0.1:11434/v1`, set with `--local-url`): prompts, code and responses stay on your machine and inference costs $0.00. Corex checks for a newer release at startup unless you turn it off with `--no-update-check` or `COREX_NO_UPDATE_CHECK=1` (see [Offline Sovereign Local LLM](#2-offline-sovereign-local-llm-air-gapped--private)).
- **Mobile Vertical / Termux Responsive UX:** Auto-detection for Android Termux portrait screens, native tap-to-keyboard, non-overlapping 2-column status bar, and stacked modals for narrow terminals.
- **Real-Time Prompt Queuing:** Type or paste follow-up prompts while the model is streaming responses. Queued items run sequentially without interrupting generation.
- **Fuzzy Diff Patch Engine (`apply_patch`):** Precise unified diff patching with whitespace tolerance, accurate line offset calculation, and CRLF/LF line ending preservation.
- **File Locks & Protected Paths:** File-level advisory concurrency locks (`file_lock.rs`) across parallel background tasks and processes, plus protection of sensitive files (SSH keys, GPG credentials, git configs) even in YOLO mode.
- **Session Audit Log:** A plain-text log of every tool call, shell command and LLM exchange, with millisecond timestamps, in `~/.corex/logs/corex-forensic-YYYY-MM-DD.log`, kept for 7 days. It is not signed or encrypted; pasted text is never written to it, and `COREX_FORENSIC_LOG=0` turns it off.
- **Global & Workspace Personality (`SYSTEM.md`):** Loads every instruction file it finds (`~/.corex/SYSTEM.md`, `COREX.md`, `AGENTS.md`, `DEEPSEEK.md`, global first, then the workspace); `/memory list` shows which ones are in use.
- **Clipboard Integration:** Copies text through `wl-copy`, `xclip`/`xsel` or OSC 52, and pastes images, or copied image files, with `Ctrl+V` (as `[Image #N]`) for models that can see them. `Alt+V` pastes the text only. Code blocks copy with one click on `⧉ copy`, without the rail or the line breaks added to fit the screen.
- **KV Cache Preservation:** Context management and tool compression keep the prompt prefix stable so provider prompt caching applies. The hit rate depends on the provider and the conversation; `/stats` and the summary printed on exit report it.
- **Non-Blocking Authentication:** Silent AskPass integration for Linux `sudo`, git, and SSH commands, plus contextual non-intrusive provider auth.
- **Model Context Protocol (MCP):** Compatible with standard MCP servers (database connectors, GitHub tools, Docker controllers).
- **Browser Control:** With the [Corex Browser Bridge](https://chromewebstore.google.com/detail/pgbencaoenigjeekljfindfeaolenaip) extension, Corex opens pages in your own browser and reads, clicks, types and fills forms there, using your sign-ins (see [Browser Control](#browser-control)).
- **Copy & Export:** Every code block in a reply has a `⧉ copy` label (`Alt+Y` copies the last one), and `/export` saves the chat as Markdown, text or HTML to `~/.corex/exports/`.

---

## Performance & Resource Footprint

Corex is built with Link-Time Optimization (`lto = "fat"`), a single codegen unit, size optimization (`opt-level = "s"`), `panic = "abort"` and stripped symbols. On Linux it returns freed memory to the OS after heavy turns (`malloc_trim`).

### Measured footprint (v0.10.1, Linux x86_64)

| Metric | Result |
| :--- | :--- |
| **Binary size** | 9,675,104 bytes (9.68 MB) |
| **Download** (`.tar.gz`) | 8.65 MB |
| **Startup**, `cx --version` | median 1.6 ms (min 1.4, max 2.0; 50 runs) |
| **First frame** of the interactive TUI | median 10.4 ms (min 10.1, max 11.9; 10 runs) |
| **Resident memory**, TUI idle | 13.4 MB, of which 2.3 MB anonymous (heap and stacks); peak 13.4 MB |
| **CPU**, TUI idle | 0.10% over 10 s |
| **Linking** | Dynamic: `libc`, `libm`, `libpthread`, `libdl` and `libutil`. The official Linux builds are linked against glibc 2.17 with `cargo-zigbuild` and run on Debian, Ubuntu, Linux Mint and Fedora out of the box. |

### Binaries by platform (v0.10.1)

| Platform | Download | Binary | Notes |
| :--- | ---: | ---: | :--- |
| Linux x86_64 | 8.65 MB | 9.68 MB | glibc 2.17 or newer |
| Linux ARM64 | 8.15 MB | 8.68 MB | glibc 2.17 or newer |
| macOS Apple Silicon | 6.74 MB | 6.64 MB | |
| macOS Intel | 7.27 MB | 7.40 MB | |
| Windows x86_64 | 7.17 MB | 7.30 MB | |
| Android Termux (aarch64) | 7.16 MB | 7.67 MB | |

Sizes are in MB of 1,000,000 bytes.

*Methodology:* Measured on 2026-11-02 on an AMD Ryzen 5 7430U, Linux 7.2.11 (Arch), with the official v0.10.1 x86_64 release binary. The TUI ran on a pseudo-terminal with an empty `HOME` and no API key, and sat idle with no input. Reproduce the Linux figures with [`scripts/benchmark.sh`](scripts/benchmark.sh) (`./scripts/benchmark.sh path/to/cx` measures any binary). The run-time measurements were taken on Linux x86_64 only.

*Not measured:* memory while streaming a model response, the footprint of other tools, and run-time behaviour on macOS, Windows, ARM64 and Android.

---

## Operating Modes

Corex supports three unified execution paradigms:

```text
                           ┌─────────────────────────────┐
                           │    Corex Operating Modes    │
                           └──────────────┬──────────────┘
                                          │
                  ┌───────────────────────┼───────────────────────┐
                  ▼                       ▼                       ▼
       ┌──────────────────────┐┌──────────────────────┐┌──────────────────────┐
       │ 1. Frontier Cloud    ││  2. Sovereign Local  ││ 3. Fleet & SysAdmin  │
       │ (DeepSeek / Gemini / ││ (llama-server/Ollama/││ (cxs / Remote VPS /  │
       │  Claude / OpenAI...) ││  Air-Gapped Privacy) ││  SSH Clusters)       │
       └──────────────────────┘└──────────────────────┘└──────────────────────┘
```

### 1. Frontier Cloud Matrix
- **Providers:** DeepSeek (`deepseek-flash`, `deepseek-pro`), Google Gemini (`gemini-3.5-flash-lite`, `gemini-3.5-flash`, `gemini-3.8-flash`), Anthropic (`claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`, `claude-fable-5-1`), OpenAI (`o1`, `o3-mini`, `gpt-4o`, `gpt-4o-mini`), Groq (Llama 3.3 70B, Llama 3.1 8B, DeepSeek R1 70B), OpenRouter, Mistral AI (`codestral-latest`, `mistral-large-latest`), and GitHub Models.
- **Behavior:** High-throughput streaming with Google thought signatures, adaptive reasoning effort, and transparent multi-turn tool calling.
- **Best For:** Heavy architectural refactoring, full-stack implementations, and complex reasoning tasks.

### 2. Offline Sovereign Local LLM (Air-Gapped & Private)
- **Engines:** Any model served through an OpenAI-compatible endpoint: `llama-server` (llama.cpp) on `http://127.0.0.1:8080/v1`, the default, or `Ollama` on `http://127.0.0.1:11434/v1` (pass it with `--local-url` or `COREX_LOCAL_LLM_URL`).
- **Behavior:** Inference runs entirely on your machine: prompts, code and responses are never sent to a provider, and API cost is $0.00. At startup Corex makes a background request to find out whether a newer release exists (`corex.sluisr.com`, then GitHub), plain `GET` requests with a `corex/<version>` user agent and no conversation data (the GitHub API request adds your `GITHUB_TOKEN` or `GH_TOKEN` when one is set, to avoid rate limits). To turn it off, run `cx --no-update-check` or set `COREX_NO_UPDATE_CHECK=1`; no request is made then. Without it, a fully air-gapped machine will see the request fail silently.
- **Best For:** Proprietary codebases, classified perimeters, air-gapped environments, and offline development.

### 3. Fleet & SysAdmin Remote Infrastructure (`cxs`)
- **Engine:** Multi-node orchestrator (`cxs` or `cx --fleet`) that drives the system OpenSSH `ssh` client, which must be installed.
- **Behavior:** Dual-pane split-screen TUI with AI assistance on the left and a live remote server terminal + real-time CPU/RAM/Disk metrics on the right. Supports bastion/jump hosts, key-based auth, and automated diagnostic commands.
- **Best For:** Server monitoring, incident response, cluster maintenance, and automated deployments.

---

## Installation

### Requirements

Corex ships as a single native binary, so there is no runtime to install and no Node.js or Python toolchain to manage. What it needs:

| Item | Requirement |
| :--- | :--- |
| **Linux** | x86_64 or ARM64, glibc 2.17 or newer (2.39 for ARM64 builds older than `v0.9.0`) |
| **macOS** | Apple Silicon or Intel |
| **Windows** | x86_64 (`x86_64-pc-windows-msvc`) |
| **Android** | aarch64 via Termux |
| **Terminal** | Unicode and 256-color support; mouse reporting and bracketed paste are used when the terminal offers them |
| **Disk** | The binary is 6.3 – 8.1 MB and the release archive around 7 MB, plus `~/.corex` for configuration and logs |

Everything below is optional and only needed for the feature you want to use:

- **A model provider.** Either an API key for one of the cloud providers (see [Authentication & Configuration](#authentication--configuration)), or a local `llama-server` / `Ollama` endpoint on `http://127.0.0.1:8080/v1` to run fully offline at $0.00.
- **Fleet mode (`cxs`).** The OpenSSH client (`ssh`) must be installed and on `PATH`, and the target nodes must accept key-based authentication.
- **Clipboard integration.** `wl-copy` on Wayland, `xclip` or `xsel` on X11, or a terminal that supports OSC 52. Without any of these, copy and paste fall back to the terminal's own shortcuts.
- **Building from source.** A Rust toolchain (2021 edition) — see [Method 6](#method-6-build-from-source).

### Method 1: Pre-compiled Binary (Recommended for Linux, macOS & Android)

Install via the official installation script:

```bash
curl -fsSL https://raw.githubusercontent.com/sluisr/corex/main/install.sh | sh
```

Or download binaries manually from the [GitHub Releases](https://github.com/sluisr/corex/releases) page for:
- Linux x86_64 (`x86_64-unknown-linux-gnu`)
- Linux ARM64 (`aarch64-unknown-linux-gnu`)
- macOS Apple Silicon (`aarch64-apple-darwin`)
- macOS Intel (`x86_64-apple-darwin`)
- Android Termux (`aarch64-linux-android`)
- Windows (`x86_64-pc-windows-msvc`): unzip the `.zip` and put `cx.exe` in a folder on your `PATH`.

### Method 2: Homebrew (macOS & Linuxbrew)

Install via the official Homebrew tap:

```bash
brew install sluisr/tap/cx
```

Or tap and install:

```bash
brew tap sluisr/tap
brew install cx
```

### Method 3: Official Crates.io (Cargo)

Install the official package from crates.io:

```bash
cargo install corex-cli
```

### Method 4: Nix Flake (NixOS & Linux/macOS Flakes)

Run directly without installing any local toolchains:

```bash
nix run github:sluisr/corex
```

Or enter a temporary shell with `cx`:

```bash
nix shell github:sluisr/corex
```

### Method 5: Via npm / npx

```bash
# Run headlessly or interactively without installation:
npx @sluisr/corex

# Or install globally:
npm install -g @sluisr/corex
```

### Method 6: Build from Source

```bash
git clone https://github.com/sluisr/corex.git
cd corex
cargo build --release
sudo cp target/release/cx /usr/local/bin/
```

### The `cxs` command

`cxs` is the same binary started under another name: it opens the Fleet console, like `cx --fleet`. `install.sh`, the Nix flake and the npm package create it for you. With any other method, use `cx --fleet` or link it yourself:

```bash
ln -s "$(command -v cx)" ~/.local/bin/cxs
```

---

## Authentication & Configuration

### Cloud Provider Credentials

Corex features dynamic, contextual authentication. You can export the environment variable for your preferred provider:

```bash
# DeepSeek API
export COREX_API_KEY="sk-your-deepseek-api-key"
# or
export DEEPSEEK_API_KEY="sk-your-deepseek-api-key"

# Google Gemini API
export GEMINI_API_KEY="AIzaSy-your-gemini-key"

# Anthropic Claude API
export ANTHROPIC_API_KEY="sk-ant-your-claude-key"

# OpenAI API
export OPENAI_API_KEY="sk-your-openai-key"

# Groq Cloud API
export GROQ_API_KEY="gsk_your-groq-key"

# Mistral AI API
export MISTRAL_API_KEY="your-mistral-key"

# GitHub Models
export GITHUB_TOKEN="ghp_your-github-token"
```

You can also configure credentials interactively at any time inside the TUI by typing `/auth` or selecting a model via `/model`.

### Local LLM Endpoint Setup (Optional)

Start `llama-server` (from `llama.cpp`) on port 8080, which Corex uses by default:

```bash
llama-server -m models/Llama-3.2-3B-Instruct-Q4_K_M.gguf --port 8080 -c 8192
```

For Ollama, which listens on port 11434, point Corex at it:

```bash
ollama serve
cx --local --local-url http://127.0.0.1:11434/v1 --local-model qwen2.5-coder:32b
```

---

## Usage

### Interactive TUI Mode

Launch Corex inside any project directory:

```bash
cd my-project/
cx
```

### Fleet & SysAdmin Remote Infrastructure Mode (`cxs`)

Launch the live interactive split-screen SysAdmin console or execute cluster commands:

```bash
# Launch interactive dual-pane Fleet console (AI Chat + Live SSH Server Terminal)
cxs
# or
cx --fleet

# Ping all nodes
cxs ping

# View live top metrics (CPU, RAM, Disk, Load) across fleet
cxs top

# Execute command on a specific node or broadcast (all, cluster:<name>, tag:<name>, node1,node2)
cxs run prod-web-01 "systemctl status nginx"
cxs broadcast cluster:production "uptime && free -h"

# Initialize template fleet configuration in current workspace (.corex/fleet.toml)
cxs init
```

### Browser Control

Corex can work in a tab of your own browser: open a page, read it, click, type, fill a form, upload a file or take a screenshot, with the sessions you are already signed in to.

1. Install **[Corex Browser Bridge](https://chromewebstore.google.com/detail/pgbencaoenigjeekljfindfeaolenaip)** from the Chrome Web Store, in Chrome, Chromium, Brave, Edge or Vivaldi.
2. Start `cx` once. It registers the native messaging link to the extension (no ports, no pairing), and does it again on every start, so installing the browser later only needs another start.
3. Ask for it in plain words: *"open my GitHub notifications and summarize them"*, *"fill in this form with…"*.

- Corex only acts in the tabs it opens, kept in a Corex tab group. Closing them, or pressing **Release tabs** in the extension, takes them back.
- While Corex works, the browser shows *"Corex Browser Bridge started debugging this browser"*: that is how Chromium browsers show the `debugger` permission the extension uses to click and type.
- Supported on Linux and macOS. Browsers installed with Snap or Flatpak keep their configuration elsewhere and are not detected yet; Windows is not supported yet.

### Non-Interactive / Scripting Mode

Execute automated one-shot tasks directly from your shell:

```bash
# Execute prompt directly
cx -p "Analyze current git status and fix formatting issues"

# Query live web search
cx -w "Latest DeepSeek API release updates"

# Execute with auto-approval (YOLO mode)
cx -y -p "Run cargo clippy and apply proposed fixes"

# Override model engine
cx -m deepseek-v4-pro -p "Prove correctness of this algorithm"

# Target specific workspace
cx -C /path/to/project -p "Run tests"

# Self-update binary in-place with SHA-256 verification
cx update
cx update --force

# Generate shell completions (Bash, Zsh, Fish, PowerShell, Elvish)
cx --completions zsh > ~/.zfunc/_cx
cx --completions bash > ~/.local/share/bash-completion/completions/cx
cx --completions fish > ~/.config/fish/completions/cx.fish
```

---

## Slash Commands

Within the interactive TUI, type `/` to access built-in commands:

| Command | Description |
| :--- | :--- |
| `/fleet` / `/sysadmin` / `/vps` | Open interactive Fleet dual-pane console: select remote VPS nodes, view metrics & attach PTY. |
| `/export [md\|txt\|html] [full]` | Save the chat to `~/.corex/exports/<project>/` (or `exports_dir`) as Markdown, text or an HTML page to print to PDF. |
| `/model` | Master-detail catalog: select provider/model (DeepSeek, Gemini, Claude, OpenAI, Groq, Local) & tune CoT sliders. |
| `/key [api_key]` (alias `/auth`) | Set or replace the API key of the active provider; without an argument it opens the credentials dialog. |
| `/balance` | Check current account credits asynchronously, plus what the current session has spent (`/wallet`). |
| `/fim <file>` | Fill-in-the-Middle code completion at cursor location. |
| `/local <prompt>` | Query local offline LLM directly ($0.00 cost) or check status (`/local status`). |
| `/web <query>` | Query the live internet via autonomous web search engine (`/search`). |
| `/fetch <url>` | Fetch a web page as Markdown. |
| `/yolo` | Toggle automatic tool approval (`/yolo [on \| off]`). |
| `/plan` | Enter read-only architectural planning mode. |
| `/prefix <text>` | Enforce specific response prefixes for strict formatting. |
| `/chat` / `/resume` | List, save, resume or delete conversation sessions (`/chat list\|save\|resume\|delete\|new`). |
| `/new` | Start a fresh chat session. |
| `/save <tag>` | Save a checkpoint of the current session. |
| `/rewind` | Go back to any earlier message: rewind the conversation, revert the agent's file edits since then, or both. |
| `/compact` / `/compress` | Compress session context to conserve token limits. |
| `/memory [list\|show\|add\|reload]` | List the instruction files in the prompt, show the memory block, save a note (`~/.corex/COREX.md`, or private to the project with `--project`), or reload them after editing. The model can save notes too with `save_memory`, after your approval. |
| `/directory [add\|show]` | Let the agent read directories outside the workspace without confirmation, for this session (alias `/dir`). |
| `/mcp` | Interactive Model Context Protocol server catalog (`/mcp status`, `/mcp catalog`, `/mcp add`, `/mcp remove`, `/mcp reload`). |
| `/tasks` | Manage background tasks (`list`, `status`, `kill`, `send`). |
| `/stats` (alias `/context`) | View session token metrics, KV Cache hit rate and the context in use. `/stats full` (or `/context`) adds the window source, history cap and where to change them. |
| `/info` | Display system telemetry, version, and links. |
| `/update` | Check for updates or apply in-place atomically (`/update apply` / `/update force`). |
| `/mouse` | Toggle mouse reporting: wheel and drag, or the terminal's native touch. |
| `/keyboard` | Pause the mouse so the next tap raises the keyboard (Termux). |
| `/clear` | Clear the terminal viewport. |
| `/help` | Display shortcuts and usage guidelines. |
| `/quit` / `/exit` | Terminate session and save state. |

---

## Core Developer Tools

Corex exposes native tools directly to the reasoning engine:

- **`apply_patch`:** Atomic unified diff patch tool with fuzzy whitespace tolerance, offset calculation, and CRLF/LF line ending preservation.
- **File System:** `read_file`, `write_file`, `edit`, `list_directory`, `glob`, and `grep`.
- **`run_shell_command`:** Sandboxed shell execution with safety checks, adaptive background detachment, and sudo AskPass.
- **`manage_task`:** Background process manager supporting process inspection, signals, and standard input forwarding.
- **`web_search` & `web_fetch`:** Live search engine integration and clean HTML-to-markdown extraction.
- **`delegate_subagent` & `manage_subagents`:** Parallel delegation to specialized autonomous subagents (`scout`, `tester`, `researcher`, `worker`) with context isolation and reactive completion notifications.
- **`write_todos`:** Multi-step autonomous task tracking and verification.
- **Persistent Memory:** Context retention across runs via `~/.corex/COREX.md` (global), `./COREX.md` / `./AGENTS.md` (project) and private per-project notes in `~/.corex/projects/`.

---

## Configuration Files

Configuration and logs are stored globally in `~/.corex/` and can be overridden per workspace:

| Path | Description |
| :--- | :--- |
| `~/.corex/fleet.toml` | Global Fleet configuration: VPS nodes, clusters, SSH keys, jump hosts, and command whitelist. |
| `.corex/fleet.toml` | Workspace-level Fleet nodes and clusters for project-specific infrastructure. It replaces the global file while present, but its `safe_commands`/`dangerous_commands` are ignored (the global lists apply) and every command or status probe on its servers asks for confirmation, since the file arrives with the repository. |
| `~/.corex/SYSTEM.md` | Global AI personality, operational rules, and behavioral guidelines injected into all sessions. |
| `.corex/SYSTEM.md` | Workspace-level AI instructions and coding constraints (`COREX.md`, `AGENTS.md` and `DEEPSEEK.md` at the project root are loaded too, in that order). |
| `~/.corex/settings.json` | Global settings: provider, base URLs, default model, allowed commands. Only values that differ from the defaults are written. |
| System keyring | API keys, in the `corex` / `credentials` entry (GNOME Keyring or KWallet through Secret Service, macOS Keychain, Windows Credential Manager). |
| `~/.corex/credentials.json` | API keys where no keyring answers (SSH, servers, Termux, containers) or with `COREX_KEYRING=0`. Mode 600. |
| `.corex/settings.json` | Project-level workspace settings overrides (secrets automatically stripped before git). |
| `~/.corex/flash_settings.json` | CoT reasoning depth configuration for Flash. |
| `~/.corex/pro_settings.json` | Deep reasoning configuration for Pro. |
| `~/.corex/logs/` | Daily forensic audit logs (`corex-forensic-YYYY-MM-DD.log`, 7-day retention). |
| `~/.corex/locks/` | Advisory per-path concurrency file locks (`<hash>.lock`). |

Corex reads only `.corex/` and `~/.corex/`. It never reads `~/.gemini` or `~/.deepseek`, so it can run next to Gemini CLI or deepseek-cli without sharing their settings or instructions.

### Context window and compaction

Corex sizes its limits from the active model's context window instead of a fixed number:

| Setting (`settings.json`) | Default | Meaning |
| :--- | :--- | :--- |
| `context_window_tokens` | What the provider reports for the model; else a default by provider: DeepSeek and Google 1,000,000; Anthropic 200,000; OpenAI, GitHub Models, Groq, OpenRouter, Mistral and custom endpoints 128,000; local 24,000 | Context window of the active model. Set it when your model differs and the provider does not say. |
| `compact_threshold_tokens` | 80% of the window | Prompt size that triggers auto-compaction. |
| `history_token_limit` | 90% of the window, never below the compaction threshold | Above this, the oldest turns are dropped from a request. |

When you pick a model, Corex asks its provider for the window in the background (DeepSeek, Anthropic, Google, OpenRouter, Groq, Mistral and OpenAI-compatible servers that list it; OpenAI and GitHub Models do not report one). Order of precedence: `context_window_tokens`, then a `context_window` on the model in your `providers` list, then what the provider reported, then the default above. Anthropic's answer is capped at 200,000, since the 1M window needs a beta header Corex does not send.

The status bar shows the share of the window in use (`~13%`) and turns yellow once the prompt passes the auto-compact limit. The size is the prompt size the API reported for the last request plus an estimate of what was added since. `/stats full` (or `/context`) lists the limits in force and where the window comes from, and the compaction notice says at how many tokens it ran.

Earlier versions saved `"compact_threshold_tokens": 95000` to `settings.json`; that exact value is treated as unset. Use `95001` to keep a threshold that low.

### Command-line flags and environment variables

| Flag / variable | Effect |
| :--- | :--- |
| `--no-update-check` / `COREX_NO_UPDATE_CHECK=1` | Skip the startup request that looks for a newer release, and hide the cached "new version" notice. `cx --upgrade` still works. An empty value, `0` or `false` leaves the check on. |
| `-p`, `--prompt` / `-w`, `--web` / `--fetch <url>` | Run one prompt, one web search or one page fetch and exit. |
| `-m`, `--model` / `COREX_MODEL` | Model to use. |
| `--base-url` / `COREX_BASE_URL` | API base URL of the active provider. |
| `--api-key` / `COREX_API_KEY` | API key for the active provider (see [Authentication](#authentication--configuration)). |
| `--local`, `--local-url`, `--local-model` / `COREX_LOCAL_LLM_URL` | Use the local model server, at this endpoint and with this model. |
| `-y`, `--yolo` | Approve every tool call without asking. |
| `-C`, `--directory` | Workspace to work in. |
| `-r`, `--resume` / `-l`, `--list-sessions` | Resume a saved session by ID, tag or index / list this project's sessions. |
| `--upgrade` (or `cx update`), `--force` | Update in place; reinstall even when up to date. |
| `--completions <shell>` | Print the shell completion script. |
| `--fleet` (or `cxs`) | Open the Fleet console. |
| `COREX_KEYRING=0` | Keep API keys in `~/.corex/credentials.json` instead of the system keyring. |
| `COREX_FORENSIC_LOG=0` | Do not write the session audit log in `~/.corex/logs/`. |

---

## License & Commercial Licensing

- **Open Source License:** GNU Affero General Public License v3.0 ([AGPL-3.0-only](LICENSE)).
- **Author:** [sluisr](https://sluisr.com/) (`contact@sluisr.com`)
- **Website:** [https://corex.sluisr.com](https://corex.sluisr.com)
- **Discord:** [Join Corex Discord](https://discord.gg/MGMt6M6Yx)
- **Repository:** [https://github.com/sluisr/corex](https://github.com/sluisr/corex)
- **Dual Licensing:** If your organization requires embedding Corex within proprietary closed-source infrastructure or commercial SaaS without AGPL-3.0 copyleft obligations, custom commercial licenses and enterprise support agreements are available upon request. Contact `contact@sluisr.com`.
