//! Detached Handoff Worker lifecycle and provider execution.

use super::*;

pub struct HandoffWorker {
    paths: HandoffPaths,
    executables: ProviderExecutables,
    current_run_control: Arc<dyn CurrentRunControl>,
    skill_installer: Option<Arc<dyn SkillInstaller>>,
}

impl HandoffWorker {
    pub fn new(paths: HandoffPaths, executables: ProviderExecutables) -> Self {
        Self {
            paths,
            executables,
            current_run_control: Arc::new(NoopCurrentRunControl),
            skill_installer: None,
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
            skill_installer: None,
        }
    }

    pub fn with_managed_handoff_skills(
        mut self,
        managed_handoff_skills: crate::handoff_skill::ManagedHandoffSkills,
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

        if let Some(skill_installer) = self.skill_installer.as_ref() {
            if let Err(failure_code) = skill_installer.verify(task.source_provider) {
                store.mark_failed(task_id, transition_now_ms(), &failure_code)?;
                return completed_task(&store, task_id, notifier);
            }
        }

        if store
            .get(task_id)?
            .is_some_and(|task| task.state.is_terminal())
        {
            return Err(anyhow!("Task became terminal before Provider startup"));
        }

        let Some(source_executable) = self.executables.executable_for(task.source_provider) else {
            store.mark_failed(task_id, transition_now_ms(), "source_provider_unavailable")?;
            return completed_task(&store, task_id, notifier);
        };
        let candidate =
            match run_source_provider(source_executable, &task, &store, transition_now_ms()) {
                Ok(candidate) => candidate,
                Err(ProviderRunError::Failed(candidate)) => {
                    if let Some(candidate) = candidate
                        .filter(|candidate| crate::handoff_artifact::validate(candidate).is_ok())
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
                    store.mark_failed(
                        task_id,
                        transition_now_ms(),
                        "provider_invocation_failed",
                    )?;
                    return completed_task(&store, task_id, notifier);
                }
                Err(ProviderRunError::Interrupted(candidate)) => {
                    if let Some(candidate) = candidate
                        .filter(|candidate| crate::handoff_artifact::validate(candidate).is_ok())
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
        if crate::handoff_artifact::validate(&candidate).is_err() {
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
    let artifact_path = match crate::handoff_artifact::publish(
        &paths.artifact_root,
        &task.id,
        candidate.as_bytes(),
    ) {
        Ok(path) => path,
        Err(_) => {
            store.mark_failed(&task.id, now_ms, "artifact_publish_failed")?;
            return completed_task(store, &task.id, notifier);
        }
    };
    let plan = match crate::successor::build_launch_plan(task, artifact_path.clone()) {
        Ok(plan) => plan,
        Err(_) => {
            store.mark_failed(&task.id, now_ms, "successor_launch_plan_invalid")?;
            return completed_task(store, &task.id, notifier);
        }
    };
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

pub(super) fn request_graceful_process_exit(process: ProcessIdentity) -> Result<()> {
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
    let runtime = crate::providers::registry()
        .runtime_control(task.source_provider)
        .map_err(|_| ProviderRunError::Failed(None))?;
    command.args(runtime.handoff_resume_args(&task.source_session_id));
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
    let invocation = crate::providers::registry()
        .skill_transport(task.source_provider)
        .map_err(|_| ProviderRunError::Failed(None))?
        .handoff_skill_invocation();
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

pub(super) fn new_task_id() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
    format!("{time:032x}-{:08x}-{sequence:016x}", std::process::id())
}
