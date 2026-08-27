//! Explicitly authorized Managed Session launch registration and native New startup.

use crate::adapters::persistence::state::StateDatabase;
use crate::adapters::providers::state::ProcessIdentity;
use crate::adapters::providers::{registry, ProviderBindingEvidence, ProviderExecutables};
use crate::domain::{
    Provider, Session, SessionIdentity, SessionProvenance, SourceFormat, SourceMetadata,
};
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static REGISTRATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

    fn parse(value: &str) -> Result<Self> {
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
        config: &crate::adapters::config::Config,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle>;

    fn observe_many(
        &self,
        config: &crate::adapters::config::Config,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        candidates
            .iter()
            .map(|candidate| {
                self.observe(
                    config,
                    &candidate.run,
                    &candidate.session,
                    &candidate.receipt,
                )
            })
            .collect()
    }
}

pub trait ManagedResumeProviderEvidence: Send + Sync {
    fn observe(
        &self,
        config: &crate::adapters::config::Config,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle>;

    fn observe_many(
        &self,
        config: &crate::adapters::config::Config,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        candidates
            .iter()
            .map(|candidate| {
                self.observe(
                    config,
                    &candidate.run,
                    &candidate.session,
                    &candidate.receipt,
                )
            })
            .collect()
    }
}

pub struct SystemManagedResumeProviderEvidence;

impl SystemManagedResumeProviderEvidence {
    fn observe_with_detector(
        detector: &crate::adapters::providers::state::StateDetector,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle> {
        let source = match receipt.source.path.canonicalize() {
            Ok(source) if source.is_file() => source,
            _ => return Ok(ManagedCurrentRunLifecycle::Unknown),
        };
        let parsed = match registry().observation(run.provider)?.parse_file(&source) {
            Some(parsed) => parsed,
            None => return Ok(ManagedCurrentRunLifecycle::Unknown),
        };
        let parsed_working_directory = parsed
            .cwd
            .as_deref()
            .and_then(|cwd| Path::new(cwd).canonicalize().ok());
        let target_working_directory = session.target.working_directory.canonicalize().ok();
        if parsed.provider != run.provider
            || parsed.id != run.native_session_id
            || parsed_working_directory.is_none()
            || parsed_working_directory != target_working_directory
        {
            return Ok(ManagedCurrentRunLifecycle::Unknown);
        }
        if !detector.scan_complete(run.provider) {
            return Ok(ManagedCurrentRunLifecycle::Unknown);
        }
        let live = registry()
            .runtime_control(run.provider)?
            .observe_current_run(detector, &run.native_session_id, &source);
        if live.state.is_open() {
            Ok(ManagedCurrentRunLifecycle::Open)
        } else if live.state == crate::domain::SessionState::Exited
            && live.process_identity.is_none()
        {
            Ok(ManagedCurrentRunLifecycle::Exited)
        } else {
            Ok(ManagedCurrentRunLifecycle::Unknown)
        }
    }
}

impl ManagedResumeProviderEvidence for SystemManagedResumeProviderEvidence {
    fn observe(
        &self,
        config: &crate::adapters::config::Config,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle> {
        let detector =
            crate::adapters::providers::state::StateDetector::gather(&config.claude_sessions_dir());
        Self::observe_with_detector(&detector, run, session, receipt)
    }

    fn observe_many(
        &self,
        config: &crate::adapters::config::Config,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let detector =
            crate::adapters::providers::state::StateDetector::gather(&config.claude_sessions_dir());
        candidates
            .iter()
            .map(|candidate| {
                Self::observe_with_detector(
                    &detector,
                    &candidate.run,
                    &candidate.session,
                    &candidate.receipt,
                )
            })
            .collect()
    }
}

pub struct SystemManagedResumeEvidence {
    process_evidence: Box<dyn ProcessEvidence>,
    provider_evidence: Box<dyn ManagedResumeProviderEvidence>,
}

impl Default for SystemManagedResumeEvidence {
    fn default() -> Self {
        Self::new(Box::new(SystemProcessEvidence))
    }
}

impl SystemManagedResumeEvidence {
    pub fn new(process_evidence: Box<dyn ProcessEvidence>) -> Self {
        Self {
            process_evidence,
            provider_evidence: Box::new(SystemManagedResumeProviderEvidence),
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
        config: &crate::adapters::config::Config,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle> {
        let provider_lifecycle = self
            .provider_evidence
            .observe(config, run, session, receipt)?;
        let prior_process = self.process_evidence.observe(run.provider_process.pid);
        combine_resume_evidence(run, session, receipt, provider_lifecycle, prior_process)
    }

    fn observe_many(
        &self,
        config: &crate::adapters::config::Config,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let provider_lifecycles = self.provider_evidence.observe_many(config, candidates);
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
    config: crate::adapters::config::Config,
    evidence: Box<dyn ManagedResumeEvidence>,
}

impl ManagedResumeVerifier {
    pub fn new(
        config: crate::adapters::config::Config,
        evidence: Box<dyn ManagedResumeEvidence>,
    ) -> Self {
        Self { config, evidence }
    }

    pub fn verify_exited(
        &self,
        store: &ManagedLaunchStore,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<VerifiedExitedManagedSession> {
        let candidate = self.evidence_candidate(store, provider, native_session_id)?;
        let lifecycle = self.evidence.observe(
            &self.config,
            &candidate.run,
            &candidate.session,
            &candidate.receipt,
        )?;
        verified_exited_session(candidate, lifecycle)
    }

    fn evidence_candidate(
        &self,
        store: &ManagedLaunchStore,
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
        store: &ManagedLaunchStore,
    ) -> Result<Vec<VerifiedExitedManagedSession>> {
        let candidates = store
            .managed_sessions()?
            .into_iter()
            .filter_map(|session| {
                self.evidence_candidate(store, session.provider, &session.native_session_id)
                    .ok()
            })
            .collect::<Vec<_>>();
        let observations = self.evidence.observe_many(&self.config, &candidates);
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
        config: &crate::adapters::config::Config,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<()>;
}

pub struct SystemManagedCurrentRunEvidence;

impl ManagedCurrentRunEvidence for SystemManagedCurrentRunEvidence {
    fn revalidate(
        &self,
        config: &crate::adapters::config::Config,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<()> {
        match SystemProcessEvidence.observe(run.provider_process.pid) {
            ProviderProcessObservation::Live(identity) if identity == run.provider_process => {}
            ProviderProcessObservation::Live(_) | ProviderProcessObservation::Absent => {
                return Err(anyhow!("Managed Current Run process identity is stale"));
            }
            ProviderProcessObservation::Unknown => {
                return Err(anyhow!(
                    "Managed Current Run process identity could not be verified"
                ));
            }
        }
        let evidence = registry()
            .runtime_control(run.provider)?
            .binding_evidence(config, run.provider_process)
            .ok_or_else(|| anyhow!("Managed Current Run is not verified Open"))?;
        if evidence.provider != run.provider
            || evidence.source_session_id != run.native_session_id
            || evidence.process_session_id != run.native_session_id
            || evidence.provider_process != run.provider_process
            || evidence.source != receipt.source
            || evidence.working_directory != session.target.working_directory
        {
            return Err(anyhow!(
                "Managed Current Run live evidence no longer matches its Binding Receipt"
            ));
        }
        Ok(())
    }
}

/// A revalidated target. Private terminal addressing intentionally has no public accessor.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedManagedCurrentRun {
    pub opaque_ref: String,
    pub provider: Provider,
    pub native_session_id: String,
    tmux: PrivateTmuxEnvironment,
}

pub struct ManagedCurrentRunVerifier {
    config: crate::adapters::config::Config,
    evidence: Box<dyn ManagedCurrentRunEvidence>,
}

impl ManagedCurrentRunVerifier {
    pub fn new(
        config: crate::adapters::config::Config,
        evidence: Box<dyn ManagedCurrentRunEvidence>,
    ) -> Self {
        Self { config, evidence }
    }

    pub fn verify_ref(
        &self,
        store: &ManagedLaunchStore,
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
        self.evidence
            .revalidate(&self.config, run, &session, &receipt)?;
        Ok(VerifiedManagedCurrentRun {
            opaque_ref: run.opaque_ref.clone(),
            provider: run.provider,
            native_session_id: run.native_session_id.clone(),
            tmux: run.tmux.clone(),
        })
    }

    pub fn verified_runs(
        &self,
        store: &ManagedLaunchStore,
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
    store: ManagedLaunchStore,
    verifier: ManagedCurrentRunVerifier,
    terminal_access: Box<dyn ManagedCurrentRunTerminalAccess>,
}

impl ManagedCurrentRunAccessService {
    pub fn new(
        store: ManagedLaunchStore,
        verifier: ManagedCurrentRunVerifier,
        terminal_access: Box<dyn ManagedCurrentRunTerminalAccess>,
    ) -> Self {
        Self {
            store,
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
        let run = self.verifier.verify_ref(&self.store, opaque_ref)?;
        self.terminal_access.attach_foreground(&run)
    }

    fn verify_identity(
        &self,
        provider: Provider,
        native_session_id: &str,
        opaque_ref: &str,
    ) -> Result<VerifiedManagedCurrentRun> {
        let run = self.verifier.verify_ref(&self.store, opaque_ref)?;
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

pub fn run_attach_from_args<I, S>(args: I) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if args.get(1).map(OsString::as_os_str) != Some(OsStr::new("attach")) {
        return Ok(false);
    }
    let config = crate::adapters::config::Config::load();
    let tmux_executable = discover_tmux_executable().unwrap_or_default();
    let mut access = ManagedCurrentRunAccessService::new(
        ManagedLaunchStore::open(config.managed_launch_store_path())?,
        ManagedCurrentRunVerifier::new(config, Box::new(SystemManagedCurrentRunEvidence)),
        Box::new(MacosManagedTerminalAccess::new(tmux_executable)),
    );
    run_attach_from_args_with_access(args, &mut access)
}

fn valid_opaque_run_ref(value: &str) -> bool {
    value.len() == 36
        && value.starts_with("run_")
        && value[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn new_opaque_run_ref() -> Result<String> {
    let mut random = [0_u8; 16];
    std::fs::File::open("/dev/urandom")
        .context("could not create an opaque Managed Current Run reference")?
        .read_exact(&mut random)
        .context("could not create an opaque Managed Current Run reference")?;
    let encoded = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("run_{encoded}"))
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

/// Revalidates exact process identity immediately before receipt issuance.
pub trait ProcessEvidence: Send + Sync {
    fn observe(&self, pid: u32) -> ProviderProcessObservation;

    fn observe_many(&self, pids: &[u32]) -> Vec<ProviderProcessObservation> {
        pids.iter().map(|pid| self.observe(*pid)).collect()
    }
}

pub struct SystemProcessEvidence;

impl SystemProcessEvidence {
    pub fn current_identity(&self, pid: u32) -> Option<ProcessIdentity> {
        match self.observe(pid) {
            ProviderProcessObservation::Live(identity) => Some(identity),
            ProviderProcessObservation::Absent | ProviderProcessObservation::Unknown => None,
        }
    }
}

impl ProcessEvidence for SystemProcessEvidence {
    fn observe(&self, pid: u32) -> ProviderProcessObservation {
        match crate::adapters::providers::state::inspect_process_identity(pid) {
            crate::adapters::providers::state::ProcessInspection::Present(identity, false) => {
                ProviderProcessObservation::Live(identity)
            }
            crate::adapters::providers::state::ProcessInspection::Present(_, true)
            | crate::adapters::providers::state::ProcessInspection::Absent => {
                ProviderProcessObservation::Absent
            }
            crate::adapters::providers::state::ProcessInspection::Unknown => {
                ProviderProcessObservation::Unknown
            }
        }
    }

    fn observe_many(&self, pids: &[u32]) -> Vec<ProviderProcessObservation> {
        crate::adapters::providers::state::inspect_process_identities(pids)
            .into_iter()
            .map(|inspection| match inspection {
                crate::adapters::providers::state::ProcessInspection::Present(identity, false) => {
                    ProviderProcessObservation::Live(identity)
                }
                crate::adapters::providers::state::ProcessInspection::Present(_, true)
                | crate::adapters::providers::state::ProcessInspection::Absent => {
                    ProviderProcessObservation::Absent
                }
                crate::adapters::providers::state::ProcessInspection::Unknown => {
                    ProviderProcessObservation::Unknown
                }
            })
            .collect()
    }
}

pub trait ManagedLaunchCleanup: Send + Sync {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> Result<()>;
}

pub struct TmuxManagedLaunchCleanup {
    tmux_executable: PathBuf,
}

impl TmuxManagedLaunchCleanup {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self { tmux_executable }
    }
}

impl ManagedLaunchCleanup for TmuxManagedLaunchCleanup {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> Result<()> {
        clean_empty_private_tmux(&self.tmux_executable, &registration.tmux)
    }
}

/// Sole issuer of Binding Receipts.
pub struct EvidenceBinder {
    process_evidence: Box<dyn ProcessEvidence>,
    cleanup: Box<dyn ManagedLaunchCleanup>,
}

impl EvidenceBinder {
    pub fn new(
        process_evidence: Box<dyn ProcessEvidence>,
        cleanup: Box<dyn ManagedLaunchCleanup>,
    ) -> Self {
        Self {
            process_evidence,
            cleanup,
        }
    }

    pub fn bind(
        &self,
        store: &mut ManagedLaunchStore,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
    ) -> Result<BindingOutcome> {
        self.bind_with_policy(store, registration_id, evidence, now_ms, true, |_, _, _| {
            Ok(LaunchIdentityAssociationOutcome::ContinueBinding)
        })
    }

    pub fn bind_with_identity_association<F>(
        &self,
        store: &mut ManagedLaunchStore,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut ManagedLaunchStore,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
        self.bind_with_policy(store, registration_id, evidence, now_ms, true, associate)
    }

    /// Reconcile a launch retained across a Mudi restart without cleanup side effects.
    pub fn reconcile_after_restart(
        &self,
        store: &mut ManagedLaunchStore,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
    ) -> Result<BindingOutcome> {
        self.bind_with_policy(
            store,
            registration_id,
            evidence,
            now_ms,
            false,
            |_, _, _| Ok(LaunchIdentityAssociationOutcome::ContinueBinding),
        )
    }

    pub fn reconcile_after_restart_with_identity_association<F>(
        &self,
        store: &mut ManagedLaunchStore,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut ManagedLaunchStore,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
        self.bind_with_policy(store, registration_id, evidence, now_ms, false, associate)
    }

    fn bind_with_policy<F>(
        &self,
        store: &mut ManagedLaunchStore,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
        clean_confirmed_absence: bool,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut ManagedLaunchStore,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
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
        match self.process_evidence.observe(launch.provider_process.pid) {
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
        match self.process_evidence.observe(evidence.provider_process.pid) {
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
    store: ManagedLaunchStore,
    executables: ProviderExecutables,
    launch_broker: Box<dyn ManagedLaunchBroker>,
    terminal_access: Box<dyn ManagedTerminalAccess>,
    resume_verifier: Option<ManagedResumeVerifier>,
}

impl ManagedSessionService {
    pub fn new(
        store: ManagedLaunchStore,
        executables: ProviderExecutables,
        launch_broker: Box<dyn ManagedLaunchBroker>,
        terminal_access: Box<dyn ManagedTerminalAccess>,
    ) -> Self {
        Self {
            store,
            executables,
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
        validate_target(&request.target)?;

        let executable = self
            .executables
            .executable_for(request.provider)
            .ok_or_else(|| anyhow!("{} executable is unavailable", request.provider.label()))?
            .to_path_buf();
        validate_executable(&executable, request.provider.label())?;
        let id = new_registration_id(now_ms);
        let registration = ManagedLaunchRegistration {
            id: id.clone(),
            status: ManagedLaunchStatus::Binding,
            provider: request.provider,
            target: request.target,
            session_alias: request.title,
            description: request.description,
            binding_initializer: request.binding_initializer,
            provider_command: ProviderNewCommand {
                executable,
                arguments: registry()
                    .runtime_control(request.provider)?
                    .new_session_args(request.binding_initializer),
            },
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
            .verify_exited(&self.store, request.provider, &request.native_session_id)?;
        let executable = self
            .executables
            .executable_for(verified.provider)
            .ok_or_else(|| anyhow!("{} executable is unavailable", verified.provider.label()))?
            .to_path_buf();
        validate_executable(&executable, verified.provider.label())?;
        let id = new_registration_id(now_ms);
        let registration = ManagedLaunchRegistration {
            id: id.clone(),
            status: ManagedLaunchStatus::Binding,
            provider: verified.provider,
            target: verified.target,
            session_alias: verified.session_alias,
            description: verified.description,
            binding_initializer: false,
            provider_command: ProviderNewCommand {
                executable,
                arguments: registry()
                    .runtime_control(verified.provider)?
                    .resume_session_args(&verified.native_session_id),
            },
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

fn validate_target(target: &ManagedLaunchTarget) -> Result<()> {
    if !target.working_directory.is_absolute() || !target.working_directory.is_dir() {
        return Err(anyhow!("New target working directory is unavailable"));
    }
    std::fs::read_dir(&target.working_directory)
        .context("New target working directory is unreadable")?;
    Ok(())
}

fn new_registration_id(_now_ms: i64) -> String {
    let sequence = REGISTRATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("launch-{:x}-{:x}-{:x}", nonce, std::process::id(), sequence)
}

pub struct ManagedLaunchStore {
    conn: Connection,
}

fn insert_registration(conn: &Connection, registration: &ManagedLaunchRegistration) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO managed_launch_registrations (
            registration_id, status, provider, working_directory, session_alias,
            description, tmux_socket_name, tmux_session_name, created_at_ms,
            launched_at_ms, tmux_pane_id, provider_pid, provider_started_at_ms,
            terminal_opened_at_ms, terminal_error, binding_error, binding_initializer
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
            ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
        )
        "#,
        params![
            registration.id,
            registration.status.as_str(),
            registration.provider.as_str(),
            path_bytes(&registration.target.working_directory),
            registration.session_alias,
            registration.description,
            registration.tmux.socket_name,
            registration.tmux.session_name,
            registration.created_at_ms,
            registration.launched_at_ms,
            registration
                .launch_evidence
                .as_ref()
                .map(|evidence| &evidence.tmux_pane_id),
            registration
                .launch_evidence
                .as_ref()
                .map(|evidence| evidence.provider_process.pid),
            registration
                .launch_evidence
                .as_ref()
                .map(|evidence| evidence.provider_process.started_at_ms),
            registration.terminal_opened_at_ms,
            registration.terminal_error,
            registration.binding_error,
            registration.binding_initializer,
        ],
    )?;
    Ok(())
}

impl ManagedLaunchStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let database = StateDatabase::open(path)?;
        let conn = database.connect()?;
        Ok(Self { conn })
    }

    pub fn insert(&self, registration: &ManagedLaunchRegistration) -> Result<()> {
        insert_registration(&self.conn, registration)
    }

    pub fn insert_resume(
        &mut self,
        registration: &ManagedLaunchRegistration,
        target: &ManagedResumeTarget,
    ) -> Result<()> {
        if registration.provider != target.provider {
            return Err(anyhow!("Resume Provider disagrees with its Registration"));
        }
        let transaction = self.conn.transaction()?;
        insert_registration(&transaction, registration)?;
        transaction.execute(
            "INSERT INTO managed_resume_registrations (registration_id, provider, native_session_id, prior_run_id) VALUES (?1, ?2, ?3, ?4)",
            params![
                registration.id,
                target.provider.as_str(),
                target.native_session_id,
                target.prior_run_id,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn managed_resume_target(
        &self,
        registration_id: &str,
    ) -> Result<Option<ManagedResumeTarget>> {
        self.conn
            .query_row(
                "SELECT provider, native_session_id, prior_run_id FROM managed_resume_registrations WHERE registration_id = ?1",
                [registration_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .map(|(provider, native_session_id, prior_run_id)| {
                Ok(ManagedResumeTarget {
                    provider: Provider::parse(&provider)
                        .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                    native_session_id,
                    prior_run_id,
                })
            })
            .transpose()
    }

    pub(crate) fn launch_title_association(
        &self,
        registration_id: &str,
    ) -> Result<Option<SessionIdentity>> {
        self.conn
            .query_row(
                "SELECT provider, native_session_id FROM managed_launch_title_associations WHERE registration_id = ?1",
                [registration_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(provider, native_id)| {
                Ok(SessionIdentity {
                    provider: Provider::parse(&provider)
                        .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                    native_id,
                })
            })
            .transpose()
    }

    pub(crate) fn record_launch_title_association(
        &self,
        registration_id: &str,
        session: &SessionIdentity,
    ) -> Result<()> {
        let inserted = self.conn.execute(
            r#"
            INSERT OR IGNORE INTO managed_launch_title_associations (
                registration_id, provider, native_session_id
            )
            SELECT registration_id, ?2, ?3
            FROM managed_launch_registrations
            WHERE registration_id = ?1 AND status = 'binding'
            "#,
            params![
                registration_id,
                session.provider.as_str(),
                session.native_id
            ],
        )?;
        let associated = self.launch_title_association(registration_id)?;
        if inserted == 1 || associated.as_ref() == Some(session) {
            Ok(())
        } else {
            Err(anyhow!(
                "Managed Launch Registration {registration_id} cannot record its Title association"
            ))
        }
    }

    fn has_pending_resume(&self, provider: Provider, native_session_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM managed_resume_registrations WHERE provider = ?1 AND native_session_id = ?2",
                params![provider.as_str(), native_session_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn get(&self, registration_id: &str) -> Result<Option<ManagedLaunchRegistration>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT status, provider, working_directory, session_alias, description,
                   tmux_socket_name, tmux_session_name, created_at_ms, launched_at_ms,
                   tmux_pane_id, provider_pid, provider_started_at_ms,
                   terminal_opened_at_ms, terminal_error, binding_error,
                   binding_initializer
            FROM managed_launch_registrations
            WHERE registration_id = ?1
            "#,
        )?;
        statement
            .query_row([registration_id], |row| {
                let status: String = row.get(0)?;
                let provider: String = row.get(1)?;
                let working_directory = path_from_bytes(row.get(2)?);
                let tmux_pane_id = row.get::<_, Option<String>>(9)?;
                let provider_pid = row.get::<_, Option<u32>>(10)?;
                let provider_started_at_ms = row.get::<_, Option<i64>>(11)?;
                Ok((
                    status,
                    provider,
                    working_directory,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    tmux_pane_id,
                    provider_pid,
                    provider_started_at_ms,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, bool>(15)?,
                ))
            })
            .optional()?
            .map(
                |(
                    status,
                    provider,
                    working_directory,
                    session_alias,
                    description,
                    tmux_socket_name,
                    tmux_session_name,
                    created_at_ms,
                    launched_at_ms,
                    tmux_pane_id,
                    provider_pid,
                    provider_started_at_ms,
                    terminal_opened_at_ms,
                    terminal_error,
                    binding_error,
                    binding_initializer,
                )| {
                    Ok(ManagedLaunchRegistration {
                        id: registration_id.to_string(),
                        status: ManagedLaunchStatus::parse(&status)?,
                        provider: Provider::parse(&provider)
                            .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                        target: ManagedLaunchTarget { working_directory },
                        session_alias,
                        description,
                        binding_initializer,
                        provider_command: ProviderNewCommand {
                            executable: PathBuf::new(),
                            arguments: Vec::new(),
                        },
                        tmux: PrivateTmuxEnvironment {
                            socket_name: tmux_socket_name,
                            session_name: tmux_session_name,
                        },
                        created_at_ms,
                        launched_at_ms,
                        launch_evidence: persisted_launch_evidence(
                            tmux_pane_id,
                            provider_pid,
                            provider_started_at_ms,
                        )?,
                        terminal_opened_at_ms,
                        terminal_error,
                        binding_error,
                    })
                },
            )
            .transpose()
    }

    pub fn binding_registrations(&self) -> Result<Vec<ManagedLaunchRegistration>> {
        let mut statement = self.conn.prepare(
            "SELECT registration_id FROM managed_launch_registrations WHERE status = 'binding' ORDER BY created_at_ms, registration_id",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        ids.into_iter()
            .map(|id| {
                self.get(&id)?
                    .ok_or_else(|| anyhow!("Binding Managed Launch Registration {id} disappeared"))
            })
            .collect()
    }

    pub fn registrations(&self) -> Result<Vec<ManagedLaunchRegistration>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT registration_id
            FROM managed_launch_registrations
            ORDER BY CASE status WHEN 'failed' THEN 1 ELSE 0 END,
                     created_at_ms DESC,
                     registration_id DESC
            "#,
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        ids.into_iter()
            .map(|id| {
                self.get(&id)?
                    .ok_or_else(|| anyhow!("Managed Launch Registration {id} disappeared"))
            })
            .collect()
    }

    pub fn binding_receipt(&self, registration_id: &str) -> Result<Option<BindingReceipt>> {
        let mut statement = self.conn.prepare(
            "SELECT receipt_id, provider, native_session_id, source_path, source_format, provider_pid, provider_started_at_ms, issued_at_ms FROM binding_receipts WHERE registration_id = ?1",
        )?;
        statement
            .query_row([registration_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .optional()?
            .map(
                |(
                    id,
                    provider,
                    native_session_id,
                    source_path,
                    source_format,
                    pid,
                    started_at_ms,
                    issued_at_ms,
                )| {
                    Ok(BindingReceipt {
                        id,
                        registration_id: registration_id.to_string(),
                        provider: Provider::parse(&provider)
                            .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                        native_session_id,
                        source: SourceMetadata {
                            path: path_from_bytes(source_path),
                            format: match source_format.as_str() {
                                "jsonl" => SourceFormat::JsonLines,
                                _ => {
                                    return Err(anyhow!("unknown source format {source_format:?}"))
                                }
                            },
                        },
                        provider_process: ProcessIdentity { pid, started_at_ms },
                        issued_at_ms,
                    })
                },
            )
            .transpose()
    }

    fn binding_receipt_by_id(&self, receipt_id: &str) -> Result<Option<BindingReceipt>> {
        let registration_id = self
            .conn
            .query_row(
                "SELECT registration_id FROM binding_receipts WHERE receipt_id = ?1",
                [receipt_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        registration_id
            .map(|registration_id| self.binding_receipt(&registration_id))
            .transpose()
            .map(Option::flatten)
    }

    pub fn managed_session(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedSessionRecord>> {
        Ok(self.managed_sessions()?.into_iter().find(|session| {
            session.provider == provider && session.native_session_id == native_session_id
        }))
    }

    pub fn managed_sessions(&self) -> Result<Vec<ManagedSessionRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT provider, native_session_id, working_directory, session_alias, description, binding_receipt_id FROM managed_sessions ORDER BY provider, native_session_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (
                provider,
                native_session_id,
                working_directory,
                session_alias,
                description,
                binding_receipt_id,
            ) = row?;
            Ok(ManagedSessionRecord {
                provider: Provider::parse(&provider)
                    .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                native_session_id,
                target: ManagedLaunchTarget {
                    working_directory: path_from_bytes(working_directory),
                },
                session_alias,
                description,
                binding_receipt_id,
            })
        })
        .collect()
    }

    pub fn managed_current_run(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedCurrentRunRecord>> {
        let authority_receipt = self
            .managed_session(provider, native_session_id)?
            .map(|session| session.binding_receipt_id);
        Ok(self.managed_current_runs()?.into_iter().find(|run| {
            run.provider == provider
                && run.native_session_id == native_session_id
                && authority_receipt.as_deref() == Some(run.binding_receipt_id.as_str())
        }))
    }

    pub fn managed_current_runs(&self) -> Result<Vec<ManagedCurrentRunRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT run_id, opaque_ref, provider, native_session_id, provider_pid, provider_started_at_ms, tmux_socket_name, tmux_session_name, binding_receipt_id FROM managed_current_runs ORDER BY run_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                opaque_ref,
                provider,
                native_session_id,
                pid,
                started_at_ms,
                socket_name,
                session_name,
                binding_receipt_id,
            ) = row?;
            Ok(ManagedCurrentRunRecord {
                id,
                opaque_ref,
                provider: Provider::parse(&provider)
                    .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                native_session_id,
                provider_process: ProcessIdentity { pid, started_at_ms },
                tmux: PrivateTmuxEnvironment {
                    socket_name,
                    session_name,
                },
                binding_receipt_id,
            })
        })
        .collect()
    }

    pub fn managed_metadata_is_synchronized(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM managed_metadata_syncs WHERE provider = ?1 AND native_session_id = ?2",
                params![provider.as_str(), native_session_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn mark_managed_metadata_synchronized(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO managed_metadata_syncs (provider, native_session_id) VALUES (?1, ?2)",
            params![provider.as_str(), native_session_id],
        )?;
        Ok(())
    }

    pub fn apply_provenance(&self, sessions: &mut [Session]) -> Result<()> {
        let managed = self
            .managed_sessions()?
            .into_iter()
            .map(|session| SessionIdentity {
                provider: session.provider,
                native_id: session.native_session_id,
            })
            .collect::<HashSet<_>>();
        for session in sessions {
            session.provenance = if managed.contains(&SessionIdentity::from(&*session)) {
                SessionProvenance::Managed
            } else {
                SessionProvenance::Observed
            };
        }
        Ok(())
    }

    fn establish_management(
        &mut self,
        registration: &ManagedLaunchRegistration,
        receipt: &BindingReceipt,
    ) -> Result<()> {
        let resume = self.managed_resume_target(&registration.id)?;
        let resume_authority = resume
            .as_ref()
            .map(|target| {
                let run = self
                    .managed_current_run(target.provider, &target.native_session_id)?
                    .ok_or_else(|| anyhow!("Resume prior Managed Current Run is unavailable"))?;
                let session = self
                    .managed_session(target.provider, &target.native_session_id)?
                    .ok_or_else(|| anyhow!("Resume Managed Session is unavailable"))?;
                if run.id != target.prior_run_id
                    || session.binding_receipt_id != run.binding_receipt_id
                    || registration.provider != target.provider
                    || receipt.provider != target.provider
                    || receipt.native_session_id != target.native_session_id
                    || registration.target != session.target
                    || registration.session_alias != session.session_alias
                    || registration.description != session.description
                {
                    return Err(anyhow!("Resume authority changed before Receipt issuance"));
                }
                Ok(run.binding_receipt_id)
            })
            .transpose()?;
        let opaque_ref = new_opaque_run_ref()?;
        let transaction = self.conn.transaction()?;
        transaction.execute(
            "INSERT INTO binding_receipts (receipt_id, registration_id, provider, native_session_id, source_path, source_format, provider_pid, provider_started_at_ms, issued_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, 'jsonl', ?6, ?7, ?8)",
            params![
                receipt.id,
                receipt.registration_id,
                receipt.provider.as_str(),
                receipt.native_session_id,
                path_bytes(&receipt.source.path),
                receipt.provider_process.pid,
                receipt.provider_process.started_at_ms,
                receipt.issued_at_ms,
            ],
        )?;
        if let Some(prior_receipt_id) = resume_authority {
            update_one(
                transaction.execute(
                    "UPDATE managed_sessions SET binding_receipt_id = ?3 WHERE provider = ?1 AND native_session_id = ?2 AND binding_receipt_id = ?4",
                    params![
                        receipt.provider.as_str(),
                        receipt.native_session_id,
                        receipt.id,
                        prior_receipt_id,
                    ],
                )?,
                &registration.id,
            )?;
        } else {
            transaction.execute(
                "INSERT INTO managed_sessions (provider, native_session_id, working_directory, session_alias, description, binding_receipt_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    receipt.provider.as_str(),
                    receipt.native_session_id,
                    path_bytes(&registration.target.working_directory),
                    registration.session_alias,
                    registration.description,
                    receipt.id,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO managed_current_runs (run_id, opaque_ref, provider, native_session_id, provider_pid, provider_started_at_ms, tmux_socket_name, tmux_session_name, binding_receipt_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                format!("run-{}", registration.id),
                opaque_ref,
                receipt.provider.as_str(),
                receipt.native_session_id,
                receipt.provider_process.pid,
                receipt.provider_process.started_at_ms,
                registration.tmux.socket_name,
                registration.tmux.session_name,
                receipt.id,
            ],
        )?;
        transaction.execute(
            "DELETE FROM managed_resume_registrations WHERE registration_id = ?1",
            [registration.id.as_str()],
        )?;
        transaction.execute(
            "DELETE FROM managed_launch_title_associations WHERE registration_id = ?1",
            [registration.id.as_str()],
        )?;
        update_one(
            transaction.execute(
                "DELETE FROM managed_launch_registrations WHERE registration_id = ?1 AND status = 'binding'",
                [registration.id.as_str()],
            )?,
            &registration.id,
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn record_launched(
        &self,
        registration_id: &str,
        now_ms: i64,
        evidence: ManagedLaunchEvidence,
    ) -> Result<()> {
        update_one(
            self.conn.execute(
                r#"
                UPDATE managed_launch_registrations
                SET launched_at_ms = ?2,
                    tmux_pane_id = ?3,
                    provider_pid = ?4,
                    provider_started_at_ms = ?5
                WHERE registration_id = ?1
                  AND launched_at_ms IS NULL
                  AND tmux_pane_id IS NULL
                  AND provider_pid IS NULL
                  AND provider_started_at_ms IS NULL
                "#,
                params![
                    registration_id,
                    now_ms,
                    evidence.tmux_pane_id,
                    evidence.provider_process.pid,
                    evidence.provider_process.started_at_ms,
                ],
            )?,
            registration_id,
        )
    }

    fn record_terminal_opened(&self, registration_id: &str, now_ms: i64) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET terminal_opened_at_ms = ?2, terminal_error = NULL WHERE registration_id = ?1 AND launched_at_ms IS NOT NULL",
                params![registration_id, now_ms],
            )?,
            registration_id,
        )
    }

    fn record_terminal_error(&self, registration_id: &str, error: &str) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET terminal_error = ?2 WHERE registration_id = ?1 AND launched_at_ms IS NOT NULL",
                params![registration_id, error],
            )?,
            registration_id,
        )
    }

    fn remove_binding(&mut self, registration_id: &str) -> Result<()> {
        let transaction = self.conn.transaction()?;
        transaction.execute(
            "DELETE FROM managed_resume_registrations WHERE registration_id = ?1",
            [registration_id],
        )?;
        transaction.execute(
            "DELETE FROM managed_launch_title_associations WHERE registration_id = ?1",
            [registration_id],
        )?;
        update_one(
            transaction.execute(
                "DELETE FROM managed_launch_registrations WHERE registration_id = ?1 AND status = 'binding'",
                [registration_id],
            )?,
            registration_id,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn mark_needs_review(&self, registration_id: &str, error: &str) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET status = 'needs_review', binding_error = ?2 WHERE registration_id = ?1 AND status = 'binding'",
                params![registration_id, error],
            )?,
            registration_id,
        )
    }

    fn mark_failed(&mut self, registration_id: &str, error: &str) -> Result<()> {
        let transaction = self.conn.transaction()?;
        update_one(
            transaction.execute(
                "UPDATE managed_launch_registrations SET status = 'failed', binding_error = ?2 WHERE registration_id = ?1 AND status = 'binding'",
                params![registration_id, error],
            )?,
            registration_id,
        )?;
        transaction.execute(
            "DELETE FROM managed_resume_registrations WHERE registration_id = ?1",
            [registration_id],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn persisted_launch_evidence(
    tmux_pane_id: Option<String>,
    provider_pid: Option<u32>,
    provider_started_at_ms: Option<i64>,
) -> Result<Option<ManagedLaunchEvidence>> {
    match (tmux_pane_id, provider_pid, provider_started_at_ms) {
        (Some(tmux_pane_id), Some(pid), Some(started_at_ms)) => Ok(Some(ManagedLaunchEvidence {
            tmux_pane_id,
            provider_process: ProcessIdentity { pid, started_at_ms },
        })),
        (None, None, None) => Ok(None),
        _ => Err(anyhow!("incomplete persisted Managed launch evidence")),
    }
}

fn update_one(changed: usize, registration_id: &str) -> Result<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(anyhow!(
            "Managed Launch Registration {registration_id} is unavailable or in the wrong state"
        ))
    }
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}

pub struct TmuxLaunchBroker {
    tmux_executable: PathBuf,
}

impl TmuxLaunchBroker {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self { tmux_executable }
    }
}

impl ManagedLaunchBroker for TmuxLaunchBroker {
    fn launch(
        &mut self,
        registration: &ManagedLaunchRegistration,
    ) -> Result<ManagedLaunchEvidence> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_executable(
            &registration.provider_command.executable,
            registration.provider.label(),
        )?;
        validate_tmux_name(&registration.tmux.socket_name)?;
        validate_tmux_name(&registration.tmux.session_name)?;
        let terminal_title = escape_tmux_format(&registration.session_alias);
        let window_title = tmux_window_title(
            &registration.session_alias,
            registration.description.as_deref(),
        );
        let output = Command::new(&self.tmux_executable)
            .env_remove("TMUX")
            .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
            .arg(&registration.tmux.socket_name)
            .args([
                OsStr::new("new-session"),
                OsStr::new("-d"),
                OsStr::new("-P"),
                OsStr::new("-F"),
                OsStr::new("#{pane_id}\t#{pane_pid}"),
                OsStr::new("-s"),
            ])
            .arg(&registration.tmux.session_name)
            .arg("-n")
            .arg(window_title)
            .arg("-c")
            .arg(&registration.target.working_directory)
            .arg("--")
            .arg(&registration.provider_command.executable)
            .args(&registration.provider_command.arguments)
            .arg(";")
            .args(["set-option", "-w", "automatic-rename", "off"])
            .arg(";")
            .args(["set-option", "-g", "set-titles", "on"])
            .arg(";")
            .args(["set-option", "-g", "set-titles-string"])
            .arg(&terminal_title)
            .arg(";")
            .args(["set-option", "-g", "mouse", "on"])
            .arg(";")
            .args(["set-option", "-s", "copy-command", "/usr/bin/pbcopy"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .context("could not start private tmux")?;
        if !output.status.success() {
            return Err(anyhow!("private tmux launch exited with {}", output.status));
        }
        let output = String::from_utf8(output.stdout)
            .map_err(|_| anyhow!("private tmux launch evidence is not valid UTF-8"))?;
        let (tmux_pane_id, pane_pid) = output
            .trim()
            .split_once('\t')
            .ok_or_else(|| anyhow!("private tmux launch evidence is incomplete"))?;
        validate_tmux_pane_id(tmux_pane_id)?;
        let pane_pid = pane_pid
            .parse::<u32>()
            .map_err(|_| anyhow!("private tmux Provider PID is invalid"))?;
        let provider_process =
            crate::adapters::providers::state::process_identity_after_spawn(pane_pid)
                .ok_or_else(|| anyhow!("private tmux Provider process identity is unavailable"))?;
        Ok(ManagedLaunchEvidence {
            tmux_pane_id: tmux_pane_id.to_string(),
            provider_process,
        })
    }

    fn cancel(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        terminate_private_tmux(&self.tmux_executable, &registration.tmux)
    }
}

fn escape_tmux_format(value: &str) -> String {
    value.replace('#', "##")
}

fn tmux_window_title(title: &str, description: Option<&str>) -> String {
    escape_tmux_format(description.unwrap_or(title))
}

fn terminate_private_tmux(
    tmux_executable: &Path,
    environment: &PrivateTmuxEnvironment,
) -> Result<()> {
    validate_executable(tmux_executable, "tmux")?;
    validate_tmux_name(&environment.socket_name)?;
    validate_tmux_name(&environment.session_name)?;
    let status = Command::new(tmux_executable)
        .env_remove("TMUX")
        .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
        .arg(&environment.socket_name)
        .arg("kill-server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not clean private tmux")?;
    if status.success() || status.code() == Some(1) {
        Ok(())
    } else {
        Err(anyhow!("private tmux cleanup failed"))
    }
}

fn clean_empty_private_tmux(
    tmux_executable: &Path,
    environment: &PrivateTmuxEnvironment,
) -> Result<()> {
    validate_executable(tmux_executable, "tmux")?;
    validate_tmux_name(&environment.socket_name)?;
    validate_tmux_name(&environment.session_name)?;
    let probe = Command::new(tmux_executable)
        .env_remove("TMUX")
        .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
        .arg(&environment.socket_name)
        .args([OsStr::new("has-session"), OsStr::new("-t")])
        .arg(&environment.session_name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not inspect private tmux")?;
    if probe.code() == Some(1) {
        Ok(())
    } else if probe.success() {
        Err(anyhow!("private tmux environment is not empty"))
    } else {
        Err(anyhow!("private tmux environment could not be inspected"))
    }
}

pub struct MacosManagedTerminalAccess {
    tmux_executable: PathBuf,
    osascript_executable: PathBuf,
}

impl MacosManagedTerminalAccess {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self::with_osascript(tmux_executable, PathBuf::from("/usr/bin/osascript"))
    }

    pub fn with_osascript(tmux_executable: PathBuf, osascript_executable: PathBuf) -> Self {
        Self {
            tmux_executable,
            osascript_executable,
        }
    }

    fn open_private_tmux(&self, tmux: &PrivateTmuxEnvironment) -> Result<()> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_tmux_name(&tmux.socket_name)?;
        validate_tmux_name(&tmux.session_name)?;
        let command = [
            shell_quote_os(self.tmux_executable.as_os_str())?,
            "-L".to_string(),
            shell_quote(&tmux.socket_name),
            "attach-session".to_string(),
            "-t".to_string(),
            shell_quote(&tmux.session_name),
        ]
        .join(" ");
        let status = Command::new(&self.osascript_executable)
            .args([
                OsStr::new("-e"),
                OsStr::new("on run argv"),
                OsStr::new("-e"),
                OsStr::new("tell application \"Terminal\""),
                OsStr::new("-e"),
                OsStr::new("do script (item 1 of argv)"),
                OsStr::new("-e"),
                OsStr::new("activate"),
                OsStr::new("-e"),
                OsStr::new("end tell"),
                OsStr::new("-e"),
                OsStr::new("end run"),
                OsStr::new("--"),
                OsStr::new(&command),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("could not open Terminal.app")?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("Terminal.app launch failed"))
        }
    }
}

impl ManagedTerminalAccess for MacosManagedTerminalAccess {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        self.open_private_tmux(&registration.tmux)
    }
}

impl ManagedCurrentRunTerminalAccess for MacosManagedTerminalAccess {
    fn open_terminal(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()> {
        self.open_private_tmux(&run.tmux)
    }

    fn attach_foreground(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_tmux_name(&run.tmux.socket_name)?;
        validate_tmux_name(&run.tmux.session_name)?;
        let status = Command::new(&self.tmux_executable)
            .env_remove("TMUX")
            .args([
                OsStr::new("-L"),
                OsStr::new(&run.tmux.socket_name),
                OsStr::new("attach-session"),
                OsStr::new("-t"),
                OsStr::new(&run.tmux.session_name),
            ])
            .status()
            .context("could not attach to the Managed Current Run")?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("Managed Current Run attach failed"))
        }
    }
}

pub fn discover_tmux_executable() -> Option<PathBuf> {
    crate::adapters::config::configured_path([
        "MUDI_TMUX_EXECUTABLE",
        "SESSIONMESH_TMUX_EXECUTABLE",
    ])
    .filter(|path| validate_executable(path, "tmux").is_ok())
    .or_else(|| {
        std::env::var_os("PATH")
            .as_deref()
            .map(std::env::split_paths)
            .into_iter()
            .flatten()
            .map(|directory| directory.join("tmux"))
            .find(|path| validate_executable(path, "tmux").is_ok())
    })
}

fn validate_executable(path: &Path, name: &str) -> Result<()> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("{name} executable is unavailable"))?;
    if !path.is_absolute() || !metadata.is_file() {
        return Err(anyhow!("{name} executable is unavailable"));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(anyhow!("{name} executable is unavailable"));
    }
    Ok(())
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

fn validate_tmux_pane_id(value: &str) -> Result<()> {
    if value.strip_prefix('%').is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        Ok(())
    } else {
        Err(anyhow!("private tmux pane identity is invalid"))
    }
}

fn shell_quote_os(value: &OsStr) -> Result<String> {
    value
        .to_str()
        .map(shell_quote)
        .ok_or_else(|| anyhow!("tmux executable path is not valid UTF-8"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::tmux_window_title;

    #[test]
    fn tmux_window_title_falls_back_to_the_session_title() {
        assert_eq!(tmux_window_title("Title #S", None), "Title ##S".to_string());
    }
}
