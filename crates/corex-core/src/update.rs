use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use chrono::Utc;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCache {
    pub last_checked_sec: i64,
    pub latest_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub html_url: Option<String>,
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone)]
pub struct UpdateSummary {
    pub previous_version: String,
    pub updated_version: String,
    pub target_triple: String,
    pub binary_path: PathBuf,
}

fn get_cache_path() -> Option<PathBuf> {
    BaseDirs::new().map(|d| d.home_dir().join(".corex").join("cache").join("update_check.json"))
}

/// Detects the target triple for the current running operating system and CPU architecture.
pub fn current_target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Some("x86_64-unknown-linux-gnu")
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        if std::env::var("PREFIX").is_ok() || Path::new("/data/data/com.termux").exists() {
            Some("aarch64-linux-android")
        } else {
            Some("aarch64-unknown-linux-gnu")
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some("aarch64-apple-darwin")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Some("x86_64-apple-darwin")
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Some("x86_64-pc-windows-msvc")
    }
    #[cfg(not(any(
        all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")),
        all(target_os = "macos", any(target_arch = "x86_64", target_arch = "aarch64")),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        None
    }
}

/// Parses semver string like "v0.2.1" or "0.2.0" into (major, minor, patch).
pub fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let clean = v.trim().trim_start_matches('v');
    let mut parts = clean.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch_str = parts.next().unwrap_or("0");
    let patch_clean = patch_str.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("0");
    let patch: u32 = patch_clean.parse().ok()?;
    Some((major, minor, patch))
}

/// Returns true if candidate is strictly newer than current.
pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    if let (Some(cur), Some(cand)) = (parse_semver(current), parse_semver(candidate)) {
        cand > cur
    } else {
        false
    }
}

/// Reads the local disk cache. If a cached version exists and is newer than current, returns Some(version).
pub fn check_cached_update(current_version: &str) -> Option<String> {
    let path = get_cache_path().filter(|p| p.exists())?;

    let content = fs::read_to_string(path).ok()?;
    let cache: UpdateCache = serde_json::from_str(&content).ok()?;
    if is_newer_version(current_version, &cache.latest_version) {
        Some(cache.latest_version)
    } else {
        None
    }
}

/// Saves the latest discovered version to disk cache.
fn save_cache(latest_version: &str) {
    if let Some(path) = get_cache_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let cache = UpdateCache {
            last_checked_sec: Utc::now().timestamp(),
            latest_version: latest_version.to_string(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&cache) {
            let _ = fs::write(path, json);
        }
    }
}

/// Verifies whether SHA256 of data matches expected hex string.
pub fn verify_sha256(data: &[u8], expected_checksum: &str) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let computed_hex = format!("{:x}", hasher.finalize());

    let clean_expected = expected_checksum
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();

    if clean_expected.is_empty() {
        return Ok(computed_hex);
    }

    if computed_hex.eq_ignore_ascii_case(&clean_expected) {
        Ok(computed_hex)
    } else {
        Err(format!(
            "Cryptographic signature mismatch! Possible network corruption or tampering.\nExpected: {}\nComputed: {}",
            clean_expected, computed_hex
        ))
    }
}

/// Checks online (GitHub releases API, then GitHub tags fallback) for the latest Corex version in sluisr/corex.
/// Sets a strict 3-second timeout so it never hangs or blocks the CLI/TUI.
pub async fn check_for_update_online(current_version: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent(format!("corex/{}", current_version))
        .build()
        .ok()?;

    let repo_endpoints = [
        "https://api.github.com/repos/sluisr/corex",
    ];

    for base in repo_endpoints {
        // 1. Try GitHub Releases API
        let github_url = format!("{}/releases/latest", base);
        if let Ok(resp) = client.get(&github_url).send().await {
            if resp.status().is_success() {
                if let Ok(val) = resp.json::<serde_json::Value>().await {
                    if let Some(tag) = val.get("tag_name").and_then(|t| t.as_str()) {
                        let clean_tag = tag.trim_start_matches('v').to_string();
                        if is_newer_version(current_version, &clean_tag) {
                            save_cache(&clean_tag);
                            return Some(clean_tag);
                        }
                    }
                }
            }
        }

        // 2. Try GitHub Tags API fallback
        let tags_url = format!("{}/tags", base);
        if let Ok(resp) = client.get(&tags_url).send().await {
            if resp.status().is_success() {
                if let Ok(val) = resp.json::<serde_json::Value>().await {
                    if let Some(tags_array) = val.as_array() {
                        for item in tags_array {
                            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                                let clean_tag = name.trim_start_matches('v').to_string();
                                if is_newer_version(current_version, &clean_tag) {
                                    save_cache(&clean_tag);
                                    return Some(clean_tag);
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// Fetches full metadata of latest GitHub release.
pub async fn fetch_latest_release_info(current_version: &str) -> Result<ReleaseInfo, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(format!("corex/{}", current_version))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {}", e))?;

    let url = "https://api.github.com/repos/sluisr/corex/releases/latest";
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Failed to reach GitHub Releases API: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API responded with status {}", resp.status()));
    }

    let release = resp
        .json::<ReleaseInfo>()
        .await
        .map_err(|e| format!("Failed to parse release information: {}", e))?;

    Ok(release)
}

/// Extracts the target binary bytes from a tar.gz or zip archive in memory.
pub fn extract_binary_from_archive(archive_bytes: &[u8], is_zip: bool, bin_name: &str) -> Result<Vec<u8>, String> {
    if is_zip {
        let cursor = std::io::Cursor::new(archive_bytes);
        let mut zip = zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to read zip archive: {}", e))?;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| format!("Zip entry error: {}", e))?;
            let name = file.name().to_string();
            let file_base = Path::new(&name).file_name().and_then(|f| f.to_str()).unwrap_or("");
            if file_base == bin_name || file_base == format!("{}.exe", bin_name) {
                let mut buf = Vec::new();
                file.read_to_end(&mut buf).map_err(|e| format!("Failed to extract file: {}", e))?;
                return Ok(buf);
            }
        }
        Err(format!("Binary '{}' not found in zip archive", bin_name))
    } else {
        let cursor = std::io::Cursor::new(archive_bytes);
        let gz = flate2::read::GzDecoder::new(cursor);
        let mut tar = tar::Archive::new(gz);
        let entries = tar.entries().map_err(|e| format!("Failed to read tar archive: {}", e))?;
        for entry in entries {
            let mut file = entry.map_err(|e| format!("Tar entry error: {}", e))?;
            let path = file.path().map_err(|e| format!("Tar path error: {}", e))?;
            let file_base = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            if file_base == bin_name || file_base == format!("{}.exe", bin_name) {
                let mut buf = Vec::new();
                file.read_to_end(&mut buf).map_err(|e| format!("Failed to extract file: {}", e))?;
                return Ok(buf);
            }
        }
        Err(format!("Binary '{}' not found in tar.gz archive", bin_name))
    }
}

/// Atomically replaces the current executable binary on disk.
/// On Unix, uses `fs::rename` within the same directory to avoid ETXTBSY.
/// On Windows, renames the existing file to .old before moving the new one.
pub fn replace_executable_in_place(target_path: &Path, new_binary_bytes: &[u8]) -> Result<(), String> {
    let canonical = fs::canonicalize(target_path).unwrap_or_else(|_| target_path.to_path_buf());
    let parent = canonical
        .parent()
        .ok_or_else(|| "Unable to locate binary parent directory".to_string())?;

    let tmp_path = parent.join(format!(".cx-upgrade-{}.tmp", std::process::id()));

    // Write new binary to temp file next to target binary
    fs::write(&tmp_path, new_binary_bytes).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!(
                "Permission denied writing to '{}'. Run with elevated permissions (e.g. 'sudo cx update').",
                parent.display()
            )
        } else {
            format!("Failed to write temporary binary: {}", e)
        }
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755));
    }

    #[cfg(unix)]
    {
        if let Err(e) = fs::rename(&tmp_path, &canonical) {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("Failed to replace binary in-place: {}", e));
        }
    }

    #[cfg(windows)]
    {
        let old_path = parent.join(format!(".cx-old-{}.bak", std::process::id()));
        if let Err(e) = fs::rename(&canonical, &old_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("Failed to move active binary to backup: {}", e));
        }
        if let Err(e) = fs::rename(&tmp_path, &canonical) {
            let _ = fs::rename(&old_path, &canonical);
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("Failed to install new binary: {}", e));
        }
        let _ = fs::remove_file(&old_path);
    }

    Ok(())
}

/// Performs a complete in-place self-update of Corex (`cx`):
/// 1. Determines current OS/Arch.
/// 2. Queries GitHub latest release.
/// 3. Downloads matching archive and published .sha256 signature.
/// 4. Cryptographically verifies SHA-256.
/// 5. Extracts binary and replaces current executable in-place.
pub async fn perform_self_update<F>(
    current_version: &str,
    force: bool,
    progress: F,
) -> Result<UpdateSummary, String>
where
    F: Fn(&str) + Send + Sync,
{
    let target_triple = current_target_triple().ok_or_else(|| {
        "Auto-update is not supported on this platform architecture. Please update via cargo install or build from source."
            .to_string()
    })?;

    progress("Checking for latest release on GitHub...");
    let release = fetch_latest_release_info(current_version).await?;
    let latest_clean = release.tag_name.trim_start_matches('v');

    if !force && !is_newer_version(current_version, latest_clean) {
        return Err(format!(
            "Corex is already on the latest version (v{}). Use --force to reinstall.",
            current_version
        ));
    }

    progress(&format!(
        "Found Corex v{} (current: v{}, target: {})",
        latest_clean, current_version, target_triple
    ));

    let is_windows = target_triple.contains("windows");
    let archive_ext = if is_windows { ".zip" } else { ".tar.gz" };

    // Locate the matching asset
    let matching_asset = release.assets.iter().find(|a| {
        a.name.contains(target_triple) && a.name.ends_with(archive_ext)
    });

    let download_url = if let Some(asset) = matching_asset {
        asset.browser_download_url.clone()
    } else {
        format!(
            "https://github.com/sluisr/corex/releases/download/{}/corex-{}-{}{}",
            release.tag_name, release.tag_name, target_triple, archive_ext
        )
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(format!("corex/{}", current_version))
        .build()
        .map_err(|e| format!("HTTP client init failed: {}", e))?;

    progress(&format!("Downloading binary archive from {}...", download_url));
    let resp = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("Failed to download release archive: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Download failed with HTTP status {} ({})",
            resp.status(),
            download_url
        ));
    }

    let archive_bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read archive payload: {}", e))?
        .to_vec();

    progress(&format!(
        "Downloaded {} KB. Verifying SHA-256 signature...",
        archive_bytes.len() / 1024
    ));

    // Fetch matching .sha256 file if available
    let sha_asset = release.assets.iter().find(|a| {
        a.name.contains(target_triple) && a.name.ends_with(&format!("{}.sha256", archive_ext))
    });

    let sha_url = sha_asset
        .map(|a| a.browser_download_url.clone())
        .unwrap_or_else(|| format!("{}.sha256", download_url));

    if let Ok(sha_resp) = client.get(&sha_url).send().await {
        if sha_resp.status().is_success() {
            if let Ok(expected_sha) = sha_resp.text().await {
                let computed = verify_sha256(&archive_bytes, &expected_sha)?;
                progress(&format!("✓ SHA-256 signature verified: {}", &computed[..16]));
            }
        }
    }

    progress("Extracting binary from release archive...");
    let bin_name = if is_windows { "cx.exe" } else { "cx" };
    let new_binary = extract_binary_from_archive(&archive_bytes, is_windows, bin_name)?;

    let current_exe = std::env::current_exe().map_err(|e| format!("Failed to get current executable path: {}", e))?;
    progress(&format!(
        "Replacing binary in-place at {}...",
        current_exe.display()
    ));

    replace_executable_in_place(&current_exe, &new_binary)?;

    // If a sibling "corex" binary exists in the same folder, update that as well
    if let Some(parent) = current_exe.parent() {
        let corex_name = if is_windows { "corex.exe" } else { "corex" };
        let corex_sibling = parent.join(corex_name);
        if corex_sibling.exists() && corex_sibling != current_exe {
            if let Ok(corex_binary) = extract_binary_from_archive(&archive_bytes, is_windows, corex_name) {
                let _ = replace_executable_in_place(&corex_sibling, &corex_binary);
            }
        }
    }

    save_cache(latest_clean);

    Ok(UpdateSummary {
        previous_version: current_version.to_string(),
        updated_version: latest_clean.to_string(),
        target_triple: target_triple.to_string(),
        binary_path: current_exe,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_semver() {
        assert_eq!(parse_semver("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_semver("v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_semver("1.0.0-rc1"), Some((1, 0, 0)));
    }

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("0.2.0", "0.2.1"));
        assert!(is_newer_version("0.2.0", "v0.2.1"));
        assert!(is_newer_version("0.2.0", "0.3.0"));
        assert!(is_newer_version("0.2.0", "1.0.0"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
        assert!(!is_newer_version("0.2.0", "0.1.9"));
        assert!(!is_newer_version("0.2.1", "0.2.0"));
    }

    #[test]
    fn test_current_target_triple() {
        let triple = current_target_triple();
        assert!(triple.is_some(), "Current platform should be detected");
    }

    #[test]
    fn test_verify_sha256() {
        let data = b"hello corex auto update";
        // sha256 of "hello corex auto update"
        let mut hasher = Sha256::new();
        hasher.update(data);
        let expected = format!("{:x}", hasher.finalize());

        assert!(verify_sha256(data, &expected).is_ok());
        assert!(verify_sha256(data, &format!("{}  corex.tar.gz", expected)).is_ok());
        assert!(verify_sha256(data, "0000000000000000000000000000000000000000000000000000000000000000").is_err());
    }

    #[test]
    fn test_extract_tar_gz_in_memory() {
        use std::io::Write;

        // Build a small tar.gz in memory
        let mut tar_builder = tar::Builder::new(Vec::new());
        let payload = b"#!/bin/sh\necho cx";
        let mut header = tar::Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar_builder.append_data(&mut header, "cx", &payload[..]).unwrap();
        let tar_bytes = tar_builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let extracted = extract_binary_from_archive(&gz_bytes, false, "cx").unwrap();
        assert_eq!(extracted, payload);
    }
}
