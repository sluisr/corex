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

    pub fn legacy_sessions_dir() -> Option<PathBuf> {
        BaseDirs::new().map(|dirs| dirs.home_dir().join(".uti").join("sessions"))
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
        let dir = Self::sessions_dir();
        fs::create_dir_all(&dir)?;
        let file = dir.join(format!("{}.json", self.id));
        let serialized = serde_json::to_string_pretty(self)?;
        fs::write(file, serialized)?;
        Ok(())
    }

    pub fn save_checkpoint(&mut self, tag: &str) -> Result<()> {
        self.tag = Some(tag.trim().to_string());
        self.save()
    }

    pub fn load_by_id_or_tag(id_or_tag: &str) -> Result<Self> {
        let dir = Self::sessions_dir();
        let target = id_or_tag.trim();

        // 1. Direct file check with id.json
        let direct_file = dir.join(format!("{}.json", target));
        if direct_file.exists() {
            let content = fs::read_to_string(&direct_file)?;
            let session: Session = serde_json::from_str(&content)?;
            return Ok(session);
        }
        if let Some(legacy_dir) = Self::legacy_sessions_dir() {
            let legacy_file = legacy_dir.join(format!("{}.json", target));
            if legacy_file.exists() {
                let content = fs::read_to_string(&legacy_file)?;
                let session: Session = serde_json::from_str(&content)?;
                return Ok(session);
            }
        }

        // 2. Search by UUID prefix, tag, or 1-based index
        let all_summaries = Self::list_all(None);
        if let Ok(idx) = target.parse::<usize>() {
            if idx > 0 && idx <= all_summaries.len() {
                let target_id = &all_summaries[idx - 1].id;
                let file = dir.join(format!("{}.json", target_id));
                if file.exists() {
                    let content = fs::read_to_string(&file)?;
                    let session: Session = serde_json::from_str(&content)?;
                    return Ok(session);
                } else if let Some(legacy_dir) = Self::legacy_sessions_dir() {
                    let legacy_file = legacy_dir.join(format!("{}.json", target_id));
                    if legacy_file.exists() {
                        let content = fs::read_to_string(&legacy_file)?;
                        let session: Session = serde_json::from_str(&content)?;
                        return Ok(session);
                    }
                }
            }
        }

        // Search matching tag or prefix
        for summary in all_summaries {
            if summary.id.starts_with(target)
                || summary.tag.as_deref().map(|t| t.eq_ignore_ascii_case(target)).unwrap_or(false)
            {
                let file = dir.join(format!("{}.json", summary.id));
                if file.exists() {
                    let content = fs::read_to_string(&file)?;
                    let session: Session = serde_json::from_str(&content)?;
                    return Ok(session);
                } else if let Some(legacy_dir) = Self::legacy_sessions_dir() {
                    let legacy_file = legacy_dir.join(format!("{}.json", summary.id));
                    if legacy_file.exists() {
                        let content = fs::read_to_string(&legacy_file)?;
                        let session: Session = serde_json::from_str(&content)?;
                        return Ok(session);
                    }
                }
            }
        }

        bail!("No session found matching '{}'. Use `/chat list` or `cx --list-sessions` to view available sessions.", target);
    }

    pub fn delete_by_id_or_tag(id_or_tag: &str) -> Result<String> {
        let session = Self::load_by_id_or_tag(id_or_tag)?;
        let file = Self::sessions_dir().join(format!("{}.json", session.id));
        if file.exists() {
            fs::remove_file(&file)?;
        }
        if let Some(legacy_dir) = Self::legacy_sessions_dir() {
            let legacy_file = legacy_dir.join(format!("{}.json", session.id));
            if legacy_file.exists() {
                fs::remove_file(&legacy_file)?;
            }
        }
        Ok(session.id)
    }

    pub fn list_all(workspace_filter: Option<&str>) -> Vec<SessionSummary> {
        let mut dirs = vec![Self::sessions_dir()];
        if let Some(legacy) = Self::legacy_sessions_dir() {
            if legacy.exists() {
                dirs.push(legacy);
            }
        }

        let mut seen_ids = std::collections::HashSet::new();
        let mut list = Vec::new();
        for dir in dirs {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("json") {
                        if let Ok(content) = fs::read_to_string(&path) {
                            if let Ok(sess) = serde_json::from_str::<Session>(&content) {
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
        }
        list.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
        list
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
