use crate::adapters::config::Config;
use crate::adapters::providers::{registry, state};
use crate::adapters::system::process::SystemProcessEvidence;
use crate::application::managed_session::{
    ManagedCurrentRunEvidence, ManagedResumeEvidenceCandidate, ManagedResumeProviderEvidence,
    ProcessEvidence, ProviderProcessObservation,
};
use crate::domain::{
    BindingReceipt, ManagedCurrentRunLifecycle, ManagedCurrentRunRecord, ManagedSessionRecord,
    SessionState,
};
use anyhow::{anyhow, Result};
use std::path::Path;

pub struct SystemManagedResumeProviderEvidence {
    config: Config,
}

impl SystemManagedResumeProviderEvidence {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    fn observe_with_detector(
        detector: &state::StateDetector,
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
            || !detector.scan_complete(run.provider)
        {
            return Ok(ManagedCurrentRunLifecycle::Unknown);
        }
        let live = registry()
            .runtime_control(run.provider)?
            .observe_current_run(detector, &run.native_session_id, &source);
        if live.state.is_open() {
            Ok(ManagedCurrentRunLifecycle::Open)
        } else if live.state == SessionState::Exited && live.process_identity.is_none() {
            Ok(ManagedCurrentRunLifecycle::Exited)
        } else {
            Ok(ManagedCurrentRunLifecycle::Unknown)
        }
    }
}

impl ManagedResumeProviderEvidence for SystemManagedResumeProviderEvidence {
    fn observe(
        &self,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle> {
        let detector = state::StateDetector::gather(&self.config.claude_sessions_dir());
        Self::observe_with_detector(&detector, run, session, receipt)
    }

    fn observe_many(
        &self,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let detector = state::StateDetector::gather(&self.config.claude_sessions_dir());
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

pub struct SystemManagedCurrentRunEvidence {
    config: Config,
    processes: SystemProcessEvidence,
}

impl SystemManagedCurrentRunEvidence {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            processes: SystemProcessEvidence,
        }
    }
}

impl ManagedCurrentRunEvidence for SystemManagedCurrentRunEvidence {
    fn revalidate(
        &self,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<()> {
        match self.processes.observe(run.provider_process.pid) {
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
            .binding_evidence(&self.config, run.provider_process)
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
