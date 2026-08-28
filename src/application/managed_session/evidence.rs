//! Shared process-evidence port for Managed workflows.

use crate::domain::ProcessIdentity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProcessObservation {
    Live(ProcessIdentity),
    Absent,
    Unknown,
}

pub trait ProcessEvidence: Send + Sync {
    fn observe(&self, pid: u32) -> ProviderProcessObservation;

    fn observe_many(&self, pids: &[u32]) -> Vec<ProviderProcessObservation> {
        pids.iter().map(|pid| self.observe(*pid)).collect()
    }
}
