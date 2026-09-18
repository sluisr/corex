use std::fs;
use std::path::Path;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use ignore::WalkBuilder;

use crate::types::{Tool, ToolContext, ToolOutput};

// ==========================================
// 1. ReadManyFilesTool
// ==========================================
pub struct ReadManyFilesTool;

#[async_trait]
impl Tool for ReadManyFilesTool {
    fn name(&self) -> &'static str {
        "read_many_files"
    }

    fn description(&self) -> &'static str {
        "Reads and concatenates content from multiple files matching the inclusion and exclusion patterns."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "include": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Glob patterns or file paths to include."
                },
                "exclude": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Glob patterns or file paths to exclude."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to search recursively (default true)."
                },
                "useDefaultExcludes": {
                    "type": "boolean",
                    "description": "Whether to use default excludes like node_modules, target, .git (default true)."
                }
            },
            "required": ["include"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let include_vals = match args.get("include").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => return Ok(ToolOutput::error("Missing 'include' array argument.")),
        };

        let exclude_vals = args.get("exclude").and_then(|v| v.as_array());
        let recursive = args.get("recursive").and_then(|v| v.as_bool()).unwrap_or(true);
        let use_default_excludes = args.get("useDefaultExcludes").and_then(|v| v.as_bool()).unwrap_or(true);

        let mut includes = Vec::new();
        for val in include_vals {
            if let Some(s) = val.as_str() {
                if let Ok(p) = glob::Pattern::new(s) {
                    includes.push(p);
                }
            }
        }

        let mut excludes = Vec::new();
        if let Some(arr) = exclude_vals {
            for val in arr {
                if let Some(s) = val.as_str() {
                    if let Ok(p) = glob::Pattern::new(s) {
                        excludes.push(p);
                    }
                }
            }
        }

        if use_default_excludes {
            let defaults = &["**/node_modules/**", "**/target/**", "**/.git/**", "**/.cargo/**", "**/build/**", "**/dist/**"];
            for d in defaults {
                if let Ok(p) = glob::Pattern::new(d) {
                    excludes.push(p);
                }
            }
        }

        let mut max_depth = None;
        if !recursive {
            max_depth = Some(1);
        }

        let mut walker = WalkBuilder::new(&context.workspace_dir);
        walker.hidden(false).git_ignore(true);
        if let Some(depth) = max_depth {
            walker.max_depth(Some(depth));
        }

        let mut matched_files = Vec::new();
        for entry in walker.build().flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Ok(rel) = path.strip_prefix(&context.workspace_dir) {
                    let rel_str = rel.to_string_lossy();
                    
                    let mut matched = false;
                    for inc in &includes {
                        if inc.matches(&rel_str) || inc.matches(path.to_str().unwrap_or("")) {
                            matched = true;
                            break;
                        }
                    }

                    if matched {
                        let mut excluded = false;
                        for exc in &excludes {
                            if exc.matches(&rel_str) || exc.matches(path.to_str().unwrap_or("")) {
                                excluded = true;
                                break;
                            }
                        }
                        if !excluded {
                            matched_files.push(path.to_path_buf());
                        }
                    }
                }
            }
        }

        if matched_files.is_empty() {
            return Ok(ToolOutput::success("No matching files found."));
        }

        let mut concatenated = String::new();
        let max_total_bytes = 1_000_000; // 1 MB total limit
        let max_file_bytes = 200_000; // 200 KB per file limit

        for file in matched_files.iter().take(20) { // Limit to 20 files to avoid context blowout
            if concatenated.len() >= max_total_bytes {
                concatenated.push_str("\n[Output capped: Reached 1MB multi-file content limit]\n");
                break;
            }

            let rel = file.strip_prefix(&context.workspace_dir).unwrap_or(file);
            concatenated.push_str(&format!("=== FILE: {} ===\n", rel.display()));

            if crate::fs_tools::is_binary_file(file) {
                concatenated.push_str("(Skipped binary file)\n\n");
                continue;
            }

            let file_size = fs::metadata(file).map(|m| m.len()).unwrap_or(0);
            if file_size > max_file_bytes as u64 {
                concatenated.push_str(&format!(
                    "(Skipped: file size {} exceeds {} per-file limit)\n\n",
                    crate::fs_tools::format_size(file_size),
                    crate::fs_tools::format_size(max_file_bytes as u64)
                ));
                continue;
            }

            match fs::read_to_string(file) {
                Ok(content) => {
                    concatenated.push_str(&content);
                }
                Err(e) => {
                    concatenated.push_str(&format!("(Error reading file: {})\n", e));
                }
            }
            concatenated.push_str("\n\n");
        }

        if matched_files.len() > 20 {
            concatenated.push_str(&format!("... and {} more files matching the pattern.", matched_files.len() - 20));
        }

        Ok(ToolOutput::success(concatenated))
    }
}

// ==========================================
// 2. AskUserTool
// ==========================================
pub struct AskUserTool;

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &'static str {
        "ask_user"
    }

    fn description(&self) -> &'static str {
        "Requests clarification or missing information from the user via an interactive dialog."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "header": { "type": "string" },
                            "type": { "type": "string" },
                            "options": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["question"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let questions = args.get("questions");
        let mut response = String::new();
        response.push_str("Asked the user the following questions:\n");
        if let Some(arr) = questions.and_then(|q| q.as_array()) {
            for q in arr {
                if let Some(quest) = q.get("question").and_then(|v| v.as_str()) {
                    response.push_str(&format!("- Question: {}\n", quest));
                    if let Some(opts) = q.get("options").and_then(|v| v.as_array()) {
                        let opt_strs: Vec<&str> = opts.iter().filter_map(|o| o.as_str()).collect();
                        if !opt_strs.is_empty() {
                            response.push_str(&format!("  Options: {}\n", opt_strs.join(", ")));
                        }
                    }
                }
            }
        }
        response.push_str("\nExecution completed. Please await the user's direct response in the next chat message.");
        Ok(ToolOutput::success(response))
    }
}

// ==========================================
// 3. WriteTodosTool
// ==========================================
pub struct WriteTodosTool;

#[async_trait]
impl Tool for WriteTodosTool {
    fn name(&self) -> &'static str {
        "write_todos"
    }

    fn description(&self) -> &'static str {
        "Maintains an internal list of subtasks/todos. The model uses this to track its own progress."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "description": { "type": "string" },
                            "status": { "type": "string", "enum": ["pending", "completed", "in_progress"] }
                        },
                        "required": ["description", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let todos_val = match args.get("todos") {
            Some(t) => t,
            None => return Ok(ToolOutput::error("Missing 'todos' argument.")),
        };

        let dir = if context.workspace_dir.join(".corex").exists() {
            context.workspace_dir.join(".corex")
        } else if context.workspace_dir.join(".uti").exists() {
            context.workspace_dir.join(".uti")
        } else {
            context.workspace_dir.join(".corex")
        };
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("todos.json");
        
        if let Err(e) = fs::write(&path, serde_json::to_string_pretty(todos_val)?) {
            return Ok(ToolOutput::error(format!("Failed to write todos: {}", e)));
        }

        Ok(ToolOutput::success("Todos successfully updated and saved in todos.json"))
    }
}

// ==========================================
// 5. Skills and Internal Docs
// ==========================================
pub struct ActivateSkillTool;

#[async_trait]
impl Tool for ActivateSkillTool {
    fn name(&self) -> &'static str {
        "activate_skill"
    }

    fn description(&self) -> &'static str {
        "Loads specialized procedural expertise from .corex/skills, .uti/skills, or global skills directories."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the skill to load."
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => return Ok(ToolOutput::error("Missing 'name' argument.")),
        };

        // Search precedence: 1. Workspace .corex/skills 2. Workspace .uti/skills 3. Global ~/.corex/skills 4. Global ~/.uti/skills 5. Fallback .gemini/skills
        let candidates = [
            context.workspace_dir.join(".corex").join("skills").join(name).join("SKILL.md"),
            context.workspace_dir.join(".uti").join("skills").join(name).join("SKILL.md"),
            directories::BaseDirs::new()
                .map(|b| b.home_dir().join(".corex").join("skills").join(name).join("SKILL.md"))
                .unwrap_or_else(|| Path::new("").to_path_buf()),
            directories::BaseDirs::new()
                .map(|b| b.home_dir().join(".uti").join("skills").join(name).join("SKILL.md"))
                .unwrap_or_else(|| Path::new("").to_path_buf()),
            context.workspace_dir.join(".gemini").join("skills").join(name).join("SKILL.md"),
        ];

        for path in &candidates {
            if path.exists() {
                match fs::read_to_string(path) {
                    Ok(content) => return Ok(ToolOutput::success(content)),
                    Err(e) => return Ok(ToolOutput::error(format!("Failed to read skill {}: {}", name, e))),
                }
            }
        }

        Ok(ToolOutput::error(format!("Skill '{}' not found in .corex/skills or .uti/skills", name)))
    }
}

pub struct GetInternalDocsTool;

#[async_trait]
impl Tool for GetInternalDocsTool {
    fn name(&self) -> &'static str {
        "get_internal_docs"
    }

    fn description(&self) -> &'static str {
        "Accesses UTI CLI's own documentation for accurate answers about its capabilities."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path or topic in documentation."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, _args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let docs = r#"
UTI CLI (Universal Terminal Intelligence) Documentation:
- Architecture: 100% Native Rust autonomous agent with hybrid Cloud (DeepSeek) & Local (llama.cpp/Ollama) routing.
- Model Selection: Supports deepseek-flash, deepseek-v4-pro, deepseek-chat, deepseek-reasoner, and local SLM on :8080.
- Operating Modes:
  * Pure Cloud (DeepSeek Cloud API)
  * Offline Local ($0.00 air-gapped llama-server)
  * Hybrid (Auto-Triage, Local Scout, Draft & Review, Compression Only)
- Built-in Slash Commands:
  * /chat, /resume, /save, /new : Session lifecycle management
  * /model : Model switcher and hybrid settings
  * /plan : Architectural planning mode (read-only safe exploration)
  * /balance : DeepSeek account balance lookup
  * /fim : Fill-in-the-Middle code autocompletion
  * /rewind : Step back turns in the current session
  * /compress : Context compaction
  * /mcp : Model Context Protocol server inspector
  * /sudo : Session RAM AskPass authentication
  * /clear, /help, /info, /stats, /quit
- Primary Tools: apply_patch, edit, read_file, write_file, grep, glob, list_directory, run_shell_command, web_search, web_fetch.
"#;
        Ok(ToolOutput::success(docs))
    }
}

// ==========================================
// 6. Task Tracker
// ==========================================

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Task {
    id: String,
    title: String,
    description: String,
    status: String, // "pending", "in_progress", "completed", "failed"
    dependencies: Vec<String>,
}

fn load_tasks(workspace_dir: &Path) -> Vec<Task> {
    let path = workspace_dir.join(".uti").join("tasks.json");
    if path.exists() {
        if let Ok(content) = fs::read_to_string(path) {
            if let Ok(tasks) = serde_json::from_str(&content) {
                return tasks;
            }
        }
    }
    Vec::new()
}

fn save_tasks(workspace_dir: &Path, tasks: &[Task]) -> Result<()> {
    let dir = workspace_dir.join(".uti");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("tasks.json");
    fs::write(path, serde_json::to_string_pretty(tasks)?)?;
    Ok(())
}

pub struct TrackerCreateTaskTool;

#[async_trait]
impl Tool for TrackerCreateTaskTool {
    fn name(&self) -> &'static str {
        "tracker_create_task"
    }

    fn description(&self) -> &'static str {
        "Creates a new task in the tracker to monitor progress."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "description": { "type": "string" },
                "type": { "type": "string" }
            },
            "required": ["title", "description"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("Untitled Task");
        let desc = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
        
        let mut tasks = load_tasks(&context.workspace_dir);
        let id = format!("task-{}", tasks.len() + 1);
        let new_task = Task {
            id: id.clone(),
            title: title.to_string(),
            description: desc.to_string(),
            status: "pending".to_string(),
            dependencies: Vec::new(),
        };
        tasks.push(new_task);
        save_tasks(&context.workspace_dir, &tasks)?;

        Ok(ToolOutput::success(format!("Successfully created task '{}' (ID: {})", title, id)))
    }
}

pub struct TrackerUpdateTaskTool;

#[async_trait]
impl Tool for TrackerUpdateTaskTool {
    fn name(&self) -> &'static str {
        "tracker_update_task"
    }

    fn description(&self) -> &'static str {
        "Updates the status or details of an existing task."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "title": { "type": "string" },
                "description": { "type": "string" },
                "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "failed"] },
                "dependencies": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => return Ok(ToolOutput::error("Missing 'id' argument.")),
        };

        let mut tasks = load_tasks(&context.workspace_dir);
        let mut found = false;
        for t in &mut tasks {
            if t.id == id {
                found = true;
                if let Some(title) = args.get("title").and_then(|v| v.as_str()) {
                    t.title = title.to_string();
                }
                if let Some(desc) = args.get("description").and_then(|v| v.as_str()) {
                    t.description = desc.to_string();
                }
                if let Some(status) = args.get("status").and_then(|v| v.as_str()) {
                    t.status = status.to_string();
                }
                if let Some(deps) = args.get("dependencies").and_then(|v| v.as_array()) {
                    t.dependencies = deps.iter().filter_map(|d| d.as_str().map(|s| s.to_string())).collect();
                }
                break;
            }
        }

        if !found {
            return Ok(ToolOutput::error(format!("Task with ID '{}' not found.", id)));
        }

        save_tasks(&context.workspace_dir, &tasks)?;
        Ok(ToolOutput::success(format!("Task '{}' successfully updated.", id)))
    }
}

pub struct TrackerGetTaskTool;

#[async_trait]
impl Tool for TrackerGetTaskTool {
    fn name(&self) -> &'static str {
        "tracker_get_task"
    }

    fn description(&self) -> &'static str {
        "Retrieves the details and current status of a specific tracked task."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => return Ok(ToolOutput::error("Missing 'id' argument.")),
        };

        let tasks = load_tasks(&context.workspace_dir);
        if let Some(t) = tasks.iter().find(|t| t.id == id) {
            Ok(ToolOutput::success(serde_json::to_string_pretty(t)?))
        } else {
            Ok(ToolOutput::error(format!("Task with ID '{}' not found.", id)))
        }
    }
}

pub struct TrackerListTasksTool;

#[async_trait]
impl Tool for TrackerListTasksTool {
    fn name(&self) -> &'static str {
        "tracker_list_tasks"
    }

    fn description(&self) -> &'static str {
        "Lists all tasks currently being tracked."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string" },
                "type": { "type": "string" },
                "parentId": { "type": "string" }
            }
        })
    }

    async fn execute(&self, _args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let tasks = load_tasks(&context.workspace_dir);
        if tasks.is_empty() {
            Ok(ToolOutput::success("No tasks tracked yet."))
        } else {
            Ok(ToolOutput::success(serde_json::to_string_pretty(&tasks)?))
        }
    }
}

fn has_dependency_path(tasks: &[Task], start_id: &str, target_id: &str) -> bool {
    if start_id == target_id {
        return true;
    }
    if let Some(task) = tasks.iter().find(|t| t.id == start_id) {
        for dep in &task.dependencies {
            if has_dependency_path(tasks, dep, target_id) {
                return true;
            }
        }
    }
    false
}

pub struct TrackerAddDependencyTool;

#[async_trait]
impl Tool for TrackerAddDependencyTool {
    fn name(&self) -> &'static str {
        "tracker_add_dependency"
    }

    fn description(&self) -> &'static str {
        "Adds a dependency between two existing tasks in the tracker."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "taskId": { "type": "string" },
                "dependencyId": { "type": "string" }
            },
            "required": ["taskId", "dependencyId"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let task_id = match args.get("taskId").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => return Ok(ToolOutput::error("Missing 'taskId' argument.")),
        };
        let dep_id = match args.get("dependencyId").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => return Ok(ToolOutput::error("Missing 'dependencyId' argument.")),
        };

        let mut tasks = load_tasks(&context.workspace_dir);
        
        let has_task = tasks.iter().any(|t| t.id == task_id);
        let has_dep = tasks.iter().any(|t| t.id == dep_id);
        
        if !has_task {
            return Ok(ToolOutput::error(format!("Target task ID '{}' not found.", task_id)));
        }
        if !has_dep {
            return Ok(ToolOutput::error(format!("Dependency task ID '{}' not found.", dep_id)));
        }

        // Cycle detection: check if dep_id already transitively depends on task_id
        if has_dependency_path(&tasks, dep_id, task_id) {
            return Ok(ToolOutput::error(format!(
                "Circular dependency error: Adding '{}' as a dependency of '{}' would create a loop.",
                dep_id, task_id
            )));
        }

        let mut found = false;
        for t in &mut tasks {
            if t.id == task_id {
                found = true;
                if !t.dependencies.contains(&dep_id.to_string()) {
                    t.dependencies.push(dep_id.to_string());
                }
                break;
            }
        }

        if !found {
            return Ok(ToolOutput::error(format!("Task ID '{}' not found.", task_id)));
        }

        save_tasks(&context.workspace_dir, &tasks)?;
        Ok(ToolOutput::success(format!("Added dependency: task '{}' now depends on '{}'", task_id, dep_id)))
    }
}

pub struct TrackerVisualizeTool;

fn print_task_tree(
    tasks: &[Task],
    task_id: &str,
    prefix: &str,
    is_last: bool,
    visited: &mut std::collections::HashSet<String>,
    out: &mut String,
) {
    if !visited.insert(task_id.to_string()) {
        return;
    }
    if let Some(t) = tasks.iter().find(|x| x.id == task_id) {
        let status_symbol = match t.status.as_str() {
            "completed" => "✔",
            "in_progress" => "⚡",
            "failed" => "✘",
            _ => "☐",
        };
        let connector = if prefix.is_empty() { "" } else if is_last { "└── " } else { "├── " };
        out.push_str(&format!("{}{}[{}] {} ({})\n", prefix, connector, status_symbol, t.title, t.id));
        
        let child_prefix = if prefix.is_empty() {
            "".to_string()
        } else {
            format!("{}{}", prefix, if is_last { "    " } else { "│   " })
        };
        
        // Dependents are tasks that have task_id in their dependency array
        let mut dependents: Vec<&Task> = tasks
            .iter()
            .filter(|x| x.dependencies.iter().any(|d| d == task_id))
            .collect();
        dependents.sort_by_key(|x| &x.id);
        
        let len = dependents.len();
        for (i, dep) in dependents.iter().enumerate() {
            let next_prefix = if prefix.is_empty() {
                "".to_string()
            } else {
                child_prefix.clone()
            };
            // For the first level root nodes, we treat the connector prefix specifically
            let root_child_prefix = if prefix.is_empty() {
                if i == len - 1 { "    " } else { "│   " }.to_string()
            } else {
                next_prefix
            };
            
            print_task_tree(tasks, &dep.id, &root_child_prefix, i == len - 1, visited, out);
        }
    }
}

#[async_trait]
impl Tool for TrackerVisualizeTool {
    fn name(&self) -> &'static str {
        "tracker_visualize"
    }

    fn description(&self) -> &'static str {
        "Generates a visual representation of the current task dependency graph."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let tasks = load_tasks(&context.workspace_dir);
        if tasks.is_empty() {
            return Ok(ToolOutput::success("No tasks to visualize."));
        }

        let mut output = String::new();
        output.push_str("Task execution timeline graph:\n\n");
        
        // Root tasks are tasks that have NO dependencies
        let mut root_tasks: Vec<&Task> = tasks
            .iter()
            .filter(|t| t.dependencies.is_empty())
            .collect();
        root_tasks.sort_by_key(|x| &x.id);

        let mut visited = std::collections::HashSet::new();
        let len = root_tasks.len();
        for (i, root) in root_tasks.iter().enumerate() {
            print_task_tree(&tasks, &root.id, "", i == len - 1, &mut visited, &mut output);
            if i < len - 1 {
                output.push('\n');
            }
        }

        // Print any tasks that were missed due to cycles (safety fallback)
        let missed_tasks: Vec<&Task> = tasks
            .iter()
            .filter(|t| !visited.contains(&t.id))
            .collect();
        if !missed_tasks.is_empty() {
            output.push_str("\n--- Unreachable / Circular Tasks ---\n");
            for t in missed_tasks {
                output.push_str(&format!("  [✘] {} ({}) - Depends on: {}\n", t.title, t.id, t.dependencies.join(", ")));
            }
        }

        Ok(ToolOutput::success(output))
    }
}

// ==========================================
// 7. Update Topic
// ==========================================
pub struct UpdateTopicTool;

#[async_trait]
impl Tool for UpdateTopicTool {
    fn name(&self) -> &'static str {
        "update_topic"
    }

    fn description(&self) -> &'static str {
        "Updates the current topic and status to keep the user informed of progress."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "summary": { "type": "string" },
                "strategic_intent": { "type": "string" }
            },
            "required": ["title", "summary"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let summary = args.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        let intent = args.get("strategic_intent").and_then(|v| v.as_str()).unwrap_or("");

        let dir = context.workspace_dir.join(".uti");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("topic.json");

        let topic_json = json!({
            "title": title,
            "summary": summary,
            "strategic_intent": intent
        });

        fs::write(path, serde_json::to_string_pretty(&topic_json)?)?;
        Ok(ToolOutput::success(format!("Topic updated: {}", title)))
    }
}
