# ⚡ corex (`cx`)

> **Corex** — High-Performance Autonomous AI Coding Agent for DeepSeek API & Local LLMs.

[![npm version](https://img.shields.io/npm/v/corex?color=blue)](https://www.npmjs.com/package/corex)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue)](https://github.com/sluisr/corex/blob/main/LICENSE)

## 📦 Installation

Install globally via npm:

```bash
npm install -g corex
```

Or run directly without installing using `npx`:

```bash
npx corex
```

## 🚀 Usage

Once installed, simply run the ultra-ergonomic command `cx` (or alias `corex`):

```bash
cx
```

Or execute non-interactive commands:

```bash
cx -p "Analyze this project and run all unit tests"
```

## ⚡ Features

* 🦀 **100% Pure Native Rust:** Single standalone machine binary with sub-10ms startup time and ~21 MB memory footprint.
* ⌨️ **Ergonomic Command (`cx`):** Single-hand, zero-friction launch from your terminal.
* 🧠 **DeepSeek V4.1 Engine:** Native support for `deepseek-flash` (vision-language MoE, 1M context) and `deepseek-v4-pro` (Reasoning CoT).
* 🔒 **100% Offline Local LLM (@ $0.00):** Private inference with `llama-server`, `llama.cpp`, or `Ollama` on port 8080.
* 🛡️ **96%+ KV Cache Hit Rate:** Background tool compression preserving KV cache to slash API costs.
* 📥 **Prompt Queuing:** Type or paste next instructions while the model streams, auto-dequeuing upon completion.
* ⚡ **Non-blocking Balance:** Instant account credit lookups without freezing the terminal.

## 🔗 Links

* **Official Website:** [corex.sluisr.com](https://corex.sluisr.com)
* **GitHub Repository:** [github.com/sluisr/corex](https://github.com/sluisr/corex)
* **Bug Reports & Issues:** [github.com/sluisr/corex/issues](https://github.com/sluisr/corex/issues)
* **Changelog:** [corex.sluisr.com/changelog](https://corex.sluisr.com/changelog)

---
© 2026 **sluisr**. Licensed under Apache 2.0.
