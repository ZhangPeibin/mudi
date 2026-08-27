use crate::application::alerts::AlertEngine;
use crate::application::observation::read_model::{ReadModelCache, RefreshSnapshot};
use std::sync::Arc;

/// Atomically replaces the read model only after projection has produced a complete Snapshot.
pub struct PublicationService {
    cache: Arc<dyn ReadModelCache>,
    alert_engine: AlertEngine,
}

impl PublicationService {
    pub fn new(cache: Arc<dyn ReadModelCache>, alert_engine: AlertEngine) -> Self {
        Self {
            cache,
            alert_engine,
        }
    }

    pub fn publish(&mut self, snapshot: RefreshSnapshot) {
        self.alert_engine.update(&snapshot.sessions);
        self.cache.publish(snapshot);
    }
}
