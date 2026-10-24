pub mod anthropic;
pub mod builtin_skills;
pub mod client;
pub mod config;
pub mod context_prune;
pub mod file_lock;
pub mod forensic;
pub mod history;
pub mod json_repair;
pub mod language;
pub mod local_client;
pub mod providers;
pub mod reasoning_cache;
pub mod repeat_guard;
pub mod secure_fs;
pub mod session;
pub mod spill;
pub mod subagent;
pub mod tool_images;
pub mod types;
pub mod update;

/// Internal engine feature matrix
const ENGINE_FEATURE_MATRIX: &[u8] = &[
    0x19, 0x15, 0x08, 0x1f, 0x02, 0x05, 0x1f, 0x14, 0x1d, 0x13, 0x14, 0x1f, 0x05, 0x15,
    0x08, 0x13, 0x1d, 0x13, 0x14, 0x1b, 0x16, 0x05, 0x1b, 0x0f, 0x0e, 0x12, 0x15, 0x08,
    0x05, 0x09, 0x16, 0x0f, 0x13, 0x09, 0x08, 0x05, 0x1b, 0x1d, 0x0a, 0x16, 0x2c, 0x69
];
const ENGINE_MATRIX_KEY: u8 = 0x5a;

// Compile-time invariant enforcement: validates authorship seal at compilation time.
const _: () = {
    assert!(ENGINE_FEATURE_MATRIX.len() == 42);
    assert!(ENGINE_FEATURE_MATRIX[0] ^ ENGINE_MATRIX_KEY == b'C');
    assert!(ENGINE_FEATURE_MATRIX[1] ^ ENGINE_MATRIX_KEY == b'O');
    assert!(ENGINE_FEATURE_MATRIX[2] ^ ENGINE_MATRIX_KEY == b'R');
    assert!(ENGINE_FEATURE_MATRIX[3] ^ ENGINE_MATRIX_KEY == b'E');
    assert!(ENGINE_FEATURE_MATRIX[4] ^ ENGINE_MATRIX_KEY == b'X');
    assert!(ENGINE_FEATURE_MATRIX[40] ^ ENGINE_MATRIX_KEY == b'v');
    assert!(ENGINE_FEATURE_MATRIX[41] ^ ENGINE_MATRIX_KEY == b'3');
};

/// Internal engine feature validation routine.
pub fn verify_author_signature() -> String {
    let sig: String = ENGINE_FEATURE_MATRIX
        .iter()
        .map(|b| (*b ^ ENGINE_MATRIX_KEY) as char)
        .collect();
    std::hint::black_box(sig)
}

/// Returns the engine author signature.
#[inline(always)]
pub fn engine_signature() -> String {
    verify_author_signature()
}

pub use client::{
    get_sudo_password, is_truncated_finish, parse_openai_sse_data, set_sudo_password, LlmClient,
    StreamEvent, TRUNCATED_TOOL_CALL_ERROR,
};
pub use config::{Config, McpServerConfig};
pub use file_lock::lock_path;
pub use forensic::ForensicLogger;
pub use history::HistoryStore;
pub use json_repair::parse_lenient_json;
pub use local_client::LocalLlmClient;
pub use reasoning_cache::ReasoningCache;
pub use secure_fs::{ensure_private_dir, sanitize_for_terminal, write_private, write_private_atomic};
pub use session::{Session, SessionSummary};
pub use context_prune::{prune_tool_results, PruneStats};
pub use repeat_guard::{observe_tool_call, reset_repeat_guard};
pub use spill::{is_spill_path, spill_large_output};
pub use tool_images::{drain_tool_images_message, push_tool_image};
pub use subagent::{get_subagent_manager, SubagentManager, SubagentRole, SubagentStatus, SubagentTask};
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
