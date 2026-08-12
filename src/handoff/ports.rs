//! Replaceable boundaries used by Handoff application and Worker policy.

use super::{CompletionNotice, CurrentRunObservation, WorkerLaunch};
use crate::domain::Provider;
use crate::providers::state::ProcessIdentity;
use anyhow::Result;
use std::path::Path;

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

    fn now_ms(&self) -> i64;

    fn wait_before_recheck(&self);
}

pub trait CompletionNotifier {
    fn notify(&self, notice: &CompletionNotice) -> std::io::Result<()>;
}
