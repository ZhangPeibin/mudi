//! Structured, metadata-only projection published by refresh and consumed by the UI.

use crate::application::alerts::AlertCounts;
use crate::application::attention::AttentionSnapshot;
use crate::application::managed_sessions::{
    ManagedLaunchAction, ManagedLaunchRegistration, ManagedLaunchStatus,
    VerifiedExitedManagedSession, VerifiedManagedCurrentRun,
};
use crate::application::signals::RecentSignal;
use crate::application::tasks::OperationTask;
use crate::domain::Provider;
use crate::domain::Session;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Progress reported while (re)indexing.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanStats {
    pub discovered: usize,
    pub parsed: usize,
    pub skipped: usize,
    pub pruned: usize,
}

/// A component-scoped explanation of why the current projection is degraded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshDiagnostic {
    pub component: String,
    pub message: String,
}

impl RefreshDiagnostic {
    pub fn new(component: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            component: component.into(),
            message: message.into(),
        }
    }
}

/// Freshness and persistence health of a [`RefreshSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RefreshHealth {
    #[default]
    Fresh,
    Degraded {
        diagnostics: Vec<RefreshDiagnostic>,
    },
}

impl RefreshHealth {
    pub fn from_diagnostics(diagnostics: Vec<RefreshDiagnostic>) -> Self {
        if diagnostics.is_empty() {
            Self::Fresh
        } else {
            Self::Degraded { diagnostics }
        }
    }

    pub fn is_fresh(&self) -> bool {
        matches!(self, Self::Fresh)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedTerminalState {
    Pending,
    Opened,
    Error,
}

/// Metadata-only recovery view of an incomplete Managed launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLaunchSummary {
    pub id: String,
    pub status: ManagedLaunchStatus,
    pub provider: Provider,
    pub working_directory: PathBuf,
    pub title: String,
    pub created_at_ms: i64,
    pub terminal_state: ManagedTerminalState,
    pub terminal_error: Option<String>,
    pub binding_error: Option<String>,
    pub available_actions: Vec<ManagedLaunchAction>,
}

/// Metadata-only reference to a Managed Current Run revalidated as Open for this snapshot.
/// Private tmux addressing remains behind the application and terminal-adapter boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedCurrentRunSummary {
    pub provider: Provider,
    pub native_session_id: String,
    pub opaque_ref: String,
}

/// Metadata-only reference to a Managed Session whose prior Current Run is verified Exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResumeSummary {
    pub provider: Provider,
    pub native_session_id: String,
}

impl ManagedResumeSummary {
    pub fn is_verified_exited_for(&self, session: &Session) -> bool {
        session.provenance == crate::domain::SessionProvenance::Managed
            && session.state == crate::domain::SessionState::Exited
            && self.provider == session.provider
            && self.native_session_id == session.id
    }
}

impl From<&VerifiedExitedManagedSession> for ManagedResumeSummary {
    fn from(session: &VerifiedExitedManagedSession) -> Self {
        Self {
            provider: session.provider,
            native_session_id: session.native_session_id.clone(),
        }
    }
}

impl From<&ManagedLaunchRegistration> for ManagedLaunchSummary {
    fn from(registration: &ManagedLaunchRegistration) -> Self {
        let terminal_state = if registration.terminal_error.is_some() {
            ManagedTerminalState::Error
        } else if registration.terminal_opened_at_ms.is_some() {
            ManagedTerminalState::Opened
        } else {
            ManagedTerminalState::Pending
        };
        Self {
            id: registration.id.clone(),
            status: registration.status,
            provider: registration.provider,
            working_directory: registration.target.working_directory.clone(),
            title: registration.session_alias.clone(),
            created_at_ms: registration.created_at_ms,
            terminal_state,
            terminal_error: registration.terminal_error.clone(),
            binding_error: registration.binding_error.clone(),
            available_actions: registration.available_actions(),
        }
    }
}

impl ManagedCurrentRunSummary {
    pub fn is_verified_open_for(&self, session: &Session) -> bool {
        session.provenance == crate::domain::SessionProvenance::Managed
            && session.state.is_open()
            && self.provider == session.provider
            && self.native_session_id == session.id
    }
}

impl From<&VerifiedManagedCurrentRun> for ManagedCurrentRunSummary {
    fn from(run: &VerifiedManagedCurrentRun) -> Self {
        Self {
            provider: run.provider,
            native_session_id: run.native_session_id.clone(),
            opaque_ref: run.opaque_ref.clone(),
        }
    }
}

/// The read-only projection of the latest observed local state.
///
/// It intentionally contains only normalized metadata. Provider conversation content,
/// prompt bodies, raw tool output, source-file content, and Handoff Artifact bodies never
/// enter this type.
#[derive(Debug, Clone, Default)]
pub struct RefreshSnapshot {
    pub revision: u64,
    pub generated_at_ms: i64,
    pub health: RefreshHealth,
    pub sessions: Vec<Session>,
    pub tasks: Vec<OperationTask>,
    pub managed_launches: Vec<ManagedLaunchSummary>,
    pub verified_managed_current_runs: Vec<ManagedCurrentRunSummary>,
    pub resumable_managed_sessions: Vec<ManagedResumeSummary>,
    pub recent_signals: Vec<RecentSignal>,
    pub attention: Option<AttentionSnapshot>,
    pub stats: ScanStats,
    pub alerts: AlertCounts,
    /// Compatibility timestamp for existing presentation code. Refresh keeps it equal to
    /// `generated_at_ms`; consumers should prefer that canonical field for new code.
    pub generated_ms: i64,
    /// True once at least one refresh has attempted to produce a projection.
    pub ready: bool,
}

/// The publication boundary between refresh and every read-only consumer.
pub trait ReadModelCache: Send + Sync {
    fn publish(&self, snapshot: RefreshSnapshot);
    fn current(&self) -> Arc<RefreshSnapshot>;
}

/// Thread-safe in-process cache. A different transport can implement [`ReadModelCache`]
/// without changing UI code.
pub struct InMemoryReadModelCache {
    current: RwLock<Arc<RefreshSnapshot>>,
}

impl Default for InMemoryReadModelCache {
    fn default() -> Self {
        Self {
            current: RwLock::new(Arc::new(RefreshSnapshot::default())),
        }
    }
}

impl ReadModelCache for InMemoryReadModelCache {
    fn publish(&self, snapshot: RefreshSnapshot) {
        let mut current = self
            .current
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *current = Arc::new(snapshot);
    }

    fn current(&self) -> Arc<RefreshSnapshot> {
        self.current
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_is_fresh_only_when_no_diagnostic_exists() {
        assert!(RefreshHealth::from_diagnostics(vec![]).is_fresh());
        assert!(
            !RefreshHealth::from_diagnostics(vec![RefreshDiagnostic::new("Index", "unavailable")])
                .is_fresh()
        );
    }
}
