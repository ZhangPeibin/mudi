use super::{ObservationFrame, ReconciliationReport};
use crate::application::alerts;
use crate::application::attention::AttentionEngine;
use crate::application::observation::read_model::{
    ManagedLaunchSummary, RefreshDiagnostic, RefreshHealth, RefreshSnapshot, RefreshStage,
    SessionMetadataSnapshot,
};
use crate::application::session_labels::SessionMetadataQuery;
use crate::application::signals::RecentSignalsEngine;
use crate::application::tasks::OperationTask;
use crate::domain::Session;

/// Read-only authoritative facts used by projection.
pub trait ManagedProjectionQuery: Send {
    fn apply_provenance(&self, sessions: &mut [Session]) -> anyhow::Result<()>;
    fn managed_launches(&self) -> anyhow::Result<Vec<ManagedLaunchSummary>>;
}

pub trait OperationTaskQuery: Send {
    fn tasks(&self) -> anyhow::Result<Vec<OperationTask>>;
}

pub struct ProjectPorts {
    pub task_query: std::result::Result<Box<dyn OperationTaskQuery>, String>,
    pub managed_query: std::result::Result<Box<dyn ManagedProjectionQuery>, String>,
    pub session_metadata: std::result::Result<Box<dyn SessionMetadataQuery>, String>,
}

/// Builds the complete read model using only the frame and authoritative reads.
pub struct ProjectionService {
    task_query: std::result::Result<Box<dyn OperationTaskQuery>, String>,
    managed_query: std::result::Result<Box<dyn ManagedProjectionQuery>, String>,
    session_metadata: std::result::Result<Box<dyn SessionMetadataQuery>, String>,
    attention_engine: AttentionEngine,
    recent_signals_engine: RecentSignalsEngine,
    revision: u64,
}

impl ProjectionService {
    pub fn new(ports: ProjectPorts) -> Self {
        Self {
            task_query: ports.task_query,
            managed_query: ports.managed_query,
            session_metadata: ports.session_metadata,
            attention_engine: AttentionEngine::default(),
            recent_signals_engine: RecentSignalsEngine::default(),
            revision: 0,
        }
    }

    pub fn project(
        &mut self,
        frame: &ObservationFrame,
        reconciliation: ReconciliationReport,
    ) -> RefreshSnapshot {
        self.revision = self.revision.saturating_add(1);
        let mut diagnostics = frame.diagnostics.clone();
        diagnostics.extend(reconciliation.diagnostics);
        let mut sessions = frame.sessions.clone();
        let mut managed_launches = Vec::new();

        match self.managed_query.as_ref() {
            Ok(store) => {
                if let Err(error) = store.apply_provenance(&mut sessions) {
                    diagnostics.push(RefreshDiagnostic::for_stage(
                        RefreshStage::Project,
                        "Managed Session Registry",
                        error.to_string(),
                    ));
                }
                match store.managed_launches() {
                    Ok(projected) => managed_launches = projected,
                    Err(error) => diagnostics.push(RefreshDiagnostic::for_stage(
                        RefreshStage::Project,
                        "Managed Registration Store",
                        error.to_string(),
                    )),
                }
            }
            Err(error) => diagnostics.push(RefreshDiagnostic::for_stage(
                RefreshStage::Project,
                "Managed Session Registry",
                error.clone(),
            )),
        }

        sort_for_display(&mut sessions);
        let alerts = alerts::counts(&sessions);
        let attention = self
            .attention_engine
            .update(&sessions, frame.observed_at_ms);
        let recent_signals = self
            .recent_signals_engine
            .update(&sessions, frame.observed_at_ms)
            .to_vec();
        let tasks = match self.task_query.as_ref() {
            Ok(store) => match store.tasks() {
                Ok(tasks) => tasks,
                Err(error) => {
                    diagnostics.push(RefreshDiagnostic::for_stage(
                        RefreshStage::Project,
                        "Task Store",
                        error.to_string(),
                    ));
                    Vec::new()
                }
            },
            Err(error) => {
                diagnostics.push(RefreshDiagnostic::for_stage(
                    RefreshStage::Project,
                    "Task Store",
                    error.clone(),
                ));
                Vec::new()
            }
        };

        let identities = sessions
            .iter()
            .map(crate::domain::SessionIdentity::from)
            .chain(tasks.iter().map(|task| crate::domain::SessionIdentity {
                provider: task.source_provider,
                native_id: task.source_session_id.clone(),
            }))
            .chain(recent_signals.iter().filter_map(|signal| match signal {
                crate::application::signals::RecentSignal::Session {
                    provider,
                    session_id,
                    ..
                } => Some(crate::domain::SessionIdentity {
                    provider: *provider,
                    native_id: session_id.clone(),
                }),
                crate::application::signals::RecentSignal::Project { .. } => None,
            }))
            .collect::<Vec<_>>();
        let session_metadata = match self.session_metadata.as_mut() {
            Ok(metadata) => {
                metadata.reload();
                let projected =
                    SessionMetadataSnapshot::project(metadata.as_ref(), identities, &sessions);
                if let Some(error) = projected.error.as_ref() {
                    diagnostics.push(RefreshDiagnostic::for_stage(
                        RefreshStage::Project,
                        "Session Metadata Store",
                        error.clone(),
                    ));
                }
                projected
            }
            Err(error) => {
                diagnostics.push(RefreshDiagnostic::for_stage(
                    RefreshStage::Project,
                    "Session Metadata Store",
                    error.clone(),
                ));
                SessionMetadataSnapshot::unavailable(error.clone())
            }
        };

        RefreshSnapshot {
            revision: self.revision,
            generated_at_ms: frame.observed_at_ms,
            health: RefreshHealth::from_diagnostics(diagnostics),
            sessions,
            tasks,
            managed_launches,
            verified_managed_current_runs: frame.verified_managed_current_runs.clone(),
            resumable_managed_sessions: frame.resumable_managed_sessions.clone(),
            session_metadata,
            recent_signals,
            attention: Some(attention),
            stats: frame.stats,
            alerts,
            generated_ms: frame.observed_at_ms,
            ready: true,
        }
    }
}

fn sort_for_display(sessions: &mut [Session]) {
    sessions.sort_by(|a, b| {
        a.state
            .sort_rank()
            .cmp(&b.state.sort_rank())
            .then(b.last_activity_ms.cmp(&a.last_activity_ms))
    });
}
