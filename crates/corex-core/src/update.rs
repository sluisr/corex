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
    // Builds for Termux target Android, where `target_os` is "android", not "linux".
    #[cfg(all(target_os = "android", target_arch = "aarch64"))]
    {
        Some("aarch64-linux-android")
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
        all(target_os = "android", target_arch = "aarch64"),
        all(target_os = "macos", any(target_arch = "x86_64", target_arch = "aarch64")),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        None
    }
}

/// Parses semver string like "v0.2.1" or "0.2.0-rc1" into (major, minor, patch).
pub fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let clean = v.trim().trim_start_matches('v');
    let mut parts = clean.split('.');
    let parse_seg = |p: &str| -> Option<u32> {
        let num_part = p.split('-').next().unwrap_or(p);
        let digits = num_part.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("");
        digits.parse::<u32>().ok()
    };
    let major: u32 = parse_seg(parts.next()?)?;
    let minor: u32 = parse_seg(parts.next()?)?;
    let patch: u32 = parse_seg(parts.next().unwrap_or("0"))?;
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

    if clean_expected.len() != 64 || !clean_expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Missing or malformed SHA-256 checksum signature.".to_string());
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

/// Verifies the SHA-256 checksum of `data` against `checksum_text` for a specific `asset_name`.
/// Supports single-hash files, `<hash>  <filename>` lines, and multi-line checksum manifests.
/// Fails if `checksum_text` is a multi-line manifest (or names a different asset) without a matching entry for `asset_name`.
pub fn verify_checksum_from_text(
    data: &[u8],
    checksum_text: &str,
    asset_name: &str,
) -> Result<String, String> {
    let non_empty_lines: Vec<&str> = checksum_text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    if non_empty_lines.is_empty() {
        return Err("Missing or empty SHA-256 checksum text.".to_string());
    }

    // Look for a line explicitly matching `asset_name`
    for line in &non_empty_lines {
        let mut parts = line.split_whitespace();
        if let (Some(hash), Some(file_tok)) = (parts.next(), parts.next()) {
            let clean_file = file_tok.trim_start_matches('*');
            let base_file = Path::new(clean_file)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(clean_file);
            if base_file == asset_name || clean_file == asset_name {
                return verify_sha256(data, hash);
            }
        }
    }

    // Only allow a bare single-line hash when there is a single line with no conflicting filename
    if non_empty_lines.len() == 1 {
        let mut parts = non_empty_lines[0].split_whitespace();
        if let Some(hash) = parts.next() {
            if parts.next().is_none() {
                return verify_sha256(data, hash);
            }
        }
    }

    Err(format!(
        "No matching SHA-256 checksum entry found for asset '{}' in checksum manifest.",
        asset_name
    ))
}

/// Checks online for a newer release (CDN -> GitHub API -> GitHub web redirect) with a 3s timeout.
pub async fn check_for_update_online(current_version: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent(format!("corex/{}", current_version))
        .build()
        .ok()?;

    if let Ok(resp) = client.get("https://corex.sluisr.com/release.json").send().await {
        if resp.status().is_success() {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                if let Some(tag) = val.get("tag").or_else(|| val.get("version")).and_then(|t| t.as_str()) {
                    let clean_tag = tag.trim_start_matches('v').to_string();
                    if is_newer_version(current_version, &clean_tag) {
                        save_cache(&clean_tag);
                        return Some(clean_tag);
                    }
                }
            }
        }
    }

    let mut req = client.get("https://api.github.com/repos/sluisr/corex/releases/latest");
    if let Ok(token) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
        let token = token.trim();
        if !token.is_empty() {
            req = req.bearer_auth(token);
        }
    }
    if let Ok(resp) = req.send().await {
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

    let no_redir_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent(format!("corex/{}", current_version))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    if let Ok(resp) = no_redir_client.get("https://github.com/sluisr/corex/releases/latest").send().await {
        if resp.status().is_redirection() {
            if let Some(loc) = resp.headers().get(reqwest::header::LOCATION) {
                if let Ok(loc_str) = loc.to_str() {
                    if let Some(tag) = loc_str.split("/tag/").nth(1) {
                        let clean_tag = tag.trim_matches('/').trim_start_matches('v').to_string();
                        if is_newer_version(current_version, &clean_tag) {
                            save_cache(&clean_tag);
                            return Some(clean_tag);
                        }
                    }
                }
            }
        }
    }

    None
}

#[derive(Debug, Deserialize)]
struct CdnReleasePayload {
    tag: Option<String>,
    version: Option<String>,
    #[serde(default)]
    binaries: Vec<CdnBinaryEntry>,
}

#[derive(Debug, Deserialize)]
struct CdnBinaryEntry {
    archive: String,
    url: String,
}

/// Fetches latest release metadata (GitHub REST API -> CDN -> GitHub web redirect).
pub async fn fetch_latest_release_info(current_version: &str) -> Result<ReleaseInfo, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(format!("corex/{}", current_version))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {}", e))?;

    let mut api_req = client.get("https://api.github.com/repos/sluisr/corex/releases/latest");
    if let Ok(token) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
        let token = token.trim();
        if !token.is_empty() {
            api_req = api_req.bearer_auth(token);
        }
    }

    let mut last_api_err = None;
    match api_req.send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                if let Ok(release) = resp.json::<ReleaseInfo>().await {
                    return Ok(release);
                }
            } else {
                last_api_err = Some(format!("GitHub API responded with status {}", resp.status()));
            }
        }
        Err(e) => {
            last_api_err = Some(format!("Failed to reach GitHub Releases API: {}", e));
        }
    }

    // Neither fallback is trusted alone. The CDN file is refreshed by hand after a release, so it can
    // still name the previous version (the API being rate limited then made `cx update` report
    // "already on the latest version"), and the web redirect names the newest tag but lists no
    // assets. The newest of the two wins; the download URL is rebuilt from the tag when needed.
    let cdn = fetch_release_from_cdn(&client).await;
    let redirect = fetch_release_from_web_redirect(current_version).await;

    newest_release(cdn, redirect).ok_or_else(|| {
        last_api_err.unwrap_or_else(|| {
            "Could not retrieve latest release information across GitHub API, CDN, and web mirrors. Check internet connectivity.".to_string()
        })
    })
}

/// The release described by the CDN's `release.json`, with its asset list.
async fn fetch_release_from_cdn(client: &reqwest::Client) -> Option<ReleaseInfo> {
    let resp = client.get("https://corex.sluisr.com/release.json").send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let payload = resp.json::<CdnReleasePayload>().await.ok()?;
    let tag_name = payload.tag.or(payload.version)?;
    let assets = payload
        .binaries
        .into_iter()
        .map(|b| ReleaseAsset {
            name: b.archive,
            browser_download_url: b.url,
            size: None,
        })
        .collect();
    Some(ReleaseInfo {
        tag_name,
        html_url: Some("https://corex.sluisr.com".to_string()),
        body: None,
        assets,
    })
}

/// The newest release tag, read from where GitHub's `releases/latest` page redirects. It uses no
/// API quota, and carries no asset list.
async fn fetch_release_from_web_redirect(current_version: &str) -> Option<ReleaseInfo> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(format!("corex/{}", current_version))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let resp = client.get("https://github.com/sluisr/corex/releases/latest").send().await.ok()?;
    if !resp.status().is_redirection() {
        return None;
    }
    let location = resp.headers().get(reqwest::header::LOCATION)?.to_str().ok()?.to_string();
    let tag_name = location.split("/tag/").nth(1)?.trim_matches('/').to_string();
    Some(ReleaseInfo {
        tag_name,
        html_url: Some(location),
        body: None,
        assets: vec![],
    })
}

/// The newer of two releases. On a tie the first is kept, since it is the one with the asset list.
fn newest_release(first: Option<ReleaseInfo>, second: Option<ReleaseInfo>) -> Option<ReleaseInfo> {
    match (first, second) {
        (Some(a), Some(b)) => Some(if is_newer_version(&a.tag_name, &b.tag_name) { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Maximum extracted binary size (200 MB) to guard against decompression bombs.
const MAX_EXTRACTED_BINARY_BYTES: u64 = 200 * 1024 * 1024;

/// Extracts the target binary bytes from a tar.gz or zip archive in memory.
pub fn extract_binary_from_archive(archive_bytes: &[u8], is_zip: bool, bin_name: &str) -> Result<Vec<u8>, String> {
    if is_zip {
        let cursor = std::io::Cursor::new(archive_bytes);
        let mut zip = zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to read zip archive: {}", e))?;
        for i in 0..zip.len() {
            let file = zip.by_index(i).map_err(|e| format!("Zip entry error: {}", e))?;
            // An entry whose name cannot be decoded cannot be the binary we are looking for.
            let name = match file.name() {
                Ok(name) => name.into_owned(),
                Err(_) => continue,
            };
            let file_base = Path::new(&name).file_name().and_then(|f| f.to_str()).unwrap_or("");
            if file_base == bin_name || file_base == format!("{}.exe", bin_name) {
                if file.size() > MAX_EXTRACTED_BINARY_BYTES {
                    return Err("Archive entry exceeds maximum allowed binary size (200 MB)".to_string());
                }
                let mut buf = Vec::new();
                file.take(MAX_EXTRACTED_BINARY_BYTES + 1)
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("Failed to extract file: {}", e))?;
                if buf.len() as u64 > MAX_EXTRACTED_BINARY_BYTES {
                    return Err("Extracted binary exceeds maximum allowed size (200 MB)".to_string());
                }
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
            let file = entry.map_err(|e| format!("Tar entry error: {}", e))?;
            let path = file.path().map_err(|e| format!("Tar path error: {}", e))?;
            let file_base = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            if file_base == bin_name || file_base == format!("{}.exe", bin_name) {
                let mut buf = Vec::new();
                file.take(MAX_EXTRACTED_BINARY_BYTES + 1)
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("Failed to extract file: {}", e))?;
                if buf.len() as u64 > MAX_EXTRACTED_BINARY_BYTES {
                    return Err("Extracted binary exceeds maximum allowed size (200 MB)".to_string());
                }
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
    use std::io::Write;
    let canonical = fs::canonicalize(target_path).unwrap_or_else(|_| target_path.to_path_buf());
    let parent = canonical
        .parent()
        .ok_or_else(|| "Unable to locate binary parent directory".to_string())?;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_path = parent.join(format!(".cx-upgrade-{}-{:x}.tmp", std::process::id(), nonce));

    // Exclusively create new temp file next to target binary (prevents symlink attacks)
    let mut open_opts = fs::OpenOptions::new();
    open_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(0o755);
    }

    let mut tmp_file = open_opts.open(&tmp_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!(
                "Permission denied writing to '{}'. Run with elevated permissions (e.g. 'sudo cx update').",
                parent.display()
            )
        } else {
            format!("Failed to create temporary binary: {}", e)
        }
    })?;

    if let Err(e) = tmp_file.write_all(new_binary_bytes).and_then(|_| tmp_file.sync_all()) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("Failed to write temporary binary: {}", e));
    }
    drop(tmp_file);

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

    let asset_name = matching_asset
        .map(|a| a.name.clone())
        .unwrap_or_else(|| format!("corex-{}-{}{}", release.tag_name, target_triple, archive_ext));

    let download_url = if let Some(asset) = matching_asset {
        asset.browser_download_url.clone()
    } else {
        format!(
            "https://github.com/sluisr/corex/releases/download/{}/{}",
            release.tag_name, asset_name
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

    // Fetch matching .sha256 file (required for cryptographic integrity verification)
    let sha_asset = release.assets.iter().find(|a| {
        a.name.contains(target_triple) && a.name.ends_with(&format!("{}.sha256", archive_ext))
    });

    let sha_url = sha_asset
        .map(|a| a.browser_download_url.clone())
        .unwrap_or_else(|| format!("{}.sha256", download_url));

    let sha_resp = client
        .get(&sha_url)
        .send()
        .await
        .map_err(|e| format!("Failed to download SHA-256 checksum ({}): {}", sha_url, e))?;

    if !sha_resp.status().is_success() {
        return Err(format!(
            "Refusing to install unverified binary: SHA-256 checksum download returned HTTP {} ({})",
            sha_resp.status(),
            sha_url
        ));
    }

    let expected_sha = sha_resp
        .text()
        .await
        .map_err(|e| format!("Failed to read SHA-256 checksum response: {}", e))?;
    let computed = verify_checksum_from_text(&archive_bytes, &expected_sha, &asset_name)?;
    progress(&format!("SHA-256 signature verified: {}", &computed[..16]));

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
        assert_eq!(parse_semver("0.6.4-rc1"), Some((0, 6, 4)));
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

        let multi = format!(
            "1111111111111111111111111111111111111111111111111111111111111111  other.tar.gz\n{}  corex.tar.gz\n",
            expected
        );
        assert!(verify_checksum_from_text(data, &multi, "corex.tar.gz").is_ok());
        assert!(verify_checksum_from_text(data, &multi, "missing.tar.gz").is_err());
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

    #[tokio::test]
    async fn test_fetch_latest_release_info_fallback() {
        let release = fetch_latest_release_info("0.6.3").await;
        assert!(release.is_ok(), "fetch_latest_release_info should succeed via CDN/fallback: {:?}", release.err());
        let rel = release.unwrap();
        assert!(rel.tag_name.starts_with('v'));
    }

    #[test]
    fn test_extract_zip_in_memory() {
        use std::io::Write;

        let payload = b"MZ fake windows binary".to_vec();
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("docs/readme.txt", options).unwrap();
            writer.write_all(b"hello").unwrap();
            writer.start_file("cx.exe", options).unwrap();
            writer.write_all(&payload).unwrap();
            writer.finish().unwrap();
        }
        let bytes = cursor.into_inner();

        // The Windows release names the binary `cx.exe`; it is found by its base name.
        assert_eq!(extract_binary_from_archive(&bytes, true, "cx").unwrap(), payload);
        assert!(extract_binary_from_archive(&bytes, true, "missing").unwrap_err().contains("not found"));
    }

    /// Runs against a real release archive when `COREX_TEST_ZIP` names one, for example
    /// `COREX_TEST_ZIP=corex-v0.8.1-x86_64-pc-windows-msvc.zip cargo test -p corex-core real_release_zip`.
    #[test]
    fn real_release_zip_yields_the_windows_binary() {
        let Some(path) = std::env::var_os("COREX_TEST_ZIP") else {
            return;
        };
        let bytes = std::fs::read(path).expect("readable archive");
        let binary = extract_binary_from_archive(&bytes, true, "cx").expect("cx.exe in the archive");
        assert!(binary.starts_with(b"MZ"), "a Windows executable starts with the MZ header");
    }

    fn release(tag: &str, assets: usize) -> ReleaseInfo {
        ReleaseInfo {
            tag_name: tag.to_string(),
            html_url: None,
            body: None,
            assets: (0..assets)
                .map(|i| ReleaseAsset { name: format!("asset-{}", i), browser_download_url: String::new(), size: None })
                .collect(),
        }
    }

    #[test]
    fn a_stale_cdn_does_not_hide_the_newest_release() {
        // The CDN still names v0.7.0 while GitHub's redirect already names v0.8.1.
        let newest = newest_release(Some(release("v0.7.0", 6)), Some(release("v0.8.1", 0))).unwrap();
        assert_eq!(newest.tag_name, "v0.8.1");
        // The same version on both keeps the CDN entry, which is the one with the asset list.
        let tie = newest_release(Some(release("v0.8.1", 6)), Some(release("v0.8.1", 0))).unwrap();
        assert_eq!(tie.assets.len(), 6);
        // A newer CDN beats an older redirect, and either source alone is used as it is.
        assert_eq!(newest_release(Some(release("v0.9.0", 6)), Some(release("v0.8.1", 0))).unwrap().tag_name, "v0.9.0");
        assert_eq!(newest_release(None, Some(release("v0.8.1", 0))).unwrap().tag_name, "v0.8.1");
        assert!(newest_release(None, None).is_none());
    }
}

