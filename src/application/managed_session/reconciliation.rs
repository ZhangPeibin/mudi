//! Recovery and reconciliation for incomplete Managed launch registrations.

use super::{
    BindingOutcome, LaunchIdentityAssociationOutcome, ManagedBindingService, ManagedLaunchBroker,
    ManagedSessionRepository, ManagedTerminalAccess, ProcessEvidence, ProviderProcessObservation,
};
use crate::domain::{
    ManagedLaunchAction, ManagedLaunchRegistration, ManagedLaunchStatus, ProcessIdentity,
    ProviderBindingEvidence, SessionIdentity,
};
use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingProcessObservation {
    pub launch_process: ProviderProcessObservation,
    pub provider_process: Option<ProviderProcessObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedBindingAttempt<'a> {
    pub registration_id: &'a str,
    pub provider_evidence: Option<ProviderBindingEvidence>,
    pub process_evidence: BindingProcessObservation,
    pub observed_at_ms: i64,
    pub after_restart: bool,
}

struct BindingAttempt<'a> {
    registration_id: &'a str,
    provider_evidence: Option<ProviderBindingEvidence>,
    process_evidence: Option<BindingProcessObservation>,
    observed_at_ms: i64,
    clean_confirmed_absence: bool,
}

pub trait ManagedLaunchCleanup: Send + Sync {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> Result<()>;
}

/// Advances an existing authorized registration without creating new authority.
pub struct ManagedLaunchReconciler {
    process_evidence: Arc<dyn ProcessEvidence>,
    cleanup: Box<dyn ManagedLaunchCleanup>,
    binding: ManagedBindingService,
}

impl ManagedLaunchReconciler {
    pub fn new(
        process_evidence: Box<dyn ProcessEvidence>,
        cleanup: Box<dyn ManagedLaunchCleanup>,
    ) -> Self {
        Self {
            process_evidence: Arc::from(process_evidence),
            cleanup,
            binding: ManagedBindingService,
        }
    }

    pub fn process_evidence(&self) -> Arc<dyn ProcessEvidence> {
        self.process_evidence.clone()
    }

    pub fn bind(
        &self,
        store: &mut dyn ManagedSessionRepository,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
    ) -> Result<BindingOutcome> {
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id,
                provider_evidence: evidence,
                process_evidence: None,
                observed_at_ms: now_ms,
                clean_confirmed_absence: true,
            },
            |_, _, _| Ok(LaunchIdentityAssociationOutcome::ContinueBinding),
        )
    }

    pub fn bind_with_identity_association<F>(
        &self,
        store: &mut dyn ManagedSessionRepository,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
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
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id,
                provider_evidence: evidence,
                process_evidence: None,
                observed_at_ms: now_ms,
                clean_confirmed_absence: true,
            },
            associate,
        )
    }

    pub fn reconcile_after_restart(
        &self,
        store: &mut dyn ManagedSessionRepository,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
    ) -> Result<BindingOutcome> {
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id,
                provider_evidence: evidence,
                process_evidence: None,
                observed_at_ms: now_ms,
                clean_confirmed_absence: false,
            },
            |_, _, _| Ok(LaunchIdentityAssociationOutcome::ContinueBinding),
        )
    }

    pub fn reconcile_after_restart_with_identity_association<F>(
        &self,
        store: &mut dyn ManagedSessionRepository,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
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
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id,
                provider_evidence: evidence,
                process_evidence: None,
                observed_at_ms: now_ms,
                clean_confirmed_absence: false,
            },
            associate,
        )
    }

    pub fn reconcile_observed_with_identity_association<F>(
        &self,
        store: &mut dyn ManagedSessionRepository,
        attempt: ObservedBindingAttempt<'_>,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut dyn ManagedSessionRepository,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
        self.bind_with_policy(
            store,
            BindingAttempt {
                registration_id: attempt.registration_id,
                provider_evidence: attempt.provider_evidence,
                process_evidence: Some(attempt.process_evidence),
                observed_at_ms: attempt.observed_at_ms,
                clean_confirmed_absence: !attempt.after_restart,
            },
            associate,
        )
    }

    fn bind_with_policy<F>(
        &self,
        store: &mut dyn ManagedSessionRepository,
        attempt: BindingAttempt<'_>,
        associate: F,
    ) -> Result<BindingOutcome>
    where
        F: FnOnce(
            &mut dyn ManagedSessionRepository,
            &ManagedLaunchRegistration,
            &SessionIdentity,
        ) -> Result<LaunchIdentityAssociationOutcome>,
    {
        let BindingAttempt {
            registration_id,
            provider_evidence: evidence,
            process_evidence: processes,
            observed_at_ms: now_ms,
            clean_confirmed_absence,
        } = attempt;
        let registration = store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Binding Managed Launch Registration {registration_id} is unavailable")
        })?;
        let launch = registration
            .launch_evidence
            .as_ref()
            .ok_or_else(|| anyhow!("Binding requires conclusive launch evidence"))?;
        if registration.status != ManagedLaunchStatus::Binding {
            return Err(anyhow!("only a Binding registration can issue a Receipt"));
        }
        let launch_process = processes.map_or_else(
            || self.process_evidence.observe(launch.provider_process.pid),
            |observed| observed.launch_process,
        );
        match launch_process {
            ProviderProcessObservation::Absent => {
                if !clean_confirmed_absence {
                    store.mark_needs_review(
                        registration_id,
                        "Provider process is absent after restart; launch scene was preserved",
                    )?;
                    return Ok(BindingOutcome::NeedsReview);
                }
                if let Err(error) = self.cleanup.clean_empty(&registration) {
                    store.mark_needs_review(
                        registration_id,
                        &format!("empty private control channel could not be confirmed: {error}"),
                    )?;
                    return Ok(BindingOutcome::NeedsReview);
                }
                store.mark_failed(registration_id, "Provider process is confirmed absent")?;
                return Ok(BindingOutcome::Failed);
            }
            ProviderProcessObservation::Unknown => {
                store.mark_needs_review(
                    registration_id,
                    "launched Provider process could not be verified",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(identity) if identity != launch.provider_process => {
                store.mark_needs_review(
                    registration_id,
                    "launched Provider process is stale or its PID was reused",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(_) => {}
        }
        let Some(evidence) = evidence else {
            return Ok(BindingOutcome::Waiting);
        };
        let provider_process = processes.map_or_else(
            || self.process_evidence.observe(evidence.provider_process.pid),
            |observed| {
                observed
                    .provider_process
                    .unwrap_or(ProviderProcessObservation::Unknown)
            },
        );
        match provider_process {
            ProviderProcessObservation::Absent => {
                store.mark_needs_review(
                    registration_id,
                    "Provider source process is no longer present",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Unknown => {
                store.mark_needs_review(
                    registration_id,
                    "Provider source process could not be verified",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(identity) if identity != evidence.provider_process => {
                store.mark_needs_review(
                    registration_id,
                    "Provider source process is stale or its PID was reused",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(_) => {}
        }
        self.binding
            .bind_conclusive(store, &registration, evidence, now_ms, associate)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedTerminalOutcome {
    pub registration: ManagedLaunchRegistration,
    pub opened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLaunchDiagnostics {
    pub registration_id: String,
    pub provider: crate::domain::Provider,
    pub working_directory: PathBuf,
    pub launch_process: ProcessIdentity,
    pub channel_id: String,
    pub binding_error: Option<String>,
    pub terminal_error: Option<String>,
}

/// Explicit user-driven recovery actions for incomplete Managed launches.
pub struct ManagedLaunchRecoveryService {
    store: Box<dyn ManagedSessionRepository>,
    launch_broker: Box<dyn ManagedLaunchBroker>,
    terminal_access: Box<dyn ManagedTerminalAccess>,
}

impl ManagedLaunchRecoveryService {
    pub fn new(
        store: impl ManagedSessionRepository + 'static,
        launch_broker: Box<dyn ManagedLaunchBroker>,
        terminal_access: Box<dyn ManagedTerminalAccess>,
    ) -> Self {
        Self {
            store: Box::new(store),
            launch_broker,
            terminal_access,
        }
    }

    pub fn cancel_start(&mut self, registration_id: &str) -> Result<()> {
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration {registration_id} is unavailable")
        })?;
        if registration.status != ManagedLaunchStatus::Binding {
            return Err(anyhow!("Cancel Start is available only while Binding"));
        }
        self.launch_broker
            .cancel(&registration)
            .context("Cancel Start could not stop the Managed launch")?;
        self.store.remove_binding(registration_id)
    }

    pub fn retry_open_terminal(
        &mut self,
        registration_id: &str,
        now_ms: i64,
    ) -> Result<ManagedTerminalOutcome> {
        self.access_terminal(
            registration_id,
            now_ms,
            ManagedLaunchAction::RetryOpenTerminal,
            "Retry Open Terminal",
        )
    }

    pub fn open_terminal(
        &mut self,
        registration_id: &str,
        now_ms: i64,
    ) -> Result<ManagedTerminalOutcome> {
        self.access_terminal(
            registration_id,
            now_ms,
            ManagedLaunchAction::OpenTerminal,
            "Open Terminal",
        )
    }

    pub fn diagnostics(&self, registration_id: &str) -> Result<ManagedLaunchDiagnostics> {
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration {registration_id} is unavailable")
        })?;
        if !registration.allows(ManagedLaunchAction::CopyDiagnostics) {
            return Err(anyhow!(
                "Copy Diagnostics is unavailable for this Managed launch"
            ));
        }
        let launch = registration
            .launch_evidence
            .as_ref()
            .ok_or_else(|| anyhow!("Managed launch evidence is unavailable"))?;
        Ok(ManagedLaunchDiagnostics {
            registration_id: registration.id,
            provider: registration.provider,
            working_directory: registration.target.working_directory,
            launch_process: launch.provider_process,
            channel_id: launch.channel_id.clone(),
            binding_error: registration.binding_error,
            terminal_error: registration.terminal_error,
        })
    }

    fn access_terminal(
        &mut self,
        registration_id: &str,
        now_ms: i64,
        action: ManagedLaunchAction,
        label: &str,
    ) -> Result<ManagedTerminalOutcome> {
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration {registration_id} is unavailable")
        })?;
        if !registration.allows(action) {
            return Err(anyhow!("{label} is unavailable for this Managed launch"));
        }
        let opened = match self.terminal_access.open(&registration) {
            Ok(()) => {
                self.store.record_terminal_opened(registration_id, now_ms)?;
                true
            }
            Err(error) => {
                self.store
                    .record_terminal_error(registration_id, &error.to_string())?;
                false
            }
        };
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration disappeared after Terminal access")
        })?;
        Ok(ManagedTerminalOutcome {
            registration,
            opened,
        })
    }
}
