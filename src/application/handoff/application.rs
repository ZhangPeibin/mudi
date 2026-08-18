//! Handoff authorization, retry, cancellation, and reconciliation policy.

use super::*;

pub struct HandoffService<L> {
    paths: HandoffPaths,
    executables: ProviderExecutables,
    launcher: L,
    store: TaskStore,
    current_run_control: Arc<dyn CurrentRunControl>,
    skill_installer: Option<Arc<dyn SkillInstaller>>,
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
            skill_installer: None,
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
            skill_installer: None,
        })
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
                .executable_for(successor_provider)
                .expect("preflight validates the Successor Provider executable")
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
                .executable_for(successor_provider)
                .expect("preflight validates the Successor Provider executable")
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
        if !crate::adapters::providers::state::process_identity_is_live(provider) {
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
                    !crate::adapters::providers::state::process_identity_is_live(process)
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
                if crate::adapters::providers::state::process_identity_is_live(provider) {
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
                    .is_some_and(crate::adapters::providers::state::process_identity_is_live);
                let provider_live = task
                    .provider_process_identity
                    .is_some_and(crate::adapters::providers::state::process_identity_is_live);
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
        if self.executables.executable_for(session.provider).is_none() {
            return Err(HandoffStartError::Ineligible(
                "source Provider CLI is unavailable",
            ));
        }
        if self
            .executables
            .executable_for(successor_provider)
            .is_none()
        {
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
        if let Some(skill_installer) = self.skill_installer.as_ref() {
            skill_installer
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
    let worker_absent = task.worker_process_identity.is_some_and(|process| {
        !crate::adapters::providers::state::process_identity_is_live(process)
    });
    let provider_absent = match task.provider_process_identity {
        Some(process) => !crate::adapters::providers::state::process_identity_is_live(process),
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
    if crate::application::handoff::artifact::validate(&candidate).is_err() {
        return Ok(false);
    }
    let plan = crate::application::successor::build_launch_plan(task, artifact_path.clone())?;
    store.mark_ready(&task.id, now_ms, &artifact_path, &plan)?;
    Ok(true)
}
