//! Conclusive native identity association and atomic Binding Receipt issuance.

use super::ManagedSessionRepository;
use crate::domain::{
    BindingReceipt, ManagedLaunchRegistration, ProviderBindingEvidence, SessionIdentity,
    SourceMetadata,
};
use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingOutcome {
    Waiting,
    Bound(BindingReceipt),
    NeedsReview,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchIdentityAssociationOutcome {
    ContinueBinding,
    NeedsReview,
}

/// Sole application owner for conclusive identity association and Receipt issuance.
#[derive(Default)]
pub struct ManagedBindingService;

impl ManagedBindingService {
    pub fn bind_conclusive<F>(
        &self,
        store: &mut dyn ManagedSessionRepository,
        registration: &ManagedLaunchRegistration,
        evidence: ProviderBindingEvidence,
        now_ms: i64,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut dyn ManagedSessionRepository,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
        let identity = match conclusive_launch_identity(registration, &evidence) {
            Ok(identity) => identity,
            Err(error) => {
                store.mark_needs_review(&registration.id, &error.to_string())?;
                return Ok(BindingOutcome::NeedsReview);
            }
        };
        if associate(store, registration, &identity.session)?
            == LaunchIdentityAssociationOutcome::NeedsReview
        {
            return Ok(BindingOutcome::NeedsReview);
        }
        if let Some(target) = store.managed_resume_target(&registration.id)? {
            let current = store.managed_current_run(target.provider, &target.native_session_id)?;
            let session = store.managed_session(target.provider, &target.native_session_id)?;
            let resume_identity_matches = target.provider == registration.provider
                && evidence.provider == target.provider
                && evidence.source_session_id == target.native_session_id
                && evidence.process_session_id == target.native_session_id
                && current
                    .as_ref()
                    .is_some_and(|run| run.id == target.prior_run_id)
                && session.as_ref().is_some_and(|managed| {
                    current
                        .as_ref()
                        .is_some_and(|run| managed.binding_receipt_id == run.binding_receipt_id)
                });
            if !resume_identity_matches {
                store.mark_needs_review(
                    &registration.id,
                    "Resume evidence does not match the exact Managed Session and prior Run",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
        }
        let receipt = match binding_receipt_candidate(registration, identity, now_ms) {
            Ok(receipt) => receipt,
            Err(error) => {
                store.mark_needs_review(&registration.id, &error.to_string())?;
                return Ok(BindingOutcome::NeedsReview);
            }
        };
        if let Err(error) = store.establish_management(registration, &receipt) {
            store.mark_needs_review(&registration.id, &error.to_string())?;
            return Ok(BindingOutcome::NeedsReview);
        }
        Ok(BindingOutcome::Bound(receipt))
    }
}

struct ConclusiveLaunchIdentity {
    session: SessionIdentity,
    evidence: ProviderBindingEvidence,
    source_path: PathBuf,
}

fn conclusive_launch_identity(
    registration: &ManagedLaunchRegistration,
    evidence: &ProviderBindingEvidence,
) -> Result<ConclusiveLaunchIdentity> {
    let launch = registration
        .launch_evidence
        .as_ref()
        .ok_or_else(|| anyhow!("Binding requires conclusive launch evidence"))?;
    if evidence.provider != registration.provider {
        return Err(anyhow!(
            "Provider source evidence disagrees with the launch"
        ));
    }
    if evidence.source_session_id.is_empty()
        || evidence.process_session_id.is_empty()
        || evidence.source_session_id != evidence.process_session_id
    {
        return Err(anyhow!(
            "native Session ID evidence is incomplete or conflicting"
        ));
    }
    if evidence.launch_process != launch.provider_process {
        return Err(anyhow!(
            "Provider source evidence disagrees with the launched control process"
        ));
    }
    let source_path = evidence
        .source
        .path
        .canonicalize()
        .context("Binding Provider source is unavailable")?;
    if !source_path.is_file() {
        return Err(anyhow!("Binding Provider source is not a file"));
    }
    Ok(ConclusiveLaunchIdentity {
        session: SessionIdentity {
            provider: registration.provider,
            native_id: evidence.source_session_id.clone(),
        },
        evidence: evidence.clone(),
        source_path,
    })
}

fn binding_receipt_candidate(
    registration: &ManagedLaunchRegistration,
    identity: ConclusiveLaunchIdentity,
    now_ms: i64,
) -> Result<BindingReceipt> {
    let evidence = identity.evidence;
    let working_directory = evidence
        .working_directory
        .canonicalize()
        .context("Binding working directory is unavailable")?;
    let target_directory = registration
        .target
        .working_directory
        .canonicalize()
        .context("Managed launch target is unavailable")?;
    if working_directory != target_directory {
        return Err(anyhow!(
            "Provider source working directory disagrees with the launch"
        ));
    }
    Ok(BindingReceipt {
        id: format!("receipt-{}", registration.id),
        registration_id: registration.id.clone(),
        provider: identity.session.provider,
        native_session_id: identity.session.native_id,
        source: SourceMetadata {
            path: identity.source_path,
            format: evidence.source.format,
        },
        provider_process: evidence.provider_process,
        issued_at_ms: now_ms,
    })
}
