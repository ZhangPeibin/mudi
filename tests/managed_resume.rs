mod support;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mudi::adapters::config::Config;
use mudi::adapters::handoff::open_handoff_service;
use mudi::adapters::managed_sessions::{
    SystemManagedResumeProviderEvidence, SystemProcessEvidence,
};
use mudi::adapters::persistence::managed_sessions::ManagedLaunchStore;
use mudi::adapters::providers::state::ProcessIdentity;
use mudi::adapters::providers::{registry, ProviderBindingEvidence, ProviderExecutables};
use mudi::adapters::refresh::open_refresh_cycle;
use mudi::application::clipboard::ClipboardPort;
use mudi::application::handoff::{HandoffPaths, WorkerLaunch, WorkerLauncher};
use mudi::application::managed_sessions::{
    BindingOutcome, EvidenceBinder, ManagedCurrentRunLifecycle, ManagedLaunchBroker,
    ManagedLaunchCleanup, ManagedLaunchEvidence, ManagedLaunchRegistration, ManagedLaunchTarget,
    ManagedResumeEvidence, ManagedResumeEvidenceCandidate, ManagedResumeProviderEvidence,
    ManagedResumeRequest, ManagedResumeVerifier, ManagedSessionRecord, ManagedSessionService,
    ManagedTerminalAccess, ProcessEvidence, ProviderProcessObservation,
    SystemManagedResumeEvidence,
};
use mudi::application::observation::read_model::{ManagedResumeSummary, RefreshSnapshot};
use mudi::application::successor::TerminalLauncher;
use mudi::domain::{
    ContextState, Project, ProjectIdentity, Provider, Session, SessionProvenance, SessionState,
    SourceFormat, SourceMetadata, Usage,
};
use mudi::presentation::tui::{draw, Ui, UiAction};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use support::{workflow_handlers, TestActionRouter};

#[test]
fn provider_adapters_build_exact_interactive_resume_commands() {
    let native_session_id = "native-session-49";

    assert_eq!(
        registry()
            .runtime_control(Provider::CodexCli)
            .unwrap()
            .resume_session_args(native_session_id),
        vec![
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "resume".to_string(),
            native_session_id.to_string(),
        ]
    );
    assert_eq!(
        registry()
            .runtime_control(Provider::ClaudeCode)
            .unwrap()
            .resume_session_args(native_session_id),
        vec!["--resume".to_string(), native_session_id.to_string()]
    );
}

#[test]
fn verified_exited_session_launches_an_exact_resume_from_a_durable_registration() {
    let temp = TempDir::new("exact-resume");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let prior_process = ProcessIdentity {
        pid: 42,
        started_at_ms: 41_000,
    };
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    let prior_run_id = bind_initial_managed_session(
        &mut store,
        &temp,
        Provider::CodexCli,
        "native-session-49",
        prior_process,
    );
    let launched = Arc::new(Mutex::new(Vec::new()));
    let opened = Arc::new(Mutex::new(Vec::new()));
    let verified = Arc::new(Mutex::new(Vec::new()));
    let codex = temp.executable("codex");
    let mut service = ManagedSessionService::new(
        store,
        ProviderExecutables::new(temp.executable("claude"), codex.clone()),
        Box::new(RecordingLaunchBroker {
            store_path: store_path.clone(),
            launched: launched.clone(),
        }),
        Box::new(RecordingTerminal {
            opened: opened.clone(),
        }),
    )
    .with_resume_verifier(ManagedResumeVerifier::new(Box::new(FixedResumeEvidence {
        lifecycle: ManagedCurrentRunLifecycle::Exited,
        verified: verified.clone(),
    })));

    let outcome = service
        .start_resume(
            ManagedResumeRequest {
                provider: Provider::CodexCli,
                native_session_id: "native-session-49".into(),
            },
            60_000,
        )
        .unwrap();

    assert!(outcome.terminal_opened);
    assert_eq!(
        *verified.lock().unwrap(),
        vec![(
            Provider::CodexCli,
            "native-session-49".to_string(),
            prior_run_id.clone(),
        )]
    );
    assert_eq!(
        *launched.lock().unwrap(),
        vec![(
            Provider::CodexCli,
            "native-session-49".to_string(),
            codex,
            vec![
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "resume".to_string(),
                "native-session-49".to_string(),
            ],
        )]
    );
    assert_eq!(
        *opened.lock().unwrap(),
        vec![outcome.registration.id.clone()]
    );
    let reopened = ManagedLaunchStore::open(&store_path).unwrap();
    let target = reopened
        .managed_resume_target(&outcome.registration.id)
        .unwrap()
        .unwrap();
    assert_eq!(target.provider, Provider::CodexCli);
    assert_eq!(target.native_session_id, "native-session-49");
    assert_eq!(target.prior_run_id, prior_run_id);
    let persisted = reopened.get(&outcome.registration.id).unwrap().unwrap();
    assert!(persisted.provider_command.executable.as_os_str().is_empty());
    assert!(persisted.provider_command.arguments.is_empty());
    assert_eq!(reopened.managed_current_runs().unwrap().len(), 1);
}

#[test]
fn resume_binding_rejects_self_consistent_evidence_for_a_different_native_session() {
    let temp = TempDir::new("wrong-resume-identity");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let prior_process = ProcessIdentity {
        pid: 42,
        started_at_ms: 41_000,
    };
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    bind_initial_managed_session(
        &mut store,
        &temp,
        Provider::ClaudeCode,
        "intended-native-session",
        prior_process,
    );
    let mut service = ManagedSessionService::new(
        store,
        ProviderExecutables::new(temp.executable("claude"), temp.executable("codex")),
        Box::new(RecordingLaunchBroker {
            store_path: store_path.clone(),
            launched: Arc::new(Mutex::new(Vec::new())),
        }),
        Box::new(RecordingTerminal {
            opened: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .with_resume_verifier(ManagedResumeVerifier::new(Box::new(FixedResumeEvidence {
        lifecycle: ManagedCurrentRunLifecycle::Exited,
        verified: Arc::new(Mutex::new(Vec::new())),
    })));
    let outcome = service
        .start_resume(
            ManagedResumeRequest {
                provider: Provider::ClaudeCode,
                native_session_id: "intended-native-session".into(),
            },
            60_000,
        )
        .unwrap();
    drop(service);
    let resumed_process = outcome
        .registration
        .launch_evidence
        .as_ref()
        .unwrap()
        .provider_process;
    let source = temp.file("wrong-source.jsonl", "{}\n");
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(resumed_process)),
        Box::new(NoopCleanup),
    );

    let binding = binder
        .bind(
            &mut store,
            &outcome.registration.id,
            Some(ProviderBindingEvidence {
                provider: Provider::ClaudeCode,
                source_session_id: "different-native-session".into(),
                process_session_id: "different-native-session".into(),
                source: SourceMetadata {
                    path: source,
                    format: SourceFormat::JsonLines,
                },
                working_directory: outcome.registration.target.working_directory.clone(),
                launch_process: resumed_process,
                provider_process: resumed_process,
            }),
            61_000,
        )
        .unwrap();

    assert_eq!(binding, BindingOutcome::NeedsReview);
    assert!(store
        .managed_session(Provider::ClaudeCode, "different-native-session")
        .unwrap()
        .is_none());
    assert!(store
        .binding_receipt(&outcome.registration.id)
        .unwrap()
        .is_none());
}

#[test]
fn successful_resume_atomically_establishes_a_distinct_run_and_new_receipt() {
    let temp = TempDir::new("resume-receipt");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let prior_process = ProcessIdentity {
        pid: 42,
        started_at_ms: 41_000,
    };
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    let prior_run_id = bind_initial_managed_session(
        &mut store,
        &temp,
        Provider::CodexCli,
        "native-session-49",
        prior_process,
    );
    let prior_receipt_id = store
        .managed_session(Provider::CodexCli, "native-session-49")
        .unwrap()
        .unwrap()
        .binding_receipt_id;
    let mut service = ManagedSessionService::new(
        store,
        ProviderExecutables::new(temp.executable("claude"), temp.executable("codex")),
        Box::new(RecordingLaunchBroker {
            store_path: store_path.clone(),
            launched: Arc::new(Mutex::new(Vec::new())),
        }),
        Box::new(RecordingTerminal {
            opened: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .with_resume_verifier(ManagedResumeVerifier::new(Box::new(FixedResumeEvidence {
        lifecycle: ManagedCurrentRunLifecycle::Exited,
        verified: Arc::new(Mutex::new(Vec::new())),
    })));
    let outcome = service
        .start_resume(
            ManagedResumeRequest {
                provider: Provider::CodexCli,
                native_session_id: "native-session-49".into(),
            },
            60_000,
        )
        .unwrap();
    drop(service);
    let resumed_process = outcome
        .registration
        .launch_evidence
        .as_ref()
        .unwrap()
        .provider_process;
    let resumed_source = temp.file("resumed-source.jsonl", "{}\n");
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(resumed_process)),
        Box::new(NoopCleanup),
    );

    let binding = binder
        .bind(
            &mut store,
            &outcome.registration.id,
            Some(ProviderBindingEvidence {
                provider: Provider::CodexCli,
                source_session_id: "native-session-49".into(),
                process_session_id: "native-session-49".into(),
                source: SourceMetadata {
                    path: resumed_source,
                    format: SourceFormat::JsonLines,
                },
                working_directory: outcome.registration.target.working_directory.clone(),
                launch_process: resumed_process,
                provider_process: resumed_process,
            }),
            61_000,
        )
        .unwrap();

    let BindingOutcome::Bound(receipt) = binding else {
        panic!("exact Resume evidence must establish new authority");
    };
    assert_ne!(receipt.id, prior_receipt_id);
    assert!(store.get(&outcome.registration.id).unwrap().is_none());
    assert!(store
        .managed_resume_target(&outcome.registration.id)
        .unwrap()
        .is_none());
    let session = store
        .managed_session(Provider::CodexCli, "native-session-49")
        .unwrap()
        .unwrap();
    assert_eq!(session.binding_receipt_id, receipt.id);
    assert_eq!(session.session_alias, "Resume target");
    assert_eq!(session.description.as_deref(), Some("Keep local metadata"));
    let runs = store.managed_current_runs().unwrap();
    assert_eq!(runs.len(), 2);
    let prior = runs.iter().find(|run| run.id == prior_run_id).unwrap();
    let resumed = runs
        .iter()
        .find(|run| run.binding_receipt_id == receipt.id)
        .unwrap();
    assert_ne!(resumed.id, prior.id);
    assert_ne!(resumed.opaque_ref, prior.opaque_ref);
    assert_eq!(prior.provider_process, prior_process);
    assert_eq!(resumed.provider_process, resumed_process);
    assert_eq!(
        store
            .managed_current_run(Provider::CodexCli, "native-session-49")
            .unwrap()
            .unwrap()
            .id,
        resumed.id
    );
}

#[test]
fn claude_resume_lifecycle_uses_exact_identity_and_establishes_a_distinct_run() {
    let temp = TempDir::new("claude-resume-lifecycle");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let prior_process = ProcessIdentity {
        pid: 42,
        started_at_ms: 41_000,
    };
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    let prior_run_id = bind_initial_managed_session(
        &mut store,
        &temp,
        Provider::ClaudeCode,
        "claude-native-session-49",
        prior_process,
    );
    let launched = Arc::new(Mutex::new(Vec::new()));
    let claude = temp.executable("claude");
    let mut service = ManagedSessionService::new(
        store,
        ProviderExecutables::new(claude.clone(), temp.executable("codex")),
        Box::new(RecordingLaunchBroker {
            store_path: store_path.clone(),
            launched: launched.clone(),
        }),
        Box::new(RecordingTerminal {
            opened: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .with_resume_verifier(ManagedResumeVerifier::new(Box::new(FixedResumeEvidence {
        lifecycle: ManagedCurrentRunLifecycle::Exited,
        verified: Arc::new(Mutex::new(Vec::new())),
    })));
    let outcome = service
        .start_resume(
            ManagedResumeRequest {
                provider: Provider::ClaudeCode,
                native_session_id: "claude-native-session-49".into(),
            },
            60_000,
        )
        .unwrap();
    assert_eq!(
        *launched.lock().unwrap(),
        vec![(
            Provider::ClaudeCode,
            "claude-native-session-49".to_string(),
            claude,
            vec![
                "--resume".to_string(),
                "claude-native-session-49".to_string(),
            ],
        )]
    );
    drop(service);
    let resumed_process = outcome
        .registration
        .launch_evidence
        .as_ref()
        .unwrap()
        .provider_process;
    let resumed_source = temp.file("claude-resumed-source.jsonl", "{}\n");
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedProcessEvidence(resumed_process)),
        Box::new(NoopCleanup),
    );
    let binding = binder
        .bind(
            &mut store,
            &outcome.registration.id,
            Some(ProviderBindingEvidence {
                provider: Provider::ClaudeCode,
                source_session_id: "claude-native-session-49".into(),
                process_session_id: "claude-native-session-49".into(),
                source: SourceMetadata {
                    path: resumed_source,
                    format: SourceFormat::JsonLines,
                },
                working_directory: outcome.registration.target.working_directory.clone(),
                launch_process: resumed_process,
                provider_process: resumed_process,
            }),
            61_000,
        )
        .unwrap();
    let BindingOutcome::Bound(receipt) = binding else {
        panic!("exact Claude Resume evidence must establish new authority");
    };
    let runs = store.managed_current_runs().unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().any(|run| run.id == prior_run_id));
    let current = store
        .managed_current_run(Provider::ClaudeCode, "claude-native-session-49")
        .unwrap()
        .unwrap();
    assert_eq!(current.binding_receipt_id, receipt.id);
    assert_ne!(current.id, prior_run_id);
    assert_eq!(current.provider_process, resumed_process);
}

#[test]
fn pending_resume_is_unique_and_cancel_clears_it_for_a_later_authorization() {
    let temp = TempDir::new("resume-cancel");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    bind_initial_managed_session(
        &mut store,
        &temp,
        Provider::ClaudeCode,
        "native-session-49",
        ProcessIdentity {
            pid: 42,
            started_at_ms: 41_000,
        },
    );
    let mut service = ManagedSessionService::new(
        store,
        ProviderExecutables::new(temp.executable("claude"), temp.executable("codex")),
        Box::new(RecordingLaunchBroker {
            store_path: store_path.clone(),
            launched: Arc::new(Mutex::new(Vec::new())),
        }),
        Box::new(RecordingTerminal {
            opened: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .with_resume_verifier(ManagedResumeVerifier::new(Box::new(FixedResumeEvidence {
        lifecycle: ManagedCurrentRunLifecycle::Exited,
        verified: Arc::new(Mutex::new(Vec::new())),
    })));
    let first = service
        .start_resume(
            ManagedResumeRequest {
                provider: Provider::ClaudeCode,
                native_session_id: "native-session-49".into(),
            },
            60_000,
        )
        .unwrap();

    let first_target = ManagedLaunchStore::open(&store_path)
        .unwrap()
        .managed_resume_target(&first.registration.id)
        .unwrap()
        .unwrap();
    let mut direct_store = ManagedLaunchStore::open(&store_path).unwrap();
    let mut direct_duplicate = first.registration.clone();
    direct_duplicate.id = "launch-direct-duplicate".into();
    direct_duplicate.tmux.socket_name = "mudi-direct-duplicate".into();
    assert!(direct_store
        .insert_resume(&direct_duplicate, &first_target)
        .unwrap_err()
        .to_string()
        .contains("UNIQUE constraint failed"));
    assert!(direct_store.get(&direct_duplicate.id).unwrap().is_none());
    drop(direct_store);

    let duplicate = service.start_resume(
        ManagedResumeRequest {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session-49".into(),
        },
        61_000,
    );
    assert!(duplicate
        .unwrap_err()
        .to_string()
        .contains("already in progress"));

    service.cancel_start(&first.registration.id).unwrap();
    let reopened = ManagedLaunchStore::open(&store_path).unwrap();
    assert!(reopened.get(&first.registration.id).unwrap().is_none());
    assert!(reopened
        .managed_resume_target(&first.registration.id)
        .unwrap()
        .is_none());
    drop(reopened);

    let second = service
        .start_resume(
            ManagedResumeRequest {
                provider: Provider::ClaudeCode,
                native_session_id: "native-session-49".into(),
            },
            62_000,
        )
        .unwrap();
    assert_ne!(second.registration.id, first.registration.id);
}

#[test]
fn open_or_unknown_prior_run_never_reaches_the_resume_launch_broker() {
    for provider in [Provider::ClaudeCode, Provider::CodexCli] {
        for lifecycle in [
            ManagedCurrentRunLifecycle::Open,
            ManagedCurrentRunLifecycle::Unknown,
        ] {
            let temp = TempDir::new("blocked-resume");
            let config = temp.config();
            let store_path = config.managed_launch_store_path();
            let mut store = ManagedLaunchStore::open(&store_path).unwrap();
            bind_initial_managed_session(
                &mut store,
                &temp,
                provider,
                "native-session-49",
                ProcessIdentity {
                    pid: 42,
                    started_at_ms: 41_000,
                },
            );
            let launched = Arc::new(Mutex::new(Vec::new()));
            let opened = Arc::new(Mutex::new(Vec::new()));
            let mut service = ManagedSessionService::new(
                store,
                ProviderExecutables::new(temp.executable("claude"), temp.executable("codex")),
                Box::new(RecordingLaunchBroker {
                    store_path: store_path.clone(),
                    launched: launched.clone(),
                }),
                Box::new(RecordingTerminal {
                    opened: opened.clone(),
                }),
            )
            .with_resume_verifier(ManagedResumeVerifier::new(Box::new(FixedResumeEvidence {
                lifecycle,
                verified: Arc::new(Mutex::new(Vec::new())),
            })));

            assert!(service
                .start_resume(
                    ManagedResumeRequest {
                        provider,
                        native_session_id: "native-session-49".into(),
                    },
                    60_000,
                )
                .is_err());
            assert!(launched.lock().unwrap().is_empty());
            assert!(opened.lock().unwrap().is_empty());
            let reopened = ManagedLaunchStore::open(&store_path).unwrap();
            assert!(reopened.registrations().unwrap().is_empty());
            assert_eq!(reopened.managed_current_runs().unwrap().len(), 1);
        }
    }
}

#[test]
fn resume_launch_and_terminal_or_evidence_failures_never_change_authority() {
    let temp = TempDir::new("resume-failures");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let prior_process = ProcessIdentity {
        pid: 42,
        started_at_ms: 41_000,
    };
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    bind_initial_managed_session(
        &mut store,
        &temp,
        Provider::CodexCli,
        "native-session-49",
        prior_process,
    );
    let prior_receipt = store
        .managed_session(Provider::CodexCli, "native-session-49")
        .unwrap()
        .unwrap()
        .binding_receipt_id;
    let mut launch_failure = ManagedSessionService::new(
        store,
        ProviderExecutables::new(temp.executable("claude"), temp.executable("codex")),
        Box::new(FailingLaunchBroker),
        Box::new(NeverTerminal),
    )
    .with_resume_verifier(ManagedResumeVerifier::new(Box::new(FixedResumeEvidence {
        lifecycle: ManagedCurrentRunLifecycle::Exited,
        verified: Arc::new(Mutex::new(Vec::new())),
    })));
    assert!(launch_failure
        .start_resume(
            ManagedResumeRequest {
                provider: Provider::CodexCli,
                native_session_id: "native-session-49".into(),
            },
            60_000,
        )
        .unwrap_err()
        .to_string()
        .contains("Resume launch failed"));
    drop(launch_failure);
    let failed_store = ManagedLaunchStore::open(&store_path).unwrap();
    let failed_registration = failed_store.registrations().unwrap().pop().unwrap();
    assert!(failed_registration.launch_evidence.is_none());
    assert!(failed_store
        .managed_resume_target(&failed_registration.id)
        .unwrap()
        .is_some());
    assert_eq!(failed_store.managed_current_runs().unwrap().len(), 1);
    assert_eq!(
        failed_store
            .managed_session(Provider::CodexCli, "native-session-49")
            .unwrap()
            .unwrap()
            .binding_receipt_id,
        prior_receipt
    );
    drop(failed_store);

    let mut cleanup = ManagedSessionService::new(
        ManagedLaunchStore::open(&store_path).unwrap(),
        ProviderExecutables::new(PathBuf::new(), PathBuf::new()),
        Box::new(RecordingLaunchBroker {
            store_path: store_path.clone(),
            launched: Arc::new(Mutex::new(Vec::new())),
        }),
        Box::new(NeverTerminal),
    );
    cleanup.cancel_start(&failed_registration.id).unwrap();
    drop(cleanup);

    let store = ManagedLaunchStore::open(&store_path).unwrap();
    let mut terminal_failure = ManagedSessionService::new(
        store,
        ProviderExecutables::new(temp.executable("claude-2"), temp.executable("codex-2")),
        Box::new(RecordingLaunchBroker {
            store_path: store_path.clone(),
            launched: Arc::new(Mutex::new(Vec::new())),
        }),
        Box::new(FailingTerminal),
    )
    .with_resume_verifier(ManagedResumeVerifier::new(Box::new(FixedResumeEvidence {
        lifecycle: ManagedCurrentRunLifecycle::Exited,
        verified: Arc::new(Mutex::new(Vec::new())),
    })));
    let outcome = terminal_failure
        .start_resume(
            ManagedResumeRequest {
                provider: Provider::CodexCli,
                native_session_id: "native-session-49".into(),
            },
            61_000,
        )
        .unwrap();
    assert!(!outcome.terminal_opened);
    assert_eq!(
        outcome.registration.terminal_error.as_deref(),
        Some("Terminal.app launch failed")
    );
    drop(terminal_failure);
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    let binder = EvidenceBinder::new(
        Box::new(FixedObservation(ProviderProcessObservation::Unknown)),
        Box::new(NoopCleanup),
    );
    assert_eq!(
        binder
            .bind(&mut store, &outcome.registration.id, None, 62_000)
            .unwrap(),
        BindingOutcome::NeedsReview
    );
    assert_eq!(store.managed_current_runs().unwrap().len(), 1);
    assert_eq!(
        store
            .managed_session(Provider::CodexCli, "native-session-49")
            .unwrap()
            .unwrap()
            .binding_receipt_id,
        prior_receipt
    );
}

#[test]
fn system_resume_evidence_requires_process_absence_and_no_external_open_run() {
    let temp = TempDir::new("system-exit-evidence");
    let config = temp.config();
    let working_directory = temp.directory("working_directory");
    let transcript_dir = config.claude_dir.join("projects/project");
    fs::create_dir_all(&transcript_dir).unwrap();
    let source = transcript_dir.join("native-session-49.jsonl");
    fs::write(
        &source,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "sessionId": "native-session-49",
                "cwd": working_directory,
                "timestamp": "2026-08-11T00:00:00Z",
                "message": {
                    "id": "message-1",
                    "role": "assistant",
                    "model": "claude-sonnet",
                    "usage": {"input_tokens": 1, "output_tokens": 1}
                }
            })
        ),
    )
    .unwrap();
    let prior_process = ProcessIdentity {
        pid: u32::MAX,
        started_at_ms: 41_000,
    };
    let receipt = mudi::application::managed_sessions::BindingReceipt {
        id: "receipt-prior".into(),
        registration_id: "launch-prior".into(),
        provider: Provider::ClaudeCode,
        native_session_id: "native-session-49".into(),
        source: SourceMetadata {
            path: source.clone(),
            format: SourceFormat::JsonLines,
        },
        provider_process: prior_process,
        issued_at_ms: 50_000,
    };
    let session = ManagedSessionRecord {
        provider: Provider::ClaudeCode,
        native_session_id: "native-session-49".into(),
        target: ManagedLaunchTarget { working_directory },
        session_alias: "Resume target".into(),
        description: None,
        binding_receipt_id: receipt.id.clone(),
    };
    let run = mudi::application::managed_sessions::ManagedCurrentRunRecord {
        id: "run-prior".into(),
        opaque_ref: "run_0123456789abcdef0123456789abcdef".into(),
        provider: Provider::ClaudeCode,
        native_session_id: "native-session-49".into(),
        provider_process: prior_process,
        tmux: mudi::application::managed_sessions::PrivateTmuxEnvironment {
            socket_name: "mudi-prior".into(),
            session_name: "managed".into(),
        },
        binding_receipt_id: receipt.id.clone(),
    };

    let exited = SystemManagedResumeEvidence::new(
        Box::new(FixedObservation(ProviderProcessObservation::Absent)),
        Box::new(SystemManagedResumeProviderEvidence::new(config.clone())),
    )
    .observe(&run, &session, &receipt)
    .unwrap();
    assert_eq!(exited, ManagedCurrentRunLifecycle::Exited);

    let broken_config = Config {
        claude_dir: temp.0.join("broken-claude"),
        ..config.clone()
    };
    fs::create_dir_all(&broken_config.claude_dir).unwrap();
    fs::write(broken_config.claude_sessions_dir(), "not a directory").unwrap();
    let failed_probe = SystemManagedResumeEvidence::new(
        Box::new(FixedObservation(ProviderProcessObservation::Absent)),
        Box::new(SystemManagedResumeProviderEvidence::new(
            broken_config.clone(),
        )),
    )
    .observe(&run, &session, &receipt)
    .unwrap();
    assert_eq!(failed_probe, ManagedCurrentRunLifecycle::Unknown);

    let unknown = SystemManagedResumeEvidence::new(
        Box::new(FixedObservation(ProviderProcessObservation::Unknown)),
        Box::new(SystemManagedResumeProviderEvidence::new(config.clone())),
    )
    .observe(&run, &session, &receipt)
    .unwrap();
    assert_eq!(unknown, ManagedCurrentRunLifecycle::Unknown);
    let reused = SystemManagedResumeEvidence::new(
        Box::new(FixedObservation(ProviderProcessObservation::Live(
            ProcessIdentity {
                pid: prior_process.pid,
                started_at_ms: 99_000,
            },
        ))),
        Box::new(SystemManagedResumeProviderEvidence::new(config.clone())),
    )
    .observe(&run, &session, &receipt)
    .unwrap();
    assert_eq!(reused, ManagedCurrentRunLifecycle::Unknown);

    let mut external_child = std::process::Command::new("sleep")
        .arg("10")
        .spawn()
        .unwrap();
    let external_process = SystemProcessEvidence
        .current_identity(external_child.id())
        .unwrap();
    fs::create_dir_all(config.claude_sessions_dir()).unwrap();
    fs::write(
        config
            .claude_sessions_dir()
            .join(format!("{}.json", external_process.pid)),
        serde_json::json!({
            "pid": external_process.pid,
            "sessionId": "native-session-49",
            "status": "idle",
            "startedAt": external_process.started_at_ms
        })
        .to_string(),
    )
    .unwrap();
    let externally_open = SystemManagedResumeEvidence::new(
        Box::new(FixedObservation(ProviderProcessObservation::Absent)),
        Box::new(SystemManagedResumeProviderEvidence::new(config.clone())),
    )
    .observe(&run, &session, &receipt)
    .unwrap();
    assert_eq!(externally_open, ManagedCurrentRunLifecycle::Open);
    let _ = external_child.kill();
    let _ = external_child.wait();
}

#[test]
fn only_verified_exited_managed_sessions_offer_an_explicit_resume_confirmation() {
    let resumable = ManagedResumeSummary {
        provider: Provider::ClaudeCode,
        native_session_id: "native-session-49".into(),
    };
    let eligible = RefreshSnapshot {
        sessions: vec![
            display_session(SessionState::Exited, SessionProvenance::Managed),
            open_companion_session(),
        ],
        resumable_managed_sessions: vec![resumable.clone()],
        ready: true,
        ..RefreshSnapshot::default()
    };
    let mut ui = Ui::default();
    open_session_detail(&mut ui, &eligible);
    assert!(render_ui(&ui, &eligible).contains("r Resume"));

    assert_eq!(ui.handle_key(key('r'), &eligible).unwrap(), UiAction::None);
    let confirmation = render_ui(&ui, &eligible);
    assert!(confirmation.contains("Authorize Resume"));
    assert!(confirmation.contains("Claude Code"));
    assert!(confirmation.contains("native-session-49"));
    let compact_confirmation = render_ui_at(&ui, &eligible, 80, 24);
    for required in [
        "Provider    Claude Code",
        "Session ID  native-session-49",
        "Enter Authorize",
        "Esc Cancel",
    ] {
        assert!(
            compact_confirmation.contains(required),
            "missing {required:?}"
        );
    }
    assert!(!compact_confirmation.contains("Full access · No sandbox · No approvals"));
    assert!(!compact_confirmation.contains("Terminal too small"));
    assert_eq!(
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &eligible)
            .unwrap(),
        UiAction::StartManagedResume {
            request: ManagedResumeRequest {
                provider: Provider::ClaudeCode,
                native_session_id: "native-session-49".into(),
            }
        }
    );

    let ineligible = [
        (
            display_session(SessionState::Idle, SessionProvenance::Managed),
            vec![resumable.clone()],
        ),
        (
            display_session(SessionState::Exited, SessionProvenance::Observed),
            vec![resumable.clone()],
        ),
        (
            display_session(SessionState::Exited, SessionProvenance::Managed),
            Vec::new(),
        ),
    ];
    for (session, resumable_managed_sessions) in ineligible {
        let snapshot = RefreshSnapshot {
            sessions: vec![session, open_companion_session()],
            resumable_managed_sessions,
            ready: true,
            ..RefreshSnapshot::default()
        };
        let mut ui = Ui::default();
        open_session_detail(&mut ui, &snapshot);
        assert!(!render_ui(&ui, &snapshot).contains("r Resume"));
        assert!(!matches!(
            ui.handle_key(key('r'), &snapshot).unwrap(),
            UiAction::StartManagedResume { .. }
        ));
    }
}

#[test]
fn codex_resume_discloses_full_access_before_authorization() {
    let mut session = display_session(SessionState::Exited, SessionProvenance::Managed);
    session.provider = Provider::CodexCli;
    session.latest_model = Some("gpt-5-codex".into());
    let eligible = RefreshSnapshot {
        sessions: vec![session, open_companion_session()],
        resumable_managed_sessions: vec![ManagedResumeSummary {
            provider: Provider::CodexCli,
            native_session_id: "native-session-49".into(),
        }],
        ready: true,
        ..RefreshSnapshot::default()
    };
    let mut ui = Ui::default();
    open_session_detail(&mut ui, &eligible);
    ui.handle_key(key('r'), &eligible).unwrap();

    let confirmation = render_ui_at(&ui, &eligible, 80, 24);
    assert!(confirmation.contains("Authorize Resume"));
    assert!(confirmation.contains("Provider    Codex CLI"));
    assert!(confirmation.contains("Full access · No sandbox · No approvals"));
}

#[test]
fn refresh_projects_resume_eligibility_only_for_verified_exited_evidence() {
    for (lifecycle, expected) in [
        (ManagedCurrentRunLifecycle::Open, 0),
        (ManagedCurrentRunLifecycle::Unknown, 0),
        (ManagedCurrentRunLifecycle::Exited, 1),
    ] {
        let temp = TempDir::new("resume-refresh-transition");
        let config = temp.config();
        let mut store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
        bind_initial_managed_session(
            &mut store,
            &temp,
            Provider::CodexCli,
            "native-session-49",
            ProcessIdentity {
                pid: 42,
                started_at_ms: 41_000,
            },
        );
        drop(store);
        let mut refresh = open_refresh_cycle(config).with_managed_resume_evidence(Box::new(
            FixedResumeEvidence {
                lifecycle,
                verified: Arc::new(Mutex::new(Vec::new())),
            },
        ));

        let snapshot = refresh.run_once();

        assert_eq!(snapshot.resumable_managed_sessions.len(), expected);
        if expected == 1 {
            assert_eq!(
                snapshot.resumable_managed_sessions[0],
                ManagedResumeSummary {
                    provider: Provider::CodexCli,
                    native_session_id: "native-session-49".into(),
                }
            );
        }
    }
}

#[test]
fn bulk_resume_projection_samples_process_and_provider_evidence_once() {
    let temp = TempDir::new("resume-bulk-projection");
    let config = temp.config();
    let mut store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
    for (index, native_session_id) in ["native-session-a", "native-session-b", "native-session-c"]
        .into_iter()
        .enumerate()
    {
        bind_initial_managed_session(
            &mut store,
            &temp,
            Provider::CodexCli,
            native_session_id,
            ProcessIdentity {
                pid: 100 + index as u32,
                started_at_ms: 41_000 + index as i64,
            },
        );
    }
    let process_single = Arc::new(AtomicUsize::new(0));
    let process_bulk = Arc::new(AtomicUsize::new(0));
    let provider_single = Arc::new(AtomicUsize::new(0));
    let provider_bulk = Arc::new(AtomicUsize::new(0));
    let verifier =
        ManagedResumeVerifier::new(Box::new(SystemManagedResumeEvidence::with_evidence(
            Box::new(CountingProcessEvidence {
                single: process_single.clone(),
                bulk: process_bulk.clone(),
            }),
            Box::new(CountingProviderEvidence {
                single: provider_single.clone(),
                bulk: provider_bulk.clone(),
            }),
        )));

    let eligible = verifier.verified_exited_sessions(&store).unwrap();

    assert_eq!(eligible.len(), 3);
    assert_eq!(process_bulk.load(Ordering::SeqCst), 1);
    assert_eq!(provider_bulk.load(Ordering::SeqCst), 1);
    assert_eq!(process_single.load(Ordering::SeqCst), 0);
    assert_eq!(provider_single.load(Ordering::SeqCst), 0);

    verifier
        .verify_exited(&store, Provider::CodexCli, "native-session-a")
        .unwrap();
    assert_eq!(process_bulk.load(Ordering::SeqCst), 1);
    assert_eq!(provider_bulk.load(Ordering::SeqCst), 1);
    assert_eq!(process_single.load(Ordering::SeqCst), 1);
    assert_eq!(provider_single.load(Ordering::SeqCst), 1);
}

#[test]
fn empty_resume_projection_samples_no_system_evidence() {
    let temp = TempDir::new("resume-empty-projection");
    let config = temp.config();
    let store = ManagedLaunchStore::open(config.managed_launch_store_path()).unwrap();
    let process_single = Arc::new(AtomicUsize::new(0));
    let process_bulk = Arc::new(AtomicUsize::new(0));
    let provider_single = Arc::new(AtomicUsize::new(0));
    let provider_bulk = Arc::new(AtomicUsize::new(0));
    let verifier =
        ManagedResumeVerifier::new(Box::new(SystemManagedResumeEvidence::with_evidence(
            Box::new(CountingProcessEvidence {
                single: process_single.clone(),
                bulk: process_bulk.clone(),
            }),
            Box::new(CountingProviderEvidence {
                single: provider_single.clone(),
                bulk: provider_bulk.clone(),
            }),
        )));

    assert!(verifier
        .verified_exited_sessions(&store)
        .unwrap()
        .is_empty());
    assert_eq!(process_bulk.load(Ordering::SeqCst), 0);
    assert_eq!(provider_bulk.load(Ordering::SeqCst), 0);
    assert_eq!(process_single.load(Ordering::SeqCst), 0);
    assert_eq!(provider_single.load(Ordering::SeqCst), 0);
}

#[test]
fn application_routes_authorized_resume_and_requests_a_fresh_projection() {
    let temp = TempDir::new("resume-application");
    let config = temp.config();
    let store_path = config.managed_launch_store_path();
    let mut store = ManagedLaunchStore::open(&store_path).unwrap();
    bind_initial_managed_session(
        &mut store,
        &temp,
        Provider::ClaudeCode,
        "native-session-49",
        ProcessIdentity {
            pid: 42,
            started_at_ms: 41_000,
        },
    );
    let launched = Arc::new(Mutex::new(Vec::new()));
    let opened = Arc::new(Mutex::new(Vec::new()));
    let executables = ProviderExecutables::new(temp.executable("claude"), temp.executable("codex"));
    let managed_sessions = ManagedSessionService::new(
        store,
        executables.clone(),
        Box::new(RecordingLaunchBroker {
            store_path: store_path.clone(),
            launched: launched.clone(),
        }),
        Box::new(RecordingTerminal {
            opened: opened.clone(),
        }),
    )
    .with_resume_verifier(ManagedResumeVerifier::new(Box::new(FixedResumeEvidence {
        lifecycle: ManagedCurrentRunLifecycle::Exited,
        verified: Arc::new(Mutex::new(Vec::new())),
    })));
    let handoff = open_handoff_service(
        HandoffPaths {
            task_store: temp.0.join("tasks.db"),
            artifact_root: temp.0.join("artifacts"),
        },
        executables,
        NeverHandoffWorker,
    )
    .unwrap();
    let mut controller = workflow_handlers(
        handoff,
        NeverSuccessorTerminal,
        managed_sessions,
        temp.0.join("workflow-state.db"),
    );
    let snapshot = RefreshSnapshot {
        sessions: vec![display_session(
            SessionState::Exited,
            SessionProvenance::Managed,
        )],
        resumable_managed_sessions: vec![ManagedResumeSummary {
            provider: Provider::ClaudeCode,
            native_session_id: "native-session-49".into(),
        }],
        ready: true,
        ..RefreshSnapshot::default()
    };
    let mut ui = Ui::default();
    let (rescan_tx, mut rescan_rx) = tokio::sync::mpsc::unbounded_channel();

    assert!(!controller.handle(
        UiAction::StartManagedResume {
            request: ManagedResumeRequest {
                provider: Provider::ClaudeCode,
                native_session_id: "native-session-49".into(),
            },
        },
        &snapshot,
        &mut ui,
        &mut NoopClipboard,
        &rescan_tx,
    ));

    assert!(rescan_rx.try_recv().is_ok());
    assert_eq!(launched.lock().unwrap().len(), 1);
    assert_eq!(opened.lock().unwrap().len(), 1);
    assert!(render_ui(&ui, &snapshot).contains("Started Claude Code Resume in Terminal.app"));
}

struct NeverHandoffWorker;

impl WorkerLauncher for NeverHandoffWorker {
    fn launch(&self, _launch: &WorkerLaunch) -> std::io::Result<()> {
        panic!("Managed Resume must not launch a Handoff worker")
    }
}

struct NeverSuccessorTerminal;

impl TerminalLauncher for NeverSuccessorTerminal {
    fn open(&mut self, _task_id: &str, _now_ms: i64) -> Result<()> {
        panic!("Managed Resume must not launch a Successor")
    }
}

struct NoopClipboard;

impl ClipboardPort for NoopClipboard {
    fn write_text(&mut self, _value: &str) -> std::io::Result<()> {
        Ok(())
    }
}

fn display_session(state: SessionState, provenance: SessionProvenance) -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: "native-session-49".into(),
        provenance,
        source: SourceMetadata::default(),
        project: Some(Project {
            identity: ProjectIdentity::new(PathBuf::from("/workspace/mudi")),
            label: "mudi".into(),
        }),
        cwd: Some("/workspace/mudi".into()),
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

fn open_companion_session() -> Session {
    let mut session = display_session(SessionState::Idle, SessionProvenance::Observed);
    session.id = "open-companion".into();
    session
}

fn key(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
}

fn open_session_detail(ui: &mut Ui, snapshot: &RefreshSnapshot) {
    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), snapshot)
        .unwrap();
}

fn render_ui(ui: &Ui, snapshot: &RefreshSnapshot) -> String {
    render_ui_at(ui, snapshot, 120, 36)
}

fn render_ui_at(ui: &Ui, snapshot: &RefreshSnapshot, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
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

struct FixedObservation(ProviderProcessObservation);

impl ProcessEvidence for FixedObservation {
    fn observe(&self, _pid: u32) -> ProviderProcessObservation {
        self.0
    }
}

struct FixedResumeEvidence {
    lifecycle: ManagedCurrentRunLifecycle,
    verified: Arc<Mutex<Vec<(Provider, String, String)>>>,
}

struct CountingProcessEvidence {
    single: Arc<AtomicUsize>,
    bulk: Arc<AtomicUsize>,
}

impl ProcessEvidence for CountingProcessEvidence {
    fn observe(&self, _pid: u32) -> ProviderProcessObservation {
        self.single.fetch_add(1, Ordering::SeqCst);
        ProviderProcessObservation::Absent
    }

    fn observe_many(&self, pids: &[u32]) -> Vec<ProviderProcessObservation> {
        self.bulk.fetch_add(1, Ordering::SeqCst);
        vec![ProviderProcessObservation::Absent; pids.len()]
    }
}

struct CountingProviderEvidence {
    single: Arc<AtomicUsize>,
    bulk: Arc<AtomicUsize>,
}

impl ManagedResumeProviderEvidence for CountingProviderEvidence {
    fn observe(
        &self,
        _run: &mudi::application::managed_sessions::ManagedCurrentRunRecord,
        _session: &ManagedSessionRecord,
        _receipt: &mudi::application::managed_sessions::BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle> {
        self.single.fetch_add(1, Ordering::SeqCst);
        Ok(ManagedCurrentRunLifecycle::Exited)
    }

    fn observe_many(
        &self,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        self.bulk.fetch_add(1, Ordering::SeqCst);
        candidates
            .iter()
            .map(|_| Ok(ManagedCurrentRunLifecycle::Exited))
            .collect()
    }
}

impl ManagedResumeEvidence for FixedResumeEvidence {
    fn observe(
        &self,
        run: &mudi::application::managed_sessions::ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        _receipt: &mudi::application::managed_sessions::BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle> {
        self.verified.lock().unwrap().push((
            session.provider,
            session.native_session_id.clone(),
            run.id.clone(),
        ));
        Ok(self.lifecycle)
    }
}

type RecordedLaunch = (Provider, String, PathBuf, Vec<String>);

struct RecordingLaunchBroker {
    store_path: PathBuf,
    launched: Arc<Mutex<Vec<RecordedLaunch>>>,
}

struct FailingLaunchBroker;

impl ManagedLaunchBroker for FailingLaunchBroker {
    fn launch(
        &mut self,
        _registration: &ManagedLaunchRegistration,
    ) -> Result<ManagedLaunchEvidence> {
        anyhow::bail!("provider executable rejected Resume launch")
    }
}

struct NeverTerminal;

impl ManagedTerminalAccess for NeverTerminal {
    fn open(&mut self, _registration: &ManagedLaunchRegistration) -> Result<()> {
        panic!("failed or rejected Resume must not open Terminal.app")
    }
}

struct FailingTerminal;

impl ManagedTerminalAccess for FailingTerminal {
    fn open(&mut self, _registration: &ManagedLaunchRegistration) -> Result<()> {
        anyhow::bail!("Terminal.app launch failed")
    }
}

impl ManagedLaunchBroker for RecordingLaunchBroker {
    fn launch(
        &mut self,
        registration: &ManagedLaunchRegistration,
    ) -> Result<ManagedLaunchEvidence> {
        let target = ManagedLaunchStore::open(&self.store_path)?
            .managed_resume_target(&registration.id)?
            .expect("Resume identity must be durable before Provider launch");
        self.launched.lock().unwrap().push((
            registration.provider,
            target.native_session_id,
            registration.provider_command.executable.clone(),
            registration.provider_command.arguments.clone(),
        ));
        Ok(ManagedLaunchEvidence {
            tmux_pane_id: "%84".into(),
            provider_process: ProcessIdentity {
                pid: 84,
                started_at_ms: 59_000,
            },
        })
    }

    fn cancel(&mut self, _registration: &ManagedLaunchRegistration) -> Result<()> {
        Ok(())
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

#[derive(Clone)]
struct FixedProcessEvidence(ProcessIdentity);

impl ProcessEvidence for FixedProcessEvidence {
    fn observe(&self, _pid: u32) -> ProviderProcessObservation {
        ProviderProcessObservation::Live(self.0)
    }
}

struct NoopCleanup;

impl ManagedLaunchCleanup for NoopCleanup {
    fn clean_empty(&self, _registration: &ManagedLaunchRegistration) -> Result<()> {
        Ok(())
    }
}

fn bind_initial_managed_session(
    store: &mut ManagedLaunchStore,
    temp: &TempDir,
    provider: Provider,
    native_session_id: &str,
    process: ProcessIdentity,
) -> String {
    let working_directory = temp.directory("working_directory");
    let source = temp.file("source.jsonl", "{}\n");
    let registration = ManagedLaunchRegistration {
        id: format!("launch-initial-{native_session_id}"),
        status: mudi::application::managed_sessions::ManagedLaunchStatus::Binding,
        provider,
        target: ManagedLaunchTarget {
            working_directory: working_directory.clone(),
        },
        session_alias: "Resume target".into(),
        description: Some("Keep local metadata".into()),
        binding_initializer: false,
        provider_command: mudi::application::managed_sessions::ProviderNewCommand {
            executable: PathBuf::new(),
            arguments: Vec::new(),
        },
        tmux: mudi::application::managed_sessions::PrivateTmuxEnvironment {
            socket_name: format!("mudi-launch-initial-{native_session_id}"),
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
    assert!(matches!(
        binder
            .bind(
                store,
                &registration.id,
                Some(ProviderBindingEvidence {
                    provider,
                    source_session_id: native_session_id.into(),
                    process_session_id: native_session_id.into(),
                    source: SourceMetadata {
                        path: source.canonicalize().unwrap(),
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
    store.managed_current_runs().unwrap()[0].id.clone()
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mudi-managed-resume-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
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
        path
    }

    fn executable(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
