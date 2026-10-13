# Changelog

All notable changes to **Corex** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
- **Security & Engine Authorship Signature**:
  - Implemented XOR-encrypted steganographic authorship signature (`verify_author_signature`, `engine_signature`) in the core binary.
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
