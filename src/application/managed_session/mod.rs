//! Managed Session workflows and application-owned external capability ports.

mod binding;
mod current_run_access;
mod evidence;
mod launch;
mod ports;
mod reconciliation;
mod repository;
mod start_new;
mod start_resume;

pub use binding::{BindingOutcome, LaunchIdentityAssociationOutcome, ManagedBindingService};
pub use current_run_access::{
    run_attach_from_args_with_access, ManagedCurrentRunAccessService, ManagedCurrentRunEvidence,
    ManagedCurrentRunTerminalAccess, ManagedCurrentRunVerifier, VerifiedManagedCurrentRun,
};
pub use evidence::{ProcessEvidence, ProviderProcessObservation};
pub use ports::{
    ManagedLaunchBroker, ManagedProviderRuntime, ManagedTerminalAccess, ProviderLaunchCommand,
};
pub use reconciliation::{
    BindingProcessObservation, ManagedLaunchCleanup, ManagedLaunchDiagnostics,
    ManagedLaunchReconciler, ManagedLaunchRecoveryService, ManagedTerminalOutcome,
    ObservedBindingAttempt,
};
pub use repository::ManagedSessionRepository;
pub use start_new::{ManagedNewOutcome, ManagedNewRequest, ManagedNewService};
pub use start_resume::{
    CombinedManagedResumeEvidence, ManagedResumeEvidence, ManagedResumeEvidenceCandidate,
    ManagedResumeOutcome, ManagedResumeProviderEvidence, ManagedResumeRequest,
    ManagedResumeService, ManagedResumeVerifier, VerifiedExitedManagedSession,
};

pub use crate::domain::{
    BindingReceipt, ManagedControlChannel, ManagedCurrentRunLifecycle, ManagedCurrentRunRecord,
    ManagedLaunchAction, ManagedLaunchEvidence, ManagedLaunchRegistration, ManagedLaunchStatus,
    ManagedLaunchTarget, ManagedResumeTarget, ManagedSessionRecord, ProviderBindingEvidence,
};
