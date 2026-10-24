# Corex (`cx` & `cxs`)

> High-performance autonomous terminal agent for software engineering & server infrastructure automation (DeepSeek, Gemini, Claude, OpenAI & Local LLMs), written in native Rust.

[![Release](https://img.shields.io/github/v/release/sluisr/corex?style=flat-square&color=2563eb&label=release)](https://github.com/sluisr/corex/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/sluisr/corex/ci.yml?style=flat-square&label=ci)](https://github.com/sluisr/corex/actions)
[![Discord](https://img.shields.io/badge/Discord-Join%20Community-5865F2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/MGMt6M6Yx)
[![Rust](https://img.shields.io/badge/rust-2021-f97316?style=flat-square)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue?style=flat-square)](LICENSE)

---

## Overview

**Corex** (invoked as `cx`, `corex`, or `cxs`) is a native terminal agent architected for software engineers and systems administrators who require low latency, deterministic execution, and air-gapped privacy:

- **`cx` / `corex`**: Autonomous coding and full-stack software development assistant with multi-turn reasoning, subagents, and fuzzy patching.
- **`cxs` / `cx --fleet`**: Dedicated SysAdmin & Fleet automation console for remote VPS nodes and server clusters, featuring live SSH telemetries (CPU, RAM, Disk) and embedded interactive PTY terminal multiplexing.

Unlike traditional terminal assistants built on Node.js or Python runtimes, Corex is engineered from the ground up in pure Rust. It delivers sub-10ms startup times, runs as single self-contained static binaries, and consumes negligible system memory (~11.9 MB RSS).

> [!NOTE]
> **Release Status:** Active Development / Beta (`v0.8.1`). Corex is moving quickly towards `v1.0` stabilization. APIs, internal interfaces, and CLI flags are continuously refined. Community feedback and contributions are welcome!

---

## Key Highlights

- **Server & Fleet Automation (`cxs`):** Built-in remote VPS cluster management and SysAdmin console (`cxs` or `cx --fleet`). Inspect remote CPU/RAM/Disk metrics, broadcast commands across node clusters, and launch interactive embedded SSH sessions with zero external dependencies.
- **Native Systems Architecture:** Compiled static binary (~7.0 MB stripped) with zero external runtime dependencies. Instant startup (<10ms) and minimal RAM utilization (~11.9 MB RSS, with only ~2.2 MB anonymous heap).
- **Multi-Provider Wire Protocols:** Native streaming adapters for DeepSeek V4.1, Google Gemini (Gemini 3.x Flash & 2.5 with Thought Signatures), Anthropic Claude (Claude 3.7 / 3.5 Sonnet via `/v1/messages` SSE), OpenAI (`/chat/completions`, o1/o3-mini reasoning effort), Groq (500+ tok/s), GitHub Models, OpenRouter, and Mistral AI.
- **Air-Gapped Offline Inference:** Direct integration with `llama-server` (llama.cpp) and `Ollama` on `http://127.0.0.1:8080/v1` for 100% private, zero-cost ($0.00) execution with 0 bytes network egress.
- **Mobile Vertical / Termux Responsive UX:** Auto-detection for Android Termux portrait screens, native tap-to-keyboard, non-overlapping 2-column status bar, and stacked modals for narrow terminals.
- **Real-Time Prompt Queuing:** Type or paste follow-up prompts while the model is streaming responses. Queued items run sequentially without interrupting generation.
- **Fuzzy Diff Patch Engine (`apply_patch`):** Precise unified diff patching with whitespace tolerance, accurate line offset calculation, and CRLF/LF line ending preservation.
- **Enterprise Sandbox & Concurrency Locks:** File-level advisory concurrency locks (`file_lock.rs`) across parallel background tasks and processes, plus strict protection of sensitive files (SSH keys, GPG credentials, git configs) even in YOLO mode.
- **Forensic Session Audit:** Cryptographic session logging with millisecond timestamps to `~/.corex/logs/corex-forensic-YYYY-MM-DD.log` with automatic 7-day retention.
- **Global & Workspace Personality (`SYSTEM.md`):** Discovers and applies persistent behavioral rules from `~/.corex/SYSTEM.md` and workspace `COREX.md`.
- **Native Clipboard Integration:** Seamless terminal text copying via OSC 52 sequences and platform clipboard integration.
- **KV Cache Preservation:** Context management and tool compression maintain up to a 96%+ KV Cache hit rate across supported providers.
- **Non-Blocking Authentication:** Silent AskPass integration for Linux `sudo`, git, and SSH commands, plus contextual non-intrusive provider auth.
- **Model Context Protocol (MCP):** Compatible with standard MCP servers (database connectors, GitHub tools, Docker controllers).

---

## Performance & Resource Footprint

Corex is compiled with Link-Time Optimization (`lto = "fat"`), single codegen units, and automatic OS memory compaction (`malloc_trim` on Linux) to minimize memory fragmentation.

### Hardware & OS Specifications

| Component | Minimum Specification | Recommended |
| :--- | :--- | :--- |
| **RAM** | 32 MB | 64 MB+ |
| **CPU** | Any 64-bit x86_64 or ARM64 processor | Multi-core processor |
| **Disk Footprint** | ~5 MB (download) / ~7.0 MB (stripped binary) | 50 MB (including logs) |
| **Operating System** | Linux (glibc 2.17+ or musl), macOS 11+ (Apple Silicon & Intel), Windows 10/11 | Any modern OS |
| **Terminal** | ANSI / 256-color terminal | Truecolor (24-bit) terminal |

### Comparative Benchmarks

| Metric | Corex (Native Rust) | Node.js / TS CLIs | Electron CLIs |
| :--- | :--- | :--- | :--- |
| **Download Size** | **~4.5 – 5.2 MB** | ~80 – 150 MB | ~180 – 350 MB |
| **Binary Size** | **~7.0 MB** (static stripped) | Multi-file tree + Node | Chromium + Node bundle |
| **Memory (Idle)** | **~11.9 MB RSS** (~2.2 MB Heap) | ~120 – 220 MB | ~350 – 600 MB |
| **Memory (Active Streaming)** | **~14 – 18 MB RSS** | ~180 – 350 MB | ~500 – 850 MB |
| **Cold Startup Latency** | **< 10 ms** | ~600 – 1,400 ms | ~2,000 – 4,000 ms |
| **Idle CPU Utilization** | **< 0.1%** | ~1 – 3% | ~3 – 8% |
| **External Dependencies** | **None** | Node.js >= 18, npm | Node, Chromium |

*Benchmark Methodology & Reproduction:*
Corex metrics are benchmarked on Linux x86_64 (Linux 6.x kernel, modern x86_64/ARM64) and can be independently reproduced using [`scripts/benchmark.sh`](scripts/benchmark.sh). Comparative Node.js and Electron ranges reflect typical idle and streaming memory/latency profiles of popular terminal assistants running on Node.js v20+ and Chromium/Electron bundles under identical terminal emulator conditions.

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
- **Providers:** DeepSeek (`deepseek-flash`, `deepseek-v4-pro`), Google Gemini (`gemini-3.5-flash-lite`, `gemini-3.8-flash`, `gemini-2.5-pro`), Anthropic (`claude-3-7-sonnet`, `claude-3-5-sonnet`), OpenAI (`o1`, `o3-mini`, `gpt-4o`), Groq (Llama 3.3 70B @ 500+ tok/s), OpenRouter, Mistral AI, and GitHub Models.
- **Behavior:** High-throughput streaming with Google thought signatures, adaptive reasoning effort, and transparent multi-turn tool calling.
- **Best For:** Heavy architectural refactoring, full-stack implementations, and complex reasoning tasks.

### 2. Offline Sovereign Local LLM (Air-Gapped & Private)
- **Engines:** Any GGUF model served via `llama-server` (llama.cpp) or `Ollama` on `http://127.0.0.1:8080/v1`.
- **Behavior:** 100% offline, air-gapped execution. Zero network calls, zero telemetry, null-routed egress, and $0.00 API cost.
- **Best For:** Proprietary codebases, classified perimeters, air-gapped environments, and offline development.

### 3. Fleet & SysAdmin Remote Infrastructure (`cxs`)
- **Engine:** Built-in SSH client and multi-node orchestrator (`cxs` or `cx --fleet`).
- **Behavior:** Dual-pane split-screen TUI with AI assistance on the left and a live remote server terminal + real-time CPU/RAM/Disk metrics on the right. Supports bastion/jump hosts, key-based auth, and automated diagnostic commands.
- **Best For:** Server monitoring, incident response, cluster maintenance, and automated deployments.

---

## Installation

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
- Windows (`x86_64-pc-windows-msvc`)

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

Start `llama-server` (from `llama.cpp`) or `Ollama` on port 8080:

```bash
llama-server -m models/Llama-3.2-3B-Instruct-Q4_K_M.gguf --port 8080 -c 8192
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

# Ping all nodes or a specific cluster
cxs ping
cxs ping --cluster production

# View live top metrics (CPU, RAM, Disk, Load) across fleet
cxs top
cxs top --cluster production

# Execute command on a specific node or broadcast across entire cluster
cxs run prod-web-01 "systemctl status nginx"
cxs broadcast production "uptime && free -h"

# Initialize template fleet configuration in current workspace (.corex/fleet.toml)
cxs init
```

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

# Generate shell completions (Bash, Zsh, Fish)
cx --completions zsh > ~/.zfunc/_cx
cx --completions bash > ~/.local/share/bash-completion/completions/cx
cx --completions fish > ~/.config/fish/completions/cx.fish
```

---

## Slash Commands

Within the interactive TUI, type `/` to access built-in commands:

| Command | Description |
| :--- | :--- |
| `/fleet` / `/sysadmin` | Open interactive Fleet dual-pane console: select remote VPS nodes, view metrics & attach PTY. |
| `/model` | Master-detail catalog: select provider/model (DeepSeek, Gemini, Claude, OpenAI, Groq, Local) & tune CoT sliders. |
| `/auth` | Configure API credentials securely. |
| `/balance` | Check current account credits and token balance asynchronously (`/wallet`). |
| `/fim <file>` | Fill-in-the-Middle code completion at cursor location. |
| `/local <prompt>` | Query local offline LLM directly ($0.00 cost) or check status (`/local status`). |
| `/web <query>` | Query the live internet via autonomous web search engine (`/search`). |
| `/yolo` | Toggle automatic tool approval (`/yolo [on \| off]`). |
| `/plan` | Enter read-only architectural planning mode. |
| `/prefix <text>` | Enforce specific response prefixes for strict formatting. |
| `/chat` / `/resume` | List, save, or resume conversation sessions. |
| `/save <tag>` | Save a checkpoint of the current session. |
| `/rewind` | Revert conversation history by one turn. |
| `/compact` / `/compress` | Compress session context to conserve token limits. |
| `/mcp` | Interactive Model Context Protocol server catalog (`/mcp catalog`, `/mcp add`, `/mcp reload`). |
| `/tasks` | Manage background tasks (`status`, `kill`, `send_input`). |
| `/stats` | View session metrics, KV Cache hit rates, and financial cost. |
| `/info` | Display system telemetry, version, and links. |
| `/update` | Check for updates or apply in-place atomically (`/update apply` / `/update force`). |
| `/clear` | Clear the terminal viewport. |
| `/help` | Display shortcuts and usage guidelines. |
| `/quit` / `/exit` | Terminate session and save state. |

---

## Core Developer Tools

Corex exposes native tools directly to the reasoning engine:

- **`apply_patch`:** Atomic unified diff patch tool with fuzzy whitespace tolerance, offset calculation, and CRLF/LF line ending preservation.
- **File System:** `read_file`, `write_file`, `smart_replace`, `list_directory`, `glob`, and `grep`.
- **`run_shell_command`:** Sandboxed shell execution with safety checks, adaptive background detachment, and sudo AskPass.
- **`manage_task`:** Background process manager supporting process inspection, signals, and standard input forwarding.
- **`web_search` & `web_fetch`:** Live search engine integration and clean HTML-to-markdown extraction.
- **`delegate_subagent` & `manage_subagents`:** Parallel delegation to specialized autonomous subagents (`scout`, `tester`, `researcher`, `worker`) with context isolation and reactive completion notifications.
- **`write_todos`:** Multi-step autonomous task tracking and verification.
- **Persistent Memory:** Context retention across runs via `./COREX.md` (project) and `~/.corex/COREX.md` (global).

---

## Configuration Files

Configuration and logs are stored globally in `~/.corex/` and can be overridden per workspace:

| Path | Description |
| :--- | :--- |
| `~/.corex/fleet.toml` | Global Fleet configuration: VPS nodes, clusters, SSH keys, jump hosts, and command whitelist. |
| `.corex/fleet.toml` | Workspace-level Fleet overrides for project-specific infrastructure nodes. |
| `~/.corex/system.md` | Global AI personality, operational rules, and behavioral guidelines injected into all sessions. |
| `.corex/system.md` | Workspace-level AI instructions and coding constraints. |
| `~/.corex/settings.json` | Global provider credentials, base URLs, default model, and allowed commands. |
| `.corex/settings.json` | Project-level workspace settings overrides (secrets automatically stripped before git). |
| `~/.corex/flash_settings.json` | CoT reasoning depth configuration for Flash. |
| `~/.corex/pro_settings.json` | Deep reasoning configuration for Pro. |
| `~/.corex/logs/` | Daily forensic audit logs (`corex-forensic-YYYY-MM-DD.log`, 7-day retention). |
| `~/.corex/locks/` | Advisory per-path concurrency file locks (`<hash>.lock`). |

---

## License & Commercial Licensing

- **Open Source License:** GNU Affero General Public License v3.0 ([AGPL-3.0-only](LICENSE)).
- **Author:** [sluisr](https://sluisr.com/) (`contact@sluisr.com`)
- **Website:** [https://corex.sluisr.com](https://corex.sluisr.com)
- **Discord:** [Join Corex Discord](https://discord.gg/MGMt6M6Yx)
- **Repository:** [https://github.com/sluisr/corex](https://github.com/sluisr/corex)
- **Dual Licensing:** If your organization requires embedding Corex within proprietary closed-source infrastructure or commercial SaaS without AGPL-3.0 copyleft obligations, custom commercial licenses and enterprise support agreements are available upon request. Contact `contact@sluisr.com`.
