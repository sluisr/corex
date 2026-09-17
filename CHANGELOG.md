# Changelog

All notable changes to **UTI CLI** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
