//! Typed application handlers for commands initiated by presentation.

use crate::application::handoff::{
    CompletionNotice, CompletionNotifier, HandoffService, WorkerLauncher,
};
use crate::application::managed_sessions::{
    ManagedCurrentRunAccessService, ManagedNewOutcome, ManagedNewRequest, ManagedResumeOutcome,
    ManagedResumeRequest, ManagedSessionService, ManagedTerminalOutcome,
};
use crate::application::observation::read_model::RefreshSnapshot;
use crate::application::observation::Clock;
use crate::application::session_labels::SessionMetadataRepository;
use crate::application::successor::{render_successor_command, SuccessorRuntime, TerminalLauncher};
use crate::application::tasks::{OperationTask, OperationTaskState};
use crate::domain::{Provider, Session, SessionIdentity};
use anyhow::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::managed_sessions::ManagedLaunchTarget;

pub trait ManagedTargetResolver: Send + Sync {
    fn resolve(&self, directory: &Path) -> anyhow::Result<ManagedLaunchTarget>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffAuthorizationRequest {
    pub source: SessionIdentity,
    pub successor_provider: Provider,
    pub end_current_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRetryRequest {
    pub predecessor_task_id: String,
    pub source: SessionIdentity,
    pub successor_provider: Provider,
    pub end_current_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedCurrentRunRequest {
    pub provider: Provider,
    pub native_session_id: String,
    pub opaque_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadataUpdateRequest {
    pub session: SessionIdentity,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadataUpdateOutcome {
    pub session: SessionIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurationRequest {
    pub sessions: Vec<SessionIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurationOutcome {
    pub changed: usize,
}

#[derive(Debug)]
pub enum WorkflowError {
    TargetUnavailable(SessionIdentity),
    InvalidState(&'static str),
    Operation(Error),
}

impl fmt::Display for WorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetUnavailable(identity) => write!(
                formatter,
                "{} Session {} is unavailable",
                identity.provider.label(),
                identity.native_id
            ),
            Self::InvalidState(reason) => formatter.write_str(reason),
            Self::Operation(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for WorkflowError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Operation(error) => Some(error.as_ref()),
            Self::TargetUnavailable(_) | Self::InvalidState(_) => None,
        }
    }
}

impl From<Error> for WorkflowError {
    fn from(error: Error) -> Self {
        Self::Operation(error)
    }
}

pub struct WorkflowPorts {
    pub clock: Box<dyn Clock>,
    pub completion_notifier: Box<dyn CompletionNotifier>,
    pub successor_runtime: Arc<dyn SuccessorRuntime>,
    pub target_resolver: Box<dyn ManagedTargetResolver>,
    pub session_metadata: Box<dyn SessionMetadataRepository>,
}

struct WorkflowRuntimePorts {
    clock: Arc<dyn Clock>,
    target_resolver: Box<dyn ManagedTargetResolver>,
}

pub struct HandoffHandler<L> {
    service: HandoffService<L>,
    clock: Arc<dyn Clock>,
    completion_notifier: Box<dyn CompletionNotifier>,
    successor_runtime: Arc<dyn SuccessorRuntime>,
}

impl<L: WorkerLauncher> HandoffHandler<L> {
    pub fn new(
        service: HandoffService<L>,
        clock: Arc<dyn Clock>,
        completion_notifier: Box<dyn CompletionNotifier>,
        successor_runtime: Arc<dyn SuccessorRuntime>,
    ) -> Self {
        Self {
            service,
            clock,
            completion_notifier,
            successor_runtime,
        }
    }

    pub fn reconcile_tasks(&self) -> Result<Vec<OperationTask>, WorkflowError> {
        self.service
            .reconcile_tasks(self.clock.now_ms())
            .map_err(Into::into)
    }

    pub fn authorize(
        &mut self,
        request: HandoffAuthorizationRequest,
        snapshot: &RefreshSnapshot,
    ) -> Result<OperationTask, WorkflowError> {
        let session = find_session(snapshot, &request.source)?;
        let task = self
            .service
            .authorize_session_to(
                session,
                request.successor_provider,
                request.end_current_run,
                self.clock.now_ms(),
            )
            .map_err(|error| WorkflowError::Operation(error.into()))?;
        self.notify_launch_failure(&task);
        self.service
            .reconcile_tasks(self.clock.now_ms())
            .map_err(WorkflowError::Operation)?;
        self.service
            .task(&task.id)
            .map_err(WorkflowError::Operation)?
            .ok_or_else(|| WorkflowError::TargetUnavailable(request.source))
    }

    pub fn acknowledge(&self, task_id: &str) -> Result<OperationTask, WorkflowError> {
        self.service
            .acknowledge_task(task_id, self.clock.now_ms())
            .map_err(Into::into)
    }

    pub fn cancel(&self, task_id: &str) -> Result<OperationTask, WorkflowError> {
        self.service
            .cancel_task(task_id, self.clock.now_ms())
            .map_err(Into::into)
    }

    pub fn delete(&self, task_id: &str) -> Result<(), WorkflowError> {
        self.service
            .delete_task(task_id, self.clock.now_ms())
            .map_err(Into::into)
    }

    pub fn retry(
        &mut self,
        request: HandoffRetryRequest,
        snapshot: &RefreshSnapshot,
    ) -> Result<OperationTask, WorkflowError> {
        let session = find_session(snapshot, &request.source)?;
        let task = self
            .service
            .retry_task_to(
                &request.predecessor_task_id,
                session,
                request.successor_provider,
                request.end_current_run,
                self.clock.now_ms(),
            )
            .map_err(|error| WorkflowError::Operation(error.into()))?;
        self.notify_launch_failure(&task);
        self.service
            .reconcile_tasks(self.clock.now_ms())
            .map_err(WorkflowError::Operation)?;
        self.service
            .task(&task.id)
            .map_err(WorkflowError::Operation)?
            .ok_or_else(|| WorkflowError::TargetUnavailable(request.source))
    }

    pub fn successor_command(&self, task_id: &str) -> Result<String, WorkflowError> {
        let task = self
            .service
            .task(task_id)
            .map_err(WorkflowError::Operation)?
            .ok_or(WorkflowError::InvalidState("Operation Task is unavailable"))?;
        render_successor_command(&task, self.successor_runtime.as_ref()).map_err(Into::into)
    }

    fn notify_launch_failure(&self, task: &OperationTask) {
        if task.state == OperationTaskState::Failed {
            let _ = self.completion_notifier.notify(&CompletionNotice {
                task_id: task.id.clone(),
                state: task.state,
                source_provider: task.source_provider,
                source_session_id: task.source_session_id.clone(),
            });
        }
    }
}

pub struct SessionMetadataHandler {
    repository: Box<dyn SessionMetadataRepository>,
}

impl SessionMetadataHandler {
    pub fn new(repository: Box<dyn SessionMetadataRepository>) -> Self {
        Self { repository }
    }

    pub fn save(
        &mut self,
        request: SessionMetadataUpdateRequest,
    ) -> Result<SessionMetadataUpdateOutcome, WorkflowError> {
        self.repository
            .set_metadata(
                request.session.provider,
                &request.session.native_id,
                &request.title,
                &request.description,
            )
            .map_err(|error| WorkflowError::Operation(error.into()))?;
        Ok(SessionMetadataUpdateOutcome {
            session: request.session,
        })
    }

    pub fn hide(
        &mut self,
        request: CurationRequest,
        snapshot: &RefreshSnapshot,
    ) -> Result<CurationOutcome, WorkflowError> {
        let sessions = exited_targets(request, snapshot)?;
        self.repository
            .hide_sessions(&sessions)
            .map_err(|error| WorkflowError::Operation(error.into()))?;
        Ok(CurationOutcome {
            changed: sessions.len(),
        })
    }

    pub fn restore(&mut self, request: CurationRequest) -> Result<CurationOutcome, WorkflowError> {
        if request.sessions.is_empty() {
            return Err(WorkflowError::InvalidState("No Sessions were selected"));
        }
        self.repository
            .remove_from_hide(&request.sessions)
            .map_err(|error| WorkflowError::Operation(error.into()))?;
        Ok(CurationOutcome {
            changed: request.sessions.len(),
        })
    }

    pub fn permanently_exclude(
        &mut self,
        request: CurationRequest,
    ) -> Result<CurationOutcome, WorkflowError> {
        if request.sessions.is_empty() {
            return Err(WorkflowError::InvalidState("No Sessions were selected"));
        }
        self.repository
            .permanently_exclude_sessions(&request.sessions)
            .map_err(|error| WorkflowError::Operation(error.into()))?;
        Ok(CurationOutcome {
            changed: request.sessions.len(),
        })
    }
}

pub struct WorkflowHandlers<L, T> {
    handoff: HandoffHandler<L>,
    terminal_launcher: T,
    managed_sessions: ManagedSessionService,
    managed_current_runs: Option<ManagedCurrentRunAccessService>,
    session_metadata: SessionMetadataHandler,
    ports: WorkflowRuntimePorts,
}

impl<L: WorkerLauncher, T: TerminalLauncher> WorkflowHandlers<L, T> {
    pub fn new(
        handoff: HandoffService<L>,
        terminal_launcher: T,
        managed_sessions: ManagedSessionService,
        ports: WorkflowPorts,
    ) -> Self {
        let WorkflowPorts {
            clock,
            completion_notifier,
            successor_runtime,
            target_resolver,
            session_metadata,
        } = ports;
        let clock = Arc::<dyn Clock>::from(clock);
        Self {
            handoff: HandoffHandler::new(
                handoff,
                clock.clone(),
                completion_notifier,
                successor_runtime,
            ),
            terminal_launcher,
            managed_sessions,
            managed_current_runs: None,
            session_metadata: SessionMetadataHandler::new(session_metadata),
            ports: WorkflowRuntimePorts {
                clock,
                target_resolver,
            },
        }
    }

    pub fn with_managed_current_run_access(
        mut self,
        access: ManagedCurrentRunAccessService,
    ) -> Self {
        self.managed_current_runs = Some(access);
        self
    }

    pub fn reconcile_tasks(&self) -> Result<Vec<OperationTask>, WorkflowError> {
        self.handoff.reconcile_tasks()
    }

    pub fn authorize_handoff(
        &mut self,
        request: HandoffAuthorizationRequest,
        snapshot: &RefreshSnapshot,
    ) -> Result<OperationTask, WorkflowError> {
        self.handoff.authorize(request, snapshot)
    }

    pub fn acknowledge_task(&self, task_id: &str) -> Result<OperationTask, WorkflowError> {
        self.handoff.acknowledge(task_id)
    }

    pub fn cancel_task(&self, task_id: &str) -> Result<OperationTask, WorkflowError> {
        self.handoff.cancel(task_id)
    }

    pub fn delete_task(&self, task_id: &str) -> Result<(), WorkflowError> {
        self.handoff.delete(task_id)
    }

    pub fn retry_handoff(
        &mut self,
        request: HandoffRetryRequest,
        snapshot: &RefreshSnapshot,
    ) -> Result<OperationTask, WorkflowError> {
        self.handoff.retry(request, snapshot)
    }

    pub fn successor_command(&self, task_id: &str) -> Result<String, WorkflowError> {
        self.handoff.successor_command(task_id)
    }

    pub fn open_successor_terminal(&mut self, task_id: &str) -> Result<(), WorkflowError> {
        self.terminal_launcher
            .open(task_id, self.ports.clock.now_ms())
            .map_err(Into::into)
    }

    pub fn resolve_managed_target(
        &self,
        directory: PathBuf,
    ) -> Result<ManagedLaunchTarget, WorkflowError> {
        self.ports
            .target_resolver
            .resolve(&directory)
            .map_err(Into::into)
    }

    pub fn start_managed_new(
        &mut self,
        request: ManagedNewRequest,
    ) -> Result<ManagedNewOutcome, WorkflowError> {
        self.managed_sessions
            .start_new(request, self.ports.clock.now_ms())
            .map_err(Into::into)
    }

    pub fn start_managed_resume(
        &mut self,
        request: ManagedResumeRequest,
    ) -> Result<ManagedResumeOutcome, WorkflowError> {
        self.managed_sessions
            .start_resume(request, self.ports.clock.now_ms())
            .map_err(Into::into)
    }

    pub fn cancel_managed_start(&mut self, registration_id: &str) -> Result<(), WorkflowError> {
        self.managed_sessions
            .cancel_start(registration_id)
            .map_err(Into::into)
    }

    pub fn open_managed_terminal(
        &mut self,
        registration_id: &str,
    ) -> Result<ManagedTerminalOutcome, WorkflowError> {
        self.managed_sessions
            .open_terminal(registration_id, self.ports.clock.now_ms())
            .map_err(Into::into)
    }

    pub fn retry_managed_terminal(
        &mut self,
        registration_id: &str,
    ) -> Result<ManagedTerminalOutcome, WorkflowError> {
        self.managed_sessions
            .retry_open_terminal(registration_id, self.ports.clock.now_ms())
            .map_err(Into::into)
    }

    pub fn managed_launch_diagnostics(
        &self,
        registration_id: &str,
    ) -> Result<String, WorkflowError> {
        self.managed_sessions
            .copy_diagnostics(registration_id)
            .map_err(Into::into)
    }

    pub fn open_managed_current_run(
        &mut self,
        request: &ManagedCurrentRunRequest,
    ) -> Result<(), WorkflowError> {
        self.managed_current_runs
            .as_mut()
            .ok_or(WorkflowError::InvalidState(
                "Managed Current Run access is unavailable",
            ))?
            .open_terminal(
                request.provider,
                &request.native_session_id,
                &request.opaque_ref,
            )
            .map_err(Into::into)
    }

    pub fn managed_current_run_attach_command(
        &self,
        request: &ManagedCurrentRunRequest,
    ) -> Result<String, WorkflowError> {
        self.managed_current_runs
            .as_ref()
            .ok_or(WorkflowError::InvalidState(
                "Managed Current Run access is unavailable",
            ))?
            .attach_command(
                request.provider,
                &request.native_session_id,
                &request.opaque_ref,
            )
            .map_err(Into::into)
    }

    pub fn save_session_metadata(
        &mut self,
        request: SessionMetadataUpdateRequest,
    ) -> Result<SessionMetadataUpdateOutcome, WorkflowError> {
        self.session_metadata.save(request)
    }

    pub fn hide_sessions(
        &mut self,
        request: CurationRequest,
        snapshot: &RefreshSnapshot,
    ) -> Result<CurationOutcome, WorkflowError> {
        self.session_metadata.hide(request, snapshot)
    }

    pub fn restore_sessions(
        &mut self,
        request: CurationRequest,
    ) -> Result<CurationOutcome, WorkflowError> {
        self.session_metadata.restore(request)
    }

    pub fn permanently_exclude_sessions(
        &mut self,
        request: CurationRequest,
    ) -> Result<CurationOutcome, WorkflowError> {
        self.session_metadata.permanently_exclude(request)
    }
}

fn find_session<'a>(
    snapshot: &'a RefreshSnapshot,
    identity: &SessionIdentity,
) -> Result<&'a Session, WorkflowError> {
    snapshot
        .sessions
        .iter()
        .find(|session| session.provider == identity.provider && session.id == identity.native_id)
        .ok_or_else(|| WorkflowError::TargetUnavailable(identity.clone()))
}

fn exited_targets(
    request: CurationRequest,
    snapshot: &RefreshSnapshot,
) -> Result<Vec<SessionIdentity>, WorkflowError> {
    if request.sessions.is_empty() {
        return Err(WorkflowError::InvalidState("No Sessions were selected"));
    }
    let mut exited = Vec::with_capacity(request.sessions.len());
    for identity in request.sessions {
        let session = find_session(snapshot, &identity)?;
        if session.state.is_open() {
            return Err(WorkflowError::InvalidState(
                "Open Sessions cannot be hidden",
            ));
        }
        exited.push(identity);
    }
    Ok(exited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::session_labels::{
        CurationDisposition, SessionMetadataQuery, SessionMetadataRepository,
    };
    use crate::application::tasks::{SuccessorLaunchPlan, TaskRepository};
    use crate::domain::{
        ContextState, ProcessIdentity, SessionProvenance, SessionState, SourceMetadata, Usage,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct FakeMetadataState {
        saved: Vec<(SessionIdentity, String, String)>,
        hidden: Vec<SessionIdentity>,
        restored: Vec<SessionIdentity>,
        excluded: Vec<SessionIdentity>,
    }

    struct FakeMetadataRepository {
        state: Arc<Mutex<FakeMetadataState>>,
        fail_writes: bool,
    }

    impl FakeMetadataRepository {
        fn write(&self, update: impl FnOnce(&mut FakeMetadataState)) -> std::io::Result<()> {
            if self.fail_writes {
                return Err(std::io::Error::other("metadata write failed"));
            }
            update(&mut self.state.lock().unwrap());
            Ok(())
        }
    }

    impl SessionMetadataQuery for FakeMetadataRepository {
        fn is_editable(&self) -> bool {
            !self.fail_writes
        }

        fn error(&self) -> Option<&str> {
            self.fail_writes.then_some("metadata write failed")
        }

        fn reload(&mut self) {}

        fn alias(&self, _provider: Provider, _native_id: &str) -> Option<&str> {
            None
        }

        fn description(&self, _provider: Provider, _native_id: &str) -> Option<&str> {
            None
        }

        fn disposition(
            &self,
            _provider: Provider,
            _native_id: &str,
        ) -> Option<CurationDisposition> {
            None
        }
    }

    impl SessionMetadataRepository for FakeMetadataRepository {
        fn hide_sessions(&mut self, sessions: &[SessionIdentity]) -> std::io::Result<()> {
            self.write(|state| state.hidden.extend_from_slice(sessions))
        }

        fn remove_from_hide(&mut self, sessions: &[SessionIdentity]) -> std::io::Result<()> {
            self.write(|state| state.restored.extend_from_slice(sessions))
        }

        fn permanently_exclude_sessions(
            &mut self,
            sessions: &[SessionIdentity],
        ) -> std::io::Result<()> {
            self.write(|state| state.excluded.extend_from_slice(sessions))
        }

        fn set_alias(
            &mut self,
            _provider: Provider,
            _native_id: &str,
            _alias: &str,
        ) -> std::io::Result<()> {
            panic!("handler must use the atomic metadata operation")
        }

        fn associate_alias(
            &mut self,
            _provider: Provider,
            _native_id: &str,
            _alias: &str,
        ) -> std::io::Result<bool> {
            panic!("handler must not associate a launch Alias")
        }

        fn clear_title(&mut self, _provider: Provider, _native_id: &str) -> std::io::Result<()> {
            panic!("handler must use the atomic metadata operation")
        }

        fn set_description(
            &mut self,
            _provider: Provider,
            _native_id: &str,
            _description: &str,
        ) -> std::io::Result<()> {
            panic!("handler must use the atomic metadata operation")
        }

        fn set_metadata(
            &mut self,
            provider: Provider,
            native_id: &str,
            title: &str,
            description: &str,
        ) -> std::io::Result<()> {
            self.write(|state| {
                state.saved.push((
                    SessionIdentity {
                        provider,
                        native_id: native_id.to_string(),
                    },
                    title.to_string(),
                    description.to_string(),
                ));
            })
        }
    }

    #[test]
    fn metadata_handler_returns_typed_outcome_after_authoritative_change() {
        let state = Arc::new(Mutex::new(FakeMetadataState::default()));
        let mut handler = metadata_handler(state.clone(), false);
        let session = identity("session-1");

        let outcome = handler
            .save(SessionMetadataUpdateRequest {
                session: session.clone(),
                title: "Release coordination".into(),
                description: "Keeps the release work recognizable".into(),
            })
            .unwrap();

        assert_eq!(outcome.session, session);
        assert_eq!(state.lock().unwrap().saved.len(), 1);
    }

    #[test]
    fn curation_handler_rejects_disappeared_target_without_state_change() {
        let state = Arc::new(Mutex::new(FakeMetadataState::default()));
        let mut handler = metadata_handler(state.clone(), false);
        let missing = identity("missing-session");

        let error = handler
            .hide(
                CurationRequest {
                    sessions: vec![missing.clone()],
                },
                &RefreshSnapshot::default(),
            )
            .unwrap_err();

        assert!(matches!(error, WorkflowError::TargetUnavailable(target) if target == missing));
        assert!(state.lock().unwrap().hidden.is_empty());
    }

    #[test]
    fn curation_handler_rejects_open_target_without_state_change() {
        let state = Arc::new(Mutex::new(FakeMetadataState::default()));
        let mut handler = metadata_handler(state.clone(), false);
        let session = session("open-session", SessionState::Busy);

        let error = handler
            .hide(
                CurationRequest {
                    sessions: vec![SessionIdentity::from(&session)],
                },
                &snapshot(session),
            )
            .unwrap_err();

        assert!(matches!(error, WorkflowError::InvalidState(_)));
        assert!(state.lock().unwrap().hidden.is_empty());
    }

    #[test]
    fn curation_handler_returns_changed_count_after_authoritative_change() {
        let state = Arc::new(Mutex::new(FakeMetadataState::default()));
        let mut handler = metadata_handler(state.clone(), false);
        let session = session("exited-session", SessionState::Exited);
        let target = SessionIdentity::from(&session);

        let outcome = handler
            .hide(
                CurationRequest {
                    sessions: vec![target.clone()],
                },
                &snapshot(session),
            )
            .unwrap();

        assert_eq!(outcome, CurationOutcome { changed: 1 });
        assert_eq!(state.lock().unwrap().hidden, vec![target]);
    }

    #[test]
    fn metadata_handler_reports_write_failure_without_authoritative_change() {
        let state = Arc::new(Mutex::new(FakeMetadataState::default()));
        let mut handler = metadata_handler(state.clone(), true);

        let error = handler
            .save(SessionMetadataUpdateRequest {
                session: identity("session-1"),
                title: "Release coordination".into(),
                description: String::new(),
            })
            .unwrap_err();

        assert!(matches!(error, WorkflowError::Operation(_)));
        assert!(state.lock().unwrap().saved.is_empty());
    }

    #[test]
    fn handoff_handler_persists_only_the_explicitly_authorized_request() {
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "mudi-handoff-handler-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        let working_directory = root.join("working");
        let source_path = root.join("session.jsonl");
        std::fs::create_dir_all(&working_directory).unwrap();
        std::fs::write(&source_path, "{}\n").unwrap();
        let tasks = Arc::new(Mutex::new(Vec::new()));
        let launches = Arc::new(Mutex::new(Vec::new()));
        let service = HandoffService::new(
            crate::application::handoff::HandoffPaths {
                task_store: root.join("state.db"),
                artifact_root: root.join("artifacts"),
            },
            FakeProviderExecutables,
            RecordingWorker {
                launches: launches.clone(),
            },
            Arc::new(NoopCurrentRun),
            Arc::new(FixedTaskId),
            Arc::new(FakeSuccessorRuntime),
            FakeTaskRepository {
                tasks: tasks.clone(),
            },
        )
        .unwrap();
        let mut handler = HandoffHandler::new(
            service,
            Arc::new(FixedClock),
            Box::new(NoopNotifier),
            Arc::new(FakeSuccessorRuntime),
        );
        let mut source = session("source-session", SessionState::Exited);
        source.source.path = source_path;
        source.cwd = Some(working_directory.to_string_lossy().into_owned());

        let task = handler
            .authorize(
                HandoffAuthorizationRequest {
                    source: SessionIdentity::from(&source),
                    successor_provider: Provider::CodexCli,
                    end_current_run: false,
                },
                &snapshot(source),
            )
            .unwrap();

        assert_eq!(task.id, "task-1");
        assert_eq!(task.successor_provider, Provider::CodexCli);
        assert!(!task.end_current_run_authorized);
        assert_eq!(launches.lock().unwrap().as_slice(), &["task-1"]);
        assert_eq!(tasks.lock().unwrap().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            50_000
        }
    }

    struct FixedTaskId;

    impl crate::application::handoff::TaskIdGenerator for FixedTaskId {
        fn new_task_id(&self) -> String {
            "task-1".into()
        }
    }

    struct NoopNotifier;

    impl CompletionNotifier for NoopNotifier {
        fn notify(&self, _notice: &CompletionNotice) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct RecordingWorker {
        launches: Arc<Mutex<Vec<String>>>,
    }

    impl WorkerLauncher for RecordingWorker {
        fn launch(
            &self,
            launch: &crate::application::handoff::WorkerLaunch,
        ) -> std::io::Result<()> {
            self.launches.lock().unwrap().push(launch.task_id.clone());
            Ok(())
        }
    }

    struct NoopCurrentRun;

    impl crate::application::handoff::CurrentRunControl for NoopCurrentRun {
        fn observe(
            &self,
            _provider: Provider,
            _source_session_id: &str,
            _source_record_path: &Path,
        ) -> anyhow::Result<crate::application::handoff::CurrentRunObservation> {
            Ok(crate::application::handoff::CurrentRunObservation::Exited)
        }

        fn request_graceful_exit(&self, _process: ProcessIdentity) -> anyhow::Result<()> {
            Ok(())
        }

        fn process_is_live(&self, _process: ProcessIdentity) -> bool {
            false
        }

        fn now_ms(&self) -> i64 {
            50_000
        }

        fn wait_before_recheck(&self) {}
    }

    struct FakeSuccessorRuntime;

    impl SuccessorRuntime for FakeSuccessorRuntime {
        fn launch_arguments(
            &self,
            _provider: Provider,
            _artifact_path: &Path,
        ) -> anyhow::Result<Vec<String>> {
            Ok(Vec::new())
        }

        fn validate_environment(&self, _plan: &SuccessorLaunchPlan) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct FakeProviderExecutables;

    impl crate::application::ProviderExecutableResolver for FakeProviderExecutables {
        fn resolve_executable(&self, provider: Provider) -> Option<PathBuf> {
            Some(PathBuf::from(provider.as_str()))
        }
    }

    struct FakeTaskRepository {
        tasks: Arc<Mutex<Vec<OperationTask>>>,
    }

    impl FakeTaskRepository {
        fn unsupported<T>() -> anyhow::Result<T> {
            Err(anyhow::anyhow!("unexpected Task Repository operation"))
        }
    }

    impl TaskRepository for FakeTaskRepository {
        fn insert(&self, task: &OperationTask) -> anyhow::Result<()> {
            self.tasks.lock().unwrap().push(task.clone());
            Ok(())
        }

        fn get(&self, task_id: &str) -> anyhow::Result<Option<OperationTask>> {
            Ok(self
                .tasks
                .lock()
                .unwrap()
                .iter()
                .find(|task| task.id == task_id)
                .cloned())
        }

        fn list(&self) -> anyhow::Result<Vec<OperationTask>> {
            Ok(self.tasks.lock().unwrap().clone())
        }

        fn request_cancellation(&self, _task_id: &str, _now_ms: i64) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn mark_termination_requested(&self, _task_id: &str, _now_ms: i64) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn mark_cancelled(
            &self,
            _task_id: &str,
            _now_ms: i64,
            _failure_code: &str,
        ) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn mark_retry_ready(&self, _task_id: &str, _now_ms: i64) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn mark_ready(
            &self,
            _task_id: &str,
            _now_ms: i64,
            _artifact_path: &Path,
            _plan: &SuccessorLaunchPlan,
        ) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn request_deletion(&self, _task_id: &str, _now_ms: i64) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn record_deletion_failure(
            &self,
            _task_id: &str,
            _failure_code: &str,
        ) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn delete_terminal(&self, _task_id: &str) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn mark_failed(
            &self,
            _task_id: &str,
            _now_ms: i64,
            _failure_code: &str,
        ) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn mark_interrupted(
            &self,
            _task_id: &str,
            _now_ms: i64,
            _failure_code: &str,
        ) -> anyhow::Result<()> {
            Self::unsupported()
        }

        fn cancel_before_execution(&self, _task_id: &str, _now_ms: i64) -> anyhow::Result<bool> {
            Self::unsupported()
        }

        fn has_non_terminal(
            &self,
            provider: Provider,
            source_session_id: &str,
        ) -> anyhow::Result<bool> {
            Ok(self.tasks.lock().unwrap().iter().any(|task| {
                task.source_provider == provider
                    && task.source_session_id == source_session_id
                    && !task.state.is_terminal()
            }))
        }

        fn acknowledge(&self, _task_id: &str, _now_ms: i64) -> anyhow::Result<OperationTask> {
            Self::unsupported()
        }
    }

    fn metadata_handler(
        state: Arc<Mutex<FakeMetadataState>>,
        fail_writes: bool,
    ) -> SessionMetadataHandler {
        SessionMetadataHandler::new(Box::new(FakeMetadataRepository { state, fail_writes }))
    }

    fn identity(native_id: &str) -> SessionIdentity {
        SessionIdentity {
            provider: Provider::ClaudeCode,
            native_id: native_id.to_string(),
        }
    }

    fn snapshot(session: Session) -> RefreshSnapshot {
        RefreshSnapshot {
            sessions: vec![session],
            ready: true,
            ..RefreshSnapshot::default()
        }
    }

    fn session(native_id: &str, state: SessionState) -> Session {
        Session {
            provider: Provider::ClaudeCode,
            id: native_id.to_string(),
            provenance: SessionProvenance::Observed,
            source: SourceMetadata::default(),
            project: None,
            cwd: None,
            latest_model: None,
            state,
            context: ContextState::default(),
            usage: Usage::default(),
            per_model: Vec::new(),
            current_run_usage: None,
            compactions: Vec::new(),
            last_activity_ms: 0,
            cost: None,
        }
    }
}
