pub mod anthropic;
pub mod client;
pub mod config;
pub mod file_lock;
pub mod forensic;
pub mod history;
pub mod language;
pub mod local_client;
pub mod providers;
pub mod reasoning_cache;
pub mod secure_fs;
pub mod session;
pub mod types;
pub mod update;

pub use client::{get_sudo_password, set_sudo_password, LlmClient, StreamEvent};
pub use config::{Config, McpServerConfig};
pub use file_lock::lock_path;
pub use forensic::ForensicLogger;
pub use history::HistoryStore;
pub use local_client::LocalLlmClient;
pub use reasoning_cache::ReasoningCache;
pub use secure_fs::{ensure_private_dir, sanitize_for_terminal, write_private, write_private_atomic};
pub use session::{Session, SessionSummary};
pub use types::*;

/// Trims free memory pages back to the OS allocator on Linux (glibc malloc_trim).
/// Prevents memory fragmentation and lowers RSS after large tasks/turns.
pub fn trim_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim(0)` is a glibc function that releases free memory pages
    // back to the OS. It has no preconditions and is safe to call at any time.
    // It only affects the calling thread's malloc arena.
    unsafe {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        malloc_trim(0);
    }
}
