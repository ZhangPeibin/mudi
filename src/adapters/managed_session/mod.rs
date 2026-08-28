//! Provider-specific evidence and runtime adapters for Managed Session workflows.

mod evidence;
mod provider_runtime;

pub use evidence::{SystemManagedCurrentRunEvidence, SystemManagedResumeProviderEvidence};
