//! Concrete Handoff construction and macOS process integrations.

use crate::adapters::persistence::tasks::TaskStore;
use crate::adapters::providers::{registry, state, ProviderExecutables};
use crate::adapters::skills::{ManagedSkillCatalog, SkillInstaller};
use crate::adapters::successor::RegistrySuccessorRuntime;
use crate::application::handoff::{
    CompletionNotice, CompletionNotifier, CurrentRunControl, CurrentRunObservation, HandoffPaths,
    HandoffService, TaskIdGenerator, WorkerLaunch, WorkerLauncher,
};
use crate::domain::{ProcessIdentity, Provider, SessionState};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(0);

pub struct SystemTaskIdGenerator;

impl TaskIdGenerator for SystemTaskIdGenerator {
    fn new_task_id(&self) -> String {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let sequence = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
        format!("{time:032x}-{:08x}-{sequence:016x}", std::process::id())
    }
}

pub(crate) struct ReconciliationOnlyLauncher;

impl WorkerLauncher for ReconciliationOnlyLauncher {
    fn launch(&self, _launch: &WorkerLaunch) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "reconciliation cannot launch a Worker",
        ))
    }
}

pub(crate) struct NoopCompletionNotifier;

impl CompletionNotifier for NoopCompletionNotifier {
    fn notify(&self, _notice: &CompletionNotice) -> std::io::Result<()> {
        Ok(())
    }
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
        let detector = state::StateDetector::gather(&self.claude_sessions_dir);
        let live = registry().runtime_control(provider)?.observe_current_run(
            &detector,
            source_session_id,
            source_record_path,
        );
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
        super::handoff_worker::request_graceful_process_exit(process)
    }

    fn process_is_live(&self, process: ProcessIdentity) -> bool {
        state::process_identity_is_live(process)
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
    skill_installer: Option<Arc<dyn SkillInstaller>>,
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
            skill_installer: None,
        }
    }

    pub fn with_managed_handoff_skills(
        mut self,
        managed_handoff_skills: crate::adapters::skills::handoff::ManagedHandoffSkills,
    ) -> Self {
        self.skill_installer = Some(Arc::new(ManagedSkillCatalog::handoff(
            managed_handoff_skills,
        )));
        self
    }

    pub fn with_skill_installer(mut self, skill_installer: Arc<dyn SkillInstaller>) -> Self {
        self.skill_installer = Some(skill_installer);
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
            .arg(self.executables.claude_path())
            .arg("--codex-executable")
            .arg(self.executables.codex_path())
            .arg("--claude-sessions-dir")
            .arg(&self.claude_sessions_dir)
            .arg("--task-id")
            .arg(&launch.task_id);
        if let Some((codex_root, claude_root)) = self
            .skill_installer
            .as_ref()
            .and_then(|installer| installer.worker_roots())
        {
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

pub struct MacosCompletionNotifier;

impl CompletionNotifier for MacosCompletionNotifier {
    fn notify(&self, notice: &CompletionNotice) -> std::io::Result<()> {
        let message = format!(
            "Handoff {} for {}:{}",
            notice.state.label(),
            notice.source_provider.label(),
            crate::application::session_labels::short_session_id(&notice.source_session_id)
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
                "Mudi",
                message.as_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("notification command failed"))
        }
    }
}

pub fn open_handoff_service<L: WorkerLauncher>(
    paths: HandoffPaths,
    executables: ProviderExecutables,
    launcher: L,
) -> Result<HandoffService<L>> {
    let store = TaskStore::open(&paths.task_store)?;
    HandoffService::new(
        paths,
        executables,
        launcher,
        Arc::new(MacosCurrentRunControl::new(PathBuf::new())),
        Arc::new(SystemTaskIdGenerator),
        Arc::new(RegistrySuccessorRuntime),
        store,
    )
}

pub fn open_handoff_service_with_current_run_control<L: WorkerLauncher>(
    paths: HandoffPaths,
    executables: ProviderExecutables,
    launcher: L,
    current_run_control: Arc<dyn CurrentRunControl>,
) -> Result<HandoffService<L>> {
    let store = TaskStore::open(&paths.task_store)?;
    HandoffService::new(
        paths,
        executables,
        launcher,
        current_run_control,
        Arc::new(SystemTaskIdGenerator),
        Arc::new(RegistrySuccessorRuntime),
        store,
    )
}
