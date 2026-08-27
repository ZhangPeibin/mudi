//! Explicit authorization entry points for Assisted Handoff.

mod application;
pub mod artifact;
mod contract;
mod ports;

pub use crate::application::{ProviderExecutableResolver, ProviderExecutables};
pub use application::HandoffService;
pub use contract::{
    CompletionNotice, CurrentRunObservation, HandoffPaths, HandoffStartError, WorkerLaunch,
};
pub use ports::{
    CompletionNotifier, CurrentRunControl, SkillInstaller, TaskIdGenerator, WorkerLauncher,
};

use crate::application::successor::SuccessorRuntime;
use crate::application::tasks::{OperationTask, OperationTaskState, TaskRepository};
use crate::domain::{Provider, Session, SessionState};
use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
