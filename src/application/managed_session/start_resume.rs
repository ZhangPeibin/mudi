//! Explicitly authorized Managed Resume workflow and exact prior-run verification.

use super::{
    launch::launch_and_open, ManagedLaunchBroker, ManagedProviderRuntime, ManagedSessionRepository,
    ManagedTerminalAccess, ProcessEvidence, ProviderProcessObservation,
};
use crate::domain::{
    BindingReceipt, ManagedCurrentRunLifecycle, ManagedCurrentRunRecord, ManagedLaunchRegistration,
    ManagedLaunchStatus, ManagedLaunchTarget, ManagedResumeTarget, ManagedSessionRecord, Provider,
    SourceFormat,
};
use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResumeRequest {
    pub provider: Provider,
    pub native_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResumeOutcome {
    pub registration: ManagedLaunchRegistration,
    pub terminal_opened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResumeEvidenceCandidate {
    pub run: ManagedCurrentRunRecord,
    pub session: ManagedSessionRecord,
    pub receipt: BindingReceipt,
}

pub trait ManagedResumeEvidence: Send + Sync {
    fn observe(
        &self,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle>;

    fn observe_many(
        &self,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        candidates
            .iter()
            .map(|candidate| self.observe(&candidate.run, &candidate.session, &candidate.receipt))
            .collect()
    }
}

pub trait ManagedResumeProviderEvidence: Send + Sync {
    fn observe(
        &self,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle>;

    fn observe_many(
        &self,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        candidates
            .iter()
            .map(|candidate| self.observe(&candidate.run, &candidate.session, &candidate.receipt))
            .collect()
    }
}

pub struct CombinedManagedResumeEvidence {
    process_evidence: Box<dyn ProcessEvidence>,
    provider_evidence: Box<dyn ManagedResumeProviderEvidence>,
}

impl CombinedManagedResumeEvidence {
    pub fn new(
        process_evidence: Box<dyn ProcessEvidence>,
        provider_evidence: Box<dyn ManagedResumeProviderEvidence>,
    ) -> Self {
        Self {
            process_evidence,
            provider_evidence,
        }
    }

    pub fn with_evidence(
        process_evidence: Box<dyn ProcessEvidence>,
        provider_evidence: Box<dyn ManagedResumeProviderEvidence>,
    ) -> Self {
        Self::new(process_evidence, provider_evidence)
    }
}

impl ManagedResumeEvidence for CombinedManagedResumeEvidence {
    fn observe(
        &self,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle> {
        let provider_lifecycle = self.provider_evidence.observe(run, session, receipt)?;
        let prior_process = self.process_evidence.observe(run.provider_process.pid);
        combine_resume_evidence(run, session, receipt, provider_lifecycle, prior_process)
    }

    fn observe_many(
        &self,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let provider_lifecycles = self.provider_evidence.observe_many(candidates);
        let pids = candidates
            .iter()
            .map(|candidate| candidate.run.provider_process.pid)
            .collect::<Vec<_>>();
        let process_observations = self.process_evidence.observe_many(&pids);
        if provider_lifecycles.len() != candidates.len()
            || process_observations.len() != candidates.len()
        {
            return candidates
                .iter()
                .map(|_| Err(anyhow!("Managed Resume bulk evidence was incomplete")))
                .collect();
        }
        candidates
            .iter()
            .zip(provider_lifecycles)
            .zip(process_observations)
            .map(|((candidate, provider_lifecycle), prior_process)| {
                combine_resume_evidence(
                    &candidate.run,
                    &candidate.session,
                    &candidate.receipt,
                    provider_lifecycle?,
                    prior_process,
                )
            })
            .collect()
    }
}

fn combine_resume_evidence(
    run: &ManagedCurrentRunRecord,
    session: &ManagedSessionRecord,
    receipt: &BindingReceipt,
    provider_lifecycle: ManagedCurrentRunLifecycle,
    prior_process: ProviderProcessObservation,
) -> Result<ManagedCurrentRunLifecycle> {
    if run.provider != session.provider
        || run.provider != receipt.provider
        || run.native_session_id != session.native_session_id
        || run.native_session_id != receipt.native_session_id
        || run.provider_process != receipt.provider_process
        || run.binding_receipt_id != receipt.id
        || session.binding_receipt_id != receipt.id
        || receipt.source.format != SourceFormat::JsonLines
    {
        return Ok(ManagedCurrentRunLifecycle::Unknown);
    }
    match prior_process {
        ProviderProcessObservation::Unknown => Ok(ManagedCurrentRunLifecycle::Unknown),
        ProviderProcessObservation::Live(identity) if identity != run.provider_process => {
            Ok(ManagedCurrentRunLifecycle::Unknown)
        }
        ProviderProcessObservation::Live(_) => {
            if provider_lifecycle == ManagedCurrentRunLifecycle::Open {
                Ok(ManagedCurrentRunLifecycle::Open)
            } else {
                Ok(ManagedCurrentRunLifecycle::Unknown)
            }
        }
        ProviderProcessObservation::Absent
            if provider_lifecycle == ManagedCurrentRunLifecycle::Open =>
        {
            Ok(ManagedCurrentRunLifecycle::Open)
        }
        ProviderProcessObservation::Absent
            if provider_lifecycle == ManagedCurrentRunLifecycle::Exited =>
        {
            Ok(ManagedCurrentRunLifecycle::Exited)
        }
        ProviderProcessObservation::Absent => Ok(ManagedCurrentRunLifecycle::Unknown),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedExitedManagedSession {
    pub provider: Provider,
    pub native_session_id: String,
    pub target: ManagedLaunchTarget,
    pub session_alias: String,
    pub description: Option<String>,
    prior_run_id: String,
}

pub struct ManagedResumeVerifier {
    evidence: Box<dyn ManagedResumeEvidence>,
}

impl ManagedResumeVerifier {
    pub fn new(evidence: Box<dyn ManagedResumeEvidence>) -> Self {
        Self { evidence }
    }

    pub fn verify_exited(
        &self,
        store: &dyn ManagedSessionRepository,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<VerifiedExitedManagedSession> {
        let candidate = self.evidence_candidate(store, provider, native_session_id)?;
        let lifecycle =
            self.evidence
                .observe(&candidate.run, &candidate.session, &candidate.receipt)?;
        verified_exited_session(candidate, lifecycle)
    }

    fn evidence_candidate(
        &self,
        store: &dyn ManagedSessionRepository,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<ManagedResumeEvidenceCandidate> {
        if native_session_id.trim().is_empty() {
            return Err(anyhow!("Resume requires an exact native Session ID"));
        }
        let session = store
            .managed_session(provider, native_session_id)?
            .ok_or_else(|| anyhow!("Resume requires an existing Managed Session"))?;
        let run = store
            .managed_current_run(provider, native_session_id)?
            .ok_or_else(|| anyhow!("Resume requires a prior Managed Current Run"))?;
        let receipt = store
            .binding_receipt_by_id(&run.binding_receipt_id)?
            .ok_or_else(|| anyhow!("prior Managed Current Run Binding Receipt is unavailable"))?;
        if session.provider != provider
            || session.native_session_id != native_session_id
            || session.binding_receipt_id != run.binding_receipt_id
            || run.provider != provider
            || run.native_session_id != native_session_id
            || receipt.id != run.binding_receipt_id
            || receipt.provider != provider
            || receipt.native_session_id != native_session_id
            || receipt.provider_process != run.provider_process
        {
            return Err(anyhow!("Resume authority metadata is contradictory"));
        }
        if store.has_pending_resume(provider, native_session_id)? {
            return Err(anyhow!(
                "Resume is already in progress for this Managed Session"
            ));
        }
        Ok(ManagedResumeEvidenceCandidate {
            run,
            session,
            receipt,
        })
    }

    pub fn verified_exited_sessions(
        &self,
        store: &dyn ManagedSessionRepository,
    ) -> Result<Vec<VerifiedExitedManagedSession>> {
        let candidates = store
            .managed_sessions()?
            .into_iter()
            .filter_map(|session| {
                self.evidence_candidate(store, session.provider, &session.native_session_id)
                    .ok()
            })
            .collect::<Vec<_>>();
        let observations = self.evidence.observe_many(&candidates);
        if observations.len() != candidates.len() {
            return Err(anyhow!("Managed Resume bulk evidence was incomplete"));
        }
        Ok(candidates
            .into_iter()
            .zip(observations)
            .filter_map(|(candidate, observation)| {
                verified_exited_session(candidate, observation.ok()?).ok()
            })
            .collect())
    }
}

fn verified_exited_session(
    candidate: ManagedResumeEvidenceCandidate,
    lifecycle: ManagedCurrentRunLifecycle,
) -> Result<VerifiedExitedManagedSession> {
    match lifecycle {
        ManagedCurrentRunLifecycle::Exited => Ok(VerifiedExitedManagedSession {
            provider: candidate.session.provider,
            native_session_id: candidate.session.native_session_id,
            target: candidate.session.target,
            session_alias: candidate.session.session_alias,
            description: candidate.session.description,
            prior_run_id: candidate.run.id,
        }),
        ManagedCurrentRunLifecycle::Open => Err(anyhow!(
            "Resume is unavailable while the Managed Current Run is Open"
        )),
        ManagedCurrentRunLifecycle::Unknown => Err(anyhow!(
            "prior Managed Current Run exit could not be verified"
        )),
    }
}

pub struct ManagedResumeService {
    store: Box<dyn ManagedSessionRepository>,
    provider_runtime: Box<dyn ManagedProviderRuntime>,
    launch_broker: Box<dyn ManagedLaunchBroker>,
    terminal_access: Box<dyn ManagedTerminalAccess>,
    verifier: Option<ManagedResumeVerifier>,
}

impl ManagedResumeService {
    pub fn new(
        store: impl ManagedSessionRepository + 'static,
        provider_runtime: impl ManagedProviderRuntime + 'static,
        launch_broker: Box<dyn ManagedLaunchBroker>,
        terminal_access: Box<dyn ManagedTerminalAccess>,
    ) -> Self {
        Self {
            store: Box::new(store),
            provider_runtime: Box::new(provider_runtime),
            launch_broker,
            terminal_access,
            verifier: None,
        }
    }

    pub fn with_resume_verifier(mut self, verifier: ManagedResumeVerifier) -> Self {
        self.verifier = Some(verifier);
        self
    }

    pub fn start(
        &mut self,
        request: ManagedResumeRequest,
        now_ms: i64,
    ) -> Result<ManagedResumeOutcome> {
        let verified = self
            .verifier
            .as_ref()
            .ok_or_else(|| anyhow!("Managed Resume verification is unavailable"))?
            .verify_exited(
                self.store.as_ref(),
                request.provider,
                &request.native_session_id,
            )?;
        let command = self
            .provider_runtime
            .resume_session_command(verified.provider, &verified.native_session_id)?;
        let id = self.provider_runtime.new_registration_id(now_ms);
        let control_channel = self.launch_broker.control_channel(&id)?;
        if !control_channel.is_valid() {
            return Err(anyhow!("private Managed control channel is invalid"));
        }
        let registration = ManagedLaunchRegistration {
            id,
            status: ManagedLaunchStatus::Binding,
            provider: verified.provider,
            target: verified.target,
            session_alias: verified.session_alias,
            description: verified.description,
            binding_initializer: false,
            control_channel,
            created_at_ms: now_ms,
            launched_at_ms: None,
            launch_evidence: None,
            terminal_opened_at_ms: None,
            terminal_error: None,
            binding_error: None,
        };
        self.store.insert_resume(
            &registration,
            &ManagedResumeTarget {
                provider: verified.provider,
                native_session_id: verified.native_session_id,
                prior_run_id: verified.prior_run_id,
            },
        )?;
        let (registration, terminal_opened) = launch_and_open(
            self.store.as_ref(),
            self.launch_broker.as_mut(),
            self.terminal_access.as_mut(),
            &registration,
            &command,
            now_ms,
            "Managed Provider Resume launch failed",
        )?;
        Ok(ManagedResumeOutcome {
            registration,
            terminal_opened,
        })
    }
}
