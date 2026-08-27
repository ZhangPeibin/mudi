use super::ObservationFrame;
use crate::application::handoff::{HandoffService, WorkerLauncher};
use crate::application::managed_sessions::{
    BindingOutcome, EvidenceBinder, LaunchIdentityAssociationOutcome, ManagedLaunchRegistration,
    ManagedSessionRepository, ObservedBindingAttempt,
};
use crate::application::observation::read_model::{RefreshDiagnostic, RefreshStage};
use crate::domain::SessionIdentity;
use anyhow::Result;
use std::collections::HashSet;

pub trait OperationTaskReconciler: Send {
    fn reconcile(&mut self, frame: &ObservationFrame) -> Result<()>;
}

impl<L: WorkerLauncher + Send> OperationTaskReconciler for HandoffService<L> {
    fn reconcile(&mut self, frame: &ObservationFrame) -> Result<()> {
        self.reconcile_tasks(frame.observed_at_ms).map(|_| ())
    }
}

pub struct ReconciliationPorts {
    pub managed_repository: std::result::Result<Box<dyn ManagedSessionRepository>, String>,
    pub managed_binding_reconciler: EvidenceBinder,
    pub task_reconciler: std::result::Result<Box<dyn OperationTaskReconciler>, String>,
}

#[derive(Debug, Default)]
pub struct ReconciliationReport {
    pub diagnostics: Vec<RefreshDiagnostic>,
}

/// Advances only records whose authorization already exists in authoritative state.
pub struct ReconciliationService {
    managed_repository: std::result::Result<Box<dyn ManagedSessionRepository>, String>,
    restart_binding_registrations: HashSet<String>,
    managed_binding_reconciler: EvidenceBinder,
    task_reconciler: std::result::Result<Box<dyn OperationTaskReconciler>, String>,
}

impl ReconciliationService {
    pub fn new(ports: ReconciliationPorts) -> Self {
        let restart_binding_registrations = ports
            .managed_repository
            .as_ref()
            .ok()
            .and_then(|store| store.binding_registrations().ok())
            .unwrap_or_default()
            .into_iter()
            .map(|registration| registration.id)
            .collect();
        Self {
            managed_repository: ports.managed_repository,
            restart_binding_registrations,
            managed_binding_reconciler: ports.managed_binding_reconciler,
            task_reconciler: ports.task_reconciler,
        }
    }

    pub fn reconcile(&mut self, frame: &ObservationFrame) -> ReconciliationReport {
        let mut report = ReconciliationReport::default();
        match self.managed_repository.as_mut() {
            Ok(store) => {
                for observation in &frame.managed_bindings {
                    let registration = match store.get(&observation.registration_id) {
                        Ok(Some(registration)) => registration,
                        Ok(None) => continue,
                        Err(error) => {
                            report.diagnostics.push(RefreshDiagnostic::for_stage(
                                RefreshStage::Reconcile,
                                "Managed Registration Store",
                                error.to_string(),
                            ));
                            continue;
                        }
                    };
                    let restarting = self
                        .restart_binding_registrations
                        .contains(&observation.registration_id);
                    let outcome = self
                        .managed_binding_reconciler
                        .bind_observed_with_identity_association(
                            store.as_mut(),
                            ObservedBindingAttempt {
                                registration_id: &observation.registration_id,
                                provider_evidence: observation.evidence.clone(),
                                process_evidence: observation.processes,
                                observed_at_ms: frame.observed_at_ms,
                                after_restart: restarting,
                            },
                            associate_launch_title,
                        );
                    match outcome {
                        Ok(BindingOutcome::Bound(_))
                        | Ok(BindingOutcome::NeedsReview)
                        | Ok(BindingOutcome::Failed) => {
                            self.restart_binding_registrations
                                .remove(&observation.registration_id);
                        }
                        Ok(BindingOutcome::Waiting) => {}
                        Err(error) => report.diagnostics.push(RefreshDiagnostic::for_stage(
                            RefreshStage::Reconcile,
                            "Managed Binding",
                            format!("{}: {error}", registration.id),
                        )),
                    }
                }
            }
            Err(error) => report.diagnostics.push(RefreshDiagnostic::for_stage(
                RefreshStage::Reconcile,
                "Managed Session Registry",
                error.clone(),
            )),
        }

        match self.task_reconciler.as_mut() {
            Ok(reconciler) => {
                if let Err(error) = reconciler.reconcile(frame) {
                    report.diagnostics.push(RefreshDiagnostic::for_stage(
                        RefreshStage::Reconcile,
                        "Operation Tasks",
                        error.to_string(),
                    ));
                }
            }
            Err(error) => report.diagnostics.push(RefreshDiagnostic::for_stage(
                RefreshStage::Reconcile,
                "Operation Tasks",
                error.clone(),
            )),
        }
        report
    }

    pub fn with_managed_binding_reconciler(mut self, reconciler: EvidenceBinder) -> Self {
        self.managed_binding_reconciler = reconciler;
        self
    }
}

fn associate_launch_title(
    store: &mut dyn ManagedSessionRepository,
    launch: &ManagedLaunchRegistration,
    session: &SessionIdentity,
) -> Result<LaunchIdentityAssociationOutcome> {
    if store.managed_resume_target(&launch.id)?.is_some() {
        return Ok(LaunchIdentityAssociationOutcome::ContinueBinding);
    }
    if let Some(associated) = store.launch_title_association(&launch.id)? {
        if associated == *session {
            return Ok(LaunchIdentityAssociationOutcome::ContinueBinding);
        }
        store.mark_needs_review(
            &launch.id,
            "Provider evidence disagrees with the Session identity already associated with the Title",
        )?;
        return Ok(LaunchIdentityAssociationOutcome::NeedsReview);
    }
    store.associate_launch_title(&launch.id, session, &launch.session_alias)?;
    Ok(LaunchIdentityAssociationOutcome::ContinueBinding)
}
