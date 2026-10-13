---
name: corex-yolo-mode
description: >-
  Use this skill when the user asks about YOLO mode in Corex, auto-approval
  of tool executions, skipping confirmations, allowed commands, or running
  Corex autonomously without prompts. Also activate for questions about the
  confirmation system, how to whitelist commands, or how tool execution
  approval works. Trigger phrases: 'yolo', 'auto approve', 'no confirmations',
  'skip confirmation', 'autonomous mode', 'allowed commands', 'whitelist',
  'run without asking'.
---

# Corex YOLO Mode & Tool Execution Control

By default, Corex asks for confirmation before executing any **mutating or potentially dangerous** action (writing files, running shell commands, applying patches). YOLO mode disables this entirely.

---

## ⚡ Enabling YOLO Mode

### Option 1 — From inside the TUI
```
/yolo
/yolo on
/yolo off
```

Toggling `/yolo` without arguments switches the current state. Status is shown in the TUI status bar.

### Option 2 — From the CLI flag
```bash
cx -y
cx --yolo
cx --yolo -p "Refactor all files in src/ and run tests"
```

### Option 3 — Persist via `settings.json`
```json
// ~/.corex/settings.json
{
  "yolo_mode": true
}
```

This makes YOLO permanent across all sessions until changed.

---

## 🛡️ What Requires Confirmation (without YOLO)

Corex asks before:
- **Writing files** (`write_file`, `edit`, `apply_patch`)
- **Running shell commands** (`run_shell_command` / `bash`)
- **Deleting or overwriting** files

Corex does NOT ask before:
- Reading files (`read_file`, `glob`, `grep`, `list_directory`)
- Web searches (`web_search`, `web_fetch`)
- Background task inspection (`manage_task` with `list` or `status`)

---

## ✅ Allowed Commands (Granular Whitelist)

Instead of full YOLO, you can whitelist specific commands that never require confirmation:

```json
// ~/.corex/settings.json
{
  "allowed_commands": [
    "cargo",
    "git status",
    "git diff",
    "ls",
    "cat",
    "echo"
  ]
}
```

- Entries can be a **binary name** (`cargo`) — matches any `cargo ...` command
- Or a **prefix** (`git status`) — matches exactly `git status` and nothing else
- Everything else still prompts for confirmation

---

## 🚨 YOLO + sudo

Corex has native **Silent Sudo** support. If a command needs `sudo`, Corex will:
1. Show a sudo password dialog in the TUI (first time)
2. Cache the password **in memory** for the session (never written to disk)
3. In YOLO mode, subsequent `sudo` commands run without interrupting the flow

The password is stored only in RAM and cleared when the session ends.

---

## 💡 Recommended Usage

| Scenario | Recommended setting |
| :--- | :--- |
| Quick one-shot task | `cx -y -p "do the thing"` |
| Trusted project, long session | `yolo_mode: true` in project `.corex/settings.json` |
| Shared machine / sensitive repo | Keep YOLO off, use `allowed_commands` whitelist |
| CI/CD pipeline | `cx --yolo -p "..."` — YOLO required for non-interactive mode |

> ⚠️ YOLO mode skips ALL confirmation dialogs. Commands execute instantly. Use with care on production systems.

---

## 🔍 Check Current Mode

```
/stats     ← shows current YOLO status in session info
/info      ← shows session telemetry including YOLO state
```
