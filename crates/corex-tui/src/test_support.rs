//! Test-only fixtures.

use std::path::PathBuf;
use std::sync::OnceLock;

static TEST_HOME: OnceLock<PathBuf> = OnceLock::new();

/// Points `$HOME` at a throwaway directory for the rest of the test process.
///
/// Sessions, history and settings live under `~/.corex`, so a test that builds an `App` and saves
/// a session would otherwise write into the real home of whoever runs `cargo test`. Every thread
/// goes through the same `OnceLock`, so none can observe the real home after the first call.
pub(crate) fn isolate_home() {
    TEST_HOME.get_or_init(|| {
        // One fixed directory, emptied on every run, so repeated runs do not pile up folders.
        let dir = std::env::temp_dir().join("corex-tui-test-home");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("HOME", &dir);
        dir
    });
}
