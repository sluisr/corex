//! Helpers for persisting user-private data (credentials, sessions, history, logs).
//!
//! Everything Corex stores under `~/.corex` may contain secrets: API keys, prompts, tool output
//! that echoed tokens, etc. Files are therefore created `0600` *at open time* (no window in which
//! they are world-readable) and directories `0700`.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// True for shared or system directories whose permissions must never be tightened: forcing
/// `0700` on `/tmp` or `/home` would lock every other user out of the machine. A previous
/// fallback (`unwrap_or_else(|| PathBuf::from("/tmp"))`) could reach here with `/tmp`.
fn is_protected_system_dir(dir: &Path) -> bool {
    // The filesystem root ("/") has no parent.
    if dir.parent().is_none() {
        return true;
    }
    let s = dir.to_string_lossy();
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() {
        return true;
    }
    matches!(
        trimmed,
        "/tmp" | "/var/tmp" | "/dev/shm" | "/usr" | "/etc" | "/home" | "/Users"
            | "/var" | "/opt" | "/root" | "/bin" | "/sbin" | "/lib"
    )
}

/// Creates `dir` (and parents) restricted to the current user. Tightens permissions of an
/// already-existing directory as well, so installs created by older versions get fixed.
///
/// Shared/system directories are deliberately left untouched (see [`is_protected_system_dir`]).
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    if is_protected_system_dir(dir) {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !dir.exists() {
            fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        } else {
            let mut perms = fs::metadata(dir)?.permissions();
            if perms.mode() & 0o077 != 0 {
                perms.set_mode(0o700);
                let _ = fs::set_permissions(dir, perms);
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Opens `path` for writing (truncate) with `0600` permissions from the very first byte.
pub fn open_private(path: &Path, append: bool) -> io::Result<fs::File> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            ensure_private_dir(parent)?;
        }
    }
    let mut opts = fs::OpenOptions::new();
    opts.create(true);
    if append {
        opts.append(true);
    } else {
        opts.write(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    // `mode()` only applies on creation: tighten pre-existing files too.
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

/// Writes `content` to `path` with `0600` permissions.
pub fn write_private(path: &Path, content: impl AsRef<[u8]>) -> io::Result<()> {
    let mut f = open_private(path, false)?;
    f.write_all(content.as_ref())?;
    f.flush()
}

/// Atomically replaces `path` with `content` (temp file + rename), keeping `0600` permissions.
pub fn write_private_atomic(path: &Path, content: impl AsRef<[u8]>) -> io::Result<()> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let tmp = path.with_file_name(format!(".{}.tmp.{}.{}", file_name, std::process::id(), ts));
    if let Err(e) = write_private(&tmp, content) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Replaces terminal control characters (ESC, CR, BEL, backspace, C1 controls...) so that text
/// controlled by the model or by a remote server cannot rewrite what the user sees — e.g. hide the
/// real command behind `\r` or `\x1b[2K` in a confirmation prompt. Newlines and tabs are kept.
pub fn sanitize_for_terminal(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\n' || c == '\t' {
                c
            } else if c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}') {
                // C0/C1 controls and bidi overrides ("Trojan Source").
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_escape_sequences_and_carriage_returns() {
        let evil = "ls\r\x1b[2Krm -rf ~\x07";
        let clean = sanitize_for_terminal(evil);
        assert!(!clean.contains('\x1b'));
        assert!(!clean.contains('\r'));
        assert!(!clean.contains('\x07'));
        assert!(clean.contains("rm -rf ~"), "the real payload must stay visible");
    }

    #[test]
    fn sanitize_keeps_newlines_tabs_and_unicode() {
        assert_eq!(sanitize_for_terminal("a\n\tñ✓"), "a\n\tñ✓");
    }

    #[test]
    fn sanitize_strips_bidi_overrides() {
        assert!(!sanitize_for_terminal("a\u{202E}b").contains('\u{202E}'));
    }

    #[cfg(unix)]
    #[test]
    fn protected_system_dirs_are_not_tightened() {
        use std::os::unix::fs::PermissionsExt;

        assert!(is_protected_system_dir(Path::new("/")));
        assert!(is_protected_system_dir(Path::new("/tmp")));
        assert!(is_protected_system_dir(Path::new("/home")));
        assert!(!is_protected_system_dir(Path::new("/home/alice/.corex")));

        // The real /tmp is world-shared; its mode must survive the call untouched.
        let before = fs::metadata("/tmp").unwrap().permissions().mode() & 0o777;
        ensure_private_dir(Path::new("/tmp")).unwrap();
        let after = fs::metadata("/tmp").unwrap().permissions().mode() & 0o777;
        assert_eq!(before, after, "ensure_private_dir must not chmod /tmp");
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_0600_and_dirs_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("corex_secure_fs_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("nested").join("secret.json");
        write_private_atomic(&file, "{\"api_key\":\"x\"}").unwrap();
        let fmode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        let dmode = fs::metadata(file.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!(fmode, 0o600);
        assert_eq!(dmode, 0o700);
        let _ = fs::remove_dir_all(&dir);
    }
}
