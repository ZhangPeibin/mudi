use mudi::adapters::persistence::session_metadata::SessionLabels;
use mudi::adapters::persistence::state::managed_sessions::StateManagedSessionRepository;
use mudi::application::alerts::{AlertEngine, NotificationPort};
use mudi::application::managed_session::{
    BindingProcessObservation, BindingReceipt, ManagedControlChannel, ManagedCurrentRunEvidence,
    ManagedCurrentRunRecord, ManagedCurrentRunVerifier, ManagedLaunchCleanup,
    ManagedLaunchEvidence, ManagedLaunchReconciler, ManagedLaunchRegistration, ManagedLaunchStatus,
    ManagedLaunchTarget, ManagedResumeEvidence, ManagedResumeEvidenceCandidate,
    ManagedResumeVerifier, ManagedSessionRecord, ProcessEvidence, ProviderBindingEvidence,
    ProviderProcessObservation,
};
use mudi::application::observation::read_model::{
    InMemoryReadModelCache, ManagedCurrentRunSummary, ManagedLaunchSummary, ManagedResumeSummary,
    ReadModelCache, RefreshDiagnostic, RefreshHealth, RefreshSnapshot, RefreshStage, ScanStats,
};
use mudi::application::observation::{
    CollectPorts, CollectService, ManagedBindingEvidenceSource, ManagedBindingObservation,
    ManagedObservationQuery, ManagedProjectionQuery, ObservationFrame, OperationTaskQuery,
    OperationTaskReconciler, ProjectPorts, ProjectionService, PublicationService,
    ReconciliationPorts, ReconciliationReport, ReconciliationService, SessionObservationBatch,
    SessionObservationSource,
};
use mudi::application::tasks::OperationTask;
use mudi::domain::{ProcessIdentity, Provider, Session, SourceFormat, SourceMetadata};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn collect_uses_only_read_ports_and_bulk_samples_process_evidence_once() {
    let reads = Arc::new(AtomicUsize::new(0));
    let process_samples = Arc::new(AtomicUsize::new(0));
    let registration = registration("collect-existing", Path::new("/tmp"), process(42, 41_000));
    let mut collector = CollectService::new(CollectPorts {
        observations: Box::new(FixedObservations),
        managed_query: Ok(Box::new(RecordingObservationQuery {
            reads: reads.clone(),
            registration,
        })),
        binding_evidence: Box::new(NoBindingEvidence),
        process_evidence: Arc::new(RecordingProcessEvidence {
            samples: process_samples.clone(),
            started_at_ms: 41_000,
        }),
        managed_current_run_verifier: ManagedCurrentRunVerifier::new(Box::new(NeverCurrentRun)),
        managed_resume_verifier: ManagedResumeVerifier::new(Box::new(NeverResume)),
    });

    let frame = collector.collect(50_000);

    assert_eq!(frame.observed_at_ms, 50_000);
    assert_eq!(frame.stats.discovered, 1);
    assert_eq!(reads.load(Ordering::Relaxed), 3);
    assert_eq!(process_samples.load(Ordering::Relaxed), 1);
    assert_eq!(frame.managed_bindings.len(), 1);
    assert_eq!(
        frame.managed_bindings[0].processes.launch_process,
        ProviderProcessObservation::Live(process(42, 41_000))
    );
    assert!(frame
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.stage == RefreshStage::Collect));
}

#[test]
fn project_uses_only_read_ports_and_preserves_stage_diagnostics() {
    let managed_reads = Arc::new(AtomicUsize::new(0));
    let task_reads = Arc::new(AtomicUsize::new(0));
    let mut projector = ProjectionService::new(ProjectPorts {
        managed_query: Ok(Box::new(RecordingProjectionQuery {
            reads: managed_reads.clone(),
        })),
        task_query: Ok(Box::new(RecordingTaskQuery {
            reads: task_reads.clone(),
        })),
        session_metadata: Ok(Box::new(
            mudi::application::session_labels::SessionMetadataView::default(),
        )),
    });
    let mut frame = empty_frame(50_000);
    frame.diagnostics.push(RefreshDiagnostic::for_stage(
        RefreshStage::Collect,
        "Provider Source",
        "malformed record was skipped",
    ));
    let reconciliation = ReconciliationReport {
        diagnostics: vec![RefreshDiagnostic::for_stage(
            RefreshStage::Reconcile,
            "Managed Binding",
            "evidence remained ambiguous",
        )],
    };

    let snapshot = projector.project(&frame, reconciliation);

    assert_eq!(managed_reads.load(Ordering::Relaxed), 2);
    assert_eq!(task_reads.load(Ordering::Relaxed), 1);
    let RefreshHealth::Degraded { diagnostics } = snapshot.health else {
        panic!("stage failures must produce a degraded projection");
    };
    assert_eq!(diagnostics[0].stage, RefreshStage::Collect);
    assert_eq!(diagnostics[1].stage, RefreshStage::Reconcile);
}

#[test]
fn reconciliation_cannot_create_authority_without_an_existing_registration() {
    let temp = TempDir::new("no-authorization");
    let state_path = temp.path().join("state.db");
    let task_reconciliations = Arc::new(AtomicUsize::new(0));
    let mut reconciler = reconciliation_service(&state_path, task_reconciliations.clone());
    let mut frame = empty_frame(50_000);
    frame.managed_bindings.push(ManagedBindingObservation {
        registration_id: "not-authorized".into(),
        evidence: None,
        processes: BindingProcessObservation {
            launch_process: ProviderProcessObservation::Unknown,
            provider_process: None,
        },
    });

    let report = reconciler.reconcile(&frame);

    assert!(report.diagnostics.is_empty());
    assert_eq!(task_reconciliations.load(Ordering::Relaxed), 1);
    let store = StateManagedSessionRepository::open(state_path).unwrap();
    assert!(store.registrations().unwrap().is_empty());
    assert!(store.managed_sessions().unwrap().is_empty());
    assert!(store.managed_current_runs().unwrap().is_empty());
}

#[test]
fn replaying_one_frame_is_idempotent_for_binding_title_metadata_and_tasks() {
    let temp = TempDir::new("idempotent-frame");
    let state_path = temp.path().join("state.db");
    let working_directory = temp.directory("project");
    let source_path = temp.file("session.jsonl", "{}\n");
    let provider_process = process(42, 41_000);
    let launch = registration("authorized-launch", &working_directory, provider_process);
    StateManagedSessionRepository::open(&state_path)
        .unwrap()
        .insert(&launch)
        .unwrap();
    let task_reconciliations = Arc::new(AtomicUsize::new(0));
    let mut reconciler = reconciliation_service(&state_path, task_reconciliations.clone());
    let mut frame = empty_frame(50_000);
    frame.managed_bindings.push(ManagedBindingObservation {
        registration_id: launch.id.clone(),
        evidence: Some(ProviderBindingEvidence {
            provider: Provider::CodexCli,
            source_session_id: "native-session".into(),
            process_session_id: "native-session".into(),
            source: SourceMetadata {
                path: source_path,
                format: SourceFormat::JsonLines,
            },
            working_directory,
            launch_process: provider_process,
            provider_process,
        }),
        processes: BindingProcessObservation {
            launch_process: ProviderProcessObservation::Live(provider_process),
            provider_process: Some(ProviderProcessObservation::Live(provider_process)),
        },
    });

    assert!(reconciler.reconcile(&frame).diagnostics.is_empty());
    assert!(reconciler.reconcile(&frame).diagnostics.is_empty());

    assert_eq!(task_reconciliations.load(Ordering::Relaxed), 2);
    let store = StateManagedSessionRepository::open(&state_path).unwrap();
    assert!(store.registrations().unwrap().is_empty());
    assert_eq!(store.managed_sessions().unwrap().len(), 1);
    assert_eq!(store.managed_current_runs().unwrap().len(), 1);
    assert!(store.binding_receipt(&launch.id).unwrap().is_some());
    let labels = SessionLabels::load(&state_path);
    assert_eq!(
        labels.alias(Provider::CodexCli, "native-session"),
        Some("Managed work")
    );
    assert_eq!(
        labels.description(Provider::CodexCli, "native-session"),
        Some("Durable description")
    );
}

#[test]
fn publication_atomically_replaces_the_completed_snapshot() {
    let temp = TempDir::new("publication");
    let state_path = temp.path().join("state.db");
    StateManagedSessionRepository::open(&state_path).unwrap();
    let cache = Arc::new(InMemoryReadModelCache::default());
    cache.publish(RefreshSnapshot {
        revision: 1,
        ..RefreshSnapshot::default()
    });
    let previous = cache.current();
    let mut publisher = PublicationService::new(
        cache.clone(),
        AlertEngine::new(
            Box::new(SessionLabels::load(&state_path)),
            Box::new(NoNotifications),
        ),
    );

    publisher.publish(RefreshSnapshot {
        revision: 2,
        ready: true,
        ..RefreshSnapshot::default()
    });

    assert_eq!(previous.revision, 1);
    assert_eq!(cache.current().revision, 2);
    assert!(cache.current().ready);
}

struct FixedObservations;

impl SessionObservationSource for FixedObservations {
    fn collect(&mut self) -> SessionObservationBatch {
        SessionObservationBatch {
            stats: ScanStats {
                discovered: 1,
                ..ScanStats::default()
            },
            sessions: Vec::new(),
            diagnostics: vec![RefreshDiagnostic::new("Index", "read-only sample")],
        }
    }
}

struct RecordingObservationQuery {
    reads: Arc<AtomicUsize>,
    registration: ManagedLaunchRegistration,
}

impl ManagedObservationQuery for RecordingObservationQuery {
    fn binding_registrations(&self) -> anyhow::Result<Vec<ManagedLaunchRegistration>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(vec![self.registration.clone()])
    }

    fn verified_managed_current_runs(
        &self,
        _verifier: &ManagedCurrentRunVerifier,
    ) -> anyhow::Result<Vec<ManagedCurrentRunSummary>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(Vec::new())
    }

    fn resumable_managed_sessions(
        &self,
        _verifier: &ManagedResumeVerifier,
    ) -> anyhow::Result<Vec<ManagedResumeSummary>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(Vec::new())
    }
}

struct RecordingProjectionQuery {
    reads: Arc<AtomicUsize>,
}

impl ManagedProjectionQuery for RecordingProjectionQuery {
    fn apply_provenance(&self, _sessions: &mut [Session]) -> anyhow::Result<()> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn managed_launches(&self) -> anyhow::Result<Vec<ManagedLaunchSummary>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(Vec::new())
    }
}

struct RecordingTaskQuery {
    reads: Arc<AtomicUsize>,
}

impl OperationTaskQuery for RecordingTaskQuery {
    fn tasks(&self) -> anyhow::Result<Vec<OperationTask>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(Vec::new())
    }
}

struct NoBindingEvidence;

impl ManagedBindingEvidenceSource for NoBindingEvidence {
    fn evidence(
        &self,
        _registration: &ManagedLaunchRegistration,
    ) -> Option<ProviderBindingEvidence> {
        None
    }
}

struct RecordingProcessEvidence {
    samples: Arc<AtomicUsize>,
    started_at_ms: i64,
}

impl ProcessEvidence for RecordingProcessEvidence {
    fn observe(&self, pid: u32) -> ProviderProcessObservation {
        ProviderProcessObservation::Live(process(pid, self.started_at_ms))
    }

    fn observe_many(&self, pids: &[u32]) -> Vec<ProviderProcessObservation> {
        self.samples.fetch_add(1, Ordering::Relaxed);
        pids.iter().map(|pid| self.observe(*pid)).collect()
    }
}

struct NeverCurrentRun;

impl ManagedCurrentRunEvidence for NeverCurrentRun {
    fn revalidate(
        &self,
        _run: &ManagedCurrentRunRecord,
        _session: &ManagedSessionRecord,
        _receipt: &BindingReceipt,
    ) -> anyhow::Result<()> {
        panic!("the read-only fake query must not invoke a concrete verifier")
    }
}

struct NeverResume;

impl ManagedResumeEvidence for NeverResume {
    fn observe(
        &self,
        _run: &ManagedCurrentRunRecord,
        _session: &ManagedSessionRecord,
        _receipt: &BindingReceipt,
    ) -> anyhow::Result<mudi::application::managed_session::ManagedCurrentRunLifecycle> {
        panic!("the read-only fake query must not invoke a concrete verifier")
    }

    fn observe_many(
        &self,
        _candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<anyhow::Result<mudi::application::managed_session::ManagedCurrentRunLifecycle>> {
        panic!("the read-only fake query must not invoke a concrete verifier")
    }
}

struct PanicProcessEvidence;

impl ProcessEvidence for PanicProcessEvidence {
    fn observe(&self, _pid: u32) -> ProviderProcessObservation {
        panic!("reconciliation must consume process evidence from the ObservationFrame")
    }
}

struct NoopCleanup;

impl ManagedLaunchCleanup for NoopCleanup {
    fn clean_empty(&self, _registration: &ManagedLaunchRegistration) -> anyhow::Result<()> {
        Ok(())
    }
}

struct CountingTaskReconciler(Arc<AtomicUsize>);

impl OperationTaskReconciler for CountingTaskReconciler {
    fn reconcile(&mut self, _frame: &ObservationFrame) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct NoNotifications;

impl NotificationPort for NoNotifications {
    fn notify(&self, _message: &str) {}
}

fn reconciliation_service(
    state_path: &Path,
    task_reconciliations: Arc<AtomicUsize>,
) -> ReconciliationService {
    ReconciliationService::new(ReconciliationPorts {
        managed_repository: Ok(Box::new(
            StateManagedSessionRepository::open(state_path).unwrap(),
        )),
        managed_binding_reconciler: ManagedLaunchReconciler::new(
            Box::new(PanicProcessEvidence),
            Box::new(NoopCleanup),
        ),
        task_reconciler: Ok(Box::new(CountingTaskReconciler(task_reconciliations))),
    })
}

fn empty_frame(observed_at_ms: i64) -> ObservationFrame {
    ObservationFrame {
        observed_at_ms,
        stats: ScanStats::default(),
        sessions: Vec::new(),
        managed_bindings: Vec::new(),
        verified_managed_current_runs: Vec::new(),
        resumable_managed_sessions: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn registration(
    id: &str,
    working_directory: &Path,
    provider_process: ProcessIdentity,
) -> ManagedLaunchRegistration {
    ManagedLaunchRegistration {
        id: id.into(),
        status: ManagedLaunchStatus::Binding,
        provider: Provider::CodexCli,
        target: ManagedLaunchTarget {
            working_directory: working_directory.to_path_buf(),
        },
        session_alias: "Managed work".into(),
        description: Some("Durable description".into()),
        binding_initializer: false,
        control_channel: ManagedControlChannel {
            namespace: format!("mudi-{id}"),
            run_target: "managed".into(),
        },
        created_at_ms: 40_000,
        launched_at_ms: Some(41_000),
        launch_evidence: Some(ManagedLaunchEvidence {
            channel_id: "%42".into(),
            provider_process,
        }),
        terminal_opened_at_ms: Some(42_000),
        terminal_error: None,
        binding_error: None,
    }
}

fn process(pid: u32, started_at_ms: i64) -> ProcessIdentity {
    ProcessIdentity { pid, started_at_ms }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mudi-refresh-stages-{name}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, contents).unwrap();
        path.canonicalize().unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
