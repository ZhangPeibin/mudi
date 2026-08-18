//! Handoff application and Worker data contracts.

use crate::adapters::providers::state::ProcessIdentity;
use crate::application::tasks::OperationTaskState;
use crate::domain::Provider;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffPaths {
    pub task_store: PathBuf,
    pub artifact_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLaunch {
    pub task_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentRunObservation {
    Busy(ProcessIdentity),
    Idle(ProcessIdentity),
    Exited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionNotice {
    pub task_id: String,
    pub state: OperationTaskState,
    pub source_provider: Provider,
    pub source_session_id: String,
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
