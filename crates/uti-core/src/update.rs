use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use chrono::Utc;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCache {
    pub last_checked_sec: i64,
    pub latest_version: String,
}

fn get_cache_path() -> Option<PathBuf> {
    BaseDirs::new().map(|d| d.home_dir().join(".corex").join("cache").join("update_check.json"))
}

fn get_legacy_cache_path() -> Option<PathBuf> {
    BaseDirs::new().map(|d| d.home_dir().join(".uti").join("cache").join("update_check.json"))
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
    let path = get_cache_path().and_then(|p| if p.exists() { Some(p) } else { None })
        .or_else(|| get_legacy_cache_path().and_then(|p| if p.exists() { Some(p) } else { None }))?;

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

/// Checks online (GitHub releases API, then GitHub tags fallback) for the latest Corex version in sluisr/corex.
/// Sets a strict 3-second timeout so it never hangs or blocks the CLI/TUI.
pub async fn check_for_update_online(current_version: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent(format!("corex/{}", current_version))
        .build()
        .ok()?;

    // Try primary sluisr/corex first, then fallback to sluisr/uti-cli
    let repo_endpoints = [
        "https://api.github.com/repos/sluisr/corex",
        "https://api.github.com/repos/sluisr/uti-cli",
    ];

    for base in repo_endpoints {
        // 1. Try GitHub Releases API
        let github_url = format!("{}/releases/latest", base);
        if let Ok(resp) = client.get(&github_url).send().await {
            if resp.status().is_success() {
                if let Ok(val) = resp.json::<serde_json::Value>().await {
                    if let Some(tag) = val.get("tag_name").and_then(|t| t.as_str()) {
                        let clean_tag = tag.trim_start_matches('v').to_string();
                        save_cache(&clean_tag);
                        if is_newer_version(current_version, &clean_tag) {
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
                                save_cache(&clean_tag);
                                if is_newer_version(current_version, &clean_tag) {
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
}
