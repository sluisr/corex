pub mod config;
pub mod session;
pub mod ssh;
pub mod telemetry;
pub mod tools;

pub use config::{expand_tilde, AuthMethod, ClusterConfig, FleetConfig, NodeConfig, NodeSource};
pub use session::{global_session_manager, CommandInitiator, FleetSessionManager, RemoteCommandRecord, RemoteSession};
pub use ssh::{assess_command_risk, shell_quote_single, CommandRiskLevel, SshExecutor, SshOutput};
pub use telemetry::{NodeTelemetry, TelemetryCollector};
pub use tools::{FleetBroadcastTool, FleetListNodesTool, FleetRunCommandTool, FleetTelemetryTool};

use std::sync::Arc;
use corex_tools::grouped::{ActionGroupTool, FLEET_ACTIONS};
use corex_tools::registry::ToolRegistry;

pub fn register_fleet_tools(registry: &mut ToolRegistry) {
    registry.register_group(ActionGroupTool::new(
        "fleet",
        "Manage the user's remote servers (VPS / dedicated nodes over SSH).",
        vec![
            (FLEET_ACTIONS[0].0, Arc::new(FleetListNodesTool)),
            (FLEET_ACTIONS[1].0, Arc::new(FleetTelemetryTool)),
            (FLEET_ACTIONS[2].0, Arc::new(FleetRunCommandTool)),
            (FLEET_ACTIONS[3].0, Arc::new(FleetBroadcastTool)),
        ],
    ));
}
