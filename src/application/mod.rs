//! Application workflows, policies, ports, and read-model construction.

pub mod alerts;
pub mod attention;
pub mod clipboard;
pub mod handlers;
pub mod handoff;
pub mod managed_sessions;
pub mod observation;
pub mod provider_runtime;
pub mod session_labels;
pub mod signals;
pub mod successor;
pub mod tasks;

pub use handlers::WorkflowHandlers;
pub use provider_runtime::{ProviderExecutableResolver, ProviderExecutables};

/// Warning context threshold (percent of window).
pub const WARN_PCT: f64 = 70.0;
/// Critical context threshold (percent of window).
pub const CRIT_PCT: f64 = 85.0;
