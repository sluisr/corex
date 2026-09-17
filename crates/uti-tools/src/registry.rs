use std::collections::HashMap;
use std::sync::Arc;
use anyhow::Result;
use uti_core::types::ToolDefinition;

use crate::apply_patch::ApplyPatchTool;
use crate::background::{
    KillBackgroundProcessTool, ListBackgroundProcessesTool, ManageTaskTool, ReadBackgroundOutputTool,
    WriteBackgroundInputTool,
};
use crate::fs_tools::{EditTool, GlobTool, LsTool, ReadFileTool, WriteFileTool};
use crate::grep::GrepTool;
use crate::shell::ShellTool;
use crate::types::{Tool, ToolContext, ToolOutput};
use crate::web::{WebFetchTool, WebSearchTool};
use crate::extra_tools::{
    ReadManyFilesTool, AskUserTool, WriteTodosTool, ActivateSkillTool,
    GetInternalDocsTool, TrackerCreateTaskTool, TrackerUpdateTaskTool,
    TrackerGetTaskTool, TrackerListTasksTool, TrackerAddDependencyTool,
    TrackerVisualizeTool, UpdateTopicTool,
};

#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    registered_definitions: Vec<ToolDefinition>,
}

impl ToolRegistry {
    pub fn new() -> Self {
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
        let google_web_search = Arc::new(WebSearchTool);
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

        // Register official primary tool definitions
        registry.register_primary(apply_patch.clone());
        registry.register_primary(read_file.clone());
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
        registry.register_primary(google_web_search.clone());
        registry.register_primary(web_fetch.clone());

        registry.register_primary(read_many_files);
        registry.register_primary(ask_user);
        registry.register_primary(write_todos);
        registry.register_primary(activate_skill);
        registry.register_primary(get_internal_docs);
        registry.register_primary(tracker_create_task);
        registry.register_primary(tracker_update_task);
        registry.register_primary(tracker_get_task);
        registry.register_primary(tracker_list_tasks);
        registry.register_primary(tracker_add_dependency);
        registry.register_primary(tracker_visualize);
        registry.register_primary(update_topic);

        // Dynamic 1:1 aliases so DeepSeek / Codex / Gemini can invoke either canonical name or alias:
        registry.tools.insert("replace".to_string(), edit.clone());
        registry.tools.insert("edit_file".to_string(), edit);
        registry.tools.insert("grep".to_string(), grep_search.clone());
        registry.tools.insert("search_files".to_string(), grep_search);
        registry.tools.insert("web_search".to_string(), google_web_search);
        registry.tools.insert("ls".to_string(), list_directory.clone());
        registry.tools.insert("read_dir".to_string(), list_directory);
        registry.tools.insert("run_command".to_string(), run_shell_command.clone());
        registry.tools.insert("execute_command".to_string(), run_shell_command.clone());
        registry.tools.insert("bash".to_string(), run_shell_command);
        registry.tools.insert("view_file".to_string(), read_file);
        registry.tools.insert("find_files".to_string(), glob.clone());
        registry.tools.insert("tasks".to_string(), manage_task);

        registry
    }

    fn register_primary(&mut self, tool: Arc<dyn Tool>) {
        self.registered_definitions.push(tool.to_definition());
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.register_primary(tool);
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
        match self.get(name) {
            Some(tool) => tool.execute(args, context).await,
            None => Ok(ToolOutput::error(format!("Unknown tool: {}", name))),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

