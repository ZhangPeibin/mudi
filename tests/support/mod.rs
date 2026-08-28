use mudi::adapters::handoff::open_handoff_service;
use mudi::adapters::persistence::session_metadata::SessionLabels;
use mudi::adapters::persistence::state::managed_sessions::StateManagedSessionRepository;
use mudi::adapters::project::CanonicalManagedTargetResolver;
use mudi::adapters::successor::RegistrySuccessorRuntime;
use mudi::application::clipboard::ClipboardPort;
use mudi::application::handlers::{ManagedWorkflowServices, WorkflowHandlers, WorkflowPorts};
use mudi::application::handoff::{
    CompletionNotice, CompletionNotifier, HandoffPaths, HandoffService, ProviderExecutables,
    WorkerLaunch, WorkerLauncher,
};
use mudi::application::managed_session::{
    ManagedLaunchBroker, ManagedLaunchEvidence, ManagedLaunchRegistration, ManagedNewService,
    ManagedTerminalAccess, ProviderLaunchCommand,
};
use mudi::application::observation::read_model::RefreshSnapshot;
use mudi::application::observation::Clock;
use mudi::application::successor::TerminalLauncher;
use mudi::presentation::tui::{route_action, Ui, UiAction};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

struct FixedClock;

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        50_000
    }
}

struct NoopCompletionNotifier;

impl CompletionNotifier for NoopCompletionNotifier {
    fn notify(&self, _notice: &CompletionNotice) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn workflow_handlers<L: WorkerLauncher, T: TerminalLauncher>(
    handoff: HandoffService<L>,
    terminal_launcher: T,
    managed: ManagedWorkflowServices,
    metadata_path: impl AsRef<Path>,
) -> WorkflowHandlers<L, T> {
    WorkflowHandlers::new(
        handoff,
        terminal_launcher,
        managed,
        WorkflowPorts {
            clock: Box::new(FixedClock),
            completion_notifier: Box::new(NoopCompletionNotifier),
            successor_runtime: Arc::new(RegistrySuccessorRuntime),
            target_resolver: Box::new(CanonicalManagedTargetResolver),
            session_metadata: Box::new(SessionLabels::load(metadata_path)),
        },
    )
}

#[allow(dead_code)]
pub fn sqlite_workflow_handlers(root: &Path) -> impl TestActionRouter {
    let state_path = root.join("state.db");
    let executables = ProviderExecutables::new(root.join("claude"), root.join("codex"));
    let handoff = open_handoff_service(
        HandoffPaths {
            task_store: state_path.clone(),
            artifact_root: root.join("artifacts"),
        },
        executables.clone(),
        NeverWorker,
    )
    .unwrap();
    let managed_new = ManagedNewService::new(
        StateManagedSessionRepository::open(&state_path).unwrap(),
        executables,
        Box::new(NeverManagedLaunch),
        Box::new(NeverManagedTerminal),
    );
    workflow_handlers(
        handoff,
        NeverSuccessorTerminal,
        ManagedWorkflowServices {
            start_new: Some(managed_new),
            start_resume: None,
            recovery: None,
        },
        state_path,
    )
}

#[allow(dead_code)]
struct NeverWorker;

impl WorkerLauncher for NeverWorker {
    fn launch(&self, _launch: &WorkerLaunch) -> std::io::Result<()> {
        panic!("metadata and curation tests must not launch a Handoff worker")
    }
}

#[allow(dead_code)]
struct NeverManagedLaunch;

impl ManagedLaunchBroker for NeverManagedLaunch {
    fn launch(
        &mut self,
        _registration: &ManagedLaunchRegistration,
        _command: &ProviderLaunchCommand,
    ) -> anyhow::Result<ManagedLaunchEvidence> {
        panic!("metadata and curation tests must not launch a Managed Session")
    }
}

#[allow(dead_code)]
struct NeverManagedTerminal;

impl ManagedTerminalAccess for NeverManagedTerminal {
    fn open(&mut self, _registration: &ManagedLaunchRegistration) -> anyhow::Result<()> {
        panic!("metadata and curation tests must not open a Managed Session terminal")
    }
}

#[allow(dead_code)]
struct NeverSuccessorTerminal;

impl TerminalLauncher for NeverSuccessorTerminal {
    fn open(&mut self, _task_id: &str, _now_ms: i64) -> anyhow::Result<()> {
        panic!("metadata and curation tests must not open a Successor terminal")
    }
}

pub trait TestActionRouter {
    fn handle(
        &mut self,
        action: UiAction,
        snapshot: &RefreshSnapshot,
        ui: &mut Ui,
        clipboard: &mut dyn ClipboardPort,
        rescan_tx: &mpsc::UnboundedSender<()>,
    ) -> bool;
}

impl<L: WorkerLauncher, T: TerminalLauncher> TestActionRouter for WorkflowHandlers<L, T> {
    fn handle(
        &mut self,
        action: UiAction,
        snapshot: &RefreshSnapshot,
        ui: &mut Ui,
        clipboard: &mut dyn ClipboardPort,
        rescan_tx: &mpsc::UnboundedSender<()>,
    ) -> bool {
        let mut request_rescan = || {
            let _ = rescan_tx.send(());
        };
        route_action(action, snapshot, ui, clipboard, &mut request_rescan, self)
    }
}
