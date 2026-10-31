use std::fs;
use std::path::Path;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use ignore::WalkBuilder;

use crate::types::{Tool, ToolContext, ToolOutput};

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
        walker.hidden(false).git_ignore(true).filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != ".git" && name != "node_modules" && name != "target"
        });
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
        let max_total_bytes = 256_000; // 256 KB total limit (tool outputs are truncated before reaching the LLM anyway)
        let max_file_bytes = 64_000; // 64 KB per file limit

        for file in matched_files.iter().take(20) { // Limit to 20 files to avoid context blowout
            if concatenated.len() >= max_total_bytes {
                concatenated.push_str("\n[Output capped: Reached 256KB multi-file content limit]\n");
                break;
            }

            let rel = file.strip_prefix(&context.workspace_dir).unwrap_or(file);
            concatenated.push_str(&format!("=== FILE: {} ===\n", rel.display()));

            let readable = std::iter::once(&context.workspace_dir)
                .chain(&context.extra_dirs)
                .any(|root| crate::fs_tools::is_within_workspace(file, root));
            if !readable {
                concatenated.push_str("(Skipped symlink escaping workspace)\n\n");
                continue;
            }

            if crate::fs_tools::is_sensitive_system_path(file) {
                concatenated.push_str("(Skipped sensitive system/credential file)\n\n");
                continue;
            }

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
                    crate::fs_tools::observe_file(file);
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
        let Some(items) = todos_val.as_array() else {
            return Ok(ToolOutput::error("'todos' must be a list of {description, status} items."));
        };
        for (i, item) in items.iter().enumerate() {
            let description = item.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if description.trim().is_empty() {
                return Ok(ToolOutput::error(format!("Todo {} has no description.", i + 1)));
            }
            if !matches!(status, "pending" | "in_progress" | "completed") {
                return Ok(ToolOutput::error(format!(
                    "Todo {} has status '{}': use pending, in_progress or completed.",
                    i + 1,
                    status
                )));
            }
        }

        let content = serde_json::to_string_pretty(todos_val)?;
        if let Err(e) = write_workspace_corex_json(&context.workspace_dir, "todos.json", &content) {
            return Ok(ToolOutput::error(format!("Failed to write todos: {}", e)));
        }

        Ok(ToolOutput::success("Todos successfully updated and saved in todos.json"))
    }
}

pub struct ActivateSkillTool;

#[async_trait]
impl Tool for ActivateSkillTool {
    fn name(&self) -> &'static str {
        "activate_skill"
    }

    fn description(&self) -> &'static str {
        "Loads specialized procedural expertise from .agents/skills, .corex/skills, or global skills directories."
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
            Some(n) => n.trim(),
            None => return Ok(ToolOutput::error("Missing 'name' argument.")),
        };

        if name.is_empty()
            || name.contains("..")
            || name.contains('/')
            || name.contains('\\')
            || name.contains('\0')
        {
            return Ok(ToolOutput::error(format!("Invalid skill name '{}'.", name)));
        }

        // Search precedence: 1. Workspace .agents/skills 2. Workspace .corex/skills 3. Global ~/.agents/skills 4. Global ~/.corex/skills
        let home_dir = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
        let candidates = [
            (
                context.workspace_dir.join(".agents").join("skills"),
                context.workspace_dir.join(".agents").join("skills").join(name).join("SKILL.md"),
            ),
            (
                context.workspace_dir.join(".corex").join("skills"),
                context.workspace_dir.join(".corex").join("skills").join(name).join("SKILL.md"),
            ),
            (
                home_dir.as_ref().map(|h| h.join(".agents").join("skills")).unwrap_or_default(),
                home_dir
                    .as_ref()
                    .map(|h| h.join(".agents").join("skills").join(name).join("SKILL.md"))
                    .unwrap_or_default(),
            ),
            (
                home_dir.as_ref().map(|h| h.join(".corex").join("skills")).unwrap_or_default(),
                home_dir
                    .as_ref()
                    .map(|h| h.join(".corex").join("skills").join(name).join("SKILL.md"))
                    .unwrap_or_default(),
            ),
        ];

        for (base_root, path) in &candidates {
            if !path.as_os_str().is_empty()
                && path.exists()
                && crate::fs_tools::is_within_workspace(path, base_root)
                && !crate::fs_tools::is_sensitive_system_path(path)
            {
                match fs::read_to_string(path) {
                    Ok(content) => return Ok(ToolOutput::success(content)),
                    Err(e) => return Ok(ToolOutput::error(format!("Failed to read skill {}: {}", name, e))),
                }
            }
        }

        // Built-in skills fallback (always available anywhere)
        if let Some(content) = corex_core::builtin_skills::get_builtin_skill(name) {
            return Ok(ToolOutput::success(content));
        }

        Ok(ToolOutput::error(format!("Skill '{}' not found in .agents/skills, .corex/skills or built-in skills", name)))
    }
}

pub struct GetInternalDocsTool;

#[async_trait]
impl Tool for GetInternalDocsTool {
    fn name(&self) -> &'static str {
        "get_internal_docs"
    }

    fn description(&self) -> &'static str {
        "Accesses Corex's own documentation for accurate answers about its capabilities."
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
Corex Documentation:
- Architecture: Rust autonomous agent with DeepSeek Cloud, Anthropic Claude, and local (llama.cpp/Ollama) inference.
- Model Selection: Supports deepseek-flash, deepseek-v4-pro, deepseek-chat, deepseek-reasoner, claude-opus-4-6, claude-sonnet-4-6, claude-haiku-4-5, and local OpenAI-compatible servers.
- Operating Modes:
  * Cloud API (DeepSeek / Anthropic)
  * Offline Local (llama-server / Ollama)
- Built-in Slash Commands:
  * /chat, /resume, /save, /new : Session lifecycle management
  * /model : Model switcher
  * /local : Query local LLM directly
  * /web : Search live internet via DeepSeek
  * /yolo : Toggle auto-approval
  * /plan : Architectural planning mode (read-only exploration)
  * /balance : DeepSeek account balance lookup
  * /fim : Fill-in-the-Middle code autocompletion
  * /rewind : Step back turns in the current session
  * /compress, /compact : Context compaction
  * /mcp : Model Context Protocol server inspector
  * /tasks : Background task manager
  * /sudo : Session RAM AskPass authentication
  * /update : Check for newer Corex releases
  * /clear, /help, /info, /stats, /quit
- Primary Tools: apply_patch, edit, read_file, write_file, grep, glob, list_directory, run_shell_command, web_search, web_fetch.
"#;
        Ok(ToolOutput::success(docs))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Task {
    id: String,
    title: String,
    description: String,
    status: String, // "pending", "in_progress", "completed", "failed"
    dependencies: Vec<String>,
}

fn write_workspace_corex_json(workspace_dir: &Path, filename: &str, content: &str) -> std::io::Result<()> {
    let dir = workspace_dir.join(".corex");
    if let Ok(meta) = fs::symlink_metadata(&dir) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Refusing to write through a symlinked .corex directory",
            ));
        }
    }
    let path = dir.join(filename);
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("Refusing to write to symlinked {}", filename),
            ));
        }
    }
    if !crate::fs_tools::is_within_workspace(&path, workspace_dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("Target {} resolves outside the workspace", filename),
        ));
    }
    corex_core::write_private_atomic(&path, content)
}

fn load_tasks(workspace_dir: &Path) -> Vec<Task> {
    let path = workspace_dir.join(".corex").join("tasks.json");
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink())
        || !crate::fs_tools::is_within_workspace(&path, workspace_dir)
    {
        return Vec::new();
    }
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
    write_workspace_corex_json(workspace_dir, "tasks.json", &serde_json::to_string_pretty(tasks)?)?;
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

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let tasks = load_tasks(&context.workspace_dir);
        if tasks.is_empty() {
            return Ok(ToolOutput::success("No tasks tracked yet."));
        }
        let status_filter = args.get("status").and_then(|v| v.as_str()).filter(|s| !s.is_empty());

        let filtered: Vec<&Task> = tasks
            .iter()
            .filter(|t| status_filter.map_or(true, |s| t.status.eq_ignore_ascii_case(s)))
            .collect();

        if filtered.is_empty() {
            Ok(ToolOutput::success("No tasks matched the given filters."))
        } else {
            Ok(ToolOutput::success(serde_json::to_string_pretty(&filtered)?))
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
            "in_progress" => "●",
            "failed" => "✘",
            _ => "○",
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

        let topic_json = json!({
            "title": title,
            "summary": summary,
            "strategic_intent": intent
        });

        write_workspace_corex_json(
            &context.workspace_dir,
            "topic.json",
            &serde_json::to_string_pretty(&topic_json)?,
        )?;
        Ok(ToolOutput::success(format!("Topic updated: {}", title)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_activate_skill_loads_builtin() {
        let tool = ActivateSkillTool;
        let ctx = ToolContext {
            workspace_dir: PathBuf::from("/tmp/nonexistent-skill-workspace-xyz"),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: vec![],
            extra_dirs: Vec::new(),
        };

        let result = tool.execute(json!({"name": "corex-about"}), &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("sluisr"));
        assert!(result.output.contains("Corex"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_tools_refuse_symlinked_targets() {
        let ws = std::env::temp_dir().join(format!("corex_f07_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        let corex_dir = ws.join(".corex");
        fs::create_dir_all(&corex_dir).unwrap();

        let outside_victim = std::env::temp_dir().join(format!("corex_f07_victim_{}", std::process::id()));
        fs::write(&outside_victim, "ORIGINAL_SECRET").unwrap();

        std::os::unix::fs::symlink(&outside_victim, corex_dir.join("todos.json")).unwrap();
        std::os::unix::fs::symlink(&outside_victim, corex_dir.join("tasks.json")).unwrap();
        std::os::unix::fs::symlink(&outside_victim, corex_dir.join("topic.json")).unwrap();

        let ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: vec![],
            extra_dirs: Vec::new(),
        };

        let _ = WriteTodosTool
            .execute(json!({"todos": [{"description": "pwn", "status": "pending"}]}), &ctx)
            .await;
        let _ = TrackerCreateTaskTool
            .execute(json!({"title": "pwn", "description": "pwn"}), &ctx)
            .await;
        let _ = UpdateTopicTool
            .execute(json!({"title": "pwn", "summary": "pwn"}), &ctx)
            .await;

        let victim_after = fs::read_to_string(&outside_victim).unwrap();
        let _ = fs::remove_dir_all(&ws);
        let _ = fs::remove_file(&outside_victim);
        assert_eq!(victim_after, "ORIGINAL_SECRET", "symlinked metadata files must never overwrite target");
    }

    #[tokio::test]
    async fn write_todos_rejects_malformed_items() {
        let ws = std::env::temp_dir().join(format!("corex_todos_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        let ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: vec![],
            extra_dirs: vec![],
        };
        let run = |todos: serde_json::Value| {
            let ctx = &ctx;
            async move { WriteTodosTool.execute(json!({ "todos": todos }), ctx).await.unwrap() }
        };
        assert!(run(json!([{"description": "x", "status": "weird"}])).await.is_error);
        assert!(run(json!([{"description": "  ", "status": "pending"}])).await.is_error);
        assert!(run(json!("not a list")).await.is_error);
        assert!(!run(json!([{"description": "probar", "status": "in_progress"}])).await.is_error);
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_many_files_follows_links_into_added_directories() {
        let ws = std::env::temp_dir().join(format!("corex_rmf_ws_{}", std::process::id()));
        let data = std::env::temp_dir().join(format!("corex_rmf_data_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        let _ = fs::remove_dir_all(&data);
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("notes.txt"), "SHARED_DATA").unwrap();
        std::os::unix::fs::symlink(data.join("notes.txt"), ws.join("notes.txt")).unwrap();

        let ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: vec![],
            extra_dirs: vec![data.clone()],
        };
        let out = ReadManyFilesTool.execute(json!({"include": ["*.txt"]}), &ctx).await.unwrap();

        let _ = fs::remove_dir_all(&ws);
        let _ = fs::remove_dir_all(&data);
        assert!(out.output.contains("SHARED_DATA"), "{}", out.output);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escapes_and_credential_listings_are_blocked() {
        let ws = std::env::temp_dir().join(format!("corex_f12_ws_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        let skill_dir = ws.join(".agents").join("skills").join("escape");
        fs::create_dir_all(&skill_dir).unwrap();

        let outside_file = std::env::temp_dir().join(format!("corex_f12_outside_{}.txt", std::process::id()));
        fs::write(&outside_file, "TOP_SECRET_OUTSIDE_WORKSPACE").unwrap();

        std::os::unix::fs::symlink(&outside_file, ws.join("linked.txt")).unwrap();
        std::os::unix::fs::symlink(&outside_file, skill_dir.join("SKILL.md")).unwrap();

        let ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: vec![],
            extra_dirs: Vec::new(),
        };

        let many = ReadManyFilesTool
            .execute(json!({"include": ["*.txt"]}), &ctx)
            .await
            .unwrap();
        let skill = ActivateSkillTool
            .execute(json!({"name": "escape"}), &ctx)
            .await
            .unwrap();
        let grep = crate::grep::GrepTool
            .execute(json!({"pattern": "TOP_SECRET"}), &ctx)
            .await
            .unwrap();
        let ls_ssh = crate::fs_tools::LsTool
            .execute(json!({"dir_path": "~/.ssh"}), &ctx)
            .await
            .unwrap();

        let _ = fs::remove_dir_all(&ws);
        let _ = fs::remove_file(&outside_file);

        assert!(!many.output.contains("TOP_SECRET_OUTSIDE_WORKSPACE"));
        assert!(!skill.output.contains("TOP_SECRET_OUTSIDE_WORKSPACE"));
        assert!(!grep.output.contains("TOP_SECRET_OUTSIDE_WORKSPACE"));
        assert!(ls_ssh.is_error, "LsTool must refuse sensitive credential directories");
    }
}
