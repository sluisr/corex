use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{Message, Usage};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default = "default_session_model")]
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub workspace_dir: Option<String>,
    pub messages: Vec<Message>,
    pub total_usage: Usage,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_session_model() -> String {
    "deepseek-flash".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub tag: Option<String>,
    pub model: String,
    pub message_count: usize,
    pub updated_at: DateTime<Utc>,
    pub workspace_dir: Option<String>,
}

#[derive(Deserialize)]
struct SessionSummaryDeserializer {
    id: String,
    title: String,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default = "default_session_model")]
    model: String,
    #[serde(default)]
    messages: Vec<serde::de::IgnoredAny>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    workspace_dir: Option<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self::new_with_params("deepseek-flash", 1.0, "high", None)
    }

    pub fn new_with_params(
        model: &str,
        temperature: f32,
        reasoning_effort: &str,
        workspace: Option<&Path>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: "New Session".to_string(),
            tag: None,
            model: model.to_string(),
            temperature: Some(temperature),
            reasoning_effort: Some(reasoning_effort.to_string()),
            workspace_dir: workspace.map(|p| p.display().to_string()),
            messages: Vec::new(),
            total_usage: Usage::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    pub fn sessions_dir() -> PathBuf {
        BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".corex").join("sessions"))
            .unwrap_or_else(|| PathBuf::from(".corex/sessions"))
    }

    pub fn fleet_sessions_dir() -> PathBuf {
        BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".corex").join("fleet_sessions"))
            .unwrap_or_else(|| PathBuf::from(".corex/fleet_sessions"))
    }

    pub fn is_fleet(&self) -> bool {
        self.tag.as_deref() == Some("fleet") || self.title == "SysAdmin Fleet Console"
    }

    pub fn target_dir(&self) -> PathBuf {
        if self.is_fleet() {
            Self::fleet_sessions_dir()
        } else {
            Self::sessions_dir()
        }
    }

    pub fn find_session_file(id: &str) -> Option<PathBuf> {
        let f1 = Self::sessions_dir().join(format!("{}.json", id));
        if f1.exists() {
            return Some(f1);
        }
        let f2 = Self::fleet_sessions_dir().join(format!("{}.json", id));
        if f2.exists() {
            return Some(f2);
        }
        None
    }

    pub fn add_message(&mut self, message: Message) {
        if self.messages.is_empty() && message.role == "user" {
            if let Some(text) = message.text_content() {
                let trimmed = text.trim();
                let first_line = trimmed.lines().next().unwrap_or("Session");
                self.title = first_line.chars().take(50).collect();
            }
        }
        self.messages.push(message);
        self.updated_at = Utc::now();
    }

    pub fn update_usage(&mut self, usage: &Usage) {
        self.total_usage.prompt_tokens += usage.prompt_tokens;
        self.total_usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
        self.total_usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
        self.total_usage.completion_tokens += usage.completion_tokens;
        self.total_usage.total_tokens += usage.total_tokens;
    }

    pub fn save(&self) -> Result<()> {
        let dir = self.target_dir();
        crate::secure_fs::ensure_private_dir(&dir)?;
        let safe_id: String = self
            .id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if safe_id.is_empty() {
            bail!("Invalid session identifier.");
        }
        let file = dir.join(format!("{}.json", safe_id));
        let serialized = serde_json::to_string_pretty(self)?;
        crate::secure_fs::write_private_atomic(&file, serialized.as_bytes())?;
        Ok(())
    }

    pub fn save_checkpoint(&mut self, tag: &str) -> Result<()> {
        self.tag = Some(tag.trim().to_string());
        self.save()
    }

    fn is_safe_session_id(s: &str) -> bool {
        !s.is_empty()
            && !s.contains("..")
            && !s.contains('/')
            && !s.contains('\\')
            && !s.contains('\0')
    }

    pub fn load_by_id_or_tag(id_or_tag: &str) -> Result<Self> {
        let target = id_or_tag.trim();

        if target.is_empty() {
            bail!("Session identifier cannot be empty.");
        }

        // 1. Direct file check with id.json
        if Self::is_safe_session_id(target) {
            if let Some(direct_file) = Self::find_session_file(target) {
                let content = fs::read_to_string(&direct_file)?;
                let session: Session = serde_json::from_str(&content)?;
                return Ok(session);
            }
        }

        let mut all_summaries = Self::list_from_dir(&Self::sessions_dir(), None);
        for s in Self::list_from_dir(&Self::fleet_sessions_dir(), None) {
            if !all_summaries.iter().any(|item| item.id == s.id) {
                all_summaries.push(s);
            }
        }

        // 2. Exact 1-based index
        if let Ok(idx) = target.parse::<usize>() {
            if idx > 0 && idx <= all_summaries.len() {
                let target_id = &all_summaries[idx - 1].id;
                if Self::is_safe_session_id(target_id) {
                    if let Some(file) = Self::find_session_file(target_id) {
                        let content = fs::read_to_string(&file)?;
                        let session: Session = serde_json::from_str(&content)?;
                        return Ok(session);
                    }
                }
            }
        }

        // 3. Search matching tag or prefix
        let mut matches = Vec::new();
        for summary in &all_summaries {
            let is_exact_tag = summary
                .tag
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case(target))
                .unwrap_or(false);
            let is_prefix = summary.id.starts_with(target);
            if is_exact_tag || is_prefix {
                matches.push((summary.clone(), is_exact_tag));
            }
        }

        if matches.is_empty() {
            bail!(
                "No session found matching '{}'. Use `/chat list` or `cx --list-sessions` to view available sessions.",
                target
            );
        }

        // If there is an exact tag match, prioritize it if it's unique
        let exact_tag_matches: Vec<_> = matches.iter().filter(|(_, exact)| *exact).collect();
        if exact_tag_matches.len() == 1 {
            let id = &exact_tag_matches[0].0.id;
            if Self::is_safe_session_id(id) {
                if let Some(file) = Self::find_session_file(id) {
                    let content = fs::read_to_string(&file)?;
                    let session: Session = serde_json::from_str(&content)?;
                    return Ok(session);
                }
            }
        }

        // Single unique match (either tag or prefix)
        if matches.len() == 1 {
            let id = &matches[0].0.id;
            if Self::is_safe_session_id(id) {
                if let Some(file) = Self::find_session_file(id) {
                    let content = fs::read_to_string(&file)?;
                    let session: Session = serde_json::from_str(&content)?;
                    return Ok(session);
                }
            }
        }

        // If multiple sessions matched, fail safely instead of guessing or deleting wrong session
        let samples: Vec<String> = matches
            .iter()
            .take(3)
            .map(|(s, _)| {
                format!(
                    "{} (tag: {})",
                    crate::types::safe_truncate_str(&s.id, 8),
                    s.tag.as_deref().unwrap_or("none")
                )
            })
            .collect();
        bail!(
            "Ambiguous session identifier '{}' matches {} sessions (e.g. {}). Please specify more characters or the full ID.",
            target,
            matches.len(),
            samples.join(", ")
        );
    }

    pub fn delete_by_id_or_tag(id_or_tag: &str) -> Result<String> {
        let session = Self::load_by_id_or_tag(id_or_tag)?;
        if !Self::is_safe_session_id(&session.id) {
            bail!("Refusing to delete session with unsafe ID '{}'.", session.id);
        }
        let f1 = Self::sessions_dir().join(format!("{}.json", session.id));
        if f1.exists() {
            let _ = fs::remove_file(&f1);
        }
        let f2 = Self::fleet_sessions_dir().join(format!("{}.json", session.id));
        if f2.exists() {
            let _ = fs::remove_file(&f2);
        }
        Ok(session.id)
    }

    pub fn list_from_dir(dir: &Path, workspace_filter: Option<&str>) -> Vec<SessionSummary> {
        let mut list = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(sess) = serde_json::from_str::<SessionSummaryDeserializer>(&content) {
                            if !seen_ids.insert(sess.id.clone()) {
                                continue;
                            }
                            if let Some(ws) = workspace_filter {
                                if let Some(ref sess_ws) = sess.workspace_dir {
                                    if sess_ws != ws {
                                        continue;
                                    }
                                }
                            }
                            list.push(SessionSummary {
                                id: sess.id,
                                title: sess.title,
                                tag: sess.tag,
                                model: sess.model,
                                message_count: sess.messages.len(),
                                updated_at: sess.updated_at,
                                workspace_dir: sess.workspace_dir,
                            });
                        }
                    }
                }
            }
        }
        list
    }

    pub fn list_normal(workspace_filter: Option<&str>) -> Vec<SessionSummary> {
        let mut list: Vec<SessionSummary> = Self::list_from_dir(&Self::sessions_dir(), workspace_filter)
            .into_iter()
            .filter(|s| s.tag.as_deref() != Some("fleet") && s.title != "SysAdmin Fleet Console")
            .collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
        list
    }

    pub fn list_fleet(workspace_filter: Option<&str>) -> Vec<SessionSummary> {
        let mut list = Self::list_from_dir(&Self::fleet_sessions_dir(), workspace_filter);
        for s in Self::list_from_dir(&Self::sessions_dir(), workspace_filter) {
            if (s.tag.as_deref() == Some("fleet") || s.title == "SysAdmin Fleet Console")
                && !list.iter().any(|item| item.id == s.id)
            {
                list.push(s);
            }
        }
        list.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
        list
    }

    pub fn list_all(workspace_filter: Option<&str>) -> Vec<SessionSummary> {
        Self::list_normal(workspace_filter)
    }

    pub fn format_relative_time(dt: DateTime<Utc>) -> String {
        let now = Utc::now();
        let duration = now.signed_duration_since(dt);
        let secs = duration.num_seconds();

        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            let mins = secs / 60;
            format!("{}m ago", mins)
        } else if secs < 86400 {
            let hours = secs / 3600;
            format!("{}h ago", hours)
        } else {
            let days = secs / 86400;
            format!("{}d ago", days)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_relative_time_renders_correctly() {
        let now = Utc::now();
        assert_eq!(Session::format_relative_time(now), "just now");
        assert_eq!(Session::format_relative_time(now - chrono::Duration::seconds(120)), "2m ago");
        assert_eq!(Session::format_relative_time(now - chrono::Duration::hours(5)), "5h ago");
        assert_eq!(Session::format_relative_time(now - chrono::Duration::days(3)), "3d ago");
    }

    #[test]
    fn load_by_empty_id_or_tag_bails() {
        let res = Session::load_by_id_or_tag("   ");
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_fleet_session_routing_and_isolation() {
        let mut normal_sess = Session::new();
        normal_sess.title = "Coding Rust kernel patch".to_string();
        assert!(!normal_sess.is_fleet());
        assert_eq!(normal_sess.target_dir(), Session::sessions_dir());

        let mut fleet_sess = Session::new();
        fleet_sess.tag = Some("fleet".to_string());
        fleet_sess.title = "SysAdmin Fleet Console".to_string();
        assert!(fleet_sess.is_fleet());
        assert_eq!(fleet_sess.target_dir(), Session::fleet_sessions_dir());
    }
}

