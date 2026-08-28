use super::{CollectService, ProjectionService, PublicationService, ReconciliationService};
use crate::application::observation::read_model::RefreshSnapshot;

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct RefreshCyclePorts {
    pub collector: CollectService,
    pub reconciler: ReconciliationService,
    pub projector: ProjectionService,
    pub publisher: PublicationService,
    pub clock: Box<dyn Clock>,
}

/// TUI-owned background lifecycle that orders the four explicit stages.
pub struct RefreshCycle {
    collector: CollectService,
    reconciler: ReconciliationService,
    projector: ProjectionService,
    publisher: PublicationService,
    clock: Box<dyn Clock>,
}

impl RefreshCycle {
    pub fn new(ports: RefreshCyclePorts) -> Self {
        Self {
            collector: ports.collector,
            reconciler: ports.reconciler,
            projector: ports.projector,
            publisher: ports.publisher,
            clock: ports.clock,
        }
    }

    pub fn run_once(&mut self) -> RefreshSnapshot {
        let frame = self.collector.collect(self.clock.now_ms());
        let reconciliation = self.reconciler.reconcile(&frame);
        let snapshot = self.projector.project(&frame, reconciliation);
        self.publisher.publish(snapshot.clone());
        snapshot
    }

    pub fn with_managed_current_run_evidence(
        mut self,
        evidence: Box<dyn crate::application::managed_session::ManagedCurrentRunEvidence>,
    ) -> Self {
        self.collector = self.collector.with_managed_current_run_evidence(evidence);
        self
    }

    pub fn with_managed_resume_evidence(
        mut self,
        evidence: Box<dyn crate::application::managed_session::ManagedResumeEvidence>,
    ) -> Self {
        self.collector = self.collector.with_managed_resume_evidence(evidence);
        self
    }

    pub fn with_managed_binding_reconciler(
        mut self,
        reconciler: crate::application::managed_session::ManagedLaunchReconciler,
    ) -> Self {
        self.collector = self
            .collector
            .with_binding_process_evidence(reconciler.process_evidence());
        self.reconciler = self.reconciler.with_managed_binding_reconciler(reconciler);
        self
    }
}
