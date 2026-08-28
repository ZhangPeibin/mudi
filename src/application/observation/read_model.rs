//! Structured, metadata-only projection published by refresh and consumed by the UI.

use crate::application::alerts::AlertCounts;
use crate::application::attention::AttentionSnapshot;
use crate::application::managed_session::{
    ManagedLaunchAction, ManagedLaunchRegistration, ManagedLaunchStatus,
    VerifiedExitedManagedSession, VerifiedManagedCurrentRun,
};
use crate::application::session_labels::{
    resolve_titles_with_alias, short_session_id, CurationDisposition, SessionMetadataQuery,
};
use crate::application::signals::RecentSignal;
use crate::application::tasks::OperationTask;
use crate::domain::{Provider, Session, SessionIdentity};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Progress reported while (re)indexing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub discovered: usize,
    pub parsed: usize,
    pub skipped: usize,
    pub pruned: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RefreshStage {
    Collect,
    Reconcile,
    #[default]
    Project,
    Publish,
}

/// A component-scoped explanation of why the current projection is degraded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshDiagnostic {
    pub stage: RefreshStage,
    pub component: String,
    pub message: String,
}

impl RefreshDiagnostic {
    pub fn new(component: impl Into<String>, message: impl Into<String>) -> Self {
        Self::for_stage(RefreshStage::Project, component, message)
    }

    pub fn for_stage(
        stage: RefreshStage,
        component: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            stage,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMetadataAction {
    Edit,
    Hide,
    Restore,
    PermanentlyExclude,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadataSummary {
    pub identity: SessionIdentity,
    pub alias: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub disposition: Option<CurationDisposition>,
    pub available_actions: Vec<SessionMetadataAction>,
    pub normally_visible: bool,
    pub hidden_inventory: bool,
}

/// Complete metadata and curation projection consumed by presentation.
#[derive(Debug, Clone, Default)]
pub struct SessionMetadataSnapshot {
    entries: HashMap<SessionIdentity, SessionMetadataSummary>,
    pub error: Option<String>,
}

impl SessionMetadataSnapshot {
    pub(crate) fn unavailable(error: impl Into<String>) -> Self {
        Self {
            entries: HashMap::new(),
            error: Some(error.into()),
        }
    }

    /// Project provider-neutral Session metadata for one refresh boundary.
    pub fn project(
        metadata: &dyn SessionMetadataQuery,
        identities: impl IntoIterator<Item = SessionIdentity>,
        sessions: &[Session],
    ) -> Self {
        let error = metadata.error().map(str::to_string);
        let editable = metadata.is_editable();
        let observed = sessions
            .iter()
            .map(|session| (SessionIdentity::from(session), session))
            .collect::<HashMap<_, _>>();
        let mut entries = HashMap::new();
        for identity in identities {
            if entries.contains_key(&identity) {
                continue;
            }
            let disposition = metadata.disposition(identity.provider, &identity.native_id);
            let mut available_actions = Vec::new();
            if editable && disposition != Some(CurationDisposition::PermanentlyExcluded) {
                available_actions.push(SessionMetadataAction::Edit);
            }
            if editable
                && disposition.is_none()
                && observed
                    .get(&identity)
                    .is_some_and(|session| !session.state.is_open())
            {
                available_actions.push(SessionMetadataAction::Hide);
            }
            if editable && disposition == Some(CurationDisposition::Hidden) {
                available_actions.extend([
                    SessionMetadataAction::Restore,
                    SessionMetadataAction::PermanentlyExclude,
                ]);
            }
            let alias = metadata
                .alias(identity.provider, &identity.native_id)
                .map(str::to_string);
            let title = alias
                .clone()
                .unwrap_or_else(|| short_session_id(&identity.native_id));
            let normally_visible = match disposition {
                Some(CurationDisposition::Hidden) => observed
                    .get(&identity)
                    .is_some_and(|session| session.state.is_open()),
                Some(CurationDisposition::PermanentlyExcluded) => false,
                None => true,
            };
            let hidden_inventory = disposition == Some(CurationDisposition::Hidden)
                && observed
                    .get(&identity)
                    .is_some_and(|session| !session.state.is_open());
            entries.insert(
                identity.clone(),
                SessionMetadataSummary {
                    identity: identity.clone(),
                    alias,
                    title,
                    description: metadata
                        .description(identity.provider, &identity.native_id)
                        .map(str::to_string),
                    disposition,
                    available_actions,
                    normally_visible,
                    hidden_inventory,
                },
            );
        }
        Self { entries, error }
    }

    #[cfg(test)]
    pub(crate) fn from_summaries(
        summaries: impl IntoIterator<Item = SessionMetadataSummary>,
    ) -> Self {
        Self {
            entries: summaries
                .into_iter()
                .map(|summary| (summary.identity.clone(), summary))
                .collect(),
            error: None,
        }
    }

    pub fn is_editable(&self) -> bool {
        self.error.is_none()
    }

    pub fn get(&self, provider: Provider, native_id: &str) -> Option<&SessionMetadataSummary> {
        self.entries.get(&SessionIdentity {
            provider,
            native_id: native_id.to_string(),
        })
    }

    pub fn alias(&self, provider: Provider, native_id: &str) -> Option<&str> {
        self.get(provider, native_id)?.alias.as_deref()
    }

    pub fn description(&self, provider: Provider, native_id: &str) -> Option<&str> {
        self.get(provider, native_id)?.description.as_deref()
    }

    pub fn disposition(&self, provider: Provider, native_id: &str) -> Option<CurationDisposition> {
        self.get(provider, native_id)?.disposition
    }

    pub fn title(&self, provider: Provider, native_id: &str) -> String {
        self.get(provider, native_id)
            .map(|entry| entry.title.clone())
            .unwrap_or_else(|| short_session_id(native_id))
    }

    pub fn titles_for(&self, sessions: &[(Provider, &str)]) -> Vec<String> {
        resolve_titles_with_alias(sessions, |provider, native_id| {
            self.alias(provider, native_id)
        })
    }

    pub fn allows(
        &self,
        provider: Provider,
        native_id: &str,
        action: SessionMetadataAction,
    ) -> bool {
        self.get(provider, native_id)
            .is_some_and(|entry| entry.available_actions.contains(&action))
    }

    pub fn allows_for(&self, session: &Session, action: SessionMetadataAction) -> bool {
        self.get(session.provider, &session.id).map_or_else(
            || {
                self.is_editable()
                    && match action {
                        SessionMetadataAction::Edit => true,
                        SessionMetadataAction::Hide => !session.state.is_open(),
                        SessionMetadataAction::Restore
                        | SessionMetadataAction::PermanentlyExclude => false,
                    }
            },
            |entry| entry.available_actions.contains(&action),
        )
    }

    pub fn normally_visible(&self, provider: Provider, native_id: &str) -> bool {
        self.get(provider, native_id)
            .is_none_or(|entry| entry.normally_visible)
    }

    pub fn in_hidden_inventory(&self, provider: Provider, native_id: &str) -> bool {
        self.get(provider, native_id)
            .is_some_and(|entry| entry.hidden_inventory)
    }
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
    pub session_metadata: SessionMetadataSnapshot,
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

impl RefreshSnapshot {
    /// Presentation-ready normal inventory with curation and Open override already applied.
    pub fn normal_inventory(&self) -> Self {
        let mut normal = self.clone();
        let observed_session_count = normal.sessions.len();
        normal.sessions.retain(|session| {
            self.session_metadata
                .normally_visible(session.provider, &session.id)
        });
        let excluded_count = observed_session_count.saturating_sub(normal.sessions.len());
        normal.stats.discovered = normal.stats.discovered.saturating_sub(excluded_count);
        normal.recent_signals.retain(|signal| match signal {
            RecentSignal::Session {
                provider,
                session_id,
                ..
            } => self
                .session_metadata
                .normally_visible(*provider, session_id),
            RecentSignal::Project { .. } => true,
        });
        normal
    }
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
    use crate::application::session_labels::SessionMetadataView;
    use crate::domain::{ContextState, SessionProvenance, SessionState, SourceMetadata, Usage};

    #[test]
    fn health_is_fresh_only_when_no_diagnostic_exists() {
        assert!(RefreshHealth::from_diagnostics(vec![]).is_fresh());
        assert!(
            !RefreshHealth::from_diagnostics(vec![RefreshDiagnostic::new("Index", "unavailable")])
                .is_fresh()
        );
    }

    #[test]
    fn metadata_projection_owns_fields_inventory_and_action_eligibility() {
        let exited = session(Provider::ClaudeCode, "exited-session", SessionState::Exited);
        let open = session(Provider::ClaudeCode, "open-session", SessionState::Busy);
        let editable = session(Provider::CodexCli, "editable-session", SessionState::Exited);
        let mut metadata = SessionMetadataView::default();
        metadata.set_metadata(
            SessionIdentity::from(&exited),
            "Release coordination",
            "Tracks the release boundary",
        );
        metadata.set_disposition(SessionIdentity::from(&exited), CurationDisposition::Hidden);
        let sessions = vec![exited.clone(), open.clone(), editable.clone()];

        let projected = SessionMetadataSnapshot::project(
            &metadata,
            sessions.iter().map(SessionIdentity::from),
            &sessions,
        );

        let hidden = projected
            .get(exited.provider, &exited.id)
            .expect("hidden Session should be projected");
        assert_eq!(hidden.title, "Release coordination");
        assert_eq!(
            hidden.description.as_deref(),
            Some("Tracks the release boundary")
        );
        assert_eq!(hidden.disposition, Some(CurationDisposition::Hidden));
        assert!(!hidden.normally_visible);
        assert!(hidden.hidden_inventory);
        assert_eq!(
            hidden.available_actions,
            vec![
                SessionMetadataAction::Edit,
                SessionMetadataAction::Restore,
                SessionMetadataAction::PermanentlyExclude,
            ]
        );
        assert!(!projected.allows_for(&open, SessionMetadataAction::Hide));
        assert!(projected.allows_for(&editable, SessionMetadataAction::Edit));
        assert!(projected.allows_for(&editable, SessionMetadataAction::Hide));
    }

    #[test]
    fn metadata_projection_keeps_provider_scoped_identity_isolated() {
        let claude = session(Provider::ClaudeCode, "shared-id", SessionState::Exited);
        let codex = session(Provider::CodexCli, "shared-id", SessionState::Exited);
        let mut metadata = SessionMetadataView::default();
        metadata.set_metadata(SessionIdentity::from(&claude), "Claude title", "");
        let sessions = vec![claude.clone(), codex.clone()];

        let projected = SessionMetadataSnapshot::project(
            &metadata,
            sessions.iter().map(SessionIdentity::from),
            &sessions,
        );

        assert_eq!(projected.title(claude.provider, &claude.id), "Claude title");
        assert_eq!(projected.alias(codex.provider, &codex.id), None);
    }

    #[test]
    fn normal_inventory_applies_hidden_open_override_and_signal_visibility() {
        let hidden = session(Provider::ClaudeCode, "hidden", SessionState::Exited);
        let open = session(Provider::CodexCli, "open", SessionState::Busy);
        let mut metadata = SessionMetadataView::default();
        metadata.set_disposition(SessionIdentity::from(&hidden), CurationDisposition::Hidden);
        metadata.set_disposition(SessionIdentity::from(&open), CurationDisposition::Hidden);
        let sessions = vec![hidden.clone(), open.clone()];
        let session_metadata = SessionMetadataSnapshot::project(
            &metadata,
            sessions.iter().map(SessionIdentity::from),
            &sessions,
        );
        let snapshot = RefreshSnapshot {
            sessions,
            session_metadata,
            recent_signals: vec![
                RecentSignal::Session {
                    observed_at_ms: 1,
                    provider: hidden.provider,
                    session_id: hidden.id.clone(),
                    kind: crate::application::signals::RecentSignalKind::SessionStateChanged {
                        from: SessionState::Busy,
                        to: SessionState::Exited,
                    },
                },
                RecentSignal::Session {
                    observed_at_ms: 2,
                    provider: open.provider,
                    session_id: open.id.clone(),
                    kind: crate::application::signals::RecentSignalKind::SessionStateChanged {
                        from: SessionState::Idle,
                        to: SessionState::Busy,
                    },
                },
                RecentSignal::Project {
                    observed_at_ms: 3,
                    project_path: PathBuf::from("/project"),
                    change: crate::application::signals::SharedProjectRiskChange::Cleared,
                    before: Vec::new(),
                    after: Vec::new(),
                },
            ],
            stats: ScanStats {
                discovered: 2,
                ..ScanStats::default()
            },
            ..RefreshSnapshot::default()
        };

        let normal = snapshot.normal_inventory();

        assert_eq!(normal.sessions.len(), 1);
        assert_eq!(normal.sessions[0].id, open.id);
        assert_eq!(normal.recent_signals.len(), 2);
        assert!(normal.recent_signals.iter().any(|signal| matches!(
            signal,
            RecentSignal::Session { session_id, .. } if session_id == &open.id
        )));
        assert!(normal
            .recent_signals
            .iter()
            .any(|signal| matches!(signal, RecentSignal::Project { .. })));
        assert_eq!(normal.stats.discovered, 1);
    }

    #[test]
    fn degraded_metadata_projection_disables_actions_without_hiding_observation() {
        struct DegradedMetadata;

        impl SessionMetadataQuery for DegradedMetadata {
            fn is_editable(&self) -> bool {
                false
            }

            fn error(&self) -> Option<&str> {
                Some("state database unavailable")
            }

            fn reload(&mut self) {}

            fn alias(&self, _provider: Provider, _native_id: &str) -> Option<&str> {
                None
            }

            fn description(&self, _provider: Provider, _native_id: &str) -> Option<&str> {
                None
            }

            fn disposition(
                &self,
                _provider: Provider,
                _native_id: &str,
            ) -> Option<CurationDisposition> {
                None
            }
        }

        let observed = session(Provider::ClaudeCode, "observed", SessionState::Exited);
        let projected = SessionMetadataSnapshot::project(
            &DegradedMetadata,
            [SessionIdentity::from(&observed)],
            std::slice::from_ref(&observed),
        );

        assert_eq!(
            projected.error.as_deref(),
            Some("state database unavailable")
        );
        assert!(!projected.is_editable());
        assert!(projected.normally_visible(observed.provider, &observed.id));
        assert!(!projected.allows_for(&observed, SessionMetadataAction::Edit));
        assert!(!projected.allows_for(&observed, SessionMetadataAction::Hide));
    }

    fn session(provider: Provider, native_id: &str, state: SessionState) -> Session {
        Session {
            provider,
            id: native_id.to_string(),
            provenance: SessionProvenance::Observed,
            source: SourceMetadata::default(),
            project: None,
            cwd: None,
            latest_model: None,
            state,
            context: ContextState::default(),
            usage: Usage::default(),
            per_model: Vec::new(),
            current_run_usage: None,
            compactions: Vec::new(),
            last_activity_ms: 0,
            cost: None,
        }
    }
}
