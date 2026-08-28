//! macOS process identity inspection for Managed workflow evidence.

use crate::adapters::providers::state;
use crate::application::managed_session::{ProcessEvidence, ProviderProcessObservation};

pub struct SystemProcessEvidence;

impl SystemProcessEvidence {
    pub fn current_identity(&self, pid: u32) -> Option<crate::domain::ProcessIdentity> {
        match self.observe(pid) {
            ProviderProcessObservation::Live(identity) => Some(identity),
            ProviderProcessObservation::Absent | ProviderProcessObservation::Unknown => None,
        }
    }
}

impl ProcessEvidence for SystemProcessEvidence {
    fn observe(&self, pid: u32) -> ProviderProcessObservation {
        process_observation(state::inspect_process_identity(pid))
    }

    fn observe_many(&self, pids: &[u32]) -> Vec<ProviderProcessObservation> {
        state::inspect_process_identities(pids)
            .into_iter()
            .map(process_observation)
            .collect()
    }
}

fn process_observation(inspection: state::ProcessInspection) -> ProviderProcessObservation {
    match inspection {
        state::ProcessInspection::Present(identity, false) => {
            ProviderProcessObservation::Live(identity)
        }
        state::ProcessInspection::Present(_, true) | state::ProcessInspection::Absent => {
            ProviderProcessObservation::Absent
        }
        state::ProcessInspection::Unknown => ProviderProcessObservation::Unknown,
    }
}
