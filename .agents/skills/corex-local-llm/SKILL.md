---
name: corex-local-llm
description: >-
  Use this skill when the user asks about using Corex with a local LLM,
  offline mode, air-gapped usage, llama.cpp, llama-server, Ollama, how to
  configure the local model, what the local URL is, how to enable local LLM,
  or how to save API costs. Trigger phrases: 'local llm', 'offline', 'llama',
  'ollama', 'free cost', '$0.00', 'no api key', 'private', 'air-gapped',
  'local model setup'.
---

# Corex Local LLM Setup

Corex supports running a 100% standalone local LLM via any OpenAI-compatible endpoint (llama-server, Ollama, vLLM, LM Studio, etc.). This enables **$0.00 cost** and **100% private, air-gapped** execution.

---

## 🔒 Standalone Offline Local LLM

All queries go directly to your local model with zero Cloud API calls and zero telemetry.

### Step 1 — Start your local server

**Option A: llama-server (llama.cpp)**
```bash
# Download a GGUF model (example: Llama 3.2 3B)
llama-server -m models/Llama-3.2-3B-Instruct-Q4_K_M.gguf --port 8080 -c 8192
```

**Option B: Ollama**
```bash
ollama serve        # starts on port 11434 by default
```

Corex connects to any **OpenAI-compatible API** at the configured URL.

### Step 2 — Enable local mode

**From CLI:**
```bash
cx --local -p "Explain this code"
cx --local --local-url http://127.0.0.1:8080/v1 --local-model Llama-3.2-3B
```

**Via environment variables:**
```bash
export COREX_LOCAL_LLM_ENABLED=true
export COREX_LOCAL_LLM_URL=http://127.0.0.1:8080/v1
export COREX_LOCAL_LLM_MODEL=Llama-3.2-3B-Instruct
cx
```

**Via `~/.corex/settings.json`:**
```json
{
  "local_llm_enabled": true,
  "local_llm_url": "http://127.0.0.1:8080/v1",
  "local_llm_model": "Llama-3.2-3B-Instruct"
}
```

**From inside the TUI:**
```
/model  →  select "Local Offline Assistant"
/local status   →  check if server is reachable
```

---

## 🏥 Health Check

Corex performs a fast health check on startup to verify if the local server is reachable.

Manually verify from CLI:
```bash
curl http://127.0.0.1:8080/v1/models
# Should return JSON with model list if server is running
```

---

## 💡 Recommended Models

| Model | Size | Best for |
| :--- | :--- | :--- |
| `Llama-3.2-3B-Instruct-Q4_K_M.gguf` | ~2GB | Fast responses, chat, Q&A |
| `Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf` | ~4GB | Code understanding, refactoring |
| `Mistral-7B-Instruct-v0.3-Q4_K_M.gguf` | ~4GB | General purpose |

> For small models (Gemma 2B, Llama 3B), set `"local_prompt_lite": true` in `settings.json` to use a reduced system prompt that strips complex tool enforcement instructions.
