//! Production composition for the refresh application service.

use crate::adapters::config::Config;
use crate::adapters::macos::notification::MacosNotification;
use crate::adapters::managed_sessions::{
    discover_tmux_executable, SystemManagedCurrentRunEvidence, SystemManagedResumeProviderEvidence,
    SystemProcessEvidence, TmuxManagedLaunchCleanup,
};
use crate::adapters::observation::IndexedObservationSource;
use crate::adapters::persistence::managed_sessions::ManagedLaunchStore;
use crate::adapters::persistence::session_metadata::SessionLabels;
use crate::adapters::persistence::tasks::TaskStore;
use crate::adapters::providers::registry;
use crate::application::alerts::AlertEngine;
use crate::application::managed_sessions::{
    EvidenceBinder, ManagedCurrentRunVerifier, ManagedLaunchRegistration, ManagedResumeVerifier,
    ProviderBindingEvidence, SystemManagedResumeEvidence,
};
use crate::application::observation::refresh::{
    Clock, ManagedBindingEvidenceSource, RefreshPorts, RefreshService,
};

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Local::now().timestamp_millis()
    }
}

pub struct RegistryManagedBindingEvidenceSource {
    config: Config,
}

impl RegistryManagedBindingEvidenceSource {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl ManagedBindingEvidenceSource for RegistryManagedBindingEvidenceSource {
    fn evidence(
        &self,
        registration: &ManagedLaunchRegistration,
    ) -> Option<ProviderBindingEvidence> {
        let process = registration.launch_evidence.as_ref()?.provider_process;
        registry()
            .runtime_control(registration.provider)
            .ok()?
            .binding_evidence(&self.config, process)
    }
}

pub fn open_refresh_service(config: Config) -> RefreshService {
    let task_store = TaskStore::open(&config.task_store_path())
        .map(|store| Box::new(store) as Box<_>)
        .map_err(|error| error.to_string());
    let managed_store = ManagedLaunchStore::open(config.managed_launch_store_path())
        .map(|store| Box::new(store) as Box<_>)
        .map_err(|error| error.to_string());
    let session_metadata = Ok(Box::new(SessionLabels::load(config.label_store_path())) as Box<_>);
    let tmux_executable = discover_tmux_executable().unwrap_or_default();
    RefreshService::new(RefreshPorts {
        observations: Box::new(IndexedObservationSource::new(config.clone())),
        task_store,
        managed_store,
        session_metadata,
        binding_evidence: Box::new(RegistryManagedBindingEvidenceSource::new(config.clone())),
        managed_binding_reconciler: EvidenceBinder::new(
            Box::new(SystemProcessEvidence),
            Box::new(TmuxManagedLaunchCleanup::new(tmux_executable)),
        ),
        managed_current_run_verifier: ManagedCurrentRunVerifier::new(Box::new(
            SystemManagedCurrentRunEvidence::new(config.clone()),
        )),
        managed_resume_verifier: ManagedResumeVerifier::new(Box::new(
            SystemManagedResumeEvidence::new(
                Box::new(SystemProcessEvidence),
                Box::new(SystemManagedResumeProviderEvidence::new(config.clone())),
            ),
        )),
        alert_engine: AlertEngine::new(
            Box::new(SessionLabels::load(config.label_store_path())),
            Box::new(MacosNotification),
        ),
        clock: Box::new(SystemClock),
    })
}
