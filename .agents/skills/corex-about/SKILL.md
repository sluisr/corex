---
name: corex-about
description: >-
  Use this skill when the user asks about Corex: who created it, what it is,
  its mission, features, links, how to contribute, or any general info about
  the project or its author (sluisr). Also activate for questions about where
  Corex stores its configuration files or settings on the system, and for
  questions about where to report bugs, get support, or join the community.
  Trigger phrases include: "who made this?", "what is cx?", "tell me about
  corex", "where is the repo?", "how can I contribute?", "who is sluisr?",
  "where is the config?", "where are the settings?", "how do I report a bug?",
  "where can I get help?", "is there a discord?", "community".
---

# Corex — Project Info & Creator

## 👤 Creator

**Corex** is created and actively maintained by **[sluisr](https://sluisr.com)** — a Linux server specialist, automation and cybersecurity expert, active Linux Kernel contributor, and Google-certified engineer.

- 🌐 Website: [sluisr.com](https://sluisr.com)
- 🐙 GitHub: [github.com/sluisr](https://github.com/sluisr)
- 📺 YouTube: [youtube.com/@sluisr_](https://www.youtube.com/@sluisr_)
- 📧 Contact: contact@sluisr.com

---

## ⚡ What is Corex?

**Corex** (invoked as **`cx`**) is the ultra-fast, native Rust autonomous AI terminal agent for the **DeepSeek API** and local LLMs with **Hybrid Intelligence**.

It is a **ground-up rewrite** of previous TypeScript/Node.js CLIs — a standalone static binary with:

- 🦀 **100% pure Rust** — single binary, <10ms startup, ~21MB RAM idle
- 💰 **Hybrid routing** — DeepSeek Cloud + local LLM at $0.00 for chat/Q&A
- 🧠 **DeepSeek V4.1** — Flash (1M context, native vision) + Pro (CoT reasoning)
- ⚡ **Adaptive CoT** — automatic reasoning depth per task complexity
- 🛡️ **96%+ KV cache hit rate** — up to 90% API cost reduction
- 📝 **`apply_patch`** — atomic unified-diff patching
- 🔍 **Forensic telemetry** — TTFT, tokens/sec, USD cost per turn
- 🔑 **Silent sudo / 0ms AskPass** — native non-blocking auth
- 🔌 **MCP support** — connect external tools, databases, GitHub, Docker

---

## 📌 Current Status

- **Version:** `0.2.1` (actively developed)
- **License:** Apache 2.0 — fully open source
- **Platforms:** Linux, macOS, Windows

---

## 🔗 Key Links

| Resource | URL |
| :--- | :--- |
| Repository | [github.com/sluisr/corex](https://github.com/sluisr/corex) |
| Releases | [github.com/sluisr/corex/releases](https://github.com/sluisr/corex/releases) |
| Bug Reports | [github.com/sluisr/corex/issues](https://github.com/sluisr/corex/issues) |
| Author site | [sluisr.com](https://sluisr.com) |

---

## 🤝 Open to Contributions

Corex is **open source** and **open to proposals**. If you want to contribute:

1. Check open issues at [github.com/sluisr/corex/issues](https://github.com/sluisr/corex/issues)
2. Fork the repo and submit a PR
3. Or open a new issue with your idea / bug report

The project is **actively developed** — new features and fixes ship frequently. Community feedback and contributions are welcome.

---

## 📦 Quick Install

```bash
# From source
cargo install --git https://github.com/sluisr/corex.git --force

# Pre-built binary (Linux/macOS/Windows)
curl -fsSL https://raw.githubusercontent.com/sluisr/corex/main/install.sh | sh

# Via npm
npx corex-cli
```

Set your API key:

```bash
export COREX_API_KEY="sk-your-deepseek-api-key"
```

Then launch:

```bash
cx
```

---

## 🗂️ Configuration Files & Paths

Corex stores all its configuration and data in **`~/.corex/`** (in your home directory):

| File / Path | Purpose |
| :--- | :--- |
| `~/.corex/settings.json` | API key, base URL, default model |
| `~/.corex/flash_settings.json` | CoT reasoning depth (`none` / `low` / `high` / `xhigh` / `max`) |
| `~/.corex/hybrid_settings.json` | Hybrid mode strategy, local server endpoint, scout config |
| `~/.corex/logs/` | Forensic audit logs (`corex-forensic-YYYY-MM-DD.log`) |
| `~/.corex/COREX.md` | Global persistent memory (applies to all projects) |
| `./COREX.md` | Project-level persistent memory (per working directory) |

To inspect or edit config directly:

```bash
# View all settings
ls ~/.corex/

# Edit API key / model
nano ~/.corex/settings.json

# View today's forensic log
cat ~/.corex/logs/corex-forensic-$(date +%Y-%m-%d).log
```

You can also update the API key interactively inside Corex with:

```
/auth
```

---

## 🐛 Bug Reports & Support

### Option 1 — GitHub Issues (recommended for bugs & feature requests)

Open an issue at: **[github.com/sluisr/corex/issues](https://github.com/sluisr/corex/issues)**

Include:
- Corex version (`/info` inside the TUI)
- OS and terminal
- Steps to reproduce
- Expected vs actual behavior
- Relevant forensic log snippet from `~/.corex/logs/`

### Option 2 — Community Discord

Join the sluisr community Discord for questions, ideas, and general chat:

👾 **[discord.com/invite/q3wWh6NjEC](https://discord.com/invite/q3wWh6NjEC)**

Also linked from [sluisr.com](https://sluisr.com) → Community section.

> Use GitHub Issues for reproducible bugs. Use Discord for questions, ideas, or just hanging out.

