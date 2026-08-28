//! Versioned storage for all Mudi-owned authoritative metadata.

pub mod bindings;
pub mod managed_sessions;

use crate::adapters::persistence::index::Index;
use anyhow::{anyhow, Context, Result};
use rusqlite::{Connection, TransactionBehavior};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

pub const STATE_SCHEMA_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
static NEXT_BACKUP: AtomicU64 = AtomicU64::new(0);

const LEGACY_PATHS: &[&str] = &[
    "tasks.db",
    "tasks.db-wal",
    "tasks.db-shm",
    "managed-launches.db",
    "managed-launches.db-wal",
    "managed-launches.db-shm",
    "session-labels.json",
    "index.db",
    "index.db-wal",
    "index.db-shm",
    "handoff-artifacts",
];

const STATE_SCHEMA: &str = r#"
CREATE TABLE operation_tasks (
    task_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    source_provider TEXT NOT NULL,
    source_session_id TEXT NOT NULL,
    source_record_path TEXT NOT NULL,
    source_working_directory TEXT NOT NULL,
    successor_provider TEXT,
    successor_executable TEXT,
    created_at_ms INTEGER NOT NULL,
    state_changed_at_ms INTEGER NOT NULL,
    worker_pid INTEGER,
    worker_started_at_ms INTEGER,
    provider_pid INTEGER,
    provider_started_at_ms INTEGER,
    prompt_sent_at_ms INTEGER,
    provider_exited_at_ms INTEGER,
    provider_exit_code INTEGER,
    running_deadline_ms INTEGER,
    cancellation_requested_at_ms INTEGER,
    termination_requested_at_ms INTEGER,
    retry_of_task_id TEXT,
    retry_ready_at_ms INTEGER,
    artifact_path TEXT,
    plan_provider TEXT,
    plan_working_directory TEXT,
    plan_executable TEXT,
    plan_arguments_json TEXT,
    plan_artifact_path TEXT,
    successor_launch_requested_at_ms INTEGER,
    successor_launch_failed_at_ms INTEGER,
    successor_launch_failure_code TEXT,
    successor_pid INTEGER,
    successor_process_started_at_ms INTEGER,
    successor_started_at_ms INTEGER,
    successor_exited_at_ms INTEGER,
    successor_exit_code INTEGER,
    successor_session_id TEXT,
    successor_linked_at_ms INTEGER,
    failure_code TEXT,
    acknowledged_at_ms INTEGER,
    end_current_run_authorized INTEGER NOT NULL DEFAULT 0,
    source_process_pid INTEGER,
    source_process_started_at_ms INTEGER,
    waiting_reason TEXT,
    waiting_deadline_ms INTEGER,
    control_used_at_ms INTEGER,
    deletion_requested_at_ms INTEGER,
    deletion_failure_code TEXT
);

CREATE UNIQUE INDEX one_non_terminal_task_per_session
ON operation_tasks(source_provider, source_session_id)
WHERE state IN ('queued', 'waiting', 'running');

CREATE TABLE managed_launch_registrations (
    registration_id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    provider TEXT NOT NULL,
    working_directory BLOB NOT NULL,
    session_alias TEXT NOT NULL,
    description TEXT,
    tmux_socket_name TEXT NOT NULL,
    tmux_session_name TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    launched_at_ms INTEGER,
    tmux_pane_id TEXT,
    provider_pid INTEGER,
    provider_started_at_ms INTEGER,
    terminal_opened_at_ms INTEGER,
    terminal_error TEXT,
    binding_error TEXT,
    binding_initializer INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE binding_receipts (
    receipt_id TEXT PRIMARY KEY,
    registration_id TEXT NOT NULL UNIQUE,
    provider TEXT NOT NULL,
    native_session_id TEXT NOT NULL,
    source_path BLOB NOT NULL,
    source_format TEXT NOT NULL,
    provider_pid INTEGER NOT NULL,
    provider_started_at_ms INTEGER NOT NULL,
    issued_at_ms INTEGER NOT NULL
);

CREATE TABLE managed_sessions (
    provider TEXT NOT NULL,
    native_session_id TEXT NOT NULL,
    working_directory BLOB NOT NULL,
    session_alias TEXT NOT NULL,
    description TEXT,
    binding_receipt_id TEXT NOT NULL UNIQUE
        REFERENCES binding_receipts(receipt_id) ON DELETE RESTRICT,
    PRIMARY KEY (provider, native_session_id)
);

CREATE TABLE managed_current_runs (
    run_id TEXT PRIMARY KEY,
    opaque_ref TEXT NOT NULL UNIQUE,
    provider TEXT NOT NULL,
    native_session_id TEXT NOT NULL,
    provider_pid INTEGER NOT NULL,
    provider_started_at_ms INTEGER NOT NULL,
    tmux_socket_name TEXT NOT NULL,
    tmux_session_name TEXT NOT NULL,
    binding_receipt_id TEXT NOT NULL UNIQUE
        REFERENCES binding_receipts(receipt_id) ON DELETE RESTRICT
);

CREATE TABLE managed_resume_registrations (
    registration_id TEXT PRIMARY KEY
        REFERENCES managed_launch_registrations(registration_id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    native_session_id TEXT NOT NULL,
    prior_run_id TEXT NOT NULL
        REFERENCES managed_current_runs(run_id) ON DELETE RESTRICT,
    UNIQUE (provider, native_session_id)
);

CREATE TABLE managed_metadata_syncs (
    provider TEXT NOT NULL,
    native_session_id TEXT NOT NULL,
    PRIMARY KEY (provider, native_session_id),
    FOREIGN KEY (provider, native_session_id)
        REFERENCES managed_sessions(provider, native_session_id) ON DELETE CASCADE
);

CREATE TABLE managed_launch_title_associations (
    registration_id TEXT PRIMARY KEY
        REFERENCES managed_launch_registrations(registration_id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    native_session_id TEXT NOT NULL
);

CREATE TABLE session_metadata (
    provider TEXT NOT NULL,
    native_session_id TEXT NOT NULL,
    alias TEXT,
    description TEXT,
    disposition TEXT CHECK (
        disposition IS NULL OR disposition IN ('hidden', 'permanently_excluded')
    ),
    PRIMARY KEY (provider, native_session_id),
    CHECK (
        disposition != 'permanently_excluded'
        OR (alias IS NULL AND description IS NULL)
    )
);
"#;

#[derive(Debug, Clone)]
pub struct StateDatabase {
    path: PathBuf,
    legacy_backup: Option<PathBuf>,
}

impl StateDatabase {
    pub fn open_data_dir(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        let state_path = data_dir.join("state.db");
        if state_path.exists() {
            return Self::open(state_path);
        }

        prepare_private_parent(&state_path)?;
        let sources = LEGACY_PATHS
            .iter()
            .map(|name| data_dir.join(name))
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        if sources.is_empty() {
            return Self::open(state_path);
        }

        let backup_root = data_dir.join("legacy-reset");
        create_private_directory(&backup_root)?;
        let backup = backup_root.join(backup_name());
        create_private_directory(&backup)?;
        let mut moved = Vec::with_capacity(sources.len());
        for source in sources {
            let destination = backup.join(
                source
                    .file_name()
                    .ok_or_else(|| anyhow!("legacy State path has no file name"))?,
            );
            if let Err(error) = fs::rename(&source, &destination) {
                rollback_moves(&moved, &backup, &backup_root)?;
                return Err(error).with_context(|| {
                    format!("could not back up legacy State path {}", source.display())
                });
            }
            moved.push((source, destination));
        }
        if let Err(error) = secure_backup_tree(&backup) {
            rollback_moves(&moved, &backup, &backup_root)?;
            return Err(error);
        }

        let mut database = match Self::open(&state_path) {
            Ok(database) => database,
            Err(error) => {
                remove_database_files(&state_path);
                rollback_moves(&moved, &backup, &backup_root)?;
                return Err(error);
            }
        };
        let index_path = data_dir.join("index.db");
        if let Err(error) = Index::open(&index_path) {
            remove_database_files(&state_path);
            remove_database_files(&index_path);
            rollback_moves(&moved, &backup, &backup_root)?;
            return Err(error).context("could not create fresh disposable Session index");
        }
        database.legacy_backup = Some(backup);
        Ok(database)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let existed = path.exists();
        if !existed {
            prepare_private_parent(&path)?;
        }
        let mut connection = match open_connection(&path) {
            Ok(connection) => connection,
            Err(error) => {
                if !existed {
                    remove_database_files(&path);
                }
                return Err(error);
            }
        };
        if let Err(error) = initialize_schema(&mut connection) {
            drop(connection);
            if !existed {
                remove_database_files(&path);
            }
            return Err(error);
        }
        prepare_private_parent(&path)?;
        set_private_file_permissions(&path)?;
        Ok(Self {
            path,
            legacy_backup: None,
        })
    }

    pub fn connect(&self) -> Result<Connection> {
        let connection = open_connection(&self.path)?;
        let version = schema_version(&connection)?;
        if version != STATE_SCHEMA_VERSION {
            return Err(unsupported_version(version));
        }
        configure_connection(&connection)?;
        set_private_file_permissions(&self.path)?;
        Ok(connection)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn legacy_backup(&self) -> Option<&Path> {
        self.legacy_backup.as_deref()
    }
}

fn prepare_private_parent(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        create_private_directory(parent)?;
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).with_context(|| {
        format!(
            "could not secure State Database directory {}",
            parent.display()
        )
    })?;
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(path)
            .with_context(|| format!("could not create private directory {}", path.display()))?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("could not secure private directory {}", path.display()))
}

fn backup_name() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let sequence = NEXT_BACKUP.fetch_add(1, Ordering::Relaxed);
    format!("{timestamp}-{}-{sequence}", std::process::id())
}

fn secure_backup_tree(root: &Path) -> Result<()> {
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        fs::set_permissions(entry.path(), fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn rollback_moves(moved: &[(PathBuf, PathBuf)], backup: &Path, backup_root: &Path) -> Result<()> {
    for (source, destination) in moved.iter().rev() {
        fs::rename(destination, source).with_context(|| {
            format!(
                "could not restore legacy State path {} from {}",
                source.display(),
                destination.display()
            )
        })?;
    }
    fs::remove_dir(backup)?;
    if fs::read_dir(backup_root)?.next().is_none() {
        fs::remove_dir(backup_root)?;
    }
    Ok(())
}

fn remove_database_files(path: &Path) {
    for candidate in [
        path.to_path_buf(),
        sidecar_path(path, "-wal"),
        sidecar_path(path, "-shm"),
    ] {
        let _ = fs::remove_file(candidate);
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

fn open_connection(path: &Path) -> Result<Connection> {
    let connection = Connection::open(path)
        .with_context(|| format!("could not open State Database {}", path.display()))?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(connection)
}

fn initialize_schema(connection: &mut Connection) -> Result<()> {
    let version = schema_version(connection)?;
    match version {
        STATE_SCHEMA_VERSION => return configure_connection(connection),
        0 if user_table_count(connection)? == 0 => {}
        0 => {
            let version_after_table_read = schema_version(connection)?;
            if version_after_table_read == STATE_SCHEMA_VERSION {
                return configure_connection(connection);
            }
            return Err(unversioned_database());
        }
        _ => return Err(unsupported_version(version)),
    }

    configure_connection(connection)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version = schema_version(&transaction)?;
    match version {
        STATE_SCHEMA_VERSION => {}
        0 => {
            if user_table_count(&transaction)? != 0 {
                return Err(unversioned_database());
            }
            transaction.execute_batch(STATE_SCHEMA)?;
            transaction.pragma_update(None, "user_version", STATE_SCHEMA_VERSION)?;
        }
        _ => return Err(unsupported_version(version)),
    }
    transaction.commit()?;
    Ok(())
}

fn configure_connection(connection: &Connection) -> Result<()> {
    let journal_mode =
        connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        let started = Instant::now();
        loop {
            match connection.query_row("PRAGMA journal_mode = WAL", [], |row| {
                row.get::<_, String>(0)
            }) {
                Ok(_) => break,
                Err(error) if is_lock_error(&error) && started.elapsed() < BUSY_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    connection.execute_batch("PRAGMA synchronous = FULL;")?;
    Ok(())
}

fn is_lock_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(sqlite_error, _)
            if matches!(
                sqlite_error.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
}

fn user_table_count(connection: &Connection) -> Result<i64> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn schema_version(connection: &Connection) -> Result<i64> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(Into::into)
}

fn unsupported_version(version: i64) -> anyhow::Error {
    anyhow!("unsupported State Database schema version {version}; expected {STATE_SCHEMA_VERSION}")
}

fn unversioned_database() -> anyhow::Error {
    anyhow!("unversioned State Database is not supported; explicit reset is required")
}

fn set_private_file_permissions(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("could not secure State Database {}", path.display()))
}
