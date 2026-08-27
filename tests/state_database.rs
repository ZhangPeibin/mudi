use mudi::adapters::persistence::managed_sessions::ManagedLaunchStore;
use mudi::adapters::persistence::session_metadata::SessionLabels;
use mudi::adapters::persistence::state::StateDatabase;
use mudi::adapters::persistence::tasks::TaskStore;
use mudi::application::session_labels::CurationDisposition;
use mudi::domain::{Provider, SessionIdentity};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

#[test]
fn fresh_state_database_has_one_complete_versioned_private_schema() {
    let temp = TempDir::new("fresh-schema");
    let path = temp.path().join("state.db");

    let database = StateDatabase::open(&path).unwrap();
    let connection = database.connect().unwrap();

    assert_eq!(pragma_i64(&connection, "user_version"), 1);
    assert_eq!(pragma_i64(&connection, "foreign_keys"), 1);
    assert_eq!(pragma_i64(&connection, "busy_timeout"), 5_000);
    assert_eq!(
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "wal"
    );

    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        tables,
        [
            "binding_receipts",
            "managed_current_runs",
            "managed_launch_registrations",
            "managed_launch_title_associations",
            "managed_metadata_syncs",
            "managed_resume_registrations",
            "managed_sessions",
            "operation_tasks",
            "session_metadata",
        ]
    );

    #[cfg(unix)]
    {
        assert_eq!(
            std::fs::metadata(temp.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn state_database_v1_has_exact_columns_foreign_keys_and_named_indexes() {
    let temp = TempDir::new("exact-schema-contract");
    let path = temp.path().join("state.db");
    let connection = StateDatabase::open(&path).unwrap().connect().unwrap();

    assert_eq!(
        column_names(&connection, "operation_tasks"),
        [
            "task_id",
            "state",
            "source_provider",
            "source_session_id",
            "source_record_path",
            "source_working_directory",
            "successor_provider",
            "successor_executable",
            "created_at_ms",
            "state_changed_at_ms",
            "worker_pid",
            "worker_started_at_ms",
            "provider_pid",
            "provider_started_at_ms",
            "prompt_sent_at_ms",
            "provider_exited_at_ms",
            "provider_exit_code",
            "running_deadline_ms",
            "cancellation_requested_at_ms",
            "termination_requested_at_ms",
            "retry_of_task_id",
            "retry_ready_at_ms",
            "artifact_path",
            "plan_provider",
            "plan_working_directory",
            "plan_executable",
            "plan_arguments_json",
            "plan_artifact_path",
            "successor_launch_requested_at_ms",
            "successor_launch_failed_at_ms",
            "successor_launch_failure_code",
            "successor_pid",
            "successor_process_started_at_ms",
            "successor_started_at_ms",
            "successor_exited_at_ms",
            "successor_exit_code",
            "successor_session_id",
            "successor_linked_at_ms",
            "failure_code",
            "acknowledged_at_ms",
            "end_current_run_authorized",
            "source_process_pid",
            "source_process_started_at_ms",
            "waiting_reason",
            "waiting_deadline_ms",
            "control_used_at_ms",
            "deletion_requested_at_ms",
            "deletion_failure_code",
        ]
    );
    assert_eq!(
        column_names(&connection, "managed_launch_registrations"),
        [
            "registration_id",
            "status",
            "provider",
            "working_directory",
            "session_alias",
            "description",
            "tmux_socket_name",
            "tmux_session_name",
            "created_at_ms",
            "launched_at_ms",
            "tmux_pane_id",
            "provider_pid",
            "provider_started_at_ms",
            "terminal_opened_at_ms",
            "terminal_error",
            "binding_error",
            "binding_initializer",
        ]
    );
    assert_eq!(
        column_names(&connection, "binding_receipts"),
        [
            "receipt_id",
            "registration_id",
            "provider",
            "native_session_id",
            "source_path",
            "source_format",
            "provider_pid",
            "provider_started_at_ms",
            "issued_at_ms",
        ]
    );
    assert_eq!(
        column_names(&connection, "managed_sessions"),
        [
            "provider",
            "native_session_id",
            "working_directory",
            "session_alias",
            "description",
            "binding_receipt_id",
        ]
    );
    assert_eq!(
        column_names(&connection, "managed_current_runs"),
        [
            "run_id",
            "opaque_ref",
            "provider",
            "native_session_id",
            "provider_pid",
            "provider_started_at_ms",
            "tmux_socket_name",
            "tmux_session_name",
            "binding_receipt_id",
        ]
    );
    assert_eq!(
        column_names(&connection, "managed_resume_registrations"),
        [
            "registration_id",
            "provider",
            "native_session_id",
            "prior_run_id",
        ]
    );
    assert_eq!(
        column_names(&connection, "managed_metadata_syncs"),
        ["provider", "native_session_id"]
    );
    assert_eq!(
        column_names(&connection, "managed_launch_title_associations"),
        ["registration_id", "provider", "native_session_id"]
    );
    assert_eq!(
        column_names(&connection, "session_metadata"),
        [
            "provider",
            "native_session_id",
            "alias",
            "description",
            "disposition",
        ]
    );

    assert_eq!(
        named_indexes(&connection),
        ["one_non_terminal_task_per_session"]
    );
    assert_eq!(
        foreign_keys(&connection, "managed_sessions"),
        ["binding_receipts.binding_receipt_id -> receipt_id ON DELETE RESTRICT"]
    );
    assert_eq!(
        foreign_keys(&connection, "managed_current_runs"),
        ["binding_receipts.binding_receipt_id -> receipt_id ON DELETE RESTRICT"]
    );
    assert_eq!(
        foreign_keys(&connection, "managed_resume_registrations"),
        [
            "managed_current_runs.prior_run_id -> run_id ON DELETE RESTRICT",
            "managed_launch_registrations.registration_id -> registration_id ON DELETE CASCADE",
        ]
    );
    assert_eq!(
        foreign_keys(&connection, "managed_metadata_syncs"),
        [
            "managed_sessions.native_session_id -> native_session_id ON DELETE CASCADE",
            "managed_sessions.provider -> provider ON DELETE CASCADE",
        ]
    );
    assert_eq!(
        foreign_keys(&connection, "managed_launch_title_associations"),
        ["managed_launch_registrations.registration_id -> registration_id ON DELETE CASCADE"]
    );
}

#[test]
fn first_state_database_cutover_backs_up_the_complete_legacy_set() {
    let temp = TempDir::new("legacy-cutover");
    let data_dir = temp.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    let legacy_files = [
        ("tasks.db", b"task-private-content".as_slice()),
        ("managed-launches.db", b"managed-private-content".as_slice()),
        ("session-labels.json", b"label-private-content".as_slice()),
        ("index.db", b"disposable-index-content".as_slice()),
    ];
    for (name, contents) in legacy_files {
        std::fs::write(data_dir.join(name), contents).unwrap();
    }
    let artifact_root = data_dir.join("handoff-artifacts");
    std::fs::create_dir(&artifact_root).unwrap();
    std::fs::write(artifact_root.join("artifact.md"), b"private artifact").unwrap();

    let database = StateDatabase::open_data_dir(&data_dir).unwrap();

    assert_eq!(database.path(), data_dir.join("state.db"));
    let backup = database.legacy_backup().expect("legacy backup");
    assert_eq!(
        backup.parent(),
        Some(data_dir.join("legacy-reset").as_path())
    );
    for (name, contents) in legacy_files {
        if name != "index.db" {
            assert!(!data_dir.join(name).exists());
        }
        assert_eq!(std::fs::read(backup.join(name)).unwrap(), contents);
    }
    assert!(!artifact_root.exists());
    assert!(data_dir.join("index.db").exists());
    assert_eq!(
        std::fs::read(backup.join("handoff-artifacts/artifact.md")).unwrap(),
        b"private artifact"
    );
    assert_eq!(pragma_i64(&database.connect().unwrap(), "user_version"), 1);

    #[cfg(unix)]
    {
        assert_eq!(
            std::fs::metadata(backup).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for private_file in [
            backup.join("tasks.db"),
            backup.join("managed-launches.db"),
            backup.join("session-labels.json"),
            backup.join("index.db"),
            backup.join("handoff-artifacts/artifact.md"),
        ] {
            assert_eq!(
                std::fs::metadata(private_file)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn failed_cutover_after_legacy_moves_restores_every_source_path() {
    let temp = TempDir::new("cutover-rollback");
    let data_dir = temp.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    let task_path = data_dir.join("tasks.db");
    std::fs::write(&task_path, b"task-state").unwrap();
    let artifact_root = data_dir.join("handoff-artifacts");
    std::fs::create_dir(&artifact_root).unwrap();
    std::fs::write(artifact_root.join("artifact.md"), b"artifact").unwrap();
    std::os::unix::fs::symlink("missing-parent/index.db", data_dir.join("index.db")).unwrap();

    assert!(StateDatabase::open_data_dir(&data_dir).is_err());

    assert_eq!(std::fs::read(task_path).unwrap(), b"task-state");
    assert_eq!(
        std::fs::read(artifact_root.join("artifact.md")).unwrap(),
        b"artifact"
    );
    assert!(std::fs::symlink_metadata(data_dir.join("state.db")).is_err());
    assert!(std::fs::symlink_metadata(data_dir.join("index.db")).is_err());
    assert!(!data_dir.join("legacy-reset").exists());
}

#[test]
fn unknown_state_database_version_fails_without_modifying_the_database() {
    let temp = TempDir::new("unknown-version");
    let path = temp.path().join("state.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    assert_eq!(journal_mode(&connection), "delete");
    drop(connection);
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

    let error = StateDatabase::open(&path).unwrap_err();

    assert!(error
        .to_string()
        .contains("unsupported State Database schema version 99"));
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(pragma_i64(&connection, "user_version"), 99);
    assert_eq!(journal_mode(&connection), "delete");
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn task_store_uses_the_versioned_state_database_schema() {
    let temp = TempDir::new("task-repository-schema");
    let path = temp.path().join("state.db");

    drop(TaskStore::open(&path).unwrap());

    let connection = StateDatabase::open(&path).unwrap().connect().unwrap();
    assert_eq!(pragma_i64(&connection, "user_version"), 1);
    assert_eq!(table_count(&connection, "operation_tasks"), 1);
    assert_eq!(table_count(&connection, "managed_sessions"), 1);
    assert_eq!(table_count(&connection, "session_metadata"), 1);
}

#[test]
fn managed_store_uses_the_versioned_state_database_schema() {
    let temp = TempDir::new("managed-repository-schema");
    let path = temp.path().join("state.db");

    drop(ManagedLaunchStore::open(&path).unwrap());

    let connection = StateDatabase::open(&path).unwrap().connect().unwrap();
    assert_eq!(pragma_i64(&connection, "user_version"), 1);
    assert_eq!(table_count(&connection, "managed_launch_registrations"), 1);
    assert_eq!(table_count(&connection, "operation_tasks"), 1);
    assert_eq!(table_count(&connection, "session_metadata"), 1);
}

#[test]
fn session_labels_use_the_versioned_state_database_schema() {
    let temp = TempDir::new("session-metadata-repository-schema");
    let path = temp.path().join("state.db");
    let identity = SessionIdentity {
        provider: Provider::ClaudeCode,
        native_id: "claude-session".into(),
    };

    let mut labels = SessionLabels::load(&path);
    labels
        .set_metadata(
            identity.provider,
            &identity.native_id,
            "Architecture work",
            "Stabilize authoritative State",
        )
        .unwrap();
    labels
        .hide_sessions(std::slice::from_ref(&identity))
        .unwrap();

    let reloaded = SessionLabels::load(&path);
    assert_eq!(
        reloaded.alias(identity.provider, &identity.native_id),
        Some("Architecture work")
    );
    assert_eq!(
        reloaded.description(identity.provider, &identity.native_id),
        Some("Stabilize authoritative State")
    );
    assert_eq!(
        reloaded.disposition(identity.provider, &identity.native_id),
        Some(CurationDisposition::Hidden)
    );
    let connection = StateDatabase::open(&path).unwrap().connect().unwrap();
    assert_eq!(pragma_i64(&connection, "user_version"), 1);
    assert_eq!(table_count(&connection, "operation_tasks"), 1);
    assert_eq!(table_count(&connection, "session_metadata"), 1);
}

#[test]
fn failed_state_transaction_rolls_back_every_authoritative_write() {
    let temp = TempDir::new("transaction-rollback");
    let path = temp.path().join("state.db");
    let database = StateDatabase::open(&path).unwrap();
    let mut connection = database.connect().unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO session_metadata (provider, native_session_id, alias) \
             VALUES ('claude', 'first-session', 'First')",
            [],
        )
        .unwrap();

    let violation = transaction.execute(
        "INSERT INTO session_metadata (provider, native_session_id, alias, disposition) \
         VALUES ('codex', 'excluded-session', 'Forbidden', 'permanently_excluded')",
        [],
    );
    assert!(violation.is_err());
    transaction.rollback().unwrap();

    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM session_metadata", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn wal_readers_see_only_committed_state_and_use_the_busy_timeout() {
    let temp = TempDir::new("wal-visibility");
    let path = temp.path().join("state.db");
    let database = StateDatabase::open(&path).unwrap();
    let mut writer = database.connect().unwrap();
    let reader = database.connect().unwrap();
    assert_eq!(journal_mode(&writer), "wal");
    assert_eq!(journal_mode(&reader), "wal");
    assert_eq!(pragma_i64(&writer, "busy_timeout"), 5_000);
    assert_eq!(pragma_i64(&reader, "busy_timeout"), 5_000);
    #[cfg(unix)]
    for sidecar in [path.with_extension("db-wal"), path.with_extension("db-shm")] {
        assert_eq!(
            std::fs::metadata(sidecar).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let transaction = writer.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO session_metadata (provider, native_session_id, alias) \
             VALUES ('claude', 'wal-session', 'Visible after commit')",
            [],
        )
        .unwrap();
    assert_eq!(session_metadata_count(&reader), 0);

    transaction.commit().unwrap();
    assert_eq!(session_metadata_count(&reader), 1);
}

#[test]
fn state_schema_allows_only_one_non_terminal_task_per_session() {
    let temp = TempDir::new("non-terminal-task-constraint");
    let path = temp.path().join("state.db");
    let connection = StateDatabase::open(&path).unwrap().connect().unwrap();

    insert_task(&connection, "queued-task", "queued").unwrap();
    assert!(insert_task(&connection, "running-task", "running").is_err());
    insert_task(&connection, "ready-task", "ready").unwrap();

    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM operation_tasks", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn initialized_state_database_passes_integrity_check() {
    let temp = TempDir::new("integrity-check");
    let path = temp.path().join("state.db");
    let mut labels = SessionLabels::load(&path);
    labels
        .set_alias(Provider::CodexCli, "codex-session", "Integrity check")
        .unwrap();
    drop(TaskStore::open(&path).unwrap());
    drop(ManagedLaunchStore::open(&path).unwrap());

    let connection = StateDatabase::open(&path).unwrap().connect().unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

fn pragma_i64(connection: &rusqlite::Connection, name: &str) -> i64 {
    connection
        .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
        .unwrap()
}

fn table_count(connection: &rusqlite::Connection, table: &str) -> i64 {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .unwrap()
}

fn column_names(connection: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    statement
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn named_indexes(connection: &rusqlite::Connection) -> Vec<String> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'index' AND name NOT LIKE 'sqlite_autoindex_%' ORDER BY name",
        )
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn foreign_keys(connection: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA foreign_key_list({table})"))
        .unwrap();
    let mut keys = statement
        .query_map([], |row| {
            Ok(format!(
                "{}.{} -> {} ON DELETE {}",
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(6)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    keys.sort();
    keys
}

fn session_metadata_count(connection: &rusqlite::Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM session_metadata", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn journal_mode(connection: &rusqlite::Connection) -> String {
    connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap()
}

fn insert_task(
    connection: &rusqlite::Connection,
    task_id: &str,
    state: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO operation_tasks (
             task_id, state, source_provider, source_session_id,
             source_record_path, source_working_directory,
             created_at_ms, state_changed_at_ms
         ) VALUES (?1, ?2, 'claude', 'shared-session', '/source', '/project', 1, 1)",
        [task_id, state],
    )
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(test_name: &str) -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mudi-state-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
