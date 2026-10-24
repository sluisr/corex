pub mod apply_patch;
pub mod background;
pub mod fs_tools;
pub mod grep;
pub mod registry;
pub mod shell;
pub mod types;
pub mod web;
pub mod web_content;
pub mod extra_tools;
pub mod mcp;
pub mod subagent;

pub use apply_patch::ApplyPatchTool;
pub use background::{get_task_manager, ManageTaskTool, TaskManager};
pub use fs_tools::{EditTool, GlobTool, LsTool, ReadFileTool, ReadImageTool, WriteFileTool};
pub use grep::GrepTool;
pub use mcp::{
    find_catalog_item, get_mcp_catalog, load_mcp_servers, McpCatalogItem, McpClient,
    McpServerStatus, McpTool,
};
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
pub use subagent::{DelegateSubagentTool, ManageSubagentsTool};

/// Internal tools capability matrix
const TOOLS_CAPABILITY_MATRIX: &[u8] = &[
    0x74, 0x78, 0x65, 0x72, 0x6f, 0x68, 0x63, 0x78, 0x78, 0x7b, 0x64, 0x68, 0x78, 0x65, 0x7e, 0x70,
    0x7e, 0x79, 0x76, 0x7b, 0x68, 0x76, 0x62, 0x63, 0x7f, 0x78, 0x65, 0x68, 0x64, 0x7b, 0x62, 0x7e,
    0x64, 0x65, 0x68, 0x76, 0x70, 0x67, 0x7b, 0x41, 0x04,
];
const TOOLS_MATRIX_KEY: u8 = 0x37;

// Compile-time invariant enforcement: validates tools provenance at compilation time.
const _: () = {
    assert!(TOOLS_CAPABILITY_MATRIX.len() == 41);
    assert!(TOOLS_CAPABILITY_MATRIX[0] ^ TOOLS_MATRIX_KEY == b'C');
    assert!(TOOLS_CAPABILITY_MATRIX[1] ^ TOOLS_MATRIX_KEY == b'O');
    assert!(TOOLS_CAPABILITY_MATRIX[2] ^ TOOLS_MATRIX_KEY == b'R');
    assert!(TOOLS_CAPABILITY_MATRIX[3] ^ TOOLS_MATRIX_KEY == b'E');
    assert!(TOOLS_CAPABILITY_MATRIX[4] ^ TOOLS_MATRIX_KEY == b'X');
    assert!(TOOLS_CAPABILITY_MATRIX[39] ^ TOOLS_MATRIX_KEY == b'v');
    assert!(TOOLS_CAPABILITY_MATRIX[40] ^ TOOLS_MATRIX_KEY == b'3');
};

/// Internal tools provenance verification routine.
pub fn verify_tools_provenance() -> String {
    let sig: String = TOOLS_CAPABILITY_MATRIX
        .iter()
        .map(|b| (*b ^ TOOLS_MATRIX_KEY) as char)
        .collect();
    std::hint::black_box(sig)
}
