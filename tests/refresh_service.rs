use mudi::adapters::config::Config;
use mudi::adapters::refresh::open_refresh_service;
use mudi::application::observation::read_model::RefreshHealth;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let suffix = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mudi-refresh-service-{}-{suffix}",
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

    let mut refresh = open_refresh_service(config);
    let snapshot = refresh.refresh();

    assert!(snapshot.ready);
    assert!(snapshot.sessions.is_empty());
    assert!(matches!(snapshot.health, RefreshHealth::Degraded { .. }));
}

#[cfg(unix)]
#[test]
fn project_resolver_failure_degrades_snapshot_without_hiding_the_session() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new();
    let claude_dir = temp.0.join("claude");
    let transcript_dir = claude_dir.join("projects/project");
    std::fs::create_dir_all(&transcript_dir).unwrap();

    let first_link = temp.0.join("cwd-loop-a");
    let second_link = temp.0.join("cwd-loop-b");
    symlink(&second_link, &first_link).unwrap();
    symlink(&first_link, &second_link).unwrap();
    std::fs::write(
        transcript_dir.join("broken-project.jsonl"),
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"2026-08-11T00:00:00Z\",\"cwd\":\"{}\",\"message\":{{\"id\":\"message\",\"model\":\"claude-opus-4-8\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}}}\n",
            first_link.display()
        ),
    )
    .unwrap();

    let config = Config {
        claude_dir,
        codex_dir: temp.0.join("codex"),
        data_dir: temp.0.join("data"),
        extra_dirs: vec![],
    };

    let snapshot = open_refresh_service(config).refresh();

    assert_eq!(snapshot.sessions.len(), 1);
    assert!(snapshot.sessions[0].project.is_none());
    let RefreshHealth::Degraded { diagnostics } = snapshot.health else {
        panic!("Project resolution failure must degrade the snapshot");
    };
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.component == "Project Resolver" && diagnostic.message.contains("cwd-loop-a")
    }));
}

#[test]
fn refresh_snapshot_omits_provider_conversation_content() {
    let temp = TempDir::new();
    let claude_dir = temp.0.join("claude");
    let transcript_dir = claude_dir.join("projects/project");
    let working_directory = temp.0.join("project");
    std::fs::create_dir_all(&transcript_dir).unwrap();
    std::fs::create_dir_all(&working_directory).unwrap();
    let private_content = "PRIVATE_PROVIDER_CONVERSATION_SENTINEL";
    std::fs::write(
        transcript_dir.join("content-safe.jsonl"),
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"2026-08-11T00:00:00Z\",\"cwd\":\"{}\",\"message\":{{\"id\":\"message\",\"model\":\"claude-opus-4-8\",\"content\":\"{}\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}}}\n",
            working_directory.display(),
            private_content
        ),
    )
    .unwrap();
    let config = Config {
        claude_dir,
        codex_dir: temp.0.join("codex"),
        data_dir: temp.0.join("data"),
        extra_dirs: vec![],
    };

    let snapshot = open_refresh_service(config).refresh();

    assert_eq!(snapshot.sessions.len(), 1);
    assert!(!format!("{snapshot:#?}").contains(private_content));
}
