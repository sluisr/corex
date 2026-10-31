---
name: corex-session-management
description: >-
  Use this skill when the user asks about sessions in Corex: how to save a
  conversation, how to resume a previous session, how to list sessions, how
  to tag or label a session, how to delete a session, or how conversation
  history works. Trigger phrases: 'resume session', 'save chat', 'continue
  conversation', 'session history', 'chat list', 'how to resume', 'load
  previous chat', 'save checkpoint', 'tag session', 'delete session'.
---

# Corex Session Management

Corex automatically saves every conversation. Sessions are stored in `~/.corex/sessions/` as JSON files, one per session UUID.

---

## Listing Sessions

**From the TUI:**
```
/chat list
```

**From the CLI (without opening TUI):**
```bash
cx --list-sessions
cx -l
```

Output example:
```
Available sessions (3):
  1. Fix authentication bug (2h ago) [tag: auth-fix] · DeepSeek-V4.1-Flash [uuid]
  2. Refactor API client (1d ago) · DeepSeek-V4.1-Flash [uuid]
  3. New Session (3d ago) · DeepSeek-V4-Pro [uuid]
```

Sessions are sorted **most recent first** and scoped to your current project directory.

---

## Saving a Session Checkpoint

**From TUI — tag the current session:**
```
/save my-tag-name
/chat save my-tag-name
```

This sets a human-readable tag on the session so you can resume it by name instead of UUID.

---

## ▶ Resuming a Session

**From TUI:**
```
/resume my-tag-name
/chat resume my-tag-name
/resume 2             ← by index number from the list
/resume a3f9...       ← by UUID prefix
```

**From CLI (before opening TUI):**
```bash
cx --resume my-tag-name
cx --resume 2
cx --resume a3f9c...
cx -r auth-fix
```

Corex will restore the full conversation history, model, temperature, and reasoning settings from that session.

---

## Starting a New Session

```
/chat new
```

Or simply open a new terminal and run `cx` — each `cx` invocation starts fresh unless `--resume` is specified.

---

## ↩ Rewinding to a Checkpoint

```
/rewind
```

Lists every message you sent, each with the files the agent changed from it on (`3 files changed +37 -4`). Pick one (↑/↓, Enter) and choose:

1. **Rewind conversation and revert code changes**
2. **Rewind conversation**: the files stay as they are
3. **Revert code changes**: the conversation stays
4. **Do nothing** (Esc goes back to the list; "Stay at current position" closes it)

Rewinding drops that message and everything after it, and puts the message back in the prompt so you can edit and resend it. Reverting undoes the edits made by `write_file`, `edit` and `apply_patch` (also a subagent's), newest first; if you changed a file afterwards, only the agent's edit is reversed, and a conflicting file is left untouched with a warning. Shell commands (`rm`, `mv`, builds, `git`) are not undone. Edits are saved with the session, so `/rewind` still works after `/resume`.

---

## Where Sessions Are Stored

```
~/.corex/sessions/
  ├── a3f9c7b2-....json    ← full session with all messages
  ├── 8e2d1f4a-....json
  └── ...
```

Each file is a JSON object containing:
- Session UUID, title, tag
- Model, temperature, reasoning_effort used
- Full message history (user + assistant + tool calls)
- Total token usage stats
- Timestamps (created_at, updated_at)

---

## Session Title

Corex automatically sets the session title from the **first line of your first message** (up to 50 characters). No manual action needed.

---

## Searching Sessions

Sessions can be resumed by:
|Method | Example |
|:--- | :--- |
|**Index** (1-based) | `cx -r 1` |
|**Tag** (exact, case-insensitive) | `cx -r auth-fix` |
|**UUID prefix** | `cx -r a3f9c7` |
|**Full UUID** | `cx -r a3f9c7b2-1234-...` |

---

## Notes

- Sessions are **global** (stored in `~/.corex/sessions/`) but the `/chat list` TUI view filters by current workspace directory
- Use `cx -l` (no workspace filter) to see all sessions across all projects
- Sessions are never auto-deleted — clean up manually with:
```bash
rm ~/.corex/sessions/<uuid>.json
```
