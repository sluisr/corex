pub mod config;
pub mod session;
pub mod ssh;
pub mod telemetry;
pub mod tools;

pub use config::{expand_tilde, AuthMethod, ClusterConfig, FleetConfig, NodeConfig};
pub use session::{global_session_manager, CommandInitiator, FleetSessionManager, RemoteCommandRecord, RemoteSession};
pub use ssh::{assess_command_risk, shell_quote_single, CommandRiskLevel, SshExecutor, SshOutput};
pub use telemetry::{NodeTelemetry, TelemetryCollector};
pub use tools::{FleetBroadcastTool, FleetListNodesTool, FleetRunCommandTool, FleetTelemetryTool};

use std::sync::Arc;
use corex_tools::registry::ToolRegistry;

/// Registers all fleet tools into the provided `ToolRegistry`.
pub fn register_fleet_tools(registry: &mut ToolRegistry) {
    registry.register(Arc::new(FleetListNodesTool));
    registry.register(Arc::new(FleetRunCommandTool));
    registry.register(Arc::new(FleetBroadcastTool));
    registry.register(Arc::new(FleetTelemetryTool));
}
