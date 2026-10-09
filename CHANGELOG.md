# Changelog

All notable changes to **Corex** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
