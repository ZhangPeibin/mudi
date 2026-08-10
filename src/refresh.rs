//! Refresh application service: the sole publisher of the UI read model.

use crate::alerts::{self, AlertEngine};
use crate::attention::AttentionEngine;
use crate::collector::{self, ProjectResolverCache, ScanStats};
use crate::config::Config;
use crate::index::Index;
use crate::managed_sessions::{
    discover_tmux_executable, BindingOutcome, EvidenceBinder, ManagedLaunchStore,
    SystemProcessEvidence, TmuxManagedLaunchCleanup,
};
use crate::models::ModelCatalog;
use crate::providers::registry;
use crate::read_model::{ReadModelCache, RefreshDiagnostic, RefreshHealth, RefreshSnapshot};
use crate::session_labels::SessionLabels;
use crate::signals::RecentSignalsEngine;
use crate::tasks::TaskStore;

/// Owns source discovery, disposable indexing, derived signals, and read-model publication.
pub struct RefreshService {
    cfg: Config,
    index: std::result::Result<Index, String>,
    task_store: std::result::Result<TaskStore, String>,
    managed_store: std::result::Result<ManagedLaunchStore, String>,
    models: ModelCatalog,
    alert_engine: AlertEngine,
    attention_engine: AttentionEngine,
    recent_signals_engine: RecentSignalsEngine,
    project_resolver: ProjectResolverCache,
    revision: u64,
}

impl RefreshService {
    pub fn new(cfg: Config) -> Self {
        let index = Index::open(&cfg.index_path()).map_err(|error| error.to_string());
        let task_store = TaskStore::open(&cfg.task_store_path()).map_err(|error| error.to_string());
        let managed_store = ManagedLaunchStore::open(cfg.managed_launch_store_path())
            .map_err(|error| error.to_string());
        Self {
            models: ModelCatalog::load(Some(&cfg.model_catalog_override_path())),
            alert_engine: AlertEngine::new(cfg.label_store_path()),
            cfg,
            index,
            task_store,
            managed_store,
            attention_engine: AttentionEngine::default(),
            recent_signals_engine: RecentSignalsEngine::default(),
            project_resolver: ProjectResolverCache::default(),
            revision: 0,
        }
    }

    /// Build one snapshot even when a persistence dependency is unavailable.
    pub fn refresh(&mut self) -> RefreshSnapshot {
        self.revision = self.revision.saturating_add(1);
        let now = chrono::Local::now().timestamp_millis();
        let mut diagnostics = Vec::new();
        let mut stats = ScanStats::default();
        let mut sessions = Vec::new();
        let mut managed_launches = Vec::new();

        match self.index.as_mut() {
            Ok(index) => match collector::refresh_checked(&self.cfg, index) {
                Ok(scanned) => {
                    stats = scanned;
                    match collector::build_sessions_with_resolver(
                        &self.cfg,
                        index,
                        &self.models,
                        &mut self.project_resolver,
                    ) {
                        Ok(built) => sessions = built,
                        Err(error) => diagnostics
                            .push(RefreshDiagnostic::new("Index read", error.to_string())),
                    }
                }
                Err(error) => {
                    diagnostics.push(RefreshDiagnostic::new("Index refresh", error.to_string()));
                    if let Ok(built) = collector::build_sessions_with_resolver(
                        &self.cfg,
                        index,
                        &self.models,
                        &mut self.project_resolver,
                    ) {
                        sessions = built;
                    }
                }
            },
            Err(error) => diagnostics.push(RefreshDiagnostic::new("Index", error.clone())),
        }

        match self.managed_store.as_mut() {
            Ok(store) => {
                let binder = EvidenceBinder::new(
                    Box::new(SystemProcessEvidence),
                    Box::new(TmuxManagedLaunchCleanup::new(
                        discover_tmux_executable().unwrap_or_default(),
                    )),
                );
                match store.binding_registrations() {
                    Ok(registrations) => {
                        for registration in registrations {
                            let Some(process) = registration
                                .launch_evidence
                                .as_ref()
                                .map(|evidence| evidence.provider_process)
                            else {
                                continue;
                            };
                            let evidence = registry()
                                .runtime_control(registration.provider)
                                .ok()
                                .and_then(|adapter| adapter.binding_evidence(&self.cfg, process));
                            match binder.bind(store, &registration.id, evidence, now) {
                                Ok(BindingOutcome::Bound(_))
                                | Ok(BindingOutcome::Waiting)
                                | Ok(BindingOutcome::NeedsReview)
                                | Ok(BindingOutcome::Failed) => {}
                                Err(error) => diagnostics.push(RefreshDiagnostic::new(
                                    "Managed Binding",
                                    error.to_string(),
                                )),
                            }
                        }
                    }
                    Err(error) => diagnostics.push(RefreshDiagnostic::new(
                        "Managed Registration Store",
                        error.to_string(),
                    )),
                }
                if let Err(error) = sync_managed_metadata(&self.cfg, store) {
                    diagnostics.push(RefreshDiagnostic::new(
                        "Managed Session Metadata",
                        error.to_string(),
                    ));
                }
                if let Err(error) = store.apply_provenance(&mut sessions) {
                    diagnostics.push(RefreshDiagnostic::new(
                        "Managed Session Registry",
                        error.to_string(),
                    ));
                }
                match store.registrations() {
                    Ok(registrations) => {
                        managed_launches = registrations
                            .iter()
                            .map(crate::read_model::ManagedLaunchSummary::from)
                            .collect();
                    }
                    Err(error) => diagnostics.push(RefreshDiagnostic::new(
                        "Managed Registration Store",
                        error.to_string(),
                    )),
                }
            }
            Err(error) => diagnostics.push(RefreshDiagnostic::new(
                "Managed Session Registry",
                error.clone(),
            )),
        }

        collector::sort_for_display(&mut sessions);
        self.alert_engine.update(&sessions);
        let alerts = alerts::counts(&sessions);
        let attention = self.attention_engine.update(&sessions, now);
        let recent_signals = self.recent_signals_engine.update(&sessions, now).to_vec();
        let tasks = match self.task_store.as_ref() {
            Ok(store) => match store.list() {
                Ok(tasks) => tasks,
                Err(error) => {
                    diagnostics.push(RefreshDiagnostic::new("Task Store", error.to_string()));
                    Vec::new()
                }
            },
            Err(error) => {
                diagnostics.push(RefreshDiagnostic::new("Task Store", error.clone()));
                Vec::new()
            }
        };
        let health = RefreshHealth::from_diagnostics(diagnostics);
        RefreshSnapshot {
            revision: self.revision,
            generated_at_ms: now,
            health,
            sessions,
            tasks,
            managed_launches,
            recent_signals,
            attention: Some(attention),
            stats,
            alerts,
            generated_ms: now,
            ready: true,
        }
    }

    pub fn refresh_and_publish(&mut self, cache: &dyn ReadModelCache) {
        cache.publish(self.refresh());
    }
}

fn sync_managed_metadata(cfg: &Config, store: &ManagedLaunchStore) -> anyhow::Result<()> {
    let mut labels = SessionLabels::load(cfg.label_store_path());
    if let Some(error) = labels.error() {
        anyhow::bail!(error.to_string());
    }
    for session in store.managed_sessions()? {
        if store.managed_metadata_is_synchronized(session.provider, &session.native_session_id)? {
            continue;
        }
        let description = session.description.as_deref().unwrap_or_default();
        if labels.alias(session.provider, &session.native_session_id)
            != Some(session.session_alias.as_str())
            || labels.description(session.provider, &session.native_session_id)
                != session.description.as_deref()
        {
            labels.set_metadata(
                session.provider,
                &session.native_session_id,
                &session.session_alias,
                description,
            )?;
        }
        store.mark_managed_metadata_synchronized(session.provider, &session.native_session_id)?;
    }
    Ok(())
}
