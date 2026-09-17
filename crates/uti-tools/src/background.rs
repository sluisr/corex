use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::json;
use tokio::sync::mpsc;

use crate::types::{Tool, ToolContext, ToolOutput};

pub struct BackgroundProcess {
    pub pid: u32,
    pub command: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Arc<Mutex<Option<DateTime<Utc>>>>,
    pub exit_code: Arc<Mutex<Option<i32>>>,
    pub output_buffer: Arc<Mutex<String>>,
    pub is_running: Arc<Mutex<bool>>,
    pub stdin_tx: Option<mpsc::Sender<String>>,
}

impl BackgroundProcess {
    pub fn is_active(&self) -> bool {
        self.is_running.lock().map(|g| *g).unwrap_or(false)
    }

    pub fn get_exit_code(&self) -> Option<i32> {
        self.exit_code.lock().ok().and_then(|g| *g)
    }

    pub fn get_finished_at(&self) -> Option<DateTime<Utc>> {
        self.finished_at.lock().ok().and_then(|g| *g)
    }

    pub fn duration_str(&self) -> String {
        let end = self.get_finished_at().unwrap_or_else(Utc::now);
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

pub struct TaskManager {
    processes: HashMap<u32, BackgroundProcess>,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            processes: HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        pid: u32,
        command: String,
        output_buffer: Arc<Mutex<String>>,
        is_running: Arc<Mutex<bool>>,
        finished_at: Arc<Mutex<Option<DateTime<Utc>>>>,
        exit_code: Arc<Mutex<Option<i32>>>,
        stdin_tx: Option<mpsc::Sender<String>>,
    ) {
        if self.processes.len() > 50 {
            let stopped_pids: Vec<u32> = self.processes
                .iter()
                .filter(|(_, p)| !p.is_active())
                .map(|(pid, _)| *pid)
                .collect();
            for p in stopped_pids.into_iter().take(20) {
                self.processes.remove(&p);
            }
        }

        self.processes.insert(
            pid,
            BackgroundProcess {
                pid,
                command,
                started_at: Utc::now(),
                finished_at,
                exit_code,
                output_buffer,
                is_running,
                stdin_tx,
            },
        );
    }

    pub fn active_count(&self) -> usize {
        self.processes.values().filter(|p| p.is_active()).count()
    }

    pub fn get_process(&self, pid: u32) -> Option<&BackgroundProcess> {
        self.processes.get(&pid)
    }

    pub fn all_processes(&self) -> &HashMap<u32, BackgroundProcess> {
        &self.processes
    }

    pub fn list(&self) -> String {
        if self.processes.is_empty() {
            return "No background tasks found in this session.".to_string();
        }

        let mut out = Vec::new();
        out.push(format!("{:<8} {:<14} {:<10} {:<30}", "TASK ID", "STATUS", "DURATION", "COMMAND"));
        out.push("-".repeat(68));

        let mut procs: Vec<&BackgroundProcess> = self.processes.values().collect();
        procs.sort_by_key(|p| p.started_at);

        for proc in procs {
            let running = proc.is_active();
            let status = if running {
                "RUNNING".to_string()
            } else {
                match proc.get_exit_code() {
                    Some(c) => format!("EXITED({})", c),
                    None => "STOPPED".to_string(),
                }
            };
            let dur = proc.duration_str();
            let cmd_display = uti_core::truncate_ellipsis(&proc.command, 30);
            out.push(format!("{:<8} {:<14} {:<10} {:<30}", proc.pid, status, dur, cmd_display));
        }

        out.join("\n")
    }

    pub fn get_status(&self, pid: u32) -> Option<String> {
        let proc = self.processes.get(&pid)?;
        let running = proc.is_active();
        let status = if running {
            "RUNNING".to_string()
        } else {
            match proc.get_exit_code() {
                Some(c) => format!("EXITED (code: {})", c),
                None => "STOPPED / TERMINATED".to_string(),
            }
        };

        let output = proc.output_buffer.lock().map(|b| b.clone()).unwrap_or_default();
        let output_lines: Vec<&str> = output.lines().collect();
        let snippet = if output_lines.is_empty() {
            "(No output captured yet)".to_string()
        } else if output_lines.len() > 50 {
            format!("... [{} earlier lines omitted]\n{}", output_lines.len() - 50, output_lines[output_lines.len() - 50..].join("\n"))
        } else {
            output.clone()
        };

        let fin_info = match proc.get_finished_at() {
            Some(f) => format!("Finished At: {} (Duration: {})", f.to_rfc3339(), proc.duration_str()),
            None => format!("Started At: {} (Elapsed: {})", proc.started_at.to_rfc3339(), proc.duration_str()),
        };

        Some(format!(
            "Task ID (PID): {}\n\
             Command: {}\n\
             Status: {}\n\
             {}\n\
             Output Buffer: {} bytes\n\n\
             --- Recent Output ---\n\
             {}",
            proc.pid,
            proc.command,
            status,
            fin_info,
            output.len(),
            snippet
        ))
    }

    pub fn get_output(&self, pid: u32) -> Option<String> {
        let proc = self.processes.get(&pid)?;
        let guard = proc.output_buffer.lock().ok()?;
        Some(guard.clone())
    }

    pub fn send_input(&self, pid: u32, mut input: String) -> Result<bool> {
        if let Some(proc) = self.processes.get(&pid) {
            if !proc.is_active() {
                return Ok(false);
            }
            if !input.ends_with('\n') {
                input.push('\n');
            }
            if let Some(ref tx) = proc.stdin_tx {
                let _ = tx.try_send(input);
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn kill(&mut self, pid: u32) -> bool {
        if let Some(proc) = self.processes.get(&pid) {
            if let Ok(mut g) = proc.is_running.lock() {
                *g = false;
            }
            if let Ok(mut ec) = proc.exit_code.lock() {
                if ec.is_none() {
                    *ec = Some(137);
                }
            }
            if let Ok(mut fa) = proc.finished_at.lock() {
                if fa.is_none() {
                    *fa = Some(Utc::now());
                }
            }
            #[cfg(unix)]
            {
                if pid > 1 {
                    let p = pid as i32;
                    // First try direct POSIX syscalls: kill process group (-p) and process (p).
                    // These never output text to stdout/stderr and don't spawn child processes.
                    unsafe {
                        libc::kill(-p, libc::SIGTERM);
                        libc::kill(p, libc::SIGTERM);
                        libc::kill(-p, libc::SIGKILL);
                        libc::kill(p, libc::SIGKILL);
                    }
                    // Fallback to /bin/kill with stdio suppressed so no stderr leak can ever corrupt the TUI
                    let _ = std::process::Command::new("kill")
                        .arg("-9")
                        .arg(format!("-{}", pid))
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                    let _ = std::process::Command::new("kill")
                        .arg("-9")
                        .arg(pid.to_string())
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
            }
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill")
                    .arg("/F")
                    .arg("/PID")
                    .arg(pid.to_string())
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
            return true;
        }
        false
    }
}

pub static GLOBAL_TASK_MANAGER: Mutex<Option<Arc<Mutex<TaskManager>>>> = Mutex::new(None);

pub fn get_task_manager() -> Arc<Mutex<TaskManager>> {
    let mut guard = GLOBAL_TASK_MANAGER.lock().unwrap();
    if guard.is_none() {
        *guard = Some(Arc::new(Mutex::new(TaskManager::new())));
    }
    guard.as_ref().unwrap().clone()
}

// --- ListBackgroundProcessesTool ---
pub struct ListBackgroundProcessesTool;

#[async_trait]
impl Tool for ListBackgroundProcessesTool {
    fn name(&self) -> &'static str {
        "list_background_processes"
    }

    fn description(&self) -> &'static str {
        "Lists all background processes spawned during this session."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let mgr = get_task_manager();
        let guard = mgr.lock().unwrap();
        Ok(ToolOutput::success(guard.list()))
    }
}

// --- ReadBackgroundOutputTool ---
pub struct ReadBackgroundOutputTool;

#[async_trait]
impl Tool for ReadBackgroundOutputTool {
    fn name(&self) -> &'static str {
        "read_background_output"
    }

    fn description(&self) -> &'static str {
        "Reads stdout and stderr output logs of a background process by PID."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "Process ID of the background task."
                }
            },
            "required": ["pid"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(p) => p as u32,
            None => return Ok(ToolOutput::error("Missing 'pid' argument.")),
        };

        let mgr = get_task_manager();
        let guard = mgr.lock().unwrap();
        match guard.get_output(pid) {
            Some(output) if !output.is_empty() => Ok(ToolOutput::success(output)),
            Some(_) => Ok(ToolOutput::success("Process has produced no output yet.")),
            None => Ok(ToolOutput::error(format!("No background process found with PID {}", pid))),
        }
    }
}

// --- KillBackgroundProcessTool ---
pub struct KillBackgroundProcessTool;

#[async_trait]
impl Tool for KillBackgroundProcessTool {
    fn name(&self) -> &'static str {
        "kill_background_process"
    }

    fn description(&self) -> &'static str {
        "Terminates or kills a background process by PID."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "PID of the process to terminate."
                }
            },
            "required": ["pid"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(p) => p as u32,
            None => return Ok(ToolOutput::error("Missing 'pid' argument.")),
        };

        let mgr = get_task_manager();
        let mut guard = mgr.lock().unwrap();
        if guard.kill(pid) {
            Ok(ToolOutput::success(format!("Successfully terminated background process {}.", pid)))
        } else {
            Ok(ToolOutput::error(format!("No running background process found with PID {}", pid)))
        }
    }
}

// --- WriteBackgroundInputTool ---
pub struct WriteBackgroundInputTool;

#[async_trait]
impl Tool for WriteBackgroundInputTool {
    fn name(&self) -> &'static str {
        "write_background_input"
    }

    fn description(&self) -> &'static str {
        "Sends stdin input to an active background process."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "PID of the process."
                },
                "input": {
                    "type": "string",
                    "description": "Input text to send to stdin."
                }
            },
            "required": ["pid", "input"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(p) => p as u32,
            None => return Ok(ToolOutput::error("Missing 'pid' argument.")),
        };
        let input = match args.get("input").and_then(|v| v.as_str()) {
            Some(i) => i.to_string(),
            None => return Ok(ToolOutput::error("Missing 'input' argument.")),
        };

        let mgr = get_task_manager();
        let guard = mgr.lock().unwrap();
        match guard.send_input(pid, input) {
            Ok(true) => Ok(ToolOutput::success(format!("Input sent to process {}.", pid))),
            _ => Ok(ToolOutput::error(format!("Failed to send input to process {}", pid))),
        }
    }
}

// --- ManageTaskTool (Antigravity-compatible Unified Task Management Tool) ---
pub struct ManageTaskTool;

#[async_trait]
impl Tool for ManageTaskTool {
    fn name(&self) -> &'static str {
        "manage_task"
    }

    fn description(&self) -> &'static str {
        "Manage background tasks. Actions: 'list' (list running and completed background tasks), 'kill' (cancel/terminate execution), 'status' (inspect task status, duration, and output log), 'send_input' (send stdin to running task)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "kill", "status", "send_input"],
                    "description": "The action to perform: 'list', 'kill', 'status', 'send_input'."
                },
                "task_id": {
                    "type": "integer",
                    "description": "The task ID (PID) to manage. Required for 'kill', 'status', and 'send_input'."
                },
                "input": {
                    "type": "string",
                    "description": "The input to send to stdin. Required when action is 'send_input'."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let action = args.get("action")
            .or_else(|| args.get("Action"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let task_id = args.get("task_id")
            .or_else(|| args.get("TaskId"))
            .or_else(|| args.get("pid"))
            .or_else(|| args.get("Pid"))
            .and_then(|v| {
                v.as_u64().map(|n| n as u32).or_else(|| v.as_str().and_then(|s| s.parse::<u32>().ok()))
            });

        let mgr = get_task_manager();

        match action.as_str() {
            "list" => {
                let guard = mgr.lock().unwrap();
                Ok(ToolOutput::success(guard.list()))
            }
            "status" => {
                let pid = match task_id {
                    Some(p) => p,
                    None => return Ok(ToolOutput::error("Missing 'task_id' parameter for action 'status'.")),
                };
                let guard = mgr.lock().unwrap();
                match guard.get_status(pid) {
                    Some(st) => Ok(ToolOutput::success(st)),
                    None => Ok(ToolOutput::error(format!("No task found with ID {}.", pid))),
                }
            }
            "kill" => {
                let pid = match task_id {
                    Some(p) => p,
                    None => return Ok(ToolOutput::error("Missing 'task_id' parameter for action 'kill'.")),
                };
                let mut guard = mgr.lock().unwrap();
                if guard.kill(pid) {
                    Ok(ToolOutput::success(format!("Successfully terminated task {}.", pid)))
                } else {
                    Ok(ToolOutput::error(format!("No active task found with ID {}.", pid)))
                }
            }
            "send_input" => {
                let pid = match task_id {
                    Some(p) => p,
                    None => return Ok(ToolOutput::error("Missing 'task_id' parameter for action 'send_input'.")),
                };
                let input = args.get("input")
                    .or_else(|| args.get("Input"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if input.is_empty() {
                    return Ok(ToolOutput::error("Missing 'input' parameter for action 'send_input'."));
                }
                let guard = mgr.lock().unwrap();
                match guard.send_input(pid, input.to_string()) {
                    Ok(true) => Ok(ToolOutput::success(format!("Input successfully sent to task {}.", pid))),
                    _ => Ok(ToolOutput::error(format!("Task {} is not active or stdin is unavailable.", pid))),
                }
            }
            _ => Ok(ToolOutput::error(format!(
                "Unknown action '{}'. Supported actions: 'list', 'status', 'kill', 'send_input'.",
                action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_manager_register_and_list() {
        let mut mgr = TaskManager::new();
        let buf = Arc::new(Mutex::new("Starting test process...\nBuild passed.".to_string()));
        let running = Arc::new(Mutex::new(false));
        let finished = Arc::new(Mutex::new(Some(Utc::now())));
        let code = Arc::new(Mutex::new(Some(0)));

        mgr.register(
            99991,
            "cargo test".to_string(),
            buf,
            running,
            finished,
            code,
            None,
        );

        let list = mgr.list();
        assert!(list.contains("99991"));
        assert!(list.contains("EXITED(0)"));
        assert!(list.contains("cargo test"));
    }

    #[test]
    fn test_task_manager_get_status() {
        let mut mgr = TaskManager::new();
        let buf = Arc::new(Mutex::new("Line 1\nLine 2\nDone!".to_string()));
        let running = Arc::new(Mutex::new(true));
        let finished = Arc::new(Mutex::new(None));
        let code = Arc::new(Mutex::new(None));

        mgr.register(
            99992,
            "npm start".to_string(),
            buf,
            running,
            finished,
            code,
            None,
        );

        let st = mgr.get_status(99992).expect("Status should be present");
        assert!(st.contains("Task ID (PID): 99992"));
        assert!(st.contains("Status: RUNNING"));
        assert!(st.contains("npm start"));
        assert!(st.contains("Line 1"));
    }

    #[tokio::test]
    async fn test_manage_task_tool_list() {
        let tool = ManageTaskTool;
        let ctx = ToolContext {
            workspace_dir: std::env::current_dir().unwrap(),
            yolo_mode: false,
            allowed_commands: vec![],
            sudo_password: None,
        };

        let res = tool.execute(json!({ "action": "list" }), &ctx).await.unwrap();
        assert!(!res.is_error);
    }
}

