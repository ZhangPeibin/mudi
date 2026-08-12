use anyhow::Result;
use sessionmesh::domain::{ProjectIdentity, Provider, SourceFormat, SourceMetadata};
use sessionmesh::managed_sessions::{
    BindingOutcome, EvidenceBinder, ManagedLaunchAction, ManagedLaunchBroker, ManagedLaunchCleanup,
    ManagedLaunchEvidence, ManagedLaunchRegistration, ManagedLaunchStatus, ManagedLaunchStore,
    ManagedLaunchTarget, ManagedSessionService, ManagedTerminalAccess, PrivateTmuxEnvironment,
    ProcessEvidence, ProviderNewCommand, ProviderProcessObservation, TmuxLaunchBroker,
    TmuxManagedLaunchCleanup,
};
use sessionmesh::providers::state::ProcessIdentity;
use sessionmesh::providers::{ProviderBindingEvidence, ProviderExecutables};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

struct RecordingLaunchBroker {
    cancelled: Arc<Mutex<Vec<String>>>,
}

impl ManagedLaunchBroker for RecordingLaunchBroker {
    fn launch(
        &mut self,
        _registration: &ManagedLaunchRegistration,
    ) -> Result<ManagedLaunchEvidence> {
        panic!("Cancel Start must not launch another Provider")
    }

    fn cancel(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        self.cancelled.lock().unwrap().push(registration.id.clone());
        Ok(())
    }
}

struct NeverOpenTerminal;

impl ManagedTerminalAccess for NeverOpenTerminal {
    fn open(&mut self, _registration: &ManagedLaunchRegistration) -> Result<()> {
        panic!("Cancel Start must not open Terminal.app")
    }
}

struct RecordingTerminal {
    opened: Arc<Mutex<Vec<String>>>,
}

impl ManagedTerminalAccess for RecordingTerminal {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        self.opened.lock().unwrap().push(registration.id.clone());
        Ok(())
    }
}

struct FixedProcessEvidence(ProviderProcessObservation);

impl ProcessEvidence for FixedProcessEvidence {
    fn observe(&self, _pid: u32) -> ProviderProcessObservation {
        self.0
    }
}

struct RecordingCleanup {
    cleaned: Arc<Mutex<Vec<String>>>,
}

impl ManagedLaunchCleanup for RecordingCleanup {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> Result<()> {
        self.cleaned.lock().unwrap().push(registration.id.clone());
        Ok(())
    }
}

#[test]
fn cancel_start_stops_only_a_binding_launch_and_removes_its_registration() {
    let temp = TempDir::new("cancel-binding");
    let store_path = temp.path().join("managed.db");
    let registration = registration("launch-cancel", temp.path());
    let store = ManagedLaunchStore::open(&store_path).unwrap();
    store.insert(&registration).unwrap();
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let mut service = ManagedSessionService::new(
        store,
        provider_executables(temp.path()),
        Box::new(RecordingLaunchBroker {
            cancelled: cancelled.clone(),
        }),
        Box::new(NeverOpenTerminal),
    );

    service.cancel_start(&registration.id).unwrap();

    assert_eq!(*cancelled.lock().unwrap(), vec![registration.id.clone()]);
    let store = ManagedLaunchStore::open(&store_path).unwrap();
    assert!(store.get(&registration.id).unwrap().is_none());
    assert!(store.managed_sessions().unwrap().is_empty());
    assert!(store.managed_current_runs().unwrap().is_empty());
}

#[test]
fn confirmed_absent_provider_fails_registration_and_cleans_empty_private_tmux() {
    let temp = TempDir::new("confirmed-absent");
    let registration = registration("launch-absent", temp.path());
    let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let cleaned = Arc::new(Mutex::new(Vec::new()));
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(ProviderProcessObservation::Absent)),
        Box::new(RecordingCleanup {
            cleaned: cleaned.clone(),
        }),
    );

    let outcome = binder
        .bind(&mut store, &registration.id, None, 50_000)
        .unwrap();

    assert_eq!(outcome, BindingOutcome::Failed);
    assert_eq!(*cleaned.lock().unwrap(), vec![registration.id.clone()]);
    let retained = store.get(&registration.id).unwrap().unwrap();
    assert_eq!(retained.status, ManagedLaunchStatus::Failed);
    assert!(store.managed_sessions().unwrap().is_empty());
    assert!(store.managed_current_runs().unwrap().is_empty());
}

#[test]
fn contradictory_live_evidence_needs_review_without_retry_or_managed_authority() {
    let temp = TempDir::new("contradictory-live");
    let source_path = temp.path().join("source.jsonl");
    fs::write(&source_path, "{}\n").unwrap();
    let registration = registration("launch-review", temp.path());
    let process = registration
        .launch_evidence
        .as_ref()
        .unwrap()
        .provider_process;
    let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let cleaned = Arc::new(Mutex::new(Vec::new()));
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(ProviderProcessObservation::Live(
            process,
        ))),
        Box::new(RecordingCleanup {
            cleaned: cleaned.clone(),
        }),
    );
    let contradictory = ProviderBindingEvidence {
        provider: Provider::ClaudeCode,
        source_session_id: "native-session".into(),
        process_session_id: "native-session".into(),
        source: SourceMetadata {
            path: source_path,
            format: SourceFormat::JsonLines,
        },
        working_directory: temp.path().to_path_buf(),
        launch_process: process,
        provider_process: process,
    };

    let outcome = binder
        .bind(
            &mut store,
            &registration.id,
            Some(contradictory.clone()),
            50_000,
        )
        .unwrap();

    assert_eq!(outcome, BindingOutcome::NeedsReview);
    assert!(cleaned.lock().unwrap().is_empty());
    let retained = store.get(&registration.id).unwrap().unwrap();
    assert_eq!(retained.status, ManagedLaunchStatus::NeedsReview);
    assert_eq!(retained.launch_evidence, registration.launch_evidence);
    assert_eq!(
        retained.binding_error.as_deref(),
        Some("Provider source evidence disagrees with the launch")
    );
    assert!(store.binding_registrations().unwrap().is_empty());
    assert!(binder
        .bind(&mut store, &registration.id, Some(contradictory), 51_000,)
        .is_err());
    assert!(store.binding_receipt(&registration.id).unwrap().is_none());
    assert!(store.managed_sessions().unwrap().is_empty());
    assert!(store.managed_current_runs().unwrap().is_empty());
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let mut service = ManagedSessionService::new(
        store,
        provider_executables(temp.path()),
        Box::new(RecordingLaunchBroker {
            cancelled: cancelled.clone(),
        }),
        Box::new(NeverOpenTerminal),
    );
    assert!(service.cancel_start(&registration.id).is_err());
    assert!(cancelled.lock().unwrap().is_empty());
}

#[test]
fn registration_state_machine_exposes_only_explicitly_permitted_actions() {
    let temp = TempDir::new("action-matrix");
    let mut launch = registration("launch-actions", temp.path());

    assert_eq!(
        launch.available_actions(),
        vec![ManagedLaunchAction::CancelStart]
    );

    launch.terminal_error = Some("Terminal.app launch failed".into());
    assert_eq!(
        launch.available_actions(),
        vec![
            ManagedLaunchAction::CancelStart,
            ManagedLaunchAction::RetryOpenTerminal,
        ]
    );

    launch.status = ManagedLaunchStatus::NeedsReview;
    launch.terminal_error = None;
    assert_eq!(
        launch.available_actions(),
        vec![
            ManagedLaunchAction::OpenTerminal,
            ManagedLaunchAction::CopyDiagnostics,
        ]
    );
    for forbidden in [
        ManagedLaunchAction::Resume,
        ManagedLaunchAction::CancelStart,
        ManagedLaunchAction::RetryBinding,
        ManagedLaunchAction::AdoptAsManaged,
    ] {
        assert!(
            !launch.allows(forbidden),
            "unexpected action: {forbidden:?}"
        );
    }

    launch.terminal_error = Some("Terminal.app launch failed".into());
    assert_eq!(
        launch.available_actions(),
        vec![
            ManagedLaunchAction::RetryOpenTerminal,
            ManagedLaunchAction::CopyDiagnostics,
        ]
    );

    launch.status = ManagedLaunchStatus::Failed;
    assert!(launch.available_actions().is_empty());
}

#[test]
fn registration_listing_prioritizes_recoverable_states_then_orders_newest_first() {
    let temp = TempDir::new("registration-order");
    let store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    let mut binding = registration("R-BINDING", temp.path());
    binding.created_at_ms = 30_000;
    let mut review = registration("R-REVIEW", temp.path());
    review.status = ManagedLaunchStatus::NeedsReview;
    review.created_at_ms = 40_000;
    let mut failed = registration("R-FAILED", temp.path());
    failed.status = ManagedLaunchStatus::Failed;
    failed.created_at_ms = 50_000;
    for registration in [&failed, &binding, &review] {
        store.insert(registration).unwrap();
    }

    let ids = store
        .registrations()
        .unwrap()
        .into_iter()
        .map(|registration| registration.id)
        .collect::<Vec<_>>();

    assert_eq!(ids, ["R-REVIEW", "R-BINDING", "R-FAILED"]);
}

#[test]
fn retry_open_terminal_only_reuses_the_preserved_launch_scene() {
    let temp = TempDir::new("retry-terminal");
    let store_path = temp.path().join("managed.db");
    let mut registration = registration("launch-terminal-retry", temp.path());
    registration.status = ManagedLaunchStatus::NeedsReview;
    registration.terminal_error = Some("Terminal.app launch failed".into());
    registration.binding_initializer = true;
    registration.provider_command.arguments = vec!["initializer-must-not-repeat".into()];
    let original_evidence = registration.launch_evidence.clone();
    let store = ManagedLaunchStore::open(&store_path).unwrap();
    store.insert(&registration).unwrap();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let mut service = ManagedSessionService::new(
        store,
        provider_executables(temp.path()),
        Box::new(RecordingLaunchBroker {
            cancelled: Arc::new(Mutex::new(Vec::new())),
        }),
        Box::new(RecordingTerminal {
            opened: opened.clone(),
        }),
    );

    let outcome = service
        .retry_open_terminal(&registration.id, 50_000)
        .unwrap();

    assert!(outcome.opened);
    assert_eq!(*opened.lock().unwrap(), vec![registration.id.clone()]);
    assert_eq!(
        outcome.registration.status,
        ManagedLaunchStatus::NeedsReview
    );
    assert_eq!(outcome.registration.launch_evidence, original_evidence);
    assert!(outcome.registration.provider_command.arguments.is_empty());
    assert_eq!(outcome.registration.terminal_opened_at_ms, Some(50_000));
    assert!(outcome.registration.terminal_error.is_none());
    let store = ManagedLaunchStore::open(&store_path).unwrap();
    assert!(store.managed_sessions().unwrap().is_empty());
    assert!(store.managed_current_runs().unwrap().is_empty());
}

#[test]
fn needs_review_can_open_terminal_for_inspection_without_changing_status() {
    let temp = TempDir::new("inspect-terminal");
    let mut registration = registration("launch-terminal-inspection", temp.path());
    registration.status = ManagedLaunchStatus::NeedsReview;
    registration.binding_error = Some("native Session evidence conflicts".into());
    let store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let mut service = ManagedSessionService::new(
        store,
        provider_executables(temp.path()),
        Box::new(RecordingLaunchBroker {
            cancelled: Arc::new(Mutex::new(Vec::new())),
        }),
        Box::new(RecordingTerminal {
            opened: opened.clone(),
        }),
    );

    let outcome = service.open_terminal(&registration.id, 51_000).unwrap();

    assert!(outcome.opened);
    assert_eq!(*opened.lock().unwrap(), vec![registration.id.clone()]);
    assert_eq!(
        outcome.registration.status,
        ManagedLaunchStatus::NeedsReview
    );
    assert_eq!(
        outcome.registration.binding_error,
        registration.binding_error
    );
    assert!(outcome.registration.launch_evidence.is_some());
}

#[test]
fn needs_review_diagnostics_include_only_launch_metadata() {
    let temp = TempDir::new("copy-diagnostics");
    let mut registration = registration("launch-diagnostics", temp.path());
    registration.status = ManagedLaunchStatus::NeedsReview;
    registration.session_alias = "private title".into();
    registration.description = Some("private description".into());
    registration.provider_command.arguments = vec!["private initializer content".into()];
    registration.binding_error = Some("native Session evidence conflicts".into());
    let store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let service = ManagedSessionService::new(
        store,
        provider_executables(temp.path()),
        Box::new(RecordingLaunchBroker {
            cancelled: Arc::new(Mutex::new(Vec::new())),
        }),
        Box::new(NeverOpenTerminal),
    );

    let diagnostics = service.copy_diagnostics(&registration.id).unwrap();

    for expected in [
        "Managed Launch Registration: launch-diagnostics",
        "Status: Needs Review",
        "Provider: Codex CLI",
        "Project: project",
        "Worktree: N/A",
        &format!("Working directory: {}", temp.path().display()),
        "Launched pane process: pid 42, started 41000",
        "Evidence: private tmux pane %42; Binding Receipt not issued",
        "Binding error: native Session evidence conflicts",
    ] {
        assert!(
            diagnostics.contains(expected),
            "missing {expected:?} in:\n{diagnostics}"
        );
    }
    for private in [
        "private title",
        "private description",
        "private initializer content",
    ] {
        assert!(!diagnostics.contains(private), "leaked {private:?}");
    }
}

#[test]
fn confirmed_absence_cleans_only_the_registered_private_tmux_environment() {
    let temp = TempDir::new("tmux-cleanup");
    let captured_args = temp.path().join("tmux-args");
    let tmux = temp.path().join("tmux");
    fs::write(
        &tmux,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 1\n",
            captured_args.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmux, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let registration = registration("launch-cleanup", temp.path());
    let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(ProviderProcessObservation::Absent)),
        Box::new(TmuxManagedLaunchCleanup::new(tmux)),
    );

    assert_eq!(
        binder
            .bind(&mut store, &registration.id, None, 50_000)
            .unwrap(),
        BindingOutcome::Failed
    );

    let arguments = fs::read_to_string(captured_args)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        vec![
            "-f",
            "/dev/null",
            "-L",
            registration.tmux.socket_name.as_str(),
            "has-session",
            "-t",
            "managed",
        ]
    );
    assert!(!arguments.iter().any(|argument| argument == "codex"));
    assert!(!arguments.iter().any(|argument| argument == "new-session"));
}

#[test]
fn nonempty_private_tmux_is_preserved_for_review() {
    let temp = TempDir::new("tmux-not-empty");
    let tmux = executable(temp.path(), "tmux");
    let registration = registration("launch-scene-preserved", temp.path());
    let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(ProviderProcessObservation::Absent)),
        Box::new(TmuxManagedLaunchCleanup::new(tmux)),
    );

    assert_eq!(
        binder
            .bind(&mut store, &registration.id, None, 50_000)
            .unwrap(),
        BindingOutcome::NeedsReview
    );
    let retained = store.get(&registration.id).unwrap().unwrap();
    assert_eq!(retained.status, ManagedLaunchStatus::NeedsReview);
    assert!(retained
        .binding_error
        .as_deref()
        .unwrap()
        .contains("private tmux environment is not empty"));
}

#[test]
fn cancel_start_terminates_the_registered_private_tmux_environment() {
    let temp = TempDir::new("cancel-private-tmux");
    let captured_args = temp.path().join("tmux-args");
    let tmux = temp.path().join("tmux");
    fs::write(
        &tmux,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            captured_args.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmux, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let registration = registration("launch-cancel-tmux", temp.path());
    let store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let mut service = ManagedSessionService::new(
        store,
        provider_executables(temp.path()),
        Box::new(TmuxLaunchBroker::new(tmux)),
        Box::new(NeverOpenTerminal),
    );

    service.cancel_start(&registration.id).unwrap();

    let arguments = fs::read_to_string(captured_args)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        vec![
            "-f",
            "/dev/null",
            "-L",
            registration.tmux.socket_name.as_str(),
            "kill-server",
        ]
    );
}

#[test]
fn already_absent_private_tmux_is_a_successful_empty_cleanup() {
    let temp = TempDir::new("tmux-already-absent");
    let tmux = temp.path().join("tmux");
    fs::write(&tmux, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmux, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let registration = registration("launch-already-clean", temp.path());
    let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(ProviderProcessObservation::Absent)),
        Box::new(TmuxManagedLaunchCleanup::new(tmux)),
    );

    assert_eq!(
        binder
            .bind(&mut store, &registration.id, None, 50_000)
            .unwrap(),
        BindingOutcome::Failed
    );
    assert_eq!(
        store.get(&registration.id).unwrap().unwrap().status,
        ManagedLaunchStatus::Failed
    );
}

fn registration(id: &str, working_directory: &Path) -> ManagedLaunchRegistration {
    ManagedLaunchRegistration {
        id: id.into(),
        status: ManagedLaunchStatus::Binding,
        provider: Provider::CodexCli,
        target: ManagedLaunchTarget {
            project: ProjectIdentity::CanonicalCwd(working_directory.to_path_buf()),
            project_label: "project".into(),
            worktree_root: None,
            working_directory: working_directory.to_path_buf(),
        },
        session_alias: "Investigate launch".into(),
        description: None,
        binding_initializer: false,
        provider_command: ProviderNewCommand {
            executable: PathBuf::from("/usr/bin/true"),
            arguments: Vec::new(),
        },
        tmux: PrivateTmuxEnvironment {
            socket_name: format!("sessionmesh-{id}"),
            session_name: "managed".into(),
        },
        created_at_ms: 40_000,
        launched_at_ms: Some(41_000),
        launch_evidence: Some(ManagedLaunchEvidence {
            tmux_pane_id: "%42".into(),
            provider_process: ProcessIdentity {
                pid: 42,
                started_at_ms: 41_000,
            },
        }),
        terminal_opened_at_ms: Some(42_000),
        terminal_error: None,
        binding_error: None,
    }
}

fn provider_executables(root: &Path) -> ProviderExecutables {
    let claude = executable(root, "claude");
    let codex = executable(root, "codex");
    ProviderExecutables::new(claude, codex)
}

fn executable(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sessionmesh-managed-launch-failures-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
