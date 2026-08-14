use mudi::read_model::{
    InMemoryReadModelCache, ReadModelCache, RefreshDiagnostic, RefreshHealth, RefreshSnapshot,
};

#[test]
fn cache_publishes_the_latest_structured_snapshot_with_degraded_health() {
    let cache = InMemoryReadModelCache::default();
    let snapshot = RefreshSnapshot {
        revision: 7,
        generated_at_ms: 123,
        health: RefreshHealth::Degraded {
            diagnostics: vec![RefreshDiagnostic::new(
                "Task Store",
                "database is unavailable",
            )],
        },
        ..RefreshSnapshot::default()
    };

    cache.publish(snapshot);
    let published = cache.current();

    assert_eq!(published.revision, 7);
    assert_eq!(published.generated_at_ms, 123);
    assert_eq!(
        published.health,
        RefreshHealth::Degraded {
            diagnostics: vec![RefreshDiagnostic::new(
                "Task Store",
                "database is unavailable"
            )],
        }
    );
}
