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

Corex provides two YOLO modes for autonomous execution:

1. **YOLO Normal** (`/yolo`, `/yolo normal`, `/yolo on`, `cx -y`, `cx --yolo`): Skips confirmation prompts for mutating tools (writing files, running commands, applying patches), with safety exceptions that always ask:
   - shell commands that read or write credentials (`~/.ssh`, `~/.aws`, `~/.corex/settings.json`…) or files that run code by themselves (`.git/hooks/`, `~/.bashrc` and other shell startup files, `crontab`);
   - notes saved to global memory (`~/.corex/COREX.md`), which every future session loads;
   - browser file uploads.
2. **YOLO All** (`/yolo all`, `cx --yolo-all`): Complete autonomy with **zero permission prompts**. Auto-approves all tool executions, bypassing even the sensitive path and memory confirmations.

Editing a sensitive file with the file tools is refused outright.

---

## Enabling YOLO Mode

### Option 1 — From inside the TUI
```
/yolo            # Toggle YOLO mode (cycles between Normal and Off)
/yolo normal     # Enable YOLO Normal mode
/yolo all        # Enable YOLO All mode (no confirmations for anything)
/yolo off        # Disable YOLO mode
```

Toggling `/yolo` without arguments switches between Normal and Off. Status is shown in the TUI status bar as `[YOLO]` or `[YOLO ALL]`.

### Option 2 — With Shift+Tab
In the TUI, press `Shift+Tab` to cycle between execution modes:
`Normal` → `[ACCEPT EDITS]` → `[PLAN MODE]` → `[YOLO]` → `[YOLO ALL]` → `Normal`

### Option 3 — From the CLI flags
```bash
cx -y                    # YOLO Normal
cx --yolo                # YOLO Normal
cx --yolo-all            # YOLO All (no permissions asked)
cx --yolo-all -p "Build and test"
```

### Option 4 — Persist via `settings.json`
```json
// ~/.corex/settings.json
{
  "yolo_mode": "normal"  // or "all", true, false
}
```

This makes YOLO permanent across all sessions until changed.

---

## What Requires Confirmation (without YOLO)

Corex asks before:
- **Writing files** (`write_file`, `edit_file`, `apply_patch`)
- **Running shell commands** (`run_shell_command`)
- **Remote fleet mutating/dangerous commands** (`fleet_run_command`, `fleet_broadcast_command`, `fleet_manage`)
- **Deleting or overwriting** files

Corex does NOT ask before:
- Reading files (`read_file`, `glob_files`, `grep_search`, `list_dir`)
- Web searches (`web_search`, `web_fetch`)
- Background task inspection (`manage_background_process` with `list` or `status`)

When a tool requires approval, Corex presents a **5-option confirmation selector**:
1. **`1` / `y`** — Allow this execution once
2. **`2` / `s`** — Allow for the rest of this session (enables YOLO for the session)
3. **`3` / `n`** — Deny execution
4. **`4` / `p`** — Always allow & save rule to `~/.corex/settings.json`
5. **`5` / `w`** — Deny and type custom instructions for the agent
*(Use `Ctrl+O` to expand/collapse full diff previews in the confirmation dialog.)*

---

## Allowed Commands (Granular Whitelist)

Instead of full YOLO, you can whitelist specific commands that never require confirmation. In the approval dialog, `[s]` allows the command for the session and `[p]` saves it permanently. Or edit `~/.corex/settings.json`:

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
- Or a **prefix** (`git status`) — matches `git status` commands
- Everything else still prompts for confirmation

---

## YOLO + sudo

Corex has native **Silent Sudo** support. If a command needs `sudo`, Corex will:
1. Show a sudo password dialog in the TUI (first time)
2. Cache the password **in memory** for the session (never written to disk)
3. In YOLO mode, subsequent `sudo` commands run without interrupting the flow

The password is stored only in RAM and cleared when the session ends.

---

## Recommended Usage

|Scenario | Recommended setting |
|:--- | :--- |
|Quick one-shot task | `cx -y -p "do the thing"` |
|Trusted project, long session | `yolo_mode: true` in project `.corex/settings.json` |
|Shared machine / sensitive repo | Keep YOLO off, use `allowed_commands` whitelist |
|CI/CD pipeline | `cx --yolo -p "..."` — YOLO required for non-interactive mode |

>  YOLO mode skips ALL confirmation dialogs. Commands execute instantly. Use with care on production systems.

---

## Check Current Mode

```
/stats     ← shows current YOLO status in session info
/info      ← shows session telemetry including YOLO state
```
