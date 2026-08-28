//! Conclusive Provider and process evidence used to establish Managed authority.

use crate::domain::{ProcessIdentity, Provider, SourceMetadata};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBindingEvidence {
    pub provider: Provider,
    pub source_session_id: String,
    pub process_session_id: String,
    pub source: SourceMetadata,
    pub working_directory: PathBuf,
    pub launch_process: ProcessIdentity,
    pub provider_process: ProcessIdentity,
}

/// Conclusive record that atomically established a Managed Session and Current Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingReceipt {
    pub id: String,
    pub registration_id: String,
    pub provider: Provider,
    pub native_session_id: String,
    pub source: SourceMetadata,
    pub provider_process: ProcessIdentity,
    pub issued_at_ms: i64,
}
