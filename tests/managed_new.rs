use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sessionmesh::app::Snapshot;
use sessionmesh::application::{ApplicationController, DirectoryPicker};
use sessionmesh::clipboard::Clipboard;
use sessionmesh::domain::{
    ContextState, Project, ProjectIdentity, Provider, Session, SessionState, SourceMetadata, Usage,
};
use sessionmesh::handoff::{HandoffPaths, HandoffService, WorkerLaunch, WorkerLauncher};
use sessionmesh::managed_sessions::{
    MacosManagedTerminalAccess, ManagedLaunchBroker, ManagedLaunchEvidence,
    ManagedLaunchRegistration, ManagedLaunchStore, ManagedLaunchTarget, ManagedNewRequest,
    ManagedSessionService, ManagedTerminalAccess, TmuxLaunchBroker,
};
use sessionmesh::providers::state::ProcessIdentity;
use sessionmesh::providers::ProviderExecutables;
use sessionmesh::providers::{registry, BINDING_INITIALIZER};
use sessionmesh::successor::TerminalLauncher;
use sessionmesh::ui::{draw, Ui, UiAction};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn provider_adapters_build_native_new_commands_without_collecting_a_prompt() {
    for provider in [Provider::CodexCli, Provider::ClaudeCode] {
        let adapter = registry().runtime_control(provider).unwrap();

        assert_eq!(adapter.new_session_args(false), Vec::<String>::new());
        assert_eq!(
            adapter.new_session_args(true),
            vec![BINDING_INITIALIZER.to_string()]
        );
    }
}

struct AssertPersistedBeforeLaunch {
    store_path: PathBuf,
    launched: Arc<Mutex<Vec<String>>>,
}

impl ManagedLaunchBroker for AssertPersistedBeforeLaunch {
    fn launch(
        &mut self,
        registration: &ManagedLaunchRegistration,
    ) -> Result<ManagedLaunchEvidence> {
        let persisted = ManagedLaunchStore::open(&self.store_path)?
            .get(&registration.id)?
            .expect("registration must exist before launch");
        assert_eq!(persisted, *registration);
        assert!(persisted.launched_at_ms.is_none());
        assert!(persisted.launch_evidence.is_none());
        self.launched.lock().unwrap().push(registration.id.clone());
        Ok(ManagedLaunchEvidence {
            tmux_pane_id: "%42".into(),
            provider_process: ProcessIdentity {
                pid: 42,
                started_at_ms: 41_000,
            },
        })
    }
}

struct AssertLaunchPersistedBeforeTerminal {
    store_path: PathBuf,
    opened: Arc<Mutex<Vec<String>>>,
}

impl ManagedTerminalAccess for AssertLaunchPersistedBeforeTerminal {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        let persisted = ManagedLaunchStore::open(&self.store_path)?
            .get(&registration.id)?
            .expect("registration must exist before Terminal access");
        assert!(persisted.launched_at_ms.is_some());
        assert!(persisted.launch_evidence.is_some());
        assert!(persisted.terminal_opened_at_ms.is_none());
        self.opened.lock().unwrap().push(registration.id.clone());
        Ok(())
    }
}

#[test]
fn selected_context_new_persists_binding_launch_before_opening_terminal() {
    let temp = TempDir::new("selected-context");
    let worktree = temp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let claude = executable(temp.path(), "claude");
    let codex = executable(temp.path(), "codex");
    let store_path = temp.path().join("managed-launches.db");
    let launched = Arc::new(Mutex::new(Vec::new()));
    let opened = Arc::new(Mutex::new(Vec::new()));
    let mut service = ManagedSessionService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        ProviderExecutables::new(claude, codex.clone()),
        Box::new(AssertPersistedBeforeLaunch {
            store_path: store_path.clone(),
            launched: launched.clone(),
        }),
        Box::new(AssertLaunchPersistedBeforeTerminal {
            store_path: store_path.clone(),
            opened: opened.clone(),
        }),
    );
    let request = ManagedNewRequest {
        provider: Provider::CodexCli,
        target: ManagedLaunchTarget {
            project: ProjectIdentity::GitCommonDir(temp.path().join("project/.git")),
            project_label: "project".into(),
            worktree_root: Some(worktree.clone()),
            working_directory: worktree.clone(),
        },
        title: "Investigate startup".into(),
        description: Some("Trace the selected-context path".into()),
        binding_initializer: false,
    };

    let outcome = service.start_new(request, 42_000).unwrap();

    assert!(outcome.terminal_opened);
    assert_eq!(
        *launched.lock().unwrap(),
        vec![outcome.registration.id.clone()]
    );
    assert_eq!(
        *opened.lock().unwrap(),
        vec![outcome.registration.id.clone()]
    );
    let persisted = ManagedLaunchStore::open(&store_path)
        .unwrap()
        .get(&outcome.registration.id)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.status.as_str(), "binding");
    assert_eq!(persisted.provider, Provider::CodexCli);
    assert_eq!(persisted.target.working_directory, worktree);
    assert_eq!(persisted.session_alias, "Investigate startup");
    assert_eq!(
        persisted.description.as_deref(),
        Some("Trace the selected-context path")
    );
    assert_eq!(persisted.provider_command.executable, codex);
    assert!(persisted.provider_command.arguments.is_empty());
    assert!(!persisted.binding_initializer);
    assert!(persisted.launched_at_ms.is_some());
    assert_eq!(
        persisted.launch_evidence,
        Some(ManagedLaunchEvidence {
            tmux_pane_id: "%42".into(),
            provider_process: ProcessIdentity {
                pid: 42,
                started_at_ms: 41_000,
            },
        })
    );
    assert!(persisted.terminal_opened_at_ms.is_some());
    assert!(persisted.terminal_error.is_none());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&store_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn private_tmux_broker_runs_the_selected_native_command_in_the_target_worktree() {
    let temp = TempDir::new("tmux-command");
    let worktree = temp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let captured_args = temp.path().join("tmux-args");
    let tmux = temp.path().join("tmux");
    fs::write(
        &tmux,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nsleep 30 >/dev/null 2>&1 &\nprintf '%%7\\t%s\\n' \"$!\"\n",
            captured_args.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(&tmux, fs::Permissions::from_mode(0o755)).unwrap();
    let claude = executable(temp.path(), "claude");
    let codex = executable(temp.path(), "codex");
    let store_path = temp.path().join("managed-launches.db");
    let opened = Arc::new(Mutex::new(Vec::new()));
    let mut service = ManagedSessionService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        ProviderExecutables::new(claude.clone(), codex),
        Box::new(TmuxLaunchBroker::new(tmux)),
        Box::new(AssertLaunchPersistedBeforeTerminal { store_path, opened }),
    );

    let outcome = service
        .start_new(
            ManagedNewRequest {
                provider: Provider::ClaudeCode,
                target: ManagedLaunchTarget {
                    project: ProjectIdentity::CanonicalCwd(worktree.clone()),
                    project_label: "worktree".into(),
                    worktree_root: None,
                    working_directory: worktree.clone(),
                },
                title: "Initializer contract".into(),
                description: None,
                binding_initializer: true,
            },
            43_000,
        )
        .unwrap();

    let arguments = fs::read_to_string(captured_args)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        vec![
            "-f".to_string(),
            "/dev/null".to_string(),
            "-L".to_string(),
            outcome.registration.tmux.socket_name,
            "new-session".to_string(),
            "-d".to_string(),
            "-P".to_string(),
            "-F".to_string(),
            "#{pane_id}\t#{pane_pid}".to_string(),
            "-s".to_string(),
            "managed".to_string(),
            "-c".to_string(),
            worktree.to_string_lossy().into_owned(),
            "--".to_string(),
            claude.to_string_lossy().into_owned(),
            BINDING_INITIALIZER.to_string(),
        ]
    );
    let provider_process = outcome
        .registration
        .launch_evidence
        .as_ref()
        .unwrap()
        .provider_process;
    let _ = std::process::Command::new("kill")
        .arg(provider_process.pid.to_string())
        .status();
}

struct FailTerminal;

impl ManagedTerminalAccess for FailTerminal {
    fn open(&mut self, _registration: &ManagedLaunchRegistration) -> Result<()> {
        anyhow::bail!("Terminal.app launch failed")
    }
}

#[test]
fn terminal_failure_preserves_the_single_running_binding_registration() {
    let temp = TempDir::new("terminal-failure");
    let worktree = temp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let claude = executable(temp.path(), "claude");
    let codex = executable(temp.path(), "codex");
    let store_path = temp.path().join("managed-launches.db");
    let launched = Arc::new(Mutex::new(Vec::new()));
    let mut service = ManagedSessionService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        ProviderExecutables::new(claude, codex),
        Box::new(AssertPersistedBeforeLaunch {
            store_path: store_path.clone(),
            launched: launched.clone(),
        }),
        Box::new(FailTerminal),
    );

    let outcome = service
        .start_new(
            ManagedNewRequest {
                provider: Provider::CodexCli,
                target: ManagedLaunchTarget {
                    project: ProjectIdentity::CanonicalCwd(worktree.clone()),
                    project_label: "worktree".into(),
                    worktree_root: None,
                    working_directory: worktree,
                },
                title: "Keep launch alive".into(),
                description: None,
                binding_initializer: false,
            },
            44_000,
        )
        .unwrap();

    assert!(!outcome.terminal_opened);
    assert_eq!(launched.lock().unwrap().len(), 1);
    assert!(outcome.registration.launched_at_ms.is_some());
    assert!(outcome.registration.launch_evidence.is_some());
    assert_eq!(
        outcome.registration.terminal_error.as_deref(),
        Some("Terminal.app launch failed")
    );
}

#[test]
fn terminal_access_only_attaches_to_the_existing_private_tmux_environment() {
    let temp = TempDir::new("terminal-attach");
    let worktree = temp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let claude = executable(temp.path(), "claude");
    let codex = executable(temp.path(), "codex");
    let tmux = executable(temp.path(), "tmux");
    let store_path = temp.path().join("managed-launches.db");
    let launched = Arc::new(Mutex::new(Vec::new()));
    let mut service = ManagedSessionService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        ProviderExecutables::new(claude, codex),
        Box::new(AssertPersistedBeforeLaunch {
            store_path,
            launched,
        }),
        Box::new(FailTerminal),
    );
    let outcome = service
        .start_new(
            ManagedNewRequest {
                provider: Provider::CodexCli,
                target: ManagedLaunchTarget {
                    project: ProjectIdentity::CanonicalCwd(worktree.clone()),
                    project_label: "worktree".into(),
                    worktree_root: None,
                    working_directory: worktree,
                },
                title: "Attach only".into(),
                description: None,
                binding_initializer: false,
            },
            45_000,
        )
        .unwrap();
    let captured_args = temp.path().join("osascript-args");
    let osascript = temp.path().join("osascript");
    fs::write(
        &osascript,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            captured_args.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(&osascript, fs::Permissions::from_mode(0o755)).unwrap();
    let mut terminal = MacosManagedTerminalAccess::with_osascript(tmux.clone(), osascript);

    terminal.open(&outcome.registration).unwrap();

    let arguments = fs::read_to_string(captured_args).unwrap();
    assert!(arguments.contains("tell application \"Terminal\""));
    assert!(arguments.contains("do script (item 1 of argv)"));
    assert!(arguments.contains(&format!(
        "'{}' -L '{}' attach-session -t 'managed'",
        tmux.display(),
        outcome.registration.tmux.socket_name
    )));
    assert!(!arguments.contains("codex"));
    assert!(!arguments.contains(BINDING_INITIALIZER));
}

struct NeverLaunchHandoff;

impl WorkerLauncher for NeverLaunchHandoff {
    fn launch(&self, _launch: &WorkerLaunch) -> std::io::Result<()> {
        panic!("Managed New must not start a Handoff Worker")
    }
}

struct NeverOpenSuccessor;

impl TerminalLauncher for NeverOpenSuccessor {
    fn open(&mut self, _task_id: &str, _now_ms: i64) -> Result<()> {
        panic!("Managed New must not use Successor Terminal access")
    }
}

struct NoopClipboard;

impl Clipboard for NoopClipboard {
    fn write_text(&mut self, _value: &str) -> std::io::Result<()> {
        Ok(())
    }
}

struct FakeDirectoryPicker {
    selected: VecDeque<PathBuf>,
}

impl DirectoryPicker for FakeDirectoryPicker {
    fn pick_directory(&mut self) -> Result<Option<PathBuf>> {
        Ok(self.selected.pop_front())
    }
}

#[test]
fn no_context_ui_flow_can_switch_the_picked_directory_before_managed_new() {
    let temp = TempDir::new("picked-directory");
    let selected = temp.path().join("notes");
    let selected_alias = temp.path().join("notes-alias");
    let replacement = temp.path().join("replacement-notes");
    let broken = temp.path().join("broken-worktree");
    fs::create_dir(&selected).unwrap();
    fs::create_dir(&replacement).unwrap();
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join(".git"), "gitdir: ../missing-git-dir\n").unwrap();
    std::os::unix::fs::symlink(&selected, &selected_alias).unwrap();
    let claude = executable(temp.path(), "claude");
    let codex = executable(temp.path(), "codex");
    let executables = ProviderExecutables::new(claude, codex);
    let store_path = temp.path().join("managed-launches.db");
    let launched = Arc::new(Mutex::new(Vec::new()));
    let opened = Arc::new(Mutex::new(Vec::new()));
    let managed_sessions = ManagedSessionService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        executables.clone(),
        Box::new(AssertPersistedBeforeLaunch {
            store_path: store_path.clone(),
            launched: launched.clone(),
        }),
        Box::new(AssertLaunchPersistedBeforeTerminal {
            store_path: store_path.clone(),
            opened: opened.clone(),
        }),
    );
    let handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        executables,
        NeverLaunchHandoff,
    )
    .unwrap();
    let mut controller = ApplicationController::new(handoff, NeverOpenSuccessor, managed_sessions)
        .with_directory_picker(Box::new(FakeDirectoryPicker {
            selected: VecDeque::from([broken, selected_alias.clone(), replacement.clone()]),
        }));
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let action = ui
        .handle_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
    let (rescan_tx, mut rescan_rx) = tokio::sync::mpsc::unbounded_channel();

    assert!(!controller.handle(action, &snapshot, &mut ui, &mut NoopClipboard, &rescan_tx,));

    assert!(rescan_rx.try_recv().is_err());
    let rejected = render_ui(&ui, &snapshot);
    assert!(
        rejected.contains("ambiguous Git identity"),
        "resolution error was not visible:\n{rejected}"
    );
    assert!(launched.lock().unwrap().is_empty());

    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let retry = ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert_eq!(retry, UiAction::ChooseManagedNewDirectory);
    assert!(!controller.handle(retry, &snapshot, &mut ui, &mut NoopClipboard, &rescan_tx,));
    let rendered = render_ui(&ui, &snapshot);
    assert!(
        rendered.contains("New Session"),
        "form did not open:\n{rendered}"
    );
    assert!(
        rendered.contains("notes"),
        "Project was not visible:\n{rendered}"
    );
    assert!(
        !rendered.contains(
            selected_alias
                .file_name()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        ),
        "directory alias was not canonicalized:\n{rendered}"
    );

    let switch = ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert_eq!(switch, UiAction::ChooseManagedNewDirectory);
    assert!(!controller.handle(switch, &snapshot, &mut ui, &mut NoopClipboard, &rescan_tx,));
    let switched = render_ui(&ui, &snapshot);
    assert!(
        switched.contains("replacement-notes"),
        "replacement context was not visible:\n{switched}"
    );

    ui.handle_key(key(KeyCode::BackTab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    for character in "Picked directory launch".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    let start = ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert!(matches!(start, UiAction::StartManagedNew { .. }));
    assert!(!controller.handle(start, &snapshot, &mut ui, &mut NoopClipboard, &rescan_tx,));

    let registration_id = launched.lock().unwrap().first().unwrap().clone();
    assert_eq!(*opened.lock().unwrap(), vec![registration_id.clone()]);
    let registration = ManagedLaunchStore::open(&store_path)
        .unwrap()
        .get(&registration_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        registration.target.working_directory,
        fs::canonicalize(replacement).unwrap()
    );
    assert_eq!(registration.session_alias, "Picked directory launch");
}

#[test]
fn selected_project_ui_flow_reaches_the_managed_session_application_service() {
    let temp = TempDir::new("application-flow");
    let project_root = std::env::current_dir().unwrap();
    let claude = executable(temp.path(), "claude");
    let codex = executable(temp.path(), "codex");
    let executables = ProviderExecutables::new(claude.clone(), codex);
    let store_path = temp.path().join("managed-launches.db");
    let launched = Arc::new(Mutex::new(Vec::new()));
    let opened = Arc::new(Mutex::new(Vec::new()));
    let managed_sessions = ManagedSessionService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        executables.clone(),
        Box::new(AssertPersistedBeforeLaunch {
            store_path: store_path.clone(),
            launched: launched.clone(),
        }),
        Box::new(AssertLaunchPersistedBeforeTerminal {
            store_path: store_path.clone(),
            opened: opened.clone(),
        }),
    );
    let handoff = HandoffService::open(
        HandoffPaths {
            task_store: temp.path().join("tasks.db"),
            artifact_root: temp.path().join("artifacts"),
        },
        executables,
        NeverLaunchHandoff,
    )
    .unwrap();
    let mut controller = ApplicationController::new(handoff, NeverOpenSuccessor, managed_sessions);
    let snapshot = Snapshot {
        sessions: vec![session_in_project(&project_root)],
        ready: true,
        ..Snapshot::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut ui = Ui::default();
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    let new_form = render_ui(&ui, &snapshot);
    assert!(
        new_form.contains("New Session"),
        "New form did not open:\n{new_form}"
    );
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    for character in "Application launch".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    let intent = ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert!(
        matches!(intent, UiAction::StartManagedNew { .. }),
        "unexpected UI intent: {intent:?}"
    );

    let (rescan_tx, mut rescan_rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(!controller.handle(intent, &snapshot, &mut ui, &mut NoopClipboard, &rescan_tx,));

    assert!(rescan_rx.try_recv().is_ok());
    let registration_id = launched.lock().unwrap().first().unwrap().clone();
    assert_eq!(*opened.lock().unwrap(), vec![registration_id.clone()]);
    let registration = ManagedLaunchStore::open(&store_path)
        .unwrap()
        .get(&registration_id)
        .unwrap()
        .unwrap();
    assert_eq!(registration.provider, Provider::ClaudeCode);
    assert_eq!(registration.target.working_directory, project_root);
    assert_eq!(registration.session_alias, "Application launch");
    assert_eq!(registration.provider_command.executable, claude);
    assert!(registration.terminal_opened_at_ms.is_some());
}

fn render_ui(ui: &Ui, snapshot: &Snapshot) -> String {
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

fn session_in_project(project_root: &Path) -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: "observed-session".into(),
        source: SourceMetadata::default(),
        project: Some(Project {
            identity: ProjectIdentity::CanonicalCwd(project_root.to_path_buf()),
            label: "project".into(),
        }),
        worktree: None,
        cwd: Some(project_root.to_string_lossy().into_owned()),
        latest_model: None,
        state: SessionState::Idle,
        context: ContextState::default(),
        usage: Usage::default(),
        per_model: Vec::new(),
        current_run_usage: None,
        compactions: Vec::new(),
        last_activity_ms: 1_000,
        cost: None,
    }
}

fn executable(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
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
            "sessionmesh-managed-new-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
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
