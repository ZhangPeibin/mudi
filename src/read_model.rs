//! Structured, metadata-only projection published by refresh and consumed by the UI.

use crate::alerts::AlertCounts;
use crate::attention::AttentionSnapshot;
use crate::collector::ScanStats;
use crate::domain::Session;
use crate::signals::RecentSignal;
use crate::tasks::OperationTask;
use std::sync::{Arc, RwLock};

/// A component-scoped explanation of why the current projection is degraded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshDiagnostic {
    pub component: String,
    pub message: String,
}

impl RefreshDiagnostic {
    pub fn new(component: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            component: component.into(),
            message: message.into(),
        }
    }
}

/// Freshness and persistence health of a [`RefreshSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RefreshHealth {
    #[default]
    Fresh,
    Degraded {
        diagnostics: Vec<RefreshDiagnostic>,
    },
}

impl RefreshHealth {
    pub fn from_diagnostics(diagnostics: Vec<RefreshDiagnostic>) -> Self {
        if diagnostics.is_empty() {
            Self::Fresh
        } else {
            Self::Degraded { diagnostics }
        }
    }

    pub fn is_fresh(&self) -> bool {
        matches!(self, Self::Fresh)
    }
}

/// The read-only projection of the latest observed local state.
///
/// It intentionally contains only normalized metadata. Provider conversation content,
/// prompt bodies, raw tool output, source-file content, and Handoff Artifact bodies never
/// enter this type.
#[derive(Debug, Clone, Default)]
pub struct RefreshSnapshot {
    pub revision: u64,
    pub generated_at_ms: i64,
    pub health: RefreshHealth,
    pub sessions: Vec<Session>,
    pub tasks: Vec<OperationTask>,
    pub recent_signals: Vec<RecentSignal>,
    pub attention: Option<AttentionSnapshot>,
    pub stats: ScanStats,
    pub alerts: AlertCounts,
    /// Compatibility timestamp for existing presentation code. Refresh keeps it equal to
    /// `generated_at_ms`; consumers should prefer that canonical field for new code.
    pub generated_ms: i64,
    /// True once at least one refresh has attempted to produce a projection.
    pub ready: bool,
}

/// The publication boundary between refresh and every read-only consumer.
pub trait ReadModelCache: Send + Sync {
    fn publish(&self, snapshot: RefreshSnapshot);
    fn current(&self) -> Arc<RefreshSnapshot>;
}

/// Thread-safe in-process cache. A different transport can implement [`ReadModelCache`]
/// without changing UI code.
pub struct InMemoryReadModelCache {
    current: RwLock<Arc<RefreshSnapshot>>,
}

impl Default for InMemoryReadModelCache {
    fn default() -> Self {
        Self {
            current: RwLock::new(Arc::new(RefreshSnapshot::default())),
        }
    }
}

impl ReadModelCache for InMemoryReadModelCache {
    fn publish(&self, snapshot: RefreshSnapshot) {
        let mut current = self
            .current
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *current = Arc::new(snapshot);
    }

    fn current(&self) -> Arc<RefreshSnapshot> {
        self.current
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_is_fresh_only_when_no_diagnostic_exists() {
        assert!(RefreshHealth::from_diagnostics(vec![]).is_fresh());
        assert!(
            !RefreshHealth::from_diagnostics(vec![RefreshDiagnostic::new("Index", "unavailable")])
                .is_fresh()
        );
    }
}
