use std::collections::HashMap;
use std::sync::Arc;
use anyhow::Result;
use corex_core::types::ToolDefinition;

use crate::apply_patch::ApplyPatchTool;
use crate::background::{
    KillBackgroundProcessTool, ListBackgroundProcessesTool, ManageTaskTool, ReadBackgroundOutputTool,
    WriteBackgroundInputTool,
};
use crate::fs_tools::{EditTool, GlobTool, LsTool, ReadFileTool, ReadImageTool, WriteFileTool};
use crate::grep::GrepTool;
use crate::memory_tool::SaveMemoryTool;
use crate::shell::ShellTool;
use crate::types::{Tool, ToolContext, ToolOutput};
use crate::web::{WebFetchTool, WebSearchTool};
use crate::extra_tools::{
    ReadManyFilesTool, AskUserTool, WriteTodosTool, ActivateSkillTool,
    GetInternalDocsTool, TrackerCreateTaskTool, TrackerUpdateTaskTool,
    TrackerGetTaskTool, TrackerListTasksTool, TrackerAddDependencyTool,
    TrackerVisualizeTool, UpdateTopicTool,
};
use crate::subagent::{DelegateSubagentTool, ManageSubagentsTool};

#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    registered_definitions: Vec<ToolDefinition>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let _ = crate::verify_tools_provenance();
        let mut registry = Self {
            tools: HashMap::new(),
            registered_definitions: Vec::new(),
        };

        let apply_patch = Arc::new(ApplyPatchTool);
        let read_file = Arc::new(ReadFileTool);
        let write_file = Arc::new(WriteFileTool);
        let edit = Arc::new(EditTool);
        let grep_search = Arc::new(GrepTool);
        let glob = Arc::new(GlobTool);
        let list_directory = Arc::new(LsTool);
        let run_shell_command = Arc::new(ShellTool);
        let manage_task = Arc::new(ManageTaskTool);
        let list_background_processes = Arc::new(ListBackgroundProcessesTool);
        let read_background_output = Arc::new(ReadBackgroundOutputTool);
        let kill_background_process = Arc::new(KillBackgroundProcessTool);
        let write_background_input = Arc::new(WriteBackgroundInputTool);
        let web_search = Arc::new(WebSearchTool);
        let web_fetch = Arc::new(WebFetchTool);

        let read_many_files = Arc::new(ReadManyFilesTool);
        let ask_user = Arc::new(AskUserTool);
        let write_todos = Arc::new(WriteTodosTool);
        let activate_skill = Arc::new(ActivateSkillTool);
        let get_internal_docs = Arc::new(GetInternalDocsTool);
        let tracker_create_task = Arc::new(TrackerCreateTaskTool);
        let tracker_update_task = Arc::new(TrackerUpdateTaskTool);
        let tracker_get_task = Arc::new(TrackerGetTaskTool);
        let tracker_list_tasks = Arc::new(TrackerListTasksTool);
        let tracker_add_dependency = Arc::new(TrackerAddDependencyTool);
        let tracker_visualize = Arc::new(TrackerVisualizeTool);
        let update_topic = Arc::new(UpdateTopicTool);
        let delegate_subagent = Arc::new(DelegateSubagentTool);
        let manage_subagents = Arc::new(ManageSubagentsTool);

        // Register official primary tool definitions
        registry.register_primary(apply_patch.clone());
        registry.register_primary(read_file.clone());
        registry.register_primary(Arc::new(ReadImageTool));
        registry.register_primary(write_file.clone());
        registry.register_primary(edit.clone());
        registry.register_primary(grep_search.clone());
        registry.register_primary(glob.clone());
        registry.register_primary(list_directory.clone());
        registry.register_primary(run_shell_command.clone());
        registry.register_primary(manage_task.clone());
        registry.register_primary(list_background_processes.clone());
        registry.register_primary(read_background_output.clone());
        registry.register_primary(kill_background_process.clone());
        registry.register_primary(write_background_input.clone());
        registry.register_primary(web_search.clone());
        registry.register_primary(web_fetch.clone());

        registry.register_primary(read_many_files);
        registry.register_primary(ask_user);
        registry.register_primary(write_todos);
        registry.register_primary(Arc::new(SaveMemoryTool));
        registry.register_primary(activate_skill);
        registry.register_primary(get_internal_docs);
        registry.register_primary(tracker_create_task);
        registry.register_primary(tracker_update_task);
        registry.register_primary(tracker_get_task);
        registry.register_primary(tracker_list_tasks);
        registry.register_primary(tracker_add_dependency);
        registry.register_primary(tracker_visualize);
        registry.register_primary(update_topic);
        registry.register_primary(delegate_subagent.clone());
        registry.register_primary(manage_subagents.clone());

        // Tool name aliases for compatibility across model families:
        registry.tools.insert("replace".to_string(), edit.clone());
        registry.tools.insert("edit_file".to_string(), edit);
        registry.tools.insert("grep".to_string(), grep_search.clone());
        registry.tools.insert("search_files".to_string(), grep_search);
        registry.tools.insert("google_web_search".to_string(), web_search.clone());
        registry.tools.insert("web_search".to_string(), web_search);
        registry.tools.insert("ls".to_string(), list_directory.clone());
        registry.tools.insert("read_dir".to_string(), list_directory);
        registry.tools.insert("run_command".to_string(), run_shell_command.clone());
        registry.tools.insert("execute_command".to_string(), run_shell_command.clone());
        registry.tools.insert("bash".to_string(), run_shell_command);
        registry.tools.insert("view_file".to_string(), read_file);
        registry.tools.insert("find_files".to_string(), glob.clone());
        registry.tools.insert("tasks".to_string(), manage_task);
        registry.tools.insert("invoke_subagent".to_string(), delegate_subagent.clone());
        registry.tools.insert("subagent".to_string(), delegate_subagent);
        registry.tools.insert("subagents".to_string(), manage_subagents);

        registry
    }

    fn register_primary(&mut self, tool: Arc<dyn Tool>) {
        let def = tool.to_definition();
        if let Some(pos) = self.registered_definitions.iter().position(|d| d.function.name == def.function.name) {
            self.registered_definitions[pos] = def;
        } else {
            self.registered_definitions.push(def);
        }
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.register_primary(tool);
    }

    pub fn unregister(&mut self, name: &str) {
        self.tools.remove(name);
        self.registered_definitions.retain(|d| d.function.name != name);
    }

    pub fn unregister_prefix(&mut self, prefix: &str) {
        self.tools.retain(|k, _| !k.starts_with(prefix));
        self.registered_definitions.retain(|d| !d.function.name.starts_with(prefix));
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn list_definitions(&self) -> Vec<ToolDefinition> {
        self.registered_definitions.clone()
    }

    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolOutput> {
        let Some(tool) = self.get(name) else {
            return Ok(ToolOutput::error(format!("Unknown tool: {}", name)));
        };
        let edited = edited_file(tool.name(), &args, context);
        // Edits to the same file run one at a time here, so each records the state the previous
        // one left instead of all of them claiming the original as their "before".
        let _turn = match &edited {
            Some(path) => Some(edit_lock(path).lock_owned().await),
            None => None,
        };
        let before = edited.as_deref().and_then(corex_core::file_history::snapshot);
        let result = tool.execute(args, context).await;
        if let (Some(path), Some(before)) = (edited, before) {
            if let Some(after) = corex_core::file_history::snapshot(&path) {
                corex_core::file_history::record(path, before, after);
            }
        }
        result
    }
}

/// The file a file-editing tool call will change, so `/rewind` can undo it.
/// One async mutex per edited path, shared by every registry in the process.
fn edit_lock(path: &std::path::Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<std::sync::Mutex<HashMap<std::path::PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
        std::sync::OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    locks.entry(path.to_path_buf()).or_default().clone()
}

fn edited_file(tool: &str, args: &serde_json::Value, context: &ToolContext) -> Option<std::path::PathBuf> {
    let arg = |key: &str| args.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let raw = match tool {
        "write_file" | "edit" => arg("file_path"),
        "apply_patch" => ["patch", "input", "diff"]
            .iter()
            .find_map(|k| arg(k))
            .and_then(|patch| ApplyPatchTool::extract_file_path(&patch))
            .or_else(|| arg("file_path").or_else(|| arg("path")).or_else(|| arg("file"))),
        _ => None,
    }?;
    Some(crate::fs_tools::resolve_path(&context.workspace_dir, &raw))
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `file_history` keeps pending edits process-wide, so the tests that drain it take turns.
    static HISTORY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn file_edits_are_recorded_for_rewind() {
        let _history = HISTORY.lock().await;
        let ws = std::env::temp_dir().join(format!("corex_registry_rewind_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        let ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };
        let reg = ToolRegistry::new();
        let _ = corex_core::file_history::drain(0);

        reg.execute("write_file", serde_json::json!({"file_path": "notes.txt", "content": "hi\n"}), &ctx).await.unwrap();
        reg.execute("replace", serde_json::json!({"file_path": "notes.txt", "old_string": "hi", "new_string": "bye"}), &ctx).await.unwrap();
        let edits: Vec<_> = corex_core::file_history::drain(7)
            .into_iter()
            .filter(|e| e.path.starts_with(&ws))
            .collect();
        let _ = std::fs::remove_dir_all(&ws);

        assert_eq!(edits.len(), 2);
        assert_eq!((edits[0].before.as_deref(), edits[0].after.as_deref()), (None, Some("hi\n")));
        assert_eq!((edits[1].before.as_deref(), edits[1].after.as_deref()), (Some("hi\n"), Some("bye\n")));
        assert!(edits.iter().all(|e| e.turn == 7));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_edits_to_one_file_record_a_chain_that_reverts_cleanly() {
        let _history = HISTORY.lock().await;
        let ws = std::env::temp_dir().join(format!("corex_registry_parallel_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).unwrap();
        let file = ws.join("calc.py");
        let original = "def add(a, b):\n    return a - b\n\n\ndef mean(values):\n    return sum(values) / len(values)\n";
        std::fs::write(&file, original).unwrap();
        crate::fs_tools::observe_file(&file);
        let ctx = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };
        let reg = std::sync::Arc::new(ToolRegistry::new());
        let _ = corex_core::file_history::drain(0);

        let edits = [
            serde_json::json!({"file_path": "calc.py", "old_string": "a - b", "new_string": "a + b"}),
            serde_json::json!({"file_path": "calc.py", "old_string": "def mean(values):\n", "new_string": "def mean(values):\n    if not values:\n        return 0\n"}),
        ];
        let handles: Vec<_> = edits
            .into_iter()
            .map(|args| {
                let (reg, ctx) = (reg.clone(), ctx.clone());
                tokio::spawn(async move { reg.execute("edit", args, &ctx).await.unwrap() })
            })
            .collect();
        for h in handles {
            assert!(!h.await.unwrap().is_error);
        }
        let recorded: Vec<_> = corex_core::file_history::drain(1).into_iter().filter(|e| e.path == file).collect();
        let warnings = corex_core::file_history::revert(&recorded);
        let restored = std::fs::read_to_string(&file).unwrap();
        let _ = std::fs::remove_dir_all(&ws);

        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].after, recorded[1].before, "each edit must start where the previous one ended");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(restored, original);
    }

    #[test]
    fn test_codex_aliases_registered() {
        let reg = ToolRegistry::new();
        assert!(reg.get("execute_command").is_some());
        assert_eq!(reg.get("execute_command").unwrap().name(), "run_shell_command");

        assert!(reg.get("view_file").is_some());
        assert_eq!(reg.get("view_file").unwrap().name(), "read_file");

        assert!(reg.get("search_files").is_some());
        assert_eq!(reg.get("search_files").unwrap().name(), "grep_search");

        assert!(reg.get("find_files").is_some());
        assert_eq!(reg.get("find_files").unwrap().name(), "glob");

        assert!(reg.get("edit_file").is_some());
        assert_eq!(reg.get("edit_file").unwrap().name(), "edit");
    }
}

