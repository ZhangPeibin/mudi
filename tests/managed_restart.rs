use mudi::adapters::config::Config;
use mudi::adapters::persistence::managed_sessions::ManagedLaunchStore;
use mudi::adapters::persistence::session_metadata::SessionLabels;
use mudi::adapters::providers::state::ProcessIdentity;
use mudi::adapters::providers::{ProviderBindingEvidence, ProviderExecutables};
use mudi::adapters::refresh::open_refresh_cycle;
use mudi::application::managed_sessions::{
    BindingOutcome, BindingReceipt, EvidenceBinder, ManagedCurrentRunEvidence,
    ManagedCurrentRunRecord, ManagedLaunchCleanup, ManagedLaunchEvidence,
    ManagedLaunchRegistration, ManagedLaunchStatus, ManagedLaunchTarget, ManagedSessionRecord,
    ManagedSessionService, ManagedTerminalAccess, PrivateTmuxEnvironment, ProcessEvidence,
    ProviderNewCommand, ProviderProcessObservation,
};
use mudi::application::observation::read_model::RefreshHealth;
use mudi::domain::{Provider, SourceFormat, SourceMetadata};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn managed_launch_persistence_records_initializer_without_provider_command_content() {
    let temp = TempDir::new("content-safe-persistence");
    let store_path = temp.path().join("managed.db");
    let private_initializer = "private initializer conversation content";
    let private_executable = temp.path().join("private-provider-executable");
    let mut registration = registration("launch-private-content", temp.path());
    registration.binding_initializer = true;
    registration.provider_command = ProviderNewCommand {
        executable: private_executable.clone(),
        arguments: vec![private_initializer.into()],
    };

    let store = ManagedLaunchStore::open(&store_path).unwrap();
    store.insert(&registration).unwrap();
    drop(store);

    let reopened = ManagedLaunchStore::open(&store_path).unwrap();
    let persisted = reopened.get(&registration.id).unwrap().unwrap();
    assert!(persisted.binding_initializer);
    assert!(persisted.provider_command.executable.as_os_str().is_empty());
    assert!(persisted.provider_command.arguments.is_empty());
    drop(reopened);

    let connection = rusqlite::Connection::open(&store_path).unwrap();
    let mut statement = connection
        .prepare("PRAGMA table_info(managed_launch_registrations)")
        .unwrap();
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    drop(statement);
    drop(connection);
    assert!(columns.iter().any(|column| column == "binding_initializer"));
    assert!(!columns.iter().any(|column| column == "provider_executable"));
    assert!(!columns
        .iter()
        .any(|column| column == "provider_arguments_json"));
    assert_eq!(
        columns,
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

    let persisted_bytes = fs::read(&store_path).unwrap();
    let persisted_text = String::from_utf8_lossy(&persisted_bytes);
    assert!(!persisted_text.contains(private_initializer));
    assert!(!persisted_text.contains(private_executable.to_string_lossy().as_ref()));
}

#[test]
fn restart_uncertainty_needs_review_without_cleaning_or_assuming_authority() {
    let temp = TempDir::new("uncertain-reconciliation");
    let launched_process = ProcessIdentity {
        pid: 42,
        started_at_ms: 41_000,
    };
    let source = temp.path().join("source.jsonl");
    fs::write(&source, "{}\n").unwrap();
    let cases = [
        ("absent", ProviderProcessObservation::Absent, None),
        ("unknown", ProviderProcessObservation::Unknown, None),
        (
            "pid-reused",
            ProviderProcessObservation::Live(ProcessIdentity {
                pid: 42,
                started_at_ms: 99_000,
            }),
            None,
        ),
        (
            "contradictory",
            ProviderProcessObservation::Live(launched_process),
            Some(ProviderBindingEvidence {
                provider: Provider::ClaudeCode,
                source_session_id: "unrelated-session".into(),
                process_session_id: "unrelated-session".into(),
                source: SourceMetadata {
                    path: source,
                    format: SourceFormat::JsonLines,
                },
                working_directory: temp.path().to_path_buf(),
                launch_process: launched_process,
                provider_process: launched_process,
            }),
        ),
    ];

    for (name, observation, provider_evidence) in cases {
        let store_path = temp.path().join(format!("{name}.db"));
        let mut store = ManagedLaunchStore::open(store_path).unwrap();
        let registration = registration(&format!("launch-{name}"), temp.path());
        store.insert(&registration).unwrap();
        let cleaned = Arc::new(Mutex::new(Vec::new()));
        let binder = EvidenceBinder::new(
            Box::new(FixedProcessEvidence(observation)),
            Box::new(RecordingCleanup {
                cleaned: cleaned.clone(),
            }),
        );

        let outcome = binder
            .reconcile_after_restart(&mut store, &registration.id, provider_evidence, 50_000)
            .unwrap();

        assert_eq!(outcome, BindingOutcome::NeedsReview, "{name}");
        assert!(cleaned.lock().unwrap().is_empty(), "{name}");
        assert_eq!(
            store.get(&registration.id).unwrap().unwrap().status,
            ManagedLaunchStatus::NeedsReview,
            "{name}"
        );
        assert!(store.managed_sessions().unwrap().is_empty(), "{name}");
        assert!(store.managed_current_runs().unwrap().is_empty(), "{name}");
    }
}

#[test]
fn startup_refresh_preserves_a_preexisting_uncertain_launch_for_review() {
    let temp = TempDir::new("startup-refresh");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let mut registration = registration("launch-before-restart", temp.path());
    registration.launch_evidence = Some(ManagedLaunchEvidence {
        tmux_pane_id: "%42".into(),
        provider_process: ProcessIdentity {
            pid: u32::MAX,
            started_at_ms: 41_000,
        },
    });
    let store = ManagedLaunchStore::open(&store_path).unwrap();
    store.insert(&registration).unwrap();
    drop(store);

    let cleaned = Arc::new(Mutex::new(Vec::new()));
    let reconciler = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(ProviderProcessObservation::Absent)),
        Box::new(RecordingCleanup {
            cleaned: cleaned.clone(),
        }),
    );
    let snapshot = open_refresh_cycle(config)
        .with_managed_binding_reconciler(reconciler)
        .run_once();

    let retained = ManagedLaunchStore::open(store_path)
        .unwrap()
        .get(&registration.id)
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, ManagedLaunchStatus::NeedsReview);
    assert_eq!(
        retained.binding_error.as_deref(),
        Some("Provider process is absent after restart; launch scene was preserved")
    );
    assert_eq!(snapshot.managed_launches.len(), 1);
    assert_eq!(
        snapshot.managed_launches[0].status,
        ManagedLaunchStatus::NeedsReview
    );
    assert!(cleaned.lock().unwrap().is_empty());
}

#[test]
fn verified_managed_relationship_and_local_metadata_survive_restart() {
    let temp = TempDir::new("verified-recovery");
    let config = temp.config();
    let working_directory = temp.directory("working_directory");
    let source_dir = config.claude_dir.join("projects/mudi");
    fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("native-session.jsonl");
    let provider_content =
        "provider conversation and tool output must stay outside Managed storage";
    fs::write(
        &source,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "sessionId": "native-session",
                "cwd": working_directory,
                "message": {"role": "user", "content": provider_content}
            })
        ),
    )
    .unwrap();
    let process = ProcessIdentity {
        pid: 42,
        started_at_ms: 41_000,
    };
    let mut registration = registration("launch-verified", &working_directory);
    registration.provider = Provider::ClaudeCode;
    registration.session_alias = "Investigate restart".into();
    registration.description = Some("Keep the exact recovery context".into());
    let store_path = config.managed_launch_store_path();
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    store.insert(&registration).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(ProviderProcessObservation::Live(
            process,
        ))),
        Box::new(RecordingCleanup {
            cleaned: Arc::new(Mutex::new(Vec::new())),
        }),
    );
    assert!(matches!(
        binder
            .bind(
                &mut store,
                &registration.id,
                Some(ProviderBindingEvidence {
                    provider: Provider::ClaudeCode,
                    source_session_id: "native-session".into(),
                    process_session_id: "native-session".into(),
                    source: SourceMetadata {
                        path: source.canonicalize().unwrap(),
                        format: SourceFormat::JsonLines,
                    },
                    working_directory: working_directory.clone(),
                    launch_process: process,
                    provider_process: process,
                }),
                50_000,
            )
            .unwrap(),
        BindingOutcome::Bound(_)
    ));
    let opaque_ref = store.managed_current_runs().unwrap()[0].opaque_ref.clone();
    drop(store);

    let mut first_refresh = open_refresh_cycle(config.clone())
        .with_managed_current_run_evidence(Box::new(AcceptCurrentRunEvidence));
    assert_eq!(
        first_refresh.run_once().verified_managed_current_runs[0].opaque_ref,
        opaque_ref
    );
    drop(first_refresh);

    let mut restarted_refresh = open_refresh_cycle(config.clone())
        .with_managed_current_run_evidence(Box::new(AcceptCurrentRunEvidence));
    let restarted = restarted_refresh.run_once();
    assert_eq!(restarted.verified_managed_current_runs.len(), 1);
    assert_eq!(
        restarted.verified_managed_current_runs[0].opaque_ref,
        opaque_ref
    );
    let reopened = ManagedLaunchStore::open(store_path).unwrap();
    let session = reopened
        .managed_session(Provider::ClaudeCode, "native-session")
        .unwrap()
        .unwrap();
    assert_eq!(session.session_alias, "Investigate restart");
    assert_eq!(
        session.description.as_deref(),
        Some("Keep the exact recovery context")
    );
    let labels = SessionLabels::load(config.label_store_path());
    assert_eq!(
        labels.alias(Provider::ClaudeCode, "native-session"),
        Some("Investigate restart")
    );
    assert_eq!(
        labels.description(Provider::ClaudeCode, "native-session"),
        Some("Keep the exact recovery context")
    );
    drop(reopened);
    drop(restarted_refresh);
    assert_files_with_prefix_omit(&config.data_dir, "managed-launches.db", provider_content);
}

#[test]
fn normal_application_lifetime_end_does_not_cancel_or_restart_the_managed_launch() {
    let temp = TempDir::new("normal-detach");
    let store = ManagedLaunchStore::open(temp.path().join("managed.db")).unwrap();
    store
        .insert(&registration("launch-kept-alive", temp.path()))
        .unwrap();
    let launched = Arc::new(Mutex::new(Vec::new()));
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let terminals = Arc::new(Mutex::new(Vec::new()));
    let broker = RecordingLaunchBroker {
        launched: launched.clone(),
        cancelled: cancelled.clone(),
    };
    let service = ManagedSessionService::new(
        store,
        ProviderExecutables::new(PathBuf::new(), PathBuf::new()),
        Box::new(broker),
        Box::new(RecordingTerminalAccess {
            opened: terminals.clone(),
        }),
    );

    drop(service);

    assert!(launched.lock().unwrap().is_empty());
    assert!(terminals.lock().unwrap().is_empty());
    assert!(cancelled.lock().unwrap().is_empty());
}

#[test]
fn corrupted_persisted_identity_stays_unassociated_and_degrades_startup() {
    let temp = TempDir::new("corrupt-metadata");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let registration = registration("launch-corrupt", temp.path());
    let store = ManagedLaunchStore::open(&store_path).unwrap();
    store.insert(&registration).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&store_path).unwrap();
    connection
        .execute(
            "UPDATE managed_launch_registrations SET provider = 'not-a-provider' WHERE registration_id = ?1",
            [&registration.id],
        )
        .unwrap();
    drop(connection);

    let cleaned = Arc::new(Mutex::new(Vec::new()));
    let reconciler = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(ProviderProcessObservation::Absent)),
        Box::new(RecordingCleanup {
            cleaned: cleaned.clone(),
        }),
    );
    let snapshot = open_refresh_cycle(config)
        .with_managed_binding_reconciler(reconciler)
        .run_once();

    assert!(matches!(snapshot.health, RefreshHealth::Degraded { .. }));
    assert!(snapshot.managed_launches.is_empty());
    assert!(snapshot.verified_managed_current_runs.is_empty());
    assert!(cleaned.lock().unwrap().is_empty());
    let connection = rusqlite::Connection::open(store_path).unwrap();
    let retained = connection
        .query_row(
            "SELECT status, provider FROM managed_launch_registrations WHERE registration_id = ?1",
            [&registration.id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(retained, ("binding".into(), "not-a-provider".into()));
}

#[test]
fn stale_managed_current_run_is_not_reassociated_or_rewritten_on_restart() {
    let temp = TempDir::new("stale-current-run");
    let config = temp.config();
    let working_directory = temp.directory("working_directory");
    let source = temp.path().join("source.jsonl");
    fs::write(&source, "{}\n").unwrap();
    let registration = registration("launch-stale-run", &working_directory);
    let store_path = config.managed_launch_store_path();
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    let opaque_ref = bind_registration(
        &mut store,
        &registration,
        &source,
        &working_directory,
        "native-stale",
    );
    let before = store
        .managed_current_run(Provider::CodexCli, "native-stale")
        .unwrap()
        .unwrap();
    drop(store);

    let snapshot = open_refresh_cycle(config)
        .with_managed_current_run_evidence(Box::new(RejectCurrentRunEvidence))
        .run_once();

    assert!(snapshot.verified_managed_current_runs.is_empty());
    let reopened = ManagedLaunchStore::open(store_path).unwrap();
    let after = reopened
        .managed_current_run(Provider::CodexCli, "native-stale")
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(after.opaque_ref, opaque_ref);
    assert!(reopened
        .managed_session(Provider::CodexCli, "native-stale")
        .unwrap()
        .is_some());
}

struct RecordingLaunchBroker {
    launched: Arc<Mutex<Vec<String>>>,
    cancelled: Arc<Mutex<Vec<String>>>,
}

impl mudi::application::managed_sessions::ManagedLaunchBroker for RecordingLaunchBroker {
    fn launch(
        &mut self,
        registration: &ManagedLaunchRegistration,
    ) -> anyhow::Result<ManagedLaunchEvidence> {
        self.launched.lock().unwrap().push(registration.id.clone());
        Ok(ManagedLaunchEvidence {
            tmux_pane_id: "%42".into(),
            provider_process: ProcessIdentity {
                pid: 42,
                started_at_ms: 41_000,
            },
        })
    }

    fn cancel(&mut self, registration: &ManagedLaunchRegistration) -> anyhow::Result<()> {
        self.cancelled.lock().unwrap().push(registration.id.clone());
        Ok(())
    }
}

struct RecordingTerminalAccess {
    opened: Arc<Mutex<Vec<String>>>,
}

impl ManagedTerminalAccess for RecordingTerminalAccess {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> anyhow::Result<()> {
        self.opened.lock().unwrap().push(registration.id.clone());
        Ok(())
    }
}

struct AcceptCurrentRunEvidence;

impl ManagedCurrentRunEvidence for AcceptCurrentRunEvidence {
    fn revalidate(
        &self,
        _run: &ManagedCurrentRunRecord,
        _session: &ManagedSessionRecord,
        _receipt: &BindingReceipt,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

struct RejectCurrentRunEvidence;

impl ManagedCurrentRunEvidence for RejectCurrentRunEvidence {
    fn revalidate(
        &self,
        _run: &ManagedCurrentRunRecord,
        _session: &ManagedSessionRecord,
        _receipt: &BindingReceipt,
    ) -> anyhow::Result<()> {
        anyhow::bail!("persisted Current Run evidence is stale")
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

fn bind_registration(
    store: &mut ManagedLaunchStore,
    registration: &ManagedLaunchRegistration,
    source: &Path,
    working_directory: &Path,
    native_session_id: &str,
) -> String {
    store.insert(registration).unwrap();
    let process = registration
        .launch_evidence
        .as_ref()
        .unwrap()
        .provider_process;
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(ProviderProcessObservation::Live(
            process,
        ))),
        Box::new(RecordingCleanup {
            cleaned: Arc::new(Mutex::new(Vec::new())),
        }),
    );
    assert!(matches!(
        binder
            .bind(
                store,
                &registration.id,
                Some(ProviderBindingEvidence {
                    provider: registration.provider,
                    source_session_id: native_session_id.into(),
                    process_session_id: native_session_id.into(),
                    source: SourceMetadata {
                        path: source.canonicalize().unwrap(),
                        format: SourceFormat::JsonLines,
                    },
                    working_directory: working_directory.to_path_buf(),
                    launch_process: process,
                    provider_process: process,
                }),
                50_000,
            )
            .unwrap(),
        BindingOutcome::Bound(_)
    ));
    store.managed_current_runs().unwrap()[0].opaque_ref.clone()
}

impl ManagedLaunchCleanup for RecordingCleanup {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> anyhow::Result<()> {
        self.cleaned.lock().unwrap().push(registration.id.clone());
        Ok(())
    }
}

fn assert_files_with_prefix_omit(directory: &Path, prefix: &str, private_content: &str) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix))
        {
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(private_content),
            "{}",
            path.display()
        );
    }
}

fn registration(id: &str, working_directory: &Path) -> ManagedLaunchRegistration {
    ManagedLaunchRegistration {
        id: id.into(),
        status: ManagedLaunchStatus::Binding,
        provider: Provider::CodexCli,
        target: ManagedLaunchTarget {
            working_directory: working_directory.to_path_buf(),
        },
        session_alias: "Restart work".into(),
        description: Some("Durable local description".into()),
        binding_initializer: false,
        provider_command: ProviderNewCommand {
            executable: PathBuf::new(),
            arguments: Vec::new(),
        },
        tmux: PrivateTmuxEnvironment {
            socket_name: format!("mudi-{id}"),
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

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mudi-managed-restart-{name}-{}-{nonce}",
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
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
