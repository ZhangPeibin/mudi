use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sessionmesh::app::Snapshot;
use sessionmesh::config::Config;
use sessionmesh::domain::{
    ContextState, ProjectIdentity, Provider, Session, SessionProvenance, SessionState,
    SourceFormat, SourceMetadata, Usage,
};
use sessionmesh::managed_sessions::{
    BindingOutcome, EvidenceBinder, ManagedLaunchAction, ManagedLaunchCleanup,
    ManagedLaunchEvidence, ManagedLaunchRegistration, ManagedLaunchStatus, ManagedLaunchStore,
    ManagedLaunchTarget, PrivateTmuxEnvironment, ProcessEvidence, ProviderNewCommand,
    ProviderProcessObservation, SystemProcessEvidence,
};
use sessionmesh::providers::state::ProcessIdentity;
use sessionmesh::providers::{registry, ProviderBindingEvidence};
use sessionmesh::read_model::{ManagedLaunchSummary, ManagedTerminalState};
use sessionmesh::refresh::RefreshService;
use sessionmesh::session_labels::SessionLabels;
use sessionmesh::ui::{draw, Ui, UiAction};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
struct FixedProcessEvidence(Option<ProcessIdentity>);

impl ProcessEvidence for FixedProcessEvidence {
    fn observe(&self, _pid: u32) -> ProviderProcessObservation {
        self.0.map_or(
            ProviderProcessObservation::Unknown,
            ProviderProcessObservation::Live,
        )
    }
}

#[derive(Clone)]
struct MappedProcessEvidence(HashMap<u32, ProcessIdentity>);

impl ProcessEvidence for MappedProcessEvidence {
    fn observe(&self, pid: u32) -> ProviderProcessObservation {
        self.0.get(&pid).copied().map_or(
            ProviderProcessObservation::Absent,
            ProviderProcessObservation::Live,
        )
    }
}

struct NoopCleanup;

impl ManagedLaunchCleanup for NoopCleanup {
    fn clean_empty(&self, _registration: &ManagedLaunchRegistration) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn binding_receipt_atomically_establishes_the_managed_session_and_current_run() {
    let temp = TempDir::new("atomic-receipt");
    let worktree = temp.directory("worktree");
    let source_path = temp.file("rollout.jsonl", "{}\n");
    let process = process(42, 41_000);
    let registration = registration("launch-atomic", Provider::CodexCli, &worktree, process);
    let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(Some(process))),
        Box::new(NoopCleanup),
    );

    let outcome = binder
        .bind(
            &mut store,
            &registration.id,
            Some(binding_evidence(
                Provider::CodexCli,
                "native-session",
                "native-session",
                &source_path,
                &worktree,
                process,
            )),
            50_000,
        )
        .unwrap();

    let BindingOutcome::Bound(receipt) = outcome else {
        panic!("complete agreeing evidence must issue a Binding Receipt");
    };
    assert_eq!(receipt.registration_id, registration.id);
    assert!(store.get(&registration.id).unwrap().is_none());
    let managed = store
        .managed_session(Provider::CodexCli, "native-session")
        .unwrap()
        .expect("receipt must establish the Managed Session");
    assert_eq!(managed.session_alias, "Managed work");
    assert_eq!(managed.binding_receipt_id, receipt.id);
    let current_run = store
        .managed_current_run(Provider::CodexCli, "native-session")
        .unwrap()
        .expect("receipt must establish the Managed Current Run");
    assert_eq!(current_run.provider_process, process);
    assert_eq!(current_run.binding_receipt_id, receipt.id);

    let mut sessions = vec![observed_session(
        Provider::CodexCli,
        "native-session",
        &source_path,
        &worktree,
    )];
    store.apply_provenance(&mut sessions).unwrap();
    assert_eq!(sessions[0].provenance, SessionProvenance::Managed);
}

#[test]
fn provider_descendant_of_the_launched_pane_can_issue_a_binding_receipt() {
    let temp = TempDir::new("provider-descendant");
    let worktree = temp.directory("worktree");
    let source_path = temp.file("rollout.jsonl", "{}\n");
    let pane_process = process(42, 41_000);
    let provider_process = process(43, 41_001);
    let registration = registration(
        "launch-provider-descendant",
        Provider::CodexCli,
        &worktree,
        pane_process,
    );
    let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(MappedProcessEvidence(HashMap::from([
            (pane_process.pid, pane_process),
            (provider_process.pid, provider_process),
        ]))),
        Box::new(NoopCleanup),
    );

    let outcome = binder
        .bind(
            &mut store,
            &registration.id,
            Some(binding_evidence_with_launch(
                Provider::CodexCli,
                "native-session",
                "native-session",
                &source_path,
                &worktree,
                pane_process,
                provider_process,
            )),
            50_000,
        )
        .unwrap();

    let BindingOutcome::Bound(receipt) = outcome else {
        panic!("a verified provider descendant must bind");
    };
    assert_eq!(receipt.provider_process, provider_process);
    assert_eq!(
        store
            .managed_current_run(Provider::CodexCli, "native-session")
            .unwrap()
            .unwrap()
            .provider_process,
        provider_process
    );
}

#[test]
fn provider_evidence_from_a_different_launch_root_cannot_issue_a_binding_receipt() {
    let temp = TempDir::new("different-launch-root");
    let worktree = temp.directory("worktree");
    let source_path = temp.file("rollout.jsonl", "{}\n");
    let pane_process = process(42, 41_000);
    let unrelated_launch_process = process(43, 41_001);
    let provider_process = process(44, 41_002);
    let registration = registration(
        "launch-different-root",
        Provider::CodexCli,
        &worktree,
        pane_process,
    );
    let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store.insert(&registration).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(MappedProcessEvidence(HashMap::from([
            (pane_process.pid, pane_process),
            (unrelated_launch_process.pid, unrelated_launch_process),
            (provider_process.pid, provider_process),
        ]))),
        Box::new(NoopCleanup),
    );

    let outcome = binder
        .bind(
            &mut store,
            &registration.id,
            Some(binding_evidence_with_launch(
                Provider::CodexCli,
                "native-session",
                "native-session",
                &source_path,
                &worktree,
                unrelated_launch_process,
                provider_process,
            )),
            50_000,
        )
        .unwrap();

    assert_eq!(outcome, BindingOutcome::NeedsReview);
    assert!(store
        .managed_session(Provider::CodexCli, "native-session")
        .unwrap()
        .is_none());
}

#[test]
fn missing_or_disagreeing_evidence_cannot_create_management() {
    let cases = [
        InvalidEvidence::MissingSource,
        InvalidEvidence::ProviderMismatch,
        InvalidEvidence::NativeSessionMismatch,
        InvalidEvidence::LaunchProcessMismatch,
        InvalidEvidence::WorkingDirectoryMismatch,
        InvalidEvidence::StalePid,
        InvalidEvidence::ReusedPid,
    ];

    for case in cases {
        let temp = TempDir::new(case.name());
        let worktree = temp.directory("worktree");
        let other_worktree = temp.directory("other-worktree");
        let source_path = temp.file("source.jsonl", "{}\n");
        let launch_process = process(42, 41_000);
        let registration = registration(
            &format!("launch-{}", case.name()),
            Provider::CodexCli,
            &worktree,
            launch_process,
        );
        let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
        store.insert(&registration).unwrap();
        let current_process = match case {
            InvalidEvidence::StalePid => None,
            InvalidEvidence::ReusedPid => Some(process(42, 49_000)),
            _ => Some(launch_process),
        };
        let binder = EvidenceBinder::new(
            Box::new(FixedProcessEvidence(current_process)),
            Box::new(NoopCleanup),
        );
        let evidence = match case {
            InvalidEvidence::MissingSource => None,
            InvalidEvidence::ProviderMismatch => Some(binding_evidence(
                Provider::ClaudeCode,
                "native-session",
                "native-session",
                &source_path,
                &worktree,
                launch_process,
            )),
            InvalidEvidence::NativeSessionMismatch => Some(binding_evidence(
                Provider::CodexCli,
                "source-session",
                "process-session",
                &source_path,
                &worktree,
                launch_process,
            )),
            InvalidEvidence::LaunchProcessMismatch => Some(binding_evidence(
                Provider::CodexCli,
                "native-session",
                "native-session",
                &source_path,
                &worktree,
                process(43, 41_000),
            )),
            InvalidEvidence::WorkingDirectoryMismatch => Some(binding_evidence(
                Provider::CodexCli,
                "native-session",
                "native-session",
                &source_path,
                &other_worktree,
                launch_process,
            )),
            InvalidEvidence::StalePid | InvalidEvidence::ReusedPid => Some(binding_evidence(
                Provider::CodexCli,
                "native-session",
                "native-session",
                &source_path,
                &worktree,
                launch_process,
            )),
        };

        let outcome = binder.bind(&mut store, &registration.id, evidence, 50_000);

        if matches!(case, InvalidEvidence::MissingSource) {
            assert_eq!(outcome.unwrap(), BindingOutcome::Waiting, "{case:?}");
        } else {
            assert_eq!(outcome.unwrap(), BindingOutcome::NeedsReview, "{case:?}");
        }
        let retained = store.get(&registration.id).unwrap().unwrap();
        let expected_status = if matches!(case, InvalidEvidence::MissingSource) {
            ManagedLaunchStatus::Binding
        } else {
            ManagedLaunchStatus::NeedsReview
        };
        assert_eq!(retained.status, expected_status, "{case:?}");
        assert!(
            store
                .managed_session(Provider::CodexCli, "native-session")
                .unwrap()
                .is_none(),
            "{case:?} created a Managed Session"
        );
    }
}

#[test]
fn conflicting_managed_identity_rolls_back_and_needs_review() {
    let temp = TempDir::new("rollback");
    let worktree = temp.directory("worktree");
    let source_path = temp.file("source.jsonl", "{}\n");
    let process = process(42, 41_000);
    let mut store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(Some(process))),
        Box::new(NoopCleanup),
    );

    for registration_id in ["launch-first", "launch-conflict"] {
        store
            .insert(&registration(
                registration_id,
                Provider::ClaudeCode,
                &worktree,
                process,
            ))
            .unwrap();
    }
    binder
        .bind(
            &mut store,
            "launch-first",
            Some(binding_evidence(
                Provider::ClaudeCode,
                "same-native-session",
                "same-native-session",
                &source_path,
                &worktree,
                process,
            )),
            50_000,
        )
        .unwrap();

    let conflict = binder.bind(
        &mut store,
        "launch-conflict",
        Some(binding_evidence(
            Provider::ClaudeCode,
            "same-native-session",
            "same-native-session",
            &source_path,
            &worktree,
            process,
        )),
        51_000,
    );

    assert_eq!(conflict.unwrap(), BindingOutcome::NeedsReview);
    assert_eq!(
        store.get("launch-conflict").unwrap().unwrap().status,
        ManagedLaunchStatus::NeedsReview
    );
    assert!(store.binding_receipt("launch-conflict").unwrap().is_none());
    assert_eq!(store.managed_sessions().unwrap().len(), 1);
    assert_eq!(store.managed_current_runs().unwrap().len(), 1);
}

#[test]
fn claude_adapter_normalizes_first_prompt_source_and_exact_process_evidence() {
    let temp = TempDir::new("claude-adapter");
    let worktree = temp.directory("worktree");
    let claude_dir = temp.path().join("claude");
    let transcripts = claude_dir.join("projects/project");
    let states = claude_dir.join("sessions");
    fs::create_dir_all(&transcripts).unwrap();
    fs::create_dir_all(&states).unwrap();
    let process = SystemProcessEvidence
        .current_identity(std::process::id())
        .unwrap();
    let native_session_id = "claude-native-session";
    let source_path = transcripts.join(format!("{native_session_id}.jsonl"));
    fs::write(
        &source_path,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "sessionId": native_session_id,
                "cwd": worktree,
                "message": {"role": "user", "content": "first real prompt"}
            })
        ),
    )
    .unwrap();
    fs::write(
        states.join(format!("{}.json", process.pid)),
        serde_json::json!({
            "pid": process.pid,
            "sessionId": native_session_id,
            "status": "busy",
            "startedAt": process.started_at_ms
        })
        .to_string(),
    )
    .unwrap();
    let config = Config {
        claude_dir,
        codex_dir: temp.path().join("codex"),
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };

    let evidence = registry()
        .runtime_control(Provider::ClaudeCode)
        .unwrap()
        .binding_evidence(&config, process)
        .expect("first Prompt and exact process evidence should normalize");

    assert_eq!(evidence.provider, Provider::ClaudeCode);
    assert_eq!(evidence.source_session_id, native_session_id);
    assert_eq!(evidence.process_session_id, native_session_id);
    assert_eq!(evidence.source.path, source_path.canonicalize().unwrap());
    assert_eq!(evidence.working_directory, worktree);
    assert_eq!(evidence.provider_process, process);
}

#[cfg(unix)]
#[test]
fn codex_adapter_normalizes_the_rollout_held_by_the_exact_process() {
    let temp = TempDir::new("codex-adapter");
    let worktree = temp.directory("worktree");
    let codex_dir = temp.path().join("codex");
    let rollouts = codex_dir.join("sessions/2026/08/10");
    fs::create_dir_all(&rollouts).unwrap();
    let native_session_id = "11111111-1111-1111-1111-111111111111";
    let source_path = rollouts.join(format!(
        "rollout-2026-08-10T00-00-00-{native_session_id}.jsonl"
    ));
    fs::write(
        &source_path,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "session_meta",
                "payload": {"id": native_session_id, "cwd": worktree}
            })
        ),
    )
    .unwrap();
    let _held_source = fs::File::open(&source_path).unwrap();
    let process = SystemProcessEvidence
        .current_identity(std::process::id())
        .unwrap();
    let config = Config {
        claude_dir: temp.path().join("claude"),
        codex_dir,
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };

    let evidence = registry()
        .runtime_control(Provider::CodexCli)
        .unwrap()
        .binding_evidence(&config, process)
        .expect("the exact process-held rollout should normalize");

    assert_eq!(evidence.provider, Provider::CodexCli);
    assert_eq!(evidence.source_session_id, native_session_id);
    assert_eq!(evidence.process_session_id, native_session_id);
    assert_eq!(evidence.source.path, source_path.canonicalize().unwrap());
    assert_eq!(evidence.working_directory, worktree);
    assert_eq!(evidence.launch_process, process);
    assert_eq!(evidence.provider_process, process);
}

#[cfg(unix)]
#[test]
fn codex_adapter_resolves_the_rollout_holder_below_a_launch_wrapper() {
    let temp = TempDir::new("codex-wrapper-adapter");
    let worktree = temp.directory("worktree");
    let codex_dir = temp.path().join("codex");
    let rollouts = codex_dir.join("sessions/2026/08/11");
    fs::create_dir_all(&rollouts).unwrap();
    let native_session_id = "11111111-1111-1111-1111-111111111111";
    let source_path = rollouts.join(format!(
        "rollout-2026-08-11T00-00-00-{native_session_id}.jsonl"
    ));
    fs::write(
        &source_path,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "session_meta",
                "payload": {"id": native_session_id, "cwd": worktree}
            })
        ),
    )
    .unwrap();
    let mut wrapper = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            r#"tail -f "$1" >/dev/null 2>&1 & child=$!; trap 'kill "$child" 2>/dev/null; wait "$child"; exit 0' TERM INT; wait "$child""#,
            "sessionmesh-wrapper",
        ])
        .arg(&source_path)
        .spawn()
        .unwrap();
    let pane_process = SystemProcessEvidence
        .current_identity(wrapper.id())
        .expect("wrapper process identity should be available");
    let config = Config {
        claude_dir: temp.path().join("claude"),
        codex_dir,
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };
    let evidence = (0..100).find_map(|_| {
        let evidence = registry()
            .runtime_control(Provider::CodexCli)
            .unwrap()
            .binding_evidence(&config, pane_process);
        if evidence.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        evidence
    });
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &wrapper.id().to_string()])
        .status();
    let _ = wrapper.wait();
    let evidence = evidence.expect("the wrapper child that holds the rollout must normalize");

    assert_eq!(evidence.launch_process, pane_process);
    assert_ne!(evidence.provider_process, pane_process);
    assert_eq!(evidence.source.path, source_path.canonicalize().unwrap());
    assert_eq!(evidence.source_session_id, native_session_id);
}

#[test]
fn refresh_publishes_metadata_only_managed_launch_recovery_state() {
    let temp = TempDir::new("managed-launch-read-model");
    let worktree = temp.directory("worktree");
    let config = Config {
        claude_dir: temp.path().join("claude"),
        codex_dir: temp.path().join("codex"),
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };
    let mut registration = registration(
        "launch-needs-review",
        Provider::CodexCli,
        &worktree,
        process(42, 41_000),
    );
    registration.status = ManagedLaunchStatus::NeedsReview;
    registration.session_alias = "Recover payment tests".into();
    registration.description = Some("private launch description".into());
    registration.binding_initializer = true;
    registration.provider_command.arguments = vec!["private initializer content".into()];
    registration.terminal_opened_at_ms = None;
    registration.terminal_error = Some("Terminal.app denied automation".into());
    registration.binding_error = Some("native Session evidence conflicts".into());
    ManagedLaunchStore::open(config.managed_launch_store_path())
        .unwrap()
        .insert(&registration)
        .unwrap();

    let snapshot = RefreshService::new(config).refresh();

    assert_eq!(snapshot.managed_launches.len(), 1);
    let launch = &snapshot.managed_launches[0];
    assert_eq!(launch.id, "launch-needs-review");
    assert_eq!(launch.status, ManagedLaunchStatus::NeedsReview);
    assert_eq!(launch.provider, Provider::CodexCli);
    assert_eq!(launch.project_label, registration.target.project_label);
    assert_eq!(launch.worktree_root, registration.target.worktree_root);
    assert_eq!(launch.working_directory, worktree);
    assert_eq!(launch.title, "Recover payment tests");
    assert_eq!(launch.created_at_ms, 40_000);
    assert_eq!(launch.terminal_state, ManagedTerminalState::Error);
    assert_eq!(
        launch.terminal_error.as_deref(),
        Some("Terminal.app denied automation")
    );
    assert_eq!(
        launch.binding_error.as_deref(),
        Some("native Session evidence conflicts")
    );
    assert_eq!(
        launch.available_actions,
        vec![
            ManagedLaunchAction::RetryOpenTerminal,
            ManagedLaunchAction::CopyDiagnostics,
        ]
    );
    let debug_projection = format!("{launch:?}");
    for private in [
        "private launch description",
        "private initializer content",
        "/usr/bin/true",
        "%42",
    ] {
        assert!(!debug_projection.contains(private), "leaked {private:?}");
    }
}

#[test]
fn managed_start_region_renders_each_state_with_only_legal_actions() {
    let cases = [
        (
            launch_summary(
                "R-BIND",
                ManagedLaunchStatus::Binding,
                ManagedTerminalState::Opened,
                None,
                None,
                vec![ManagedLaunchAction::CancelStart],
                30_000,
            ),
            ["BINDING", "x Cancel Start", "Terminal open"].as_slice(),
            ["Retry Terminal", "c Copy Diagnostics", "o Open Terminal"].as_slice(),
        ),
        (
            launch_summary(
                "R-REVIEW",
                ManagedLaunchStatus::NeedsReview,
                ManagedTerminalState::Opened,
                None,
                Some("native evidence conflicts"),
                vec![
                    ManagedLaunchAction::OpenTerminal,
                    ManagedLaunchAction::CopyDiagnostics,
                ],
                20_000,
            ),
            ["NEEDS REVIEW", "o Open Terminal", "c Copy Diagnostics"].as_slice(),
            ["x Cancel Start", "Retry Binding", "Resume", "Adopt"].as_slice(),
        ),
        (
            launch_summary(
                "R-FAILED",
                ManagedLaunchStatus::Failed,
                ManagedTerminalState::Pending,
                None,
                Some("Provider process absent"),
                Vec::new(),
                10_000,
            ),
            ["FAILED", "Provider process absent", "No recovery action"].as_slice(),
            ["x Cancel Start", "Retry Terminal", "c Copy Diagnostics"].as_slice(),
        ),
    ];

    for (launch, expected, forbidden) in cases {
        let snapshot = Snapshot {
            managed_launches: vec![launch],
            ready: true,
            ..Snapshot::default()
        };
        let rendered = render_ui(&Ui::default(), &snapshot);
        assert!(
            rendered.contains("MANAGED START"),
            "missing region:\n{rendered}"
        );
        for expected in expected {
            assert!(
                rendered.contains(expected),
                "missing {expected:?}:\n{rendered}"
            );
        }
        for forbidden in forbidden {
            assert!(
                !rendered.contains(forbidden),
                "rendered forbidden {forbidden:?}:\n{rendered}"
            );
        }
    }
}

#[test]
fn compact_managed_start_region_preserves_navigation_actions_and_neighbor_panels() {
    let snapshot = Snapshot {
        managed_launches: vec![
            launch_summary(
                "R-REVIEW",
                ManagedLaunchStatus::NeedsReview,
                ManagedTerminalState::Error,
                Some("denied"),
                Some("conflicting evidence"),
                vec![
                    ManagedLaunchAction::RetryOpenTerminal,
                    ManagedLaunchAction::CopyDiagnostics,
                ],
                30_000,
            ),
            launch_summary(
                "R-FAILED",
                ManagedLaunchStatus::Failed,
                ManagedTerminalState::Pending,
                None,
                Some("absent"),
                Vec::new(),
                20_000,
            ),
        ],
        ready: true,
        ..Snapshot::default()
    };
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| draw(frame, &snapshot, &Ui::default()))
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    for expected in [
        "MANAGED START [/]",
        "o Retry Terminal",
        "c Copy Diagnostics",
        "RECENT SIGNALS",
        "TASKS",
    ] {
        assert!(
            rendered.contains(expected),
            "compact status band lost {expected:?}:\n{rendered}"
        );
    }
}

#[test]
fn managed_start_selection_stays_on_registration_identity_across_refresh_ordering() {
    let first = launch_summary(
        "R-FIRST",
        ManagedLaunchStatus::Binding,
        ManagedTerminalState::Opened,
        None,
        None,
        vec![ManagedLaunchAction::CancelStart],
        30_000,
    );
    let second = launch_summary(
        "R-SECOND",
        ManagedLaunchStatus::NeedsReview,
        ManagedTerminalState::Opened,
        None,
        Some("review"),
        vec![
            ManagedLaunchAction::OpenTerminal,
            ManagedLaunchAction::CopyDiagnostics,
        ],
        20_000,
    );
    let mut snapshot = Snapshot {
        managed_launches: vec![first.clone(), second.clone()],
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    ui.clamp_selections(&snapshot);

    assert_eq!(ui.handle_key(key(']'), &snapshot).unwrap(), UiAction::None);
    let selected_second = render_ui(&ui, &snapshot);
    assert!(selected_second.contains("R-SECOND"));
    assert!(selected_second.contains("2/2"));

    snapshot.revision = 2;
    snapshot.managed_launches = vec![second, first];
    ui.clamp_selections(&snapshot);
    let reordered = render_ui(&ui, &snapshot);
    assert!(reordered.contains("R-SECOND"));
    assert!(reordered.contains("1/2"));
}

#[test]
fn managed_start_keys_emit_only_actions_allowed_by_the_selected_registration() {
    let mut ui = Ui::default();
    let binding = Snapshot {
        managed_launches: vec![launch_summary(
            "R-BIND",
            ManagedLaunchStatus::Binding,
            ManagedTerminalState::Error,
            Some("denied"),
            None,
            vec![
                ManagedLaunchAction::CancelStart,
                ManagedLaunchAction::RetryOpenTerminal,
            ],
            30_000,
        )],
        ready: true,
        ..Snapshot::default()
    };
    assert_eq!(
        ui.handle_key(key('x'), &binding).unwrap(),
        UiAction::CancelManagedStart {
            registration_id: "R-BIND".into()
        }
    );
    assert_eq!(
        ui.handle_key(key('o'), &binding).unwrap(),
        UiAction::RetryManagedTerminal {
            registration_id: "R-BIND".into()
        }
    );
    assert_eq!(ui.handle_key(key('c'), &binding).unwrap(), UiAction::None);

    let review = Snapshot {
        managed_launches: vec![launch_summary(
            "R-REVIEW",
            ManagedLaunchStatus::NeedsReview,
            ManagedTerminalState::Opened,
            None,
            Some("review"),
            vec![
                ManagedLaunchAction::OpenTerminal,
                ManagedLaunchAction::CopyDiagnostics,
            ],
            20_000,
        )],
        ready: true,
        ..Snapshot::default()
    };
    assert_eq!(ui.handle_key(key('x'), &review).unwrap(), UiAction::None);
    assert_eq!(
        ui.handle_key(key('o'), &review).unwrap(),
        UiAction::OpenManagedTerminal {
            registration_id: "R-REVIEW".into()
        }
    );
    assert_eq!(
        ui.handle_key(key('c'), &review).unwrap(),
        UiAction::CopyManagedLaunchDiagnostics {
            registration_id: "R-REVIEW".into()
        }
    );

    let failed = Snapshot {
        managed_launches: vec![launch_summary(
            "R-FAILED",
            ManagedLaunchStatus::Failed,
            ManagedTerminalState::Pending,
            None,
            Some("absent"),
            Vec::new(),
            10_000,
        )],
        ready: true,
        ..Snapshot::default()
    };
    for character in ['x', 'o', 'c'] {
        assert_eq!(
            ui.handle_key(key(character), &failed).unwrap(),
            UiAction::None
        );
    }
}

fn key(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
}

fn launch_summary(
    id: &str,
    status: ManagedLaunchStatus,
    terminal_state: ManagedTerminalState,
    terminal_error: Option<&str>,
    binding_error: Option<&str>,
    available_actions: Vec<ManagedLaunchAction>,
    created_at_ms: i64,
) -> ManagedLaunchSummary {
    ManagedLaunchSummary {
        id: id.into(),
        status,
        provider: Provider::CodexCli,
        project: ProjectIdentity::CanonicalCwd(PathBuf::from("/workspace/sessionmesh")),
        project_label: "sessionMesh".into(),
        worktree_root: Some(PathBuf::from("/workspace/sessionmesh/main")),
        working_directory: PathBuf::from("/workspace/sessionmesh/main"),
        title: "Work Continuity".into(),
        created_at_ms,
        terminal_state,
        terminal_error: terminal_error.map(str::to_string),
        binding_error: binding_error.map(str::to_string),
        available_actions,
    }
}

#[test]
fn claude_first_prompt_refresh_replaces_binding_with_a_managed_project_row() {
    let temp = TempDir::new("claude-refresh");
    let worktree = std::env::current_dir().unwrap().canonicalize().unwrap();
    let claude_dir = temp.path().join("claude");
    let transcripts = claude_dir.join("projects/project");
    let states = claude_dir.join("sessions");
    fs::create_dir_all(&transcripts).unwrap();
    fs::create_dir_all(&states).unwrap();
    let process = SystemProcessEvidence
        .current_identity(std::process::id())
        .unwrap();
    let native_session_id = "claude-refresh-session";
    let source_path = transcripts.join(format!("{native_session_id}.jsonl"));
    fs::write(
        &source_path,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "sessionId": native_session_id,
                "cwd": worktree,
                "timestamp": "2026-08-10T10:00:00.000Z",
                "message": {
                    "id": "first-response",
                    "model": "claude-sonnet-4-5",
                    "usage": {"input_tokens": 10, "output_tokens": 2}
                }
            })
        ),
    )
    .unwrap();
    fs::write(
        states.join(format!("{}.json", process.pid)),
        serde_json::json!({
            "pid": process.pid,
            "sessionId": native_session_id,
            "status": "idle",
            "startedAt": process.started_at_ms
        })
        .to_string(),
    )
    .unwrap();
    let config = Config {
        claude_dir,
        codex_dir: temp.path().join("codex"),
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };
    insert_binding_registration(
        &config,
        "launch-claude-refresh",
        Provider::ClaudeCode,
        &worktree,
        process,
    );
    let mut ui = load_ui_before_binding(&config);

    let mut refresh = RefreshService::new(config.clone());
    let snapshot = refresh.refresh();

    assert_managed_refresh(
        &config,
        &snapshot,
        &mut ui,
        Provider::ClaudeCode,
        native_session_id,
        &worktree,
        "launch-claude-refresh",
    );

    let mut labels = SessionLabels::load(config.label_store_path());
    labels
        .set_metadata(
            Provider::ClaudeCode,
            native_session_id,
            "Edited managed work",
            "Edited after binding",
        )
        .unwrap();
    refresh.refresh();
    let labels = SessionLabels::load(config.label_store_path());
    assert_eq!(
        labels.alias(Provider::ClaudeCode, native_session_id),
        Some("Edited managed work")
    );
    assert_eq!(
        labels.description(Provider::ClaudeCode, native_session_id),
        Some("Edited after binding")
    );
}

#[cfg(unix)]
#[test]
fn codex_first_prompt_refresh_replaces_binding_with_a_managed_project_row() {
    let temp = TempDir::new("codex-refresh");
    let worktree = std::env::current_dir().unwrap().canonicalize().unwrap();
    let codex_dir = temp.path().join("codex");
    let rollouts = codex_dir.join("sessions/2026/08/10");
    fs::create_dir_all(&rollouts).unwrap();
    let process = SystemProcessEvidence
        .current_identity(std::process::id())
        .unwrap();
    let native_session_id = "22222222-2222-2222-2222-222222222222";
    let source_path = rollouts.join(format!(
        "rollout-2026-08-10T00-00-00-{native_session_id}.jsonl"
    ));
    let records = [
        serde_json::json!({
            "timestamp": "2026-08-10T10:00:00.000Z",
            "type": "session_meta",
            "payload": {"id": native_session_id, "cwd": worktree}
        }),
        serde_json::json!({
            "timestamp": "2026-08-10T10:00:01.000Z",
            "type": "turn_context",
            "payload": {"model": "gpt-5.6-sol", "cwd": worktree}
        }),
        serde_json::json!({
            "timestamp": "2026-08-10T10:00:02.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {"input_tokens": 10, "output_tokens": 2},
                    "total_token_usage": {
                        "input_tokens": 10,
                        "cached_input_tokens": 0,
                        "output_tokens": 2
                    },
                    "model_context_window": 200000
                }
            }
        }),
    ];
    fs::write(
        &source_path,
        records
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let _held_source = fs::File::open(&source_path).unwrap();
    let config = Config {
        claude_dir: temp.path().join("claude"),
        codex_dir,
        data_dir: temp.path().join("data"),
        extra_dirs: Vec::new(),
    };
    insert_binding_registration(
        &config,
        "launch-codex-refresh",
        Provider::CodexCli,
        &worktree,
        process,
    );
    let mut ui = load_ui_before_binding(&config);

    let snapshot = RefreshService::new(config.clone()).refresh();

    assert_managed_refresh(
        &config,
        &snapshot,
        &mut ui,
        Provider::CodexCli,
        native_session_id,
        &worktree,
        "launch-codex-refresh",
    );
}

fn insert_binding_registration(
    config: &Config,
    registration_id: &str,
    provider: Provider,
    worktree: &Path,
    process: ProcessIdentity,
) {
    let store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
    store
        .insert(&registration(registration_id, provider, worktree, process))
        .unwrap();
}

fn assert_managed_refresh(
    config: &Config,
    snapshot: &Snapshot,
    ui: &mut Ui,
    provider: Provider,
    native_session_id: &str,
    worktree: &Path,
    registration_id: &str,
) {
    let session = snapshot
        .sessions
        .iter()
        .find(|session| session.provider == provider && session.id == native_session_id)
        .expect("verified Session should appear in Session inventory");
    let expected = sessionmesh::project::resolve(Some(worktree));
    assert_eq!(session.provenance, SessionProvenance::Managed);
    assert_eq!(
        session.project.as_ref().map(|project| &project.identity),
        expected.project.as_ref().map(|project| &project.identity)
    );
    assert_eq!(
        session.worktree.as_ref().map(|worktree| &worktree.root),
        expected.worktree.as_ref().map(|worktree| &worktree.root)
    );
    let store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
    assert!(store.get(registration_id).unwrap().is_none());
    assert!(store.binding_receipt(registration_id).unwrap().is_some());
    let labels = SessionLabels::load(config.label_store_path());
    assert_eq!(
        labels.alias(provider, native_session_id),
        Some("Managed work")
    );
    assert_eq!(
        labels.description(provider, native_session_id),
        Some("Bound only after exact evidence")
    );
    ui.clamp_selections(snapshot);
    if session.state == SessionState::Exited {
        let rendered = render_ui(ui, snapshot);
        assert!(
            rendered.contains("No Open Sessions"),
            "Exited Managed Session should not create a home-page Project row:\n{rendered}"
        );
        return;
    }
    let rendered = render_ui(ui, snapshot);
    let project_label = expected.project.unwrap().label;
    assert!(
        rendered.contains(&project_label),
        "Project row missing:\n{rendered}"
    );
    assert!(
        rendered.contains("Managed work") && rendered.contains('▦'),
        "verified Managed row missing:\n{rendered}"
    );
}

fn load_ui_before_binding(config: &Config) -> Ui {
    fs::write(config.profile_path(), r#"{"help_seen":true}"#).unwrap();
    Ui::load(config.profile_path(), None).unwrap()
}

fn render_ui(ui: &Ui, snapshot: &Snapshot) -> String {
    let backend = TestBackend::new(100, 30);
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

#[derive(Debug, Clone, Copy)]
enum InvalidEvidence {
    MissingSource,
    ProviderMismatch,
    NativeSessionMismatch,
    LaunchProcessMismatch,
    WorkingDirectoryMismatch,
    StalePid,
    ReusedPid,
}

impl InvalidEvidence {
    fn name(self) -> &'static str {
        match self {
            Self::MissingSource => "missing-source",
            Self::ProviderMismatch => "provider-mismatch",
            Self::NativeSessionMismatch => "native-id-mismatch",
            Self::LaunchProcessMismatch => "launch-process-mismatch",
            Self::WorkingDirectoryMismatch => "working-directory-mismatch",
            Self::StalePid => "stale-pid",
            Self::ReusedPid => "reused-pid",
        }
    }
}

fn registration(
    id: &str,
    provider: Provider,
    working_directory: &Path,
    provider_process: ProcessIdentity,
) -> ManagedLaunchRegistration {
    let resolution = sessionmesh::project::resolve(Some(working_directory));
    let project = resolution
        .project
        .expect("test working directory should resolve to a Project");
    ManagedLaunchRegistration {
        id: id.into(),
        status: ManagedLaunchStatus::Binding,
        provider,
        target: ManagedLaunchTarget {
            project: project.identity,
            project_label: project.label,
            worktree_root: resolution.worktree.map(|worktree| worktree.root),
            working_directory: working_directory.to_path_buf(),
        },
        session_alias: "Managed work".into(),
        description: Some("Bound only after exact evidence".into()),
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
            provider_process,
        }),
        terminal_opened_at_ms: Some(42_000),
        terminal_error: None,
        binding_error: None,
    }
}

fn binding_evidence(
    provider: Provider,
    source_session_id: &str,
    process_session_id: &str,
    source_path: &Path,
    working_directory: &Path,
    provider_process: ProcessIdentity,
) -> ProviderBindingEvidence {
    binding_evidence_with_launch(
        provider,
        source_session_id,
        process_session_id,
        source_path,
        working_directory,
        provider_process,
        provider_process,
    )
}

fn binding_evidence_with_launch(
    provider: Provider,
    source_session_id: &str,
    process_session_id: &str,
    source_path: &Path,
    working_directory: &Path,
    launch_process: ProcessIdentity,
    provider_process: ProcessIdentity,
) -> ProviderBindingEvidence {
    ProviderBindingEvidence {
        provider,
        source_session_id: source_session_id.into(),
        process_session_id: process_session_id.into(),
        source: SourceMetadata {
            path: source_path.to_path_buf(),
            format: SourceFormat::JsonLines,
        },
        working_directory: working_directory.to_path_buf(),
        launch_process,
        provider_process,
    }
}

fn observed_session(
    provider: Provider,
    id: &str,
    source_path: &Path,
    working_directory: &Path,
) -> Session {
    Session {
        provider,
        id: id.into(),
        provenance: SessionProvenance::Observed,
        source: SourceMetadata {
            path: source_path.to_path_buf(),
            format: SourceFormat::JsonLines,
        },
        project: None,
        worktree: None,
        cwd: Some(working_directory.to_string_lossy().into_owned()),
        latest_model: None,
        state: SessionState::Idle,
        context: ContextState::default(),
        usage: Usage::default(),
        per_model: Vec::new(),
        current_run_usage: None,
        compactions: Vec::new(),
        last_activity_ms: 50_000,
        cost: None,
    }
}

fn process(pid: u32, started_at_ms: i64) -> ProcessIdentity {
    ProcessIdentity { pid, started_at_ms }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sessionmesh-managed-binding-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
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
