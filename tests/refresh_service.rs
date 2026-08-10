use sessionmesh::config::Config;
use sessionmesh::read_model::RefreshHealth;
use sessionmesh::refresh::RefreshService;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let suffix = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sessionmesh-refresh-service-{}-{suffix}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn failed_index_open_publishes_a_degraded_snapshot_instead_of_a_healthy_empty_one() {
    let temp = TempDir::new();
    let blocked_data_dir = temp.0.join("not-a-directory");
    std::fs::write(&blocked_data_dir, "blocked").unwrap();
    let config = Config {
        claude_dir: temp.0.join("claude"),
        codex_dir: temp.0.join("codex"),
        data_dir: blocked_data_dir,
        extra_dirs: vec![],
    };

    let mut refresh = RefreshService::new(config);
    let snapshot = refresh.refresh();

    assert!(snapshot.ready);
    assert!(snapshot.sessions.is_empty());
    assert!(matches!(snapshot.health, RefreshHealth::Degraded { .. }));
}
