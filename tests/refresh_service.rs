use mudi::adapters::config::Config;
use mudi::adapters::refresh::open_refresh_cycle;
use mudi::application::observation::read_model::RefreshHealth;
use std::io::Write;
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

    let mut refresh = open_refresh_cycle(config);
    let snapshot = refresh.run_once();

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

    let snapshot = open_refresh_cycle(config).run_once();

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

    let snapshot = open_refresh_cycle(config).run_once();

    assert_eq!(snapshot.sessions.len(), 1);
    assert!(!format!("{snapshot:#?}").contains(private_content));
}

#[test]
fn collect_handles_append_truncate_delete_malformed_source_and_index_rebuild() {
    let temp = TempDir::new();
    let claude_dir = temp.0.join("claude");
    let transcript_dir = claude_dir.join("projects/project");
    let working_directory = temp.0.join("project");
    std::fs::create_dir_all(&transcript_dir).unwrap();
    std::fs::create_dir_all(&working_directory).unwrap();
    let source = transcript_dir.join("source-lifecycle.jsonl");
    let record = |timestamp: &str, output_tokens: u64| {
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"{timestamp}\",\"cwd\":\"{}\",\"message\":{{\"id\":\"message-{output_tokens}\",\"model\":\"claude-opus-4-8\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":{output_tokens}}}}}}}\n",
            working_directory.display()
        )
    };
    std::fs::write(&source, record("2026-08-11T00:00:00Z", 1)).unwrap();
    let config = Config {
        claude_dir,
        codex_dir: temp.0.join("codex"),
        data_dir: temp.0.join("data"),
        extra_dirs: vec![],
    };
    let mut cycle = open_refresh_cycle(config.clone());

    let initial = cycle.run_once();
    assert_eq!(initial.sessions.len(), 1);
    assert_eq!(initial.stats.parsed, 1);

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap();
    file.write_all(b"malformed partial record\n").unwrap();
    file.write_all(record("2026-08-11T00:01:00Z", 22).as_bytes())
        .unwrap();
    drop(file);
    let appended = cycle.run_once();
    assert_eq!(appended.sessions.len(), 1);
    assert_eq!(appended.stats.parsed, 1);
    assert!(appended.sessions[0].usage.output >= 22);

    std::fs::write(&source, "malformed after truncation\n").unwrap();
    let malformed = cycle.run_once();
    assert!(malformed.sessions.is_empty());
    assert_eq!(malformed.stats.pruned, 1);

    std::fs::write(&source, record("2026-08-11T00:02:00Z", 3)).unwrap();
    let restored = cycle.run_once();
    assert_eq!(restored.sessions.len(), 1);
    assert_eq!(restored.stats.parsed, 1);

    std::fs::remove_file(&source).unwrap();
    let deleted = cycle.run_once();
    assert!(deleted.sessions.is_empty());
    assert_eq!(deleted.stats.pruned, 1);

    std::fs::write(&source, record("2026-08-11T00:03:00Z", 4)).unwrap();
    assert_eq!(cycle.run_once().sessions.len(), 1);
    drop(cycle);

    for path in [
        config.index_path(),
        config.data_dir.join("index.db-wal"),
        config.data_dir.join("index.db-shm"),
    ] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("could not remove disposable index: {error}"),
        }
    }
    let rebuilt = open_refresh_cycle(config).run_once();
    assert_eq!(rebuilt.sessions.len(), 1);
    assert_eq!(rebuilt.stats.parsed, 1);
}
