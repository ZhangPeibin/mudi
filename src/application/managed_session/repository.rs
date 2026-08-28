//! Authoritative repository port shared by Managed Session workflows.

use crate::domain::{
    BindingReceipt, ManagedCurrentRunRecord, ManagedLaunchEvidence, ManagedLaunchRegistration,
    ManagedResumeTarget, ManagedSessionRecord, Provider, Session, SessionIdentity,
};
use anyhow::Result;

pub trait ManagedSessionRepository: Send {
    fn insert(&self, registration: &ManagedLaunchRegistration) -> Result<()>;
    fn insert_resume(
        &mut self,
        registration: &ManagedLaunchRegistration,
        target: &ManagedResumeTarget,
    ) -> Result<()>;
    fn managed_resume_target(&self, registration_id: &str) -> Result<Option<ManagedResumeTarget>>;
    fn launch_title_association(&self, registration_id: &str) -> Result<Option<SessionIdentity>>;
    fn record_launch_title_association(
        &self,
        registration_id: &str,
        session: &SessionIdentity,
    ) -> Result<()>;
    fn associate_launch_title(
        &mut self,
        registration_id: &str,
        session: &SessionIdentity,
        _alias: &str,
    ) -> Result<()> {
        self.record_launch_title_association(registration_id, session)
    }
    fn has_pending_resume(&self, provider: Provider, native_session_id: &str) -> Result<bool>;
    fn get(&self, registration_id: &str) -> Result<Option<ManagedLaunchRegistration>>;
    fn binding_registrations(&self) -> Result<Vec<ManagedLaunchRegistration>>;
    fn registrations(&self) -> Result<Vec<ManagedLaunchRegistration>>;
    fn binding_receipt(&self, registration_id: &str) -> Result<Option<BindingReceipt>>;
    fn binding_receipt_by_id(&self, receipt_id: &str) -> Result<Option<BindingReceipt>>;
    fn managed_session(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedSessionRecord>>;
    fn managed_sessions(&self) -> Result<Vec<ManagedSessionRecord>>;
    fn managed_current_run(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedCurrentRunRecord>>;
    fn managed_current_runs(&self) -> Result<Vec<ManagedCurrentRunRecord>>;
    fn apply_provenance(&self, sessions: &mut [Session]) -> Result<()>;
    fn establish_management(
        &mut self,
        registration: &ManagedLaunchRegistration,
        receipt: &BindingReceipt,
    ) -> Result<()>;
    fn record_launched(
        &self,
        registration_id: &str,
        now_ms: i64,
        evidence: ManagedLaunchEvidence,
    ) -> Result<()>;
    fn record_terminal_opened(&self, registration_id: &str, now_ms: i64) -> Result<()>;
    fn record_terminal_error(&self, registration_id: &str, error: &str) -> Result<()>;
    fn remove_binding(&mut self, registration_id: &str) -> Result<()>;
    fn mark_needs_review(&self, registration_id: &str, error: &str) -> Result<()>;
    fn mark_failed(&mut self, registration_id: &str, error: &str) -> Result<()>;
}
