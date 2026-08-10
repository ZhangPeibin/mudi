//! Explicitly authorized Managed Session launch registration and native New startup.

use crate::domain::{
    ProjectIdentity, Provider, Session, SessionIdentity, SessionProvenance, SourceFormat,
    SourceMetadata,
};
use crate::providers::state::ProcessIdentity;
use crate::providers::{registry, ProviderBindingEvidence, ProviderExecutables};
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

static REGISTRATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedLaunchStatus {
    Binding,
    NeedsReview,
    Failed,
}

impl ManagedLaunchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::NeedsReview => "needs_review",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "binding" => Ok(Self::Binding),
            "needs_review" => Ok(Self::NeedsReview),
            "failed" => Ok(Self::Failed),
            _ => Err(anyhow!(
                "unknown Managed Launch Registration status {value:?}"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLaunchTarget {
    pub project: ProjectIdentity,
    pub project_label: String,
    pub worktree_root: Option<PathBuf>,
    pub working_directory: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedNewRequest {
    pub provider: Provider,
    pub target: ManagedLaunchTarget,
    pub title: String,
    pub description: Option<String>,
    pub binding_initializer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderNewCommand {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateTmuxEnvironment {
    pub socket_name: String,
    pub session_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLaunchRegistration {
    pub id: String,
    pub status: ManagedLaunchStatus,
    pub provider: Provider,
    pub target: ManagedLaunchTarget,
    pub session_alias: String,
    pub description: Option<String>,
    pub binding_initializer: bool,
    pub provider_command: ProviderNewCommand,
    pub tmux: PrivateTmuxEnvironment,
    pub created_at_ms: i64,
    pub launched_at_ms: Option<i64>,
    pub launch_evidence: Option<ManagedLaunchEvidence>,
    pub terminal_opened_at_ms: Option<i64>,
    pub terminal_error: Option<String>,
    pub binding_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedLaunchAction {
    CancelStart,
    OpenTerminal,
    RetryOpenTerminal,
    CopyDiagnostics,
    Resume,
    RetryBinding,
    AdoptAsManaged,
}

impl ManagedLaunchRegistration {
    pub fn available_actions(&self) -> Vec<ManagedLaunchAction> {
        match (self.status, self.terminal_error.is_some()) {
            (ManagedLaunchStatus::Binding, false) => {
                vec![ManagedLaunchAction::CancelStart]
            }
            (ManagedLaunchStatus::Binding, true) => vec![
                ManagedLaunchAction::CancelStart,
                ManagedLaunchAction::RetryOpenTerminal,
            ],
            (ManagedLaunchStatus::NeedsReview, false) => vec![
                ManagedLaunchAction::OpenTerminal,
                ManagedLaunchAction::CopyDiagnostics,
            ],
            (ManagedLaunchStatus::NeedsReview, true) => vec![
                ManagedLaunchAction::RetryOpenTerminal,
                ManagedLaunchAction::CopyDiagnostics,
            ],
            (ManagedLaunchStatus::Failed, _) => Vec::new(),
        }
    }

    pub fn allows(&self, action: ManagedLaunchAction) -> bool {
        self.available_actions().contains(&action)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLaunchEvidence {
    pub tmux_pane_id: String,
    pub provider_process: ProcessIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedNewOutcome {
    pub registration: ManagedLaunchRegistration,
    pub terminal_opened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedTerminalOutcome {
    pub registration: ManagedLaunchRegistration,
    pub opened: bool,
}

/// Conclusive record that atomically established a Managed Session and Current Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingReceipt {
    pub id: String,
    pub registration_id: String,
    pub provider: Provider,
    pub native_session_id: String,
    pub source: SourceMetadata,
    pub provider_process: ProcessIdentity,
    pub issued_at_ms: i64,
}

/// Durable management metadata for one provider-native Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSessionRecord {
    pub provider: Provider,
    pub native_session_id: String,
    pub target: ManagedLaunchTarget,
    pub session_alias: String,
    pub description: Option<String>,
    pub binding_receipt_id: String,
}

/// Durable identity of the exact Current Run established by a Binding Receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedCurrentRunRecord {
    pub id: String,
    pub provider: Provider,
    pub native_session_id: String,
    pub provider_process: ProcessIdentity,
    pub tmux: PrivateTmuxEnvironment,
    pub binding_receipt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingOutcome {
    Waiting,
    Bound(BindingReceipt),
    NeedsReview,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProcessObservation {
    Live(ProcessIdentity),
    Absent,
    Unknown,
}

/// Revalidates exact process identity immediately before receipt issuance.
pub trait ProcessEvidence: Send + Sync {
    fn observe(&self, pid: u32) -> ProviderProcessObservation;
}

pub struct SystemProcessEvidence;

impl SystemProcessEvidence {
    pub fn current_identity(&self, pid: u32) -> Option<ProcessIdentity> {
        match self.observe(pid) {
            ProviderProcessObservation::Live(identity) => Some(identity),
            ProviderProcessObservation::Absent | ProviderProcessObservation::Unknown => None,
        }
    }
}

impl ProcessEvidence for SystemProcessEvidence {
    fn observe(&self, pid: u32) -> ProviderProcessObservation {
        match crate::providers::state::inspect_process_identity(pid) {
            crate::providers::state::ProcessInspection::Present(identity, false) => {
                ProviderProcessObservation::Live(identity)
            }
            crate::providers::state::ProcessInspection::Present(_, true)
            | crate::providers::state::ProcessInspection::Absent => {
                ProviderProcessObservation::Absent
            }
            crate::providers::state::ProcessInspection::Unknown => {
                ProviderProcessObservation::Unknown
            }
        }
    }
}

pub trait ManagedLaunchCleanup: Send + Sync {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> Result<()>;
}

pub struct TmuxManagedLaunchCleanup {
    tmux_executable: PathBuf,
}

impl TmuxManagedLaunchCleanup {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self { tmux_executable }
    }
}

impl ManagedLaunchCleanup for TmuxManagedLaunchCleanup {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> Result<()> {
        clean_empty_private_tmux(&self.tmux_executable, &registration.tmux)
    }
}

/// Sole issuer of Binding Receipts.
pub struct EvidenceBinder {
    process_evidence: Box<dyn ProcessEvidence>,
    cleanup: Box<dyn ManagedLaunchCleanup>,
}

impl EvidenceBinder {
    pub fn new(
        process_evidence: Box<dyn ProcessEvidence>,
        cleanup: Box<dyn ManagedLaunchCleanup>,
    ) -> Self {
        Self {
            process_evidence,
            cleanup,
        }
    }

    pub fn bind(
        &self,
        store: &mut ManagedLaunchStore,
        registration_id: &str,
        evidence: Option<ProviderBindingEvidence>,
        now_ms: i64,
    ) -> Result<BindingOutcome> {
        let registration = store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Binding Managed Launch Registration {registration_id} is unavailable")
        })?;
        let launch = registration
            .launch_evidence
            .as_ref()
            .ok_or_else(|| anyhow!("Binding requires conclusive launch evidence"))?;

        if registration.status != ManagedLaunchStatus::Binding {
            return Err(anyhow!("only a Binding registration can issue a Receipt"));
        }
        match self.process_evidence.observe(launch.provider_process.pid) {
            ProviderProcessObservation::Absent => {
                if let Err(error) = self.cleanup.clean_empty(&registration) {
                    store.mark_needs_review(
                        registration_id,
                        &format!("empty private tmux could not be confirmed: {error}"),
                    )?;
                    return Ok(BindingOutcome::NeedsReview);
                }
                store.mark_failed(registration_id, "Provider process is confirmed absent")?;
                return Ok(BindingOutcome::Failed);
            }
            ProviderProcessObservation::Unknown => {
                store.mark_needs_review(
                    registration_id,
                    "launched Provider process could not be verified",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(identity) if identity != launch.provider_process => {
                store.mark_needs_review(
                    registration_id,
                    "launched Provider process is stale or its PID was reused",
                )?;
                return Ok(BindingOutcome::NeedsReview);
            }
            ProviderProcessObservation::Live(_) => {}
        }
        let Some(evidence) = evidence else {
            return Ok(BindingOutcome::Waiting);
        };
        let receipt = match binding_receipt_candidate(&registration, evidence, now_ms) {
            Ok(receipt) => receipt,
            Err(error) => {
                store.mark_needs_review(registration_id, &error.to_string())?;
                return Ok(BindingOutcome::NeedsReview);
            }
        };
        if let Err(error) = store.establish_management(&registration, &receipt) {
            store.mark_needs_review(registration_id, &error.to_string())?;
            return Ok(BindingOutcome::NeedsReview);
        }
        Ok(BindingOutcome::Bound(receipt))
    }
}

fn binding_receipt_candidate(
    registration: &ManagedLaunchRegistration,
    evidence: ProviderBindingEvidence,
    now_ms: i64,
) -> Result<BindingReceipt> {
    let launch = registration
        .launch_evidence
        .as_ref()
        .ok_or_else(|| anyhow!("Binding requires conclusive launch evidence"))?;
    if evidence.provider != registration.provider {
        return Err(anyhow!(
            "Provider source evidence disagrees with the launch"
        ));
    }
    if evidence.source_session_id.is_empty()
        || evidence.process_session_id.is_empty()
        || evidence.source_session_id != evidence.process_session_id
    {
        return Err(anyhow!(
            "native Session ID evidence is incomplete or conflicting"
        ));
    }
    if evidence.provider_process != launch.provider_process {
        return Err(anyhow!(
            "Provider process evidence disagrees with the launch"
        ));
    }
    let source_path = evidence
        .source
        .path
        .canonicalize()
        .context("Binding Provider source is unavailable")?;
    if !source_path.is_file() {
        return Err(anyhow!("Binding Provider source is not a file"));
    }
    let working_directory = evidence
        .working_directory
        .canonicalize()
        .context("Binding working directory is unavailable")?;
    let target_directory = registration
        .target
        .working_directory
        .canonicalize()
        .context("Managed launch target is unavailable")?;
    if working_directory != target_directory {
        return Err(anyhow!(
            "Provider source working directory disagrees with the launch"
        ));
    }
    let resolved = crate::project::resolve(Some(&working_directory));
    if resolved.project.as_ref().map(|project| &project.identity)
        != Some(&registration.target.project)
        || resolved.worktree.as_ref().map(|worktree| &worktree.root)
            != registration.target.worktree_root.as_ref()
    {
        return Err(anyhow!(
            "Provider source Project or Worktree disagrees with the launch"
        ));
    }
    Ok(BindingReceipt {
        id: format!("receipt-{}", registration.id),
        registration_id: registration.id.clone(),
        provider: registration.provider,
        native_session_id: evidence.source_session_id,
        source: SourceMetadata {
            path: source_path,
            format: evidence.source.format,
        },
        provider_process: launch.provider_process,
        issued_at_ms: now_ms,
    })
}

pub trait ManagedLaunchBroker {
    fn launch(&mut self, registration: &ManagedLaunchRegistration)
        -> Result<ManagedLaunchEvidence>;

    fn cancel(&mut self, _registration: &ManagedLaunchRegistration) -> Result<()> {
        Err(anyhow!("Managed launch cancellation is unavailable"))
    }
}

pub trait ManagedTerminalAccess {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> Result<()>;
}

pub struct ManagedSessionService {
    store: ManagedLaunchStore,
    executables: ProviderExecutables,
    launch_broker: Box<dyn ManagedLaunchBroker>,
    terminal_access: Box<dyn ManagedTerminalAccess>,
}

impl ManagedSessionService {
    pub fn new(
        store: ManagedLaunchStore,
        executables: ProviderExecutables,
        launch_broker: Box<dyn ManagedLaunchBroker>,
        terminal_access: Box<dyn ManagedTerminalAccess>,
    ) -> Self {
        Self {
            store,
            executables,
            launch_broker,
            terminal_access,
        }
    }

    pub fn start_new(
        &mut self,
        mut request: ManagedNewRequest,
        now_ms: i64,
    ) -> Result<ManagedNewOutcome> {
        if request.title.trim().is_empty() {
            return Err(anyhow!("New requires a Title"));
        }
        request.title = crate::session_labels::validate_alias(&request.title)?;
        request.description = crate::session_labels::validate_description(
            request.description.as_deref().unwrap_or_default(),
        )?;
        validate_target(&request.target)?;

        let executable = self
            .executables
            .executable_for(request.provider)
            .ok_or_else(|| anyhow!("{} executable is unavailable", request.provider.label()))?
            .to_path_buf();
        validate_executable(&executable, request.provider.label())?;
        let id = new_registration_id(now_ms);
        let registration = ManagedLaunchRegistration {
            id: id.clone(),
            status: ManagedLaunchStatus::Binding,
            provider: request.provider,
            target: request.target,
            session_alias: request.title,
            description: request.description,
            binding_initializer: request.binding_initializer,
            provider_command: ProviderNewCommand {
                executable,
                arguments: registry()
                    .runtime_control(request.provider)?
                    .new_session_args(request.binding_initializer),
            },
            tmux: PrivateTmuxEnvironment {
                socket_name: format!("sessionmesh-{id}"),
                session_name: "managed".to_string(),
            },
            created_at_ms: now_ms,
            launched_at_ms: None,
            launch_evidence: None,
            terminal_opened_at_ms: None,
            terminal_error: None,
            binding_error: None,
        };

        self.store.insert(&registration)?;
        let launch_evidence = self
            .launch_broker
            .launch(&registration)
            .context("Managed Provider launch failed")?;
        self.store
            .record_launched(&registration.id, now_ms, launch_evidence)?;
        let launched = self
            .store
            .get(&registration.id)?
            .ok_or_else(|| anyhow!("Managed Launch Registration disappeared after launch"))?;

        let terminal_opened = match self.terminal_access.open(&launched) {
            Ok(()) => {
                self.store
                    .record_terminal_opened(&registration.id, now_ms)?;
                true
            }
            Err(error) => {
                self.store
                    .record_terminal_error(&registration.id, &error.to_string())?;
                false
            }
        };
        let registration = self.store.get(&registration.id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration disappeared after Terminal access")
        })?;
        Ok(ManagedNewOutcome {
            registration,
            terminal_opened,
        })
    }

    pub fn cancel_start(&mut self, registration_id: &str) -> Result<()> {
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration {registration_id} is unavailable")
        })?;
        if registration.status != ManagedLaunchStatus::Binding {
            return Err(anyhow!("Cancel Start is available only while Binding"));
        }
        self.launch_broker
            .cancel(&registration)
            .context("Cancel Start could not stop the Managed launch")?;
        self.store.remove_binding(registration_id)
    }

    pub fn retry_open_terminal(
        &mut self,
        registration_id: &str,
        now_ms: i64,
    ) -> Result<ManagedTerminalOutcome> {
        self.access_terminal(
            registration_id,
            now_ms,
            ManagedLaunchAction::RetryOpenTerminal,
            "Retry Open Terminal",
        )
    }

    pub fn open_terminal(
        &mut self,
        registration_id: &str,
        now_ms: i64,
    ) -> Result<ManagedTerminalOutcome> {
        self.access_terminal(
            registration_id,
            now_ms,
            ManagedLaunchAction::OpenTerminal,
            "Open Terminal",
        )
    }

    pub fn copy_diagnostics(&self, registration_id: &str) -> Result<String> {
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration {registration_id} is unavailable")
        })?;
        if !registration.allows(ManagedLaunchAction::CopyDiagnostics) {
            return Err(anyhow!(
                "Copy Diagnostics is unavailable for this Managed launch"
            ));
        }
        let project_path = match &registration.target.project {
            ProjectIdentity::GitCommonDir(path) | ProjectIdentity::CanonicalCwd(path) => path,
        };
        let worktree = registration
            .target
            .worktree_root
            .as_deref()
            .map_or_else(|| "N/A".to_string(), |path| path.display().to_string());
        let launch = registration
            .launch_evidence
            .as_ref()
            .ok_or_else(|| anyhow!("Managed launch evidence is unavailable"))?;
        let mut diagnostics = vec![
            format!("Managed Launch Registration: {}", registration.id),
            "Status: Needs Review".to_string(),
            format!("Provider: {}", registration.provider.label()),
            format!(
                "Project: {} ({})",
                registration.target.project_label,
                project_path.display()
            ),
            format!("Worktree: {worktree}"),
            format!(
                "Working directory: {}",
                registration.target.working_directory.display()
            ),
            format!(
                "Provider process: pid {}, started {}",
                launch.provider_process.pid, launch.provider_process.started_at_ms
            ),
            format!(
                "Evidence: private tmux pane {}; Binding Receipt not issued",
                launch.tmux_pane_id
            ),
            format!(
                "Binding error: {}",
                registration.binding_error.as_deref().unwrap_or("unknown")
            ),
        ];
        if let Some(error) = registration.terminal_error.as_deref() {
            diagnostics.push(format!("Terminal error: {error}"));
        }
        Ok(diagnostics.join("\n"))
    }

    fn access_terminal(
        &mut self,
        registration_id: &str,
        now_ms: i64,
        action: ManagedLaunchAction,
        label: &str,
    ) -> Result<ManagedTerminalOutcome> {
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration {registration_id} is unavailable")
        })?;
        if !registration.allows(action) {
            return Err(anyhow!("{label} is unavailable for this Managed launch"));
        }
        let opened = match self.terminal_access.open(&registration) {
            Ok(()) => {
                self.store.record_terminal_opened(registration_id, now_ms)?;
                true
            }
            Err(error) => {
                self.store
                    .record_terminal_error(registration_id, &error.to_string())?;
                false
            }
        };
        let registration = self.store.get(registration_id)?.ok_or_else(|| {
            anyhow!("Managed Launch Registration disappeared after Terminal access")
        })?;
        Ok(ManagedTerminalOutcome {
            registration,
            opened,
        })
    }
}

fn validate_target(target: &ManagedLaunchTarget) -> Result<()> {
    if target.project_label.trim().is_empty() {
        return Err(anyhow!("New target Project label is unavailable"));
    }
    if !target.working_directory.is_absolute() || !target.working_directory.is_dir() {
        return Err(anyhow!("New target working directory is unavailable"));
    }
    match (&target.project, &target.worktree_root) {
        (ProjectIdentity::GitCommonDir(common_dir), Some(worktree_root))
            if common_dir.is_absolute()
                && worktree_root.is_absolute()
                && worktree_root == &target.working_directory => {}
        (ProjectIdentity::CanonicalCwd(project_root), None)
            if project_root == &target.working_directory => {}
        _ => {
            return Err(anyhow!(
                "New target Project and Worktree identity do not match its working directory"
            ));
        }
    }
    Ok(())
}

fn new_registration_id(_now_ms: i64) -> String {
    let sequence = REGISTRATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("launch-{:x}-{:x}-{:x}", nonce, std::process::id(), sequence)
}

pub struct ManagedLaunchStore {
    conn: Connection,
}

fn add_binding_error_column_if_missing(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA table_info(managed_launch_registrations)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    if !columns.iter().any(|column| column == "binding_error") {
        conn.execute(
            "ALTER TABLE managed_launch_registrations ADD COLUMN binding_error TEXT",
            [],
        )?;
    }
    Ok(())
}

impl ManagedLaunchStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                let mut builder = std::fs::DirBuilder::new();
                builder.recursive(true);
                #[cfg(unix)]
                builder.mode(0o700);
                builder.create(parent)?;
            }
            #[cfg(unix)]
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let conn = Connection::open(path)?;
        #[cfg(unix)]
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS managed_launch_registrations (
                registration_id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                provider TEXT NOT NULL,
                project_kind TEXT NOT NULL,
                project_path BLOB NOT NULL,
                project_label TEXT NOT NULL,
                worktree_root BLOB,
                working_directory BLOB NOT NULL,
                session_alias TEXT NOT NULL,
                description TEXT,
                binding_initializer INTEGER NOT NULL,
                provider_executable BLOB NOT NULL,
                provider_arguments_json TEXT NOT NULL,
                tmux_socket_name TEXT NOT NULL,
                tmux_session_name TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                launched_at_ms INTEGER,
                tmux_pane_id TEXT,
                provider_pid INTEGER,
                provider_started_at_ms INTEGER,
                terminal_opened_at_ms INTEGER,
                terminal_error TEXT,
                binding_error TEXT
            );
            CREATE TABLE IF NOT EXISTS binding_receipts (
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
            CREATE TABLE IF NOT EXISTS managed_sessions (
                provider TEXT NOT NULL,
                native_session_id TEXT NOT NULL,
                project_kind TEXT NOT NULL,
                project_path BLOB NOT NULL,
                project_label TEXT NOT NULL,
                worktree_root BLOB,
                working_directory BLOB NOT NULL,
                session_alias TEXT NOT NULL,
                description TEXT,
                binding_receipt_id TEXT NOT NULL UNIQUE,
                PRIMARY KEY (provider, native_session_id)
            );
            CREATE TABLE IF NOT EXISTS managed_current_runs (
                run_id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                native_session_id TEXT NOT NULL,
                provider_pid INTEGER NOT NULL,
                provider_started_at_ms INTEGER NOT NULL,
                tmux_socket_name TEXT NOT NULL,
                tmux_session_name TEXT NOT NULL,
                binding_receipt_id TEXT NOT NULL UNIQUE,
                UNIQUE (provider, native_session_id)
            );
            CREATE TABLE IF NOT EXISTS managed_metadata_syncs (
                provider TEXT NOT NULL,
                native_session_id TEXT NOT NULL,
                PRIMARY KEY (provider, native_session_id)
            );
            "#,
        )?;
        add_binding_error_column_if_missing(&conn)?;
        Ok(Self { conn })
    }

    pub fn insert(&self, registration: &ManagedLaunchRegistration) -> Result<()> {
        let (project_kind, project_path) = project_identity_parts(&registration.target.project);
        self.conn.execute(
            r#"
            INSERT INTO managed_launch_registrations (
                registration_id, status, provider, project_kind, project_path,
                project_label, worktree_root, working_directory, session_alias,
                description, binding_initializer, provider_executable,
                provider_arguments_json, tmux_socket_name, tmux_session_name,
                created_at_ms, launched_at_ms, tmux_pane_id, provider_pid,
                provider_started_at_ms, terminal_opened_at_ms, terminal_error,
                binding_error
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23
            )
            "#,
            params![
                registration.id,
                registration.status.as_str(),
                registration.provider.as_str(),
                project_kind,
                project_path,
                registration.target.project_label,
                registration.target.worktree_root.as_deref().map(path_bytes),
                path_bytes(&registration.target.working_directory),
                registration.session_alias,
                registration.description,
                registration.binding_initializer,
                path_bytes(&registration.provider_command.executable),
                serde_json::to_string(&registration.provider_command.arguments)?,
                registration.tmux.socket_name,
                registration.tmux.session_name,
                registration.created_at_ms,
                registration.launched_at_ms,
                registration
                    .launch_evidence
                    .as_ref()
                    .map(|evidence| &evidence.tmux_pane_id),
                registration
                    .launch_evidence
                    .as_ref()
                    .map(|evidence| evidence.provider_process.pid),
                registration
                    .launch_evidence
                    .as_ref()
                    .map(|evidence| evidence.provider_process.started_at_ms),
                registration.terminal_opened_at_ms,
                registration.terminal_error,
                registration.binding_error,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, registration_id: &str) -> Result<Option<ManagedLaunchRegistration>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT status, provider, project_kind, project_path, project_label,
                   worktree_root, working_directory, session_alias, description,
                   binding_initializer, provider_executable, provider_arguments_json,
                   tmux_socket_name, tmux_session_name, created_at_ms, launched_at_ms,
                   tmux_pane_id, provider_pid, provider_started_at_ms,
                   terminal_opened_at_ms, terminal_error, binding_error
            FROM managed_launch_registrations
            WHERE registration_id = ?1
            "#,
        )?;
        statement
            .query_row([registration_id], |row| {
                let status: String = row.get(0)?;
                let provider: String = row.get(1)?;
                let project_kind: String = row.get(2)?;
                let project_path: Vec<u8> = row.get(3)?;
                let worktree_root = row.get::<_, Option<Vec<u8>>>(5)?.map(path_from_bytes);
                let working_directory = path_from_bytes(row.get(6)?);
                let provider_executable = path_from_bytes(row.get(10)?);
                let arguments_json: String = row.get(11)?;
                let tmux_pane_id = row.get::<_, Option<String>>(16)?;
                let provider_pid = row.get::<_, Option<u32>>(17)?;
                let provider_started_at_ms = row.get::<_, Option<i64>>(18)?;
                Ok((
                    status,
                    provider,
                    project_kind,
                    project_path,
                    row.get::<_, String>(4)?,
                    worktree_root,
                    working_directory,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, bool>(9)?,
                    provider_executable,
                    arguments_json,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, i64>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                    tmux_pane_id,
                    provider_pid,
                    provider_started_at_ms,
                    row.get::<_, Option<i64>>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, Option<String>>(21)?,
                ))
            })
            .optional()?
            .map(
                |(
                    status,
                    provider,
                    project_kind,
                    project_path,
                    project_label,
                    worktree_root,
                    working_directory,
                    session_alias,
                    description,
                    binding_initializer,
                    provider_executable,
                    arguments_json,
                    tmux_socket_name,
                    tmux_session_name,
                    created_at_ms,
                    launched_at_ms,
                    tmux_pane_id,
                    provider_pid,
                    provider_started_at_ms,
                    terminal_opened_at_ms,
                    terminal_error,
                    binding_error,
                )| {
                    Ok(ManagedLaunchRegistration {
                        id: registration_id.to_string(),
                        status: ManagedLaunchStatus::parse(&status)?,
                        provider: Provider::parse(&provider)
                            .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                        target: ManagedLaunchTarget {
                            project: project_identity_from_parts(&project_kind, project_path)?,
                            project_label,
                            worktree_root,
                            working_directory,
                        },
                        session_alias,
                        description,
                        binding_initializer,
                        provider_command: ProviderNewCommand {
                            executable: provider_executable,
                            arguments: serde_json::from_str(&arguments_json)
                                .map_err(|_| anyhow!("invalid persisted Provider New arguments"))?,
                        },
                        tmux: PrivateTmuxEnvironment {
                            socket_name: tmux_socket_name,
                            session_name: tmux_session_name,
                        },
                        created_at_ms,
                        launched_at_ms,
                        launch_evidence: persisted_launch_evidence(
                            tmux_pane_id,
                            provider_pid,
                            provider_started_at_ms,
                        )?,
                        terminal_opened_at_ms,
                        terminal_error,
                        binding_error,
                    })
                },
            )
            .transpose()
    }

    pub fn binding_registrations(&self) -> Result<Vec<ManagedLaunchRegistration>> {
        let mut statement = self.conn.prepare(
            "SELECT registration_id FROM managed_launch_registrations WHERE status = 'binding' ORDER BY created_at_ms, registration_id",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        ids.into_iter()
            .map(|id| {
                self.get(&id)?
                    .ok_or_else(|| anyhow!("Binding Managed Launch Registration {id} disappeared"))
            })
            .collect()
    }

    pub fn binding_receipt(&self, registration_id: &str) -> Result<Option<BindingReceipt>> {
        let mut statement = self.conn.prepare(
            "SELECT receipt_id, provider, native_session_id, source_path, source_format, provider_pid, provider_started_at_ms, issued_at_ms FROM binding_receipts WHERE registration_id = ?1",
        )?;
        statement
            .query_row([registration_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .optional()?
            .map(
                |(
                    id,
                    provider,
                    native_session_id,
                    source_path,
                    source_format,
                    pid,
                    started_at_ms,
                    issued_at_ms,
                )| {
                    Ok(BindingReceipt {
                        id,
                        registration_id: registration_id.to_string(),
                        provider: Provider::parse(&provider)
                            .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                        native_session_id,
                        source: SourceMetadata {
                            path: path_from_bytes(source_path),
                            format: match source_format.as_str() {
                                "jsonl" => SourceFormat::JsonLines,
                                _ => {
                                    return Err(anyhow!("unknown source format {source_format:?}"))
                                }
                            },
                        },
                        provider_process: ProcessIdentity { pid, started_at_ms },
                        issued_at_ms,
                    })
                },
            )
            .transpose()
    }

    pub fn managed_session(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedSessionRecord>> {
        Ok(self.managed_sessions()?.into_iter().find(|session| {
            session.provider == provider && session.native_session_id == native_session_id
        }))
    }

    pub fn managed_sessions(&self) -> Result<Vec<ManagedSessionRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT provider, native_session_id, project_kind, project_path, project_label, worktree_root, working_directory, session_alias, description, binding_receipt_id FROM managed_sessions ORDER BY provider, native_session_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
                row.get::<_, Vec<u8>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;
        rows.map(|row| {
            let (
                provider,
                native_session_id,
                project_kind,
                project_path,
                project_label,
                worktree_root,
                working_directory,
                session_alias,
                description,
                binding_receipt_id,
            ) = row?;
            Ok(ManagedSessionRecord {
                provider: Provider::parse(&provider)
                    .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                native_session_id,
                target: ManagedLaunchTarget {
                    project: project_identity_from_parts(&project_kind, project_path)?,
                    project_label,
                    worktree_root: worktree_root.map(path_from_bytes),
                    working_directory: path_from_bytes(working_directory),
                },
                session_alias,
                description,
                binding_receipt_id,
            })
        })
        .collect()
    }

    pub fn managed_current_run(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedCurrentRunRecord>> {
        Ok(self
            .managed_current_runs()?
            .into_iter()
            .find(|run| run.provider == provider && run.native_session_id == native_session_id))
    }

    pub fn managed_current_runs(&self) -> Result<Vec<ManagedCurrentRunRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT run_id, provider, native_session_id, provider_pid, provider_started_at_ms, tmux_socket_name, tmux_session_name, binding_receipt_id FROM managed_current_runs ORDER BY run_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u32>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                provider,
                native_session_id,
                pid,
                started_at_ms,
                socket_name,
                session_name,
                binding_receipt_id,
            ) = row?;
            Ok(ManagedCurrentRunRecord {
                id,
                provider: Provider::parse(&provider)
                    .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                native_session_id,
                provider_process: ProcessIdentity { pid, started_at_ms },
                tmux: PrivateTmuxEnvironment {
                    socket_name,
                    session_name,
                },
                binding_receipt_id,
            })
        })
        .collect()
    }

    pub fn managed_metadata_is_synchronized(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM managed_metadata_syncs WHERE provider = ?1 AND native_session_id = ?2",
                params![provider.as_str(), native_session_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn mark_managed_metadata_synchronized(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO managed_metadata_syncs (provider, native_session_id) VALUES (?1, ?2)",
            params![provider.as_str(), native_session_id],
        )?;
        Ok(())
    }

    pub fn apply_provenance(&self, sessions: &mut [Session]) -> Result<()> {
        let managed = self
            .managed_sessions()?
            .into_iter()
            .map(|session| SessionIdentity {
                provider: session.provider,
                native_id: session.native_session_id,
            })
            .collect::<HashSet<_>>();
        for session in sessions {
            session.provenance = if managed.contains(&SessionIdentity::from(&*session)) {
                SessionProvenance::Managed
            } else {
                SessionProvenance::Observed
            };
        }
        Ok(())
    }

    fn establish_management(
        &mut self,
        registration: &ManagedLaunchRegistration,
        receipt: &BindingReceipt,
    ) -> Result<()> {
        let launch = registration
            .launch_evidence
            .as_ref()
            .ok_or_else(|| anyhow!("Binding requires conclusive launch evidence"))?;
        let (project_kind, project_path) = project_identity_parts(&registration.target.project);
        let transaction = self.conn.transaction()?;
        transaction.execute(
            "INSERT INTO binding_receipts (receipt_id, registration_id, provider, native_session_id, source_path, source_format, provider_pid, provider_started_at_ms, issued_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, 'jsonl', ?6, ?7, ?8)",
            params![
                receipt.id,
                receipt.registration_id,
                receipt.provider.as_str(),
                receipt.native_session_id,
                path_bytes(&receipt.source.path),
                receipt.provider_process.pid,
                receipt.provider_process.started_at_ms,
                receipt.issued_at_ms,
            ],
        )?;
        transaction.execute(
            "INSERT INTO managed_sessions (provider, native_session_id, project_kind, project_path, project_label, worktree_root, working_directory, session_alias, description, binding_receipt_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                receipt.provider.as_str(),
                receipt.native_session_id,
                project_kind,
                project_path,
                registration.target.project_label,
                registration.target.worktree_root.as_deref().map(path_bytes),
                path_bytes(&registration.target.working_directory),
                registration.session_alias,
                registration.description,
                receipt.id,
            ],
        )?;
        transaction.execute(
            "INSERT INTO managed_current_runs (run_id, provider, native_session_id, provider_pid, provider_started_at_ms, tmux_socket_name, tmux_session_name, binding_receipt_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                format!("run-{}", registration.id),
                receipt.provider.as_str(),
                receipt.native_session_id,
                launch.provider_process.pid,
                launch.provider_process.started_at_ms,
                registration.tmux.socket_name,
                registration.tmux.session_name,
                receipt.id,
            ],
        )?;
        update_one(
            transaction.execute(
                "DELETE FROM managed_launch_registrations WHERE registration_id = ?1 AND status = 'binding'",
                [registration.id.as_str()],
            )?,
            &registration.id,
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn record_launched(
        &self,
        registration_id: &str,
        now_ms: i64,
        evidence: ManagedLaunchEvidence,
    ) -> Result<()> {
        update_one(
            self.conn.execute(
                r#"
                UPDATE managed_launch_registrations
                SET launched_at_ms = ?2,
                    tmux_pane_id = ?3,
                    provider_pid = ?4,
                    provider_started_at_ms = ?5
                WHERE registration_id = ?1
                  AND launched_at_ms IS NULL
                  AND tmux_pane_id IS NULL
                  AND provider_pid IS NULL
                  AND provider_started_at_ms IS NULL
                "#,
                params![
                    registration_id,
                    now_ms,
                    evidence.tmux_pane_id,
                    evidence.provider_process.pid,
                    evidence.provider_process.started_at_ms,
                ],
            )?,
            registration_id,
        )
    }

    fn record_terminal_opened(&self, registration_id: &str, now_ms: i64) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET terminal_opened_at_ms = ?2, terminal_error = NULL WHERE registration_id = ?1 AND launched_at_ms IS NOT NULL",
                params![registration_id, now_ms],
            )?,
            registration_id,
        )
    }

    fn record_terminal_error(&self, registration_id: &str, error: &str) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET terminal_error = ?2 WHERE registration_id = ?1 AND launched_at_ms IS NOT NULL",
                params![registration_id, error],
            )?,
            registration_id,
        )
    }

    fn remove_binding(&self, registration_id: &str) -> Result<()> {
        update_one(
            self.conn.execute(
                "DELETE FROM managed_launch_registrations WHERE registration_id = ?1 AND status = 'binding'",
                [registration_id],
            )?,
            registration_id,
        )
    }

    fn mark_needs_review(&self, registration_id: &str, error: &str) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET status = 'needs_review', binding_error = ?2 WHERE registration_id = ?1 AND status = 'binding'",
                params![registration_id, error],
            )?,
            registration_id,
        )
    }

    fn mark_failed(&self, registration_id: &str, error: &str) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET status = 'failed', binding_error = ?2 WHERE registration_id = ?1 AND status = 'binding'",
                params![registration_id, error],
            )?,
            registration_id,
        )
    }
}

fn persisted_launch_evidence(
    tmux_pane_id: Option<String>,
    provider_pid: Option<u32>,
    provider_started_at_ms: Option<i64>,
) -> Result<Option<ManagedLaunchEvidence>> {
    match (tmux_pane_id, provider_pid, provider_started_at_ms) {
        (Some(tmux_pane_id), Some(pid), Some(started_at_ms)) => Ok(Some(ManagedLaunchEvidence {
            tmux_pane_id,
            provider_process: ProcessIdentity { pid, started_at_ms },
        })),
        (None, None, None) => Ok(None),
        _ => Err(anyhow!("incomplete persisted Managed launch evidence")),
    }
}

fn update_one(changed: usize, registration_id: &str) -> Result<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(anyhow!(
            "Managed Launch Registration {registration_id} is unavailable or in the wrong state"
        ))
    }
}

fn project_identity_parts(identity: &ProjectIdentity) -> (&'static str, Vec<u8>) {
    match identity {
        ProjectIdentity::GitCommonDir(path) => ("git_common_dir", path_bytes(path)),
        ProjectIdentity::CanonicalCwd(path) => ("canonical_cwd", path_bytes(path)),
    }
}

fn project_identity_from_parts(kind: &str, path: Vec<u8>) -> Result<ProjectIdentity> {
    let path = path_from_bytes(path);
    match kind {
        "git_common_dir" => Ok(ProjectIdentity::GitCommonDir(path)),
        "canonical_cwd" => Ok(ProjectIdentity::CanonicalCwd(path)),
        _ => Err(anyhow!("unknown persisted Project identity kind {kind:?}")),
    }
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}

pub struct TmuxLaunchBroker {
    tmux_executable: PathBuf,
}

impl TmuxLaunchBroker {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self { tmux_executable }
    }
}

impl ManagedLaunchBroker for TmuxLaunchBroker {
    fn launch(
        &mut self,
        registration: &ManagedLaunchRegistration,
    ) -> Result<ManagedLaunchEvidence> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_executable(
            &registration.provider_command.executable,
            registration.provider.label(),
        )?;
        validate_tmux_name(&registration.tmux.socket_name)?;
        validate_tmux_name(&registration.tmux.session_name)?;
        let output = Command::new(&self.tmux_executable)
            .env_remove("TMUX")
            .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
            .arg(&registration.tmux.socket_name)
            .args([
                OsStr::new("new-session"),
                OsStr::new("-d"),
                OsStr::new("-P"),
                OsStr::new("-F"),
                OsStr::new("#{pane_id}\t#{pane_pid}"),
                OsStr::new("-s"),
            ])
            .arg(&registration.tmux.session_name)
            .arg("-c")
            .arg(&registration.target.working_directory)
            .arg("--")
            .arg(&registration.provider_command.executable)
            .args(&registration.provider_command.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .context("could not start private tmux")?;
        if !output.status.success() {
            return Err(anyhow!("private tmux launch exited with {}", output.status));
        }
        let output = String::from_utf8(output.stdout)
            .map_err(|_| anyhow!("private tmux launch evidence is not valid UTF-8"))?;
        let (tmux_pane_id, pane_pid) = output
            .trim()
            .split_once('\t')
            .ok_or_else(|| anyhow!("private tmux launch evidence is incomplete"))?;
        validate_tmux_pane_id(tmux_pane_id)?;
        let pane_pid = pane_pid
            .parse::<u32>()
            .map_err(|_| anyhow!("private tmux Provider PID is invalid"))?;
        let provider_process = crate::providers::state::process_identity_after_spawn(pane_pid)
            .ok_or_else(|| anyhow!("private tmux Provider process identity is unavailable"))?;
        Ok(ManagedLaunchEvidence {
            tmux_pane_id: tmux_pane_id.to_string(),
            provider_process,
        })
    }

    fn cancel(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        terminate_private_tmux(&self.tmux_executable, &registration.tmux)
    }
}

fn terminate_private_tmux(
    tmux_executable: &Path,
    environment: &PrivateTmuxEnvironment,
) -> Result<()> {
    validate_executable(tmux_executable, "tmux")?;
    validate_tmux_name(&environment.socket_name)?;
    validate_tmux_name(&environment.session_name)?;
    let status = Command::new(tmux_executable)
        .env_remove("TMUX")
        .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
        .arg(&environment.socket_name)
        .arg("kill-server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not clean private tmux")?;
    if status.success() || status.code() == Some(1) {
        Ok(())
    } else {
        Err(anyhow!("private tmux cleanup failed"))
    }
}

fn clean_empty_private_tmux(
    tmux_executable: &Path,
    environment: &PrivateTmuxEnvironment,
) -> Result<()> {
    validate_executable(tmux_executable, "tmux")?;
    validate_tmux_name(&environment.socket_name)?;
    validate_tmux_name(&environment.session_name)?;
    let probe = Command::new(tmux_executable)
        .env_remove("TMUX")
        .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
        .arg(&environment.socket_name)
        .args([OsStr::new("has-session"), OsStr::new("-t")])
        .arg(&environment.session_name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not inspect private tmux")?;
    if probe.code() == Some(1) {
        Ok(())
    } else if probe.success() {
        Err(anyhow!("private tmux environment is not empty"))
    } else {
        Err(anyhow!("private tmux environment could not be inspected"))
    }
}

pub struct MacosManagedTerminalAccess {
    tmux_executable: PathBuf,
    osascript_executable: PathBuf,
}

impl MacosManagedTerminalAccess {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self::with_osascript(tmux_executable, PathBuf::from("/usr/bin/osascript"))
    }

    pub fn with_osascript(tmux_executable: PathBuf, osascript_executable: PathBuf) -> Self {
        Self {
            tmux_executable,
            osascript_executable,
        }
    }
}

impl ManagedTerminalAccess for MacosManagedTerminalAccess {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_tmux_name(&registration.tmux.socket_name)?;
        validate_tmux_name(&registration.tmux.session_name)?;
        let command = [
            shell_quote_os(self.tmux_executable.as_os_str())?,
            "-L".to_string(),
            shell_quote(&registration.tmux.socket_name),
            "attach-session".to_string(),
            "-t".to_string(),
            shell_quote(&registration.tmux.session_name),
        ]
        .join(" ");
        let status = Command::new(&self.osascript_executable)
            .args([
                OsStr::new("-e"),
                OsStr::new("on run argv"),
                OsStr::new("-e"),
                OsStr::new("tell application \"Terminal\""),
                OsStr::new("-e"),
                OsStr::new("do script (item 1 of argv)"),
                OsStr::new("-e"),
                OsStr::new("activate"),
                OsStr::new("-e"),
                OsStr::new("end tell"),
                OsStr::new("-e"),
                OsStr::new("end run"),
                OsStr::new("--"),
                OsStr::new(&command),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("could not open Terminal.app")?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("Terminal.app launch failed"))
        }
    }
}

pub fn discover_tmux_executable() -> Option<PathBuf> {
    std::env::var_os("SESSIONMESH_TMUX_EXECUTABLE")
        .map(PathBuf::from)
        .filter(|path| validate_executable(path, "tmux").is_ok())
        .or_else(|| {
            std::env::var_os("PATH")
                .as_deref()
                .map(std::env::split_paths)
                .into_iter()
                .flatten()
                .map(|directory| directory.join("tmux"))
                .find(|path| validate_executable(path, "tmux").is_ok())
        })
}

fn validate_executable(path: &Path, name: &str) -> Result<()> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("{name} executable is unavailable"))?;
    if !path.is_absolute() || !metadata.is_file() {
        return Err(anyhow!("{name} executable is unavailable"));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(anyhow!("{name} executable is unavailable"));
    }
    Ok(())
}

fn validate_tmux_name(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(anyhow!("private tmux environment is invalid"));
    }
    Ok(())
}

fn validate_tmux_pane_id(value: &str) -> Result<()> {
    if value.strip_prefix('%').is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        Ok(())
    } else {
        Err(anyhow!("private tmux pane identity is invalid"))
    }
}

fn shell_quote_os(value: &OsStr) -> Result<String> {
    value
        .to_str()
        .map(shell_quote)
        .ok_or_else(|| anyhow!("tmux executable path is not valid UTF-8"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
