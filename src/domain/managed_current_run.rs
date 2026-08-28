//! Provider-neutral identity and lifecycle of Managed Current Runs.

use crate::domain::{ManagedControlChannel, ProcessIdentity, Provider};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedCurrentRunRecord {
    pub id: String,
    pub opaque_ref: String,
    pub provider: Provider,
    pub native_session_id: String,
    pub provider_process: ProcessIdentity,
    pub control_channel: ManagedControlChannel,
    pub binding_receipt_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedCurrentRunLifecycle {
    Open,
    Exited,
    Unknown,
}
