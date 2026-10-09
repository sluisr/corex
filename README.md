# Corex (`cx`)

> High-performance autonomous terminal agent for DeepSeek API and local LLMs, written in native Rust.

[![Release](https://img.shields.io/github/v/release/sluisr/corex?style=flat-square&color=2563eb&label=release)](https://github.com/sluisr/corex/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/sluisr/corex/ci.yml?style=flat-square&label=ci)](https://github.com/sluisr/corex/actions)
[![Rust](https://img.shields.io/badge/rust-2021-f97316?style=flat-square)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue?style=flat-square)](LICENSE)

---

## Overview

**Corex** (invoked as `cx` or `corex`) is a native terminal coding agent architected for software engineers who require low latency, deterministic execution, and air-gapped privacy.

Unlike traditional terminal assistants built on Node.js or Python runtimes, Corex is engineered from the ground up in pure Rust. It delivers sub-10ms startup times, runs as a single self-contained static binary, and consumes negligible system memory.

---

## Key Highlights

- **Native Systems Architecture:** Compiled static binary (~6 MB stripped) with zero external runtime dependencies. Instant startup (<10ms) and minimal RAM utilization (~21 MB RSS).
- **DeepSeek V4.1 Cloud Engine:** Full support for `deepseek-flash` (522B vision-language MoE, 1M context) and `deepseek-v4-pro` (CoT deep reasoning architecture).
- **Air-Gapped Offline Inference:** Direct integration with `llama-server` (llama.cpp) and `Ollama` on `http://127.0.0.1:8080/v1` for 100% private, zero-cost ($0.00) execution.
- **Real-Time Prompt Queuing:** Type or paste follow-up prompts while the model is streaming responses. Queued items run sequentially without interrupting generation.
- **Fuzzy Diff Patch Engine (`apply_patch`):** Precise unified diff patching with whitespace tolerance, accurate line offset calculation, and CRLF/LF line ending preservation.
- **Enterprise Sandbox & Path Guards:** Path-level concurrency locking (`lock_path`) and strict protection of sensitive files (SSH keys, GPG credentials, git configs) even in YOLO mode.
- **Native Clipboard Integration:** Seamless terminal text copying via OSC 52 sequences and platform clipboard integration.
- **KV Cache Preservation:** Context management and tool compression maintain up to a 96%+ KV Cache hit rate, reducing DeepSeek API token costs by up to ~90%.
- **Forensic Telemetry:** Real-time tracking of Time-To-First-Token (TTFT), tokens/sec throughput, cache hit percentage, and exact financial costs ($USD).
- **Non-Blocking Authentication:** Silent AskPass integration for Linux `sudo`, git, and SSH commands.
- **Model Context Protocol (MCP):** Compatible with standard MCP servers (database connectors, GitHub tools, Docker controllers).

---

## Performance & Resource Footprint

Corex is compiled with Link-Time Optimization (`lto = "fat"`), single codegen units, and automatic OS memory compaction (`malloc_trim` on Linux) to minimize memory fragmentation.

### Hardware & OS Specifications

| Component | Minimum Specification | Recommended |
| :--- | :--- | :--- |
| **RAM** | 64 MB | 128 MB+ |
| **CPU** | Any 64-bit x86_64 or ARM64 processor | Multi-core processor |
| **Disk Footprint** | ~5 MB (download) / ~6 MB (binary) | 50 MB (including logs) |
| **Operating System** | Linux (glibc 2.17+ or musl), macOS 11+ (Apple Silicon & Intel), Windows 10/11 | Any modern OS |
| **Terminal** | ANSI / 256-color terminal | Truecolor (24-bit) terminal |

### Comparative Benchmarks

| Metric | Corex (Native Rust) | Node.js / TS CLIs | Electron CLIs |
| :--- | :--- | :--- | :--- |
| **Download Size** | **~4.5 – 5.2 MB** | ~80 – 150 MB | ~180 – 350 MB |
| **Binary Size** | **~6.1 MB** (static stripped) | Multi-file tree + Node | Chromium + Node bundle |
| **Memory (Idle)** | **~21.5 MB RSS** | ~120 – 220 MB | ~350 – 600 MB |
| **Memory (Active Streaming)** | **~25 – 35 MB RSS** | ~180 – 350 MB | ~500 – 850 MB |
| **Cold Startup Latency** | **< 10 ms** | ~600 – 1,400 ms | ~2,000 – 4,000 ms |
| **Idle CPU Utilization** | **< 0.1%** | ~1 – 3% | ~3 – 8% |
| **External Dependencies** | **None** | Node.js >= 18, npm | Node, Chromium |

---

## Operating Modes

Corex supports two independent execution paradigms, selectable at runtime via `/model`:

```text
                           ┌─────────────────────────────┐
                           │    Corex Operating Modes    │
                           └──────────────┬──────────────┘
                                          │
                  ┌───────────────────────┴───────────────────────┐
                  ▼                                               ▼
       ┌──────────────────────┐                       ┌──────────────────────┐
       │     1. Cloud API     │                       │   2. Offline Local   │
       │  (DeepSeek V4.1 API) │                       │ (llama-server / SLM) │
       └──────────────────────┘                       └──────────────────────┘
```

### 1. Cloud API Models (DeepSeek Cloud)
- **Engines:** `deepseek-flash` (DeepSeek-V4.1-Flash multimodal with vision & 1M context) and `deepseek-v4-pro` (Reasoning / Thinking CoT architecture).
- **Behavior:** High-throughput streaming with adaptive reasoning effort per task complexity.
- **Best For:** Complex multi-file refactoring, full-stack implementation, and heavy reasoning tasks.

### 2. Offline Local LLM (Air-Gapped & Private)
- **Engines:** Any GGUF model served via `llama-server` (llama.cpp) or `Ollama` on `http://127.0.0.1:8080/v1`.
- **Behavior:** 100% offline, air-gapped execution. Zero network calls, zero telemetry, and $0.00 API cost.
- **Best For:** Proprietary codebases, air-gapped environments, and offline development.

---

## Installation

### Method 1: Pre-compiled Binary (Recommended for Linux & macOS)

Install via the official installation script:

```bash
curl -fsSL https://raw.githubusercontent.com/sluisr/corex/main/install.sh | sh
```

Or download binaries manually from the [GitHub Releases](https://github.com/sluisr/corex/releases) page for:
- Linux (`x86_64-unknown-linux-gnu`)
- macOS Apple Silicon (`aarch64-apple-darwin`)
- macOS Intel (`x86_64-apple-darwin`)
- Windows (`x86_64-pc-windows-msvc`)

### Method 2: From Source via Cargo

```bash
cargo install --git https://github.com/sluisr/corex.git --force
```

### Method 3: Via npm / npx

```bash
# Run headlessly or interactively without installation:
npx @sluisr/corex

# Or install globally:
npm install -g @sluisr/corex
```

### Method 4: Build from Source

```bash
git clone https://github.com/sluisr/corex.git
cd corex
cargo build --release
sudo cp target/release/cx /usr/local/bin/
```

---

## Authentication & Configuration

### Cloud API Credentials

Obtain an API key from [platform.deepseek.com](https://platform.deepseek.com/api_keys) and export it:

```bash
export COREX_API_KEY="sk-your-deepseek-api-key"
# or
export DEEPSEEK_API_KEY="sk-your-deepseek-api-key"
```

You can also configure your credentials interactively within the TUI by typing `/auth`.

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
```

---

## Slash Commands

Within the interactive TUI, type `/` to access built-in commands:

| Command | Description |
| :--- | :--- |
| `/model` | Select active model (`Flash`, `Pro`, or `Local Offline Assistant`). |
| `/auth` | Configure API credentials securely. |
| `/balance` | Check current account credits and token balance asynchronously (`/wallet`). |
| `/fim <file>` | Fill-in-the-Middle code completion at cursor location. |
| `/local <prompt>` | Query local offline LLM directly ($0.00 cost) or check status (`/local status`). |
| `/web <query>` | Query the internet using DeepSeek native search engine (`/search`). |
| `/yolo` | Toggle automatic tool approval (`/yolo [on \| off]`). |
| `/plan` | Enter read-only architectural planning mode. |
| `/prefix <text>` | Enforce specific response prefixes for strict formatting. |
| `/chat` / `/resume` | List, save, or resume conversation sessions. |
| `/save <tag>` | Save a checkpoint of the current session. |
| `/rewind` | Revert conversation history by one turn. |
| `/compact` / `/compress` | Compress session context to conserve token limits. |
| `/mcp` | Inspect and reload Model Context Protocol servers. |
| `/tasks` | Manage background tasks (`status`, `kill`, `send_input`). |
| `/stats` | View session metrics, KV Cache hit rates, and financial cost. |
| `/info` | Display system telemetry, version, and links. |
| `/update` | Check for newer releases on GitHub. |
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
- **`write_todos`:** Multi-step autonomous task tracking and verification.
- **Persistent Memory:** Context retention across runs via `./COREX.md` (project) and `~/.corex/COREX.md` (global).

---

## Configuration Files

Configuration and logs are stored under `~/.corex/`:

| Path | Description |
| :--- | :--- |
| `~/.corex/settings.json` | API keys, base URLs, and default engine configuration. |
| `~/.corex/flash_settings.json` | CoT reasoning depth configuration for Flash. |
| `~/.corex/pro_settings.json` | Deep reasoning configuration for Pro. |
| `~/.corex/logs/` | Daily forensic audit logs (`corex-forensic-YYYY-MM-DD.log`). |

---

## License & Commercial Licensing

- **Open Source License:** GNU Affero General Public License v3.0 ([AGPL-3.0-only](LICENSE)).
- **Author:** [sluisr](https://sluisr.com/) (`contact@sluisr.com`)
- **Repository:** [https://github.com/sluisr/corex](https://github.com/sluisr/corex)
- **Dual Licensing:** If your organization requires embedding Corex within proprietary closed-source infrastructure or commercial SaaS without AGPL-3.0 copyleft obligations, custom commercial licenses and enterprise support agreements are available upon request. Contact `contact@sluisr.com`.
