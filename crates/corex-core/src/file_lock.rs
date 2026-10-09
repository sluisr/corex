//! Per-path locking that serializes file mutations *within* this process and *across* processes.
//!
//! Two writers to the same file interleave as follows: both read the original content, both
//! modify their own copy, both write back, and the last `rename` wins while both callers still
//! report success — a silent lost update. `lock_path` closes that window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

/// Registry of per-path mutexes serializing file mutations.
///
/// Tool calls within one batch run as independent Tokio tasks (the TUI spawns each of them), so
/// two writers to the same file would otherwise interleave.
///
/// This lives in `corex-core` (the lowest-level crate) on purpose: every mutation path in the
/// codebase — tool outputs, sessions, settings — must funnel through this *single* registry.
/// Two registries would not see each other's guards and would silently reintroduce the bug.
fn path_locks() -> &'static StdMutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// Held for as long as a guarded read-modify-write is in progress.
///
/// Releasing is entirely drop-based: the in-process mutex frees on drop, and the OS advisory lock
/// frees when its file handle closes — which also happens if the process is killed, so a crash
/// cannot leave the file permanently locked.
pub struct PathGuard {
    _process: tokio::sync::OwnedMutexGuard<()>,
    _os: Option<std::fs::File>,
}

/// Directory holding the sidecar lock files, following the `~/.corex` convention.
fn locks_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".corex").join("locks"))
        .unwrap_or_else(|| std::env::temp_dir().join("corex-locks"))
}

/// FNV-1a, inlined so the path-to-lock-file mapping stays identical across releases. A collision
/// only over-serializes two unrelated paths; it can never let two writers through at once.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Maps a canonical path to a stable sidecar lock file.
fn lock_file_for(path: &Path) -> PathBuf {
    locks_dir().join(format!(
        "{:016x}.lock",
        fnv1a(path.as_os_str().as_encoded_bytes())
    ))
}

/// Takes an exclusive OS advisory lock on a *sidecar* file derived from `path`.
///
/// Locking the target itself would protect nothing: every write in this codebase goes through
/// temp-file + `rename`, so the target's inode is replaced on each write and a lock held on the
/// old inode would be orphaned — a second process would open the new inode, acquire it
/// immediately, and walk straight into the race we are trying to prevent.
///
/// Returns `None` when the OS lock cannot be taken, leaving the caller with in-process protection
/// only. That degradation is logged rather than silent.
fn acquire_os_lock(path: &Path) -> Option<std::fs::File> {
    let lock_file = lock_file_for(path);

    if let Some(parent) = lock_file.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                "[LOCK] cannot create {}: {} — falling back to in-process locking only",
                parent.display(),
                e
            );
            return None;
        }
    }

    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_file)
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                "[LOCK] cannot open {}: {} — falling back to in-process locking only",
                lock_file.display(),
                e
            );
            return None;
        }
    };

    // Blocks until no other process holds it. Safe to block here: the caller moved us onto a
    // blocking thread precisely so an async worker is never parked on this.
    if let Err(e) = file.lock() {
        tracing::warn!(
            "[LOCK] OS lock failed on {}: {} — falling back to in-process locking only",
            lock_file.display(),
            e
        );
        return None;
    }

    Some(file)
}

/// Acquires the lock guarding `path`, serializing mutations of that same file **across processes
/// as well as within this one**.
///
/// The guard must be held across the whole read-modify-write. Two layers, always in this order:
///
/// 1. an in-process async mutex — cheap, and keeps same-process tasks off the blocking pool;
/// 2. an OS advisory lock, so a second `cx` running against the same workspace cannot interleave.
///
/// The order matters. Taking the OS lock first would park blocking threads on a lock that a task
/// inside this very process is still holding, which is a self-inflicted stall.
///
/// Locks whose last holder has gone are pruned so the registry does not grow without bound over a
/// long session.
pub async fn lock_path(path: &Path) -> PathGuard {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mutex = {
        let mut map = path_locks()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        map.retain(|_, lock| Arc::strong_count(lock) > 1);
        map.entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let process_guard = mutex.lock_owned().await;

    // `File::lock` blocks, so it must not run on an async worker thread.
    let os_guard = tokio::task::spawn_blocking(move || acquire_os_lock(&key))
        .await
        .ok()
        .flatten();

    PathGuard {
        _process: process_guard,
        _os: os_guard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `flock -n -x <file> true` in a *real* separate process. `-n` makes it fail
    /// immediately instead of waiting, so a non-zero exit means the lock was already taken.
    fn another_process_can_lock(lock_file: &Path) -> bool {
        std::process::Command::new("flock")
            .arg("-n")
            .arg("-x")
            .arg(lock_file)
            .arg("true")
            .status()
            .expect("flock(1) must be available")
            .success()
    }

    /// The whole point of the sidecar file: a second `cx` process must be locked out, not just a
    /// second task in this one.
    #[tokio::test]
    async fn os_lock_excludes_a_real_second_process() {
        let target = std::env::temp_dir().join(format!("corex_xproc_{}.txt", std::process::id()));
        std::fs::write(&target, "x").unwrap();
        let lock_file = lock_file_for(&target.canonicalize().unwrap());

        let guard = lock_path(&target).await;
        assert!(
            !another_process_can_lock(&lock_file),
            "another process must NOT acquire the lock while this one holds it"
        );

        drop(guard);
        assert!(
            another_process_can_lock(&lock_file),
            "dropping the guard must release the OS lock for other processes"
        );

        let _ = std::fs::remove_file(&target);
    }

    /// Two tasks in this process must still be serialized, and must not deadlock against
    /// themselves by fighting over the OS layer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn in_process_holders_still_serialize() {
        let target = std::env::temp_dir().join(format!("corex_iproc_{}.txt", std::process::id()));
        std::fs::write(&target, "x").unwrap();

        let guard = lock_path(&target).await;
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = entered.clone();
        let path = target.clone();
        let waiter = tokio::spawn(async move {
            let _guard = lock_path(&path).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(
            !entered.load(std::sync::atomic::Ordering::SeqCst),
            "a second holder must not proceed while the first holds the lock"
        );

        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
            .await
            .expect("the waiter must proceed once the guard is dropped")
            .unwrap();
        assert!(entered.load(std::sync::atomic::Ordering::SeqCst));

        let _ = std::fs::remove_file(&target);
    }
}
