---
name: corex-troubleshooting
description: >-
  Use this skill when the user reports a problem with Corex: errors, crashes,
  API failures, slow responses, connection issues, authentication problems,
  blank output, key not found, rate limits, or anything not working as expected.
  Trigger phrases: 'not working', 'error', 'API error', 'connection refused',
  'slow', 'key not found', 'unauthorized', '401', '429', 'rate limit',
  'blank output', 'hangs', 'crashes', 'local llm not connecting'.
---

# Corex Troubleshooting Guide

---

## 🔑 Authentication & API Key Problems

### "Unauthorized" / 401 error
```bash
# Check which key is being used (priority order):
# 1. COREX_API_KEY env var
# 2. DEEPSEEK_API_KEY env var
# 3. OPENAI_API_KEY env var
# 4. ~/.corex/settings.json → api_key field

echo $COREX_API_KEY
cat ~/.corex/settings.json | grep api_key
```

Fix options:
```bash
# Option A: export in shell
export COREX_API_KEY="sk-your-deepseek-key"

# Option B: set interactively inside Corex TUI
/auth

# Option C: edit settings.json directly
nano ~/.corex/settings.json
# Set: "api_key": "sk-..."
```

Get your key at: [platform.deepseek.com/api_keys](https://platform.deepseek.com/api_keys)

---

## 🌐 Connection & Network Errors

### "Connection refused" / timeout
```bash
# Test connectivity to DeepSeek API
curl -s https://api.deepseek.com/v1/models \
  -H "Authorization: Bearer $COREX_API_KEY" | head -50
```

### Custom API endpoint not working
```bash
# Check your configured base URL
cat ~/.corex/settings.json | grep base_url

# Override via env var
export COREX_BASE_URL="https://api.deepseek.com"

# Or pass inline
cx --base-url https://api.deepseek.com -p "test"
```

---

## ⚡ Rate Limits (429 Too Many Requests)

- DeepSeek free tier has rate limits — wait a few seconds and retry
- Check your account balance: `/balance` inside TUI or [platform.deepseek.com](https://platform.deepseek.com)
- Switch to `deepseek-flash` (cheaper, higher rate limits) via `/model`
- Enable Hybrid mode to route simple queries to your local LLM: `/model` → Hybrid → Auto-Triage

---

## 🤖 Local LLM Not Connecting

### Check if server is running
```bash
# Test local server health
curl http://127.0.0.1:8080/v1/models

# Should return JSON — if it fails, server is not running
```

### Start llama-server
```bash
llama-server -m /path/to/model.gguf --port 8080 -c 8192
```

### Check Corex local status from TUI
```
/local status
```

### Wrong URL configured
```bash
cat ~/.corex/hybrid_settings.json | grep local_url
# Default: http://127.0.0.1:8080/v1
# Ollama uses: http://127.0.0.1:11434/v1
```

---

## 🐢 Slow Responses

1. **Check TTFT** — type `/stats` to see Time-To-First-Token per turn
2. **Use Flash model** — `deepseek-flash` is much faster than `deepseek-v4-pro`
3. **Check KV cache hit rate** — should be >90%. Low hit rate = slow & expensive. Fix: don't change the system prompt between turns.
4. **Enable hybrid mode** — simple Q&A goes to local LLM at 0ms cost
5. **Compress context** — long conversations slow down. Run `/compress` or `/compact`

---

## 📭 Blank / Empty Output

- Verify the model finished: check `/stats` for `finish_reason`
- Try `/rewind` to undo the last turn and retry
- If using `deepseek-v4-pro` with CoT, it might still be reasoning — wait for the spinner
- Check forensic log for raw API response:
```bash
cat ~/.corex/logs/corex-forensic-$(date +%Y-%m-%d).log | tail -100
```

---

## 🔄 Context Too Long / Auto-Compact Triggered

Corex auto-compacts at **95,000 tokens** by default. You'll see a `[INFO]` notice. This is normal behavior.

To change the threshold:
```json
// ~/.corex/settings.json
{
  "compact_threshold_tokens": 120000
}
```

To manually compact before hitting the limit:
```
/compress
/compact
```

---

## 🔍 Read the Forensic Log

Every session is fully logged with raw API payloads, telemetry, and errors:

```bash
# Today's log
cat ~/.corex/logs/corex-forensic-$(date +%Y-%m-%d).log

# Last 50 lines
tail -50 ~/.corex/logs/corex-forensic-$(date +%Y-%m-%d).log

# Search for errors
grep -i "error\|fail\|401\|429" ~/.corex/logs/corex-forensic-$(date +%Y-%m-%d).log
```

---

## 🗑️ Reset Configuration

```bash
# Back up and reset settings
cp ~/.corex/settings.json ~/.corex/settings.json.bak
rm ~/.corex/settings.json

# Re-run cx and set API key fresh
cx
# → type /auth to set key
```

---

## 📬 Still Not Working?

- **GitHub Issues:** [github.com/sluisr/corex/issues](https://github.com/sluisr/corex/issues)
  - Attach the relevant section of `~/.corex/logs/corex-forensic-YYYY-MM-DD.log`
- **Discord:** [discord.com/invite/q3wWh6NjEC](https://discord.com/invite/q3wWh6NjEC)
