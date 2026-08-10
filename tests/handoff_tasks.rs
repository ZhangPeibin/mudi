use rusqlite::Connection;
use sessionmesh::config::Config;
use sessionmesh::domain::{
    ContextState, Provider, Session, SessionState, SourceFormat, SourceMetadata, Usage,
};
use sessionmesh::handoff::{
    CompletionNotice, CompletionNotifier, CurrentRunControl, CurrentRunObservation,
    DetachedWorkerLauncher, HandoffPaths, HandoffService, HandoffWorker, MacosCurrentRunControl,
    ProviderExecutables, WorkerLaunch, WorkerLauncher,
};
use sessionmesh::handoff_skill::ManagedHandoffSkills;
use sessionmesh::providers::state::ProcessIdentity;
use sessionmesh::tasks::{OperationTaskState, TaskStore};
use std::collections::VecDeque;
use std::fs;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(test_name: &str) -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sessionmesh-handoff-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Default)]
struct RecordingWorkerLauncher {
    launches: Arc<Mutex<Vec<WorkerLaunch>>>,
}

impl WorkerLauncher for RecordingWorkerLauncher {
    fn launch(&self, launch: &WorkerLaunch) -> std::io::Result<()> {
        self.launches.lock().unwrap().push(launch.clone());
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct FailingWorkerLauncher;

impl WorkerLauncher for FailingWorkerLauncher {
    fn launch(&self, _launch: &WorkerLaunch) -> std::io::Result<()> {
        Err(std::io::Error::other("worker unavailable"))
    }
}

struct FailingNotifier;

impl CompletionNotifier for FailingNotifier {
    fn notify(&self, _notice: &CompletionNotice) -> std::io::Result<()> {
        Err(std::io::Error::other("notifications disabled"))
    }
}

#[derive(Clone)]
struct FakeCurrentRunControl {
    observations: Arc<Mutex<VecDeque<CurrentRunObservation>>>,
    graceful_exit_requests: Arc<Mutex<Vec<ProcessIdentity>>>,
    now_ms: Arc<AtomicI64>,
    wait_advance_ms: i64,
    wait_sleep_ms: u64,
    graceful_exit_fails: bool,
    observe_calls: Arc<AtomicU64>,
    fail_on_observe_call: Option<u64>,
}

impl FakeCurrentRunControl {
    fn new(observations: impl IntoIterator<Item = CurrentRunObservation>, now_ms: i64) -> Self {
        Self {
            observations: Arc::new(Mutex::new(observations.into_iter().collect())),
            graceful_exit_requests: Arc::new(Mutex::new(Vec::new())),
            now_ms: Arc::new(AtomicI64::new(now_ms)),
            wait_advance_ms: 0,
            wait_sleep_ms: 0,
            graceful_exit_fails: false,
            observe_calls: Arc::new(AtomicU64::new(0)),
            fail_on_observe_call: None,
        }
    }
}

impl CurrentRunControl for FakeCurrentRunControl {
    fn observe(
        &self,
        _provider: Provider,
        _source_session_id: &str,
        _source_record_path: &Path,
    ) -> anyhow::Result<CurrentRunObservation> {
        let call = self.observe_calls.fetch_add(1, Ordering::Relaxed) + 1;
        if self.fail_on_observe_call == Some(call) {
            anyhow::bail!("Current Run observation is uncertain");
        }
        let mut observations = self.observations.lock().unwrap();
        if observations.len() > 1 {
            return Ok(observations.pop_front().unwrap());
        }
        Ok(observations
            .front()
            .copied()
            .unwrap_or(CurrentRunObservation::Exited))
    }

    fn request_graceful_exit(&self, process: ProcessIdentity) -> anyhow::Result<()> {
        self.graceful_exit_requests.lock().unwrap().push(process);
        if self.graceful_exit_fails {
            anyhow::bail!("graceful exit outcome is uncertain");
        }
        Ok(())
    }

    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::Relaxed)
    }

    fn wait_before_recheck(&self) {
        self.now_ms
            .fetch_add(self.wait_advance_ms, Ordering::Relaxed);
        if self.wait_sleep_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.wait_sleep_ms));
        }
    }
}

#[test]
fn macos_control_observes_claude_from_its_validated_pid_state_record() {
    let temp = TempDir::new("claude-current-run-control");
    let now_ms = chrono::Utc::now().timestamp_millis();
    let pid = std::process::id();
    fs::write(
        temp.path().join(format!("{pid}.json")),
        format!(
            "{{\"pid\":{pid},\"sessionId\":\"claude-native-session\",\"status\":\"busy\",\"startedAt\":{now_ms}}}"
        ),
    )
    .unwrap();
    let control = MacosCurrentRunControl::new(temp.path().to_path_buf());

    let observed = control
        .observe(
            Provider::ClaudeCode,
            "claude-native-session",
            Path::new("/unused/source.jsonl"),
        )
        .unwrap();

    assert_eq!(
        observed,
        CurrentRunObservation::Busy(ProcessIdentity {
            pid,
            started_at_ms: now_ms,
        })
    );
}

#[cfg(unix)]
#[test]
fn macos_control_sends_term_only_to_the_verified_provider_process() {
    use std::os::unix::process::ExitStatusExt;

    let temp = TempDir::new("provider-process-only");
    let mut provider_process = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = provider_process.id();
    let started_at_ms = chrono::Utc::now().timestamp_millis();
    fs::write(
        temp.path().join(format!("{pid}.json")),
        format!(
            "{{\"pid\":{pid},\"sessionId\":\"provider-child\",\"status\":\"idle\",\"startedAt\":{started_at_ms}}}"
        ),
    )
    .unwrap();
    let control = MacosCurrentRunControl::new(temp.path().to_path_buf());
    let observed = control
        .observe(
            Provider::ClaudeCode,
            "provider-child",
            Path::new("/unused/source.jsonl"),
        )
        .unwrap();
    let CurrentRunObservation::Idle(process) = observed else {
        panic!("expected an Idle Provider child, got {observed:?}");
    };

    control.request_graceful_exit(process).unwrap();
    let status = provider_process.wait().unwrap();

    assert_eq!(process.pid, pid);
    assert_ne!(process.pid, std::process::id());
    assert_eq!(status.signal(), Some(15));
}

#[cfg(unix)]
#[test]
fn macos_control_rejects_a_live_pid_with_a_different_process_start() {
    let temp = TempDir::new("reused-provider-pid");
    let mut provider_process = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = provider_process.id();
    let actual_started_at_ms = chrono::Utc::now().timestamp_millis();
    let control = MacosCurrentRunControl::new(temp.path().to_path_buf());

    let result = control.request_graceful_exit(ProcessIdentity {
        pid,
        started_at_ms: actual_started_at_ms - 60_000,
    });

    if result.is_ok() {
        let _ = provider_process.wait();
        panic!("a reused PID received the graceful-exit request");
    }
    assert!(provider_process.try_wait().unwrap().is_none());
    control
        .request_graceful_exit(ProcessIdentity {
            pid,
            started_at_ms: actual_started_at_ms,
        })
        .unwrap();
    let _ = provider_process.wait().unwrap();
}

#[test]
fn open_session_without_exact_process_identity_is_ineligible_without_a_task() {
    let temp = TempDir::new("claude-current-run-unverified");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let pid = std::process::id();
    fs::write(
        temp.path().join(format!("{pid}.json")),
        format!("{{\"pid\":{pid},\"sessionId\":\"claude-native-session\",\"status\":\"busy\"}}"),
    )
    .unwrap();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let launcher = RecordingWorkerLauncher::default();
    let launches = launcher.launches.clone();
    let mut handoff = HandoffService::open_with_current_run_control(
        paths,
        ProviderExecutables::new(claude, codex),
        launcher,
        Arc::new(MacosCurrentRunControl::new(temp.path().to_path_buf())),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;

    let error = handoff
        .authorize_session(&session, true, 10_000)
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Handoff is ineligible: exact Current Run process identity is unavailable"
    );
    assert!(handoff.tasks().unwrap().is_empty());
    assert!(launches.lock().unwrap().is_empty());
}

fn executable(path: &Path) {
    fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn provider_that_returns(path: &Path, candidate: &str) {
    let escaped = candidate.replace("'", "'\\''");
    fs::write(
        path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\ncat > \"$0.prompt\"\nprintf '%s' '{escaped}'\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn provider_that_returns_and_records_full_input(path: &Path, candidate: &str) {
    let escaped = candidate.replace("'", "'\\''");
    fs::write(
        path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\ncat > \"$0.prompt\"\nprintf '%s' '{escaped}'\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn codex_that_requires_non_git_override_then_returns(path: &Path, candidate: &str) {
    let escaped = candidate.replace("'", "'\\''");
    fs::write(
        path,
        format!(
            "#!/bin/sh\nallow_non_git=false\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"--skip-git-repo-check\" ]; then allow_non_git=true; fi\ndone\ncat > /dev/null\nif [ \"$allow_non_git\" != true ]; then exit 64; fi\nprintf '%s' '{escaped}'\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn provider_that_exits(path: &Path, code: i32) {
    fs::write(path, format!("#!/bin/sh\nexit {code}\n")).unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn provider_that_pauses_then_returns(path: &Path, candidate: &str) {
    let escaped = candidate.replace("'", "'\\''");
    fs::write(
        path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\ncat > \"$0.prompt\"\nsleep 1\nprintf '%s' '{escaped}'\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn provider_that_crashes_worker_once_then_returns(path: &Path, candidate: &str) {
    let escaped = candidate.replace("'", "'\\''");
    fs::write(
        path,
        format!(
            "#!/bin/sh\nif [ -f \"$0.retry\" ]; then\n  printf '%s\\n' \"$@\" > \"$0.args\"\n  cat > \"$0.prompt\"\n  printf '%s' '{escaped}'\n  exit 0\nfi\ncat > \"$0.prompt\"\nprintf '%s' '# Handoff\\n\\n## Objective\\n\\nPartial'\nsleep 0.1\nkill -TERM \"$PPID\"\nsleep 1\nexit 0\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn provider_that_waits_for_term(path: &Path) {
    fs::write(
        path,
        "#!/bin/sh\ntrap 'exit 0' TERM\ncat > \"$0.prompt\"\ni=0\nwhile [ \"$i\" -lt 200 ]; do sleep 0.05; i=$((i + 1)); done\nexit 1\n",
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn provider_that_outlives_a_crashed_worker(path: &Path) {
    fs::write(
        path,
        "#!/bin/sh\ncat > \"$0.prompt\"\nprintf '%s' '# Handoff\\n\\n## Objective\\n\\nPartial'\nsleep 0.1\nkill -TERM \"$PPID\"\nsleep 1\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn provider_that_returns_valid_result_on_term(path: &Path, candidate: &str) {
    fs::write(path.with_extension("result"), candidate).unwrap();
    fs::write(
        path,
        "#!/bin/sh\nresult=\"$0.result\"\ntrap 'exit 0' TERM\ncat > \"$0.prompt\"\ncat \"$result\"\n: > \"$0.candidate-complete\"\ni=0\nwhile [ \"$i\" -lt 200 ]; do sleep 0.05; i=$((i + 1)); done\nexit 1\n",
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(unix)]
fn provider_that_is_interrupted(path: &Path) {
    fs::write(
        path,
        "#!/bin/sh\ncat > /dev/null\nprintf '%s' '# Handoff\n\n## Objective\n\nPartial'\nkill -TERM $$\n",
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(unix)]
fn successor_that_records_arguments(path: &Path) {
    fs::write(
        path,
        "#!/bin/sh\npwd > \"$0.cwd\"\nprintf '%s\\n' \"$@\" > \"$0.args\"\n",
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn valid_handoff() -> &'static str {
    "# Handoff\n\n\
## Objective\n\nShip Assisted Handoff.\n\n\
## Current State\n\nTask execution is implemented.\n\n\
## Decisions\n\nUse an authoritative Task Store.\n\n\
## Completed\n\nAuthorization is complete.\n\n\
## Remaining\n\nFinish the UI.\n\n\
## Files Changed\n\nsrc/handoff.rs\n\n\
## Verification\n\ncargo test\n\n\
## Risks And Blockers\n\nNone.\n\n\
## Next Action\n\nReview the Ready Task.\n"
}

fn handoff_with_secret() -> String {
    valid_handoff().replace(
        "Use an authoritative Task Store.",
        "-----BEGIN PRIVATE KEY-----\nsecret material\n-----END PRIVATE KEY-----",
    )
}

fn handoff_with_transcript() -> String {
    valid_handoff().replace(
        "## Decisions",
        "## Complete Transcript\n\nUser: secret prompt\nAssistant: complete response\n\n## Decisions",
    )
}

fn handoff_with_tool_output() -> String {
    valid_handoff().replace(
        "## Decisions",
        "## Raw Tool Output\n\nthousands of unrelated command lines\n\n## Decisions",
    )
}

fn handoff_with_extra_section() -> String {
    valid_handoff().replace(
        "## Decisions",
        "## Notes\n\nUnexpected top-level section.\n\n## Decisions",
    )
}

fn exited_session(source_path: PathBuf, cwd: PathBuf) -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: "claude-native-session".into(),
        provenance: sessionmesh::domain::SessionProvenance::Observed,
        source: SourceMetadata {
            path: source_path,
            format: SourceFormat::JsonLines,
        },
        project: None,
        worktree: None,
        cwd: Some(cwd.to_string_lossy().into_owned()),
        latest_model: None,
        state: SessionState::Exited,
        context: ContextState::default(),
        usage: Usage::default(),
        per_model: vec![],
        current_run_usage: None,
        compactions: vec![],
        last_activity_ms: 1_000,
        cost: None,
    }
}

#[test]
fn explicit_authorization_persists_a_queued_handoff_task_for_an_exited_session() {
    let temp = TempDir::new("authorize-exited");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);

    let launcher = RecordingWorkerLauncher::default();
    let launches = launcher.launches.clone();
    let mut handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        launcher,
    )
    .unwrap();

    let task = handoff
        .authorize_exited(&exited_session(source_path.clone(), cwd.clone()), 10_000)
        .unwrap();
    let persisted = handoff.task(&task.id).unwrap().unwrap();

    assert_eq!(persisted, task);
    assert_eq!(task.state, OperationTaskState::Queued);
    assert_eq!(task.source_provider, Provider::ClaudeCode);
    assert_eq!(task.source_session_id, "claude-native-session");
    assert_eq!(task.source_record_path, source_path);
    assert_eq!(task.source_working_directory, cwd);
    assert_eq!(task.created_at_ms, 10_000);
    assert_eq!(task.state_changed_at_ms, 10_000);
    assert_eq!(
        launches.lock().unwrap().as_slice(),
        &[WorkerLaunch { task_id: task.id }]
    );
}

#[test]
fn confirmed_cancellation_before_worker_start_cancels_without_another_launch() {
    let temp = TempDir::new("cancel-queued");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let launcher = RecordingWorkerLauncher::default();
    let launches = launcher.launches.clone();
    let mut handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        launcher,
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let cancelled = handoff.cancel_task(&task.id, 11_000).unwrap();

    assert_eq!(cancelled.state, OperationTaskState::Cancelled);
    assert_eq!(cancelled.failure_code.as_deref(), Some("cancelled_by_user"));
    assert_eq!(cancelled.state_changed_at_ms, 11_000);
    assert_eq!(handoff.task(&task.id).unwrap(), Some(cancelled));
    assert_eq!(
        launches.lock().unwrap().as_slice(),
        &[WorkerLaunch { task_id: task.id }]
    );
}

#[test]
fn worker_observes_queued_cancellation_before_sending_a_prompt() {
    let temp = TempDir::new("worker-observes-cancelled-queue");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    handoff.cancel_task(&task.id, 11_000).unwrap();

    let worker_result = HandoffWorker::new(paths, executables).run(&task.id, 12_000);

    assert!(worker_result.is_err());
    assert_eq!(
        handoff.task(&task.id).unwrap().unwrap().state,
        OperationTaskState::Cancelled
    );
    assert!(!claude.with_extension("prompt").exists());
}

#[test]
fn reconciliation_fails_queued_task_without_worker_startup_after_thirty_seconds() {
    let temp = TempDir::new("queued-worker-deadline");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let mut handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    handoff.reconcile_tasks(40_000).unwrap();
    assert_eq!(
        handoff.task(&task.id).unwrap().unwrap().state,
        OperationTaskState::Queued
    );

    handoff.reconcile_tasks(40_001).unwrap();
    let failed = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(
        failed.failure_code.as_deref(),
        Some("worker_start_deadline_exceeded")
    );
    assert_eq!(failed.state_changed_at_ms, 40_001);
}

#[test]
fn existing_task_store_migrates_current_run_authority_fields_without_changing_tasks() {
    let temp = TempDir::new("current-run-schema-migration");
    let task_store = temp.path().join("tasks.db");
    let connection = Connection::open(&task_store).unwrap();
    connection
        .execute_batch(
            r#"
            CREATE TABLE operation_tasks (
                task_id TEXT PRIMARY KEY,
                state TEXT NOT NULL,
                source_provider TEXT NOT NULL,
                source_session_id TEXT NOT NULL,
                source_record_path TEXT NOT NULL,
                source_working_directory TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                state_changed_at_ms INTEGER NOT NULL,
                worker_pid INTEGER,
                artifact_path TEXT,
                plan_provider TEXT,
                plan_working_directory TEXT,
                plan_artifact_path TEXT,
                failure_code TEXT,
                acknowledged_at_ms INTEGER
            );
            INSERT INTO operation_tasks (
                task_id, state, source_provider, source_session_id,
                source_record_path, source_working_directory,
                created_at_ms, state_changed_at_ms, acknowledged_at_ms
            ) VALUES (
                'legacy-ready', 'ready', 'claude', 'legacy-session',
                '/private/source.jsonl', '/private/project',
                1000, 2000, 3000
            );
            "#,
        )
        .unwrap();
    drop(connection);
    let handoff = HandoffService::open(
        HandoffPaths {
            task_store,
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(temp.path().join("claude"), temp.path().join("codex")),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();

    let task = handoff.task("legacy-ready").unwrap().unwrap();

    assert_eq!(task.state, OperationTaskState::Ready);
    assert_eq!(task.acknowledged_at_ms, Some(3_000));
    assert!(!task.end_current_run_authorized);
    assert_eq!(task.source_process_identity, None);
    assert_eq!(task.waiting_reason, None);
    assert_eq!(task.waiting_deadline_ms, None);
    assert_eq!(task.control_used_at_ms, None);
    assert_eq!(task.deletion_requested_at_ms, None);
    assert_eq!(task.deletion_failure_code, None);
}

#[test]
fn concurrent_task_store_open_serializes_the_index_migration() {
    let temp = TempDir::new("concurrent-task-store-open");
    let task_store = temp.path().join("tasks.db");
    drop(TaskStore::open(&task_store).unwrap());
    let barrier = Arc::new(Barrier::new(8));
    let handles = (0..8)
        .map(|_| {
            let barrier = barrier.clone();
            let task_store = task_store.clone();
            std::thread::spawn(move || {
                barrier.wait();
                TaskStore::open(&task_store).map(drop)
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap().unwrap();
    }
}

#[test]
fn explicitly_authorized_busy_current_run_persists_waiting_authority_and_deadline() {
    let temp = TempDir::new("authorize-busy-open");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let current_run = FakeCurrentRunControl::new([CurrentRunObservation::Busy(process)], 10_000);
    let exit_requests = current_run.graceful_exit_requests.clone();
    let launcher = RecordingWorkerLauncher::default();
    let launches = launcher.launches.clone();
    let mut handoff = HandoffService::open_with_current_run_control(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        launcher,
        Arc::new(current_run),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;

    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    assert_eq!(task.state, OperationTaskState::Waiting);
    assert!(task.end_current_run_authorized);
    assert_eq!(task.source_process_identity, Some(process));
    assert_eq!(
        task.waiting_reason.as_deref(),
        Some("source_current_run_busy")
    );
    assert_eq!(task.waiting_deadline_ms, Some(7_210_000));
    assert_eq!(task.control_used_at_ms, None);
    assert_eq!(launches.lock().unwrap().len(), 1);
    assert!(exit_requests.lock().unwrap().is_empty());
}

#[test]
fn declining_current_run_authority_creates_no_operation_task() {
    let temp = TempDir::new("decline-open-authority");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let current_run = FakeCurrentRunControl::new(
        [CurrentRunObservation::Busy(ProcessIdentity {
            pid: 42,
            started_at_ms: 9_000,
        })],
        10_000,
    );
    let observe_calls = current_run.observe_calls.clone();
    let launcher = RecordingWorkerLauncher::default();
    let launches = launcher.launches.clone();
    let mut handoff = HandoffService::open_with_current_run_control(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        launcher,
        Arc::new(current_run),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;

    let error = handoff
        .authorize_session(&session, false, 10_000)
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Handoff is ineligible: ending the Current Run was not authorized"
    );
    assert!(handoff.tasks().unwrap().is_empty());
    assert!(launches.lock().unwrap().is_empty());
    assert_eq!(observe_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn busy_current_run_is_never_terminated_before_it_exits_naturally() {
    let temp = TempDir::new("busy-natural-exit");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let current_run = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Exited,
        ],
        10_000,
    );
    let exit_requests = current_run.graceful_exit_requests.clone();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    let ready = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert!(exit_requests.lock().unwrap().is_empty());
    assert!(claude.with_extension("args").is_file());
}

#[test]
fn waiting_task_records_the_time_when_generation_actually_finishes() {
    let temp = TempDir::new("waiting-transition-time");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let mut current_run = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Exited,
        ],
        10_000,
    );
    current_run.wait_advance_ms = 5_000;
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    let ready = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 10_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(ready.state_changed_at_ms, 15_000);
}

#[test]
fn idle_current_run_is_revalidated_before_graceful_exit_is_requested() {
    let temp = TempDir::new("idle-revalidated");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let current_run = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Exited,
        ],
        10_000,
    );
    let exit_requests = current_run.graceful_exit_requests.clone();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    let ready = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(exit_requests.lock().unwrap().as_slice(), &[process]);
    assert!(ready.control_used_at_ms.is_some());
}

#[test]
fn current_run_that_exits_during_revalidation_resumes_without_using_control() {
    let temp = TempDir::new("natural-exit-during-revalidation");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let current_run = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Exited,
        ],
        10_000,
    );
    let exit_requests = current_run.graceful_exit_requests.clone();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    let ready = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(ready.control_used_at_ms, None);
    assert!(exit_requests.lock().unwrap().is_empty());
}

#[test]
fn current_run_that_becomes_busy_during_revalidation_returns_to_waiting() {
    let temp = TempDir::new("busy-during-revalidation");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let current_run = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Exited,
        ],
        10_000,
    );
    let exit_requests = current_run.graceful_exit_requests.clone();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    let ready = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(exit_requests.lock().unwrap().as_slice(), &[process]);
}

#[test]
fn changed_process_identity_interrupts_without_control_or_resume() {
    let temp = TempDir::new("identity-changed");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let original = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let reused = ProcessIdentity {
        pid: 42,
        started_at_ms: 19_000,
    };
    let current_run = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Busy(original),
            CurrentRunObservation::Idle(original),
            CurrentRunObservation::Idle(reused),
        ],
        20_000,
    );
    let exit_requests = current_run.graceful_exit_requests.clone();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    let interrupted = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(interrupted.state, OperationTaskState::Interrupted);
    assert_eq!(
        interrupted.failure_code.as_deref(),
        Some("source_current_run_identity_changed")
    );
    assert!(exit_requests.lock().unwrap().is_empty());
    assert!(!claude.with_extension("args").exists());
}

#[test]
fn uncertain_graceful_exit_outcome_interrupts_without_resume() {
    let temp = TempDir::new("graceful-exit-uncertain");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let mut current_run = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Idle(process),
        ],
        10_000,
    );
    current_run.graceful_exit_fails = true;
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Idle;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    let interrupted = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(interrupted.state, OperationTaskState::Interrupted);
    assert_eq!(
        interrupted.failure_code.as_deref(),
        Some("source_current_run_control_uncertain")
    );
    assert!(!claude.with_extension("args").exists());
}

#[test]
fn uncertain_current_run_observation_interrupts_without_resume() {
    let temp = TempDir::new("current-run-observation-uncertain");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let mut current_run =
        FakeCurrentRunControl::new([CurrentRunObservation::Busy(process)], 10_000);
    current_run.fail_on_observe_call = Some(2);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    let interrupted = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(interrupted.state, OperationTaskState::Interrupted);
    assert_eq!(
        interrupted.failure_code.as_deref(),
        Some("source_current_run_observation_uncertain")
    );
    assert!(!claude.with_extension("args").exists());
}

#[test]
fn unused_current_run_authority_expires_after_two_hours_and_never_acts_later() {
    let temp = TempDir::new("waiting-deadline");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let mut current_run = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Exited,
        ],
        10_000,
    );
    current_run.wait_advance_ms = 7_200_001;
    let exit_requests = current_run.graceful_exit_requests.clone();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();
    let worker = HandoffWorker::new_with_current_run_control(paths, executables, control);

    let cancelled = worker.run(&task.id, 20_000).unwrap();

    assert_eq!(cancelled.state, OperationTaskState::Cancelled);
    assert_eq!(cancelled.control_used_at_ms, None);
    assert!(exit_requests.lock().unwrap().is_empty());
    assert!(!claude.with_extension("args").exists());
    assert!(worker.run(&task.id, 7_220_000).is_err());
    assert!(!claude.with_extension("args").exists());
}

#[test]
fn delayed_graceful_exit_is_requested_once_and_resume_waits_for_exit() {
    let temp = TempDir::new("graceful-exit-delay");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let current_run = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Exited,
        ],
        10_000,
    );
    let exit_requests = current_run.graceful_exit_requests.clone();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Idle;
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();

    let ready = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(exit_requests.lock().unwrap().as_slice(), &[process]);
    assert!(ready.control_used_at_ms.is_some());
}

#[test]
fn restart_preserves_waiting_authority_and_rejects_a_second_task() {
    let temp = TempDir::new("waiting-restart");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let task = {
        let mut handoff = HandoffService::open_with_current_run_control(
            paths.clone(),
            executables.clone(),
            RecordingWorkerLauncher::default(),
            Arc::new(FakeCurrentRunControl::new(
                [CurrentRunObservation::Busy(process)],
                10_000,
            )),
        )
        .unwrap();
        handoff.authorize_session(&session, true, 10_000).unwrap()
    };
    let launches_after_restart = RecordingWorkerLauncher::default();
    let launches = launches_after_restart.launches.clone();
    let restarted_control = FakeCurrentRunControl::new(
        [
            CurrentRunObservation::Busy(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Idle(process),
            CurrentRunObservation::Exited,
        ],
        20_000,
    );
    let exit_requests = restarted_control.graceful_exit_requests.clone();
    let control: Arc<dyn CurrentRunControl> = Arc::new(restarted_control);
    let mut restarted = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        launches_after_restart,
        control.clone(),
    )
    .unwrap();

    let persisted = restarted.task(&task.id).unwrap().unwrap();
    let error = restarted
        .authorize_session(&session, true, 20_000)
        .unwrap_err();

    assert_eq!(persisted.source_process_identity, Some(process));
    assert_eq!(persisted.waiting_deadline_ms, Some(7_210_000));
    assert_eq!(
        error.to_string(),
        "Handoff is ineligible: source Session already has a non-terminal Operation Task"
    );
    assert!(launches.lock().unwrap().is_empty());
    let ready = HandoffWorker::new_with_current_run_control(paths, executables, control)
        .run(&task.id, 20_000)
        .unwrap();
    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(exit_requests.lock().unwrap().as_slice(), &[process]);
}

#[test]
fn a_source_session_cannot_have_two_non_terminal_handoff_tasks() {
    let temp = TempDir::new("one-non-terminal");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let launcher = RecordingWorkerLauncher::default();
    let launches = launcher.launches.clone();
    let mut handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        launcher,
    )
    .unwrap();
    let session = exited_session(source_path, cwd);
    handoff.authorize_exited(&session, 10_000).unwrap();

    let error = handoff.authorize_exited(&session, 11_000).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Handoff is ineligible: source Session already has a non-terminal Operation Task"
    );
    assert_eq!(launches.lock().unwrap().len(), 1);
}

#[test]
fn worker_publishes_a_valid_artifact_before_the_task_becomes_ready() {
    let temp = TempDir::new("ready-artifact");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::write(cwd.join("unchanged.txt"), "source contents\n").unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd.clone()), 10_000)
        .unwrap();

    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    let artifact_path = ready.artifact_path.as_ref().unwrap();
    assert_eq!(fs::read_to_string(artifact_path).unwrap(), valid_handoff());
    let plan = ready.successor_launch_plan.as_ref().unwrap();
    assert_eq!(plan.successor_provider, Provider::ClaudeCode);
    assert_eq!(plan.working_directory, cwd.clone());
    assert_eq!(plan.artifact_path, *artifact_path);
    assert_eq!(
        fs::read_to_string(cwd.join("unchanged.txt")).unwrap(),
        "source contents\n"
    );
    assert_eq!(
        fs::read_to_string(claude.with_extension("prompt")).unwrap(),
        "/sessionmesh-handoff\n"
    );
    assert_eq!(
        fs::read_to_string(claude.with_extension("args")).unwrap(),
        "--print\n--resume\nclaude-native-session\n--bare\n--permission-mode\nplan\n--tools\n\n--strict-mcp-config\n"
    );
}

#[test]
fn restarting_sessionmesh_preserves_the_ready_task_and_handoff_result() {
    let temp = TempDir::new("ready-restart");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd.clone()), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task.id, 20_000)
        .unwrap();
    let expected_artifact = ready.artifact_path.clone().unwrap();
    let expected_plan = ready.successor_launch_plan.clone();
    drop(handoff);

    let restarted =
        HandoffService::open(paths, executables, RecordingWorkerLauncher::default()).unwrap();
    let retained = restarted.task(&task.id).unwrap().unwrap();

    assert_eq!(retained.state, OperationTaskState::Ready);
    assert_eq!(retained.artifact_path.as_ref(), Some(&expected_artifact));
    assert_eq!(retained.successor_launch_plan, expected_plan);
}

#[test]
fn codex_worker_uses_the_same_artifact_contract_inside_a_read_only_sandbox() {
    let temp = TempDir::new("codex-ready-artifact");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::write(cwd.join("unchanged.txt"), "source contents\n").unwrap();
    executable(&claude);
    provider_that_returns(&codex, valid_handoff());
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex.clone());
    let mut session = exited_session(source_path, cwd.clone());
    session.provider = Provider::CodexCli;
    session.id = "codex-native-session".into();
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff.authorize_exited(&session, 10_000).unwrap();

    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(
        fs::read_to_string(codex.with_extension("args")).unwrap(),
        "exec\n--sandbox\nread-only\n--skip-git-repo-check\nresume\ncodex-native-session\n-\n"
    );
    assert_eq!(
        fs::read_to_string(codex.with_extension("prompt")).unwrap(),
        "$sessionmesh-handoff\n"
    );
    assert_eq!(
        fs::read_to_string(cwd.join("unchanged.txt")).unwrap(),
        "source contents\n"
    );
}

#[test]
fn codex_handoff_generates_from_a_non_git_project() {
    let temp = TempDir::new("codex-non-git-project");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("standalone-project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    assert!(!cwd.join(".git").exists());
    executable(&claude);
    codex_that_requires_non_git_override_then_returns(&codex, valid_handoff());
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-non-git-session".into();
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff.authorize_exited(&session, 10_000).unwrap();

    let completed = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(completed.state, OperationTaskState::Ready);
    assert!(completed.artifact_path.is_some());
}

#[cfg(unix)]
#[test]
fn first_codex_handoff_installs_managed_skills_and_invokes_the_named_skill() {
    let temp = TempDir::new("codex-managed-handoff-skill");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let codex_skills = temp.path().join("agents/skills");
    let claude_skills = temp.path().join("claude-home/skills");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    provider_that_returns_and_records_full_input(&codex, valid_handoff());
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex.clone());
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-native-session".into();
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap()
    .with_managed_handoff_skills(ManagedHandoffSkills::new(
        codex_skills.clone(),
        claude_skills.clone(),
    ));

    let task = handoff.authorize_exited(&session, 10_000).unwrap();
    let ready = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(
        fs::read_to_string(codex.with_extension("prompt")).unwrap(),
        "$sessionmesh-handoff\n"
    );
    let codex_skill = codex_skills.join("sessionmesh-handoff/SKILL.md");
    let claude_skill = claude_skills.join("sessionmesh-handoff/SKILL.md");
    let installed = fs::read_to_string(&codex_skill).unwrap();
    assert_eq!(fs::read_to_string(&claude_skill).unwrap(), installed);
    assert!(installed.contains("sessionmesh-managed-version: 1"));
    assert!(installed.contains("## Risks And Blockers"));
    assert_eq!(
        fs::metadata(&codex_skill).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(codex_skill.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn provider_standard_user_skill_directories_receive_the_managed_copies() {
    let temp = TempDir::new("provider-standard-skill-directories");
    let home = temp.path().join("home");
    let claude_dir = home.join(".claude");
    let skills = ManagedHandoffSkills::for_user_home(&home, &claude_dir);
    assert!(!home.exists());

    skills.install_for_first_use().unwrap();

    let codex_skill = home.join(".agents/skills/sessionmesh-handoff/SKILL.md");
    let claude_skill = home.join(".claude/skills/sessionmesh-handoff/SKILL.md");
    assert!(codex_skill.is_file());
    assert!(claude_skill.is_file());
    assert_eq!(
        fs::read_to_string(codex_skill).unwrap(),
        fs::read_to_string(claude_skill).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn installing_the_current_managed_skill_twice_is_idempotent_and_owner_only() {
    let temp = TempDir::new("idempotent-managed-skill-install");
    let codex_skills = temp.path().join("agents/skills");
    let claude_skills = temp.path().join("claude/skills");
    let skills = ManagedHandoffSkills::new(codex_skills.clone(), claude_skills.clone());
    skills.install_for_first_use().unwrap();
    let codex_skill = codex_skills.join("sessionmesh-handoff/SKILL.md");
    let before = fs::metadata(&codex_skill).unwrap();

    skills.install_for_first_use().unwrap();

    let after = fs::metadata(&codex_skill).unwrap();
    assert_eq!(after.ino(), before.ino());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    for skill in [
        codex_skill,
        claude_skills.join("sessionmesh-handoff/SKILL.md"),
    ] {
        assert_eq!(
            fs::metadata(&skill).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(skill.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::read_dir(skill.parent().unwrap()).unwrap().count(),
            1,
            "idempotent install left a temporary file"
        );
    }
}

#[test]
fn claude_handoff_invokes_the_managed_skill_without_other_customizations_or_tools() {
    let temp = TempDir::new("claude-isolated-handoff-skill");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns_and_records_full_input(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap()
    .with_managed_handoff_skills(ManagedHandoffSkills::new(
        temp.path().join("agents/skills"),
        temp.path().join("claude-home/skills"),
    ));
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let ready = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(
        fs::read_to_string(claude.with_extension("prompt")).unwrap(),
        "/sessionmesh-handoff\n"
    );
    let arguments = fs::read_to_string(claude.with_extension("args")).unwrap();
    assert_eq!(
        arguments,
        "--print\n--resume\nclaude-native-session\n--bare\n--permission-mode\nplan\n--tools\n\n--strict-mcp-config\n"
    );
    assert!(!arguments.contains("--safe-mode"));
}

#[cfg(unix)]
#[test]
fn first_use_updates_an_older_sessionmesh_managed_skill_before_task_creation() {
    let temp = TempDir::new("update-managed-handoff-skill");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let codex_skills = temp.path().join("agents/skills");
    let stale_skill = codex_skills.join("sessionmesh-handoff/SKILL.md");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(stale_skill.parent().unwrap()).unwrap();
    fs::write(
        &stale_skill,
        "---\nname: sessionmesh-handoff\ndescription: Old managed copy.\n---\n\
<!-- sessionmesh-managed-skill: sessionmesh-handoff -->\n\
<!-- sessionmesh-managed-version: 0 -->\n\nOld contract.\n",
    )
    .unwrap();
    fs::set_permissions(&stale_skill, fs::Permissions::from_mode(0o644)).unwrap();
    executable(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-stale-skill-session".into();
    let mut handoff = HandoffService::open(paths, executables, RecordingWorkerLauncher::default())
        .unwrap()
        .with_managed_handoff_skills(ManagedHandoffSkills::new(
            codex_skills,
            temp.path().join("claude-home/skills"),
        ));

    let task = handoff.authorize_exited(&session, 10_000).unwrap();

    let installed = fs::read_to_string(&stale_skill).unwrap();
    assert_eq!(task.state, OperationTaskState::Queued);
    assert!(installed.contains("sessionmesh-managed-version: 1"));
    assert!(!installed.contains("Old contract."));
    assert_eq!(
        fs::metadata(stale_skill).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn preflight_reports_a_newer_managed_skill_as_out_of_date_without_downgrading_it() {
    let temp = TempDir::new("newer-managed-handoff-skill");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let codex_skills = temp.path().join("agents/skills");
    let newer_skill = codex_skills.join("sessionmesh-handoff/SKILL.md");
    let newer_content = "---\nname: sessionmesh-handoff\ndescription: Future managed copy.\n---\n\
<!-- sessionmesh-managed-skill: sessionmesh-handoff -->\n\
<!-- sessionmesh-managed-version: 2 -->\n\nFuture contract.\n";
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(newer_skill.parent().unwrap()).unwrap();
    fs::write(&newer_skill, newer_content).unwrap();
    executable(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-newer-skill-session".into();
    let mut handoff = HandoffService::open(
        paths,
        ProviderExecutables::new(claude, codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap()
    .with_managed_handoff_skills(ManagedHandoffSkills::new(
        codex_skills,
        temp.path().join("claude-home/skills"),
    ));

    let error = handoff.authorize_exited(&session, 10_000).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Handoff Skill preflight failed: Codex CLI managed Skill is out of date (installed 2, expected 1)"
    );
    assert!(handoff.tasks().unwrap().is_empty());
    assert_eq!(fs::read_to_string(newer_skill).unwrap(), newer_content);
}

#[test]
fn preflight_refuses_to_overwrite_an_unrelated_same_name_skill() {
    let temp = TempDir::new("conflicting-user-handoff-skill");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let codex_skills = temp.path().join("agents/skills");
    let user_skill = codex_skills.join("sessionmesh-handoff/SKILL.md");
    let user_content =
        "---\nname: sessionmesh-handoff\ndescription: My own Skill.\n---\n\nKeep this.\n";
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(user_skill.parent().unwrap()).unwrap();
    fs::write(&user_skill, user_content).unwrap();
    #[cfg(unix)]
    fs::set_permissions(
        user_skill.parent().unwrap(),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    executable(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-conflicting-skill-session".into();
    let mut handoff = HandoffService::open(
        paths,
        ProviderExecutables::new(claude, codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap()
    .with_managed_handoff_skills(ManagedHandoffSkills::new(
        codex_skills,
        temp.path().join("claude-home/skills"),
    ));

    let error = handoff.authorize_exited(&session, 10_000).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Handoff Skill preflight failed: Codex CLI managed Skill conflicts with an unrelated same-name Skill"
    );
    assert!(handoff.tasks().unwrap().is_empty());
    assert_eq!(fs::read_to_string(user_skill).unwrap(), user_content);
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(temp.path().join("agents/skills/sessionmesh-handoff"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

#[test]
fn preflight_reports_an_unreadable_managed_skill_before_task_creation() {
    let temp = TempDir::new("unreadable-managed-handoff-skill");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let codex_skills = temp.path().join("agents/skills");
    let unreadable_skill = codex_skills.join("sessionmesh-handoff/SKILL.md");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(unreadable_skill.parent().unwrap()).unwrap();
    fs::write(&unreadable_skill, [0xff, 0xfe, 0xfd]).unwrap();
    executable(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-unreadable-skill-session".into();
    let mut handoff = HandoffService::open(
        paths,
        ProviderExecutables::new(claude, codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap()
    .with_managed_handoff_skills(ManagedHandoffSkills::new(
        codex_skills,
        temp.path().join("claude-home/skills"),
    ));

    let error = handoff.authorize_exited(&session, 10_000).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Handoff Skill preflight failed: Codex CLI managed Skill is unreadable"
    );
    assert!(handoff.tasks().unwrap().is_empty());
    assert_eq!(fs::read(unreadable_skill).unwrap(), [0xff, 0xfe, 0xfd]);
}

#[cfg(unix)]
#[test]
fn preflight_rejects_a_same_name_skill_file_symlink_before_task_creation() {
    let temp = TempDir::new("symlinked-managed-handoff-skill");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let codex_skills = temp.path().join("agents/skills");
    let skill_path = codex_skills.join("sessionmesh-handoff/SKILL.md");
    let outside = temp.path().join("outside-skill.md");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    fs::write(
        &outside,
        include_str!("../skills/sessionmesh-handoff/SKILL.md"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, &skill_path).unwrap();
    executable(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-symlinked-skill-session".into();
    let mut handoff = HandoffService::open(
        paths,
        ProviderExecutables::new(claude, codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap()
    .with_managed_handoff_skills(ManagedHandoffSkills::new(
        codex_skills,
        temp.path().join("claude-home/skills"),
    ));

    let error = handoff.authorize_exited(&session, 10_000).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Handoff Skill preflight failed: Codex CLI managed Skill conflicts with an unrelated same-name Skill"
    );
    assert!(handoff.tasks().unwrap().is_empty());
    assert!(fs::symlink_metadata(skill_path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_to_string(outside).unwrap(),
        include_str!("../skills/sessionmesh-handoff/SKILL.md")
    );
}

#[test]
fn worker_records_a_bounded_failure_if_the_managed_skill_disappears_after_preflight() {
    let temp = TempDir::new("managed-handoff-skill-disappears");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let codex_skills = temp.path().join("agents/skills");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    provider_that_returns_and_records_full_input(&codex, valid_handoff());
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex.clone());
    let skills =
        ManagedHandoffSkills::new(codex_skills.clone(), temp.path().join("claude-home/skills"));
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-missing-worker-skill".into();
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap()
    .with_managed_handoff_skills(skills.clone());
    let task = handoff.authorize_exited(&session, 10_000).unwrap();
    fs::remove_file(codex_skills.join("sessionmesh-handoff/SKILL.md")).unwrap();

    let failed = HandoffWorker::new(paths, executables)
        .with_managed_handoff_skills(skills)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(
        failed.failure_code.as_deref(),
        Some("handoff_skill_missing")
    );
    assert!(failed.artifact_path.is_none());
    assert!(!codex.with_extension("prompt").exists());
}

#[test]
fn every_source_and_successor_provider_combination_builds_a_structured_launch_plan() {
    for (source_provider, successor_provider) in [
        (Provider::ClaudeCode, Provider::ClaudeCode),
        (Provider::ClaudeCode, Provider::CodexCli),
        (Provider::CodexCli, Provider::ClaudeCode),
        (Provider::CodexCli, Provider::CodexCli),
    ] {
        let temp = TempDir::new(&format!(
            "launch-plan-{}-{}",
            source_provider.as_str(),
            successor_provider.as_str()
        ));
        let source_path = temp.path().join("source.jsonl");
        let cwd = temp.path().join("project");
        let claude = temp.path().join("Claude CLI 'quoted' $dollar");
        let codex = temp.path().join("Codex CLI ; unicode-\u{7ec8}\u{7aef}");
        fs::write(&source_path, "{}\n").unwrap();
        fs::create_dir(&cwd).unwrap();
        match source_provider {
            Provider::ClaudeCode => {
                provider_that_returns(&claude, valid_handoff());
                executable(&codex);
            }
            Provider::CodexCli => {
                executable(&claude);
                provider_that_returns(&codex, valid_handoff());
            }
        }
        let paths = HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp
                .path()
                .join("Artifacts 'quoted' $dollar \u{4ea4}\u{63a5}"),
        };
        let executables = ProviderExecutables::new(claude.clone(), codex.clone());
        let mut session = exited_session(source_path, cwd.clone());
        session.provider = source_provider;
        session.id = format!("{}-source", source_provider.as_str());
        let mut handoff = HandoffService::open(
            paths.clone(),
            executables.clone(),
            RecordingWorkerLauncher::default(),
        )
        .unwrap();
        let task = handoff
            .authorize_exited_to(&session, successor_provider, 10_000)
            .unwrap();

        let ready = HandoffWorker::new(paths.clone(), executables)
            .run(&task.id, 20_000)
            .unwrap();

        let artifact_path = paths.artifact_root.join(&task.id).join("handoff.md");
        let plan = ready.successor_launch_plan.unwrap();
        let successor_executable = match successor_provider {
            Provider::ClaudeCode => claude,
            Provider::CodexCli => codex,
        };
        let provider_name = match successor_provider {
            Provider::ClaudeCode => "Claude Code",
            Provider::CodexCli => "Codex CLI",
        };
        assert_eq!(ready.state, OperationTaskState::Ready);
        assert_eq!(fs::read_to_string(&artifact_path).unwrap(), valid_handoff());
        assert_eq!(plan.successor_provider, successor_provider);
        assert_eq!(plan.working_directory, cwd);
        assert_eq!(plan.executable, successor_executable);
        assert_eq!(&plan.artifact_path, &artifact_path);
        assert_eq!(
            plan.arguments,
            vec![
                "--add-dir".to_string(),
                artifact_path
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                format!(
                    "Read the Handoff Artifact at {} and continue the work in this new {provider_name} Session.",
                    plan.artifact_path.to_string_lossy()
                ),
            ]
        );
    }
}

#[cfg(unix)]
#[test]
fn copied_successor_command_preserves_adversarial_paths_and_arguments_exactly() {
    let temp = TempDir::new("copy-successor-command");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp
        .path()
        .join("Project 'quoted' $value; \u{9879}\u{76ee}");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("Codex 'quoted' $value; \u{7ec8}\u{7aef}");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    successor_that_records_arguments(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp
            .path()
            .join("Artifacts 'quoted' $value; \u{4ea4}\u{63a5}"),
    };
    let executables = ProviderExecutables::new(claude, codex.clone());
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited_to(
            &exited_session(source_path, cwd.clone()),
            Provider::CodexCli,
            10_000,
        )
        .unwrap();
    let ready = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    let command = sessionmesh::successor::render_successor_command(&ready).unwrap();
    let status = Command::new("/bin/sh")
        .args(["-c", &command])
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(codex.with_extension("cwd")).unwrap(),
        format!("{}\n", cwd.display())
    );
    let plan = ready.successor_launch_plan.unwrap();
    assert_eq!(
        fs::read_to_string(codex.with_extension("args")).unwrap(),
        format!("{}\n", plan.arguments.join("\n"))
    );
    assert!(!cwd.join("injected").exists());
}

#[test]
fn malformed_missing_and_tampered_successor_plans_are_rejected() {
    let temp = TempDir::new("reject-invalid-successor-plans");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited_to(
            &exited_session(source_path, cwd),
            Provider::CodexCli,
            10_000,
        )
        .unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task.id, 20_000)
        .unwrap();

    let mut missing = ready.clone();
    missing.successor_launch_plan = None;
    assert_eq!(
        sessionmesh::successor::render_successor_command(&missing)
            .unwrap_err()
            .to_string(),
        "Ready Task has no Successor Launch Plan"
    );

    let mut mismatched = ready.clone();
    mismatched
        .successor_launch_plan
        .as_mut()
        .unwrap()
        .artifact_path = temp.path().join("different.md");
    assert_eq!(
        sessionmesh::successor::render_successor_command(&mismatched)
            .unwrap_err()
            .to_string(),
        "Successor Launch Plan does not match its Operation Task"
    );

    let mut tampered = ready;
    tampered
        .successor_launch_plan
        .as_mut()
        .unwrap()
        .arguments
        .push("; touch injected".into());
    assert_eq!(
        sessionmesh::successor::render_successor_command(&tampered)
            .unwrap_err()
            .to_string(),
        "Successor arguments do not match the Handoff Artifact"
    );

    let mut substituted_executable = tampered;
    let plan = substituted_executable
        .successor_launch_plan
        .as_mut()
        .unwrap();
    plan.arguments.pop();
    plan.executable = PathBuf::from("/bin/sh");
    assert_eq!(
        sessionmesh::successor::render_successor_command(&substituted_executable)
            .unwrap_err()
            .to_string(),
        "Successor Launch Plan does not match its Operation Task"
    );
    assert!(!temp.path().join("injected").exists());

    drop(handoff);
    let connection = Connection::open(&paths.task_store).unwrap();
    connection
        .execute(
            "UPDATE operation_tasks SET successor_provider = 'tampered' WHERE task_id = ?1",
            [&task.id],
        )
        .unwrap();
    drop(connection);
    let reopened =
        HandoffService::open(paths, executables, RecordingWorkerLauncher::default()).unwrap();
    assert_eq!(
        reopened.task(&task.id).unwrap_err().to_string(),
        "unknown selected Successor Provider \"tampered\""
    );
}

#[cfg(unix)]
#[test]
fn terminal_app_launches_the_foreground_helper_and_failure_preserves_the_ready_result() {
    use sessionmesh::successor::{MacosTerminalLauncher, TerminalLauncher};

    let temp = TempDir::new("terminal-app-launcher");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp
        .path()
        .join("Project 'quoted' $value; \u{9879}\u{76ee}");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd.clone()), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();
    let helper = temp.path().join("SessionMesh helper 'quoted' $value");
    fs::write(
        &helper,
        "#!/bin/sh\npwd > \"$0.cwd\"\nprintf '%s\\n' \"$@\" > \"$0.args\"\n",
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let osascript = temp.path().join("osascript");
    fs::write(
        &osascript,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nfor argument in \"$@\"; do command=$argument; done\n/bin/sh -c \"$command\"\nprintf 'open\\n' > \"$0.shell-open\"\n",
    )
    .unwrap();
    fs::set_permissions(&osascript, fs::Permissions::from_mode(0o700)).unwrap();
    let mut launcher = MacosTerminalLauncher::with_osascript(
        helper.clone(),
        paths.task_store.clone(),
        osascript.clone(),
    );

    launcher.open(&task.id, 30_000).unwrap();

    assert_eq!(
        fs::read_to_string(helper.with_extension("cwd")).unwrap(),
        format!("{}\n", cwd.display())
    );
    assert_eq!(
        fs::read_to_string(helper.with_extension("args")).unwrap(),
        format!(
            "--launch-successor\n--task-store\n{}\n--task-id\n{}\n",
            paths.task_store.display(),
            task.id
        )
    );
    assert_eq!(
        fs::read_to_string(osascript.with_extension("shell-open")).unwrap(),
        "open\n"
    );
    let opened = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(
        opened
            .successor_launch_metadata
            .as_ref()
            .and_then(|metadata| metadata.requested_at_ms),
        Some(30_000)
    );
    assert_eq!(opened.state_changed_at_ms, ready.state_changed_at_ms);

    let failing_osascript = temp.path().join("failing-osascript");
    fs::write(&failing_osascript, "#!/bin/sh\nexit 9\n").unwrap();
    fs::set_permissions(&failing_osascript, fs::Permissions::from_mode(0o700)).unwrap();
    let mut failing =
        MacosTerminalLauncher::with_osascript(helper, paths.task_store, failing_osascript);
    assert_eq!(
        failing.open(&task.id, 31_000).unwrap_err().to_string(),
        "Terminal.app launch failed"
    );
    let retained = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(retained.state, OperationTaskState::Ready);
    assert_eq!(retained.artifact_path, ready.artifact_path);
    let metadata = retained.successor_launch_metadata.as_ref().unwrap();
    assert_eq!(metadata.requested_at_ms, Some(30_000));
    assert_eq!(metadata.failed_at_ms, Some(31_000));
    assert_eq!(
        metadata.failure_code.as_deref(),
        Some("terminal_app_launch_failed")
    );
    assert!(sessionmesh::successor::render_successor_command(&retained).is_ok());
}

#[cfg(unix)]
#[test]
fn foreground_helper_inherits_interactive_streams_waits_and_persists_no_conversation() {
    let temp = TempDir::new("foreground-successor-helper");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let bin_dir = temp.path().join("bin");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::create_dir(&bin_dir).unwrap();
    provider_that_returns(&claude, valid_handoff());
    fs::write(
        &codex,
        "#!/bin/sh\nIFS= read -r line\nprintf 'successor stdout: %s\\n' \"$line\"\nprintf 'successor stderr: private tool output\\n' >&2\nsleep 0.2\n",
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).unwrap();
    let ps = bin_dir.join("ps");
    fs::write(&ps, "#!/bin/sh\nsleep 0.3\nexec /bin/ps \"$@\"\n").unwrap();
    fs::set_permissions(&ps, fs::Permissions::from_mode(0o700)).unwrap();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited_to(
            &exited_session(source_path, cwd),
            Provider::CodexCli,
            10_000,
        )
        .unwrap();
    HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_sessionmesh"))
        .arg("--launch-successor")
        .arg("--task-store")
        .arg(&paths.task_store)
        .arg("--task-id")
        .arg(&task.id)
        .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"private interactive prompt\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success());
    assert!(started.elapsed() >= Duration::from_millis(180));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "successor stdout: private interactive prompt\n"
    );
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "successor stderr: private tool output\n"
    );
    let launched = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(launched.state, OperationTaskState::Ready);
    assert_eq!(launched.successor_session_id, None);
    assert_eq!(launched.successor_linked_at_ms, None);
    let metadata = launched.successor_launch_metadata.unwrap();
    assert!(metadata.process_identity.is_some());
    assert!(metadata.started_at_ms.is_some());
    assert!(metadata.exited_at_ms >= metadata.started_at_ms);
    assert_eq!(metadata.exit_code, Some(0));
    assert_eq!(metadata.failure_code, None);
    drop(handoff);
    for entry in fs::read_dir(temp.path()).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_name().to_string_lossy().starts_with("tasks.db") {
            continue;
        }
        let contents = fs::read(entry.path()).unwrap();
        for prohibited in [
            b"private interactive prompt".as_slice(),
            b"private tool output".as_slice(),
            b"successor stdout".as_slice(),
        ] {
            assert!(!contents
                .windows(prohibited.len())
                .any(|window| window == prohibited));
        }
    }
}

#[cfg(unix)]
#[test]
fn foreground_helper_links_claude_after_exact_pid_state_evidence_appears() {
    let temp = TempDir::new("foreground-claude-linkage");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let claude_dir = temp.path().join("claude-data");
    let claude_sessions = claude_dir.join("sessions");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(&claude_sessions).unwrap();
    executable(&claude);
    provider_that_returns(&codex, valid_handoff());
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-source-session".into();
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited_to(&session, Provider::ClaudeCode, 10_000)
        .unwrap();
    HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();
    assert_eq!(
        handoff
            .task(&task.id)
            .unwrap()
            .unwrap()
            .successor_session_id,
        None
    );
    fs::write(&claude, "#!/bin/sh\nsleep 2\n").unwrap();
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o700)).unwrap();
    let task_store_path = paths.task_store.clone();
    let task_id = task.id.clone();
    let evidence_writer = std::thread::spawn(move || {
        let store = TaskStore::open(&task_store_path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let process = loop {
            let observed = store.get(&task_id).unwrap().unwrap();
            if let Some(process) = observed
                .successor_launch_metadata
                .and_then(|metadata| metadata.process_identity)
            {
                break process;
            }
            assert!(
                Instant::now() < deadline,
                "Successor process was not recorded"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        fs::write(
            claude_sessions.join(format!("{}.json", process.pid)),
            format!(
                "{{\"pid\":{},\"sessionId\":\"claude-successor-session\",\"status\":\"idle\",\"startedAt\":{}}}",
                process.pid, process.started_at_ms
            ),
        )
        .unwrap();
    });
    let config = Config {
        claude_dir,
        codex_dir: temp.path().join("codex-data"),
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };

    assert!(sessionmesh::successor::run_successor_from_args_with_config(
        [
            "sessionmesh".into(),
            "--launch-successor".into(),
            "--task-store".into(),
            paths.task_store.as_os_str().to_owned(),
            "--task-id".into(),
            task.id.clone().into(),
        ],
        &config,
    )
    .unwrap());
    evidence_writer.join().unwrap();

    let linked = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(linked.state, OperationTaskState::Ready);
    assert_eq!(
        linked.successor_session_id.as_deref(),
        Some("claude-successor-session")
    );
    assert!(linked.successor_linked_at_ms.is_some());
    assert_ne!(
        linked.successor_session_id.as_deref(),
        Some(linked.source_session_id.as_str())
    );
}

#[cfg(unix)]
#[test]
fn foreground_helper_links_codex_only_to_the_rollout_held_by_the_exact_process() {
    let temp = TempDir::new("foreground-codex-linkage");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let codex_dir = temp.path().join("codex-data");
    let rollout_dir = codex_dir.join("sessions/2026/08/04");
    let held_rollout =
        rollout_dir.join("rollout-2026-08-04T00-00-00-11111111-1111-1111-1111-111111111111.jsonl");
    let newer_unheld_rollout =
        rollout_dir.join("rollout-2026-08-04T00-00-01-22222222-2222-2222-2222-222222222222.jsonl");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(&rollout_dir).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex.clone());
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited_to(
            &exited_session(source_path, cwd),
            Provider::CodexCli,
            10_000,
        )
        .unwrap();
    HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();
    fs::write(
        &held_rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-held-successor\"}}\n",
    )
    .unwrap();
    fs::write(
        &newer_unheld_rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-newer-decoy\"}}\n",
    )
    .unwrap();
    fs::write(
        codex.with_extension("rollout-path"),
        held_rollout.to_string_lossy().as_bytes(),
    )
    .unwrap();
    fs::write(
        &codex,
        "#!/bin/sh\nrollout=$(cat \"$0.rollout-path\")\nexec 3<\"$rollout\"\nsleep 2\n",
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        claude_dir: temp.path().join("claude-data"),
        codex_dir,
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };

    assert!(sessionmesh::successor::run_successor_from_args_with_config(
        [
            "sessionmesh".into(),
            "--launch-successor".into(),
            "--task-store".into(),
            paths.task_store.as_os_str().to_owned(),
            "--task-id".into(),
            task.id.clone().into(),
        ],
        &config,
    )
    .unwrap());

    let linked = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(linked.state, OperationTaskState::Ready);
    assert_eq!(
        linked.successor_session_id.as_deref(),
        Some("codex-held-successor")
    );
    assert_ne!(
        linked.successor_session_id.as_deref(),
        Some("codex-newer-decoy")
    );
    assert!(linked.successor_linked_at_ms.is_some());
}

#[cfg(unix)]
#[test]
fn concurrent_same_directory_claude_launches_link_each_task_by_its_own_process() {
    let temp = TempDir::new("concurrent-claude-linkage");
    let cwd = temp.path().join("shared-project");
    let source_a = temp.path().join("source-a.jsonl");
    let source_b = temp.path().join("source-b.jsonl");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let claude_dir = temp.path().join("claude-data");
    let claude_sessions = claude_dir.join("sessions");
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(&claude_sessions).unwrap();
    fs::write(&source_a, "{}\n").unwrap();
    fs::write(&source_b, "{}\n").unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let mut session_a = exited_session(source_a, cwd.clone());
    session_a.id = "claude-source-a".into();
    let task_a = handoff.authorize_exited(&session_a, 10_000).unwrap();
    HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task_a.id, 20_000)
        .unwrap();
    let mut session_b = exited_session(source_b, cwd);
    session_b.id = "claude-source-b".into();
    let task_b = handoff.authorize_exited(&session_b, 11_000).unwrap();
    HandoffWorker::new(paths.clone(), executables)
        .run(&task_b.id, 21_000)
        .unwrap();
    fs::write(&claude, "#!/bin/sh\nsleep 2\n").unwrap();
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o700)).unwrap();
    let evidence_task_store = paths.task_store.clone();
    let evidence_tasks = [
        (task_a.id.clone(), "claude-successor-a"),
        (task_b.id.clone(), "claude-successor-b"),
    ];
    let evidence_writer = std::thread::spawn(move || {
        let store = TaskStore::open(&evidence_task_store).unwrap();
        for (task_id, session_id) in evidence_tasks {
            let deadline = Instant::now() + Duration::from_secs(5);
            let process = loop {
                let observed = store.get(&task_id).unwrap().unwrap();
                if let Some(process) = observed
                    .successor_launch_metadata
                    .and_then(|metadata| metadata.process_identity)
                {
                    break process;
                }
                assert!(
                    Instant::now() < deadline,
                    "Successor process was not recorded"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            fs::write(
                claude_sessions.join(format!("{}.json", process.pid)),
                format!(
                    "{{\"pid\":{},\"sessionId\":\"{session_id}\",\"status\":\"idle\",\"startedAt\":{}}}",
                    process.pid, process.started_at_ms
                ),
            )
            .unwrap();
        }
    });
    let config = Config {
        claude_dir,
        codex_dir: temp.path().join("codex-data"),
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };
    let launch = |task_id: String, config: Config, task_store: PathBuf| {
        std::thread::spawn(move || {
            sessionmesh::successor::run_successor_from_args_with_config(
                [
                    "sessionmesh".into(),
                    "--launch-successor".into(),
                    "--task-store".into(),
                    task_store.into_os_string(),
                    "--task-id".into(),
                    task_id.into(),
                ],
                &config,
            )
            .unwrap()
        })
    };
    let launch_a = launch(task_a.id.clone(), config.clone(), paths.task_store.clone());
    let launch_b = launch(task_b.id.clone(), config, paths.task_store.clone());

    assert!(launch_a.join().unwrap());
    assert!(launch_b.join().unwrap());
    evidence_writer.join().unwrap();

    let linked_a = handoff.task(&task_a.id).unwrap().unwrap();
    let linked_b = handoff.task(&task_b.id).unwrap().unwrap();
    assert_eq!(
        linked_a.successor_session_id.as_deref(),
        Some("claude-successor-a")
    );
    assert_eq!(
        linked_b.successor_session_id.as_deref(),
        Some("claude-successor-b")
    );
    assert_eq!(linked_a.state, OperationTaskState::Ready);
    assert_eq!(linked_b.state, OperationTaskState::Ready);
}

#[cfg(unix)]
#[test]
fn concurrent_same_directory_codex_launches_link_each_task_by_its_held_rollout() {
    let temp = TempDir::new("concurrent-codex-linkage");
    let cwd = temp.path().join("shared-project");
    let source_a = temp.path().join("source-a.jsonl");
    let source_b = temp.path().join("source-b.jsonl");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let codex_dir = temp.path().join("codex-data");
    let rollout_dir = codex_dir.join("sessions/2026/08/04");
    fs::create_dir(&cwd).unwrap();
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(&source_a, "{}\n").unwrap();
    fs::write(&source_b, "{}\n").unwrap();
    executable(&claude);
    provider_that_returns(&codex, valid_handoff());
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex.clone());
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let mut session_a = exited_session(source_a, cwd.clone());
    session_a.provider = Provider::CodexCli;
    session_a.id = "codex-source-a".into();
    let task_a = handoff.authorize_exited(&session_a, 10_000).unwrap();
    HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task_a.id, 20_000)
        .unwrap();
    let mut session_b = exited_session(source_b, cwd);
    session_b.provider = Provider::CodexCli;
    session_b.id = "codex-source-b".into();
    let task_b = handoff.authorize_exited(&session_b, 11_000).unwrap();
    HandoffWorker::new(paths.clone(), executables)
        .run(&task_b.id, 21_000)
        .unwrap();
    fs::write(
        codex.with_extension("sessions-dir"),
        rollout_dir.to_string_lossy().as_bytes(),
    )
    .unwrap();
    fs::write(
        &codex,
        "#!/bin/sh\nsessions=$(cat \"$0.sessions-dir\")\nsession_id=codex-successor-$$\nrollout=\"$sessions/rollout-process-$$.jsonl\"\nprintf '{\"type\":\"session_meta\",\"payload\":{\"id\":\"%s\"}}\\n' \"$session_id\" > \"$rollout\"\nexec 3<\"$rollout\"\nsleep 2\n",
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        claude_dir: temp.path().join("claude-data"),
        codex_dir,
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };
    let launch = |task_id: String, config: Config, task_store: PathBuf| {
        std::thread::spawn(move || {
            sessionmesh::successor::run_successor_from_args_with_config(
                [
                    "sessionmesh".into(),
                    "--launch-successor".into(),
                    "--task-store".into(),
                    task_store.into_os_string(),
                    "--task-id".into(),
                    task_id.into(),
                ],
                &config,
            )
            .unwrap()
        })
    };
    let launch_a = launch(task_a.id.clone(), config.clone(), paths.task_store.clone());
    let launch_b = launch(task_b.id.clone(), config, paths.task_store.clone());

    assert!(launch_a.join().unwrap());
    assert!(launch_b.join().unwrap());

    let linked_a = handoff.task(&task_a.id).unwrap().unwrap();
    let linked_b = handoff.task(&task_b.id).unwrap().unwrap();
    let process_a = linked_a
        .successor_launch_metadata
        .as_ref()
        .and_then(|metadata| metadata.process_identity)
        .unwrap();
    let process_b = linked_b
        .successor_launch_metadata
        .as_ref()
        .and_then(|metadata| metadata.process_identity)
        .unwrap();
    assert_eq!(
        linked_a.successor_session_id,
        Some(format!("codex-successor-{}", process_a.pid))
    );
    assert_eq!(
        linked_b.successor_session_id,
        Some(format!("codex-successor-{}", process_b.pid))
    );
    assert_ne!(linked_a.successor_session_id, linked_b.successor_session_id);
    assert_eq!(linked_a.state, OperationTaskState::Ready);
    assert_eq!(linked_b.state, OperationTaskState::Ready);
}

#[cfg(unix)]
#[test]
fn foreground_helper_launch_failure_is_bounded_and_preserves_the_ready_result() {
    let temp = TempDir::new("foreground-successor-launch-failure");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    fs::write(&codex, "#!/missing-successor-interpreter\n").unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).unwrap();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited_to(
            &exited_session(source_path, cwd),
            Provider::CodexCli,
            10_000,
        )
        .unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_sessionmesh"))
        .arg("--launch-successor")
        .arg("--task-store")
        .arg(&paths.task_store)
        .arg("--task-id")
        .arg(&task.id)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Error: Successor Provider launch failed\n"
    );
    let retained = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(retained.state, OperationTaskState::Ready);
    assert_eq!(retained.artifact_path, ready.artifact_path);
    assert_eq!(retained.successor_launch_plan, ready.successor_launch_plan);
    let metadata = retained.successor_launch_metadata.as_ref().unwrap();
    assert!(metadata.failed_at_ms.is_some());
    assert_eq!(
        metadata.failure_code.as_deref(),
        Some("successor_provider_start_failed")
    );
    assert!(sessionmesh::successor::render_successor_command(&retained).is_ok());
}

#[test]
fn preflight_rejects_missing_exited_session_evidence_before_task_creation() {
    let temp = TempDir::new("preflight");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let launcher = RecordingWorkerLauncher::default();
    let launches = launcher.launches.clone();
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let mut handoff = HandoffService::open(
        paths.clone(),
        ProviderExecutables::new(claude.clone(), codex.clone()),
        launcher,
    )
    .unwrap();
    let valid = exited_session(source_path.clone(), cwd.clone());

    let mut open = valid.clone();
    open.state = SessionState::Idle;
    assert!(handoff.authorize_exited(&open, 10_000).is_err());

    let mut no_id = valid.clone();
    no_id.id.clear();
    assert!(handoff.authorize_exited(&no_id, 10_000).is_err());

    let mut no_source = valid.clone();
    no_source.source.path = temp.path().join("missing.jsonl");
    assert!(handoff.authorize_exited(&no_source, 10_000).is_err());

    let mut no_cwd = valid.clone();
    no_cwd.cwd = None;
    assert!(handoff.authorize_exited(&no_cwd, 10_000).is_err());

    let mut no_cli = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("missing-cli-tasks.db"),
            artifact_root: temp.path().join("missing-cli-artifacts"),
        },
        ProviderExecutables::new(temp.path().join("missing-claude"), codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    assert!(no_cli.authorize_exited(&valid, 10_000).is_err());

    assert!(handoff.tasks().unwrap().is_empty());
    assert!(no_cli.tasks().unwrap().is_empty());
    assert!(launches.lock().unwrap().is_empty());
}

#[test]
fn preflight_rejects_a_missing_selected_successor_cli_before_task_creation() {
    let temp = TempDir::new("missing-successor-cli");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    let launcher = RecordingWorkerLauncher::default();
    let launches = launcher.launches.clone();
    let mut handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, temp.path().join("missing-codex")),
        launcher,
    )
    .unwrap();

    let error = handoff
        .authorize_exited_to(
            &exited_session(source_path, cwd),
            Provider::CodexCli,
            10_000,
        )
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Handoff is ineligible: selected Successor Provider CLI is unavailable"
    );
    assert!(handoff.tasks().unwrap().is_empty());
    assert!(launches.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn completed_artifact_is_atomically_published_with_owner_only_permissions() {
    let temp = TempDir::new("artifact-permissions");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let ready = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    let artifact = ready.artifact_path.unwrap();
    let task_directory = artifact.parent().unwrap();
    assert_eq!(
        fs::metadata(task_directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&artifact).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::read_dir(task_directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        vec![std::ffi::OsString::from("handoff.md")]
    );
}

#[test]
fn task_store_never_contains_prompt_or_artifact_content() {
    let temp = TempDir::new("task-store-privacy");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();
    drop(handoff);

    for entry in fs::read_dir(temp.path()).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_name().to_string_lossy().starts_with("tasks.db") {
            continue;
        }
        let contents = fs::read(entry.path()).unwrap();
        for prohibited in [
            b"/sessionmesh-handoff\n".as_slice(),
            b"Ship Assisted Handoff.".as_slice(),
            b"Task execution is implemented.".as_slice(),
        ] {
            assert!(
                !contents
                    .windows(prohibited.len())
                    .any(|window| window == prohibited),
                "Task Store file {} contains prohibited content",
                entry.path().display()
            );
        }
    }
}

#[test]
fn candidate_with_an_extra_contract_section_is_rejected() {
    let temp = TempDir::new("extra-artifact-section");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, &handoff_with_extra_section());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let failed = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(failed.failure_code.as_deref(), Some("invalid_artifact"));
}

#[test]
fn artifact_publication_failure_becomes_a_known_failed_task() {
    let temp = TempDir::new("artifact-publish-failed");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let blocked_artifact_root = temp.path().join("not-a-directory");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::write(&blocked_artifact_root, "blocks directory creation").unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: blocked_artifact_root,
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let failed = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(
        failed.failure_code.as_deref(),
        Some("artifact_publish_failed")
    );
    assert!(failed.artifact_path.is_none());
}

#[cfg(unix)]
#[test]
fn authoritative_task_store_is_owner_only() {
    let temp = TempDir::new("task-store-permissions");
    let private_data = temp.path().join("private-data");
    let task_store = private_data.join("tasks.db");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    executable(&claude);
    executable(&codex);

    let _handoff = HandoffService::open(
        HandoffPaths {
            task_store: task_store.clone(),
            artifact_root: private_data.join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();

    assert_eq!(
        fs::metadata(&private_data).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&task_store).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn artifact_publication_refuses_a_symlinked_artifact_root() {
    let temp = TempDir::new("artifact-root-symlink");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    let outside = temp.path().join("outside");
    let artifact_root = temp.path().join("artifacts-link");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &artifact_root).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root,
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let failed = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(
        failed.failure_code.as_deref(),
        Some("artifact_publish_failed")
    );
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

#[test]
fn invalid_provider_markdown_fails_without_publishing_an_artifact() {
    let temp = TempDir::new("invalid-artifact");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(
        &claude,
        "# Handoff\n\n## Objective\n\n\n## Current State\n\nKnown.\n",
    );
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let failed = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(failed.failure_code.as_deref(), Some("invalid_artifact"));
    assert!(failed.artifact_path.is_none());
    assert!(!paths
        .artifact_root
        .join(task.id)
        .join("handoff.md")
        .exists());
}

#[test]
fn invalid_codex_markdown_fails_without_publishing_an_artifact() {
    let temp = TempDir::new("codex-invalid-artifact");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    provider_that_returns(
        &codex,
        "# Handoff\n\n## Objective\n\n\n## Current State\n\nKnown.\n",
    );
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-native-session".into();
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff.authorize_exited(&session, 10_000).unwrap();

    let failed = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(failed.failure_code.as_deref(), Some("invalid_artifact"));
    assert!(failed.artifact_path.is_none());
    assert!(!paths
        .artifact_root
        .join(task.id)
        .join("handoff.md")
        .exists());
}

#[test]
fn a_known_provider_failure_becomes_failed_without_storing_provider_output() {
    let temp = TempDir::new("provider-failed");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_exits(&claude, 7);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let failed = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(
        failed.failure_code.as_deref(),
        Some("provider_invocation_failed")
    );
    assert!(failed.artifact_path.is_none());
}

#[test]
fn a_known_codex_failure_becomes_failed_without_an_artifact() {
    let temp = TempDir::new("codex-provider-failed");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    provider_that_exits(&codex, 7);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-native-session".into();
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff.authorize_exited(&session, 10_000).unwrap();

    let failed = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(
        failed.failure_code.as_deref(),
        Some("provider_invocation_failed")
    );
    assert!(failed.artifact_path.is_none());
}

#[test]
fn provider_nonzero_exit_is_retained_as_execution_trace_metadata() {
    let temp = TempDir::new("provider-exit-trace");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_exits(&claude, 23);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();
    let retained = TaskStore::open(&paths.task_store)
        .unwrap()
        .get(&task.id)
        .unwrap()
        .unwrap();

    assert_eq!(retained.state, OperationTaskState::Failed);
    assert!(retained.provider_process_identity.is_some());
    assert!(retained.prompt_sent_at_ms.is_some());
    assert!(retained.provider_exited_at_ms.is_some());
    assert_eq!(retained.provider_exit_code, Some(23));
}

#[cfg(unix)]
#[test]
fn interrupted_provider_output_never_becomes_a_completed_artifact() {
    let temp = TempDir::new("provider-interrupted");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_is_interrupted(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let interrupted = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(interrupted.state, OperationTaskState::Interrupted);
    assert_eq!(
        interrupted.failure_code.as_deref(),
        Some("provider_outcome_uncertain")
    );
    assert!(!paths
        .artifact_root
        .join(task.id)
        .join("handoff.md")
        .exists());
}

#[cfg(unix)]
#[test]
fn interrupted_codex_output_never_becomes_a_completed_artifact() {
    let temp = TempDir::new("codex-provider-interrupted");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    provider_that_is_interrupted(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-native-session".into();
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff.authorize_exited(&session, 10_000).unwrap();

    let interrupted = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(interrupted.state, OperationTaskState::Interrupted);
    assert_eq!(
        interrupted.failure_code.as_deref(),
        Some("provider_outcome_uncertain")
    );
    assert!(!paths
        .artifact_root
        .join(task.id)
        .join("handoff.md")
        .exists());
}

#[test]
fn candidate_containing_secret_material_is_rejected_before_publication() {
    let temp = TempDir::new("secret-artifact");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, &handoff_with_secret());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let failed = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(failed.failure_code.as_deref(), Some("invalid_artifact"));
    assert!(!paths
        .artifact_root
        .join(task.id)
        .join("handoff.md")
        .exists());
}

#[test]
fn candidate_containing_a_complete_transcript_section_is_rejected() {
    let temp = TempDir::new("transcript-artifact");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, &handoff_with_transcript());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let failed = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(failed.failure_code.as_deref(), Some("invalid_artifact"));
    assert!(!paths
        .artifact_root
        .join(task.id)
        .join("handoff.md")
        .exists());
}

#[test]
fn candidate_containing_a_raw_tool_output_section_is_rejected() {
    let temp = TempDir::new("tool-output-artifact");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, &handoff_with_tool_output());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let failed = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(failed.failure_code.as_deref(), Some("invalid_artifact"));
    assert!(!paths
        .artifact_root
        .join(task.id)
        .join("handoff.md")
        .exists());
}

#[test]
fn worker_launch_failure_is_persisted_as_a_failed_operation_task() {
    let temp = TempDir::new("worker-launch-failed");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let mut handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        FailingWorkerLauncher,
    )
    .unwrap();

    let failed = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(failed.failure_code.as_deref(), Some("worker_start_failed"));
    assert_eq!(handoff.task(&failed.id).unwrap(), Some(failed));
}

#[test]
fn detached_worker_completes_after_the_launching_service_is_dropped() {
    let temp = TempDir::new("detached-worker");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let claude_sessions_dir = temp.path().join("claude-sessions");
    fs::create_dir(&claude_sessions_dir).unwrap();
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        claude_sessions_dir,
    );
    let mut handoff = HandoffService::open(paths.clone(), executables.clone(), launcher).unwrap();

    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    drop(handoff);

    let observer =
        HandoffService::open(paths, executables, RecordingWorkerLauncher::default()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let completed = loop {
        let observed = observer.task(&task.id).unwrap().unwrap();
        if observed.state.is_terminal() {
            break observed;
        }
        assert!(Instant::now() < deadline, "detached Worker did not finish");
        std::thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(completed.state, OperationTaskState::Ready);
    assert!(completed.artifact_path.unwrap().is_file());
}

#[test]
fn detached_worker_receives_managed_skill_roots_and_revalidates_the_installation() {
    let temp = TempDir::new("detached-worker-managed-skills");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns_and_records_full_input(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let skills = ManagedHandoffSkills::new(
        temp.path().join("agents/skills"),
        temp.path().join("claude-home/skills"),
    );
    let claude_sessions_dir = temp.path().join("claude-sessions");
    fs::create_dir(&claude_sessions_dir).unwrap();
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        claude_sessions_dir,
    )
    .with_managed_handoff_skills(skills.clone());
    let mut handoff = HandoffService::open(paths.clone(), executables.clone(), launcher)
        .unwrap()
        .with_managed_handoff_skills(skills);

    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let completed = loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if observed.state.is_terminal() {
            break observed;
        }
        assert!(Instant::now() < deadline, "detached Worker did not finish");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(completed.state, OperationTaskState::Ready);
    assert_eq!(
        fs::read_to_string(claude.with_extension("prompt")).unwrap(),
        "/sessionmesh-handoff\n"
    );
}

#[test]
fn running_task_persists_revalidatable_worker_and_provider_evidence() {
    let temp = TempDir::new("running-process-evidence");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_pauses_then_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        temp.path().join("claude-sessions"),
    );
    fs::create_dir(temp.path().join("claude-sessions")).unwrap();
    let mut handoff = HandoffService::open(paths.clone(), executables.clone(), launcher).unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let running = loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if observed.prompt_sent_at_ms.is_some() {
            break observed;
        }
        assert!(Instant::now() < deadline, "Worker did not send the Prompt");
        std::thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(running.state, OperationTaskState::Running);
    assert!(running.worker_process_identity.is_some());
    assert!(running.provider_process_identity.is_some());
    assert_eq!(
        running.running_deadline_ms,
        Some(running.state_changed_at_ms + 30 * 60 * 1_000)
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if observed.state == OperationTaskState::Ready {
            break;
        }
        assert!(Instant::now() < deadline, "detached Worker did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn reconciliation_promotes_only_the_declared_final_artifact_after_publication_crash() {
    let temp = TempDir::new("reconcile-declared-final-artifact");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_pauses_then_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        temp.path().join("claude-sessions"),
    );
    fs::create_dir(temp.path().join("claude-sessions")).unwrap();
    let mut handoff = HandoffService::open(paths.clone(), executables, launcher).unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if observed.prompt_sent_at_ms.is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "Worker did not send the Prompt");
        std::thread::sleep(Duration::from_millis(20));
    }
    let declared_artifact = paths.artifact_root.join(&task.id).join("handoff.md");
    fs::create_dir_all(declared_artifact.parent().unwrap()).unwrap();
    fs::write(&declared_artifact, valid_handoff()).unwrap();
    fs::write(
        paths.artifact_root.join("newest-unrelated.md"),
        valid_handoff(),
    )
    .unwrap();

    handoff.reconcile_tasks(20_000).unwrap();
    let ready = handoff.task(&task.id).unwrap().unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(ready.artifact_path.as_ref(), Some(&declared_artifact));
    assert_eq!(
        ready
            .successor_launch_plan
            .as_ref()
            .map(|plan| &plan.artifact_path),
        Some(&declared_artifact)
    );
    std::thread::sleep(Duration::from_millis(1_100));
}

#[test]
fn reconciliation_keeps_the_successor_executable_selected_at_authorization() {
    let temp = TempDir::new("reconcile-selected-successor-executable");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let selected_codex = temp.path().join("selected-codex");
    let later_codex = temp.path().join("later-codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_pauses_then_returns(&claude, valid_handoff());
    executable(&selected_codex);
    executable(&later_codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), selected_codex.clone());
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables,
        temp.path().join("claude-sessions"),
    );
    fs::create_dir(temp.path().join("claude-sessions")).unwrap();
    let mut handoff = HandoffService::open(
        paths.clone(),
        ProviderExecutables::new(claude.clone(), selected_codex.clone()),
        launcher,
    )
    .unwrap();
    let task = handoff
        .authorize_exited_to(
            &exited_session(source_path, cwd),
            Provider::CodexCli,
            10_000,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if handoff
            .task(&task.id)
            .unwrap()
            .unwrap()
            .prompt_sent_at_ms
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "Worker did not send the Prompt");
        std::thread::sleep(Duration::from_millis(20));
    }
    let artifact_path = paths.artifact_root.join(&task.id).join("handoff.md");
    fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
    fs::write(&artifact_path, valid_handoff()).unwrap();
    drop(handoff);
    let restarted = HandoffService::open(
        paths,
        ProviderExecutables::new(claude, later_codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();

    restarted.reconcile_tasks(20_000).unwrap();
    let ready = restarted.task(&task.id).unwrap().unwrap();

    assert_eq!(
        ready
            .successor_launch_plan
            .as_ref()
            .map(|plan| &plan.executable),
        Some(&selected_codex)
    );
    assert!(sessionmesh::successor::render_successor_command(&ready).is_ok());
    std::thread::sleep(Duration::from_millis(1_100));
}

#[test]
fn confirmed_cancellation_after_prompt_gracefully_stops_the_owned_provider() {
    let temp = TempDir::new("cancel-running-provider");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_waits_for_term(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        temp.path().join("claude-sessions"),
    );
    fs::create_dir(temp.path().join("claude-sessions")).unwrap();
    let mut handoff = HandoffService::open(paths, executables, launcher).unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if handoff
            .task(&task.id)
            .unwrap()
            .unwrap()
            .prompt_sent_at_ms
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "Worker did not send the Prompt");
        std::thread::sleep(Duration::from_millis(20));
    }

    let cancelling = handoff.cancel_task(&task.id, 20_000).unwrap();
    assert_eq!(cancelling.state, OperationTaskState::Running);
    assert_eq!(cancelling.cancellation_requested_at_ms, Some(20_000));
    assert_eq!(cancelling.termination_requested_at_ms, Some(20_000));

    let deadline = Instant::now() + Duration::from_secs(5);
    let cancelled = loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if observed.state.is_terminal() {
            break observed;
        }
        assert!(Instant::now() < deadline, "cancelled Provider did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(cancelled.state, OperationTaskState::Cancelled);
    assert_eq!(cancelled.failure_code.as_deref(), Some("cancelled_by_user"));
    assert!(cancelled.artifact_path.is_none());
}

#[test]
fn valid_result_completed_during_cancellation_becomes_ready() {
    let temp = TempDir::new("cancel-valid-late-result");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns_valid_result_on_term(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let candidate_complete = claude.with_extension("candidate-complete");
    let executables = ProviderExecutables::new(claude, codex);
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        temp.path().join("claude-sessions"),
    );
    fs::create_dir(temp.path().join("claude-sessions")).unwrap();
    let mut handoff = HandoffService::open(paths, executables, launcher).unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if candidate_complete.is_file() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Provider did not complete its candidate"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    handoff.cancel_task(&task.id, 20_000).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let ready = loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if observed.state.is_terminal() {
            break observed;
        }
        assert!(Instant::now() < deadline, "late Result did not settle");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(ready.state, OperationTaskState::Ready);
    assert!(ready.artifact_path.unwrap().is_file());
}

#[test]
fn running_deadline_gracefully_stops_provider_and_resolves_known_failure() {
    let temp = TempDir::new("running-deadline");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_waits_for_term(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        temp.path().join("claude-sessions"),
    );
    fs::create_dir(temp.path().join("claude-sessions")).unwrap();
    let mut handoff = HandoffService::open(paths, executables, launcher).unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let running_deadline_ms = loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if let Some(deadline_ms) = observed.prompt_sent_at_ms.and(observed.running_deadline_ms) {
            break deadline_ms;
        }
        assert!(Instant::now() < deadline, "Worker did not send the Prompt");
        std::thread::sleep(Duration::from_millis(20));
    };

    handoff.reconcile_tasks(running_deadline_ms + 1).unwrap();
    let terminating = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(terminating.state, OperationTaskState::Running);
    assert_eq!(
        terminating.termination_requested_at_ms,
        Some(running_deadline_ms + 1)
    );
    assert_eq!(terminating.cancellation_requested_at_ms, None);

    let deadline = Instant::now() + Duration::from_secs(5);
    let failed = loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if observed.state.is_terminal() {
            break observed;
        }
        assert!(
            Instant::now() < deadline,
            "deadline termination did not settle"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(failed.state, OperationTaskState::Failed);
    assert_eq!(
        failed.failure_code.as_deref(),
        Some("running_deadline_exceeded")
    );
}

#[test]
fn crash_after_prompt_becomes_interrupted_without_accepting_partial_output_or_resending() {
    let temp = TempDir::new("worker-crash-after-prompt");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    provider_that_outlives_a_crashed_worker(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex.clone());
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        temp.path().join("claude-sessions"),
    );
    fs::create_dir(temp.path().join("claude-sessions")).unwrap();
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-crash-source".into();
    let mut handoff = HandoffService::open(paths.clone(), executables.clone(), launcher).unwrap();
    let task = handoff.authorize_exited(&session, 10_000).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let running = loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if observed.prompt_sent_at_ms.is_some() {
            break observed;
        }
        assert!(Instant::now() < deadline, "Worker did not send the Prompt");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(running.state, OperationTaskState::Running);

    let reconciled_by_restarted_worker = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task.id, running.state_changed_at_ms + 1)
        .unwrap();
    assert_eq!(
        reconciled_by_restarted_worker.state,
        OperationTaskState::Running
    );
    let prompt_path = codex.with_extension("prompt");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !prompt_path.is_file() {
        assert!(
            Instant::now() < deadline,
            "Provider did not receive the Prompt"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read_to_string(&prompt_path).unwrap(),
        "$sessionmesh-handoff\n"
    );

    let observer_launches = RecordingWorkerLauncher::default();
    let launches = observer_launches.launches.clone();
    let observer = HandoffService::open(paths, executables, observer_launches).unwrap();
    observer
        .reconcile_tasks(running.state_changed_at_ms + 1)
        .unwrap();
    assert_eq!(
        observer.task(&task.id).unwrap().unwrap().state,
        OperationTaskState::Running
    );
    fs::create_dir_all(temp.path().join("artifacts")).unwrap();
    fs::write(
        temp.path().join("artifacts").join("newest-unrelated.md"),
        valid_handoff(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let interrupted = loop {
        observer
            .reconcile_tasks(running.state_changed_at_ms + 2_000)
            .unwrap();
        let observed = observer.task(&task.id).unwrap().unwrap();
        if observed.state.is_terminal() {
            break observed;
        }
        assert!(
            Instant::now() < deadline,
            "crashed execution did not settle"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(interrupted.state, OperationTaskState::Interrupted);
    assert_eq!(
        interrupted.failure_code.as_deref(),
        Some("execution_outcome_uncertain")
    );
    assert!(interrupted.artifact_path.is_none());
    assert!(!temp
        .path()
        .join("artifacts")
        .join(format!("{}.md", task.id))
        .exists());
    assert!(launches.lock().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(codex.with_extension("prompt")).unwrap(),
        "$sessionmesh-handoff\n"
    );
}

#[test]
fn explicit_retry_creates_a_fresh_task_and_preserves_cancelled_predecessor() {
    let temp = TempDir::new("retry-cancelled");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let launcher = RecordingWorkerLauncher::default();
    let launches = launcher.launches.clone();
    let mut handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        launcher,
    )
    .unwrap();
    let session = exited_session(source_path, cwd);
    let predecessor = handoff.authorize_exited(&session, 10_000).unwrap();
    let cancelled = handoff.cancel_task(&predecessor.id, 11_000).unwrap();

    let retry = handoff
        .retry_task(&predecessor.id, &session, false, 20_000)
        .unwrap();

    assert_ne!(retry.id, predecessor.id);
    assert_eq!(
        retry.retry_of_task_id.as_deref(),
        Some(predecessor.id.as_str())
    );
    assert_eq!(retry.state, OperationTaskState::Queued);
    assert_eq!(retry.created_at_ms, 20_000);
    assert_eq!(retry.artifact_path, None);
    assert_eq!(handoff.task(&predecessor.id).unwrap(), Some(cancelled));
    assert_eq!(launches.lock().unwrap().len(), 2);
}

#[test]
fn waiting_worker_observes_cancellation_before_current_run_control_or_prompt() {
    let temp = TempDir::new("cancel-waiting-worker");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let mut current_run = FakeCurrentRunControl::new(
        std::iter::repeat_n(CurrentRunObservation::Busy(process), 100)
            .chain([CurrentRunObservation::Exited]),
        10_000,
    );
    current_run.wait_sleep_ms = 10;
    let exit_requests = current_run.graceful_exit_requests.clone();
    let control: Arc<dyn CurrentRunControl> = Arc::new(current_run);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let mut session = exited_session(source_path, cwd);
    session.state = SessionState::Busy;
    let mut handoff = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        control.clone(),
    )
    .unwrap();
    let task = handoff.authorize_session(&session, true, 10_000).unwrap();
    let task_id = task.id.clone();
    let worker = std::thread::spawn(move || {
        HandoffWorker::new_with_current_run_control(paths, executables, control)
            .run(&task_id, 20_000)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let worker_started = loop {
        let observed = handoff.task(&task.id).unwrap().unwrap();
        if observed.worker_process_identity.is_some() || observed.state.is_terminal() {
            break observed.worker_process_identity.is_some();
        }
        assert!(Instant::now() < deadline, "Waiting Worker did not start");
        std::thread::sleep(Duration::from_millis(10));
    };
    let cancelled = handoff.cancel_task(&task.id, 20_000).unwrap();
    let worker_result = worker.join().unwrap();

    assert!(worker_started);
    assert_eq!(cancelled.state, OperationTaskState::Cancelled);
    assert!(worker_result.is_err());
    assert!(exit_requests.lock().unwrap().is_empty());
    assert!(!claude.with_extension("prompt").exists());
}

#[test]
fn interrupted_task_retries_only_after_reconciliation_proves_process_absence() {
    let temp = TempDir::new("retry-interrupted-after-reconcile");
    let source_path = temp.path().join("rollout.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    provider_that_crashes_worker_once_then_returns(&codex, valid_handoff());
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex.clone());
    let launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        temp.path().join("claude-sessions"),
    );
    fs::create_dir(temp.path().join("claude-sessions")).unwrap();
    let mut session = exited_session(source_path, cwd);
    session.provider = Provider::CodexCli;
    session.id = "codex-interrupted-retry".into();
    let mut handoff = HandoffService::open(paths.clone(), executables.clone(), launcher).unwrap();
    let predecessor = handoff.authorize_exited(&session, 10_000).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let state_changed_at_ms = loop {
        let observed = handoff.task(&predecessor.id).unwrap().unwrap();
        if observed.prompt_sent_at_ms.is_some() {
            break observed.state_changed_at_ms;
        }
        assert!(Instant::now() < deadline, "Worker did not send the Prompt");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(handoff
        .retry_task(&predecessor.id, &session, false, 20_000)
        .is_err());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        handoff
            .reconcile_tasks(state_changed_at_ms + 2_000)
            .unwrap();
        if handoff.task(&predecessor.id).unwrap().unwrap().state == OperationTaskState::Interrupted
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "crashed execution did not settle"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    fs::write(codex.with_extension("retry"), "").unwrap();
    drop(handoff);
    let retry_launcher = DetachedWorkerLauncher::new(
        PathBuf::from(env!("CARGO_BIN_EXE_sessionmesh")),
        paths.clone(),
        executables.clone(),
        temp.path().join("claude-sessions"),
    );
    let mut handoff = HandoffService::open(paths, executables, retry_launcher).unwrap();
    let retry = handoff
        .retry_task(&predecessor.id, &session, false, 30_000)
        .unwrap();

    assert_eq!(
        retry.retry_of_task_id.as_deref(),
        Some(predecessor.id.as_str())
    );
    assert_ne!(retry.id, predecessor.id);
    assert_eq!(retry.state, OperationTaskState::Queued);
    let deadline = Instant::now() + Duration::from_secs(5);
    let ready = loop {
        let observed = handoff.task(&retry.id).unwrap().unwrap();
        if observed.state.is_terminal() {
            break observed;
        }
        assert!(Instant::now() < deadline, "Retry Worker did not finish");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        ready.state,
        OperationTaskState::Ready,
        "fresh Retry did not complete: {ready:?}"
    );
    assert!(ready.artifact_path.unwrap().is_file());
    assert_eq!(
        handoff.task(&predecessor.id).unwrap().unwrap().state,
        OperationTaskState::Interrupted
    );
}

#[test]
fn provider_executables_are_resolved_from_explicit_search_directories() {
    let temp = TempDir::new("provider-search");
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let claude = bin.join("claude");
    let codex = bin.join("codex");
    executable(&claude);
    executable(&codex);

    let discovered = ProviderExecutables::discover_in(&[bin]);

    assert_eq!(
        discovered.executable_for(Provider::ClaudeCode),
        Some(claude.as_path())
    );
    assert_eq!(
        discovered.executable_for(Provider::CodexCli),
        Some(codex.as_path())
    );
}

#[test]
fn different_source_sessions_can_coexist_in_the_persistent_task_timeline() {
    let temp = TempDir::new("concurrent-sessions");
    let cwd = temp.path().join("project");
    let first_source = temp.path().join("first.jsonl");
    let second_source = temp.path().join("second.jsonl");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::create_dir(&cwd).unwrap();
    fs::write(&first_source, "{}\n").unwrap();
    fs::write(&second_source, "{}\n").unwrap();
    executable(&claude);
    executable(&codex);
    let mut first = exited_session(first_source, cwd.clone());
    first.id = "first-session".into();
    let mut second = exited_session(second_source, cwd);
    second.id = "second-session".into();
    second.provider = Provider::CodexCli;
    let mut handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        ProviderExecutables::new(claude, codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();

    let first_task = handoff.authorize_exited(&first, 10_000).unwrap();
    let second_task = handoff.authorize_exited(&second, 11_000).unwrap();

    let tasks = handoff.tasks().unwrap();
    assert_eq!(
        tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        vec![second_task.id.as_str(), first_task.id.as_str()]
    );
}

#[test]
fn notification_failure_does_not_change_a_ready_task_or_its_result() {
    let temp = TempDir::new("notification-failed");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();

    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run_with_notifier(&task.id, 20_000, &FailingNotifier)
        .unwrap();

    assert_eq!(ready.state, OperationTaskState::Ready);
    assert_eq!(ready.acknowledged_at_ms, None);
    assert_eq!(
        fs::read_to_string(ready.artifact_path.as_ref().unwrap()).unwrap(),
        valid_handoff()
    );
    assert_eq!(
        handoff.task(&task.id).unwrap().unwrap().acknowledged_at_ms,
        None
    );
    assert_eq!(handoff.tasks().unwrap()[0].acknowledged_at_ms, None);
    drop(handoff);

    let restarted =
        HandoffService::open(paths, executables, RecordingWorkerLauncher::default()).unwrap();
    assert_eq!(
        restarted
            .task(&task.id)
            .unwrap()
            .unwrap()
            .acknowledged_at_ms,
        None
    );
}

#[test]
fn deliberate_acknowledgement_persists_without_changing_ready_state_result_or_ordering_time() {
    let temp = TempDir::new("acknowledge-ready");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task.id, 20_000)
        .unwrap();

    let acknowledged = handoff.acknowledge_task(&task.id, 30_000).unwrap();

    assert_eq!(acknowledged.state, OperationTaskState::Ready);
    assert_eq!(acknowledged.state_changed_at_ms, 20_000);
    assert_eq!(acknowledged.artifact_path, ready.artifact_path);
    assert_eq!(
        acknowledged.successor_launch_plan,
        ready.successor_launch_plan
    );
    assert_eq!(acknowledged.acknowledged_at_ms, Some(30_000));
    drop(handoff);

    let restarted =
        HandoffService::open(paths, executables, RecordingWorkerLauncher::default()).unwrap();
    assert_eq!(
        restarted
            .task(&task.id)
            .unwrap()
            .unwrap()
            .acknowledged_at_ms,
        Some(30_000)
    );
}

#[test]
fn task_timeline_orders_only_by_state_transition_creation_and_stable_task_id() {
    let temp = TempDir::new("task-ordering");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let (oldest, tied_a, tied_b) = {
        let mut authorize = |name: &str, created_at_ms| {
            let source_path = temp.path().join(format!("{name}.jsonl"));
            fs::write(&source_path, "{}\n").unwrap();
            let mut session = exited_session(source_path, cwd.clone());
            session.id = format!("{name}-session");
            handoff.authorize_exited(&session, created_at_ms).unwrap()
        };
        (
            authorize("oldest", 10_000),
            authorize("tied-a", 20_000),
            authorize("tied-b", 20_000),
        )
    };
    let mut tied_ids = [tied_a.id.clone(), tied_b.id.clone()];
    tied_ids.sort_by(|left, right| right.cmp(left));
    assert_eq!(
        handoff
            .tasks()
            .unwrap()
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            tied_ids[0].as_str(),
            tied_ids[1].as_str(),
            oldest.id.as_str()
        ]
    );

    HandoffWorker::new(paths.clone(), executables.clone())
        .run(&oldest.id, 30_000)
        .unwrap();
    handoff.acknowledge_task(&oldest.id, 40_000).unwrap();
    let expected = vec![
        oldest.id.as_str(),
        tied_ids[0].as_str(),
        tied_ids[1].as_str(),
    ];
    assert_eq!(
        handoff
            .tasks()
            .unwrap()
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    drop(handoff);

    let restarted =
        HandoffService::open(paths, executables, RecordingWorkerLauncher::default()).unwrap();
    assert_eq!(
        restarted
            .tasks()
            .unwrap()
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn deleting_a_ready_task_removes_only_its_record_and_owned_artifact() {
    let temp = TempDir::new("delete-ready-task");
    let cwd = temp.path().join("project");
    let source_a = temp.path().join("source-a.jsonl");
    let source_b = temp.path().join("source-b.jsonl");
    let project_file = cwd.join("keep.txt");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::create_dir(&cwd).unwrap();
    fs::write(&source_a, "source a\n").unwrap();
    fs::write(&source_b, "source b\n").unwrap();
    fs::write(&project_file, "project data\n").unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let mut session_a = exited_session(source_a.clone(), cwd.clone());
    session_a.id = "source-a".into();
    let mut session_b = exited_session(source_b.clone(), cwd.clone());
    session_b.id = "source-b".into();
    let task_a = handoff.authorize_exited(&session_a, 10_000).unwrap();
    let task_b = handoff.authorize_exited(&session_b, 11_000).unwrap();
    let ready_a = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task_a.id, 20_000)
        .unwrap();
    let ready_b = HandoffWorker::new(paths.clone(), executables)
        .run(&task_b.id, 21_000)
        .unwrap();
    let artifact_a = ready_a.artifact_path.unwrap();
    let artifact_b = ready_b.artifact_path.unwrap();

    handoff.delete_task(&task_a.id, 30_000).unwrap();

    assert_eq!(handoff.task(&task_a.id).unwrap(), None);
    assert!(!artifact_a.exists());
    assert!(handoff.task(&task_b.id).unwrap().is_some());
    assert!(artifact_b.is_file());
    assert_eq!(fs::read_to_string(source_a).unwrap(), "source a\n");
    assert_eq!(fs::read_to_string(source_b).unwrap(), "source b\n");
    assert_eq!(fs::read_to_string(project_file).unwrap(), "project data\n");
}

#[test]
fn deleting_a_ready_task_treats_its_already_missing_artifact_as_idempotent() {
    let temp = TempDir::new("delete-missing-artifact");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();
    let artifact_path = ready.artifact_path.unwrap();
    fs::remove_file(&artifact_path).unwrap();

    handoff.delete_task(&task.id, 30_000).unwrap();

    assert_eq!(handoff.task(&task.id).unwrap(), None);
    assert!(!artifact_path.exists());
    handoff.delete_task(&task.id, 31_000).unwrap();
}

#[cfg(unix)]
#[test]
fn deletion_rejects_a_symlinked_artifact_root_without_touching_its_target() {
    let temp = TempDir::new("delete-symlinked-root");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();
    let original_root = temp.path().join("original-artifacts");
    fs::rename(&paths.artifact_root, &original_root).unwrap();
    let outside = temp.path().join("outside");
    let outside_task_dir = outside.join(&task.id);
    let outside_file = outside_task_dir.join("handoff.md");
    fs::create_dir_all(&outside_task_dir).unwrap();
    fs::write(&outside_file, "unrelated user data\n").unwrap();
    std::os::unix::fs::symlink(&outside, &paths.artifact_root).unwrap();

    let error = handoff.delete_task(&task.id, 30_000).unwrap_err();

    assert!(error.to_string().contains("Artifact root"));
    assert_eq!(
        fs::read_to_string(&outside_file).unwrap(),
        "unrelated user data\n"
    );
    assert!(handoff.task(&task.id).unwrap().is_some());
    assert!(original_root.join(&task.id).join("handoff.md").is_file());
}

#[cfg(unix)]
#[test]
fn deletion_rejects_a_symlinked_task_artifact_without_touching_its_target() {
    let temp = TempDir::new("delete-symlinked-artifact");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();
    let artifact_path = ready.artifact_path.unwrap();
    let outside_file = temp.path().join("unrelated.md");
    fs::write(&outside_file, "unrelated user data\n").unwrap();
    fs::remove_file(&artifact_path).unwrap();
    std::os::unix::fs::symlink(&outside_file, &artifact_path).unwrap();

    let error = handoff.delete_task(&task.id, 30_000).unwrap_err();

    assert!(error.to_string().contains("Handoff Artifact"));
    assert_eq!(
        fs::read_to_string(&outside_file).unwrap(),
        "unrelated user data\n"
    );
    let retained = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(retained.deletion_requested_at_ms, Some(30_000));
    assert_eq!(
        retained.deletion_failure_code.as_deref(),
        Some("artifact_validation_failed")
    );
    assert!(!retained.offers_retry());
    assert!(fs::symlink_metadata(artifact_path)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn restart_resumes_a_persisted_deletion_after_its_artifact_path_is_repaired() {
    let temp = TempDir::new("resume-task-deletion");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task.id, 20_000)
        .unwrap();
    let artifact_path = ready.artifact_path.unwrap();
    let outside_file = temp.path().join("unrelated.md");
    fs::write(&outside_file, "unrelated user data\n").unwrap();
    fs::remove_file(&artifact_path).unwrap();
    std::os::unix::fs::symlink(&outside_file, &artifact_path).unwrap();
    handoff.delete_task(&task.id, 30_000).unwrap_err();
    drop(handoff);
    fs::remove_file(&artifact_path).unwrap();
    fs::write(&artifact_path, valid_handoff()).unwrap();
    fs::set_permissions(&artifact_path, fs::Permissions::from_mode(0o600)).unwrap();

    let restarted =
        HandoffService::open(paths, executables, RecordingWorkerLauncher::default()).unwrap();
    let remaining = restarted.reconcile_tasks(40_000).unwrap();

    assert!(remaining.iter().all(|retained| retained.id != task.id));
    assert_eq!(restarted.task(&task.id).unwrap(), None);
    assert!(!artifact_path.exists());
    assert_eq!(
        fs::read_to_string(outside_file).unwrap(),
        "unrelated user data\n"
    );
}

#[test]
fn restart_completes_deletion_interrupted_immediately_after_intent_was_persisted() {
    let temp = TempDir::new("resume-after-deletion-intent");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task.id, 20_000)
        .unwrap();
    let artifact_path = ready.artifact_path.unwrap();
    drop(handoff);
    Connection::open(&paths.task_store)
        .unwrap()
        .execute(
            "UPDATE operation_tasks SET deletion_requested_at_ms = ?1 WHERE task_id = ?2",
            rusqlite::params![30_000, task.id],
        )
        .unwrap();

    let restarted =
        HandoffService::open(paths, executables, RecordingWorkerLauncher::default()).unwrap();
    restarted.reconcile_tasks(40_000).unwrap();

    assert_eq!(restarted.task(&task.id).unwrap(), None);
    assert!(!artifact_path.exists());
}

#[test]
fn restart_completes_deletion_after_the_artifact_was_removed_but_the_row_remained() {
    let temp = TempDir::new("resume-after-artifact-removal");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task.id, 20_000)
        .unwrap();
    let artifact_path = ready.artifact_path.unwrap();
    let unexpected = artifact_path.parent().unwrap().join("unexpected.txt");
    fs::write(&unexpected, "do not delete\n").unwrap();

    handoff.delete_task(&task.id, 30_000).unwrap_err();

    assert!(!artifact_path.exists());
    assert_eq!(fs::read_to_string(&unexpected).unwrap(), "do not delete\n");
    let retained = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(retained.deletion_requested_at_ms, Some(30_000));
    assert_eq!(
        retained.deletion_failure_code.as_deref(),
        Some("artifact_remove_failed")
    );
    drop(handoff);
    fs::remove_file(unexpected).unwrap();

    let restarted =
        HandoffService::open(paths, executables, RecordingWorkerLauncher::default()).unwrap();
    restarted.reconcile_tasks(40_000).unwrap();

    assert_eq!(restarted.task(&task.id).unwrap(), None);
}

#[test]
fn task_record_deletion_failure_blocks_retry_and_reconciliation_finishes_later() {
    let temp = TempDir::new("resume-after-task-record-failure");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let session = exited_session(source_path, cwd);
    let task = handoff.authorize_exited(&session, 10_000).unwrap();
    let cancelled = handoff.cancel_task(&task.id, 20_000).unwrap();
    assert_eq!(cancelled.state, OperationTaskState::Cancelled);
    let fault = Connection::open(&paths.task_store).unwrap();
    fault
        .execute_batch(
            "CREATE TRIGGER block_task_deletion BEFORE DELETE ON operation_tasks BEGIN SELECT RAISE(ABORT, 'simulated deletion interruption'); END;",
        )
        .unwrap();

    handoff.delete_task(&task.id, 30_000).unwrap_err();

    let retained = handoff.task(&task.id).unwrap().unwrap();
    assert_eq!(retained.deletion_requested_at_ms, Some(30_000));
    assert_eq!(
        retained.deletion_failure_code.as_deref(),
        Some("task_record_remove_failed")
    );
    assert!(handoff
        .retry_task(&task.id, &session, false, 31_000)
        .unwrap_err()
        .to_string()
        .contains("being deleted"));
    assert_eq!(handoff.tasks().unwrap().len(), 1);
    fault
        .execute_batch("DROP TRIGGER block_task_deletion;")
        .unwrap();

    handoff.reconcile_tasks(40_000).unwrap();

    assert_eq!(handoff.task(&task.id).unwrap(), None);
}

#[cfg(unix)]
#[test]
fn deletion_accepts_every_terminal_operation_task_state() {
    let temp = TempDir::new("delete-terminal-states");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::create_dir(&cwd).unwrap();
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let session = |name: &str| {
        let source_path = temp.path().join(format!("{name}.jsonl"));
        fs::write(&source_path, format!("{name} source\n")).unwrap();
        let mut session = exited_session(source_path, cwd.clone());
        session.id = format!("{name}-session");
        session
    };

    provider_that_returns(&claude, valid_handoff());
    let ready = handoff.authorize_exited(&session("ready"), 10_000).unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&ready.id, 20_000)
        .unwrap();
    assert_eq!(ready.state, OperationTaskState::Ready);

    provider_that_exits(&claude, 1);
    let failed = handoff
        .authorize_exited(&session("failed"), 11_000)
        .unwrap();
    let failed = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&failed.id, 21_000)
        .unwrap();
    assert_eq!(failed.state, OperationTaskState::Failed);

    provider_that_is_interrupted(&claude);
    let interrupted = handoff
        .authorize_exited(&session("interrupted"), 12_000)
        .unwrap();
    let interrupted = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&interrupted.id, 22_000)
        .unwrap();
    assert_eq!(interrupted.state, OperationTaskState::Interrupted);

    let cancelled = handoff
        .authorize_exited(&session("cancelled"), 13_000)
        .unwrap();
    let cancelled = handoff.cancel_task(&cancelled.id, 23_000).unwrap();
    assert_eq!(cancelled.state, OperationTaskState::Cancelled);

    for (index, task) in [ready, failed, interrupted, cancelled]
        .into_iter()
        .enumerate()
    {
        handoff
            .acknowledge_task(&task.id, 24_000 + index as i64)
            .unwrap();
        handoff
            .delete_task(&task.id, 30_000 + index as i64)
            .unwrap();
    }

    assert!(handoff.tasks().unwrap().is_empty());
    for name in ["ready", "failed", "interrupted", "cancelled"] {
        assert_eq!(
            fs::read_to_string(temp.path().join(format!("{name}.jsonl"))).unwrap(),
            format!("{name} source\n")
        );
    }
}

#[test]
fn deletion_refuses_every_non_terminal_operation_task_state_without_recording_intent() {
    let temp = TempDir::new("refuse-delete-non-terminal-states");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::create_dir(&cwd).unwrap();
    provider_that_pauses_then_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let source = |name: &str| {
        let path = temp.path().join(format!("{name}.jsonl"));
        fs::write(&path, "{}\n").unwrap();
        let mut session = exited_session(path, cwd.clone());
        session.id = format!("{name}-session");
        session
    };
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let queued = handoff.authorize_exited(&source("queued"), 10_000).unwrap();

    assert!(handoff.delete_task(&queued.id, 20_000).is_err());
    let retained_queued = handoff.task(&queued.id).unwrap().unwrap();
    assert_eq!(retained_queued.state, OperationTaskState::Queued);
    assert_eq!(retained_queued.deletion_requested_at_ms, None);

    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 9_000,
    };
    let mut open_session = source("waiting");
    open_session.state = SessionState::Busy;
    let mut waiting_service = HandoffService::open_with_current_run_control(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
        Arc::new(FakeCurrentRunControl::new(
            [CurrentRunObservation::Busy(process)],
            10_000,
        )),
    )
    .unwrap();
    let waiting = waiting_service
        .authorize_session(&open_session, true, 11_000)
        .unwrap();

    assert!(waiting_service.delete_task(&waiting.id, 20_001).is_err());
    let retained_waiting = waiting_service.task(&waiting.id).unwrap().unwrap();
    assert_eq!(retained_waiting.state, OperationTaskState::Waiting);
    assert_eq!(retained_waiting.deletion_requested_at_ms, None);

    let running_task = handoff
        .authorize_exited(&source("running"), 12_000)
        .unwrap();
    let worker_paths = paths.clone();
    let worker_executables = executables.clone();
    let running_id = running_task.id.clone();
    let worker = std::thread::spawn(move || {
        HandoffWorker::new(worker_paths, worker_executables)
            .run(&running_id, 22_000)
            .unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let observed = handoff.task(&running_task.id).unwrap().unwrap();
        if observed.state == OperationTaskState::Running {
            break;
        }
        assert!(Instant::now() < deadline, "Task did not become Running");
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(handoff.delete_task(&running_task.id, 20_002).is_err());
    let retained_running = handoff.task(&running_task.id).unwrap().unwrap();
    assert_eq!(retained_running.state, OperationTaskState::Running);
    assert_eq!(retained_running.deletion_requested_at_ms, None);
    assert_eq!(worker.join().unwrap().state, OperationTaskState::Ready);
}

#[test]
fn deleting_a_predecessor_preserves_the_retained_retry_identity_and_reference() {
    let temp = TempDir::new("delete-retry-predecessor");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    executable(&claude);
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let mut handoff = HandoffService::open(
        paths,
        ProviderExecutables::new(claude, codex),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let session = exited_session(source_path, cwd);
    let predecessor = handoff.authorize_exited(&session, 10_000).unwrap();
    let predecessor = handoff.cancel_task(&predecessor.id, 20_000).unwrap();
    let retry = handoff
        .retry_task(&predecessor.id, &session, false, 21_000)
        .unwrap();

    handoff.delete_task(&predecessor.id, 30_000).unwrap();

    assert_eq!(handoff.task(&predecessor.id).unwrap(), None);
    let retained_retry = handoff.task(&retry.id).unwrap().unwrap();
    assert_eq!(retained_retry.id, retry.id);
    assert_eq!(
        retained_retry.retry_of_task_id.as_deref(),
        Some(predecessor.id.as_str())
    );
    assert_eq!(retained_retry.state, OperationTaskState::Queued);
    assert_eq!(handoff.tasks().unwrap().len(), 1);
}

#[test]
fn retained_terminal_tasks_and_artifacts_never_expire_automatically() {
    let temp = TempDir::new("no-task-auto-expiry");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();
    let artifact_path = ready.artifact_path.clone().unwrap();

    handoff
        .reconcile_tasks(20_000 + 100 * 365 * 24 * 60 * 60 * 1_000)
        .unwrap();

    assert_eq!(handoff.task(&task.id).unwrap(), Some(ready));
    assert!(artifact_path.is_file());
}

#[test]
fn deletion_rejects_a_tampered_cross_task_artifact_reference() {
    let temp = TempDir::new("delete-tampered-artifact-reference");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let make_session = |name: &str| {
        let source_path = temp.path().join(format!("{name}.jsonl"));
        fs::write(&source_path, "{}\n").unwrap();
        let mut session = exited_session(source_path, cwd.clone());
        session.id = format!("{name}-session");
        session
    };
    let task_a = handoff
        .authorize_exited(&make_session("a"), 10_000)
        .unwrap();
    let task_b = handoff
        .authorize_exited(&make_session("b"), 11_000)
        .unwrap();
    let ready_a = HandoffWorker::new(paths.clone(), executables.clone())
        .run(&task_a.id, 20_000)
        .unwrap();
    let ready_b = HandoffWorker::new(paths.clone(), executables)
        .run(&task_b.id, 21_000)
        .unwrap();
    let artifact_a = ready_a.artifact_path.unwrap();
    let artifact_b = ready_b.artifact_path.unwrap();
    Connection::open(&paths.task_store)
        .unwrap()
        .execute(
            "UPDATE operation_tasks SET artifact_path = ?1 WHERE task_id = ?2",
            rusqlite::params![artifact_b.to_string_lossy(), task_a.id],
        )
        .unwrap();

    handoff.delete_task(&task_a.id, 30_000).unwrap_err();

    let retained_a = handoff.task(&task_a.id).unwrap().unwrap();
    assert_eq!(
        retained_a.deletion_failure_code.as_deref(),
        Some("artifact_validation_failed")
    );
    assert!(handoff.task(&task_b.id).unwrap().is_some());
    assert!(artifact_a.is_file());
    assert!(artifact_b.is_file());
}

#[cfg(unix)]
#[test]
fn deletion_rejects_a_tampered_task_identity_that_escapes_the_artifact_root() {
    let temp = TempDir::new("delete-escaped-task-identity");
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    fs::create_dir(&paths.artifact_root).unwrap();
    fs::set_permissions(&paths.artifact_root, fs::Permissions::from_mode(0o700)).unwrap();
    let outside = temp.path().join("outside");
    let outside_file = outside.join("handoff.md");
    fs::create_dir(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&outside_file, "unrelated user data\n").unwrap();
    fs::set_permissions(&outside_file, fs::Permissions::from_mode(0o600)).unwrap();
    let handoff = HandoffService::open(
        paths.clone(),
        ProviderExecutables::new(temp.path().join("claude"), temp.path().join("codex")),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let escaped_task_id = "../outside";
    let escaped_artifact = paths.artifact_root.join(escaped_task_id).join("handoff.md");
    Connection::open(&paths.task_store)
        .unwrap()
        .execute(
            r#"
            INSERT INTO operation_tasks (
                task_id, state, source_provider, source_session_id,
                source_record_path, source_working_directory,
                created_at_ms, state_changed_at_ms, artifact_path
            ) VALUES (?1, 'ready', 'claude', 'source-session',
                      '/private/source.jsonl', '/private/project', 1000, 2000, ?2)
            "#,
            rusqlite::params![escaped_task_id, escaped_artifact.to_string_lossy()],
        )
        .unwrap();

    let error = handoff.delete_task(escaped_task_id, 30_000).unwrap_err();

    assert!(error.to_string().contains("Task ID"));
    assert_eq!(
        fs::read_to_string(&outside_file).unwrap(),
        "unrelated user data\n"
    );
    assert!(handoff.task(escaped_task_id).unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn deletion_rejects_an_artifact_that_is_not_owner_only() {
    let temp = TempDir::new("delete-non-private-artifact");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths, executables)
        .run(&task.id, 20_000)
        .unwrap();
    let artifact_path = ready.artifact_path.unwrap();
    fs::set_permissions(&artifact_path, fs::Permissions::from_mode(0o644)).unwrap();

    let error = handoff.delete_task(&task.id, 30_000).unwrap_err();

    assert!(error.to_string().contains("owner-only"));
    assert!(artifact_path.is_file());
    assert!(handoff.task(&task.id).unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn deletion_requires_owner_only_artifact_root_and_task_directory() {
    let temp = TempDir::new("delete-non-private-directories");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    let ready = HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();
    let artifact_path = ready.artifact_path.unwrap();
    let task_directory = artifact_path.parent().unwrap();
    fs::set_permissions(&paths.artifact_root, fs::Permissions::from_mode(0o755)).unwrap();

    let root_error = handoff.delete_task(&task.id, 30_000).unwrap_err();

    assert!(root_error.to_string().contains("Artifact root"));
    assert!(artifact_path.is_file());
    let deleting = handoff.task(&task.id).unwrap().unwrap();
    assert!(sessionmesh::successor::render_successor_command(&deleting)
        .unwrap_err()
        .to_string()
        .contains("being deleted"));
    assert!(handoff
        .acknowledge_task(&task.id, 30_500)
        .unwrap_err()
        .to_string()
        .contains("being deleted"));
    assert_eq!(
        handoff.task(&task.id).unwrap().unwrap().acknowledged_at_ms,
        None
    );
    fs::set_permissions(&paths.artifact_root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(task_directory, fs::Permissions::from_mode(0o755)).unwrap();

    let task_directory_error = handoff.delete_task(&task.id, 31_000).unwrap_err();

    assert!(task_directory_error
        .to_string()
        .contains("Task Artifact directory"));
    assert!(artifact_path.is_file());
    assert!(handoff.task(&task.id).unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn deletion_rejects_a_symlinked_task_artifact_directory() {
    let temp = TempDir::new("delete-symlinked-task-directory");
    let source_path = temp.path().join("source.jsonl");
    let cwd = temp.path().join("project");
    let claude = temp.path().join("claude");
    let codex = temp.path().join("codex");
    fs::write(&source_path, "{}\n").unwrap();
    fs::create_dir(&cwd).unwrap();
    provider_that_returns(&claude, valid_handoff());
    executable(&codex);
    let paths = HandoffPaths {
        task_store: temp.path().join("tasks.db"),
        artifact_root: temp.path().join("artifacts"),
    };
    let executables = ProviderExecutables::new(claude, codex);
    let mut handoff = HandoffService::open(
        paths.clone(),
        executables.clone(),
        RecordingWorkerLauncher::default(),
    )
    .unwrap();
    let task = handoff
        .authorize_exited(&exited_session(source_path, cwd), 10_000)
        .unwrap();
    HandoffWorker::new(paths.clone(), executables)
        .run(&task.id, 20_000)
        .unwrap();
    let task_dir = paths.artifact_root.join(&task.id);
    let original_task_dir = paths.artifact_root.join("parked-task-artifact");
    fs::rename(&task_dir, &original_task_dir).unwrap();
    let outside_task_dir = temp.path().join("outside-task-artifact");
    let outside_file = outside_task_dir.join("handoff.md");
    fs::create_dir(&outside_task_dir).unwrap();
    fs::write(&outside_file, "unrelated user data\n").unwrap();
    std::os::unix::fs::symlink(&outside_task_dir, &task_dir).unwrap();

    let error = handoff.delete_task(&task.id, 30_000).unwrap_err();

    assert!(error.to_string().contains("Task Artifact directory"));
    assert_eq!(
        fs::read_to_string(&outside_file).unwrap(),
        "unrelated user data\n"
    );
    assert!(handoff.task(&task.id).unwrap().is_some());
    assert!(original_task_dir.join("handoff.md").is_file());
}
