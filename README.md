# ⚡ Corex (`cx`)

<p align="center">
  <strong>The ultra-fast, native Rust autonomous AI terminal agent for DeepSeek API & Local LLMs.</strong>
</p>

<p align="center">
  <a href="https://github.com/sluisr/corex/releases"><img src="https://img.shields.io/github/v/release/sluisr/corex?color=blue&label=version" alt="Release"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/language-Rust%202021-orange.svg" alt="Rust 2021"></a>
  <a href="https://platform.deepseek.com/"><img src="https://img.shields.io/badge/AI-DeepSeek%20V4.1%20Flash%20%7C%20Pro-blueviolet" alt="DeepSeek V4.1"></a>
  <a href="https://github.com/ggerganov/llama.cpp"><img src="https://img.shields.io/badge/Local%20LLM-llama.cpp%20%7C%20Ollama-green" alt="Local LLM"></a>
  <a href="https://github.com/sluisr/corex/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="License"></a>
  <a href="https://sluisr.com/"><img src="https://img.shields.io/badge/author-sluisr.com-purple" alt="Author"></a>
</p>

---

## 🚀 The Evolution: Beyond `deepseek-cli`

**Corex** (invoked with the ultra-ergonomic command **`cx`**) is the official next-generation, high-performance autonomous terminal agent created and maintained 100% by [**sluisr**](https://sluisr.com/).

While previous generation CLI tools were born as TypeScript adaptations of web/node CLIs, **Corex** has been completely re-architected from scratch in **pure Rust**. It breaks free from all runtime constraints, eliminates `node_modules` and Node.js runtime bloat, and delivers instant **5ms startup times**, **100% private offline Local LLM inference**, and **adaptive CoT reasoning**.

---

## ⚡ Why Corex?

* 🦀 **100% Native Rust Architecture:** Single standalone static binary. Instant terminal startup (< 10ms), single-hand ergonomic typing (`cx`), and minimal RAM footprint.
* 🔒 **100% Offline Local Inference (@ $0.00 Cost):** Connect any local GGUF model via `llama-server`, `llama.cpp`, or `Ollama` on port 8080. 100% air-gapped, zero telemetry, zero data leaves your machine, and $0.00 API costs forever.
* 🧠 **DeepSeek V4.1 Cloud Engine:** Full native support for `deepseek-flash` (DeepSeek-V4.1-Flash 522B vision-language MoE, native vision, 1M token context) and `deepseek-v4-pro` (Reasoning / Thinking CoT mode).
* ⚡ **Dynamic Adaptive Reasoning CoT:** Automatically uses ultra-fast reasoning (~200ms TTFT) for shell commands and system inspection, reserving deep multi-stage CoT (`high` / `xhigh` / `max`) for complex code generation and refactoring.
* 🛡️ **96%+ KV Cache Hit Rate:** Zero-invalidation architecture. Background tool-log compression keeps the KV cache intact, slashing DeepSeek API costs by up to ~90%.
* 🧩 **Fuzzy Diff Patch Engine (`apply_patch`):** Precise unified diff patching with fuzzy whitespace tolerance, accurate line offset calculation, and CRLF/LF line ending preservation without file rewrites.
* ⏳ **Real-Time Prompt Queuing:** Compose or paste follow-up prompts while the model is streaming. Queued prompts execute automatically in sequence once the active generation completes.
* 📋 **Native Clipboard Integration:** Integrated OSC 52 terminal copy sequences and platform clipboard support for seamless copying of code blocks and chat selections.
* 🔒 **Enterprise Sandbox & Protected Paths:** Path-level mutex locks (`lock_path`) to serialize concurrent file writes and strict path guards that prevent touching sensitive credentials (SSH, git, GPG) even in YOLO mode.
* 🔍 **Forensic Audit Telemetry:** Real-time tracking of Time-To-First-Token (TTFT), generation throughput (tokens/sec), exact financial costs ($USD), and KV cache savings.
* 🔑 **Silent Sudo & 0ms AskPass:** Native non-blocking zero-lag system authentication for Linux `sudo`, `git`, and SSH.
* 🔌 **Model Context Protocol (MCP):** Connect external database servers, GitHub tools, Docker controllers, and custom MCP integrations.

---

## ⚡ System Requirements & Resource Footprint

Because **Corex** is built 100% in native Rust with link-time optimization (LTO) and zero runtime overhead, its resource footprint is negligible compared to traditional Node.js or Python-based CLI assistants:

### 💻 Hardware & OS Requirements

| Component | Minimum Specification | Recommended |
| :--- | :--- | :--- |
| **RAM** | **64 MB** | **128 MB+** (with automatic Linux `malloc_trim` compaction) |
| **CPU** | Any 64-bit x86_64 or ARM64 processor (1 core) | Multi-core processor |
| **Disk Footprint** | **~4.2 MB** (download) / **~13 MB** (uncompressed binary) | 50 MB (with audit logs) |
| **Operating System** | • Linux (glibc 2.17+ or musl)<br>• macOS 11+ (Apple Silicon M1-M4 & Intel)<br>• Windows 10 / 11 (64-bit) | Any modern OS |
| **Terminal** | Any modern terminal with ANSI / 256-color support (Alacritty, Kitty, WezTerm, iTerm2, Windows Terminal, Foot) | Truecolor (24-bit) terminal |

### 📊 Real-World Performance & Memory Benchmarks

| Metric | Corex (Pure Native Rust) | Typical Node.js / TS CLIs | Electron / Webview CLIs |
| :--- | :--- | :--- | :--- |
| **Download Size** | **~3.8 – 4.2 MB** (`.tar.gz` / `.zip`) | ~80 – 150 MB (with `node_modules`) | ~180 – 350 MB |
| **Executable Size** | **~13 MB** (single static binary) | Multi-file tree + Node.js runtime | Multi-file bundle + Chromium engine |
| **RAM Footprint (Idle)** | **~21.5 MB RSS** | ~120 – 220 MB | ~350 – 600 MB |
| **RAM Footprint (Active LLM Streaming)** | **~25 – 35 MB RSS** | ~180 – 350 MB | ~500 – 850 MB |
| **Cold Startup Time** | **< 10 ms** (instant) | ~600 – 1,400 ms | ~2,000 – 4,000 ms |
| **Idle CPU Utilization** | **< 0.1%** | ~1 – 3% (event loop polling) | ~3 – 8% (render loop) |
| **External Runtime Dependencies** | **None** (zero dependencies) | Node.js >= 18, npm/pnpm | Electron runtime, V8 engine |

---

## 🎯 2 Flexible Operating Modes

Corex offers 2 distinct execution paradigms switchable in real-time via `/model`:

```text
                               ┌─────────────────────────────┐
                               │    COREX OPERATING MODES    │
                               └──────────────┬──────────────┘
                                              │
                      ┌───────────────────────┴───────────────────────┐
                      ▼                                               ▼
           ┌──────────────────────┐                       ┌──────────────────────┐
           │     1. CLOUD API     │                       │   2. OFFLINE LOCAL   │
           │  (DeepSeek V4.1 API) │                       │ (llama-server / SLM) │
           └──────────────────────┘                       └──────────────────────┘
```

### 1. ☁️ Mode 1: Cloud API Models (DeepSeek Cloud)
* **Engines:** `deepseek-flash` (DeepSeek-V4.1-Flash multimodal with native vision & 1M context) and `deepseek-v4-pro` (Reasoning / Thinking CoT architecture).
* **Behavior:** High-throughput streaming, adaptive reasoning depth, and full tool autonomy powered by DeepSeek's cloud infrastructure.
* **Best For:** Complex full-stack coding, deep architectural refactoring, and multi-file project implementation.

### 2. 🔒 Mode 2: 100% Offline Local LLM (Air-Gapped & Private)
* **Engines:** Any local GGUF model via `llama-server` (e.g. `Llama-3.2-3B`, `Qwen-2.5-Coder`, `Mistral`) or `Ollama` on `http://127.0.0.1:8080/v1`.
* **Behavior:** 100% air-gapped offline execution. Zero telemetry, zero network calls, and **$0.00 API cost forever**.
* **Best For:** Confidential codebases, air-gapped environments, offline work, and local experimentation.

---

## 📦 Installation

### Option 1: From Source / Cargo (Recommended)

```bash
cargo install --git https://github.com/sluisr/corex.git --force
```

### Option 2: Pre-compiled Binary (Linux / macOS)

Install instantly via `curl`:

```bash
curl -fsSL https://raw.githubusercontent.com/sluisr/corex/main/install.sh | sh
```

Or download pre-built binaries directly for Linux, macOS, and Windows from the [GitHub Releases page](https://github.com/sluisr/corex/releases).

### Option 3: Local Clone & Build

```bash
git clone https://github.com/sluisr/corex.git
cd corex
cargo build --release
sudo cp target/release/cx /usr/local/bin/
```

### Option 4: Via npm / npx

```bash
# Run instantly without installing:
npx corex

# Or install globally:
npm install -g corex
```

---

## 🔐 Authentication & Setup

Get your DeepSeek API key from [platform.deepseek.com/api_keys](https://platform.deepseek.com/api_keys) and export it:

```bash
export COREX_API_KEY="sk-your-deepseek-api-key"
# or
export DEEPSEEK_API_KEY="sk-your-deepseek-api-key"
```

Or configure it interactively inside Corex by typing `/auth` on first launch.

### Optional: Local LLM Server Setup (for Offline Mode)

Run `llama-server` (from `llama.cpp`) or `Ollama` on port 8080:

```bash
llama-server -m models/Llama-3.2-3B-Instruct-Q4_K_M.gguf --port 8080 -c 8192
```

---

## 💻 Usage

### Interactive TUI Mode

Launch the interactive terminal UI in any project directory with the lightning-fast command `cx` (or alias `corex`):

```bash
cd my-project/
cx
```

### Non-Interactive / Scripting Mode

Execute automated one-shot tasks directly from bash:

```bash
# Run a direct prompt headlessly
cx -p "Analyze this Rust project and run all unit tests"

# Perform live web search
cx -w "Latest DeepSeek API updates"

# Run with auto-approval (YOLO mode)
cx -y -p "Format all code and commit changes"

# Target a specific model override
cx -m deepseek-v4-pro -p "Solve this mathematical proof"
```

---

## ⚡ Slash Commands Reference

Inside the interactive TUI, type `/` to access built-in commands:

| Command | Description |
| :--- | :--- |
| `/model` | Interactive TUI selector for Cloud Models (`Flash` / `Pro`) and Local Offline Assistant. |
| `/auth` | Manage and update your DeepSeek API key securely. |
| `/balance` | Live DeepSeek account token credits and currency balance lookup (`/wallet`). |
| `/fim <file>` | Fill-in-the-Middle code autocompletion at cursor position. |
| `/local <prompt>` | Query the local offline LLM directly ($0.00 cost) or check local status (`/local status`). |
| `/web <query>` | Search the live internet via DeepSeek's native search engine (`/search`). |
| `/yolo` | Toggle auto-approval of all tool executions (`/yolo [on \| off]`). |
| `/plan` | Enter interactive architectural planning mode (read-only safe discovery). |
| `/prefix <text>` | Force exact output formatting with zero conversational filler. |
| `/chat` / `/resume` | Search, save, and resume previous conversation sessions. |
| `/save <tag>` | Save current conversation checkpoint with an optional tag. |
| `/rewind` | Rewind conversation history by 1 turn (undo last query and response). |
| `/compact` / `/compress` | Intelligently compress conversation context to stay within token limits. |
| `/mcp` | Inspect and manage Model Context Protocol servers and connected tools. |
| `/tasks` | Inspect and manage background tasks (`/tasks status <pid>`, `/tasks kill <pid>`). |
| `/stats` | View session token metrics, cache hit rate, and financial costs. |
| `/info` | Version, creator credits (`sluisr`), official links, and live telemetry. |
| `/update` | Check for newer Corex releases on GitHub. |
| `/clear` | Clear the terminal conversation view. |
| `/help` | Display keyboard shortcuts and built-in commands. |
| `/quit` / `/exit` | Exit the session and save state for resumption. |

---

## 🛠️ Built-in Agent Tools

Corex equips the AI with native developer tools for autonomous development:

* **⚡ `apply_patch`:** Intelligent unified diff atomic patching with fuzzy whitespace tolerance, CRLF/LF line ending preservation, and fence parsing.
* **📁 File Operations:** `read_file`, `write_file`, `smart_replace`, `list_directory`, `glob`, and `grep`.
* **💻 Shell Execution (`run_shell_command` / `run_command`):** Autonomous bash command execution with security sandboxing, adaptive timeout (`wait_ms_before_async`, default 5000ms), automatic detachment to background tasks, and silent AskPass for `sudo`.
* **⚙️ Background Task Management (`manage_task`):** Unified task controller matching Antigravity architecture (`list`, `status`, `kill`, `send_input`).
* **🌐 Web Search (`web_search`) & Fetch (`web_fetch`):** Real-time web search and HTTP fetching with clean markdown extraction for documentation and APIs.
* **📋 Task Tracking (`write_todos`):** Dynamic multi-step task list tracking and progress monitoring.
* **🧠 Persistent Memory:** Project-level (`./COREX.md`) and global (`~/.corex/COREX.md`) persistent context.

---

## 📊 Forensic Audit Logging

Corex logs complete telemetry to `~/.corex/logs/corex-forensic-YYYY-MM-DD.log`:

```text
[2026-10-09 14:43:18.542][LLM_RESP    ] ─── INBOUND <- DeepSeek Cloud (deepseek-flash) [1613 ms] ───
Telemetry:
  • Engine:              DeepSeek Cloud
  • Model:               deepseek-flash (DeepSeek-V4.1-Flash)
  • TTFT (First Token):  722 ms
  • Total Duration:      1613 ms
  • Output Speed:        78.7 tokens/sec
  • Finish Reason:       stop
  • Output Characters:   341

Token & Cost Forensics:
  • Prompt Tokens:       4417 (Cached: 4224 · 95.6% Hit Rate)
  • Completion Tokens:   127
  • Total Tokens:        4544
  • Actual Cost:         $0.000122 USD
  • KV Cache Savings:    $0.000532 USD (81.4% discount)
───────────────────────────────────────────────────────────────────────────────────────────────
```

---

## ⚙️ Configuration

Settings are stored in `~/.corex/`:

* `~/.corex/settings.json` — API credentials, base URL, default models.
* `~/.corex/flash_settings.json` — CoT reasoning effort depths (`none` / `low` / `high` / `xhigh` / `max`).
* `~/.corex/pro_settings.json` — Deep reasoning configuration for DeepSeek-V4-Pro.
* `~/.corex/logs/` — Forensic audit logs.

---

## 🌐 Community & Author

* **Developer:** [sluisr](https://sluisr.com/)
* **Website:** [https://sluisr.com](https://sluisr.com/)
* **GitHub:** [https://github.com/sluisr](https://github.com/sluisr)
* **YouTube:** [https://www.youtube.com/@sluisr_](https://www.youtube.com/@sluisr_)
* **Repository:** [https://github.com/sluisr/corex](https://github.com/sluisr/corex)

---

## 📜 License

Licensed under the [Apache License, Version 2.0](LICENSE).  
Copyright (c) 2026 **sluisr**. Built with 🦀 in Rust.
