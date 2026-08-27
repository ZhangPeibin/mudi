//! Explicitly authorized Managed Session launch registration and native New startup.

use crate::domain::{
    ProcessIdentity, Provider, Session, SessionIdentity, SourceFormat, SourceMetadata,
};
use anyhow::{anyhow, Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::Arc;

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

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "binding" => Ok(Self::Binding),
            "needs_review" => Ok(Self::NeedsReview),
            "failed" => Ok(Self::Failed),
            _ => Err(anyhow!(
                "unknown Managed Launch Registration status {value:?}"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLaunchTarget {
    pub working_directory: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedNewRequest {
    pub provider: Provider,
    pub target: ManagedLaunchTarget,
    pub title: String,
    pub description: Option<String>,
    pub binding_initializer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResumeRequest {
    pub provider: Provider,
    pub native_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderNewCommand {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
}

/// Provider-normalized source and process evidence considered for a Binding Receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBindingEvidence {
    pub provider: Provider,
    pub source_session_id: String,
    pub process_session_id: String,
    pub source: SourceMetadata,
    pub working_directory: PathBuf,
    pub launch_process: ProcessIdentity,
    pub provider_process: ProcessIdentity,
}

/// Provider-native command construction required by Managed New and Resume.
pub trait ManagedProviderRuntime: Send + Sync {
    fn validate_target(&self, target: &ManagedLaunchTarget) -> Result<()>;

    fn new_registration_id(&self, now_ms: i64) -> String;

    fn new_session_command(
        &self,
        provider: Provider,
        binding_initializer: bool,
    ) -> Result<ProviderNewCommand>;

    fn resume_session_command(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<ProviderNewCommand>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateTmuxEnvironment {
    pub socket_name: String,
    pub session_name: String,
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
    pub provider_command: ProviderNewCommand,
    pub tmux: PrivateTmuxEnvironment,
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
            (ManagedLaunchStatus::Binding, false) => {
                vec![ManagedLaunchAction::CancelStart]
            }
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
    pub tmux_pane_id: String,
    /// Root command PID reported by tmux for this pane. It may be a provider
    /// wrapper rather than the process that later owns the source record.
    pub provider_process: ProcessIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedNewOutcome {
    pub registration: ManagedLaunchRegistration,
    pub terminal_opened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResumeOutcome {
    pub registration: ManagedLaunchRegistration,
    pub terminal_opened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedTerminalOutcome {
    pub registration: ManagedLaunchRegistration,
    pub opened: bool,
}

/// Conclusive record that atomically established a Managed Session and Current Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingReceipt {
    pub id: String,
    pub registration_id: String,
    pub provider: Provider,
    pub native_session_id: String,
    pub source: SourceMetadata,
    pub provider_process: ProcessIdentity,
    pub issued_at_ms: i64,
}

/// Durable management metadata for one provider-native Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSessionRecord {
    pub provider: Provider,
    pub native_session_id: String,
    pub target: ManagedLaunchTarget,
    pub session_alias: String,
    pub description: Option<String>,
    pub binding_receipt_id: String,
}

/// Durable identity of the exact Current Run established by a Binding Receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedCurrentRunRecord {
    pub id: String,
    pub opaque_ref: String,
    pub provider: Provider,
    pub native_session_id: String,
    pub provider_process: ProcessIdentity,
    pub tmux: PrivateTmuxEnvironment,
    pub binding_receipt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResumeTarget {
    pub provider: Provider,
    pub native_session_id: String,
    pub prior_run_id: String,
}

/// Authoritative Managed Session operations consumed by application workflows.
pub trait ManagedSessionRepository: Send {
    fn insert(&self, registration: &ManagedLaunchRegistration) -> Result<()>;
    fn insert_resume(
        &mut self,
        registration: &ManagedLaunchRegistration,
        target: &ManagedResumeTarget,
    ) -> Result<()>;
    fn managed_resume_target(&self, registration_id: &str) -> Result<Option<ManagedResumeTarget>>;
    fn launch_title_association(&self, registration_id: &str) -> Result<Option<SessionIdentity>>;
    fn record_launch_title_association(
        &self,
        registration_id: &str,
        session: &SessionIdentity,
    ) -> Result<()>;
    fn associate_launch_title(
        &mut self,
        registration_id: &str,
        session: &SessionIdentity,
        _alias: &str,
    ) -> Result<()> {
        self.record_launch_title_association(registration_id, session)
    }
    fn has_pending_resume(&self, provider: Provider, native_session_id: &str) -> Result<bool>;
    fn get(&self, registration_id: &str) -> Result<Option<ManagedLaunchRegistration>>;
    fn binding_registrations(&self) -> Result<Vec<ManagedLaunchRegistration>>;
    fn registrations(&self) -> Result<Vec<ManagedLaunchRegistration>>;
    fn binding_receipt(&self, registration_id: &str) -> Result<Option<BindingReceipt>>;
    fn binding_receipt_by_id(&self, receipt_id: &str) -> Result<Option<BindingReceipt>>;
    fn managed_session(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedSessionRecord>>;
    fn managed_sessions(&self) -> Result<Vec<ManagedSessionRecord>>;
    fn managed_current_run(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedCurrentRunRecord>>;
    fn managed_current_runs(&self) -> Result<Vec<ManagedCurrentRunRecord>>;
    fn apply_provenance(&self, sessions: &mut [Session]) -> Result<()>;
    fn establish_management(
        &mut self,
        registration: &ManagedLaunchRegistration,
        receipt: &BindingReceipt,
    ) -> Result<()>;
    fn record_launched(
        &self,
        registration_id: &str,
        now_ms: i64,
        evidence: ManagedLaunchEvidence,
    ) -> Result<()>;
    fn record_terminal_opened(&self, registration_id: &str, now_ms: i64) -> Result<()>;
    fn record_terminal_error(&self, registration_id: &str, error: &str) -> Result<()>;
    fn remove_binding(&mut self, registration_id: &str) -> Result<()>;
    fn mark_needs_review(&self, registration_id: &str, error: &str) -> Result<()>;
    fn mark_failed(&mut self, registration_id: &str, error: &str) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedCurrentRunLifecycle {
    Open,
    Exited,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResumeEvidenceCandidate {
    pub run: ManagedCurrentRunRecord,
    pub session: ManagedSessionRecord,
    pub receipt: BindingReceipt,
}

/// System boundary for proving the lifecycle of the exact prior Managed Current Run.
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

pub struct SystemManagedResumeEvidence {
    process_evidence: Box<dyn ProcessEvidence>,
    provider_evidence: Box<dyn ManagedResumeProviderEvidence>,
}

impl SystemManagedResumeEvidence {
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
        Self {
            process_evidence,
            provider_evidence,
        }
    }
}

impl ManagedResumeEvidence for SystemManagedResumeEvidence {
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

/// System boundary for proving that stored management metadata still names one Open Run.
pub trait ManagedCurrentRunEvidence: Send + Sync {
    fn revalidate(
        &self,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<()>;
}

/// A revalidated target. Private terminal addressing intentionally has no public accessor.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedManagedCurrentRun {
    pub opaque_ref: String,
    pub provider: Provider,
    pub native_session_id: String,
    pub(crate) tmux: PrivateTmuxEnvironment,
}

pub struct ManagedCurrentRunVerifier {
    evidence: Box<dyn ManagedCurrentRunEvidence>,
}

impl ManagedCurrentRunVerifier {
    pub fn new(evidence: Box<dyn ManagedCurrentRunEvidence>) -> Self {
        Self { evidence }
    }

    pub fn verify_ref(
        &self,
        store: &dyn ManagedSessionRepository,
        opaque_ref: &str,
    ) -> Result<VerifiedManagedCurrentRun> {
        if !valid_opaque_run_ref(opaque_ref) {
            return Err(anyhow!("Managed Current Run reference is invalid"));
        }
        let candidates = store
            .managed_current_runs()?
            .into_iter()
            .filter(|run| run.opaque_ref == opaque_ref)
            .collect::<Vec<_>>();
        let [run] = candidates.as_slice() else {
            return Err(if candidates.is_empty() {
                anyhow!("Managed Current Run reference is stale or unavailable")
            } else {
                anyhow!("Managed Current Run reference is ambiguous")
            });
        };
        let session = store
            .managed_session(run.provider, &run.native_session_id)?
            .ok_or_else(|| anyhow!("Managed Current Run has no Managed Session authority"))?;
        let receipt = store
            .binding_receipt_by_id(&run.binding_receipt_id)?
            .ok_or_else(|| anyhow!("Managed Current Run Binding Receipt is unavailable"))?;
        if session.provider != run.provider
            || session.native_session_id != run.native_session_id
            || session.binding_receipt_id != run.binding_receipt_id
            || receipt.provider != run.provider
            || receipt.native_session_id != run.native_session_id
            || receipt.provider_process != run.provider_process
            || receipt.id != run.binding_receipt_id
        {
            return Err(anyhow!(
                "Managed Current Run authority metadata is contradictory"
            ));
        }
        validate_tmux_name(&run.tmux.socket_name)?;
        validate_tmux_name(&run.tmux.session_name)?;
        self.evidence.revalidate(run, &session, &receipt)?;
        Ok(VerifiedManagedCurrentRun {
            opaque_ref: run.opaque_ref.clone(),
            provider: run.provider,
            native_session_id: run.native_session_id.clone(),
            tmux: run.tmux.clone(),
        })
    }

    pub fn verified_runs(
        &self,
        store: &dyn ManagedSessionRepository,
    ) -> Result<Vec<VerifiedManagedCurrentRun>> {
        let references = store
            .managed_current_runs()?
            .into_iter()
            .map(|run| run.opaque_ref)
            .collect::<Vec<_>>();
        Ok(references
            .iter()
            .filter_map(|opaque_ref| self.verify_ref(store, opaque_ref).ok())
            .collect())
    }
}

/// Terminal side effects allowed after a Managed Current Run has been revalidated.
pub trait ManagedCurrentRunTerminalAccess: Send {
    fn open_terminal(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()>;
    fn attach_foreground(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()>;
}

/// Application service shared by TUI re-entry actions and the public attach command.
pub struct ManagedCurrentRunAccessService {
    store: Box<dyn ManagedSessionRepository>,
    verifier: ManagedCurrentRunVerifier,
    terminal_access: Box<dyn ManagedCurrentRunTerminalAccess>,
}

impl ManagedCurrentRunAccessService {
    pub fn new(
        store: impl ManagedSessionRepository + 'static,
        verifier: ManagedCurrentRunVerifier,
        terminal_access: Box<dyn ManagedCurrentRunTerminalAccess>,
    ) -> Self {
        Self {
            store: Box::new(store),
            verifier,
            terminal_access,
        }
    }

    pub fn open_terminal(
        &mut self,
        provider: Provider,
        native_session_id: &str,
        opaque_ref: &str,
    ) -> Result<()> {
        let run = self.verify_identity(provider, native_session_id, opaque_ref)?;
        self.terminal_access.open_terminal(&run)
    }

    pub fn attach_command(
        &self,
        provider: Provider,
        native_session_id: &str,
        opaque_ref: &str,
    ) -> Result<String> {
        let run = self.verify_identity(provider, native_session_id, opaque_ref)?;
        Ok(format!("mudi attach {}", run.opaque_ref))
    }

    pub fn attach_foreground(&mut self, opaque_ref: &str) -> Result<()> {
        let run = self.verifier.verify_ref(self.store.as_ref(), opaque_ref)?;
        self.terminal_access.attach_foreground(&run)
    }

    fn verify_identity(
        &self,
        provider: Provider,
        native_session_id: &str,
        opaque_ref: &str,
    ) -> Result<VerifiedManagedCurrentRun> {
        let run = self.verifier.verify_ref(self.store.as_ref(), opaque_ref)?;
        if run.provider != provider || run.native_session_id != native_session_id {
            return Err(anyhow!(
                "Managed Current Run reference does not match the selected Session"
            ));
        }
        Ok(run)
    }
}

/// Handle the public `mudi attach <opaque-run-ref>` contract with injected access.
/// Returns false when the arguments belong to the normal TUI or another helper mode.
pub fn run_attach_from_args_with_access<I, S>(
    args: I,
    access: &mut ManagedCurrentRunAccessService,
) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();
    if args.next().as_deref() != Some(OsStr::new("attach")) {
        return Ok(false);
    }
    let opaque_ref = args
        .next()
        .ok_or_else(|| anyhow!("missing opaque Managed Current Run reference"))?
        .into_string()
        .map_err(|_| anyhow!("Managed Current Run reference is not valid UTF-8"))?;
    if args.next().is_some() {
        return Err(anyhow!("unexpected attach command arguments"));
    }
    access.attach_foreground(&opaque_ref)?;
    Ok(true)
}

fn valid_opaque_run_ref(value: &str) -> bool {
    value.len() == 36
        && value.starts_with("run_")
        && value[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingOutcome {
    Waiting,
    Bound(BindingReceipt),
    NeedsReview,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchIdentityAssociationOutcome {
    ContinueBinding,
    NeedsReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProcessObservation {
    Live(ProcessIdentity),
    Absent,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingProcessObservation {
    pub launch_process: ProviderProcessObservation,
    pub provider_process: Option<ProviderProcessObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedBindingAttempt<'a> {
    pub registration_id: &'a str,
    pub provider_evidence: Option<ProviderBindingEvidence>,
    pub process_evidence: BindingProcessObservation,
    pub observed_at_ms: i64,
    pub after_restart: bool,
}

struct BindingAttempt<'a> {
    registration_id: &'a str,
    provider_evidence: Option<ProviderBindingEvidence>,
    process_evidence: Option<BindingProcessObservation>,
    observed_at_ms: i64,
    clean_confirmed_absence: bool,
}

/// Revalidates exact process identity immediately before receipt issuance.
pub trait ProcessEvidence: Send + Sync {
    fn observe(&self, pid: u32) -> ProviderProcessObservation;

    fn observe_many(&self, pids: &[u32]) -> Vec<ProviderProcessObservation> {
        pids.iter().map(|pid| self.observe(*pid)).collect()
    }
}

pub trait ManagedLaunchCleanup: Send + Sync {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> Result<()>;
}

/// Sole issuer of Binding Receipts.
pub struct EvidenceBinder {
    process_evidence: Arc<dyn ProcessEvidence>,
    cleanup: Box<dyn ManagedLaunchCleanup>,
}

impl EvidenceBinder {
    pub fn new(
        process_evidence: Box<dyn ProcessEvidence>,
        cleanup: Box<dyn ManagedLaunchCleanup>,
    ) -> Self {
        Self {
            process_evidence: Arc::from(process_evidence),
            cleanup,
        }
    }

    pub fn process_evidence(&self) -> Arc<dyn ProcessEvidence> {
        self.process_evidence.clone()
    }

    pub fn bind(
        &self,
        store: &mut dyn ManagedSessionRepository,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
    ) -> Result<BindingOutcome> {
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id,
                provider_evidence: evidence,
                process_evidence: None,
                observed_at_ms: now_ms,
                clean_confirmed_absence: true,
            },
            |_, _, _| Ok(LaunchIdentityAssociationOutcome::ContinueBinding),
        )
    }

    pub fn bind_with_identity_association<F>(
        &self,
        store: &mut dyn ManagedSessionRepository,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut dyn ManagedSessionRepository,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id,
                provider_evidence: evidence,
                process_evidence: None,
                observed_at_ms: now_ms,
                clean_confirmed_absence: true,
            },
            associate,
        )
    }

    /// Reconcile a launch retained across a Mudi restart without cleanup side effects.
    pub fn reconcile_after_restart(
        &self,
        store: &mut dyn ManagedSessionRepository,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
    ) -> Result<BindingOutcome> {
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id,
                provider_evidence: evidence,
                process_evidence: None,
                observed_at_ms: now_ms,
                clean_confirmed_absence: false,
            },
            |_, _, _| Ok(LaunchIdentityAssociationOutcome::ContinueBinding),
        )
    }

    pub fn reconcile_after_restart_with_identity_association<F>(
        &self,
        store: &mut dyn ManagedSessionRepository,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut dyn ManagedSessionRepository,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id,
                provider_evidence: evidence,
                process_evidence: None,
                observed_at_ms: now_ms,
                clean_confirmed_absence: false,
            },
            associate,
        )
    }

    pub fn bind_observed_with_identity_association<F>(
        &self,
        store: &mut dyn ManagedSessionRepository,
        attempt: ObservedBindingAttempt<'_>,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut dyn ManagedSessionRepository,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id: attempt.registration_id,
                provider_evidence: attempt.provider_evidence,
                process_evidence: Some(attempt.process_evidence),
                observed_at_ms: attempt.observed_at_ms,
                clean_confirmed_absence: !attempt.after_restart,
            },
            associate,
        )
    }

    fn bind_with_policy<F>(
        &self,
        store: &mut dyn ManagedSessionRepository,
        attempt: BindingAttempt<'_>,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut dyn ManagedSessionRepository,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
        let BindingAttempt {
            registration_id,
            provider_evidence: evidence,
            process_evidence: processes,
            observed_at_ms: now_ms,
            clean_confirmed_absence,
        } = attempt;
        let registration = store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Binding Managed Launch Registration {registration_id} is unavailable")
        })?;
        let launch = registration
            .launch_evidence
            .as_ref()
            .ok_or_else(|| anyhow!("Binding requires conclusive launch evidence"))?;

        if registration.status != ManagedLaunchStatus::Binding {
            return Err(anyhow!("only a Binding registration can issue a Receipt"));
        }
        let launch_process = processes.map_or_else(
            || self.process_evidence.observe(launch.provider_process.pid),
            |observed| observed.launch_process,
        );
        match launch_process {
            ProviderProcessObservation::Absent => {
                if !clean_confirmed_absence {
                    store.mark_needs_review(
                        registration_id,
                        "Provider process is absent after restart; launch scene was preserved",
                    )?;
                    return Ok(BindingOutcome::NeedsReview);
                }
                if let Err(error) = self.cleanup.clean_empty(&registration) {
                    store.mark_needs_review(
                        registration_id,
                        &format!("empty private tmux could not be confirmed: {error}"),
                    )?;
                    return Ok(BindingOutcome::NeedsReview);
                }
                store.mark_failed(registration_id, "Provider process is confirmed absent")?;
                return Ok(BindingOutcome::Failed);
            }
            ProviderProcessObservation::Unknown => {
                store.mark_needs_review(
                    registration_id,
                    "launched Provider process could not be verified",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(identity) if identity != launch.provider_process => {
                store.mark_needs_review(
                    registration_id,
                    "launched Provider process is stale or its PID was reused",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(_) => {}
        }
        let Some(evidence) = evidence else {
            return Ok(BindingOutcome::Waiting);
        };
        let provider_process = processes.map_or_else(
            || self.process_evidence.observe(evidence.provider_process.pid),
            |observed| {
                observed
                    .provider_process
                    .unwrap_or(ProviderProcessObservation::Unknown)
            },
        );
        match provider_process {
            ProviderProcessObservation::Absent => {
                store.mark_needs_review(
                    registration_id,
                    "Provider source process is no longer present",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Unknown => {
                store.mark_needs_review(
                    registration_id,
                    "Provider source process could not be verified",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(identity) if identity != evidence.provider_process => {
                store.mark_needs_review(
                    registration_id,
                    "Provider source process is stale or its PID was reused",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(_) => {}
        }
        let identity = match conclusive_launch_identity(&registration, &evidence) {
            Ok(identity) => identity,
            Err(error) => {
                store.mark_needs_review(registration_id, &error.to_string())?;
                return Ok(BindingOutcome::NeedsReview);
            }
        };
        if associate(store, &registration, &identity.session)?
            == LaunchIdentityAssociationOutcome::NeedsReview
        {
            return Ok(BindingOutcome::NeedsReview);
        }
        if let Some(target) = store.managed_resume_target(registration_id)? {
            let current = store.managed_current_run(target.provider, &target.native_session_id)?;
            let session = store.managed_session(target.provider, &target.native_session_id)?;
            let resume_identity_matches = target.provider == registration.provider
                && evidence.provider == target.provider
                && evidence.source_session_id == target.native_session_id
                && evidence.process_session_id == target.native_session_id
                && current
                    .as_ref()
                    .is_some_and(|run| run.id == target.prior_run_id)
                && session.as_ref().is_some_and(|managed| {
                    current
                        .as_ref()
                        .is_some_and(|run| managed.binding_receipt_id == run.binding_receipt_id)
                });
            if !resume_identity_matches {
                store.mark_needs_review(
                    registration_id,
                    "Resume evidence does not match the exact Managed Session and prior Run",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
        }
        let receipt = match binding_receipt_candidate(&registration, identity, now_ms) {
            Ok(receipt) => receipt,
            Err(error) => {
                store.mark_needs_review(registration_id, &error.to_string())?;
                return Ok(BindingOutcome::NeedsReview);
            }
        };
        if let Err(error) = store.establish_management(&registration, &receipt) {
            store.mark_needs_review(registration_id, &error.to_string())?;
            return Ok(BindingOutcome::NeedsReview);
        }
        Ok(BindingOutcome::Bound(receipt))
    }
}

struct ConclusiveLaunchIdentity {
    session: SessionIdentity,
    evidence: ProviderBindingEvidence,
    source_path: PathBuf,
}

fn conclusive_launch_identity(
    registration: &ManagedLaunchRegistration,
    evidence: &ProviderBindingEvidence,
) -> Result<ConclusiveLaunchIdentity> {
    let launch = registration
        .launch_evidence
        .as_ref()
        .ok_or_else(|| anyhow!("Binding requires conclusive launch evidence"))?;
    if evidence.provider != registration.provider {
        return Err(anyhow!(
            "Provider source evidence disagrees with the launch"
        ));
    }
    if evidence.source_session_id.is_empty()
        || evidence.process_session_id.is_empty()
        || evidence.source_session_id != evidence.process_session_id
    {
        return Err(anyhow!(
            "native Session ID evidence is incomplete or conflicting"
        ));
    }
    if evidence.launch_process != launch.provider_process {
        return Err(anyhow!(
            "Provider source evidence disagrees with the launched pane process"
        ));
    }
    let source_path = evidence
        .source
        .path
        .canonicalize()
        .context("Binding Provider source is unavailable")?;
    if !source_path.is_file() {
        return Err(anyhow!("Binding Provider source is not a file"));
    }
    Ok(ConclusiveLaunchIdentity {
        session: SessionIdentity {
            provider: registration.provider,
            native_id: evidence.source_session_id.clone(),
        },
        evidence: evidence.clone(),
        source_path,
    })
}

fn binding_receipt_candidate(
    registration: &ManagedLaunchRegistration,
    identity: ConclusiveLaunchIdentity,
    now_ms: i64,
) -> Result<BindingReceipt> {
    let evidence = identity.evidence;
    let working_directory = evidence
        .working_directory
        .canonicalize()
        .context("Binding working directory is unavailable")?;
    let target_directory = registration
        .target
        .working_directory
        .canonicalize()
        .context("Managed launch target is unavailable")?;
    if working_directory != target_directory {
        return Err(anyhow!(
            "Provider source working directory disagrees with the launch"
        ));
    }
    Ok(BindingReceipt {
        id: format!("receipt-{}", registration.id),
        registration_id: registration.id.clone(),
        provider: identity.session.provider,
        native_session_id: identity.session.native_id,
        source: SourceMetadata {
            path: identity.source_path,
            format: evidence.source.format,
        },
        provider_process: evidence.provider_process,
        issued_at_ms: now_ms,
    })
}

pub trait ManagedLaunchBroker {
    fn launch(&mut self, registration: &ManagedLaunchRegistration)
        -> Result<ManagedLaunchEvidence>;

    fn cancel(&mut self, _registration: &ManagedLaunchRegistration) -> Result<()> {
        Err(anyhow!("Managed launch cancellation is unavailable"))
    }
}

pub trait ManagedTerminalAccess {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> Result<()>;
}

pub struct ManagedSessionService {
    store: Box<dyn ManagedSessionRepository>,
    provider_runtime: Box<dyn ManagedProviderRuntime>,
    launch_broker: Box<dyn ManagedLaunchBroker>,
    terminal_access: Box<dyn ManagedTerminalAccess>,
    resume_verifier: Option<ManagedResumeVerifier>,
}

impl ManagedSessionService {
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
            resume_verifier: None,
        }
    }

    pub fn with_resume_verifier(mut self, verifier: ManagedResumeVerifier) -> Self {
        self.resume_verifier = Some(verifier);
        self
    }

    pub fn start_new(
        &mut self,
        mut request: ManagedNewRequest,
        now_ms: i64,
    ) -> Result<ManagedNewOutcome> {
        request.binding_initializer = true;
        if request.title.trim().is_empty() {
            return Err(anyhow!("New requires a Title"));
        }
        request.title = crate::application::session_labels::validate_alias(&request.title)?;
        request.description = crate::application::session_labels::validate_description(
            request.description.as_deref().unwrap_or_default(),
        )?;
        self.provider_runtime.validate_target(&request.target)?;

        let provider_command = self
            .provider_runtime
            .new_session_command(request.provider, request.binding_initializer)?;
        let id = self.provider_runtime.new_registration_id(now_ms);
        let registration = ManagedLaunchRegistration {
            id: id.clone(),
            status: ManagedLaunchStatus::Binding,
            provider: request.provider,
            target: request.target,
            session_alias: request.title,
            description: request.description,
            binding_initializer: request.binding_initializer,
            provider_command,
            tmux: PrivateTmuxEnvironment {
                socket_name: format!("mudi-{id}"),
                session_name: "managed".to_string(),
            },
            created_at_ms: now_ms,
            launched_at_ms: None,
            launch_evidence: None,
            terminal_opened_at_ms: None,
            terminal_error: None,
            binding_error: None,
        };

        self.store.insert(&registration)?;
        let launch_evidence = self
            .launch_broker
            .launch(&registration)
            .context("Managed Provider launch failed")?;
        self.store
            .record_launched(&registration.id, now_ms, launch_evidence)?;
        let launched = self
            .store
            .get(&registration.id)?
            .ok_or_else(|| anyhow!("Managed Launch Registration disappeared after launch"))?;

        let terminal_opened = match self.terminal_access.open(&launched) {
            Ok(()) => {
                self.store
                    .record_terminal_opened(&registration.id, now_ms)?;
                true
            }
            Err(error) => {
                self.store
                    .record_terminal_error(&registration.id, &error.to_string())?;
                false
            }
        };
        let registration = self.store.get(&registration.id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration disappeared after Terminal access")
        })?;
        Ok(ManagedNewOutcome {
            registration,
            terminal_opened,
        })
    }

    pub fn start_resume(
        &mut self,
        request: ManagedResumeRequest,
        now_ms: i64,
    ) -> Result<ManagedResumeOutcome> {
        let verified = self
            .resume_verifier
            .as_ref()
            .ok_or_else(|| anyhow!("Managed Resume verification is unavailable"))?
            .verify_exited(
                self.store.as_ref(),
                request.provider,
                &request.native_session_id,
            )?;
        let provider_command = self
            .provider_runtime
            .resume_session_command(verified.provider, &verified.native_session_id)?;
        let id = self.provider_runtime.new_registration_id(now_ms);
        let registration = ManagedLaunchRegistration {
            id: id.clone(),
            status: ManagedLaunchStatus::Binding,
            provider: verified.provider,
            target: verified.target,
            session_alias: verified.session_alias,
            description: verified.description,
            binding_initializer: false,
            provider_command,
            tmux: PrivateTmuxEnvironment {
                socket_name: format!("mudi-{id}"),
                session_name: "managed".to_string(),
            },
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
        let launch_evidence = self
            .launch_broker
            .launch(&registration)
            .context("Managed Provider Resume launch failed")?;
        self.store
            .record_launched(&registration.id, now_ms, launch_evidence)?;
        let launched = self
            .store
            .get(&registration.id)?
            .ok_or_else(|| anyhow!("Managed Resume Registration disappeared after launch"))?;
        let terminal_opened = match self.terminal_access.open(&launched) {
            Ok(()) => {
                self.store
                    .record_terminal_opened(&registration.id, now_ms)?;
                true
            }
            Err(error) => {
                self.store
                    .record_terminal_error(&registration.id, &error.to_string())?;
                false
            }
        };
        let registration = self.store.get(&registration.id)?.ok_or_else(|| {
            anyhow!("Managed Resume Registration disappeared after Terminal access")
        })?;
        Ok(ManagedResumeOutcome {
            registration,
            terminal_opened,
        })
    }

    pub fn cancel_start(&mut self, registration_id: &str) -> Result<()> {
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration {registration_id} is unavailable")
        })?;
        if registration.status != ManagedLaunchStatus::Binding {
            return Err(anyhow!("Cancel Start is available only while Binding"));
        }
        self.launch_broker
            .cancel(&registration)
            .context("Cancel Start could not stop the Managed launch")?;
        self.store.remove_binding(registration_id)
    }

    pub fn retry_open_terminal(
        &mut self,
        registration_id: &str,
        now_ms: i64,
    ) -> Result<ManagedTerminalOutcome> {
        self.access_terminal(
            registration_id,
            now_ms,
            ManagedLaunchAction::RetryOpenTerminal,
            "Retry Open Terminal",
        )
    }

    pub fn open_terminal(
        &mut self,
        registration_id: &str,
        now_ms: i64,
    ) -> Result<ManagedTerminalOutcome> {
        self.access_terminal(
            registration_id,
            now_ms,
            ManagedLaunchAction::OpenTerminal,
            "Open Terminal",
        )
    }

    pub fn copy_diagnostics(&self, registration_id: &str) -> Result<String> {
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration {registration_id} is unavailable")
        })?;
        if !registration.allows(ManagedLaunchAction::CopyDiagnostics) {
            return Err(anyhow!(
                "Copy Diagnostics is unavailable for this Managed launch"
            ));
        }
        let launch = registration
            .launch_evidence
            .as_ref()
            .ok_or_else(|| anyhow!("Managed launch evidence is unavailable"))?;
        let mut diagnostics = vec![
            format!("Managed Launch Registration: {}", registration.id),
            "Status: Needs Review".to_string(),
            format!("Provider: {}", registration.provider.label()),
            format!(
                "Working directory: {}",
                registration.target.working_directory.display()
            ),
            format!(
                "Launched pane process: pid {}, started {}",
                launch.provider_process.pid, launch.provider_process.started_at_ms
            ),
            format!(
                "Evidence: private tmux pane {}; Binding Receipt not issued",
                launch.tmux_pane_id
            ),
            format!(
                "Binding error: {}",
                registration.binding_error.as_deref().unwrap_or("unknown")
            ),
        ];
        if let Some(error) = registration.terminal_error.as_deref() {
            diagnostics.push(format!("Terminal error: {error}"));
        }
        Ok(diagnostics.join("\n"))
    }

    fn access_terminal(
        &mut self,
        registration_id: &str,
        now_ms: i64,
        action: ManagedLaunchAction,
        label: &str,
    ) -> Result<ManagedTerminalOutcome> {
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration {registration_id} is unavailable")
        })?;
        if !registration.allows(action) {
            return Err(anyhow!("{label} is unavailable for this Managed launch"));
        }
        let opened = match self.terminal_access.open(&registration) {
            Ok(()) => {
                self.store.record_terminal_opened(registration_id, now_ms)?;
                true
            }
            Err(error) => {
                self.store
                    .record_terminal_error(registration_id, &error.to_string())?;
                false
            }
        };
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration disappeared after Terminal access")
        })?;
        Ok(ManagedTerminalOutcome {
            registration,
            opened,
        })
    }
}

fn validate_tmux_name(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(anyhow!("private tmux environment is invalid"));
    }
    Ok(())
}
