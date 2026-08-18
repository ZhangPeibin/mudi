use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mudi::adapters::config::Config;
use mudi::adapters::macos::clipboard::Clipboard;
use mudi::adapters::providers::state::ProcessIdentity;
use mudi::adapters::providers::ProviderBindingEvidence;
use mudi::adapters::providers::ProviderExecutables;
use mudi::application::handoff::{HandoffPaths, HandoffService, WorkerLaunch, WorkerLauncher};
use mudi::application::managed_sessions::{
    run_attach_from_args_with_access, BindingOutcome, BindingReceipt, EvidenceBinder,
    MacosManagedTerminalAccess, ManagedCurrentRunAccessService, ManagedCurrentRunEvidence,
    ManagedCurrentRunRecord, ManagedCurrentRunTerminalAccess, ManagedCurrentRunVerifier,
    ManagedLaunchBroker, ManagedLaunchCleanup, ManagedLaunchEvidence, ManagedLaunchRegistration,
    ManagedLaunchStatus, ManagedLaunchStore, ManagedLaunchTarget, ManagedSessionRecord,
    ManagedSessionService, ManagedTerminalAccess, PrivateTmuxEnvironment, ProcessEvidence,
    ProviderNewCommand, ProviderProcessObservation, SystemManagedCurrentRunEvidence,
    SystemProcessEvidence, VerifiedManagedCurrentRun,
};
use mudi::application::observation::read_model::{ManagedCurrentRunSummary, RefreshSnapshot};
use mudi::application::observation::refresh::RefreshService;
use mudi::application::successor::TerminalLauncher;
use mudi::application::ApplicationController;
use mudi::domain::{
    ContextState, Project, ProjectIdentity, Provider, Session, SessionProvenance, SessionState,
    SourceFormat, SourceMetadata, Usage,
};
use mudi::presentation::tui::{draw, Ui, UiAction};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

fn session(state: SessionState, provenance: SessionProvenance) -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: "native-session".into(),
        provenance,
        source: SourceMetadata::default(),
        project: Some(Project {
            identity: ProjectIdentity::new(PathBuf::from("/workspace/sessionmesh")),
            label: "sessionmesh".into(),
        }),
        cwd: Some("/workspace/sessionmesh".into()),
        latest_model: Some("claude-sonnet".into()),
        state,
        context: ContextState::default(),
        usage: Usage::default(),
        per_model: Vec::new(),
        current_run_usage: None,
        compactions: Vec::new(),
        last_activity_ms: 10,
        cost: None,
    }
}

fn key(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
}

fn open_detail(ui: &mut Ui, snapshot: &RefreshSnapshot) {
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), snapshot)
        .unwrap();
}

fn render(ui: &Ui, snapshot: &RefreshSnapshot) -> String {
    let backend = TestBackend::new(120, 36);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, snapshot, ui)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn verified_open_managed_current_run_exposes_both_reentry_intents_in_session_detail() {
    let snapshot = RefreshSnapshot {
        sessions: vec![session(SessionState::Idle, SessionProvenance::Managed)],
        verified_managed_current_runs: vec![ManagedCurrentRunSummary {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session".into(),
            opaque_ref: "run_0123456789abcdef".into(),
        }],
        ready: true,
        ..RefreshSnapshot::default()
    };
    let mut ui = Ui::default();
    open_detail(&mut ui, &snapshot);

    let rendered = render(&ui, &snapshot);
    assert!(rendered.contains("Open Terminal"));
    assert!(rendered.contains("Copy Attach Command"));
    assert!(!rendered.contains("run_0123456789abcdef"));

    assert_eq!(
        ui.handle_key(key('o'), &snapshot).unwrap(),
        UiAction::OpenManagedCurrentRunTerminal {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session".into(),
            opaque_ref: "run_0123456789abcdef".into(),
        }
    );
    assert_eq!(
        ui.handle_key(key('a'), &snapshot).unwrap(),
        UiAction::CopyManagedCurrentRunAttachCommand {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session".into(),
            opaque_ref: "run_0123456789abcdef".into(),
        }
    );
}

#[test]
fn session_detail_hides_reentry_for_unverified_exited_or_observed_runs() {
    let verified = ManagedCurrentRunSummary {
        provider: Provider::ClaudeCode,
        native_session_id: "native-session".into(),
        opaque_ref: "run_0123456789abcdef".into(),
    };
    let cases = [
        (
            session(SessionState::Idle, SessionProvenance::Managed),
            Vec::new(),
        ),
        (
            session(SessionState::Idle, SessionProvenance::Observed),
            vec![verified.clone()],
        ),
        (
            session(SessionState::Exited, SessionProvenance::Managed),
            vec![verified],
        ),
    ];

    for (session, verified_managed_current_runs) in cases {
        let snapshot = RefreshSnapshot {
            sessions: vec![session],
            verified_managed_current_runs,
            ready: true,
            ..RefreshSnapshot::default()
        };
        let mut ui = Ui::default();
        open_detail(&mut ui, &snapshot);

        let rendered = render(&ui, &snapshot);
        assert!(!rendered.contains("Copy Attach Command"), "{rendered}");
        assert_eq!(ui.handle_key(key('o'), &snapshot).unwrap(), UiAction::None);
        assert_eq!(ui.handle_key(key('a'), &snapshot).unwrap(), UiAction::None);
    }
}

#[derive(Clone)]
struct FixedProcessEvidence(ProcessIdentity);

impl ProcessEvidence for FixedProcessEvidence {
    fn observe(&self, _pid: u32) -> ProviderProcessObservation {
        ProviderProcessObservation::Live(self.0)
    }
}

struct NoopCleanup;

impl ManagedLaunchCleanup for NoopCleanup {
    fn clean_empty(&self, _registration: &ManagedLaunchRegistration) -> anyhow::Result<()> {
        Ok(())
    }
}

struct RecordingOpenEvidence {
    checked: Arc<Mutex<Vec<(String, String, String)>>>,
}

impl ManagedCurrentRunEvidence for RecordingOpenEvidence {
    fn revalidate(
        &self,
        _config: &Config,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> anyhow::Result<()> {
        self.checked.lock().unwrap().push((
            run.native_session_id.clone(),
            session.native_session_id.clone(),
            receipt.native_session_id.clone(),
        ));
        Ok(())
    }
}

#[test]
fn verifier_resolves_an_independent_opaque_reference_from_authoritative_managed_metadata() {
    let temp = TempDir::new("verified-reference");
    let config = temp.config();
    let mut store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
    bind_managed_run(&mut store, &temp, "launch-internal", "native-session");
    let checked = Arc::new(Mutex::new(Vec::new()));
    let verifier = ManagedCurrentRunVerifier::new(
        config,
        Box::new(RecordingOpenEvidence {
            checked: checked.clone(),
        }),
    );

    let verified = verifier.verified_runs(&store).unwrap();

    assert_eq!(verified.len(), 1);
    let run = &verified[0];
    assert!(run.opaque_ref.starts_with("run_"));
    assert_eq!(run.opaque_ref.len(), 36);
    assert!(!run.opaque_ref.contains("launch-internal"));
    assert_ne!(run.opaque_ref, "sessionmesh-launch-internal");
    assert_eq!(run.provider, Provider::ClaudeCode);
    assert_eq!(run.native_session_id, "native-session");
    assert_eq!(
        *checked.lock().unwrap(),
        vec![(
            "native-session".into(),
            "native-session".into(),
            "native-session".into(),
        )]
    );
}

struct RejectingEvidence;

impl ManagedCurrentRunEvidence for RejectingEvidence {
    fn revalidate(
        &self,
        _config: &Config,
        _run: &ManagedCurrentRunRecord,
        _session: &ManagedSessionRecord,
        _receipt: &BindingReceipt,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Current Run is Exited")
    }
}

#[test]
fn refresh_projects_only_revalidated_open_managed_current_runs_without_tmux_metadata() {
    let open_temp = TempDir::new("refresh-open");
    let open_config = open_temp.config();
    let mut open_store = ManagedLaunchStore::open(open_config.managed_launch_store_path()).unwrap();
    bind_managed_run(
        &mut open_store,
        &open_temp,
        "launch-private-open",
        "native-session",
    );
    let mut open_refresh = RefreshService::new(open_config).with_managed_current_run_evidence(
        Box::new(RecordingOpenEvidence {
            checked: Arc::new(Mutex::new(Vec::new())),
        }),
    );

    let open_snapshot = open_refresh.refresh();

    assert_eq!(open_snapshot.verified_managed_current_runs.len(), 1);
    let debug_projection = format!("{:?}", open_snapshot.verified_managed_current_runs);
    assert!(!debug_projection.contains("sessionmesh-launch-private-open"));
    assert!(!debug_projection.contains("launch-private-open"));
    assert!(!debug_projection.contains("managed"));

    let exited_temp = TempDir::new("refresh-exited");
    let exited_config = exited_temp.config();
    let mut exited_store =
        ManagedLaunchStore::open(exited_config.managed_launch_store_path()).unwrap();
    bind_managed_run(
        &mut exited_store,
        &exited_temp,
        "launch-private-exited",
        "native-session",
    );
    let mut exited_refresh = RefreshService::new(exited_config)
        .with_managed_current_run_evidence(Box::new(RejectingEvidence));

    assert!(exited_refresh
        .refresh()
        .verified_managed_current_runs
        .is_empty());
}

struct NeverManagedLaunch;

impl ManagedLaunchBroker for NeverManagedLaunch {
    fn launch(
        &mut self,
        _registration: &ManagedLaunchRegistration,
    ) -> anyhow::Result<ManagedLaunchEvidence> {
        panic!("Managed Current Run re-entry must not launch a Provider")
    }
}

struct NeverRegistrationTerminal;

impl ManagedTerminalAccess for NeverRegistrationTerminal {
    fn open(&mut self, _registration: &ManagedLaunchRegistration) -> anyhow::Result<()> {
        panic!("Managed Current Run re-entry must not use Managed Start terminal recovery")
    }
}

struct NeverWorker;

impl WorkerLauncher for NeverWorker {
    fn launch(&self, _launch: &WorkerLaunch) -> std::io::Result<()> {
        panic!("Managed Current Run re-entry must not launch a Handoff worker")
    }
}

struct NeverSuccessorTerminal;

impl TerminalLauncher for NeverSuccessorTerminal {
    fn open(&mut self, _task_id: &str, _now_ms: i64) -> anyhow::Result<()> {
        panic!("Managed Current Run re-entry must not launch a Successor")
    }
}

struct RecordingRunTerminal {
    opened: Arc<Mutex<Vec<String>>>,
}

impl ManagedCurrentRunTerminalAccess for RecordingRunTerminal {
    fn open_terminal(&mut self, run: &VerifiedManagedCurrentRun) -> anyhow::Result<()> {
        self.opened.lock().unwrap().push(run.opaque_ref.clone());
        Ok(())
    }

    fn attach_foreground(&mut self, _run: &VerifiedManagedCurrentRun) -> anyhow::Result<()> {
        panic!("Open Terminal must not use foreground attach")
    }
}

struct RecordingClipboard {
    values: Arc<Mutex<Vec<String>>>,
}

struct FailingClipboard;

impl Clipboard for FailingClipboard {
    fn write_text(&mut self, _value: &str) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "clipboard denied",
        ))
    }
}

struct NeverClipboard;

impl Clipboard for NeverClipboard {
    fn write_text(&mut self, _value: &str) -> std::io::Result<()> {
        panic!("an unverified Run must not reach the clipboard")
    }
}

impl Clipboard for RecordingClipboard {
    fn write_text(&mut self, value: &str) -> std::io::Result<()> {
        self.values.lock().unwrap().push(value.to_string());
        Ok(())
    }
}

#[test]
fn application_revalidates_each_reentry_action_without_replaying_provider_launch() {
    let temp = TempDir::new("application-actions");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    bind_managed_run(&mut store, &temp, "launch-private", "native-session");
    let opaque_ref = store.managed_current_runs().unwrap()[0].opaque_ref.clone();
    drop(store);
    let evidence_checks = Arc::new(Mutex::new(Vec::new()));
    let opened = Arc::new(Mutex::new(Vec::new()));
    let run_access = ManagedCurrentRunAccessService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        ManagedCurrentRunVerifier::new(
            config.clone(),
            Box::new(RecordingOpenEvidence {
                checked: evidence_checks.clone(),
            }),
        ),
        Box::new(RecordingRunTerminal {
            opened: opened.clone(),
        }),
    );
    let executables = ProviderExecutables::new(
        PathBuf::from("/usr/bin/true"),
        PathBuf::from("/usr/bin/true"),
    );
    let managed_sessions = ManagedSessionService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        executables.clone(),
        Box::new(NeverManagedLaunch),
        Box::new(NeverRegistrationTerminal),
    );
    let handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.0.join("tasks.db"),
            artifact_root: temp.0.join("artifacts"),
        },
        executables,
        NeverWorker,
    )
    .unwrap();
    let mut controller =
        ApplicationController::new(handoff, NeverSuccessorTerminal, managed_sessions)
            .with_managed_current_run_access(run_access);
    let copied = Arc::new(Mutex::new(Vec::new()));
    let mut clipboard = RecordingClipboard {
        values: copied.clone(),
    };
    let mut ui = Ui::default();
    let snapshot = RefreshSnapshot::default();
    let (rescan_tx, mut rescan_rx) = tokio::sync::mpsc::unbounded_channel();

    for action in [
        UiAction::OpenManagedCurrentRunTerminal {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session".into(),
            opaque_ref: opaque_ref.clone(),
        },
        UiAction::CopyManagedCurrentRunAttachCommand {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session".into(),
            opaque_ref: opaque_ref.clone(),
        },
    ] {
        assert!(!controller.handle(action, &snapshot, &mut ui, &mut clipboard, &rescan_tx,));
        assert!(rescan_rx.try_recv().is_ok());
    }

    assert_eq!(*opened.lock().unwrap(), vec![opaque_ref.clone()]);
    assert_eq!(
        *copied.lock().unwrap(),
        vec![format!("mudi attach {opaque_ref}")]
    );
    assert_eq!(evidence_checks.lock().unwrap().len(), 2);

    assert!(!controller.handle(
        UiAction::CopyManagedCurrentRunAttachCommand {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session".into(),
            opaque_ref: opaque_ref.clone(),
        },
        &snapshot,
        &mut ui,
        &mut FailingClipboard,
        &rescan_tx,
    ));
    assert!(rescan_rx.try_recv().is_ok());
    assert!(render(&ui, &snapshot).contains("Copy failed: clipboard denied"));
    assert_eq!(evidence_checks.lock().unwrap().len(), 3);
}

#[test]
fn application_failure_revalidates_before_terminal_or_clipboard_side_effects() {
    let temp = TempDir::new("application-failure");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    bind_managed_run(&mut store, &temp, "launch-exited", "native-session");
    let opaque_ref = store.managed_current_runs().unwrap()[0].opaque_ref.clone();
    drop(store);
    let opened = Arc::new(Mutex::new(Vec::new()));
    let run_access = ManagedCurrentRunAccessService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        ManagedCurrentRunVerifier::new(config, Box::new(RejectingEvidence)),
        Box::new(RecordingRunTerminal {
            opened: opened.clone(),
        }),
    );
    let executables = ProviderExecutables::new(
        PathBuf::from("/usr/bin/true"),
        PathBuf::from("/usr/bin/true"),
    );
    let managed_sessions = ManagedSessionService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        executables.clone(),
        Box::new(NeverManagedLaunch),
        Box::new(NeverRegistrationTerminal),
    );
    let handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.0.join("tasks.db"),
            artifact_root: temp.0.join("artifacts"),
        },
        executables,
        NeverWorker,
    )
    .unwrap();
    let mut controller =
        ApplicationController::new(handoff, NeverSuccessorTerminal, managed_sessions)
            .with_managed_current_run_access(run_access);
    let snapshot = RefreshSnapshot::default();
    let mut ui = Ui::default();
    let (rescan_tx, mut rescan_rx) = tokio::sync::mpsc::unbounded_channel();

    assert!(!controller.handle(
        UiAction::OpenManagedCurrentRunTerminal {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session".into(),
            opaque_ref: opaque_ref.clone(),
        },
        &snapshot,
        &mut ui,
        &mut NeverClipboard,
        &rescan_tx,
    ));
    assert!(rescan_rx.try_recv().is_ok());
    assert!(render(&ui, &snapshot).contains("Open Terminal failed: Current Run is Exited"));

    assert!(!controller.handle(
        UiAction::CopyManagedCurrentRunAttachCommand {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session".into(),
            opaque_ref,
        },
        &snapshot,
        &mut ui,
        &mut NeverClipboard,
        &rescan_tx,
    ));
    assert!(rescan_rx.try_recv().is_ok());
    assert!(render(&ui, &snapshot).contains("Copy Attach Command failed: Current Run is Exited"));
    assert!(opened.lock().unwrap().is_empty());
    assert_eq!(
        ManagedLaunchStore::open(&store_path)
            .unwrap()
            .managed_current_runs()
            .unwrap()
            .len(),
        1
    );
}

struct RecordingCliTerminal {
    attached: Arc<Mutex<Vec<String>>>,
}

impl ManagedCurrentRunTerminalAccess for RecordingCliTerminal {
    fn open_terminal(&mut self, _run: &VerifiedManagedCurrentRun) -> anyhow::Result<()> {
        panic!("CLI attach must not open Terminal.app")
    }

    fn attach_foreground(&mut self, run: &VerifiedManagedCurrentRun) -> anyhow::Result<()> {
        self.attached.lock().unwrap().push(run.opaque_ref.clone());
        Ok(())
    }
}

#[test]
fn attach_cli_accepts_exactly_one_opaque_reference_and_revalidates_before_foreground_attach() {
    let temp = TempDir::new("cli-contract");
    let config = temp.config();
    let mut store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
    bind_managed_run(&mut store, &temp, "launch-cli", "native-session");
    let opaque_ref = store.managed_current_runs().unwrap()[0].opaque_ref.clone();
    let attached = Arc::new(Mutex::new(Vec::new()));
    let mut access = ManagedCurrentRunAccessService::new(
        store,
        ManagedCurrentRunVerifier::new(
            config,
            Box::new(RecordingOpenEvidence {
                checked: Arc::new(Mutex::new(Vec::new())),
            }),
        ),
        Box::new(RecordingCliTerminal {
            attached: attached.clone(),
        }),
    );

    assert!(run_attach_from_args_with_access(
        ["sessionmesh", "attach", opaque_ref.as_str()],
        &mut access,
    )
    .unwrap());
    assert!(
        run_attach_from_args_with_access(["mudi", "attach", opaque_ref.as_str()], &mut access,)
            .unwrap()
    );
    assert_eq!(
        *attached.lock().unwrap(),
        vec![opaque_ref.clone(), opaque_ref]
    );

    assert!(
        !run_attach_from_args_with_access(["sessionmesh", "--theme", "light"], &mut access)
            .unwrap()
    );
    assert!(run_attach_from_args_with_access(["sessionmesh", "attach"], &mut access).is_err());
    assert!(run_attach_from_args_with_access(
        [
            "sessionmesh",
            "attach",
            "run_0123456789abcdef0123456789abcdef",
            "extra"
        ],
        &mut access,
    )
    .is_err());
    assert_eq!(attached.lock().unwrap().len(), 2);
}

#[test]
fn attach_cli_rejects_exited_and_non_managed_references_before_terminal_access() {
    let temp = TempDir::new("cli-failures");
    let config = temp.config();
    let mut store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
    bind_managed_run(&mut store, &temp, "launch-exited", "native-session");
    let opaque_ref = store.managed_current_runs().unwrap()[0].opaque_ref.clone();
    let attached = Arc::new(Mutex::new(Vec::new()));
    let mut access = ManagedCurrentRunAccessService::new(
        store,
        ManagedCurrentRunVerifier::new(config, Box::new(RejectingEvidence)),
        Box::new(RecordingCliTerminal {
            attached: attached.clone(),
        }),
    );

    assert!(run_attach_from_args_with_access(
        ["sessionmesh", "attach", opaque_ref.as_str()],
        &mut access,
    )
    .is_err());
    assert!(run_attach_from_args_with_access(
        [
            "sessionmesh",
            "attach",
            "run_ffffffffffffffffffffffffffffffff",
        ],
        &mut access,
    )
    .is_err());
    assert!(attached.lock().unwrap().is_empty());
}

#[test]
fn attach_cli_rejects_an_ambiguous_opaque_reference_before_live_evidence_or_terminal_access() {
    let temp = TempDir::new("cli-ambiguous");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    bind_managed_run(&mut store, &temp, "launch-first", "native-first");
    bind_managed_run(&mut store, &temp, "launch-second", "native-second");
    let runs = store.managed_current_runs().unwrap();
    let opaque_ref = runs
        .iter()
        .find(|run| run.native_session_id == "native-first")
        .unwrap()
        .opaque_ref
        .clone();
    let connection = rusqlite::Connection::open(&store_path).unwrap();
    connection
        .execute("DROP INDEX managed_current_runs_opaque_ref_unique", [])
        .unwrap();
    connection
        .execute(
            "UPDATE managed_current_runs SET opaque_ref = ?1 WHERE native_session_id = 'native-second'",
            [opaque_ref.as_str()],
        )
        .unwrap();
    drop(connection);
    let evidence_checks = Arc::new(Mutex::new(Vec::new()));
    let attached = Arc::new(Mutex::new(Vec::new()));
    let mut access = ManagedCurrentRunAccessService::new(
        store,
        ManagedCurrentRunVerifier::new(
            config,
            Box::new(RecordingOpenEvidence {
                checked: evidence_checks.clone(),
            }),
        ),
        Box::new(RecordingCliTerminal {
            attached: attached.clone(),
        }),
    );

    let error = run_attach_from_args_with_access(
        ["sessionmesh", "attach", opaque_ref.as_str()],
        &mut access,
    )
    .unwrap_err();

    assert!(error.to_string().contains("ambiguous"));
    assert!(evidence_checks.lock().unwrap().is_empty());
    assert!(attached.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn terminal_adapter_only_attaches_the_existing_private_tmux_for_both_entry_points() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new("terminal-adapter");
    let config = temp.config();
    let mut store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
    bind_managed_run(&mut store, &temp, "launch-terminal", "native-session");
    let verifier = ManagedCurrentRunVerifier::new(
        config,
        Box::new(RecordingOpenEvidence {
            checked: Arc::new(Mutex::new(Vec::new())),
        }),
    );
    let run = verifier.verified_runs(&store).unwrap().pop().unwrap();
    let tmux_args = temp.0.join("tmux-args");
    let tmux_environment = temp.0.join("tmux-environment");
    let osascript_args = temp.0.join("osascript-args");
    let tmux = temp.0.join("tmux");
    let osascript = temp.0.join("osascript");
    fs::write(
        &tmux,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' \"${{TMUX-unset}}\" > '{}'\n",
            tmux_args.display(),
            tmux_environment.display(),
        ),
    )
    .unwrap();
    fs::write(
        &osascript,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            osascript_args.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&tmux, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&osascript, fs::Permissions::from_mode(0o755)).unwrap();
    let mut terminal = MacosManagedTerminalAccess::with_osascript(tmux.clone(), osascript.clone());

    ManagedCurrentRunTerminalAccess::open_terminal(&mut terminal, &run).unwrap();
    let terminal_command = fs::read_to_string(osascript_args).unwrap();
    assert!(terminal_command.contains(&format!("'{}'", tmux.display())));
    assert!(terminal_command.contains("attach-session"));
    assert!(terminal_command.contains("sessionmesh-launch-terminal"));

    let previous_tmux = std::env::var_os("TMUX");
    std::env::set_var("TMUX", "outer-client");
    let foreground_result = ManagedCurrentRunTerminalAccess::attach_foreground(&mut terminal, &run);
    match previous_tmux {
        Some(value) => std::env::set_var("TMUX", value),
        None => std::env::remove_var("TMUX"),
    }
    foreground_result.unwrap();
    let foreground_args = fs::read_to_string(tmux_args).unwrap();
    assert_eq!(
        fs::read_to_string(tmux_environment).unwrap().trim(),
        "unset"
    );
    assert_eq!(
        foreground_args.lines().collect::<Vec<_>>(),
        [
            "-L",
            "sessionmesh-launch-terminal",
            "attach-session",
            "-t",
            "managed",
        ]
    );
    for forbidden in [
        "codex",
        "claude",
        "new-session",
        "kill-session",
        "kill-server",
    ] {
        assert!(!terminal_command.contains(forbidden));
        assert!(!foreground_args.contains(forbidden));
    }

    fs::write(&osascript, "#!/bin/sh\nexit 8\n").unwrap();
    fs::set_permissions(&osascript, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ManagedCurrentRunTerminalAccess::open_terminal(&mut terminal, &run).is_err());

    fs::write(&tmux, "#!/bin/sh\nexit 9\n").unwrap();
    fs::set_permissions(&tmux, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ManagedCurrentRunTerminalAccess::attach_foreground(&mut terminal, &run).is_err());
}

#[test]
fn system_revalidation_requires_the_exact_live_process_and_native_session_source() {
    let temp = TempDir::new("system-evidence");
    let config = temp.config();
    let working_directory = temp.directory("working_directory");
    let process = SystemProcessEvidence
        .current_identity(std::process::id())
        .unwrap();
    let native_session_id = "native-system-session";
    let transcript_directory = config.claude_dir.join("projects/project");
    let state_directory = config.claude_sessions_dir();
    fs::create_dir_all(&transcript_directory).unwrap();
    fs::create_dir_all(&state_directory).unwrap();
    let source_path = transcript_directory.join(format!("{native_session_id}.jsonl"));
    fs::write(
        &source_path,
        serde_json::json!({
            "type": "user",
            "sessionId": native_session_id,
            "cwd": working_directory,
        })
        .to_string(),
    )
    .unwrap();
    let state_path = state_directory.join(format!("{}.json", process.pid));
    let write_state = |started_at_ms: i64| {
        fs::write(
            &state_path,
            serde_json::json!({
                "pid": process.pid,
                "sessionId": native_session_id,
                "status": "idle",
                "startedAt": started_at_ms,
            })
            .to_string(),
        )
        .unwrap();
    };
    write_state(process.started_at_ms);
    let registration = ManagedLaunchRegistration {
        id: "launch-system".into(),
        status: ManagedLaunchStatus::Binding,
        provider: Provider::ClaudeCode,
        target: ManagedLaunchTarget {
            working_directory: working_directory.clone(),
        },
        session_alias: "System evidence".into(),
        description: None,
        binding_initializer: false,
        provider_command: ProviderNewCommand {
            executable: PathBuf::from("/usr/bin/true"),
            arguments: Vec::new(),
        },
        tmux: PrivateTmuxEnvironment {
            socket_name: "sessionmesh-launch-system".into(),
            session_name: "managed".into(),
        },
        created_at_ms: 40_000,
        launched_at_ms: Some(41_000),
        launch_evidence: Some(ManagedLaunchEvidence {
            tmux_pane_id: "%42".into(),
            provider_process: process,
        }),
        terminal_opened_at_ms: Some(42_000),
        terminal_error: None,
        binding_error: None,
    };
    let mut store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
    store.insert(&registration).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(process)),
        Box::new(NoopCleanup),
    );
    assert!(matches!(
        binder
            .bind(
                &mut store,
                &registration.id,
                Some(ProviderBindingEvidence {
                    provider: Provider::ClaudeCode,
                    source_session_id: native_session_id.into(),
                    process_session_id: native_session_id.into(),
                    source: SourceMetadata {
                        path: source_path.canonicalize().unwrap(),
                        format: SourceFormat::JsonLines,
                    },
                    working_directory,
                    launch_process: process,
                    provider_process: process,
                }),
                50_000,
            )
            .unwrap(),
        BindingOutcome::Bound(_)
    ));
    let opaque_ref = store.managed_current_runs().unwrap()[0].opaque_ref.clone();
    let verifier =
        ManagedCurrentRunVerifier::new(config, Box::new(SystemManagedCurrentRunEvidence));

    verifier.verify_ref(&store, &opaque_ref).unwrap();

    write_state(process.started_at_ms - 10_000);
    assert!(verifier.verify_ref(&store, &opaque_ref).is_err());
}

#[test]
fn existing_managed_current_runs_receive_one_stable_opaque_reference_on_store_upgrade() {
    let temp = TempDir::new("opaque-ref-migration");
    let store_path = temp.path().join("managed.db");
    let connection = rusqlite::Connection::open(&store_path).unwrap();
    connection
        .execute_batch(
            r#"
            CREATE TABLE managed_current_runs (
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
            INSERT INTO managed_current_runs (
                run_id, provider, native_session_id, provider_pid,
                provider_started_at_ms, tmux_socket_name, tmux_session_name,
                binding_receipt_id
            ) VALUES (
                'run-old-internal', 'claude', 'native-old', 42, 41000,
                'sessionmesh-old-private', 'managed', 'receipt-old'
            );
            "#,
        )
        .unwrap();
    drop(connection);

    let first = ManagedLaunchStore::open(&store_path)
        .unwrap()
        .managed_current_runs()
        .unwrap()[0]
        .opaque_ref
        .clone();
    let reopened = ManagedLaunchStore::open(&store_path)
        .unwrap()
        .managed_current_runs()
        .unwrap()[0]
        .opaque_ref
        .clone();

    assert!(first.starts_with("run_"));
    assert_eq!(first.len(), 36);
    assert_eq!(reopened, first);
    assert!(!reopened.contains("old-internal"));
    assert!(!reopened.contains("old-private"));
}

fn bind_managed_run(
    store: &mut ManagedLaunchStore,
    temp: &TempDir,
    registration_id: &str,
    native_session_id: &str,
) {
    let working_directory = temp.directory("working_directory");
    let source_path = temp.file("source.jsonl", "{}\n");
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 41_000,
    };
    let registration = ManagedLaunchRegistration {
        id: registration_id.into(),
        status: ManagedLaunchStatus::Binding,
        provider: Provider::ClaudeCode,
        target: ManagedLaunchTarget {
            working_directory: working_directory.clone(),
        },
        session_alias: "Managed work".into(),
        description: None,
        binding_initializer: false,
        provider_command: ProviderNewCommand {
            executable: PathBuf::from("/usr/bin/true"),
            arguments: Vec::new(),
        },
        tmux: PrivateTmuxEnvironment {
            socket_name: format!("sessionmesh-{registration_id}"),
            session_name: "managed".into(),
        },
        created_at_ms: 40_000,
        launched_at_ms: Some(41_000),
        launch_evidence: Some(ManagedLaunchEvidence {
            tmux_pane_id: "%42".into(),
            provider_process: process,
        }),
        terminal_opened_at_ms: Some(42_000),
        terminal_error: None,
        binding_error: None,
    };
    store.insert(&registration).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(process)),
        Box::new(NoopCleanup),
    );
    let outcome = binder
        .bind(
            store,
            registration_id,
            Some(ProviderBindingEvidence {
                provider: Provider::ClaudeCode,
                source_session_id: native_session_id.into(),
                process_session_id: native_session_id.into(),
                source: SourceMetadata {
                    path: source_path,
                    format: SourceFormat::JsonLines,
                },
                working_directory,
                launch_process: process,
                provider_process: process,
            }),
            50_000,
        )
        .unwrap();
    assert!(matches!(outcome, BindingOutcome::Bound(_)));
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sessionmesh-managed-reentry-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn config(&self) -> Config {
        Config {
            claude_dir: self.0.join("claude"),
            codex_dir: self.0.join("codex"),
            data_dir: self.0.join("data"),
            extra_dirs: Vec::new(),
        }
    }

    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, contents).unwrap();
        path.canonicalize().unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
