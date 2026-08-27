//! Operation Task contracts and the application-owned repository port.

use crate::domain::{ProcessIdentity, Provider};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::str::FromStr;

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
    pub fn as_str(self) -> &'static str {
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

impl FromStr for OperationTaskState {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "waiting" => Ok(Self::Waiting),
            "running" => Ok(Self::Running),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "cancelled" => Ok(Self::Cancelled),
            _ => anyhow::bail!("unknown Operation Task State {value:?}"),
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

/// Authoritative Operation Task operations consumed by application workflows.
pub trait TaskRepository: Send {
    fn insert(&self, task: &OperationTask) -> Result<()>;
    fn get(&self, task_id: &str) -> Result<Option<OperationTask>>;
    fn list(&self) -> Result<Vec<OperationTask>>;
    fn request_cancellation(&self, task_id: &str, now_ms: i64) -> Result<()>;
    fn mark_termination_requested(&self, task_id: &str, now_ms: i64) -> Result<()>;
    fn mark_cancelled(&self, task_id: &str, now_ms: i64, failure_code: &str) -> Result<()>;
    fn mark_retry_ready(&self, task_id: &str, now_ms: i64) -> Result<()>;
    fn mark_ready(
        &self,
        task_id: &str,
        now_ms: i64,
        artifact_path: &Path,
        plan: &SuccessorLaunchPlan,
    ) -> Result<()>;
    fn request_deletion(&self, task_id: &str, now_ms: i64) -> Result<()>;
    fn record_deletion_failure(&self, task_id: &str, failure_code: &str) -> Result<()>;
    fn delete_terminal(&self, task_id: &str) -> Result<()>;
    fn mark_failed(&self, task_id: &str, now_ms: i64, failure_code: &str) -> Result<()>;
    fn mark_interrupted(&self, task_id: &str, now_ms: i64, failure_code: &str) -> Result<()>;
    fn cancel_before_execution(&self, task_id: &str, now_ms: i64) -> Result<bool>;
    fn has_non_terminal(&self, provider: Provider, source_session_id: &str) -> Result<bool>;
    fn acknowledge(&self, task_id: &str, now_ms: i64) -> Result<OperationTask>;
}
