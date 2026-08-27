//! Authoritative persistence for user-authorized Operation Tasks.

use crate::adapters::persistence::state::StateDatabase;
use crate::adapters::providers::state::ProcessIdentity;
use crate::domain::Provider;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationTaskState {
    Queued,
    Waiting,
    Running,
    Ready,
    Failed,
    Interrupted,
    Cancelled,
}

impl OperationTaskState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Waiting => "waiting",
            Self::Running => "running",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "waiting" => Ok(Self::Waiting),
            "running" => Ok(Self::Running),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(anyhow!("unknown Operation Task State {value:?}")),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Failed | Self::Interrupted | Self::Cancelled
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Waiting => "Waiting",
            Self::Running => "Running",
            Self::Ready => "Ready",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
            Self::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuccessorLaunchPlan {
    pub successor_provider: Provider,
    pub working_directory: PathBuf,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub artifact_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuccessorLaunchMetadata {
    pub requested_at_ms: Option<i64>,
    pub failed_at_ms: Option<i64>,
    pub failure_code: Option<String>,
    pub process_identity: Option<ProcessIdentity>,
    pub started_at_ms: Option<i64>,
    pub exited_at_ms: Option<i64>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationTask {
    pub id: String,
    pub state: OperationTaskState,
    pub source_provider: Provider,
    pub source_session_id: String,
    pub source_record_path: PathBuf,
    pub source_working_directory: PathBuf,
    pub successor_provider: Provider,
    pub successor_executable: PathBuf,
    pub created_at_ms: i64,
    pub state_changed_at_ms: i64,
    pub worker_process_identity: Option<ProcessIdentity>,
    pub provider_process_identity: Option<ProcessIdentity>,
    pub prompt_sent_at_ms: Option<i64>,
    pub provider_exited_at_ms: Option<i64>,
    pub provider_exit_code: Option<i32>,
    pub running_deadline_ms: Option<i64>,
    pub cancellation_requested_at_ms: Option<i64>,
    pub termination_requested_at_ms: Option<i64>,
    pub retry_of_task_id: Option<String>,
    pub retry_ready_at_ms: Option<i64>,
    pub artifact_path: Option<PathBuf>,
    pub successor_launch_plan: Option<SuccessorLaunchPlan>,
    pub successor_launch_metadata: Option<SuccessorLaunchMetadata>,
    pub successor_session_id: Option<String>,
    pub successor_linked_at_ms: Option<i64>,
    pub failure_code: Option<String>,
    pub acknowledged_at_ms: Option<i64>,
    pub end_current_run_authorized: bool,
    pub source_process_identity: Option<ProcessIdentity>,
    pub waiting_reason: Option<String>,
    pub waiting_deadline_ms: Option<i64>,
    pub control_used_at_ms: Option<i64>,
    pub deletion_requested_at_ms: Option<i64>,
    pub deletion_failure_code: Option<String>,
}

impl OperationTask {
    pub fn is_being_deleted(&self) -> bool {
        self.deletion_requested_at_ms.is_some()
    }

    pub fn offers_deletion(&self) -> bool {
        self.state.is_terminal() && !self.is_being_deleted()
    }

    pub fn offers_retry(&self) -> bool {
        !self.is_being_deleted()
            && (matches!(
                self.state,
                OperationTaskState::Failed | OperationTaskState::Cancelled
            ) || (self.state == OperationTaskState::Interrupted
                && self.retry_ready_at_ms.is_some()))
    }
}

pub struct TaskStore {
    conn: Connection,
}

impl TaskStore {
    pub fn open(path: &Path) -> Result<Self> {
        let database = StateDatabase::open(path)?;
        let conn = database.connect()?;
        Ok(Self { conn })
    }

    pub(crate) fn insert(&self, task: &OperationTask) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO operation_tasks (
                task_id, state, source_provider, source_session_id,
                source_record_path, source_working_directory,
                successor_provider, successor_executable,
                created_at_ms, state_changed_at_ms,
                end_current_run_authorized, source_process_pid,
                source_process_started_at_ms, waiting_reason,
                waiting_deadline_ms, control_used_at_ms, retry_of_task_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            "#,
            params![
                task.id,
                task.state.as_str(),
                task.source_provider.as_str(),
                task.source_session_id,
                task.source_record_path.to_string_lossy(),
                task.source_working_directory.to_string_lossy(),
                task.successor_provider.as_str(),
                task.successor_executable.to_string_lossy(),
                task.created_at_ms,
                task.state_changed_at_ms,
                task.end_current_run_authorized,
                task.source_process_identity.map(|identity| identity.pid),
                task.source_process_identity
                    .map(|identity| identity.started_at_ms),
                task.waiting_reason,
                task.waiting_deadline_ms,
                task.control_used_at_ms,
                task.retry_of_task_id,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, task_id: &str) -> Result<Option<OperationTask>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT task_id, state, source_provider, source_session_id,
                   source_record_path, source_working_directory,
                   created_at_ms, state_changed_at_ms, worker_pid,
                   artifact_path, plan_provider, plan_working_directory,
                   plan_artifact_path, failure_code, acknowledged_at_ms,
                   end_current_run_authorized, source_process_pid,
                   source_process_started_at_ms, waiting_reason,
                   waiting_deadline_ms, control_used_at_ms,
                   worker_started_at_ms, provider_pid,
                   provider_started_at_ms, prompt_sent_at_ms,
                   running_deadline_ms, cancellation_requested_at_ms,
                   termination_requested_at_ms, retry_of_task_id,
                   retry_ready_at_ms, successor_provider,
                   plan_executable, plan_arguments_json,
                   successor_executable,
                   successor_launch_requested_at_ms,
                   successor_launch_failed_at_ms,
                   successor_launch_failure_code,
                   successor_pid, successor_process_started_at_ms,
                   successor_started_at_ms, successor_exited_at_ms,
                   successor_exit_code, successor_session_id,
                   successor_linked_at_ms, deletion_requested_at_ms,
                   deletion_failure_code, provider_exited_at_ms,
                   provider_exit_code
            FROM operation_tasks
            WHERE task_id = ?1
            "#,
        )?;
        let mut rows = statement.query([task_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let provider: String = row.get(2)?;
        let plan_provider: Option<String> = row.get(10)?;
        let plan_working_directory: Option<String> = row.get(11)?;
        let plan_artifact_path: Option<String> = row.get(12)?;
        let plan_executable: Option<String> = row.get(31)?;
        let plan_arguments_json: Option<String> = row.get(32)?;
        let successor_launch_plan = match (
            plan_provider,
            plan_working_directory,
            plan_executable,
            plan_arguments_json,
            plan_artifact_path,
        ) {
            (
                Some(provider),
                Some(working_directory),
                Some(executable),
                Some(arguments_json),
                Some(artifact_path),
            ) => Some(SuccessorLaunchPlan {
                successor_provider: Provider::parse(&provider)
                    .ok_or_else(|| anyhow!("unknown Successor Provider {provider:?}"))?,
                working_directory: PathBuf::from(working_directory),
                executable: PathBuf::from(executable),
                arguments: serde_json::from_str(&arguments_json)
                    .map_err(|_| anyhow!("invalid persisted Successor arguments"))?,
                artifact_path: PathBuf::from(artifact_path),
            }),
            (Some(provider), Some(working_directory), None, None, Some(artifact_path)) => {
                Some(SuccessorLaunchPlan {
                    successor_provider: Provider::parse(&provider)
                        .ok_or_else(|| anyhow!("unknown Successor Provider {provider:?}"))?,
                    working_directory: PathBuf::from(working_directory),
                    executable: PathBuf::new(),
                    arguments: Vec::new(),
                    artifact_path: PathBuf::from(artifact_path),
                })
            }
            (None, None, None, None, None) => None,
            _ => return Err(anyhow!("incomplete persisted Successor Launch Plan")),
        };
        let successor_provider = match row.get::<_, Option<String>>(30)? {
            Some(selected) => Provider::parse(&selected)
                .ok_or_else(|| anyhow!("unknown selected Successor Provider {selected:?}"))?,
            None => Provider::parse(&provider).expect("source Provider was validated"),
        };
        let successor_launch_requested_at_ms: Option<i64> = row.get(34)?;
        let successor_launch_failed_at_ms: Option<i64> = row.get(35)?;
        let successor_launch_failure_code: Option<String> = row.get(36)?;
        let successor_pid = row.get::<_, Option<u32>>(37)?;
        let successor_process_started_at_ms = row.get::<_, Option<i64>>(38)?;
        let successor_started_at_ms: Option<i64> = row.get(39)?;
        let successor_exited_at_ms: Option<i64> = row.get(40)?;
        let successor_exit_code: Option<i32> = row.get(41)?;
        let successor_session_id: Option<String> = row.get(42)?;
        let successor_linked_at_ms: Option<i64> = row.get(43)?;
        let deletion_requested_at_ms: Option<i64> = row.get(44)?;
        let deletion_failure_code: Option<String> = row.get(45)?;
        let has_successor_launch_metadata = successor_launch_requested_at_ms.is_some()
            || successor_launch_failed_at_ms.is_some()
            || successor_launch_failure_code.is_some()
            || successor_pid.is_some()
            || successor_process_started_at_ms.is_some()
            || successor_started_at_ms.is_some()
            || successor_exited_at_ms.is_some()
            || successor_exit_code.is_some();
        let successor_launch_metadata =
            has_successor_launch_metadata.then(|| SuccessorLaunchMetadata {
                requested_at_ms: successor_launch_requested_at_ms,
                failed_at_ms: successor_launch_failed_at_ms,
                failure_code: successor_launch_failure_code,
                process_identity: successor_pid
                    .zip(successor_process_started_at_ms)
                    .map(|(pid, started_at_ms)| ProcessIdentity { pid, started_at_ms }),
                started_at_ms: successor_started_at_ms,
                exited_at_ms: successor_exited_at_ms,
                exit_code: successor_exit_code,
            });
        Ok(Some(OperationTask {
            id: row.get(0)?,
            state: OperationTaskState::from_str(&row.get::<_, String>(1)?)?,
            source_provider: Provider::parse(&provider)
                .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
            source_session_id: row.get(3)?,
            source_record_path: PathBuf::from(row.get::<_, String>(4)?),
            source_working_directory: PathBuf::from(row.get::<_, String>(5)?),
            successor_provider,
            successor_executable: row
                .get::<_, Option<String>>(33)?
                .map(PathBuf::from)
                .unwrap_or_default(),
            created_at_ms: row.get(6)?,
            state_changed_at_ms: row.get(7)?,
            worker_process_identity: row
                .get::<_, Option<u32>>(8)?
                .zip(row.get::<_, Option<i64>>(21)?)
                .map(|(pid, started_at_ms)| ProcessIdentity { pid, started_at_ms }),
            provider_process_identity: row
                .get::<_, Option<u32>>(22)?
                .zip(row.get::<_, Option<i64>>(23)?)
                .map(|(pid, started_at_ms)| ProcessIdentity { pid, started_at_ms }),
            prompt_sent_at_ms: row.get(24)?,
            provider_exited_at_ms: row.get(46)?,
            provider_exit_code: row.get(47)?,
            running_deadline_ms: row.get(25)?,
            cancellation_requested_at_ms: row.get(26)?,
            termination_requested_at_ms: row.get(27)?,
            retry_of_task_id: row.get(28)?,
            retry_ready_at_ms: row.get(29)?,
            artifact_path: row.get::<_, Option<String>>(9)?.map(PathBuf::from),
            successor_launch_plan,
            successor_launch_metadata,
            successor_session_id,
            successor_linked_at_ms,
            failure_code: row.get(13)?,
            acknowledged_at_ms: row.get(14)?,
            end_current_run_authorized: row.get(15)?,
            source_process_identity: row
                .get::<_, Option<u32>>(16)?
                .zip(row.get::<_, Option<i64>>(17)?)
                .map(|(pid, started_at_ms)| ProcessIdentity { pid, started_at_ms }),
            waiting_reason: row.get(18)?,
            waiting_deadline_ms: row.get(19)?,
            control_used_at_ms: row.get(20)?,
            deletion_requested_at_ms,
            deletion_failure_code,
        }))
    }

    pub fn list(&self) -> Result<Vec<OperationTask>> {
        let task_ids = {
            let mut statement = self.conn.prepare(
                r#"
                SELECT task_id
                FROM operation_tasks
                ORDER BY state_changed_at_ms DESC, created_at_ms DESC, task_id DESC
                "#,
            )?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };
        task_ids
            .iter()
            .map(|task_id| {
                self.get(task_id)?
                    .ok_or_else(|| anyhow!("Operation Task {task_id} disappeared"))
            })
            .collect()
    }

    pub(crate) fn mark_running(
        &self,
        task_id: &str,
        now_ms: i64,
        worker: ProcessIdentity,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET state = 'running', state_changed_at_ms = ?2,
                worker_pid = ?3, worker_started_at_ms = ?4,
                running_deadline_ms = ?5
            WHERE task_id = ?1 AND state IN ('queued', 'waiting')
            "#,
            params![
                task_id,
                now_ms,
                worker.pid,
                worker.started_at_ms,
                now_ms.saturating_add(30 * 60 * 1_000),
            ],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not Queued or Waiting"));
        }
        Ok(())
    }

    pub(crate) fn mark_waiting_worker_started(
        &self,
        task_id: &str,
        worker: ProcessIdentity,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET worker_pid = ?2, worker_started_at_ms = ?3
            WHERE task_id = ?1 AND state = 'waiting' AND worker_pid IS NULL
            "#,
            params![task_id, worker.pid, worker.started_at_ms],
        )?;
        if changed != 1 {
            return Err(anyhow!("Waiting Task cannot record Worker startup"));
        }
        Ok(())
    }

    pub(crate) fn mark_control_used(
        &self,
        task_id: &str,
        now_ms: i64,
        worker: ProcessIdentity,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET state = 'running',
                state_changed_at_ms = ?2,
                worker_pid = ?3,
                worker_started_at_ms = ?4,
                running_deadline_ms = ?5,
                control_used_at_ms = ?2
            WHERE task_id = ?1
              AND state = 'waiting'
              AND end_current_run_authorized = 1
              AND control_used_at_ms IS NULL
            "#,
            params![
                task_id,
                now_ms,
                worker.pid,
                worker.started_at_ms,
                now_ms.saturating_add(30 * 60 * 1_000),
            ],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task has no unused Current Run authority"));
        }
        Ok(())
    }

    pub(crate) fn mark_provider_started(
        &self,
        task_id: &str,
        provider: ProcessIdentity,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET provider_pid = ?2, provider_started_at_ms = ?3
            WHERE task_id = ?1 AND state = 'running' AND provider_pid IS NULL
            "#,
            params![task_id, provider.pid, provider.started_at_ms],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task cannot record Provider startup"));
        }
        Ok(())
    }

    pub(crate) fn mark_prompt_sent(&self, task_id: &str, now_ms: i64) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET prompt_sent_at_ms = ?2
            WHERE task_id = ?1 AND state = 'running'
              AND provider_pid IS NOT NULL AND prompt_sent_at_ms IS NULL
            "#,
            params![task_id, now_ms],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task cannot record Prompt evidence"));
        }
        Ok(())
    }

    pub(crate) fn mark_provider_exited(
        &self,
        task_id: &str,
        now_ms: i64,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET provider_exited_at_ms = ?2, provider_exit_code = ?3
            WHERE task_id = ?1 AND state = 'running'
              AND provider_pid IS NOT NULL AND provider_exited_at_ms IS NULL
            "#,
            params![task_id, now_ms, exit_code],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task cannot record Provider exit"));
        }
        Ok(())
    }

    pub(crate) fn request_cancellation(&self, task_id: &str, now_ms: i64) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET cancellation_requested_at_ms = COALESCE(cancellation_requested_at_ms, ?2)
            WHERE task_id = ?1 AND state IN ('queued', 'waiting', 'running')
            "#,
            params![task_id, now_ms],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not non-terminal"));
        }
        Ok(())
    }

    pub(crate) fn mark_termination_requested(&self, task_id: &str, now_ms: i64) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET termination_requested_at_ms = COALESCE(termination_requested_at_ms, ?2)
            WHERE task_id = ?1 AND state = 'running'
              AND provider_pid IS NOT NULL
            "#,
            params![task_id, now_ms],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task cannot request Provider termination"));
        }
        Ok(())
    }

    pub(crate) fn mark_cancelled(
        &self,
        task_id: &str,
        now_ms: i64,
        failure_code: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET state = 'cancelled', state_changed_at_ms = ?2, failure_code = ?3
            WHERE task_id = ?1 AND state IN ('queued', 'waiting', 'running')
            "#,
            params![task_id, now_ms, failure_code],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not non-terminal"));
        }
        Ok(())
    }

    pub(crate) fn mark_retry_ready(&self, task_id: &str, now_ms: i64) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET retry_ready_at_ms = COALESCE(retry_ready_at_ms, ?2)
            WHERE task_id = ?1 AND state = 'interrupted'
            "#,
            params![task_id, now_ms],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not Interrupted"));
        }
        Ok(())
    }

    pub(crate) fn mark_ready(
        &self,
        task_id: &str,
        now_ms: i64,
        artifact_path: &Path,
        plan: &SuccessorLaunchPlan,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET state = 'ready',
                state_changed_at_ms = ?2,
                artifact_path = ?3,
                plan_provider = ?4,
                plan_working_directory = ?5,
                plan_executable = ?6,
                plan_arguments_json = ?7,
                plan_artifact_path = ?8,
                failure_code = NULL
            WHERE task_id = ?1 AND state = 'running'
            "#,
            params![
                task_id,
                now_ms,
                artifact_path.to_string_lossy(),
                plan.successor_provider.as_str(),
                plan.working_directory.to_string_lossy(),
                plan.executable.to_string_lossy(),
                serde_json::to_string(&plan.arguments)?,
                plan.artifact_path.to_string_lossy(),
            ],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not Running"));
        }
        Ok(())
    }

    pub(crate) fn record_terminal_launch_succeeded(
        &self,
        task_id: &str,
        now_ms: i64,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET successor_launch_requested_at_ms = ?2,
                successor_launch_failed_at_ms = NULL,
                successor_launch_failure_code = NULL
            WHERE task_id = ?1 AND state = 'ready'
              AND deletion_requested_at_ms IS NULL
            "#,
            params![task_id, now_ms],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not Ready"));
        }
        Ok(())
    }

    pub(crate) fn record_terminal_launch_failed(
        &self,
        task_id: &str,
        now_ms: i64,
        failure_code: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET successor_launch_failed_at_ms = ?2,
                successor_launch_failure_code = ?3
            WHERE task_id = ?1 AND state = 'ready'
              AND deletion_requested_at_ms IS NULL
            "#,
            params![task_id, now_ms, failure_code],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not Ready"));
        }
        Ok(())
    }

    pub(crate) fn record_successor_started(
        &self,
        task_id: &str,
        now_ms: i64,
        process: ProcessIdentity,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET successor_pid = ?3,
                successor_process_started_at_ms = ?4,
                successor_started_at_ms = ?2,
                successor_exited_at_ms = NULL,
                successor_exit_code = NULL,
                successor_launch_failure_code = NULL
            WHERE task_id = ?1 AND state = 'ready'
              AND deletion_requested_at_ms IS NULL
            "#,
            params![task_id, now_ms, process.pid, process.started_at_ms],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not Ready"));
        }
        Ok(())
    }

    pub(crate) fn record_successor_exited(
        &self,
        task_id: &str,
        now_ms: i64,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET successor_exited_at_ms = ?2,
                successor_exit_code = ?3
            WHERE task_id = ?1 AND state = 'ready'
              AND deletion_requested_at_ms IS NULL
            "#,
            params![task_id, now_ms, exit_code],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not Ready"));
        }
        Ok(())
    }

    pub(crate) fn record_successor_linked(
        &self,
        task_id: &str,
        now_ms: i64,
        process: ProcessIdentity,
        session_id: &str,
    ) -> Result<()> {
        if session_id.is_empty() {
            return Err(anyhow!("Successor Session ID is empty"));
        }
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET successor_session_id = ?5,
                successor_linked_at_ms = ?2
            WHERE task_id = ?1 AND state = 'ready'
              AND deletion_requested_at_ms IS NULL
              AND successor_pid = ?3
              AND successor_process_started_at_ms = ?4
              AND successor_session_id IS NULL
              AND NOT (
                  source_provider = successor_provider
                  AND source_session_id = ?5
              )
            "#,
            params![
                task_id,
                now_ms,
                process.pid,
                process.started_at_ms,
                session_id
            ],
        )?;
        if changed != 1 {
            return Err(anyhow!(
                "Task cannot link the declared Successor process and Session"
            ));
        }
        Ok(())
    }

    pub(crate) fn request_deletion(&self, task_id: &str, now_ms: i64) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET deletion_requested_at_ms = COALESCE(deletion_requested_at_ms, ?2),
                deletion_failure_code = NULL
            WHERE task_id = ?1
              AND state IN ('ready', 'failed', 'interrupted', 'cancelled')
            "#,
            params![task_id, now_ms],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not terminal"));
        }
        Ok(())
    }

    pub(crate) fn record_deletion_failure(&self, task_id: &str, failure_code: &str) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET deletion_failure_code = ?2
            WHERE task_id = ?1 AND deletion_requested_at_ms IS NOT NULL
            "#,
            params![task_id, failure_code],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task deletion was not requested"));
        }
        Ok(())
    }

    pub(crate) fn delete_terminal(&self, task_id: &str) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            DELETE FROM operation_tasks
            WHERE task_id = ?1
              AND state IN ('ready', 'failed', 'interrupted', 'cancelled')
              AND deletion_requested_at_ms IS NOT NULL
            "#,
            [task_id],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not terminal"));
        }
        Ok(())
    }

    pub(crate) fn mark_failed(&self, task_id: &str, now_ms: i64, failure_code: &str) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET state = 'failed',
                state_changed_at_ms = ?2,
                failure_code = ?3
            WHERE task_id = ?1 AND state IN ('queued', 'waiting', 'running')
            "#,
            params![task_id, now_ms, failure_code],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not non-terminal"));
        }
        Ok(())
    }

    pub(crate) fn mark_interrupted(
        &self,
        task_id: &str,
        now_ms: i64,
        failure_code: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET state = 'interrupted',
                state_changed_at_ms = ?2,
                failure_code = ?3
            WHERE task_id = ?1 AND state IN ('queued', 'waiting', 'running')
            "#,
            params![task_id, now_ms, failure_code],
        )?;
        if changed != 1 {
            return Err(anyhow!("Task is not non-terminal"));
        }
        Ok(())
    }

    pub(crate) fn cancel_expired_waiting(&self, task_id: &str, now_ms: i64) -> Result<bool> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET state = 'cancelled',
                state_changed_at_ms = ?2,
                failure_code = 'current_run_authority_expired'
            WHERE task_id = ?1
              AND state = 'waiting'
              AND control_used_at_ms IS NULL
              AND waiting_deadline_ms IS NOT NULL
              AND waiting_deadline_ms <= ?2
            "#,
            params![task_id, now_ms],
        )?;
        Ok(changed == 1)
    }

    pub(crate) fn cancel_before_execution(&self, task_id: &str, now_ms: i64) -> Result<bool> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET state = 'cancelled',
                state_changed_at_ms = ?2,
                failure_code = 'cancelled_by_user'
            WHERE task_id = ?1
              AND state IN ('queued', 'waiting')
              AND worker_pid IS NULL
              AND control_used_at_ms IS NULL
            "#,
            params![task_id, now_ms],
        )?;
        Ok(changed == 1)
    }

    pub(crate) fn has_non_terminal(
        &self,
        provider: Provider,
        source_session_id: &str,
    ) -> Result<bool> {
        Ok(self.conn.query_row(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM operation_tasks
                WHERE source_provider = ?1
                  AND source_session_id = ?2
                  AND state IN ('queued', 'waiting', 'running')
            )
            "#,
            params![provider.as_str(), source_session_id],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn acknowledge(&self, task_id: &str, now_ms: i64) -> Result<OperationTask> {
        let changed = self.conn.execute(
            r#"
            UPDATE operation_tasks
            SET acknowledged_at_ms = COALESCE(acknowledged_at_ms, ?2)
            WHERE task_id = ?1
              AND state IN ('ready', 'failed', 'interrupted', 'cancelled')
              AND deletion_requested_at_ms IS NULL
            "#,
            params![task_id, now_ms],
        )?;
        if changed != 1 {
            return match self.get(task_id)? {
                Some(task) if task.deletion_requested_at_ms.is_some() => {
                    Err(anyhow!("Operation Task is being deleted"))
                }
                Some(_) => Err(anyhow!("Task is not terminal")),
                None => Err(anyhow!("Operation Task {task_id} was not found")),
            };
        }
        self.get(task_id)?
            .ok_or_else(|| anyhow!("Operation Task {task_id} was not found"))
    }
}
