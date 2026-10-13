//! Autonomous multi-agent coordination and parallel delegation engine.
//!
//! Provides isolated actor execution for specialized subagents (Scout, Tester,
//! Researcher, Worker) with dedicated context boundaries, non-blocking background
//! scheduling, and reactive completion notifications back to the lead agent.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentRole {
    /// Read-only repo exploration & symbol finding at zero cost before cloud editing.
    Scout,
    /// Automated test runner, compiler checker, and linter verifier in background sandbox.
    Tester,
    /// Documentation researcher, specification reader, and external reference extractor.
    Researcher,
    /// General-purpose autonomous parallel subtask execution.
    Worker,
}

impl SubagentRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubagentRole::Scout => "scout",
            SubagentRole::Tester => "tester",
            SubagentRole::Researcher => "researcher",
            SubagentRole::Worker => "worker",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "scout" => SubagentRole::Scout,
            "tester" | "test" => SubagentRole::Tester,
            "researcher" | "research" | "docs" => SubagentRole::Researcher,
            _ => SubagentRole::Worker,
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            SubagentRole::Scout => "🔭",
            SubagentRole::Tester => "🧪",
            SubagentRole::Researcher => "📚",
            SubagentRole::Worker => "⚙️",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

impl SubagentStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, SubagentStatus::Pending | SubagentStatus::Running)
    }

    pub fn display_label(&self) -> &str {
        match self {
            SubagentStatus::Pending => "pending",
            SubagentStatus::Running => "running",
            SubagentStatus::Completed => "completed",
            SubagentStatus::Failed(_) => "failed",
            SubagentStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTask {
    pub id: String,
    pub role: SubagentRole,
    pub prompt: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: SubagentStatus,
    pub output: String,
    pub acknowledged: bool,
}

impl SubagentTask {
    pub fn duration_str(&self) -> String {
        let end = self.finished_at.unwrap_or_else(Utc::now);
        let secs = (end - self.started_at).num_seconds();
        if secs < 0 {
            "0s".to_string()
        } else if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }
}

pub struct SubagentManager {
    tasks: HashMap<String, SubagentTask>,
}

impl Default for SubagentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// Spawns or registers a new subagent task.
    pub fn register(&mut self, role: SubagentRole, prompt: String) -> String {
        let task_id = format!("sub_{:08x}", rand_u32());
        let task = SubagentTask {
            id: task_id.clone(),
            role,
            prompt,
            started_at: Utc::now(),
            finished_at: None,
            status: SubagentStatus::Running,
            output: String::new(),
            acknowledged: false,
        };
        self.tasks.insert(task_id.clone(), task);
        task_id
    }

    pub fn complete(&mut self, id: &str, output: String) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.finished_at = Some(Utc::now());
            task.status = SubagentStatus::Completed;
            task.output = output;
        }
    }

    pub fn fail(&mut self, id: &str, error: String) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.finished_at = Some(Utc::now());
            task.status = SubagentStatus::Failed(error.clone());
            task.output = error;
        }
    }

    pub fn cancel(&mut self, id: &str) -> bool {
        if let Some(task) = self.tasks.get_mut(id) {
            if task.status.is_active() {
                task.finished_at = Some(Utc::now());
                task.status = SubagentStatus::Cancelled;
                return true;
            }
        }
        false
    }

    pub fn get_task(&self, id: &str) -> Option<&SubagentTask> {
        self.tasks.get(id)
    }

    pub fn list(&self) -> Vec<&SubagentTask> {
        let mut list: Vec<&SubagentTask> = self.tasks.values().collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.started_at));
        list
    }

    pub fn active_count(&self) -> usize {
        self.tasks.values().filter(|t| t.status.is_active()).count()
    }

    /// Drains newly completed/failed subagent tasks that haven't been delivered to the lead agent yet.
    pub fn drain_unacknowledged(&mut self) -> Vec<(String, SubagentRole, String, String)> {
        let mut notices = Vec::new();
        for task in self.tasks.values_mut() {
            if !task.status.is_active() && !task.acknowledged {
                task.acknowledged = true;
                notices.push((
                    task.id.clone(),
                    task.role,
                    task.duration_str(),
                    task.output.clone(),
                ));
            }
        }
        notices
    }
}

pub fn get_subagent_manager() -> &'static Mutex<SubagentManager> {
    static INSTANCE: OnceLock<Mutex<SubagentManager>> = OnceLock::new();
    INSTANCE.get_or_init(|| Mutex::new(SubagentManager::new()))
}

fn rand_u32() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id() as u128;
    ((now ^ (pid << 32)) & 0xFFFF_FFFF) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subagent_registration_and_completion() {
        let mut mgr = SubagentManager::new();
        let id = mgr.register(SubagentRole::Scout, "Find all usages of lock_path".to_string());
        assert!(id.starts_with("sub_"));

        let task = mgr.get_task(&id).expect("task should exist");
        assert_eq!(task.role, SubagentRole::Scout);
        assert!(task.status.is_active());

        mgr.complete(&id, "Found 4 occurrences in fs_tools.rs and shell.rs".to_string());

        let task = mgr.get_task(&id).expect("task should exist");
        assert_eq!(task.status, SubagentStatus::Completed);
        assert_eq!(task.output, "Found 4 occurrences in fs_tools.rs and shell.rs");

        let drained = mgr.drain_unacknowledged();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].0, id);
        assert_eq!(drained[0].1, SubagentRole::Scout);

        // Second drain should return empty
        assert!(mgr.drain_unacknowledged().is_empty());
    }

    #[test]
    fn test_subagent_role_parsing() {
        assert_eq!(SubagentRole::parse("scout"), SubagentRole::Scout);
        assert_eq!(SubagentRole::parse("tester"), SubagentRole::Tester);
        assert_eq!(SubagentRole::parse("test"), SubagentRole::Tester);
        assert_eq!(SubagentRole::parse("researcher"), SubagentRole::Researcher);
        assert_eq!(SubagentRole::parse("docs"), SubagentRole::Researcher);
        assert_eq!(SubagentRole::parse("other"), SubagentRole::Worker);
    }
}
