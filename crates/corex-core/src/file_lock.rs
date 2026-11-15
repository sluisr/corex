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
        .unwrap_or_else(|| {
            #[cfg(unix)]
            {
                let uid = unsafe { libc::getuid() };
                std::env::temp_dir().join(format!("corex-locks-{}", uid))
            }
            #[cfg(not(unix))]
            {
                std::env::temp_dir().join("corex-locks")
            }
        })
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
        if let Err(e) = crate::secure_fs::ensure_private_dir(parent) {
            tracing::warn!(
                "[LOCK] cannot create {}: {} — falling back to in-process locking only",
                parent.display(),
                e
            );
            return None;
        }
    }

    // Pruning can unlink the file between open and lock; a lock on that inode excludes nobody.
    for _ in 0..8 {
        let file = match open_lock_file(&lock_file) {
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
        if still_linked(&file, &lock_file) {
            return Some(file);
        }
    }
    tracing::warn!("[LOCK] {} kept being replaced — falling back to in-process locking only", lock_file.display());
    None
}

fn open_lock_file(lock_file: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(lock_file)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = file.metadata() {
            let mut perms = meta.permissions();
            if perms.mode() & 0o077 != 0 {
                perms.set_mode(0o600);
                let _ = file.set_permissions(perms);
            }
        }
    }
    Ok(file)
}

#[cfg(unix)]
fn still_linked(file: &std::fs::File, lock_file: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), std::fs::symlink_metadata(lock_file)) {
        (Ok(held), Ok(named)) => held.ino() == named.ino() && held.dev() == named.dev(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn still_linked(_file: &std::fs::File, _lock_file: &Path) -> bool {
    true
}

pub fn prune_stale_locks(max_age: std::time::Duration) -> usize {
    prune_stale_locks_in(&locks_dir(), max_age)
}

fn prune_stale_locks_in(dir: &Path, max_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "lock") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= max_age);
        if !old {
            continue;
        }
        let Ok(file) = open_lock_file(&path) else { continue };
        if file.try_lock().is_ok() && still_linked(&file, &path) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
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
fn normalize_lock_key(path: &Path) -> PathBuf {
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut tail = Vec::new();
    let mut cur: &Path = &abs;
    while let Some(name) = cur.file_name() {
        tail.push(name.to_os_string());
        if let Some(parent) = cur.parent() {
            if let Ok(canon_parent) = parent.canonicalize() {
                let mut result = canon_parent;
                for part in tail.into_iter().rev() {
                    if part == "." {
                        continue;
                    } else if part == ".." {
                        result.pop();
                    } else {
                        result.push(part);
                    }
                }
                return result;
            }
            cur = parent;
        } else {
            break;
        }
    }
    abs
}

pub async fn lock_path(path: &Path) -> PathGuard {
    let key = normalize_lock_key(path);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn lock_dir_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let target = std::env::temp_dir().join(format!("corex_privlock_{}.txt", std::process::id()));
        std::fs::write(&target, "x").unwrap();
        let lock_file = lock_file_for(&target.canonicalize().unwrap());

        let _guard = lock_path(&target).await;
        let dir_mode = std::fs::metadata(locks_dir()).unwrap().permissions().mode() & 0o077;
        let file_mode = std::fs::metadata(&lock_file).unwrap().permissions().mode() & 0o077;
        assert_eq!(dir_mode, 0, "locks_dir must not grant group/other permissions");
        assert_eq!(file_mode, 0, "lock file must not grant group/other permissions");

        let _ = std::fs::remove_file(&target);
    }

    #[cfg(unix)]
    #[test]
    fn pruning_skips_held_and_recent_locks() {
        let dir = std::env::temp_dir().join(format!("corex_prune_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let idle = dir.join("idle.lock");
        let held = dir.join("held.lock");
        std::fs::write(&idle, "").unwrap();
        std::fs::write(&held, "").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let holder = open_lock_file(&held).unwrap();
        holder.lock().unwrap();

        assert_eq!(prune_stale_locks_in(&dir, std::time::Duration::from_secs(3600)), 0, "recent files stay");
        assert_eq!(prune_stale_locks_in(&dir, std::time::Duration::ZERO), 1);
        assert!(!idle.exists() && held.exists() && dir.join("notes.txt").exists());

        drop(holder);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_taken_on_an_unlinked_file_is_retaken() {
        let target = std::env::temp_dir().join(format!("corex_relink_{}.txt", std::process::id()));
        let lock_file = lock_file_for(&target);
        crate::secure_fs::ensure_private_dir(lock_file.parent().unwrap()).unwrap();
        let first = open_lock_file(&lock_file).unwrap();
        std::fs::remove_file(&lock_file).unwrap();
        assert!(!still_linked(&first, &lock_file));

        let guard = acquire_os_lock(&target).expect("locks the file that now has the name");
        assert!(still_linked(&guard, &lock_file));
        drop(guard);
        let _ = std::fs::remove_file(&lock_file);
    }
}
