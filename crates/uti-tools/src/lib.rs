pub mod apply_patch;
pub mod background;
pub mod fs_tools;
pub mod grep;
pub mod registry;
pub mod shell;
pub mod types;
pub mod web;
pub mod extra_tools;
pub mod mcp;

pub use apply_patch::ApplyPatchTool;
pub use background::{get_task_manager, ManageTaskTool, TaskManager};
pub use fs_tools::{EditTool, GlobTool, LsTool, ReadFileTool, WriteFileTool};
pub use grep::GrepTool;
pub use mcp::{load_mcp_servers, McpClient, McpServerStatus, McpTool};
pub use registry::ToolRegistry;
pub use shell::{command_requires_sudo, extract_first_sudo_command, ShellTool};
pub use types::{Tool, ToolContext, ToolOutput};
pub use web::{WebFetchTool, WebSearchTool};
pub use extra_tools::{
    ReadManyFilesTool, AskUserTool, WriteTodosTool, ActivateSkillTool,
    GetInternalDocsTool, TrackerCreateTaskTool, TrackerUpdateTaskTool,
    TrackerGetTaskTool, TrackerListTasksTool, TrackerAddDependencyTool,
    TrackerVisualizeTool, UpdateTopicTool,
};

