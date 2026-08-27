//! Replaceable boundaries used by Handoff application and Worker policy.

use super::{CompletionNotice, CurrentRunObservation, WorkerLaunch};
use crate::domain::{ProcessIdentity, Provider};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub trait WorkerLauncher {
    fn launch(&self, launch: &WorkerLaunch) -> std::io::Result<()>;
}

pub trait CurrentRunControl: Send + Sync {
    fn observe(
        &self,
        provider: Provider,
        source_session_id: &str,
        source_record_path: &Path,
    ) -> Result<CurrentRunObservation>;

    fn request_graceful_exit(&self, process: ProcessIdentity) -> Result<()>;

    fn process_is_live(&self, process: ProcessIdentity) -> bool;

    fn now_ms(&self) -> i64;

    fn wait_before_recheck(&self);
}

pub trait CompletionNotifier {
    fn notify(&self, notice: &CompletionNotice) -> std::io::Result<()>;
}

pub trait TaskIdGenerator: Send + Sync {
    fn new_task_id(&self) -> String;
}

/// Installs or verifies the declared managed Handoff Skill.
pub trait SkillInstaller: Send + Sync + std::fmt::Debug {
    fn install_for_first_use(&self) -> Result<()>;
    fn verify(&self, provider: Provider) -> std::result::Result<(), String>;
    fn invocation(&self, provider: Provider) -> std::result::Result<String, String>;
    fn worker_roots(&self) -> Option<(PathBuf, PathBuf)>;
}
