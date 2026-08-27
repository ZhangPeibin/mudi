//! Refresh application service: the sole publisher of the UI read model.

use crate::application::alerts::{self, AlertEngine};
use crate::application::attention::AttentionEngine;
use crate::application::managed_sessions::{
    BindingOutcome, EvidenceBinder, LaunchIdentityAssociationOutcome, ManagedCurrentRunEvidence,
    ManagedCurrentRunVerifier, ManagedLaunchRegistration, ManagedResumeEvidence,
    ManagedResumeVerifier, ManagedSessionRepository, ProviderBindingEvidence,
};
use crate::application::observation::read_model::{
    ManagedCurrentRunSummary, ManagedLaunchSummary, ManagedResumeSummary, ReadModelCache,
    RefreshDiagnostic, RefreshHealth, RefreshSnapshot, ScanStats,
};
use crate::application::session_labels::SessionMetadataRepository;
use crate::application::signals::RecentSignalsEngine;
use crate::application::tasks::TaskRepository;
use crate::domain::{Session, SessionIdentity};
use std::collections::HashSet;

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct ObservationBatch {
    pub stats: ScanStats,
    pub sessions: Vec<Session>,
    pub diagnostics: Vec<RefreshDiagnostic>,
}

pub trait SessionObservationSource: Send {
    fn refresh(&mut self) -> ObservationBatch;
}

pub trait ManagedBindingEvidenceSource: Send + Sync {
    fn evidence(&self, registration: &ManagedLaunchRegistration)
        -> Option<ProviderBindingEvidence>;
}

pub struct RefreshPorts {
    pub observations: Box<dyn SessionObservationSource>,
    pub task_store: std::result::Result<Box<dyn TaskRepository>, String>,
    pub managed_store: std::result::Result<Box<dyn ManagedSessionRepository>, String>,
    pub session_metadata: std::result::Result<Box<dyn SessionMetadataRepository>, String>,
    pub binding_evidence: Box<dyn ManagedBindingEvidenceSource>,
    pub managed_binding_reconciler: EvidenceBinder,
    pub managed_current_run_verifier: ManagedCurrentRunVerifier,
    pub managed_resume_verifier: ManagedResumeVerifier,
    pub alert_engine: AlertEngine,
    pub clock: Box<dyn Clock>,
}

/// Owns reconciliation, derived signals, and read-model publication.
pub struct RefreshService {
    observations: Box<dyn SessionObservationSource>,
    task_store: std::result::Result<Box<dyn TaskRepository>, String>,
    managed_store: std::result::Result<Box<dyn ManagedSessionRepository>, String>,
    session_metadata: std::result::Result<Box<dyn SessionMetadataRepository>, String>,
    binding_evidence: Box<dyn ManagedBindingEvidenceSource>,
    restart_binding_registrations: HashSet<String>,
    managed_binding_reconciler: EvidenceBinder,
    managed_current_run_verifier: ManagedCurrentRunVerifier,
    managed_resume_verifier: ManagedResumeVerifier,
    alert_engine: AlertEngine,
    clock: Box<dyn Clock>,
    attention_engine: AttentionEngine,
    recent_signals_engine: RecentSignalsEngine,
    revision: u64,
}

impl RefreshService {
    pub fn new(ports: RefreshPorts) -> Self {
        let restart_binding_registrations = ports
            .managed_store
            .as_ref()
            .ok()
            .and_then(|store| store.binding_registrations().ok())
            .unwrap_or_default()
            .into_iter()
            .map(|registration| registration.id)
            .collect();
        Self {
            observations: ports.observations,
            task_store: ports.task_store,
            managed_store: ports.managed_store,
            session_metadata: ports.session_metadata,
            binding_evidence: ports.binding_evidence,
            restart_binding_registrations,
            managed_binding_reconciler: ports.managed_binding_reconciler,
            managed_current_run_verifier: ports.managed_current_run_verifier,
            managed_resume_verifier: ports.managed_resume_verifier,
            alert_engine: ports.alert_engine,
            clock: ports.clock,
            attention_engine: AttentionEngine::default(),
            recent_signals_engine: RecentSignalsEngine::default(),
            revision: 0,
        }
    }

    pub fn with_managed_current_run_evidence(
        mut self,
        evidence: Box<dyn ManagedCurrentRunEvidence>,
    ) -> Self {
        self.managed_current_run_verifier = ManagedCurrentRunVerifier::new(evidence);
        self
    }

    pub fn with_managed_resume_evidence(
        mut self,
        evidence: Box<dyn ManagedResumeEvidence>,
    ) -> Self {
        self.managed_resume_verifier = ManagedResumeVerifier::new(evidence);
        self
    }

    pub fn with_managed_binding_reconciler(mut self, reconciler: EvidenceBinder) -> Self {
        self.managed_binding_reconciler = reconciler;
        self
    }

    /// Build one snapshot even when a persistence dependency is unavailable.
    pub fn refresh(&mut self) -> RefreshSnapshot {
        self.revision = self.revision.saturating_add(1);
        let now = self.clock.now_ms();
        let observation = self.observations.refresh();
        let mut diagnostics = observation.diagnostics;
        let stats = observation.stats;
        let mut sessions = observation.sessions;
        let mut managed_launches = Vec::new();
        let mut verified_managed_current_runs = Vec::new();
        let mut resumable_managed_sessions = Vec::new();

        match self.managed_store.as_mut() {
            Ok(store) => {
                match self.session_metadata.as_mut() {
                    Ok(labels) => match store.binding_registrations() {
                        Ok(registrations) => {
                            for registration in registrations {
                                let evidence = self.binding_evidence.evidence(&registration);
                                let outcome = if self
                                    .restart_binding_registrations
                                    .contains(&registration.id)
                                {
                                    self.managed_binding_reconciler
                                        .reconcile_after_restart_with_identity_association(
                                            store.as_mut(),
                                            &registration.id,
                                            evidence,
                                            now,
                                            |store, launch, session| {
                                                associate_launch_title(
                                                    store,
                                                    labels.as_mut(),
                                                    launch,
                                                    session,
                                                )
                                            },
                                        )
                                } else {
                                    self.managed_binding_reconciler
                                        .bind_with_identity_association(
                                            store.as_mut(),
                                            &registration.id,
                                            evidence,
                                            now,
                                            |store, launch, session| {
                                                associate_launch_title(
                                                    store,
                                                    labels.as_mut(),
                                                    launch,
                                                    session,
                                                )
                                            },
                                        )
                                };
                                match outcome {
                                    Ok(BindingOutcome::Bound(_))
                                    | Ok(BindingOutcome::NeedsReview)
                                    | Ok(BindingOutcome::Failed) => {
                                        self.restart_binding_registrations.remove(&registration.id);
                                    }
                                    Ok(BindingOutcome::Waiting) => {}
                                    Err(error) => diagnostics.push(RefreshDiagnostic::new(
                                        "Managed Binding",
                                        error.to_string(),
                                    )),
                                }
                            }
                        }
                        Err(error) => diagnostics.push(RefreshDiagnostic::new(
                            "Managed Registration Store",
                            error.to_string(),
                        )),
                    },
                    Err(error) => {
                        diagnostics.push(RefreshDiagnostic::new("Session Metadata", error.clone()))
                    }
                }

                if let Ok(labels) = self.session_metadata.as_mut() {
                    if let Err(error) = sync_managed_metadata(store.as_ref(), labels.as_mut()) {
                        diagnostics.push(RefreshDiagnostic::new(
                            "Managed Session Metadata",
                            error.to_string(),
                        ));
                    }
                }
                if let Err(error) = store.apply_provenance(&mut sessions) {
                    diagnostics.push(RefreshDiagnostic::new(
                        "Managed Session Registry",
                        error.to_string(),
                    ));
                }
                match self
                    .managed_current_run_verifier
                    .verified_runs(store.as_ref())
                {
                    Ok(runs) => {
                        verified_managed_current_runs =
                            runs.iter().map(ManagedCurrentRunSummary::from).collect();
                    }
                    Err(error) => diagnostics.push(RefreshDiagnostic::new(
                        "Managed Current Run Registry",
                        error.to_string(),
                    )),
                }
                match self
                    .managed_resume_verifier
                    .verified_exited_sessions(store.as_ref())
                {
                    Ok(resumable) => {
                        resumable_managed_sessions =
                            resumable.iter().map(ManagedResumeSummary::from).collect();
                    }
                    Err(error) => diagnostics.push(RefreshDiagnostic::new(
                        "Managed Resume Registry",
                        error.to_string(),
                    )),
                }
                match store.registrations() {
                    Ok(registrations) => {
                        managed_launches = registrations
                            .iter()
                            .map(ManagedLaunchSummary::from)
                            .collect();
                    }
                    Err(error) => diagnostics.push(RefreshDiagnostic::new(
                        "Managed Registration Store",
                        error.to_string(),
                    )),
                }
            }
            Err(error) => diagnostics.push(RefreshDiagnostic::new(
                "Managed Session Registry",
                error.clone(),
            )),
        }

        sort_for_display(&mut sessions);
        self.alert_engine.update(&sessions);
        let alerts = alerts::counts(&sessions);
        let attention = self.attention_engine.update(&sessions, now);
        let recent_signals = self.recent_signals_engine.update(&sessions, now).to_vec();
        let tasks = match self.task_store.as_ref() {
            Ok(store) => match store.list() {
                Ok(tasks) => tasks,
                Err(error) => {
                    diagnostics.push(RefreshDiagnostic::new("Task Store", error.to_string()));
                    Vec::new()
                }
            },
            Err(error) => {
                diagnostics.push(RefreshDiagnostic::new("Task Store", error.clone()));
                Vec::new()
            }
        };
        let health = RefreshHealth::from_diagnostics(diagnostics);
        RefreshSnapshot {
            revision: self.revision,
            generated_at_ms: now,
            health,
            sessions,
            tasks,
            managed_launches,
            verified_managed_current_runs,
            resumable_managed_sessions,
            recent_signals,
            attention: Some(attention),
            stats,
            alerts,
            generated_ms: now,
            ready: true,
        }
    }

    pub fn refresh_and_publish(&mut self, cache: &dyn ReadModelCache) {
        cache.publish(self.refresh());
    }
}

fn associate_launch_title(
    store: &mut dyn ManagedSessionRepository,
    labels: &mut dyn SessionMetadataRepository,
    launch: &ManagedLaunchRegistration,
    session: &SessionIdentity,
) -> anyhow::Result<LaunchIdentityAssociationOutcome> {
    if store.managed_resume_target(&launch.id)?.is_some() {
        return Ok(LaunchIdentityAssociationOutcome::ContinueBinding);
    }
    if let Some(associated) = store.launch_title_association(&launch.id)? {
        if associated == *session {
            return Ok(LaunchIdentityAssociationOutcome::ContinueBinding);
        }
        store.mark_needs_review(
            &launch.id,
            "Provider evidence disagrees with the Session identity already associated with the Title",
        )?;
        return Ok(LaunchIdentityAssociationOutcome::NeedsReview);
    }
    labels.associate_alias(session.provider, &session.native_id, &launch.session_alias)?;
    store.record_launch_title_association(&launch.id, session)?;
    Ok(LaunchIdentityAssociationOutcome::ContinueBinding)
}

fn sync_managed_metadata(
    store: &dyn ManagedSessionRepository,
    labels: &mut dyn SessionMetadataRepository,
) -> anyhow::Result<()> {
    labels.reload();
    if let Some(error) = labels.error() {
        anyhow::bail!(error.to_string());
    }
    for session in store.managed_sessions()? {
        if store.managed_metadata_is_synchronized(session.provider, &session.native_session_id)? {
            continue;
        }
        let description = session.description.as_deref().unwrap_or_default();
        if labels
            .alias(session.provider, &session.native_session_id)
            .is_none()
        {
            labels.associate_alias(
                session.provider,
                &session.native_session_id,
                &session.session_alias,
            )?;
        }
        if labels.description(session.provider, &session.native_session_id)
            != session.description.as_deref()
        {
            labels.set_description(session.provider, &session.native_session_id, description)?;
        }
        store.mark_managed_metadata_synchronized(session.provider, &session.native_session_id)?;
    }
    Ok(())
}

fn sort_for_display(sessions: &mut [Session]) {
    sessions.sort_by(|a, b| {
        a.state
            .sort_rank()
            .cmp(&b.state.sort_rank())
            .then(b.last_activity_ms.cmp(&a.last_activity_ms))
    });
}
