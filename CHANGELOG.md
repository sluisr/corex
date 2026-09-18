# Changelog

All notable changes to **Corex** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.2.1] - 2026-09-18

### Added
- **Official Rebranding to Corex (`cx`)**: The autonomous coding agent is now **Corex**, launched via the ultra-ergonomic command **`cx`** (adjacent keys on QWERTY keyboards for instant single-hand typing).
- **Backwards Compatibility Aliases**: Maintained full backward compatibility for `corex` and `uti` binary invocations.
- **Prompt Queuing While Streaming**: Users can now type or paste follow-up prompts and press `Enter` while the model is generating responses. Queued prompts appear with a clean non-emoji status badge (`[queued] <prompt> (pending)`) and automatically execute sequentially when the active turn completes.
- **Non-blocking `/balance` Lookup**: `/balance` (and `/wallet`) now queries account token credits and currency balances asynchronously in the background with a 10s timeout, showing an immediate status notification and live-updating once the server responds without blocking terminal interaction.
- **Seamless Config & History Migration**: Unified settings resolution checking `~/.corex/` first with fallback to `~/.uti/` and `~/.deepseek/`.
- **New Environment Variable**: Added support for `COREX_API_KEY` with fallback to `UTI_API_KEY`, `DEEPSEEK_API_KEY`, and `OPENAI_API_KEY`.

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
- Initial release of **UTI CLI (Universal Terminal Intelligence)**.
- Pure native Rust architecture (Tokio, Ratatui, Crossterm).
- DeepSeek V3 / V4 integration with native KV Cache discount tracking.
- Local SLM engine support ($0.00 cost hybrid reasoning).
- Interactive TUI with full markdown rendering, syntax highlighting, and bash/tool execution.
- Sudo security integration with in-memory session password caching.
