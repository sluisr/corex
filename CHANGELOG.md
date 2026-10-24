# Changelog

All notable changes to **Corex** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.8.1] - 2026-10-25

### Added

#### Images from the Clipboard
- **Paste an image with `Ctrl+V`**: A screenshot or copied image becomes an `[Image #N]` tag in the prompt, and the pixels travel with the message when it is sent. Terminals that cannot paste an image as text send an empty paste, which is treated the same way. When the clipboard holds no image, `Ctrl+V` pastes its text as usual.
- **The tag behaves as one piece**: It moves, and is deleted, as a whole with the arrow keys, `Backspace` and `Delete`, like `[Pasted text #N]`. Deleting it drops the image, and a message can carry several images.
- **Highlighted in the conversation**: `[Image #N]` and the other paste tags keep their blue highlight in the feed after the message is sent, not only in the composer.
- **Clipboard access**: Linux reads through `wl-paste` (Wayland) or `xclip` (X11), and macOS through `pngpaste` when it is installed. Images are limited to 5 MB and must be PNG, JPEG, GIF or WebP. Windows pastes text only for now.
- **Models that cannot see images**: DeepSeek Pro receives a note in place of each image, telling it that the image is not a file on disk and that the user should switch model with `/model`; a warning is shown when the image is pasted. If any other model rejects image input, the request is retried once with the same note and a notice says so. Images stay in the session for a model that can see them.
- **Prompts with images keep their words** in the feed, session titles, the session picker preview and `/compact` summaries.

### Changed
- **Session summary on exit**: Redesigned as five plain rows (`Model`, `Session`, `Tokens`, `Cost`, `Resume`) with dim labels and no decoration. Token counts use thousands separators, cache hits sit on the tokens row, costs show four decimals, and colour is dropped when output is not a terminal or `NO_COLOR` is set.
- **Tool approval dialog**: It lists only the calls that need approval. The others in the same batch run right away instead of being shown for confirmation.

### Removed
- **Inline hint in the composer** (`Enter queues · Alt+Enter sends now`): It was drawn next to the text being typed while the agent worked, and the activity bar already shows the same keys.

### Fixed
- **A web search asking for approval**: In 0.8.0 a search was shown in the approval dialog whenever another call in the same batch needed approval, typically a `web_fetch` to a host not yet allowed. The search itself never needed it.
- **Empty `[You]` row**: The internal message that carries the images loaded by `read_image` is no longer drawn in the feed as an empty prompt, and no longer counts as one of the user's requests in summaries.

---

## [0.8.0] - 2026-10-25

### Added

#### Anthropic (Claude) Provider
- **Claude 5.x model catalog**: `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5` and `claude-fable-5-1` replace the Claude 3.x entries in the provider list. Older ids are still accepted by name.
- **Per-generation request profiles (`anthropic::model_profile`)**: Thinking is sent as `adaptive` for Claude 4.6+/5.x and as a fixed `budget_tokens` budget for 3.7 and 4.0–4.5. `temperature` is omitted where the API rejects it (4.7+), `output_config.effort` is mapped from the reasoning setting (including `xhigh`), and `max_tokens` follows the model (8,192 for 3.x, 32,000 for 4.0–4.5, 64,000 for 4.6+).
- **Signed thinking blocks are replayed**: `thinking`, `redacted_thinking` and `signature_delta` are reassembled from the stream, stored with the turn's tool calls and sent back unchanged, as the API requires for thinking with tool results. They are dropped if the provider is switched mid-session.
- **Automatic prompt caching and cache accounting**: Requests carry a top-level `cache_control` of type `ephemeral`; `cache_read_input_tokens` and `cache_creation_input_tokens` are counted in prompt usage and cache hit statistics.
- **Images in user messages** are sent as native `image` blocks (base64 `data:` URIs inline, `http(s)` URLs by reference).
- **Forward-compatible streaming**: Unknown SSE event types, content blocks and deltas are ignored instead of breaking the stream. `max_tokens` and `model_context_window_exceeded` stop reasons are reported as output truncation, and HTTP error bodies are now shown and logged.

#### Tools
- **`read_image`**: Looks at PNG, JPEG, GIF and WebP files (type sniffed from the file signature, 5 MB limit). The image reaches the model in a single `user` message after all tool responses of the turn, a shape every provider accepts. `read_file` points binary images to it. Subagents do not get this tool.
- **Multi-query `web_search`**: A new `queries` argument runs up to 4 searches in parallel and merges the results rank by rank without duplicate URLs.
- **Readable `web_fetch` (`web_content.rs`)**: HTML is converted to Markdown (headings, links, lists, tables, code blocks), the response charset is honoured (BOM, `Content-Type`, `<meta>`), binary content types (PDF, images, archives) are refused with a clear message, and the output limit grows from 15,000 to 50,000 characters.
- **Task filter**: The task tracker's list tool (`tracker_list_tasks`) accepts a `status` filter.

#### Context Window & Token Management
- **Output spill (`spill.rs`)**: A tool result over 30,000 characters is cut to its first 10,000 and last 2,000 characters. The complete text is saved in a private per-user directory under the system temp dir (files older than 3 days are removed) and its path is placed in the cut, so the model can read the middle with `read_file`. Reading spilled files needs no confirmation.
- **Deterministic pruning before summarizing (`context_prune.rs`)**: Before `/compact` or automatic compaction calls the model, tool results over 8,192 characters from earlier turns are trimmed to head and tail. The turn in progress is never touched, and if pruning is enough no summary is requested.
- **Automatic recovery from context overflow**: When an API rejects a request for exceeding the context window, the conversation is compacted and the request retried once, invisibly to the consumer.
- **Truncated tool calls are never executed**: If a response stops because of the output token limit, its tool calls (whose arguments may be cut mid-file or mid-command) are answered with an error and the model retries in smaller pieces. A second consecutive truncation stops the loop. Applies to the TUI, headless CLI and subagents.
- **Repeated-call guard (`repeat_guard.rs`)**: At the 3rd, 5th and 8th identical consecutive tool call, a short reminder asking the model to change approach is appended to the result. It never blocks a call.
- **Compaction keeps the active task**: The latest user prompt is never removed, `assistant` tool-call / `tool` response groups are never split, an earlier summary is carried into the new one, and digest entries are longer (500 characters per request, 160 per tool argument or error).

#### Terminal UI
- **Send-now messages (`Alt+Enter`)**: While the agent works, a message can be handed to the model at the next safe point (between tool rounds) instead of waiting behind the queue. `Enter` still queues.
- **Take back pending messages**: `Esc` takes back the newest pending message, one per press, before cancelling the generation. Cancelling no longer discards what was typed: messages that never reached the model return to the composer.
- **History shortcuts**: `Ctrl+P` / `Alt+Up` and `Ctrl+N` / `Alt+Down` navigate the prompt history.
- **Light and dark themes**: The palette is picked automatically (`COREX_THEME=light|dark` wins, otherwise `COLORFGBG`), with text colours kept at a 4.5:1 contrast on black or white backgrounds.
- **Terminal-safe visuals**: An ASCII spinner is used on the Linux console and in non-UTF-8 locales, and the text caret blinks while idle.
- **Footer mode badges**: Auto-approve and other modes that change how tools run are spelled out in the status bar.
- **Collapsed logs**: Command and subagent logs show their last 5 rows when collapsed, with a single `Ctrl+O` hint in the footer instead of one per block.
- **Session picker**: Shows a model badge (when models are mixed), relative times and a conversation preview.
- **Slash command popup**: Shows the cursor position and how many commands are hidden above and below, and never leaves a group header as the last visible row.
- **Shared text helpers (`text.rs`)**: Column-aware truncation that always marks a cut with `…`, thousands grouping and plain-text previews of Markdown.

### Changed
- **Resilient update check and `cx update`**: Release information is now resolved through three tiers: the GitHub API (using `GITHUB_TOKEN` or `GH_TOKEN` when present), the `corex.sluisr.com/release.json` CDN, and the `releases/latest` web redirect, which uses no API quota. The GitHub tags fallback was removed.
- **Release pipeline**: Release notes now include an Installation & Upgrade section, and the checksum table uses aligned columns.
- **`glob`**: Results are sorted newest first, 100 are shown, up to 20,000 are collected, and the complete list is saved to a file when it does not fit. Invalid patterns report an error.
- **`read_file`**: Lines over 2,000 characters are truncated and output is paged at 100 KB. Files that are not valid UTF-8 report a clear error.
- **`write_file`**: Keeps CRLF line endings when overwriting a file that uses them.
- **`edit`**: Reports real line numbers and says when the match only held ignoring leading or trailing whitespace. Replacement indentation is adjusted to the matched block.
- **`apply_patch`**: Tracks the line offset accumulated by earlier hunks, keeps the file's original context lines, limits whitespace fuzz to within 100 lines of the expected position, and no longer mistakes `+++` / `---` content lines for file headers.
- **MCP tools**: Results flagged `isError` are reported as tool errors, and JSON-RPC errors without an id are no longer swallowed.
- **`dynamic` reasoning mode**: Uses `low` for turns that only answer a tool result and `high` otherwise, independent of per-model settings. `medium` is no longer offered for DeepSeek, which rejects it.
- **Gemini conversation history**: Consecutive assistant messages left after collapsing unsigned tool turns are merged.
- **Forensic and subagent logs**: Plain ASCII layout, without box-drawing characters or symbols.
- **Reasoning cache**: Keys include the tool call id.
- **Local LLM**: `max_tokens` raised from 2,048 to 4,096, `reasoning_content` is streamed as reasoning, and HTTP errors show the server's message.
- **Release profile**: `opt-level` is now `"s"` (size) instead of `3`.
- **Dependencies**: Added `htmd`, `encoding_rs`, `markup5ever_rcdom` and `libc`; removed `tracing-subscriber` and `shell-words`; `chrono` and `zip` use minimal features.

### Security
- **Edits require a prior read**: `edit`, `apply_patch` and overwriting with `write_file` are refused for a file the model has not read in the session, or that changed on disk since it was read. New files are created with an atomic no-overwrite operation, so a file that appeared in the meantime is never replaced.
- **Reads outside the workspace ask first**: `read_file`, `read_image`, `grep_search`, `glob` and `list_directory` need approval for paths outside the workspace; an "always allow" decision is stored per tool.
- **More protected write paths**: `.git/config`, `.git/info/attributes`, `.ssh/config`, `.cargo/config(.toml)`, `~/.config/systemd/user/` and `.gitmodules`.
- **Shell command safety**: Environment assignments before a command are only ignored for a short allowlist (locale, terminal, pager variables and a trusted `PATH`); `LD_*`, `GIT_*`, `BASH_ENV`, `NODE_OPTIONS`, `PYTHONPATH` and similar are rejected. Commands given with an explicit path are not auto-approved. Abbreviated long flags (`--out` for `--output`) and clustered short flags (`-ro`) are detected for `sort`, `rg`, `git` and `tar`, `ps` is restricted so it cannot print process environments, and a working directory outside the workspace requires approval.
- **Subagents no longer run unattended with full privileges**: They previously ran with auto-approve on and inherited the sudo password. They now run with auto-approve off and no sudo password, and any action that would need user approval is denied. Scout and Researcher stay read-only, Worker is explicitly granted file editing, and sudo, commands that touch credentials, protected config or hook paths or destructive targets (`rm -rf /`, `mkfs`, `dd if=`, `reboot`, `shutdown`), and working directories outside the workspace are refused.
- **Fleet command classification**: Command wrappers (`env`, `sudo`, `nohup`, `xargs`, `watch`, …) are never treated as safe, dangerous patterns match at word boundaries, and `$` and `\` in a command disqualify it from the safe list. Mutating forms of `ip`, `nmcli`, `hostname`, `date`, `iptables`/`nft`/`ufw`, `journalctl --vacuum`, `awk -f` and several `systemctl` actions are no longer auto-approved, and `env` is removed from the default safe list. Any fleet containing a node with `sudo` now asks for confirmation.
- **SSH multiplexing guard**: Control sockets are only used from a directory owned by the current user; otherwise multiplexing is disabled.
- **SSRF hardening**: Hostnames are resolved through a resolver that returns only public addresses, `0.0.0.0/8` is blocked, and IPv4 addresses embedded in NAT64 and 6to4 IPv6 addresses are checked.
- **Per-host approval for `web_fetch`**: Approvals can be scoped to a host (`web_fetch:<host>`) rather than every URL.
- **Untrusted web content**: Fetched pages and search results are prefixed with a notice that they are data and not instructions, and hidden elements (`display:none`, `hidden`, `aria-hidden`, scripts, iframes) are removed before conversion.
- **Safer configuration handling**: Workspace settings are never written if they cannot be parsed for secret stripping; an invalid `settings.json` is reported and copied to `settings.json.invalid` instead of being silently replaced by defaults; keys supplied through environment variables are not written to `settings.json`; and the model-family fallback (`claude-*`, `gemini-*`) validates the provider URL.
- **Private directory ownership**: Directories used for credentials, locks and spill files must belong to the current user and be private.
- **Askpass helper**: The script is created with `0700` permissions from the start and quotes the credential path correctly.
- **No orphaned processes**: Background tasks and the staged sudo credential are cleaned up on normal exit, on `SIGHUP`/`SIGTERM`/`SIGQUIT` and on panic.

### Fixed
- **JSON repair**: Escaped `\'` inside double-quoted strings, mismatched closers (`{"a":[1}`), stray closers and output cut right after a colon now produce valid JSON.
- **Chat scrolling on very long conversations**: The scroll offset no longer overflows a 16-bit counter.
- **Fleet PTY**: When the interactive terminal cannot be opened, the reason is shown and the console falls back to batch SSH.
- **Narrow terminals**: Enabling `/fleet wide` below the minimum width explains why the layout does not apply.
- **Termux**: The soft keyboard request is rate-limited to once per second, so rapid taps no longer spawn a burst of processes.
- **File search**: `.git`, `node_modules` and `target` are skipped by recursive search.
- **Truncation of tool output in history**: The cap on tool results kept in the conversation moves from 8,000 characters to the spill limit, preserving both head and tail instead of only the head.

### Removed
- **Per-mode reasoning settings**: `commandReasoningEffort`, `codeReasoningEffort` and `searchReasoningEffort` are no longer read. Existing `settings.json` files still load, and the values are simply ignored.
- **Provider list entries**: `deepseek-v4.1-flash-1017` is no longer listed separately (it resolves to the DeepSeek Flash entry and `-m deepseek-v4.1-flash-1017` keeps working), and the Claude 3.x models are no longer listed.

---

## [0.7.0] - 2026-10-17

### Added

#### Fleet & SysAdmin Multi-Server Orchestration (`crates/corex-fleet`, `cxs`)
- **Direct Interactive Fleet TUI Launch (`cxs`, `cx --fleet`)**: Invocations without subcommands launch directly into the live dual-pane SysAdmin console (AI agent chat on the left, live SSH terminal and node telemetry on the right).
- **Headless Automation CLI**: Preserved scripting subcommands (`cxs status`, `cxs ping`, `cxs top`, `cxs telemetry`, `cxs run`, `cxs broadcast`).
- **Interactive Remote PTY (`vt100`) & `/fleet wide`**: Native PTY terminal session with wide 3-column layout (`chat | terminal | telemetry`), asynchronous node polling (up to 8 in parallel), and `take_dirty()` atomic flag rendering eliminating idle 60 FPS redraws.
- **Mandatory Sudo Escalation Guards**: Enforced interactive confirmation prompts for any fleet command requiring `sudo: true` or `node.sudo`, wrapped in POSIX `sudo -n -- sh -c '<cmd>'` with `shell_quote_single` escaping.
- **OpenSSH-Compliant Config Parser (`~/.ssh/config`)**: Supports `Key=Value` syntax, multiple host aliases, ignores non-host `Match` blocks, respects *first-match-wins*, expands `~/` paths, and enforces `0600` private file permissions.
- **Multi-Distro Resilient Telemetry Probing**: Section-delimited parsing (`===CX_*===`) with `LC_ALL=C`, supporting Docker overlay, ZFS, and LVM root partitions with CPU core detection.

#### DeepSeek Ecosystem & Native Search / Fetch Protocol
- **DeepSeek Native Search & Fetch API Integration (`crates/corex-tools/src/web.rs`)**: Direct integration with DeepSeek's official `POST https://api.deepseek.com/v1/tools/search` and `POST https://api.deepseek.com/v1/tools/fetch` endpoints for autonomous web research, markdown scraping, and query operator filtering (`site:`, `lang:`, `after:`, `before:`, `-"..."`).
- **DeepSeek-V4.1-Flash-1017 Model Engine Support**: Added first-class recognition, CLI parsing (`cx -m deepseek-v4.1-flash-1017`), and status display for DeepSeek's newly released model ("wish-driven programming", faster task completion, lower latency).

#### Security & Sandbox Hardening
- **Forensic Audit Logging & 7-Day Auto-Retention (`corex-core::forensic`)**: Integrated `ForensicLogger` recording structured audit trails to `~/.corex/logs/corex-forensic-YYYY-MM-DD.log` and `.corex/forensic.log`, capturing every tool call, shell command, file change, and LLM exchange with millisecond timestamps and 7-day retention cleanup.
- **Per-Path Advisory Concurrency Locking (`corex-core::file_lock`)**: Implemented `PathGuard` and per-path advisory sidecar file locking (`~/.corex/locks/<hash>.lock`) using FNV-1a hashing to eliminate silent lost updates across concurrent Tokio background tasks and multiple Corex processes.
- **Secret Leak Prevention (`Config::strip_secrets`)**: Strips `provider_api_keys` and `mcp_servers` alongside `api_key` before persisting workspace `.corex/settings.json`, preventing accidental repository credential leaks.
- **DeepSeek API Key Isolation (`Config::endpoint`)**: `self.api_key` is strictly isolated and never forwarded as fallback to third-party endpoints unless the base URL explicitly matches.
- **Universal SSRF Guarding on Provider Endpoints**: All resolution branches enforce `validate_base_url(&p.base_url).is_ok()`, blocking insecure HTTP schemes on non-loopback hosts and embedded credentials.
- **HTTP Redirect Anti-SSRF Validation (`web.rs`)**: Replaced automatic redirects with manual 5-hop redirect following, resolving DNS and validating against private IP ranges and wildcard DNS rebinders (`*.nip.io`, `*.sslip.io`, `*.xip.io`).
- **Ephemeral Sudo Credential Guard (`AskpassGuard`)**: Enforces RAII cleanup of `.askpass_cred` immediately upon tool completion, timeout, or backgrounding, securing `/tmp/corex-askpass-<uid>` with `0700` permissions.
- **Shell Sandbox Obfuscation Defense**: Blocks variable expansion (`$VAR`), quote-wrapped flags (e.g. `find . '-delete'`), and sensitive path wildcards (`cat ~/.ssh/id_*`) across all shell executions, even when matching `allowed_commands`.
- **Cancellation-Safe Bounded MCP Transport (`mcp.rs`)**: Replaced unbounded reader with 16 MiB line-bounded reader, routed child stderr to `Stdio::null()` to eliminate 64 KB pipe deadlocks, and sanitized environment variables passed to child MCP processes.
- **Fail-Closed SHA-256 Update Verification (`update.rs`)**: Rejects updates if `.sha256` signature is missing, invalid, or mismatched, with a 200 MiB decompression bomb limit (`MAX_EXTRACTED_BINARY_BYTES`).

#### Performance, Memory (RAM) & Token Management
- **Visible Viewport Chat Rendering ($O(\text{viewport})$)**: Replaced full conversation cloning at 60 FPS with segmented index tracking (`SelectionLines`), rendering only lines visible in the viewport.
- **Streaming Markdown Cache (`streaming_md_cache`)**: Caches partial markdown structures during token streaming, eliminating re-parsing from byte 0 on non-growing frames.
- **Local LLM Context Shielding (`local_client.rs`)**: Routes local models through preventative tool truncation, automated context compaction (`compact_messages`), and emergency history capping (24,000 tokens), preventing `context_length_exceeded` crashes in `llama-server` and Ollama.
- **Deterministic Background & Subagent Log Truncation**: Bounded system event messages (`TASK_DONE`, `SUBAGENT_DONE`) to 8,000 characters while retaining the last 12 KB in memory and full forensic traces on disk.
- **FIM Bounded Context Windowing (`/fim`)**: Enforces 2 MB metadata file limits and extracts a ~24 KB bidirectional context window around `<FIM_HOLE>`.
- **Accurate Token Estimation**: Refined token calculation heuristic to `s.len().div_ceil(3)` and included `m.tool_calls` tokens to prevent prompt overflows.
- **Prompt Builder TTL Cache**: Caches on-disk skill lookups and memory rules for 15 seconds (`cached_skills_and_memory`), eliminating redundant synchronous filesystem calls.
- **Deterministic Reasoning Cache (`reasoning_cache.rs`)**: Persists up to 100 entries with SHA-256 key hashing and FIFO eviction to `~/.corex/reasoning_cache.json`.

#### TUI, UX & Visual Identity Redesign
- **Discrete Terminal Mint User Badge (`[You]`)**: Replaced `You:` with a sleek badge in terminal mint green (`#82D7A0`) with dark gray brackets and auto-aligned 8-column multiline indentation.
- **Collapsible Reasoning Badge (`[• Thought · 2.3s]`)**: Replaced geometric headers with a clean badge displaying real-time millisecond elapsed timer, toggled via `Ctrl+T`.
- **Multi-Line Wrapped Command Approval Boxes (`diff_view.rs`)**: Confirmation boxes now wrap long commands and complex flag pipelines (`format_boxed_wrapped`) preserving all paths and flags without `...` truncation.
- **Sequential Step-by-Step Tool Approval Modal**: Presents multi-tool batches individually (`[1 of N]`) with options `[y] allow this change`, `[a] allow all remaining`, `[n] deny`, and `[w] deny with feedback`.
- **Compact Status Bar Layout**: Replaced redundant badges with an ultra-compact center telemetry indicator (`1 task · 2 agents`).
- **Global & Project Personality Guidelines**: Automatically discovers and injects persistent instructions from `~/.corex/SYSTEM.md`, `~/.corex/COREX.md`, `./.corex/SYSTEM.md`, `./COREX.md`, and `./AGENTS.md`.
- **Embedded Procedural Skills (`corex-config`, `corex-fleet`)**: Added first-class built-in skills with instantaneous fallback.
- **Shell Autocompletions**: Bundled updated completion scripts for Zsh and Bash supporting `cxs`, `--fleet`, and `--fetch`, with automatic `.zcompdump` cache refresh.

### Fixed
- **Fleet TUI Direct Invocation**: Fixed issue where `cxs` or `cx --fleet` exited immediately to bash with status `Ok(())` by properly initializing `FleetDialogState` and entering SysAdmin mode on empty argument invocations.
- **Data Race / Undefined Behavior Elimination**: Removed unsafe `std::env::set_var` calls in multithreaded runtime during config loading and `/key` updates.
- **100% CPU Background Channel Busy Loop**: Fixed `select!` loop busy polling when child process stdin closes.
- **Grep Empty Query Display (`grep ""`)**: Resolved discrepancy between tool schema (`pattern`/`dir_path`) and renderer (`query`/`path`).
- **UTF-8 Slicing Panics**: Replaced raw byte slicing with `safe_truncate_str` across `json_repair.rs`, `session.rs`, and email markdown parsers.
- **Nested Truncated JSON Repair**: Fixed stream truncation parser so inner braces do not cause premature closure.
- **Symlink Workspace Escape Shield**: Canonized paths and ancestors to ensure symlinks cannot escape workspace boundaries.
- **Selective Safe Command Validation**: Permitted read-only inspection commands (`hostnamectl`, `systemctl status/--failed`, `lsblk`, `lscpu`, `dmesg`, `vmstat`, `echo`) with discard redirections (`2>/dev/null`, `2>&1`), while strictly preserving confirmation for destructive actions (`systemctl restart/stop/kill`).
- **Subagent ID Collision Prevention**: Fixed bit-shift zeroing bug in `rand_u32` with atomic sequence counters.
- **URL Balance Query**: Stripped `/v1` suffix when querying `/user/balance`.

---

## [0.6.3] - 2026-10-14

### Fixed
- **Subagent High-Precision Chronometer**: Fixed the duration timer where subagents reported `Duration: 0s` on sub-second executions. Durations are now computed using millisecond precision (`{:.1}s`, e.g. `0.2s`, `1.4s`), providing accurate real-time duration tracking across all subagents and background tasks.
- **Worker Computational Benchmark Engine**: Added a specialized computational benchmark runner in the Worker subagent. Generates persistent benchmark scripts (e.g. `/tmp/bench_agent.py`), computes prime numbers (up to 50,000) and Fibonacci sequences with `time.perf_counter()`, executes the script in Python, verifies the file exists on disk, and reports exact execution times in milliseconds, throughput (primes/sec), and computed values.
- **Worker Timing Namespace & Evaluation**: Populated `time`, `math`, and `perf_counter` into the Worker's dynamic Python evaluation namespace, preventing `NameError: name 'perf_counter' is not defined` when evaluating timing functions.
- **Researcher Live Package Registry & Version Verification**: Added live PyPI querying (`https://pypi.org/pypi/<pkg>/json`) to the Researcher subagent. When package version inquiries are made (e.g. stable pytest/pytest-cov versions in 2026), Researcher fetches the exact stable version, publication date, PyPI project URL, and local environment installation status instead of generic best practices.
- **Subagent Forensic Audit System & Execution Trace**: Implemented an exhaustive forensic logging system for all autonomous subagents (`scout`, `tester`, `researcher`, `worker`). Every subagent records its entire lifecycle to a dedicated log file under `~/.corex/logs/subagents/<task_id>.log` and mirrors to the central session audit, documenting the original prompt, workspace directory, timestamps, step-by-step decisions, shell commands executed (`sh -c ...`), exit codes, exact stdout/stderr, and timing metrics in milliseconds.
- **Forensic Inspection via `manage_subagents action=log`**: Added the `log` action to `manage_subagents` (`manage_subagents action=log task_id=...`) to retrieve full forensic traces directly in the TUI, and embedded the forensic log path in `action=status` and `action=list`.
- **Deep Code Analysis in Scout**: Scout now detects files referenced in natural language prompts (e.g. `main.py`), parses function signatures, docstrings, imports, and entry points, and executes automated syntax validation (`python3 -m py_compile`) instead of only listing filenames.
- **Dynamic Code Execution & Real Runtime Verification in Worker**: Fixed issue where worker regex greedily matched file paths without action verbs, dumping file contents instead of executing. Worker now parses function calls (e.g. `fibonacci(10)`, `is_prime(99)`, `divide(1,0)`), executes them via a secure base64-encoded Python runner with dynamic module loading (`.` and `./src`), captures outputs and expected exceptions (`ValueError`), and delivers structured verification reports.
- **Comprehensive Pytest Architectural Synthesis & Documentation in Researcher**: Fixed issue where researcher returned raw file paths when asked about pytest best practices. Researcher now provides an authoritative technical guide covering `src/` layout, automated test discovery, AST assert rewriting, modular fixtures (`conftest.py`), declarative parametrization (`@pytest.mark.parametrize`), exception testing (`pytest.raises`), and detects local workspace test files.
- **Sequential Multi-Command Execution in Tester**: Tester now parses and executes multiple numbered steps or command sequences from prompts (e.g. `1) python3 -m py_compile main.py` and `2) python3 main.py`), capturing output and exit codes for every command sequentially.
- **Authoritative Technical Synthesis in Researcher**: Researcher now synthesizes technical specifications and canonical code examples (e.g. idiomatic Python 3.10+ `sys.argv` entrypoint with `argv: list[str] | None = None`) when local workspace documentation is absent, eliminating empty returns.
- **Real File Generation & Execution in Worker**: Worker now interprets file creation instructions (e.g. generating `test_main.py` with `unittest`), inspects target modules to generate accurate tests, writes the file to disk, and runs subsequent test commands (`python3 -m unittest test_main.py`), delivering real execution traces rather than echoing instructions.
- **Subagent Terminal Isolation & Collapsible Rendering**: Fixed raw log pollution where finishing background subagents dumped unformatted IPC messages (`SUBAGENT_DONE|...`) and tens of kilobytes of raw stdout into the user's chat viewport. Subagents now render as clean, collapsible badges with role icons (`🔭 Scout`, `🧪 Tester`, `📚 Researcher`, `⚙️ Worker`), truncated 5-line previews, and `Ctrl+O` expand/collapse toggles, mirroring background task behavior.
- **Synchronous Subagent Badge Deduplication**: Eliminated duplicate badges (`completed` + `completed`) in synchronous subagent execution by completing synchronous tasks with `acknowledged: true`, preventing redundant system notifications during the event loop drain.
- **Git Argument & Shell Escaping in Scout**: Replaced unescaped shell string interpolation in `run_scout_task` with structured argv array execution (`Command::new("git").args(...)`), regex token extraction, and silent stderr suppression of `fatal: not a git repository` / `GIT_DISCOVERY_ACROSS_FILESYSTEM` on non-git workspaces.
- **Subprocess Stdio Containment**: Sealed subagent child process execution with `Stdio::null()` for stdin and fully captured piped buffers for stdout/stderr, ensuring zero leaked error traces into the parent TUI terminal.
- **Test Browser Isolation**: Protected detached URL launcher with test configuration checks (`if cfg!(test) { return; }`), preventing browser popups (e.g., sluisr.com) when compiling or running test suites.
- **Terminal Window Focus & Ghost Click Protection**: Resolved issue where focusing or activating the terminal window via mouse click inadvertently opened hyperlinks. Added a 400ms `FocusGained` guard and an 800ms startup grace period to strictly discard background focus clicks.
- **Detached URL Launcher (Zero Dependency)**: Removed the external `open` crate dependency in favor of native asynchronous detached process spawning (`xdg-open`, `gio`, `open`, `powershell`), completely isolating the terminal event loop from browser launch latency.
- **Rapid-Click Hyperlink Debouncing**: Prevented multiple browser tabs from spawning upon double or rapid clicking with a 1.5s per-URL and 3.0s global debounce window.
- **Conflicting Multiplexer Shortcuts**: Cleaned up legacy `Alt+Down` bindings that conflicted with terminal multiplexers (e.g. `tmux` pane navigation).
- **Fleet Mode Security & SSH Hardening (`corex-fleet`)**: Enforced confirmation prompts for `sudo` executions (`sudo: true` or `node.sudo`), wrapped remote sudo commands via POSIX `sudo -n -- sh -c '<cmd>'`, escaped remote working directories and completions with `shell_quote_single`, isolated trailing `#` comments in remote command wrappers, bounded concurrent SSH fan-out (`Semaphore::new(16)`) with deterministic node ordering, and hardened `~/.ssh/config` parsing (`Key=Value`, multi-host aliases, `Match` block skipping, `~/` key expansion, and `0600` atomic `fleet.toml` writes).
- **Resilient Multi-Distro Telemetry & PTY Lifecycle (`fleet_pty`, `fleet_dialog`, `telemetry`)**: Redesigned remote telemetry probing with `LC_ALL=C` and section delimiters (`===CX_*===`) supporting Docker `overlay`/`zfs`/LVM root filesystems and CPU core detection (`cpu_cores`). Added `Drop` child reaping and atomic dirty-flag rendering (`take_dirty`) to `FleetPtySession` to eliminate zombie SSH processes and idle 60 FPS redraws, enabled live PTY rendering in `/fleet wide`, and wired in-pane `PendingRemoteApproval` (`[y] allow`, `[e] edit`, `[w] instruct`, `[n] deny`).
- **Shell Sandbox, Symlink, SSRF & MCP Hardening (`corex-tools`)**: Blocked unbraced `$` variable expansion and mid-token quote/backslash obfuscation in `is_known_safe_command`, removed `curl` from auto-approved safe tools, protected `.corex/`, `.deepseek/`, and `.env` paths, closed symlink workspace escapes in `is_within_workspace`, added manual hop-by-hop SSRF validation on HTTP redirects (blocking `*.nip.io`, `*.sslip.io`, `*.xip.io`), replaced `BufReader::read_line` with a cancellation-safe 16 MiB bounded reader in `McpTransport`, and ensured RAII cleanup (`AskpassGuard`) of temporary `.askpass_cred` files across multi-command and auto-backgrounded `sudo` runs.
- **TUI In-Flight Tool Synchronization & Slash Commands (`corex-tui`, `corex-cli`, `corex-core`)**: Added `in_flight_tool_calls` tracking to prevent multi-tool batch and sudo confirmation race conditions, preserved tool outputs during context compaction (`compacted_snapshot`), wired `CancellationToken` for immediate subagent cancellation, fixed `/key` (`provider_api_keys` persistence without `std::env::set_var` UB), `/fim` (workspace check + auto-streaming), `/prefix`, and `Ctrl+T` (`ThinkingState` toggle), and fixed `--resume` with `-p` headless mode and npm `install.js`.

### Added
- **SysAdmin & Multi-Server Fleet Mode (`corex-fleet`, `cx fleet`, `cxs`, `/fleet`)**: Full SSH fleet orchestration engine, persistent multiplexed sessions, interactive terminal split-view (`/fleet` and `/fleet wide`), parallel cluster broadcast (`cluster:<name>`, `tag:<tag>`, `all`), and live server telemetry cards.
- **Geometric "C" Emblem & Minimalist Static Logo**: Redesigned the primary banner with an elegant geometric chevron emblem (`◢◤` and `◥◣`) in solid satin titanium gray, accompanied by a clean, static satin-white `Corex` title without distracting letter-shimmer animations, structured across an unbroken 2-line layout on both desktop and mobile terminals.
- **Uncluttered Markdown Headers**: Stripped decorative geometric glyphs (`◈`, `◆`, `▸`) from markdown headers `h1`–`h3`, introduced native `h4` styling, and normalized body block indentation to a clean 2 spaces.
- **Matte Satin Status & Telemetry Bar**: Refined bottom-right model indicator and reasoning telemetry with a subdued satin-gray palette, replacing harsh neon colors with soft pastel tones.
- **Smart Return-to-Bottom on `Esc`**: While scrolled up inspecting conversation history or reports, pressing `Esc` immediately snaps the view to the bottom of the feed while **preserving all drafted prompt text** in the composer. Double-tapping `Esc` within 500ms when at the bottom continues to clear the prompt.
- **Auto-Scroll on Message Send**: Pressing `Enter` to submit a message (or auto-dequeuing queued turns) immediately snaps the viewport to the bottom so streaming AI tokens and user prompts are never obscured.
- **Native `End` Key Navigation**: Pressing `End` (or `Fn+Right`) while scrolled up navigates directly to the bottom of the feed and advances the prompt cursor.
- **Comprehensive UX Test Coverage**: Added exhaustive test suites verifying focus guards, URL click hit-testing, selection boundaries, draft preservation, and multi-state `Esc` navigation.
- **Clean UNIX Tool Call Standard**: Tool calls now render with clean, universal terminal formatting (`$ <cmd>` for shell commands, `read <path>`, `write <path>`, `edit <path>`, `ls <path>`, `grep "<query>" in <path>`, `web "<query>"`), eliminating artificial chevrons and badges from tool execution spans and live streaming previews.
- **Emoji-Free In-Place Self-Updater (`cx update`)**: Stripped decorative emojis from the binary self-updater output, standardizing on clean bullet points and professional status indicators.

---

## [0.6.2] - 2026-10-13

### Fixed
- **Strict Visual URL Click Hit-Testing**: Fixed false-positive where clicking anywhere on a line containing a URL (table borders, descriptions, or whitespace) erroneously launched the browser via `open::that()`. Hit-testing now uses exact visual display columns (`UnicodeWidthChar`), opening links only when clicked directly on the URL text.
- **Uncluttered Question Prompt Border**: Removed awkward bracketed title tags (`─[ Pregunta TUI ]─`) and redundant bullet circles (`●` and `○`), providing an unbroken, clean horizontal border matching the composer design.

### Added
- **Inline Bottom-Anchored User Dialog (`ask_user`)**: Replaced centered floating popups with an elegant, non-intrusive selector integrated into the bottom composer slot. Conversation history remains 100% visible, sharp, and readable above the question.
- **Fast 1-Key Numeric Selection & Scrolling**: Immediate option selection via number keys (`1`–`9`) and `PageUp`/`PageDown` chat history scrolling while deciding responses.
- **Unified Minimalist Chevron (`›`)**: Replaced heavy `❯` glyphs across all menus, drawers, and modal prompts (`model_dialog`, `session_dialog`, `auth_dialog`, `slash_commands`, `sudo_dialog`, `diff_view`, and `user_dialog`) with the sleek chevron `›`.
- **Automated Release Notes Pipeline**: Updated release workflow to automatically parse and publish `CHANGELOG.md` release notes directly to GitHub Releases alongside canonical SHA-256 asset digests.

---

## [0.6.1] - 2026-10-13

### Fixed
- **TUI Multi-Line Table Cell Word-Wrapping**: Replaced legacy cell truncation (`…`) with dynamic multi-line word-wrapping in `wrap_cell_spans`, preserving full descriptions, long URLs, and strict table border alignment.
- **Official Identity & YouTube Link**: Corrected official YouTube channel reference to `@sluisr_` (with trailing underscore) across all procedural skills and built-ins.
- **Skill Activation Log Noise**: Suppressed raw 100+ line markdown log dumps in the chat feed when procedural skills (`activate_skill`) are loaded.

### Added
- **Smart Scroll Lock & Quick Jump-to-Bottom**: When the user scrolls up to read earlier parts of the conversation during active AI streaming, auto-scrolling locks in place so arriving tokens do not pull the viewport down. Pressing `Space` (when the composer input is empty), `End`, or `Esc` immediately snaps back to the bottom and re-enables auto-scroll, with a subtle soft-tinted border indicator on the composer.
- **Embedded Built-in Skills**: Compiled all 8 foundational domain skills (`corex-about`, `corex-fim`, `corex-local-llm`, etc.) directly into the binary with zero-latency (0.001ms) fallback and automatic system prompt baseline discovery in empty or `/tmp` workspaces.
- **Interactive Clickable Terminal Hyperlinks**: Added native single-click mouse handling on chat feed URLs to directly launch the system default browser via `open::that()`.
- **Inline Markdown Link & URL Styling**: Styled all HTTP/HTTPS links, emails, and Markdown URLs in bright blue (`accent_blue`) with underlines (`UNDERLINED`).
- **Clean Link Deduplication**: Automatically collapses redundant Markdown link labels that duplicate the target URL (e.g. `[sluisr.com](https://sluisr.com)`) into single, clean, clickable URLs.
- **Technical TUI Chrome**: Replaced decorative emojis across tool call banners and action spans with clean, minimal technical indicators (`› `, `✓`, `✕`).

---

## [0.6.0] - 2026-10-13

### Added
- **Autonomous Multi-Agent Coordination Engine (Subagents)**:
  - **Specialized Actor Roles**: Introduced `SubagentRole` with 4 dedicated roles:
    - 🔭 `Scout`: Zero-cost read-only codebase exploration and symbol searching before cloud edits.
    - 🧪 `Tester`: Automated background test runner, linter checker, and compiler validator.
    - 📚 `Researcher`: Documentation reader, API specification extractor, and reference finder.
    - ⚙️ `Worker`: General-purpose autonomous subtask executor.
  - **Isolated Context Boundaries**: Subagents run with isolated context memory, preventing context window exhaustion in the lead orchestrator turn.
  - **Native Agent Tools**: Added `delegate_subagent` (synchronous or background delegation) and `manage_subagents` (`list`, `status`, `cancel`) registered in the core runtime.
  - **Reactive Notifications**: Background subagents automatically wake up the lead agent upon completion without requiring polling.
- **Architectural Refactoring of `corex-tui`**:
  - Decomposed the legacy monolithic `app.rs` (~5,870 lines) into a modular structure:
    - `app/mod.rs`: Application core, session state, slash command dispatch, and lifecycle.
    - `app/event_loop.rs`: Async event loop, multi-channel streaming, keyboard and mouse event dispatching.
    - `app/render.rs`: Responsive Ratatui rendering pipelines, viewport layout, message feed, and borders.
    - `app/formatting.rs`: Span builder, Markdown rendering, syntax highlights, and visual badges.
    - `app/mobile.rs`: Termux portrait detection and tap-to-type soft keyboard toggle.
    - `app/state.rs`: Unified reactive app state, selection ranges, and input history.
    - `app/terminal.rs`: Terminal alternate screen and raw mode initialization/restoration.
    - `app/tests.rs`: Comprehensive test suite for TUI components.
- **In-Place Self-Updater (`cx update` / `cx --upgrade` / `/update apply`)**:
  - Direct atomic binary upgrade without requiring external package managers (`cargo` or `npm`).
  - Automatic OS/Arch detection (`linux-x64`, `linux-arm64`, `android-arm64`, `macos-arm64`, `macos-x64`, `windows-x64`).
  - Cryptographic verification via SHA-256 release checksums before binary extraction.
  - In-place atomic replacement via `fs::rename` avoiding Linux `ETXTBSY` ("Text file busy") and Windows file locking.
  - Interactive TUI update command `/update apply` and force reinstall via `cx update --force`.
- **Full Shell Autocompletion**:
  - Added `clap_complete` generating contextual shell autocompletions for Bash, Zsh, Fish, PowerShell, and Elvish.
  - New flag `cx --completions <shell>` and command `cx completions <shell>`.
  - Automatic completion script setup in `install.sh` for Zsh (`site-functions`), Bash (`bash_completion.d`), and Fish (`completions`).
- **Native Linux ARM64 (aarch64) Support**:
  - Dedicated `aarch64-unknown-linux-gnu` cross-compilation target in release CI workflow for Raspberry Pi, AWS Graviton, Hetzner ARM, and Docker ARM64.
  - Clean separation from `aarch64-linux-android` (Termux) in `install.sh` and npm installer.
- **Resilient JSON Repair Engine & Unified Multi-Provider Tool Calling**:
  - Implemented `parse_lenient_json` to seamlessly repair Markdown codeblocks, nested stringified JSON payloads, trailing commas, single quotes, and truncated SSE streams.
  - Flexible deserialization for non-standard tool call indices and raw object arguments (Ollama, Gemini, Claude 3.7, DeepSeek V4.1).
- **Interactive MCP Server Catalog & Commands**:
  - Added `/mcp catalog`, `/mcp add <catalog_id|custom>`, and `/mcp remove <name>` directly inside the TUI for seamless tool expansion.
- **Internal Engine Invariant Verification**:
  - Implemented compile-time invariant validation and internal matrix verification in the core runtime.
- **Nix Flake & Ecosystem Packaging**:
  - Added native `flake.nix` for declarative zero-install Nix flakes.
  - Memory trimmer routine (`trim_memory` via glibc `malloc_trim`) to prevent memory fragmentation and ensure minimal RSS usage.
  - Documentation and distribution assets for Homebrew tap and Crates.io publication.
- **CI/CD Pipeline Automation**:
  - Automated release publishing workflow to both Crates.io and npm Registry on tag push (`v*`).

---

## [0.5.0] - 2026-10-10

### Added
- **Multi-Provider Ecosystem (Real Official Protocols)**: Full native protocol support for OpenAI (`/chat/completions`, `o1`/`o3-mini` reasoning effort & temperature stripping), Anthropic Claude (dedicated `/v1/messages` SSE adapter, `tool_use`/`tool_result` translation), Google Gemini (`/v1beta/openai`), GitHub Models (`models.inference.ai.azure.com`), Groq (500+ tokens/s Free Tier), OpenRouter, and Mistral AI.
- **Mobile Vertical / Termux Responsive UX**:
  - **Touch-to-Keyboard in Termux**: Automatically detects mobile vertical (portrait) screens and disables terminal mouse reporting by default, allowing native Android screen taps in Termux to open the virtual keyboard instantly.
  - **Mobile Portrait Responsive Layout**: Redesigned header banner, 2-column non-overlapping status bar, adaptive input composer placeholder, and full-width/stacked modals for `/model`, `/resume`, and auth dialogs on screens narrower than 68 columns.
- **Google Gemini 3.x Streaming & Thought Signatures**: Full support for `gemini-3.5-flash-lite`, `gemini-3.8-flash`, and Google AI Studio OpenAI compatibility endpoint, handling delta tool call chunk schemas, `thought_signature` state preservation across tool turns, and automatic legacy sanitization.
- **Automated Checksum & Dynamic NPM Installer**: Replaced manual SHA-256 hash maintenance with automatic release checksum generation in GitHub Actions and dynamic verification in the npm installer.
- **Non-Intrusive Dynamic Auth Flow**: Eliminated forced startup modal; Corex now starts cleanly and contextually requests the exact API key corresponding to the selected provider with direct portal links.

---

## [0.4.0] - 2026-10-09

### Added
- **Master-Detail Model Switcher (`/model`)**: Modular provider catalog replacing hardcoded tabs. Two-column layout with status indicators (`● active`, `○ ready`, `◌ needs key`), dynamic per-model setting sliders (Temperature, General CoT, Tool CoT, Coding CoT, Web Search CoT), and secret key masking.
- **Interactive Split-View Session Explorer (`/resume`)**: Revamped session picker featuring live realtime fuzzy filtering (`Filter: ...`), dual-pane layout with compact session list on the left and full contextual preview on the right (turn counts, token consumption, and latest user/assistant dialogue snippet).
- **Categorized Slash Command Menu (`/`)**: Grouped command drawer partitioned into logical categories (Session, Model, Context, Tools, App) with substring & fuzzy matching, argument placeholders, and integrated alias support (`/wallet` -> `/balance`, `/compress` -> `/compact`, `/search` -> `/web`).
- **Unified Modal Overlay Engine (`begin_modal`)**: Centralized modal isolation across all dialogs (`/model`, `/resume`, auth, `/sudo`, user prompts) with background terminal dimming and anti-collision perimeter halos.

---

## [0.3.0] - 2026-10-09

### Breaking Changes
- **Legacy `uti` Deprecated & Removed**: Fully removed legacy `uti` binary target, `UTI_API_KEY`, and automatic migration fallbacks from `~/.uti/`. Invocations are now exclusively **`cx`** and **`corex`**.
- **CLI Crate Modular Architecture**: Refactored `corex-cli` from monolithic `main.rs` into a reusable library (`corex_cli`) with dedicated binary entrypoints (`src/bin/cx.rs`, `src/bin/corex.rs`).

### Added
- **Native Clipboard Integration**: Terminal clipboard copy support via OSC 52 escape sequences and system clipboard integration.
- **Prompt Queuing While Streaming**: Support for composing and pasting follow-up prompts while the model is streaming responses, with visual `[queued]` badges and sequential execution.
- **Mouse Selection & Drag Scrolling**: Bidirectional mouse text selection in the chat viewport with copy support and edge drag scrolling.
- **Fuzzy Patch Engine**: Enhanced `apply_patch` tool with fuzzy whitespace matching, accurate hunk offsets, fence stripping, and LF/CRLF preservation.
- **Concurrency Locking & Secure Filesystem**: Path-based mutex locking (`lock_path`) to prevent file corruption during parallel operations, atomic private writes, and terminal escape sequence sanitization.
- **Protected Paths Enforcement**: Path guard blocking modification or deletion of sensitive credentials (SSH, GPG, git configs, system paths) even in YOLO mode.
- **Hardened Shell Sandbox**: Expanded safe command whitelist (`git`, `tar`, `unzip`, `systemctl`), malicious redirection prevention, and POSIX signal exit status tracking (`128 + signal`).
- **Memory Compaction**: Automatic OS memory page reclamation on Linux via glibc `malloc_trim(0)` after large reasoning turns.
- **Non-blocking Balance Queries**: `/balance` and `/wallet` run asynchronously in the background without freezing the TUI.
- **Agent Skills Suite**: 8 specialized skills in `.agents/skills/` for local LLMs, FIM autocompletion, MCP setup, session management, and troubleshooting.
- **CI & Security Auditing**: Added automated GitHub Actions workflow for clippy, cargo tests, and RustSec vulnerability audits.
- **NPM Cryptographic Verification**: Corporate-grade SHA-256 binary checksum verification and redirect-safe download pipeline in the npm installer.

---

## [0.2.0] - 2026-09-17

### Added
- **Fixed Real-Time Activity Bar**: Dedicated activity line directly above the composer with a 60 FPS continuous braille spinner (`⣾ ⣽ ⣻ ⢿ ⡿ ⣟ ⣯ ⣷`).
- **Silk Wave Status Transitions**: Smooth, character-level cascading text transition across model states and tool executions, calibrated with a soft satin palette (`#969BAA`) to eliminate terminal strobe flickering.
- **Clickable Terminal Hyperlinks**: Interactive mouse click detection in the chat feed. Clicking any URL instantly opens the link in the user's default browser (`xdg-open` on Linux, default browser on Windows and macOS).
- **Refined `/info` Card**: Native Unicode framed box card displaying creator credits (`sluisr`), official website, changelog, bug report links, and live session telemetry.
- **Parallel Tool Batching**: Clean, single-event summaries for parallel tasks (`Running N commands in parallel...`) avoiding multi-event spam.
- **Multiplatform Release CI**: Automated GitHub Actions matrix generating pre-built release binaries for Linux (`x86_64`), Windows (`x86_64`), and macOS (`aarch64` & `x86_64`).
- **Auto-Update Checker**: Background version checking module and `/update` command.

### Fixed
- Fixed visual terminal flicker caused by high-contrast RGB luminance swings during status transitions.
- Fixed spinner freezing when interactive dialogs (`pending_confirmation`, `user_dialog`, `sudo_dialog`) are awaiting user response.
- Fixed border misalignment and line overflow in box cards caused by wide emoji characters.
- Fixed rapid-fire event collisions when multiple shell commands run concurrently.

---

## [0.1.0] - 2026-09-01

### Added
- Initial release of **Corex CLI**.
- Pure native Rust architecture (Tokio, Ratatui, Crossterm).
- DeepSeek V3 / V4 integration with native KV Cache discount tracking.
- Local SLM engine support ($0.00 cost hybrid reasoning).
- Interactive TUI with full markdown rendering, syntax highlighting, and bash/tool execution.
- Sudo security integration with in-memory session password caching.
