use crate::application::managed_sessions::{BindingProcessObservation, ProviderBindingEvidence};
use crate::application::observation::read_model::{
    ManagedCurrentRunSummary, ManagedResumeSummary, RefreshDiagnostic, ScanStats,
};
use crate::domain::Session;

/// One immutable, normalized sample of Provider sources and process-backed eligibility.
///
/// Downstream stages receive this type by shared reference. It intentionally excludes raw
/// Provider records, prompt bodies, conversation content, and tool data.
#[derive(Debug, Clone)]
pub struct ObservationFrame {
    pub observed_at_ms: i64,
    pub stats: ScanStats,
    pub sessions: Vec<Session>,
    pub managed_bindings: Vec<ManagedBindingObservation>,
    pub verified_managed_current_runs: Vec<ManagedCurrentRunSummary>,
    pub resumable_managed_sessions: Vec<ManagedResumeSummary>,
    pub diagnostics: Vec<RefreshDiagnostic>,
}

impl ObservationFrame {
    pub fn new(observed_at_ms: i64, batch: super::SessionObservationBatch) -> Self {
        Self {
            observed_at_ms,
            stats: batch.stats,
            sessions: batch.sessions,
            managed_bindings: Vec::new(),
            verified_managed_current_runs: Vec::new(),
            resumable_managed_sessions: Vec::new(),
            diagnostics: batch.diagnostics,
        }
    }
}

/// Provider-normalized Binding evidence sampled for one existing authorization record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedBindingObservation {
    pub registration_id: String,
    pub evidence: Option<ProviderBindingEvidence>,
    pub processes: BindingProcessObservation,
}
