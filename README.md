# 🦀 UTI CLI (Universal Terminal Intelligence)

<p align="center">
  <strong>The ultra-fast, native Rust autonomous AI terminal agent for DeepSeek API & Local LLMs with Hybrid Intelligence.</strong>
</p>

<p align="center">
  <a href="https://github.com/sluisr/uti-cli/releases"><img src="https://img.shields.io/github/v/release/sluisr/uti-cli?color=blue&label=version" alt="Release"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/language-Rust%202021-orange.svg" alt="Rust 2021"></a>
  <a href="https://platform.deepseek.com/"><img src="https://img.shields.io/badge/AI-DeepSeek%20V4.1%20Flash%20%7C%20Pro-blueviolet" alt="DeepSeek V4.1"></a>
  <a href="https://github.com/ggerganov/llama.cpp"><img src="https://img.shields.io/badge/Local%20LLM-llama.cpp%20%7C%20Ollama-green" alt="Local LLM"></a>
  <a href="https://github.com/sluisr/uti-cli/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="License"></a>
  <a href="https://sluisr.com/"><img src="https://img.shields.io/badge/author-sluisr.com-purple" alt="Author"></a>
</p>

---

## 🚀 The Evolution: Beyond `deepseek-cli`

**UTI CLI** is the official next-generation, high-performance successor to [`deepseek-cli`](https://github.com/sluisr/deepseek-cli), created and maintained 100% by [**sluisr**](https://sluisr.com/).

While `deepseek-cli` was born as a TypeScript adaptation of Google's Gemini CLI, **UTI CLI** has been completely re-architected from scratch in **pure Rust**. It breaks free from all Google legacy upstream constraints, eliminates `node_modules` and Node.js runtime bloat, and delivers instant **5ms startup times**, **zero-cost Local LLM hybrid routing**, and **adaptive CoT reasoning**.

---

## ⚡ Why UTI CLI?

* 🦀 **100% Native Rust Architecture:** Single standalone static binary. Zero Node.js runtime, zero `npm` dependencies, instant terminal startup (< 10ms), and minimal RAM footprint.
* 💰 **Hybrid Intelligence (@ $0.00 Local Routing):** Seamlessly pairs DeepSeek Cloud with your local SLM (`llama-server`, `llama.cpp`, or `Ollama`) on port 8080. Non-coding questions and greetings are handled locally at **$0.00**, saving up to 70% in API costs.
* 🧠 **DeepSeek V4.1 Cloud Engine:** Full native support for `deepseek-flash` (DeepSeek-V4.1-Flash 522B vision-language MoE, native vision, 1M token context) and `deepseek-v4-pro` (Reasoning / Thinking CoT mode).
* ⚡ **Dynamic Adaptive Reasoning CoT:** Automatically uses ultra-fast reasoning (~200ms TTFT) for shell commands and system inspection, reserving deep multi-stage CoT (`high` / `xhigh` / `max`) for complex code generation and refactoring.
* 🛡️ **96%+ KV Cache Hit Rate:** Zero-invalidation architecture. Background tool-log compression keeps the KV cache intact, slashing DeepSeek API costs by up to ~90%.
* 📝 **Atomic Code Patching (`apply_patch`):** Precise unified diff patching for token-efficient file modifications without rewrites.
* 🔍 **Forensic Audit Telemetry:** Real-time tracking of Time-To-First-Token (TTFT), generation throughput (tokens/sec), exact financial costs ($USD), and KV cache savings.
* 🔑 **Silent Sudo & 0ms AskPass:** Native non-blocking zero-lag system authentication for Linux `sudo`, `git`, and SSH.
* 🔌 **Model Context Protocol (MCP):** Connect external database servers, GitHub tools, Docker controllers, and custom MCP integrations.

---

## ⚡ System Requirements & Resource Footprint

Because **UTI CLI** is built 100% in native Rust with link-time optimization (LTO) and zero runtime overhead, its resource footprint is negligible compared to traditional Node.js or Python-based CLI assistants:

### 💻 Hardware & OS Requirements

| Component | Minimum Specification | Recommended |
| :--- | :--- | :--- |
| **RAM** | **64 MB** | **128 MB+** |
| **CPU** | Any 64-bit x86_64 or ARM64 processor (1 core) | Multi-core processor |
| **Disk Footprint** | **~4.2 MB** (download) / **~13 MB** (uncompressed binary) | 50 MB (with audit logs) |
| **Operating System** | • Linux (glibc 2.17+ or musl)<br>• macOS 11+ (Apple Silicon M1-M4 & Intel)<br>• Windows 10 / 11 (64-bit) | Any modern OS |
| **Terminal** | Any modern terminal with ANSI / 256-color support (Alacritty, Kitty, WezTerm, iTerm2, Windows Terminal, Foot) | Truecolor (24-bit) terminal |

### 📊 Real-World Performance & Memory Benchmarks

| Metric | UTI CLI (Pure Native Rust) | Typical Node.js / TS CLIs | Electron / Webview CLIs |
| :--- | :--- | :--- | :--- |
| **Download Size** | **~3.8 – 4.2 MB** (`.tar.gz` / `.zip`) | ~80 – 150 MB (with `node_modules`) | ~180 – 350 MB |
| **Executable Size** | **~13 MB** (single static binary) | Multi-file tree + Node.js runtime | Multi-file bundle + Chromium engine |
| **RAM Footprint (Idle)** | **~21.5 MB RSS** | ~120 – 220 MB | ~350 – 600 MB |
| **RAM Footprint (Active LLM Streaming)** | **~25 – 35 MB RSS** | ~180 – 350 MB | ~500 – 850 MB |
| **Cold Startup Time** | **< 10 ms** (instant) | ~600 – 1,400 ms | ~2,000 – 4,000 ms |
| **Idle CPU Utilization** | **< 0.1%** | ~1 – 3% (event loop polling) | ~3 – 8% (render loop) |
| **External Runtime Dependencies** | **None** (zero dependencies) | Node.js >= 18, npm/pnpm | Electron runtime, V8 engine |

---

## 🎯 3 Flexible Operating Modes

UTI CLI offers 3 distinct execution modes switchable in real-time via `/model`:

```text
                               ┌─────────────────────────────┐
                               │   UTI CLI OPERATING MODES   │
                               └──────────────┬──────────────┘
                                              │
         ┌────────────────────────────────────┼────────────────────────────────────┐
         ▼                                    ▼                                    ▼
┌──────────────────┐               ┌──────────────────────┐               ┌──────────────────┐
│   1. CLOUD API   │               │   2. OFFLINE LOCAL   │               │    3. HYBRID     │
│  (DeepSeek API)  │               │ (llama-server / SLM) │               │  (Cloud + Local) │
└──────────────────┘               └──────────────────────┘               └────────┬─────────┘
                                                                                   │
                                         ┌────────────────────┬────────────────────┼────────────────────┐
                                         ▼                    ▼                    ▼                    ▼
                                  [ Auto-Triage ]      [ Local Scout ]     [ Draft & Review ]  [ Compression Only ]
```

### 1. ☁️ Mode 1: 100% Cloud API Models (DeepSeek Cloud)
* **Engines:** `deepseek-flash` (DeepSeek-V4.1-Flash multimodal with native vision & 1M context), `deepseek-v4-pro` (Reasoning / Thinking CoT), with backward-compatibility for `deepseek-v4-flash` and `deepseek-v4-flash-vision-exp`.
* **Behavior:** All turns, reasoning, and tool calls run directly against the high-capacity DeepSeek Cloud API.
* **Best For:** Heavy architectural redesigns, complex multi-file coding, and maximum AI capability.

### 2. 🔒 Mode 2: 100% Offline Local LLM (Air-Gapped & Private)
* **Engines:** Any local GGUF model via `llama-server` (e.g. `Llama-3.2-3B`, `Qwen-2.5-Coder`, `Mistral`) or `Ollama` on `http://127.0.0.1:8080/v1`.
* **Behavior:** 100% air-gapped offline execution. Zero data leaves your machine, zero API calls, **$0.00 cost forever**.
* **Best For:** Confidential codebases, offline travel/airplane coding, and private terminal exploration.

### 3. ⚡ Mode 3: Hybrid Architecture (Cloud API + Local Assistant)
Combines the raw coding intelligence of DeepSeek Cloud with the instant speed and zero-cost of your local SLM.

Inside Hybrid Mode, you can choose between **4 specialized strategies**:

```text
                  ┌──────────────────────────────────────────────┐
                  │               USER PROMPT                    │
                  └──────────────────────┬───────────────────────┘
                                         │
                        [ 0ms Heuristic Intent Triage ]
                                         │
                 ┌───────────────────────┴───────────────────────┐
                 ▼                                               ▼
     [ Pure Chat / Theory / Q&A ]                    [ Coding / Patching / Tools ]
                 │                                               │
                 ▼                                               ▼
    ┌─────────────────────────┐                     ┌─────────────────────────┐
    │     LOCAL ASSISTANT     │                     │     DEEPSEEK CLOUD      │
    │  (llama-server @ $0.00) │                     │    (deepseek-flash)     │
    └─────────────────────────┘                     └─────────────────────────┘
```

| Hybrid Strategy | How It Works | Best For | Cost Impact |
| :--- | :--- | :--- | :--- |
| **1. Auto-Triage** *(Default & Recommended)* | Fast 0ms triage: routes chat, questions, and developer queries to Local LLM ($0.00); routes code generation, patches, and tools to DeepSeek Cloud. | Daily full-stack software development | **Maximum Savings ($0.00 chat)** |
| **2. Local Scout** | Local SLM explores the repository using `grep`/`glob`, summarizes findings into dense context, and feeds it to DeepSeek Cloud for the final patch. | Large monolithic repositories & large codebases | **Saves up to 80% prompt tokens** |
| **3. Draft & Review** | Local SLM writes the initial code draft, then DeepSeek Cloud reviews, optimizes, and executes atomic patches. | High-volume code generation | **Fast & Cost-balanced** |
| **4. Compression Only** | DeepSeek handles all tasks, while the Local SLM compresses older tool outputs and conversation turns in background threads without blocking. | Multi-hour autonomous sessions | **Maintains 96%+ KV Cache hit** |

---

## 📦 Installation

### Option 1: Cargo (Recommended)

```bash
cargo install uti-cli
```

### Option 2: Pre-compiled Binary (Linux / macOS / Windows)

Download the latest release binary from the [Releases page](https://github.com/sluisr/uti-cli/releases) or install via `curl`:

```bash
curl -fsSL https://raw.githubusercontent.com/sluisr/uti-cli/main/install.sh | sh
```

### Option 3: Build from Source

```bash
git clone https://github.com/sluisr/uti-cli.git
cd uti-cli
cargo build --release
sudo cp target/release/uti /usr/local/bin/
```

---

## 🔐 Authentication & Setup

Get your DeepSeek API key from [platform.deepseek.com/api_keys](https://platform.deepseek.com/api_keys) and export it:

```bash
export DEEPSEEK_API_KEY="sk-your-deepseek-api-key"
```

Or configure it interactively inside UTI CLI by typing `/auth` on first launch.

### Optional: Local LLM Server Setup (for Hybrid Mode)

Run `llama-server` (from `llama.cpp`) or `Ollama` on port 8080:

```bash
llama-server -m models/Llama-3.2-3B-Instruct-Q4_K_M.gguf --port 8080 -c 8192
```

---

## 💻 Usage

### Interactive TUI Mode

Launch the interactive terminal UI in any project directory:

```bash
cd my-project/
uti
```

### Non-Interactive / Scripting Mode

Execute automated one-shot tasks directly from bash:

```bash
uti -p "Analyze this Rust project and run all unit tests"
```

---

## ⚡ Slash Commands Reference

Inside the interactive TUI, type `/` to access built-in commands:

| Command | Description |
| :--- | :--- |
| `/model` | Interactive TUI selector for Cloud Models, Local Assistant, and Hybrid Configuration. |
| `/auth` | Manage and update your DeepSeek API key securely. |
| `/balance` | Live DeepSeek account balance lookup (`/wallet`, `/credits`). |
| `/fim <file>` | Fill-in-the-Middle code autocompletion. |
| `/prefix <text>` | Force exact output formatting with zero conversational filler. |
| `/info` | Version, author credits, and system telemetry (`/author`, `/credits`). |
| `/chat` / `/resume` | Search and resume previous conversation sessions. |
| `/rewind` | Rewind conversation history to a previous turn. |
| `/compress` | Manually compress conversation context to stay within token limits. |
| `/mcp` | Manage Model Context Protocol (MCP) servers and tools. |
| `/tasks` | Inspect and manage background tasks (`/tasks`, `/tasks status <pid>`, `/tasks kill <pid>`). |
| `/plan` | Enter interactive planning mode for multi-file architectural changes. |
| `/stats` | View session token metrics, cache hit rate, and financial costs. |
| `/clear` | Clear the terminal conversation view. |
| `/quit` / `/exit` | Exit the session and save state for resumption. |

---

## 🛠️ Built-in Agent Tools

UTI CLI equips DeepSeek with native developer tools for autonomous development:

* **⚡ `apply_patch`:** Unified diff atomic patching for safe, token-efficient code edits.
* **📁 File Operations:** `read_file`, `write_file`, `smart_replace`, `list_directory`, `glob`, and `grep`.
* **💻 Shell Execution (`run_shell_command` / `run_command`):** Autonomous bash command execution with adaptive execution timeout (`wait_ms_before_async`, default 5000ms). Fast commands return immediately, while long commands automatically detach to background tasks with silent AskPass for `sudo`.
* **⚙️ Background Task Management (`manage_task`):** Unified task controller matching Antigravity architecture (`list`, `status`, `kill`, `send_input`).
* **🌐 Web Fetch (`web_fetch`):** HTTP fetching and markdown extraction for online documentation and APIs.
* **📋 Task Tracking (`write_todos`):** Dynamic multi-step task list tracking and progress monitoring.
* **🧠 Persistent Memory:** Project-level (`./UTI.md`) and global (`~/.uti/UTI.md`) persistent context.

---

## 📊 Forensic Audit Logging

UTI CLI logs complete telemetry to `~/.uti/logs/uti-forensic-YYYY-MM-DD.log`:

```text
[2026-08-30 14:43:18.542][LLM_RESP    ] ─── INBOUND <- DeepSeek Cloud (deepseek-flash) [1613 ms] ───
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

Settings are stored in `~/.uti/`:

* `~/.uti/settings.json` — API credentials, base URL, default models.
* `~/.uti/flash_settings.json` — CoT reasoning effort depths (`none` / `low` / `high` / `xhigh` / `max`).
* `~/.uti/hybrid_settings.json` — Strategy, local server endpoint, scout settings.
* `~/.uti/logs/` — Forensic audit logs.

---

## 🌐 Community & Author

* **Developer:** [sluisr](https://sluisr.com/)
* **Website:** [https://sluisr.com](https://sluisr.com/)
* **GitHub:** [https://github.com/sluisr](https://github.com/sluisr)
* **YouTube:** [https://www.youtube.com/@sluisr_](https://www.youtube.com/@sluisr_)
* **Repository:** [https://github.com/sluisr/uti-cli](https://github.com/sluisr/uti-cli)

---

## 📜 License

Licensed under the [Apache License, Version 2.0](LICENSE).  
Copyright (c) 2026 **sluisr**. Built with 🦀 in Rust.
