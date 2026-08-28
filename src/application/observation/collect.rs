use super::frame::{ManagedBindingObservation, ObservationFrame};
use crate::application::managed_session::{
    BindingProcessObservation, ManagedCurrentRunVerifier, ManagedLaunchRegistration,
    ManagedResumeVerifier, ProcessEvidence, ProviderBindingEvidence, ProviderProcessObservation,
};
use crate::application::observation::read_model::{
    ManagedCurrentRunSummary, ManagedResumeSummary, RefreshDiagnostic, RefreshStage, ScanStats,
};
use crate::domain::Session;
use std::sync::Arc;

pub struct SessionObservationBatch {
    pub stats: ScanStats,
    pub sessions: Vec<Session>,
    pub diagnostics: Vec<RefreshDiagnostic>,
}

/// Disposable-index and Provider-source collection boundary.
pub trait SessionObservationSource: Send {
    fn collect(&mut self) -> SessionObservationBatch;
}

/// Provider-specific evidence lookup used only for already-authorized registrations.
pub trait ManagedBindingEvidenceSource: Send + Sync {
    fn evidence(&self, registration: &ManagedLaunchRegistration)
        -> Option<ProviderBindingEvidence>;
}

/// Read-only authoritative records required to collect control evidence.
pub trait ManagedObservationQuery: Send {
    fn binding_registrations(&self) -> anyhow::Result<Vec<ManagedLaunchRegistration>>;
    fn verified_managed_current_runs(
        &self,
        verifier: &ManagedCurrentRunVerifier,
    ) -> anyhow::Result<Vec<ManagedCurrentRunSummary>>;
    fn resumable_managed_sessions(
        &self,
        verifier: &ManagedResumeVerifier,
    ) -> anyhow::Result<Vec<ManagedResumeSummary>>;
}

pub struct CollectPorts {
    pub observations: Box<dyn SessionObservationSource>,
    pub managed_query: std::result::Result<Box<dyn ManagedObservationQuery>, String>,
    pub binding_evidence: Box<dyn ManagedBindingEvidenceSource>,
    pub process_evidence: Arc<dyn ProcessEvidence>,
    pub managed_current_run_verifier: ManagedCurrentRunVerifier,
    pub managed_resume_verifier: ManagedResumeVerifier,
}

/// Reads external facts and authoritative records without mutating authoritative state.
pub struct CollectService {
    observations: Box<dyn SessionObservationSource>,
    managed_query: std::result::Result<Box<dyn ManagedObservationQuery>, String>,
    binding_evidence: Box<dyn ManagedBindingEvidenceSource>,
    process_evidence: Arc<dyn ProcessEvidence>,
    managed_current_run_verifier: ManagedCurrentRunVerifier,
    managed_resume_verifier: ManagedResumeVerifier,
}

impl CollectService {
    pub fn new(ports: CollectPorts) -> Self {
        Self {
            observations: ports.observations,
            managed_query: ports.managed_query,
            binding_evidence: ports.binding_evidence,
            process_evidence: ports.process_evidence,
            managed_current_run_verifier: ports.managed_current_run_verifier,
            managed_resume_verifier: ports.managed_resume_verifier,
        }
    }

    pub fn collect(&mut self, observed_at_ms: i64) -> ObservationFrame {
        let mut frame = ObservationFrame::new(observed_at_ms, self.observations.collect());
        for diagnostic in &mut frame.diagnostics {
            diagnostic.stage = RefreshStage::Collect;
        }

        match self.managed_query.as_ref() {
            Ok(store) => {
                match store.binding_registrations() {
                    Ok(registrations) => {
                        let pending = registrations
                            .into_iter()
                            .map(|registration| {
                                let evidence = self.binding_evidence.evidence(&registration);
                                (registration, evidence)
                            })
                            .collect::<Vec<_>>();
                        let process_ids = pending
                            .iter()
                            .flat_map(|(registration, evidence)| {
                                let launch_pid = registration
                                    .launch_evidence
                                    .as_ref()
                                    .map_or(0, |launch| launch.provider_process.pid);
                                [
                                    launch_pid,
                                    evidence
                                        .as_ref()
                                        .map_or(launch_pid, |value| value.provider_process.pid),
                                ]
                            })
                            .collect::<Vec<_>>();
                        let sampled = self.process_evidence.observe_many(&process_ids);
                        if sampled.len() != process_ids.len() {
                            frame.diagnostics.push(RefreshDiagnostic::for_stage(
                                RefreshStage::Collect,
                                "Process Evidence",
                                "bulk process evidence was incomplete",
                            ));
                        }
                        frame.managed_bindings = pending
                            .into_iter()
                            .enumerate()
                            .map(|(index, (registration, evidence))| {
                                let launch_process = sampled
                                    .get(index * 2)
                                    .copied()
                                    .unwrap_or(ProviderProcessObservation::Unknown);
                                let provider_process = evidence.as_ref().map(|_| {
                                    sampled
                                        .get(index * 2 + 1)
                                        .copied()
                                        .unwrap_or(ProviderProcessObservation::Unknown)
                                });
                                ManagedBindingObservation {
                                    registration_id: registration.id,
                                    evidence,
                                    processes: BindingProcessObservation {
                                        launch_process,
                                        provider_process,
                                    },
                                }
                            })
                            .collect();
                    }
                    Err(error) => frame.diagnostics.push(RefreshDiagnostic::for_stage(
                        RefreshStage::Collect,
                        "Managed Registration Store",
                        error.to_string(),
                    )),
                }
                match store.verified_managed_current_runs(&self.managed_current_run_verifier) {
                    Ok(runs) => {
                        frame.verified_managed_current_runs = runs;
                    }
                    Err(error) => frame.diagnostics.push(RefreshDiagnostic::for_stage(
                        RefreshStage::Collect,
                        "Managed Current Run Evidence",
                        error.to_string(),
                    )),
                }
                match store.resumable_managed_sessions(&self.managed_resume_verifier) {
                    Ok(sessions) => {
                        frame.resumable_managed_sessions = sessions;
                    }
                    Err(error) => frame.diagnostics.push(RefreshDiagnostic::for_stage(
                        RefreshStage::Collect,
                        "Managed Resume Evidence",
                        error.to_string(),
                    )),
                }
            }
            Err(error) => frame.diagnostics.push(RefreshDiagnostic::for_stage(
                RefreshStage::Collect,
                "Managed Session Registry",
                error.clone(),
            )),
        }
        frame
    }

    pub fn with_managed_current_run_evidence(
        mut self,
        evidence: Box<dyn crate::application::managed_session::ManagedCurrentRunEvidence>,
    ) -> Self {
        self.managed_current_run_verifier = ManagedCurrentRunVerifier::new(evidence);
        self
    }

    pub fn with_managed_resume_evidence(
        mut self,
        evidence: Box<dyn crate::application::managed_session::ManagedResumeEvidence>,
    ) -> Self {
        self.managed_resume_verifier = ManagedResumeVerifier::new(evidence);
        self
    }

    pub fn with_binding_process_evidence(mut self, evidence: Arc<dyn ProcessEvidence>) -> Self {
        self.process_evidence = evidence;
        self
    }
}
