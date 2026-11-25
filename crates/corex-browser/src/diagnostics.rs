//! Diagnostics for the Corex Browser Bridge: detects installed Chromium browsers, checks
//! whether the Corex extension (`pgbencaoenigjeekljfindfeaolenaip`) is installed and what
//! version it is, verifies the Native Messaging manifest and launcher binary, and inspects
//! active host sockets and recent native host logs.

use std::path::{Path, PathBuf};

pub const EXTENSION_ID: &str = "pgbencaoenigjeekljfindfeaolenaip";
pub const EXTENSION_STORE_URL: &str =
    "https://chromewebstore.google.com/detail/pgbencaoenigjeekljfindfeaolenaip";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledExtension {
    pub browser_name: String,
    pub profile: String,
    pub version: Option<String>,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct BrowserDiagnostics {
    pub browsers_detected: Vec<(String, PathBuf)>,
    pub extensions: Vec<InstalledExtension>,
    pub native_host_launcher: PathBuf,
    pub launcher_ok: bool,
    pub launcher_issue: Option<String>,
    pub manifests_installed: Vec<PathBuf>,
    pub install_error: Option<String>,
    pub live_socket: Option<PathBuf>,
    pub last_host_log: Vec<String>,
}

/// Scans a Chromium-based browser config directory (e.g. `~/.config/google-chrome`) for
/// `Default` and `Profile *` subdirectories containing the Corex extension.
pub fn find_extensions_in_browser_dir(
    browser_name: &str,
    browser_dir: &Path,
) -> Vec<InstalledExtension> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(browser_dir) else {
        return found;
    };
    let mut profiles: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            if path.is_dir() && (name == "Default" || name.starts_with("Profile ")) {
                Some((name, path))
            } else {
                None
            }
        })
        .collect();
    profiles.sort_by(|a, b| a.0.cmp(&b.0));

    for (profile_name, profile_dir) in profiles {
        let ext_dir = profile_dir.join("Extensions").join(EXTENSION_ID);
        if !ext_dir.is_dir() {
            continue;
        }
        let version = read_extension_version(&ext_dir);
        found.push(InstalledExtension {
            browser_name: browser_name.to_string(),
            profile: profile_name,
            version,
            path: ext_dir,
        });
    }
    found
}

fn read_extension_version(ext_dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(ext_dir).ok()?;
    let mut version_dirs: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            p.is_dir().then(|| (e.file_name().to_string_lossy().to_string(), p))
        })
        .collect();
    version_dirs.sort_by(|a, b| b.0.cmp(&a.0));
    for (dir_name, dir_path) in version_dirs {
        let manifest_path = dir_path.join("manifest.json");
        if let Ok(raw) = std::fs::read_to_string(&manifest_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(ver) = v.get("version").and_then(|x| x.as_str()) {
                    return Some(ver.to_string());
                }
            }
        }
        if !dir_name.is_empty() {
            return Some(dir_name);
        }
    }
    None
}

/// Checks that the native host launcher script exists and its `exec '<path>'` target still exists.
pub fn check_launcher(launcher: &Path) -> (bool, Option<String>) {
    if !launcher.is_file() {
        return (
            false,
            Some(format!("launcher missing at {}", launcher.display())),
        );
    }
    let Ok(content) = std::fs::read_to_string(launcher) else {
        return (
            false,
            Some(format!("launcher unreadable at {}", launcher.display())),
        );
    };
    if let Some(exec_line) = content.lines().find(|l| l.starts_with("exec '")) {
        if let Some(rest) = exec_line.strip_prefix("exec '") {
            if let Some(end) = rest.find("' ") {
                let target = rest[..end].replace("'\\''", "'");
                if !Path::new(&target).is_file() {
                    return (
                        false,
                        Some(format!(
                            "launcher points to missing binary '{}' (run `/browser repair`)",
                            target
                        )),
                    );
                }
            }
        }
    }
    (true, None)
}

pub fn read_tail_lines(path: &Path, max_lines: usize) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<String> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    if lines.len() <= max_lines {
        lines
    } else {
        lines[lines.len() - max_lines..].to_vec()
    }
}

impl BrowserDiagnostics {
    /// Returns a clear, specific error message explaining why the bridge did not connect.
    pub fn diagnosis_error(&self) -> String {
        if let Some(ref err) = self.install_error {
            return format!(
                "Failed to configure the Corex Browser Bridge native host: {err}. \
                 Run `/browser repair` or check write permissions on your browser config directory."
            );
        }
        if self.browsers_detected.is_empty() {
            return format!(
                "No supported Chromium-based browser (Chrome, Brave, Chromium, Edge, Vivaldi) was detected. \
                 Install a Chromium browser and the Corex Browser Bridge extension: {EXTENSION_STORE_URL}"
            );
        }
        if self.extensions.is_empty() {
            let names: Vec<&str> = self.browsers_detected.iter().map(|(n, _)| n.as_str()).collect();
            return format!(
                "The Corex Browser Bridge extension is NOT installed in any detected browser ({}). \
                 Install it from {EXTENSION_STORE_URL}, then open the browser.",
                names.join(", ")
            );
        }
        if !self.launcher_ok || self.manifests_installed.is_empty() {
            let issue = self
                .launcher_issue
                .as_deref()
                .unwrap_or("Native Messaging manifest is missing");
            return format!(
                "Corex Browser Bridge is installed, but the native host is broken ({issue}). \
                 Run `/browser repair` and reload the extension in chrome://extensions."
            );
        }

        let ext_summary: Vec<String> = self
            .extensions
            .iter()
            .map(|e| match &e.version {
                Some(v) => format!("v{v} in {} ({})", e.browser_name, e.profile),
                None => format!("{} ({})", e.browser_name, e.profile),
            })
            .collect();
        let mut msg = format!(
            "Corex Browser Bridge is installed ({}) and the native host is configured, \
             but no active connection was found. Open your browser, verify the extension is enabled \
             (or reload it in chrome://extensions).",
            ext_summary.join(", ")
        );
        if let Some(last) = self.last_host_log.last() {
            msg.push_str(&format!("\nLast native host log: {last}"));
        }
        msg
    }

    /// Formats a human-readable status summary for `/browser status`.
    pub fn format_report(&self) -> String {
        let mut out = String::from("Corex Browser Bridge Status:\n");
        if self.browsers_detected.is_empty() {
            out.push_str("  • Browsers:     None detected (Chrome, Brave, Chromium, Edge, Vivaldi)\n");
        } else {
            let list: Vec<String> = self
                .browsers_detected
                .iter()
                .map(|(name, dir)| format!("{name} ({})", dir.display()))
                .collect();
            out.push_str(&format!("  • Browsers:     {}\n", list.join(", ")));
        }

        if self.extensions.is_empty() {
            out.push_str(&format!(
                "  • Extension:    [NOT INSTALLED] Install from {EXTENSION_STORE_URL}\n"
            ));
        } else {
            let list: Vec<String> = self
                .extensions
                .iter()
                .map(|e| match &e.version {
                    Some(v) => format!("v{v} in {} ({})", e.browser_name, e.profile),
                    None => format!("installed in {} ({})", e.browser_name, e.profile),
                })
                .collect();
            out.push_str(&format!("  • Extension:    [INSTALLED] {}\n", list.join(", ")));
        }

        if let Some(ref err) = self.install_error {
            out.push_str(&format!("  • Native Host:  [ERROR] {err}\n"));
        } else if !self.launcher_ok {
            out.push_str(&format!(
                "  • Native Host:  [BROKEN] {}\n",
                self.launcher_issue.as_deref().unwrap_or("launcher issue")
            ));
        } else {
            out.push_str(&format!(
                "  • Native Host:  [OK] {} ({} manifest{})\n",
                self.native_host_launcher.display(),
                self.manifests_installed.len(),
                if self.manifests_installed.len() == 1 { "" } else { "s" }
            ));
        }

        match &self.live_socket {
            Some(sock) => {
                let name = sock
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("active");
                out.push_str(&format!("  • Connection:   [CONNECTED] Live socket ({name})\n"));
            }
            None => {
                out.push_str("  • Connection:   [DISCONNECTED] No active extension socket\n");
            }
        }

        if !self.last_host_log.is_empty() {
            out.push_str("  • Recent Host Log:\n");
            for line in &self.last_host_log {
                out.push_str(&format!("      {line}\n"));
            }
        }

        if self.live_socket.is_none() {
            out.push_str(&format!("\nDiagnosis: {}", self.diagnosis_error()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_installed_extension_and_reads_manifest_version() {
        let root = std::env::temp_dir().join(format!("corex-browser-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ver_dir = root
            .join("Default")
            .join("Extensions")
            .join(EXTENSION_ID)
            .join("1.4.2_0");
        std::fs::create_dir_all(&ver_dir).unwrap();
        std::fs::write(ver_dir.join("manifest.json"), r#"{"version":"1.4.2"}"#).unwrap();

        let found = find_extensions_in_browser_dir("Brave", &root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].browser_name, "Brave");
        assert_eq!(found[0].profile, "Default");
        assert_eq!(found[0].version.as_deref(), Some("1.4.2"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diagnosis_distinguishes_missing_extension_from_closed_browser() {
        let missing = BrowserDiagnostics {
            browsers_detected: vec![("Brave".into(), PathBuf::from("/tmp/brave"))],
            extensions: Vec::new(),
            native_host_launcher: PathBuf::from("/tmp/host"),
            launcher_ok: true,
            launcher_issue: None,
            manifests_installed: vec![PathBuf::from("/tmp/m.json")],
            install_error: None,
            live_socket: None,
            last_host_log: Vec::new(),
        };
        let msg = missing.diagnosis_error();
        assert!(msg.contains("NOT installed"), "{msg}");
        assert!(msg.contains(EXTENSION_STORE_URL), "{msg}");

        let installed = BrowserDiagnostics {
            extensions: vec![InstalledExtension {
                browser_name: "Brave".into(),
                profile: "Default".into(),
                version: Some("1.4.2".into()),
                path: PathBuf::from("/tmp/ext"),
            }],
            last_host_log: vec!["[12:00:00] closed by browser".into()],
            ..missing
        };
        let msg2 = installed.diagnosis_error();
        assert!(msg2.contains("v1.4.2 in Brave (Default)"), "{msg2}");
        assert!(msg2.contains("closed by browser"), "{msg2}");
    }
}
