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
    pub output_buffer: Arc<Mutex<String>>,
    pub is_running: Arc<Mutex<bool>>,
    pub stdin_tx: Option<mpsc::Sender<String>>,
}

pub struct TaskManager {
    processes: HashMap<u32, BackgroundProcess>,
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
        stdin_tx: Option<mpsc::Sender<String>>,
    ) {
        self.processes.insert(
            pid,
            BackgroundProcess {
                pid,
                command,
                started_at: Utc::now(),
                output_buffer,
                is_running,
                stdin_tx,
            },
        );
    }

    pub fn list(&self) -> String {
        if self.processes.is_empty() {
            return "No background processes running in this session.".to_string();
        }

        let mut out = Vec::new();
        out.push(format!("{:<8} {:<10} {:<30}", "PID", "STATUS", "COMMAND"));
        out.push("-".repeat(50));

        for (pid, proc) in &self.processes {
            let running = proc.is_running.lock().map(|g| *g).unwrap_or(false);
            let status = if running { "RUNNING" } else { "STOPPED" };
            out.push(format!("{:<8} {:<10} {:<30}", pid, status, proc.command));
        }

        out.join("\n")
    }

    pub fn get_output(&self, pid: u32) -> Option<String> {
        let proc = self.processes.get(&pid)?;
        let guard = proc.output_buffer.lock().ok()?;
        Some(guard.clone())
    }

    pub fn send_input(&self, pid: u32, input: String) -> Result<bool> {
        if let Some(proc) = self.processes.get(&pid) {
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
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
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
