//! Explicit authorization and execution entry points for Assisted Handoff.

use crate::domain::{Provider, Session, SessionState};
use crate::providers::state::{ProcessIdentity, StateDetector};
use crate::tasks::{OperationTask, OperationTaskState, TaskStore};
use anyhow::{anyhow, Result};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_ARTIFACT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffPaths {
    pub task_store: PathBuf,
    pub artifact_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderExecutables {
    claude: PathBuf,
    codex: PathBuf,
}

impl ProviderExecutables {
    pub fn new(claude: PathBuf, codex: PathBuf) -> Self {
        Self { claude, codex }
    }

    fn for_provider(&self, provider: Provider) -> &Path {
        match provider {
            Provider::ClaudeCode => &self.claude,
            Provider::CodexCli => &self.codex,
        }
    }

    pub fn discover() -> Self {
        let search = std::env::var_os("PATH")
            .as_deref()
            .map(std::env::split_paths)
            .map(Iterator::collect::<Vec<_>>)
            .unwrap_or_default();
        let mut discovered = Self::discover_in(&search);
        if let Some(path) = std::env::var_os("SESSIONMESH_CLAUDE_EXECUTABLE") {
            discovered.claude = PathBuf::from(path);
        }
        if let Some(path) = std::env::var_os("SESSIONMESH_CODEX_EXECUTABLE") {
            discovered.codex = PathBuf::from(path);
        }
        discovered
    }

    pub fn discover_in(search_directories: &[PathBuf]) -> Self {
        Self {
            claude: find_executable(search_directories, "claude").unwrap_or_default(),
            codex: find_executable(search_directories, "codex").unwrap_or_default(),
        }
    }

    pub fn executable_for(&self, provider: Provider) -> Option<&Path> {
        let path = self.for_provider(provider);
        is_executable(path).then_some(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLaunch {
    pub task_id: String,
}

pub trait WorkerLauncher {
    fn launch(&self, launch: &WorkerLaunch) -> std::io::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentRunObservation {
    Busy(ProcessIdentity),
    Idle(ProcessIdentity),
    Exited,
}

pub trait CurrentRunControl: Send + Sync {
    fn observe(
        &self,
        provider: Provider,
        source_session_id: &str,
        source_record_path: &Path,
    ) -> Result<CurrentRunObservation>;

    fn request_graceful_exit(&self, process: ProcessIdentity) -> Result<()>;

    fn now_ms(&self) -> i64;

    fn wait_before_recheck(&self);
}

struct NoopCurrentRunControl;

struct ReconciliationOnlyLauncher;

impl WorkerLauncher for ReconciliationOnlyLauncher {
    fn launch(&self, _launch: &WorkerLaunch) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "reconciliation cannot launch a Worker",
        ))
    }
}

impl CurrentRunControl for NoopCurrentRunControl {
    fn observe(
        &self,
        _provider: Provider,
        _source_session_id: &str,
        _source_record_path: &Path,
    ) -> Result<CurrentRunObservation> {
        Ok(CurrentRunObservation::Exited)
    }

    fn request_graceful_exit(&self, _process: ProcessIdentity) -> Result<()> {
        Ok(())
    }

    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn wait_before_recheck(&self) {}
}

pub struct MacosCurrentRunControl {
    claude_sessions_dir: PathBuf,
}

impl MacosCurrentRunControl {
    pub fn new(claude_sessions_dir: PathBuf) -> Self {
        Self {
            claude_sessions_dir,
        }
    }
}

impl CurrentRunControl for MacosCurrentRunControl {
    fn observe(
        &self,
        provider: Provider,
        source_session_id: &str,
        source_record_path: &Path,
    ) -> Result<CurrentRunObservation> {
        let detector = StateDetector::gather(&self.claude_sessions_dir);
        let live = match provider {
            Provider::ClaudeCode => detector.claude_state(source_session_id),
            Provider::CodexCli => detector.codex_state(
                source_record_path,
                crate::providers::codex::latest_task_boundary(source_record_path),
            ),
        };
        match (live.state, live.process_identity) {
            (SessionState::Busy, Some(process)) => Ok(CurrentRunObservation::Busy(process)),
            (SessionState::Idle, Some(process)) => Ok(CurrentRunObservation::Idle(process)),
            (SessionState::Exited, _) => Ok(CurrentRunObservation::Exited),
            (SessionState::Busy | SessionState::Idle, None) => Err(anyhow!(
                "exact Provider CLI process identity is unavailable"
            )),
        }
    }

    fn request_graceful_exit(&self, process: ProcessIdentity) -> Result<()> {
        request_graceful_process_exit(process)
    }

    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn wait_before_recheck(&self) {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

#[derive(Debug, Clone)]
pub struct DetachedWorkerLauncher {
    worker_executable: PathBuf,
    paths: HandoffPaths,
    executables: ProviderExecutables,
    claude_sessions_dir: PathBuf,
    notifications: bool,
    managed_handoff_skills: Option<crate::handoff_skill::ManagedHandoffSkills>,
}

impl DetachedWorkerLauncher {
    pub fn new(
        worker_executable: PathBuf,
        paths: HandoffPaths,
        executables: ProviderExecutables,
        claude_sessions_dir: PathBuf,
    ) -> Self {
        Self {
            worker_executable,
            paths,
            executables,
            claude_sessions_dir,
            notifications: false,
            managed_handoff_skills: None,
        }
    }

    pub fn with_managed_handoff_skills(
        mut self,
        managed_handoff_skills: crate::handoff_skill::ManagedHandoffSkills,
    ) -> Self {
        self.managed_handoff_skills = Some(managed_handoff_skills);
        self
    }

    pub fn with_notifications(mut self) -> Self {
        self.notifications = true;
        self
    }
}

impl WorkerLauncher for DetachedWorkerLauncher {
    fn launch(&self, launch: &WorkerLaunch) -> std::io::Result<()> {
        let mut command = Command::new(&self.worker_executable);
        command
            .arg("--handoff-worker")
            .arg("--task-store")
            .arg(&self.paths.task_store)
            .arg("--artifact-root")
            .arg(&self.paths.artifact_root)
            .arg("--claude-executable")
            .arg(&self.executables.claude)
            .arg("--codex-executable")
            .arg(&self.executables.codex)
            .arg("--claude-sessions-dir")
            .arg(&self.claude_sessions_dir)
            .arg("--task-id")
            .arg(&launch.task_id);
        if let Some(skills) = self.managed_handoff_skills.as_ref() {
            let (codex_root, claude_root) = skills.roots();
            command
                .arg("--codex-skills-root")
                .arg(codex_root)
                .arg("--claude-skills-root")
                .arg(claude_root);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if self.notifications {
            command.arg("--notify");
        }
        command.spawn()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionNotice {
    pub task_id: String,
    pub state: OperationTaskState,
    pub source_provider: Provider,
    pub source_session_id: String,
}

pub trait CompletionNotifier {
    fn notify(&self, notice: &CompletionNotice) -> std::io::Result<()>;
}

struct NoopCompletionNotifier;

impl CompletionNotifier for NoopCompletionNotifier {
    fn notify(&self, _notice: &CompletionNotice) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct MacosCompletionNotifier;

impl CompletionNotifier for MacosCompletionNotifier {
    fn notify(&self, notice: &CompletionNotice) -> std::io::Result<()> {
        let message = format!(
            "Handoff {} for {}:{}",
            notice.state.label(),
            notice.source_provider.label(),
            crate::session_labels::SessionLabels::short_id(&notice.source_session_id)
        );
        let status = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv)",
                "-e",
                "end run",
                "--",
                "SessionMesh",
                message.as_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("macOS notification failed"))
        }
    }
}

#[derive(Debug)]
pub enum HandoffStartError {
    Ineligible(&'static str),
    ManagedSkill(anyhow::Error),
    Store(anyhow::Error),
}

impl fmt::Display for HandoffStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ineligible(reason) => write!(formatter, "Handoff is ineligible: {reason}"),
            Self::ManagedSkill(error) => {
                write!(formatter, "Handoff Skill preflight failed: {error}")
            }
            Self::Store(error) => write!(formatter, "Task Store error: {error}"),
        }
    }
}

impl std::error::Error for HandoffStartError {}

pub struct HandoffService<L> {
    paths: HandoffPaths,
    executables: ProviderExecutables,
    launcher: L,
    store: TaskStore,
    current_run_control: Arc<dyn CurrentRunControl>,
    managed_handoff_skills: Option<crate::handoff_skill::ManagedHandoffSkills>,
}

impl<L: WorkerLauncher> HandoffService<L> {
    pub fn open(
        paths: HandoffPaths,
        executables: ProviderExecutables,
        launcher: L,
    ) -> Result<Self> {
        let store = TaskStore::open(&paths.task_store)?;
        Ok(Self {
            paths,
            executables,
            launcher,
            store,
            current_run_control: Arc::new(NoopCurrentRunControl),
            managed_handoff_skills: None,
        })
    }

    pub fn open_with_current_run_control(
        paths: HandoffPaths,
        executables: ProviderExecutables,
        launcher: L,
        current_run_control: Arc<dyn CurrentRunControl>,
    ) -> Result<Self> {
        let store = TaskStore::open(&paths.task_store)?;
        Ok(Self {
            paths,
            executables,
            launcher,
            store,
            current_run_control,
            managed_handoff_skills: None,
        })
    }

    pub fn with_managed_handoff_skills(
        mut self,
        managed_handoff_skills: crate::handoff_skill::ManagedHandoffSkills,
    ) -> Self {
        self.managed_handoff_skills = Some(managed_handoff_skills);
        self
    }

    pub fn authorize_exited(
        &mut self,
        session: &Session,
        now_ms: i64,
    ) -> Result<OperationTask, HandoffStartError> {
        self.authorize_exited_with_retry(session, session.provider, now_ms, None)
    }

    pub fn authorize_exited_to(
        &mut self,
        session: &Session,
        successor_provider: Provider,
        now_ms: i64,
    ) -> Result<OperationTask, HandoffStartError> {
        self.authorize_exited_with_retry(session, successor_provider, now_ms, None)
    }

    fn authorize_exited_with_retry(
        &mut self,
        session: &Session,
        successor_provider: Provider,
        now_ms: i64,
        retry_of_task_id: Option<String>,
    ) -> Result<OperationTask, HandoffStartError> {
        self.preflight_exited(session, successor_provider)?;
        let task = OperationTask {
            id: new_task_id(),
            state: OperationTaskState::Queued,
            source_provider: session.provider,
            source_session_id: session.id.clone(),
            source_record_path: session.source.path.clone(),
            source_working_directory: PathBuf::from(session.cwd.as_deref().unwrap_or_default()),
            successor_provider,
            successor_executable: self
                .executables
                .for_provider(successor_provider)
                .to_path_buf(),
            created_at_ms: now_ms,
            state_changed_at_ms: now_ms,
            worker_process_identity: None,
            provider_process_identity: None,
            prompt_sent_at_ms: None,
            provider_exited_at_ms: None,
            provider_exit_code: None,
            running_deadline_ms: None,
            cancellation_requested_at_ms: None,
            termination_requested_at_ms: None,
            retry_of_task_id,
            retry_ready_at_ms: None,
            artifact_path: None,
            successor_launch_plan: None,
            successor_launch_metadata: None,
            successor_session_id: None,
            successor_linked_at_ms: None,
            failure_code: None,
            acknowledged_at_ms: None,
            end_current_run_authorized: false,
            source_process_identity: None,
            waiting_reason: None,
            waiting_deadline_ms: None,
            control_used_at_ms: None,
            deletion_requested_at_ms: None,
            deletion_failure_code: None,
        };
        self.persist_and_launch(task, now_ms)
    }

    pub fn authorize_session(
        &mut self,
        session: &Session,
        end_current_run: bool,
        now_ms: i64,
    ) -> Result<OperationTask, HandoffStartError> {
        self.authorize_session_with_retry(session, session.provider, end_current_run, now_ms, None)
    }

    pub fn authorize_session_to(
        &mut self,
        session: &Session,
        successor_provider: Provider,
        end_current_run: bool,
        now_ms: i64,
    ) -> Result<OperationTask, HandoffStartError> {
        self.authorize_session_with_retry(
            session,
            successor_provider,
            end_current_run,
            now_ms,
            None,
        )
    }

    fn authorize_session_with_retry(
        &mut self,
        session: &Session,
        successor_provider: Provider,
        end_current_run: bool,
        now_ms: i64,
        retry_of_task_id: Option<String>,
    ) -> Result<OperationTask, HandoffStartError> {
        if session.state == SessionState::Exited {
            return self.authorize_exited_with_retry(
                session,
                successor_provider,
                now_ms,
                retry_of_task_id,
            );
        }
        if !end_current_run {
            return Err(HandoffStartError::Ineligible(
                "ending the Current Run was not authorized",
            ));
        }
        self.preflight_common(session, successor_provider)?;
        let observation = self
            .current_run_control
            .observe(session.provider, &session.id, &session.source.path)
            .map_err(|_| {
                HandoffStartError::Ineligible("exact Current Run process identity is unavailable")
            })?;
        let (process, waiting_reason) = match observation {
            CurrentRunObservation::Busy(process) => {
                (process, Some("source_current_run_busy".to_string()))
            }
            CurrentRunObservation::Idle(process) => {
                (process, Some("source_current_run_idle".to_string()))
            }
            CurrentRunObservation::Exited => {
                return Err(HandoffStartError::Ineligible(
                    "source Current Run is no longer Open",
                ));
            }
        };
        let task = OperationTask {
            id: new_task_id(),
            state: OperationTaskState::Waiting,
            source_provider: session.provider,
            source_session_id: session.id.clone(),
            source_record_path: session.source.path.clone(),
            source_working_directory: PathBuf::from(session.cwd.as_deref().unwrap_or_default()),
            successor_provider,
            successor_executable: self
                .executables
                .for_provider(successor_provider)
                .to_path_buf(),
            created_at_ms: now_ms,
            state_changed_at_ms: now_ms,
            worker_process_identity: None,
            provider_process_identity: None,
            prompt_sent_at_ms: None,
            provider_exited_at_ms: None,
            provider_exit_code: None,
            running_deadline_ms: None,
            cancellation_requested_at_ms: None,
            termination_requested_at_ms: None,
            retry_of_task_id,
            retry_ready_at_ms: None,
            artifact_path: None,
            successor_launch_plan: None,
            successor_launch_metadata: None,
            successor_session_id: None,
            successor_linked_at_ms: None,
            failure_code: None,
            acknowledged_at_ms: None,
            end_current_run_authorized: true,
            source_process_identity: Some(process),
            waiting_reason,
            waiting_deadline_ms: Some(now_ms + 2 * 60 * 60 * 1_000),
            control_used_at_ms: None,
            deletion_requested_at_ms: None,
            deletion_failure_code: None,
        };
        self.persist_and_launch(task, now_ms)
    }

    fn persist_and_launch(
        &mut self,
        task: OperationTask,
        now_ms: i64,
    ) -> Result<OperationTask, HandoffStartError> {
        self.store.insert(&task).map_err(HandoffStartError::Store)?;
        if self
            .launcher
            .launch(&WorkerLaunch {
                task_id: task.id.clone(),
            })
            .is_err()
        {
            self.store
                .mark_failed(&task.id, now_ms, "worker_start_failed")
                .map_err(HandoffStartError::Store)?;
            return self
                .store
                .get(&task.id)
                .map_err(HandoffStartError::Store)?
                .ok_or_else(|| {
                    HandoffStartError::Store(anyhow!("Operation Task {} disappeared", task.id))
                });
        }
        Ok(task)
    }

    pub fn task(&self, task_id: &str) -> Result<Option<OperationTask>> {
        self.store.get(task_id)
    }

    pub fn tasks(&self) -> Result<Vec<OperationTask>> {
        self.store.list()
    }

    pub fn acknowledge_task(&self, task_id: &str, now_ms: i64) -> Result<OperationTask> {
        self.store.acknowledge(task_id, now_ms)
    }

    pub fn delete_task(&self, task_id: &str, now_ms: i64) -> Result<()> {
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        if !task.state.is_terminal() {
            return Err(anyhow!("Task is not terminal"));
        }
        self.store.request_deletion(task_id, now_ms)?;
        if let Err(failure) = remove_task_owned_artifact(&self.paths, &task) {
            self.store.record_deletion_failure(task_id, failure.code)?;
            return Err(failure.error);
        }
        if let Err(error) = self.store.delete_terminal(task_id) {
            self.store
                .record_deletion_failure(task_id, "task_record_remove_failed")?;
            return Err(error);
        }
        Ok(())
    }

    pub fn cancel_task(&self, task_id: &str, now_ms: i64) -> Result<OperationTask> {
        if self.store.cancel_before_execution(task_id, now_ms)? {
            return self
                .store
                .get(task_id)?
                .ok_or_else(|| anyhow!("Operation Task {task_id} disappeared"));
        }
        self.store.request_cancellation(task_id, now_ms)?;
        let task = self
            .store
            .get(task_id)?
            .ok_or_else(|| anyhow!("Operation Task {task_id} disappeared"))?;
        let Some(provider) = task.provider_process_identity else {
            self.store
                .mark_cancelled(task_id, now_ms, "cancelled_by_user")?;
            return self
                .store
                .get(task_id)?
                .ok_or_else(|| anyhow!("Operation Task {task_id} disappeared"));
        };
        if task.termination_requested_at_ms.is_some() {
            return Ok(task);
        }
        if !crate::providers::state::process_identity_is_live(provider) {
            self.store
                .mark_cancelled(task_id, now_ms, "cancelled_by_user")?;
        } else {
            self.store.mark_termination_requested(task_id, now_ms)?;
            if request_graceful_process_exit(provider).is_err() {
                self.store
                    .mark_interrupted(task_id, now_ms, "provider_termination_uncertain")?;
            }
        }
        self.store
            .get(task_id)?
            .ok_or_else(|| anyhow!("Operation Task {task_id} disappeared"))
    }

    pub fn reconcile_tasks(&self, now_ms: i64) -> Result<Vec<OperationTask>> {
        for task in self.store.list()? {
            if task.deletion_requested_at_ms.is_some() {
                let _ = self.delete_task(&task.id, now_ms);
                continue;
            }
            if task.state == OperationTaskState::Interrupted
                && task.retry_ready_at_ms.is_none()
                && prior_execution_processes_are_absent(&task)
            {
                self.store.mark_retry_ready(&task.id, now_ms)?;
                continue;
            }
            if task.state == OperationTaskState::Running
                && reconcile_declared_result(&self.store, &self.paths, &task, now_ms)?
            {
                continue;
            }
            if task.state == OperationTaskState::Queued
                && task.created_at_ms.saturating_add(30_000) < now_ms
            {
                self.store
                    .mark_failed(&task.id, now_ms, "worker_start_deadline_exceeded")?;
            } else if task.state == OperationTaskState::Waiting
                && task.worker_process_identity.is_some_and(|process| {
                    !crate::providers::state::process_identity_is_live(process)
                })
            {
                if task.control_used_at_ms.is_none() {
                    self.store
                        .mark_failed(&task.id, now_ms, "worker_lost_before_control")?;
                } else {
                    let observation = self.current_run_control.observe(
                        task.source_provider,
                        &task.source_session_id,
                        &task.source_record_path,
                    );
                    match observation {
                        Ok(
                            CurrentRunObservation::Busy(process)
                            | CurrentRunObservation::Idle(process),
                        ) if Some(process) == task.source_process_identity => {}
                        Ok(CurrentRunObservation::Exited) => {
                            self.store.mark_failed(
                                &task.id,
                                now_ms,
                                "worker_lost_after_current_run_control",
                            )?;
                        }
                        Ok(CurrentRunObservation::Busy(_) | CurrentRunObservation::Idle(_))
                        | Err(_) => {
                            self.store.mark_interrupted(
                                &task.id,
                                now_ms,
                                "current_run_reconciliation_uncertain",
                            )?;
                        }
                    }
                }
            } else if task.state == OperationTaskState::Running
                && task
                    .running_deadline_ms
                    .is_some_and(|deadline| deadline < now_ms)
                && task.termination_requested_at_ms.is_none()
            {
                let Some(provider) = task.provider_process_identity else {
                    continue;
                };
                if crate::providers::state::process_identity_is_live(provider) {
                    self.store.mark_termination_requested(&task.id, now_ms)?;
                    if request_graceful_process_exit(provider).is_err() {
                        self.store.mark_interrupted(
                            &task.id,
                            now_ms,
                            "provider_termination_uncertain",
                        )?;
                    }
                }
            } else if task.state == OperationTaskState::Running {
                let worker_live = task
                    .worker_process_identity
                    .is_some_and(crate::providers::state::process_identity_is_live);
                let provider_live = task
                    .provider_process_identity
                    .is_some_and(crate::providers::state::process_identity_is_live);
                if worker_live || provider_live {
                    continue;
                }
                if task.cancellation_requested_at_ms.is_some() {
                    self.store
                        .mark_cancelled(&task.id, now_ms, "cancelled_by_user")?;
                } else if task.termination_requested_at_ms.is_some() {
                    self.store
                        .mark_failed(&task.id, now_ms, "running_deadline_exceeded")?;
                } else if task.prompt_sent_at_ms.is_some()
                    || task.provider_process_identity.is_some()
                {
                    self.store
                        .mark_interrupted(&task.id, now_ms, "execution_outcome_uncertain")?;
                    self.store.mark_retry_ready(&task.id, now_ms)?;
                } else {
                    self.store
                        .mark_failed(&task.id, now_ms, "worker_lost_before_prompt")?;
                }
            }
        }
        self.store.list()
    }

    pub fn retry_task(
        &mut self,
        predecessor_task_id: &str,
        session: &Session,
        end_current_run: bool,
        now_ms: i64,
    ) -> Result<OperationTask, HandoffStartError> {
        self.retry_task_to(
            predecessor_task_id,
            session,
            session.provider,
            end_current_run,
            now_ms,
        )
    }

    pub fn retry_task_to(
        &mut self,
        predecessor_task_id: &str,
        session: &Session,
        successor_provider: Provider,
        end_current_run: bool,
        now_ms: i64,
    ) -> Result<OperationTask, HandoffStartError> {
        let predecessor = self
            .store
            .get(predecessor_task_id)
            .map_err(HandoffStartError::Store)?
            .ok_or(HandoffStartError::Ineligible(
                "predecessor Operation Task is unavailable",
            ))?;
        if predecessor.deletion_requested_at_ms.is_some() {
            return Err(HandoffStartError::Ineligible(
                "predecessor Task is being deleted",
            ));
        }
        let interrupted_processes_are_absent = predecessor.state == OperationTaskState::Interrupted
            && predecessor.retry_ready_at_ms.is_some();
        if !matches!(
            predecessor.state,
            OperationTaskState::Failed | OperationTaskState::Cancelled
        ) && !interrupted_processes_are_absent
        {
            return Err(HandoffStartError::Ineligible(
                "predecessor Task does not offer Retry",
            ));
        }
        if predecessor.source_provider != session.provider
            || predecessor.source_session_id != session.id
        {
            return Err(HandoffStartError::Ineligible(
                "source Session does not match the predecessor Task",
            ));
        }
        self.authorize_session_with_retry(
            session,
            successor_provider,
            end_current_run,
            now_ms,
            Some(predecessor.id),
        )
    }

    fn preflight_exited(
        &self,
        session: &Session,
        successor_provider: Provider,
    ) -> Result<(), HandoffStartError> {
        if session.state != SessionState::Exited {
            return Err(HandoffStartError::Ineligible("source Session is Open"));
        }
        self.preflight_common(session, successor_provider)
    }

    fn preflight_common(
        &self,
        session: &Session,
        successor_provider: Provider,
    ) -> Result<(), HandoffStartError> {
        if session.id.is_empty() {
            return Err(HandoffStartError::Ineligible(
                "native Session ID is unavailable",
            ));
        }
        if self
            .store
            .has_non_terminal(session.provider, &session.id)
            .map_err(HandoffStartError::Store)?
        {
            return Err(HandoffStartError::Ineligible(
                "source Session already has a non-terminal Operation Task",
            ));
        }
        if !is_executable(self.executables.for_provider(session.provider)) {
            return Err(HandoffStartError::Ineligible(
                "source Provider CLI is unavailable",
            ));
        }
        if !is_executable(self.executables.for_provider(successor_provider)) {
            return Err(HandoffStartError::Ineligible(
                "selected Successor Provider CLI is unavailable",
            ));
        }
        if !session.source.path.is_file() || fs::File::open(&session.source.path).is_err() {
            return Err(HandoffStartError::Ineligible(
                "Provider source record is unavailable",
            ));
        }
        let Some(cwd) = session.cwd.as_deref().filter(|cwd| !cwd.is_empty()) else {
            return Err(HandoffStartError::Ineligible(
                "source working directory is unavailable",
            ));
        };
        if fs::read_dir(cwd).is_err() {
            return Err(HandoffStartError::Ineligible(
                "source working directory is unreadable",
            ));
        }
        if let Some(skills) = self.managed_handoff_skills.as_ref() {
            skills
                .install_for_first_use()
                .map_err(HandoffStartError::ManagedSkill)?;
        }
        Ok(())
    }
}

struct ArtifactDeletionFailure {
    code: &'static str,
    error: anyhow::Error,
}

impl ArtifactDeletionFailure {
    fn validation(error: anyhow::Error) -> Self {
        Self {
            code: "artifact_validation_failed",
            error,
        }
    }

    fn removal(error: impl Into<anyhow::Error>) -> Self {
        Self {
            code: "artifact_remove_failed",
            error: error.into(),
        }
    }
}

fn remove_task_owned_artifact(
    paths: &HandoffPaths,
    task: &OperationTask,
) -> std::result::Result<(), ArtifactDeletionFailure> {
    let Some(artifact_path) = task.artifact_path.as_ref() else {
        return Ok(());
    };
    let mut task_id_components = Path::new(&task.id).components();
    if !matches!(
        (task_id_components.next(), task_id_components.next()),
        (Some(std::path::Component::Normal(_)), None)
    ) {
        return Err(ArtifactDeletionFailure::validation(anyhow!(
            "Task ID is invalid for Artifact ownership"
        )));
    }
    let expected = paths.artifact_root.join(&task.id).join("handoff.md");
    if artifact_path != &expected {
        return Err(ArtifactDeletionFailure::validation(anyhow!(
            "Task-owned Handoff Artifact path is invalid"
        )));
    }
    let task_directory = expected
        .parent()
        .expect("Artifact path has a Task directory");
    let root_metadata = existing_real_directory(&paths.artifact_root, "Artifact root")?;
    let task_directory_metadata =
        existing_real_directory(task_directory, "Task Artifact directory")?;
    let artifact_metadata = existing_real_file(artifact_path, "Task-owned Handoff Artifact")?;

    validate_canonical_artifact_location(
        &paths.artifact_root,
        task_directory,
        artifact_path,
        root_metadata.as_ref(),
        task_directory_metadata.as_ref(),
        artifact_metadata.as_ref(),
    )?;
    validate_artifact_ownership(
        paths,
        [
            ("Artifact root", root_metadata.as_ref()),
            ("Task Artifact directory", task_directory_metadata.as_ref()),
            ("Task-owned Handoff Artifact", artifact_metadata.as_ref()),
        ],
    )?;

    remove_if_present(artifact_path, false)?;
    remove_if_present(task_directory, true)?;
    Ok(())
}

fn existing_real_directory(
    path: &Path,
    label: &str,
) -> std::result::Result<Option<fs::Metadata>, ArtifactDeletionFailure> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => Err(
            ArtifactDeletionFailure::validation(anyhow!("{label} is not a real directory")),
        ),
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ArtifactDeletionFailure::validation(error.into())),
    }
}

fn existing_real_file(
    path: &Path,
    label: &str,
) -> std::result::Result<Option<fs::Metadata>, ArtifactDeletionFailure> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            ArtifactDeletionFailure::validation(anyhow!("{label} is not a real file")),
        ),
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ArtifactDeletionFailure::validation(error.into())),
    }
}

fn validate_canonical_artifact_location(
    root: &Path,
    task_directory: &Path,
    artifact: &Path,
    root_metadata: Option<&fs::Metadata>,
    task_directory_metadata: Option<&fs::Metadata>,
    artifact_metadata: Option<&fs::Metadata>,
) -> std::result::Result<(), ArtifactDeletionFailure> {
    let canonicalize_existing = |path: &Path, metadata: Option<&fs::Metadata>| {
        metadata
            .map(|_| fs::canonicalize(path))
            .transpose()
            .map_err(|error| ArtifactDeletionFailure::validation(error.into()))
    };
    let canonical_root = canonicalize_existing(root, root_metadata)?;
    let canonical_task_directory = canonicalize_existing(task_directory, task_directory_metadata)?;
    let canonical_artifact = canonicalize_existing(artifact, artifact_metadata)?;
    if let (Some(root), Some(task_directory)) =
        (canonical_root.as_ref(), canonical_task_directory.as_ref())
    {
        if task_directory.parent() != Some(root.as_path()) {
            return Err(ArtifactDeletionFailure::validation(anyhow!(
                "Task Artifact directory is outside the canonical Artifact root"
            )));
        }
    }
    if let (Some(task_directory), Some(artifact)) = (
        canonical_task_directory.as_ref(),
        canonical_artifact.as_ref(),
    ) {
        if artifact.parent() != Some(task_directory.as_path()) {
            return Err(ArtifactDeletionFailure::validation(anyhow!(
                "Task-owned Handoff Artifact is outside its canonical Task directory"
            )));
        }
    }
    Ok(())
}

fn validate_artifact_ownership<const N: usize>(
    paths: &HandoffPaths,
    entries: [(&str, Option<&fs::Metadata>); N],
) -> std::result::Result<(), ArtifactDeletionFailure> {
    #[cfg(unix)]
    {
        let store_owner = fs::metadata(&paths.task_store)
            .map_err(|error| ArtifactDeletionFailure::validation(error.into()))?
            .uid();
        for (label, metadata) in entries {
            let Some(metadata) = metadata else {
                continue;
            };
            if metadata.uid() != store_owner {
                return Err(ArtifactDeletionFailure::validation(anyhow!(
                    "{label} is not owned by the Task Store owner"
                )));
            }
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(ArtifactDeletionFailure::validation(anyhow!(
                    "{label} is not owner-only"
                )));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (paths, entries);
    }
    Ok(())
}

fn remove_if_present(
    path: &Path,
    directory: bool,
) -> std::result::Result<(), ArtifactDeletionFailure> {
    let result = if directory {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ArtifactDeletionFailure::removal(error)),
    }
}

fn prior_execution_processes_are_absent(task: &OperationTask) -> bool {
    let worker_absent = task
        .worker_process_identity
        .is_some_and(|process| !crate::providers::state::process_identity_is_live(process));
    let provider_absent = match task.provider_process_identity {
        Some(process) => !crate::providers::state::process_identity_is_live(process),
        None => task.prompt_sent_at_ms.is_none(),
    };
    worker_absent && provider_absent
}

fn reconcile_declared_result(
    store: &TaskStore,
    paths: &HandoffPaths,
    task: &OperationTask,
    now_ms: i64,
) -> Result<bool> {
    let artifact_path = paths.artifact_root.join(&task.id).join("handoff.md");
    let Ok(candidate) = fs::read_to_string(&artifact_path) else {
        return Ok(false);
    };
    if validate_handoff_artifact(&candidate).is_err() {
        return Ok(false);
    }
    let plan = crate::successor::build_launch_plan(task, artifact_path.clone());
    store.mark_ready(&task.id, now_ms, &artifact_path, &plan)?;
    Ok(true)
}

pub struct HandoffWorker {
    paths: HandoffPaths,
    executables: ProviderExecutables,
    current_run_control: Arc<dyn CurrentRunControl>,
    managed_handoff_skills: Option<crate::handoff_skill::ManagedHandoffSkills>,
}

impl HandoffWorker {
    pub fn new(paths: HandoffPaths, executables: ProviderExecutables) -> Self {
        Self {
            paths,
            executables,
            current_run_control: Arc::new(NoopCurrentRunControl),
            managed_handoff_skills: None,
        }
    }

    pub fn new_with_current_run_control(
        paths: HandoffPaths,
        executables: ProviderExecutables,
        current_run_control: Arc<dyn CurrentRunControl>,
    ) -> Self {
        Self {
            paths,
            executables,
            current_run_control,
            managed_handoff_skills: None,
        }
    }

    pub fn with_managed_handoff_skills(
        mut self,
        managed_handoff_skills: crate::handoff_skill::ManagedHandoffSkills,
    ) -> Self {
        self.managed_handoff_skills = Some(managed_handoff_skills);
        self
    }

    pub fn run(&self, task_id: &str, now_ms: i64) -> Result<OperationTask> {
        self.run_with_notifier(task_id, now_ms, &NoopCompletionNotifier)
    }

    pub fn run_with_notifier(
        &self,
        task_id: &str,
        now_ms: i64,
        notifier: &dyn CompletionNotifier,
    ) -> Result<OperationTask> {
        let store = TaskStore::open(&self.paths.task_store)?;
        let task = store
            .get(task_id)?
            .ok_or_else(|| anyhow!("Operation Task {task_id} was not found"))?;
        if task.state == OperationTaskState::Running {
            drop(store);
            let reconciler = HandoffService::open_with_current_run_control(
                self.paths.clone(),
                self.executables.clone(),
                ReconciliationOnlyLauncher,
                self.current_run_control.clone(),
            )?;
            reconciler.reconcile_tasks(now_ms)?;
            return reconciler
                .task(task_id)?
                .ok_or_else(|| anyhow!("Operation Task {task_id} disappeared"));
        }
        let transition_now_ms = || {
            if task.state == OperationTaskState::Waiting {
                self.current_run_control.now_ms()
            } else {
                now_ms
            }
        };
        let mut control_used = false;
        let worker_process = crate::providers::state::process_identity(std::process::id())
            .ok_or_else(|| anyhow!("detached Worker process identity is unavailable"))?;
        if task.state == OperationTaskState::Waiting {
            store.mark_waiting_worker_started(task_id, worker_process)?;
            loop {
                if store
                    .get(task_id)?
                    .is_some_and(|task| task.state.is_terminal())
                {
                    return Err(anyhow!("Task became terminal while Waiting"));
                }
                if store.cancel_expired_waiting(task_id, self.current_run_control.now_ms())? {
                    return completed_task(&store, task_id, notifier);
                }
                let observation = match self.current_run_control.observe(
                    task.source_provider,
                    &task.source_session_id,
                    &task.source_record_path,
                ) {
                    Ok(observation) => observation,
                    Err(_) => {
                        return interrupt_task(
                            &store,
                            task_id,
                            self.current_run_control.now_ms(),
                            "source_current_run_observation_uncertain",
                            notifier,
                        );
                    }
                };
                match observation {
                    CurrentRunObservation::Busy(process)
                        if Some(process) == task.source_process_identity =>
                    {
                        self.current_run_control.wait_before_recheck();
                    }
                    CurrentRunObservation::Idle(process)
                        if !control_used && Some(process) == task.source_process_identity =>
                    {
                        let revalidated = match self.current_run_control.observe(
                            task.source_provider,
                            &task.source_session_id,
                            &task.source_record_path,
                        ) {
                            Ok(observation) => observation,
                            Err(_) => {
                                return interrupt_task(
                                    &store,
                                    task_id,
                                    self.current_run_control.now_ms(),
                                    "source_current_run_observation_uncertain",
                                    notifier,
                                );
                            }
                        };
                        if revalidated == CurrentRunObservation::Exited {
                            break;
                        }
                        if revalidated == CurrentRunObservation::Busy(process) {
                            self.current_run_control.wait_before_recheck();
                            continue;
                        }
                        if revalidated != CurrentRunObservation::Idle(process) {
                            return interrupt_task(
                                &store,
                                task_id,
                                self.current_run_control.now_ms(),
                                "source_current_run_identity_changed",
                                notifier,
                            );
                        }
                        store.mark_control_used(
                            task_id,
                            self.current_run_control.now_ms(),
                            worker_process,
                        )?;
                        if self
                            .current_run_control
                            .request_graceful_exit(process)
                            .is_err()
                        {
                            return interrupt_task(
                                &store,
                                task_id,
                                self.current_run_control.now_ms(),
                                "source_current_run_control_uncertain",
                                notifier,
                            );
                        }
                        control_used = true;
                        self.current_run_control.wait_before_recheck();
                    }
                    CurrentRunObservation::Idle(process)
                        if control_used && Some(process) == task.source_process_identity =>
                    {
                        self.current_run_control.wait_before_recheck();
                    }
                    CurrentRunObservation::Exited => break,
                    CurrentRunObservation::Busy(_) | CurrentRunObservation::Idle(_) => {
                        return interrupt_task(
                            &store,
                            task_id,
                            self.current_run_control.now_ms(),
                            "source_current_run_identity_changed",
                            notifier,
                        );
                    }
                }
            }
        }
        if !control_used {
            store.mark_running(task_id, transition_now_ms(), worker_process)?;
        }

        if let Some(skills) = self.managed_handoff_skills.as_ref() {
            if let Err(status) = skills.verify(task.source_provider) {
                store.mark_failed(task_id, transition_now_ms(), status.failure_code())?;
                return completed_task(&store, task_id, notifier);
            }
        }

        if store
            .get(task_id)?
            .is_some_and(|task| task.state.is_terminal())
        {
            return Err(anyhow!("Task became terminal before Provider startup"));
        }

        let candidate = match run_source_provider(
            self.executables.for_provider(task.source_provider),
            &task,
            &store,
            transition_now_ms(),
        ) {
            Ok(candidate) => candidate,
            Err(ProviderRunError::Failed(candidate)) => {
                if let Some(candidate) =
                    candidate.filter(|candidate| validate_handoff_artifact(candidate).is_ok())
                {
                    return complete_valid_candidate(
                        &store,
                        &self.paths,
                        &task,
                        &candidate,
                        transition_now_ms(),
                        notifier,
                    );
                }
                if finish_cancelled_provider(&store, task_id, transition_now_ms())? {
                    return completed_task(&store, task_id, notifier);
                }
                if finish_deadline_terminated_provider(&store, task_id, transition_now_ms())? {
                    return completed_task(&store, task_id, notifier);
                }
                store.mark_failed(task_id, transition_now_ms(), "provider_invocation_failed")?;
                return completed_task(&store, task_id, notifier);
            }
            Err(ProviderRunError::Interrupted(candidate)) => {
                if let Some(candidate) =
                    candidate.filter(|candidate| validate_handoff_artifact(candidate).is_ok())
                {
                    return complete_valid_candidate(
                        &store,
                        &self.paths,
                        &task,
                        &candidate,
                        transition_now_ms(),
                        notifier,
                    );
                }
                if finish_cancelled_provider(&store, task_id, transition_now_ms())? {
                    return completed_task(&store, task_id, notifier);
                }
                if finish_deadline_terminated_provider(&store, task_id, transition_now_ms())? {
                    return completed_task(&store, task_id, notifier);
                }
                store.mark_interrupted(
                    task_id,
                    transition_now_ms(),
                    "provider_outcome_uncertain",
                )?;
                return completed_task(&store, task_id, notifier);
            }
        };
        if validate_handoff_artifact(&candidate).is_err() {
            if finish_cancelled_provider(&store, task_id, transition_now_ms())? {
                return completed_task(&store, task_id, notifier);
            }
            if finish_deadline_terminated_provider(&store, task_id, transition_now_ms())? {
                return completed_task(&store, task_id, notifier);
            }
            store.mark_failed(task_id, transition_now_ms(), "invalid_artifact")?;
            return completed_task(&store, task_id, notifier);
        }
        complete_valid_candidate(
            &store,
            &self.paths,
            &task,
            &candidate,
            transition_now_ms(),
            notifier,
        )
    }
}

fn complete_valid_candidate(
    store: &TaskStore,
    paths: &HandoffPaths,
    task: &OperationTask,
    candidate: &str,
    now_ms: i64,
    notifier: &dyn CompletionNotifier,
) -> Result<OperationTask> {
    let artifact_path = match publish_artifact(&paths.artifact_root, &task.id, candidate.as_bytes())
    {
        Ok(path) => path,
        Err(_) => {
            store.mark_failed(&task.id, now_ms, "artifact_publish_failed")?;
            return completed_task(store, &task.id, notifier);
        }
    };
    let plan = crate::successor::build_launch_plan(task, artifact_path.clone());
    store.mark_ready(&task.id, now_ms, &artifact_path, &plan)?;
    completed_task(store, &task.id, notifier)
}

fn finish_cancelled_provider(store: &TaskStore, task_id: &str, now_ms: i64) -> Result<bool> {
    let task = store
        .get(task_id)?
        .ok_or_else(|| anyhow!("Operation Task {task_id} disappeared"))?;
    let cancelled = task.cancellation_requested_at_ms.is_some()
        && task
            .provider_process_identity
            .is_some_and(|process| !crate::providers::state::process_identity_is_live(process));
    if cancelled {
        store.mark_cancelled(task_id, now_ms, "cancelled_by_user")?;
    }
    Ok(cancelled)
}

fn finish_deadline_terminated_provider(
    store: &TaskStore,
    task_id: &str,
    now_ms: i64,
) -> Result<bool> {
    let task = store
        .get(task_id)?
        .ok_or_else(|| anyhow!("Operation Task {task_id} disappeared"))?;
    let deadline_terminated = task.cancellation_requested_at_ms.is_none()
        && task.termination_requested_at_ms.is_some()
        && task
            .provider_process_identity
            .is_some_and(|process| !crate::providers::state::process_identity_is_live(process));
    if deadline_terminated {
        store.mark_failed(task_id, now_ms, "running_deadline_exceeded")?;
    }
    Ok(deadline_terminated)
}

fn request_graceful_process_exit(process: ProcessIdentity) -> Result<()> {
    if !crate::providers::state::process_identity_is_live(process) {
        return Err(anyhow!(
            "Provider CLI process identity changed before graceful exit"
        ));
    }
    let status = Command::new("/bin/kill")
        .args(["-TERM", &process.pid.to_string()])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("graceful Provider CLI exit request failed"))
    }
}

fn interrupt_task(
    store: &TaskStore,
    task_id: &str,
    now_ms: i64,
    failure_code: &str,
    notifier: &dyn CompletionNotifier,
) -> Result<OperationTask> {
    store.mark_interrupted(task_id, now_ms, failure_code)?;
    completed_task(store, task_id, notifier)
}

fn completed_task(
    store: &TaskStore,
    task_id: &str,
    notifier: &dyn CompletionNotifier,
) -> Result<OperationTask> {
    let task = store
        .get(task_id)?
        .ok_or_else(|| anyhow!("Operation Task {task_id} disappeared"))?;
    let _ = notifier.notify(&CompletionNotice {
        task_id: task.id.clone(),
        state: task.state,
        source_provider: task.source_provider,
        source_session_id: task.source_session_id.clone(),
    });
    Ok(task)
}

/// Run the internal detached-Worker command when present in process arguments.
///
/// Returns false for a normal TUI invocation.
pub fn run_worker_from_args<I, S>(args: I) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();
    if args.next().as_deref() != Some(OsStr::new("--handoff-worker")) {
        return Ok(false);
    }
    let task_store = required_worker_arg(&mut args, "--task-store")?;
    let artifact_root = required_worker_arg(&mut args, "--artifact-root")?;
    let claude = required_worker_arg(&mut args, "--claude-executable")?;
    let codex = required_worker_arg(&mut args, "--codex-executable")?;
    let claude_sessions_dir = required_worker_arg(&mut args, "--claude-sessions-dir")?;
    let task_id = required_worker_arg(&mut args, "--task-id")?
        .into_string()
        .map_err(|_| anyhow!("Task ID is not valid UTF-8"))?;
    let mut codex_skills_root = None;
    let mut claude_skills_root = None;
    let mut notify = false;
    while let Some(flag) = args.next() {
        if flag == OsStr::new("--codex-skills-root") {
            codex_skills_root = Some(
                args.next()
                    .ok_or_else(|| anyhow!("missing value for --codex-skills-root"))?,
            );
        } else if flag == OsStr::new("--claude-skills-root") {
            claude_skills_root = Some(
                args.next()
                    .ok_or_else(|| anyhow!("missing value for --claude-skills-root"))?,
            );
        } else if flag == OsStr::new("--notify") && !notify {
            notify = true;
        } else {
            return Err(anyhow!("unexpected detached Worker arguments"));
        }
    }
    let worker = HandoffWorker::new_with_current_run_control(
        HandoffPaths {
            task_store: PathBuf::from(task_store),
            artifact_root: PathBuf::from(artifact_root),
        },
        ProviderExecutables::new(PathBuf::from(claude), PathBuf::from(codex)),
        Arc::new(MacosCurrentRunControl::new(PathBuf::from(
            claude_sessions_dir,
        ))),
    );
    let worker = match (codex_skills_root, claude_skills_root) {
        (Some(codex), Some(claude)) => {
            worker.with_managed_handoff_skills(crate::handoff_skill::ManagedHandoffSkills::new(
                PathBuf::from(codex),
                PathBuf::from(claude),
            ))
        }
        (None, None) => worker,
        _ => return Err(anyhow!("detached Worker requires both managed Skill roots")),
    };
    if notify {
        worker.run_with_notifier(
            &task_id,
            chrono::Utc::now().timestamp_millis(),
            &MacosCompletionNotifier,
        )?;
    } else {
        worker.run(&task_id, chrono::Utc::now().timestamp_millis())?;
    }
    Ok(true)
}

fn required_worker_arg(
    args: &mut impl Iterator<Item = OsString>,
    expected: &str,
) -> Result<OsString> {
    let flag = args
        .next()
        .ok_or_else(|| anyhow!("missing detached Worker argument {expected}"))?;
    if flag != OsStr::new(expected) {
        return Err(anyhow!(
            "expected detached Worker argument {expected}, got {:?}",
            flag
        ));
    }
    args.next()
        .ok_or_else(|| anyhow!("missing value for detached Worker argument {expected}"))
}

enum ProviderRunError {
    Failed(Option<String>),
    Interrupted(Option<String>),
}

fn run_source_provider(
    executable: &Path,
    task: &OperationTask,
    store: &TaskStore,
    now_ms: i64,
) -> std::result::Result<String, ProviderRunError> {
    let mut command = Command::new(executable);
    command.args(
        crate::providers::adapter_for(task.source_provider)
            .handoff_resume_args(&task.source_session_id),
    );
    let mut child = command
        .current_dir(&task.source_working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ProviderRunError::Failed(None))?;
    let Some(provider_process) = crate::providers::state::process_identity_after_spawn(child.id())
    else {
        // A short-lived Provider can exit before its process identity is sampled. Its exit
        // status is still conclusive, so do not report an uncertain outcome prematurely.
        return record_provider_exit_before_prompt(child, task, store, now_ms);
    };
    store
        .mark_provider_started(&task.id, provider_process)
        .map_err(|_| ProviderRunError::Interrupted(None))?;
    let invocation = crate::providers::adapter_for(task.source_provider).handoff_skill_invocation();
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => return record_provider_exit_before_prompt(child, task, store, now_ms),
    };
    if stdin
        .write_all(format!("{invocation}\n").as_bytes())
        .is_err()
    {
        drop(stdin);
        return record_provider_exit_before_prompt(child, task, store, now_ms);
    }
    drop(stdin);
    store
        .mark_prompt_sent(&task.id, now_ms)
        .map_err(|_| ProviderRunError::Interrupted(None))?;
    let output = child
        .wait_with_output()
        .map_err(|_| ProviderRunError::Interrupted(None))?;
    store
        .mark_provider_exited(&task.id, now_ms, output.status.code())
        .map_err(|_| ProviderRunError::Interrupted(None))?;
    let candidate = String::from_utf8(output.stdout).ok();
    if !output.status.success() {
        return Err(if output.status.code().is_some() {
            ProviderRunError::Failed(candidate)
        } else {
            ProviderRunError::Interrupted(candidate)
        });
    }
    candidate.ok_or(ProviderRunError::Failed(None))
}

fn record_provider_exit_before_prompt(
    child: Child,
    task: &OperationTask,
    store: &TaskStore,
    now_ms: i64,
) -> std::result::Result<String, ProviderRunError> {
    let output = child
        .wait_with_output()
        .map_err(|_| ProviderRunError::Interrupted(None))?;
    store
        .mark_provider_exited(&task.id, now_ms, output.status.code())
        .map_err(|_| ProviderRunError::Interrupted(None))?;
    let candidate = String::from_utf8(output.stdout).ok();
    Err(if output.status.code().is_some() {
        ProviderRunError::Failed(candidate)
    } else {
        ProviderRunError::Interrupted(candidate)
    })
}

pub(crate) fn validate_handoff_artifact(candidate: &str) -> Result<()> {
    if candidate.trim().is_empty() {
        return Err(anyhow!("Handoff Artifact is empty"));
    }
    if candidate.contains("-----BEGIN PRIVATE KEY-----")
        || candidate.contains("-----BEGIN RSA PRIVATE KEY-----")
        || candidate.contains("-----BEGIN OPENSSH PRIVATE KEY-----")
    {
        return Err(anyhow!("Handoff Artifact contains secret material"));
    }
    let lines = candidate.lines().collect::<Vec<_>>();
    if contains_heading(&lines, "transcript") {
        return Err(anyhow!("Handoff Artifact contains a Transcript section"));
    }
    if contains_heading(&lines, "tool output") {
        return Err(anyhow!("Handoff Artifact contains a Tool Output section"));
    }
    let required_headings = required_handoff_headings();
    let contract_headings = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("# ") || line.starts_with("## "))
        .collect::<Vec<_>>();
    if contract_headings.as_slice() != required_headings {
        return Err(anyhow!(
            "Handoff Artifact headings do not match the contract"
        ));
    }
    let mut positions = Vec::with_capacity(required_headings.len());
    let mut search_from = 0;
    for heading in required_headings {
        let relative = lines[search_from..]
            .iter()
            .position(|line| *line == heading)
            .ok_or_else(|| anyhow!("Handoff Artifact is missing {heading:?}"))?;
        let position = search_from + relative;
        positions.push(position);
        search_from = position + 1;
    }
    for heading_index in [1, 2, 5, 9] {
        let start = positions[heading_index] + 1;
        let end = positions
            .get(heading_index + 1)
            .copied()
            .unwrap_or(lines.len());
        if !lines[start..end].iter().any(|line| !line.trim().is_empty()) {
            return Err(anyhow!(
                "Handoff Artifact section {:?} is empty",
                required_headings[heading_index]
            ));
        }
    }
    Ok(())
}

pub(crate) fn required_handoff_headings() -> [&'static str; 10] {
    crate::handoff_skill::artifact_headings()
}

fn contains_heading(lines: &[&str], needle: &str) -> bool {
    lines.iter().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('#')
            && trimmed
                .trim_start_matches('#')
                .trim()
                .to_ascii_lowercase()
                .contains(needle)
    })
}

fn publish_artifact(root: &Path, task_id: &str, contents: &[u8]) -> Result<PathBuf> {
    create_private_dir(root)?;
    let task_dir = root.join(task_id);
    create_private_dir(&task_dir)?;
    let final_path = task_dir.join("handoff.md");
    let sequence = NEXT_ARTIFACT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temporary_path =
        task_dir.join(format!(".handoff.md.tmp-{}-{sequence}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut temporary = options.open(&temporary_path)?;
    temporary.write_all(contents)?;
    temporary.sync_all()?;
    fs::rename(&temporary_path, &final_path)?;
    #[cfg(unix)]
    fs::set_permissions(&final_path, fs::Permissions::from_mode(0o600))?;
    fs::File::open(&task_dir)?.sync_all()?;
    Ok(final_path)
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "private directory is not a real directory: {}",
                    path.display()
                ),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            builder.create(path)?;
        }
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn find_executable(search_directories: &[PathBuf], name: &str) -> Option<PathBuf> {
    search_directories
        .iter()
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

fn new_task_id() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
    format!("{time:032x}-{:08x}-{sequence:016x}", std::process::id())
}
