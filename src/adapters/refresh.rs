//! Production composition for the four-stage background refresh cycle.

use crate::adapters::config::Config;
use crate::adapters::handoff::{
    open_handoff_service_with_current_run_control, MacosCurrentRunControl,
    ReconciliationOnlyLauncher,
};
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
use crate::application::handoff::{HandoffPaths, ProviderExecutables};
use crate::application::managed_sessions::{
    EvidenceBinder, ManagedCurrentRunVerifier, ManagedLaunchRegistration, ManagedResumeVerifier,
    ProviderBindingEvidence, SystemManagedResumeEvidence,
};
use crate::application::observation::read_model::{
    InMemoryReadModelCache, ManagedCurrentRunSummary, ManagedLaunchSummary, ManagedResumeSummary,
    ReadModelCache,
};
use crate::application::observation::{
    Clock, CollectPorts, CollectService, ManagedBindingEvidenceSource, ManagedObservationQuery,
    ManagedProjectionQuery, OperationTaskQuery, ProjectPorts, ProjectionService,
    PublicationService, ReconciliationPorts, ReconciliationService, RefreshCycle,
    RefreshCyclePorts,
};
use crate::application::tasks::OperationTask;
use crate::domain::Session;
use std::sync::Arc;

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

pub fn open_refresh_cycle(config: Config) -> RefreshCycle {
    open_refresh_cycle_with_cache(config, Arc::new(InMemoryReadModelCache::default()))
}

pub fn open_refresh_cycle_with_cache(
    config: Config,
    cache: Arc<dyn ReadModelCache>,
) -> RefreshCycle {
    let collect_managed_query = ManagedLaunchStore::open(config.managed_launch_store_path())
        .map(|store| Box::new(store) as Box<dyn ManagedObservationQuery>)
        .map_err(|error| error.to_string());
    let reconcile_managed_repository = open_managed_repository(&config);
    let project_managed_query = ManagedLaunchStore::open(config.managed_launch_store_path())
        .map(|store| Box::new(store) as Box<dyn ManagedProjectionQuery>)
        .map_err(|error| error.to_string());
    let task_query = TaskStore::open(&config.task_store_path())
        .map(|store| Box::new(store) as Box<dyn OperationTaskQuery>)
        .map_err(|error| error.to_string());
    let task_reconciler = open_task_reconciler(&config);
    let tmux_executable = discover_tmux_executable().unwrap_or_default();
    let managed_binding_reconciler = EvidenceBinder::new(
        Box::new(SystemProcessEvidence),
        Box::new(TmuxManagedLaunchCleanup::new(tmux_executable)),
    );
    let binding_process_evidence = managed_binding_reconciler.process_evidence();

    let collector = CollectService::new(CollectPorts {
        observations: Box::new(IndexedObservationSource::new(config.clone())),
        managed_query: collect_managed_query,
        binding_evidence: Box::new(RegistryManagedBindingEvidenceSource::new(config.clone())),
        process_evidence: binding_process_evidence,
        managed_current_run_verifier: ManagedCurrentRunVerifier::new(Box::new(
            SystemManagedCurrentRunEvidence::new(config.clone()),
        )),
        managed_resume_verifier: ManagedResumeVerifier::new(Box::new(
            SystemManagedResumeEvidence::new(
                Box::new(SystemProcessEvidence),
                Box::new(SystemManagedResumeProviderEvidence::new(config.clone())),
            ),
        )),
    });
    let reconciler = ReconciliationService::new(ReconciliationPorts {
        managed_repository: reconcile_managed_repository,
        managed_binding_reconciler,
        task_reconciler,
    });
    let projector = ProjectionService::new(ProjectPorts {
        task_query,
        managed_query: project_managed_query,
    });
    let publisher = PublicationService::new(
        cache,
        AlertEngine::new(
            Box::new(SessionLabels::load(config.label_store_path())),
            Box::new(MacosNotification),
        ),
    );

    RefreshCycle::new(RefreshCyclePorts {
        collector,
        reconciler,
        projector,
        publisher,
        clock: Box::new(SystemClock),
    })
}

fn open_managed_repository(
    config: &Config,
) -> std::result::Result<
    Box<dyn crate::application::managed_sessions::ManagedSessionRepository>,
    String,
> {
    ManagedLaunchStore::open(config.managed_launch_store_path())
        .map(|store| Box::new(store) as Box<_>)
        .map_err(|error| error.to_string())
}

impl ManagedObservationQuery for ManagedLaunchStore {
    fn binding_registrations(&self) -> anyhow::Result<Vec<ManagedLaunchRegistration>> {
        ManagedLaunchStore::binding_registrations(self)
    }

    fn verified_managed_current_runs(
        &self,
        verifier: &ManagedCurrentRunVerifier,
    ) -> anyhow::Result<Vec<ManagedCurrentRunSummary>> {
        verifier
            .verified_runs(self)
            .map(|runs| runs.iter().map(ManagedCurrentRunSummary::from).collect())
    }

    fn resumable_managed_sessions(
        &self,
        verifier: &ManagedResumeVerifier,
    ) -> anyhow::Result<Vec<ManagedResumeSummary>> {
        verifier
            .verified_exited_sessions(self)
            .map(|sessions| sessions.iter().map(ManagedResumeSummary::from).collect())
    }
}

impl ManagedProjectionQuery for ManagedLaunchStore {
    fn apply_provenance(&self, sessions: &mut [Session]) -> anyhow::Result<()> {
        ManagedLaunchStore::apply_provenance(self, sessions)
    }

    fn managed_launches(&self) -> anyhow::Result<Vec<ManagedLaunchSummary>> {
        ManagedLaunchStore::registrations(self).map(|registrations| {
            registrations
                .iter()
                .map(ManagedLaunchSummary::from)
                .collect()
        })
    }
}

impl OperationTaskQuery for TaskStore {
    fn tasks(&self) -> anyhow::Result<Vec<OperationTask>> {
        TaskStore::list(self)
    }
}

fn open_task_reconciler(
    config: &Config,
) -> std::result::Result<Box<dyn crate::application::observation::OperationTaskReconciler>, String>
{
    let paths = HandoffPaths {
        task_store: config.task_store_path(),
        artifact_root: config.artifact_root(),
    };
    open_handoff_service_with_current_run_control(
        paths,
        ProviderExecutables::discover(),
        ReconciliationOnlyLauncher,
        Arc::new(MacosCurrentRunControl::new(config.claude_sessions_dir())),
    )
    .map(|service| Box::new(service) as Box<_>)
    .map_err(|error| error.to_string())
}
