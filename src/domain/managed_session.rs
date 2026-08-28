//! Provider-neutral Managed Session state and launch registration records.

use crate::domain::{ProcessIdentity, Provider};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedLaunchStatus {
    Binding,
    NeedsReview,
    Failed,
}

impl ManagedLaunchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::NeedsReview => "needs_review",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "binding" => Some(Self::Binding),
            "needs_review" => Some(Self::NeedsReview),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLaunchTarget {
    pub working_directory: PathBuf,
}

/// Opaque address of the private control channel owned by one Managed launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedControlChannel {
    pub namespace: String,
    pub run_target: String,
}

impl ManagedControlChannel {
    pub fn is_valid(&self) -> bool {
        valid_component(&self.namespace) && valid_component(&self.run_target)
    }
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLaunchRegistration {
    pub id: String,
    pub status: ManagedLaunchStatus,
    pub provider: Provider,
    pub target: ManagedLaunchTarget,
    pub session_alias: String,
    pub description: Option<String>,
    pub binding_initializer: bool,
    pub control_channel: ManagedControlChannel,
    pub created_at_ms: i64,
    pub launched_at_ms: Option<i64>,
    pub launch_evidence: Option<ManagedLaunchEvidence>,
    pub terminal_opened_at_ms: Option<i64>,
    pub terminal_error: Option<String>,
    pub binding_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedLaunchAction {
    CancelStart,
    OpenTerminal,
    RetryOpenTerminal,
    CopyDiagnostics,
    Resume,
    RetryBinding,
    AdoptAsManaged,
}

impl ManagedLaunchRegistration {
    pub fn available_actions(&self) -> Vec<ManagedLaunchAction> {
        match (self.status, self.terminal_error.is_some()) {
            (ManagedLaunchStatus::Binding, false) => vec![ManagedLaunchAction::CancelStart],
            (ManagedLaunchStatus::Binding, true) => vec![
                ManagedLaunchAction::CancelStart,
                ManagedLaunchAction::RetryOpenTerminal,
            ],
            (ManagedLaunchStatus::NeedsReview, false) => vec![
                ManagedLaunchAction::OpenTerminal,
                ManagedLaunchAction::CopyDiagnostics,
            ],
            (ManagedLaunchStatus::NeedsReview, true) => vec![
                ManagedLaunchAction::RetryOpenTerminal,
                ManagedLaunchAction::CopyDiagnostics,
            ],
            (ManagedLaunchStatus::Failed, _) => Vec::new(),
        }
    }

    pub fn allows(&self, action: ManagedLaunchAction) -> bool {
        self.available_actions().contains(&action)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLaunchEvidence {
    /// Opaque adapter-owned identity of the private launch channel.
    pub channel_id: String,
    /// Root process reported by the control channel. It may be a Provider wrapper.
    pub provider_process: ProcessIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSessionRecord {
    pub provider: Provider,
    pub native_session_id: String,
    pub target: ManagedLaunchTarget,
    pub session_alias: String,
    pub description: Option<String>,
    pub binding_receipt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResumeTarget {
    pub provider: Provider,
    pub native_session_id: String,
    pub prior_run_id: String,
}
