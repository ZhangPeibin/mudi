use sessionmesh::attention::SessionIdentity;
use sessionmesh::domain::{
    Compaction, ContextState, Project, ProjectIdentity, Provider, Session, SessionState, Usage,
    Worktree,
};
use sessionmesh::signals::{
    ContextAlertLevel, RecentSignal, RecentSignalKind, RecentSignalsEngine,
    SharedWorktreeRiskChange,
};

fn session(id: &str, state: SessionState, context_tokens: u64) -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: id.into(),
        source: Default::default(),
        project: Some(Project {
            identity: ProjectIdentity::CanonicalCwd("/tmp/sessionmesh".into()),
            label: "sessionmesh".into(),
        }),
        worktree: None,
        cwd: Some("/tmp/sessionmesh".into()),
        latest_model: Some("claude-opus-4-1".into()),
        state,
        context: ContextState {
            tokens: context_tokens,
            window: Some(200_000),
        },
        usage: Usage::default(),
        per_model: vec![],
        current_run_usage: None,
        compactions: vec![],
        last_activity_ms: 1_000,
        cost: None,
    }
}

fn session_signal(signal: &RecentSignal) -> (i64, Provider, &str, &RecentSignalKind) {
    match signal {
        RecentSignal::Session {
            observed_at_ms,
            provider,
            session_id,
            kind,
        } => (*observed_at_ms, *provider, session_id, kind),
        RecentSignal::Worktree { .. } => panic!("expected Session Signal"),
    }
}

#[test]
fn initial_snapshot_establishes_a_silent_baseline() {
    let mut engine = RecentSignalsEngine::default();

    let signals = engine.update(
        &[session("already-idle", SessionState::Idle, 180_000)],
        1_000,
    );

    assert!(signals.is_empty());
}

#[test]
fn shared_worktree_risk_detection_emits_one_worktree_scoped_signal() {
    let mut engine = RecentSignalsEngine::default();
    let worktree_root = "/tmp/sessionmesh-shared";
    let mut claude = session("claude-session", SessionState::Busy, 80_000);
    claude.worktree = Some(Worktree {
        root: worktree_root.into(),
        branch: Some("main".into()),
        writable: true,
    });
    engine.update(&[claude.clone()], 1_000);

    let mut codex = claude.clone();
    codex.provider = Provider::CodexCli;
    codex.id = "codex-session".into();
    let signals = engine.update(&[claude, codex], 2_000);

    assert_eq!(
        signals,
        &[RecentSignal::Worktree {
            observed_at_ms: 2_000,
            worktree_root: worktree_root.into(),
            change: SharedWorktreeRiskChange::Detected,
            before: vec![SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "claude-session".into(),
            }],
            after: vec![
                SessionIdentity {
                    provider: Provider::ClaudeCode,
                    native_id: "claude-session".into(),
                },
                SessionIdentity {
                    provider: Provider::CodexCli,
                    native_id: "codex-session".into(),
                },
            ],
        }]
    );
}

#[test]
fn shared_worktree_risk_clear_emits_once_with_remaining_open_membership() {
    let mut engine = RecentSignalsEngine::default();
    let worktree_root = "/tmp/sessionmesh-shared";
    let mut claude = session("claude-session", SessionState::Busy, 80_000);
    claude.worktree = Some(Worktree {
        root: worktree_root.into(),
        branch: Some("main".into()),
        writable: true,
    });
    let mut codex = claude.clone();
    codex.provider = Provider::CodexCli;
    codex.id = "codex-session".into();
    engine.update(&[claude.clone(), codex.clone()], 1_000);

    codex.state = SessionState::Exited;
    let expected = RecentSignal::Worktree {
        observed_at_ms: 2_000,
        worktree_root: worktree_root.into(),
        change: SharedWorktreeRiskChange::Cleared,
        before: vec![
            SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "claude-session".into(),
            },
            SessionIdentity {
                provider: Provider::CodexCli,
                native_id: "codex-session".into(),
            },
        ],
        after: vec![SessionIdentity {
            provider: Provider::ClaudeCode,
            native_id: "claude-session".into(),
        }],
    };
    let observed = vec![claude, codex];
    let signals = engine.update(&observed, 2_000);
    assert!(signals.contains(&expected));
    assert_eq!(
        signals
            .iter()
            .filter(|signal| matches!(signal, RecentSignal::Worktree { .. }))
            .count(),
        1
    );

    let unchanged = engine.update(&observed, 3_000);
    assert!(unchanged.contains(&expected));
    assert_eq!(
        unchanged
            .iter()
            .filter(|signal| matches!(signal, RecentSignal::Worktree { .. }))
            .count(),
        1
    );
}

#[test]
fn membership_changes_while_worktree_risk_remains_do_not_emit_a_signal() {
    let mut engine = RecentSignalsEngine::default();
    let mut first = session("first", SessionState::Busy, 80_000);
    first.worktree = Some(Worktree {
        root: "/tmp/sessionmesh-shared".into(),
        branch: Some("main".into()),
        writable: true,
    });
    let mut second = first.clone();
    second.id = "second".into();
    engine.update(&[first.clone(), second.clone()], 1_000);

    let mut third = first.clone();
    third.provider = Provider::CodexCli;
    third.id = "third".into();

    assert!(engine.update(&[first, second, third], 2_000).is_empty());
}

#[test]
fn worktree_signals_share_the_five_entry_newest_first_retention() {
    let mut engine = RecentSignalsEngine::default();
    let mut sessions = (0..6)
        .map(|index| {
            let mut observed = session(&format!("claude-{index}"), SessionState::Busy, 80_000);
            observed.worktree = Some(Worktree {
                root: format!("/tmp/worktree-{index}").into(),
                branch: Some("main".into()),
                writable: true,
            });
            observed
        })
        .collect::<Vec<_>>();
    engine.update(&sessions, 1_000);

    for index in 0..6 {
        let mut contributor = sessions[index].clone();
        contributor.provider = Provider::CodexCli;
        contributor.id = format!("codex-{index}");
        sessions.push(contributor);
        engine.update(&sessions, 2_000 + index as i64);
    }

    let retained = engine.update(&sessions, 3_000);
    assert_eq!(retained.len(), 5);
    assert_eq!(
        retained
            .iter()
            .map(|signal| match signal {
                RecentSignal::Worktree { worktree_root, .. } => {
                    worktree_root.display().to_string()
                }
                RecentSignal::Session { .. } => panic!("expected Worktree Signal"),
            })
            .collect::<Vec<_>>(),
        vec![
            "/tmp/worktree-5",
            "/tmp/worktree-4",
            "/tmp/worktree-3",
            "/tmp/worktree-2",
            "/tmp/worktree-1",
        ]
    );
}

#[test]
fn busy_to_idle_emits_a_session_scoped_signal() {
    let mut engine = RecentSignalsEngine::default();
    engine.update(&[session("needs-input", SessionState::Busy, 80_000)], 1_000);

    let signals = engine.update(&[session("needs-input", SessionState::Idle, 80_000)], 2_000);

    assert_eq!(signals.len(), 1);
    let (observed_at_ms, provider, session_id, kind) = session_signal(&signals[0]);
    assert_eq!(observed_at_ms, 2_000);
    assert_eq!(provider, Provider::ClaudeCode);
    assert_eq!(session_id, "needs-input");
    assert_eq!(
        kind,
        &RecentSignalKind::SessionStateChanged {
            from: SessionState::Busy,
            to: SessionState::Idle,
        }
    );
}

#[test]
fn an_open_session_becoming_exited_emits_a_signal() {
    for open_state in [SessionState::Busy, SessionState::Idle] {
        let mut engine = RecentSignalsEngine::default();
        engine.update(&[session("finished", open_state, 80_000)], 1_000);

        let signals = engine.update(&[session("finished", SessionState::Exited, 80_000)], 2_000);

        assert_eq!(signals.len(), 1);
        assert_eq!(
            session_signal(&signals[0]).3,
            &RecentSignalKind::SessionStateChanged {
                from: open_state,
                to: SessionState::Exited,
            }
        );
    }
}

#[test]
fn an_exited_session_becoming_open_emits_a_signal() {
    for open_state in [SessionState::Busy, SessionState::Idle] {
        let mut engine = RecentSignalsEngine::default();
        engine.update(&[session("resumed", SessionState::Exited, 80_000)], 1_000);

        let signals = engine.update(&[session("resumed", open_state, 80_000)], 2_000);

        assert_eq!(signals.len(), 1);
        assert_eq!(
            session_signal(&signals[0]).3,
            &RecentSignalKind::SessionStateChanged {
                from: SessionState::Exited,
                to: open_state,
            }
        );
    }
}

#[test]
fn context_entering_warning_emits_before_and_after_evidence() {
    let mut engine = RecentSignalsEngine::default();
    engine.update(&[session("pressure", SessionState::Busy, 120_000)], 1_000);

    let signals = engine.update(&[session("pressure", SessionState::Busy, 150_000)], 2_000);

    assert_eq!(signals.len(), 1);
    assert_eq!(
        session_signal(&signals[0]).3,
        &RecentSignalKind::ContextAlertChanged {
            from: None,
            to: Some(ContextAlertLevel::Warning),
            before: ContextState {
                tokens: 120_000,
                window: Some(200_000),
            },
            after: ContextState {
                tokens: 150_000,
                window: Some(200_000),
            },
        }
    );
}

#[test]
fn context_entering_critical_emits_the_new_alert_level() {
    let mut engine = RecentSignalsEngine::default();
    engine.update(&[session("critical", SessionState::Busy, 150_000)], 1_000);

    let signals = engine.update(&[session("critical", SessionState::Busy, 180_000)], 2_000);

    assert_eq!(signals.len(), 1);
    assert_eq!(
        session_signal(&signals[0]).3,
        &RecentSignalKind::ContextAlertChanged {
            from: Some(ContextAlertLevel::Warning),
            to: Some(ContextAlertLevel::Critical),
            before: ContextState {
                tokens: 150_000,
                window: Some(200_000),
            },
            after: ContextState {
                tokens: 180_000,
                window: Some(200_000),
            },
        }
    );
}

#[test]
fn a_newly_observed_compaction_emits_its_metadata_and_context_evidence() {
    let mut engine = RecentSignalsEngine::default();
    engine.update(&[session("compacted", SessionState::Busy, 120_000)], 1_000);
    let mut compacted = session("compacted", SessionState::Busy, 110_000);
    compacted.compactions.push(Compaction {
        at_ms: 1_900,
        trigger: Some("auto".into()),
        pre_tokens: Some(120_000),
    });

    let signals = engine.update(&[compacted], 2_000);

    assert_eq!(signals.len(), 1);
    assert_eq!(
        session_signal(&signals[0]).3,
        &RecentSignalKind::CompactionObserved {
            compaction: Compaction {
                at_ms: 1_900,
                trigger: Some("auto".into()),
                pre_tokens: Some(120_000),
            },
            context_before: ContextState {
                tokens: 120_000,
                window: Some(200_000),
            },
            context_after: ContextState {
                tokens: 110_000,
                window: Some(200_000),
            },
            cleared_alert: None,
        }
    );
}

#[test]
fn compaction_and_context_alert_clear_form_one_compound_signal() {
    let mut engine = RecentSignalsEngine::default();
    engine.update(&[session("recovered", SessionState::Busy, 180_000)], 1_000);
    let mut compacted = session("recovered", SessionState::Busy, 80_000);
    compacted.compactions.push(Compaction {
        at_ms: 1_900,
        trigger: Some("auto".into()),
        pre_tokens: Some(180_000),
    });

    let signals = engine.update(&[compacted], 2_000);

    assert_eq!(signals.len(), 1);
    assert_eq!(
        session_signal(&signals[0]).3,
        &RecentSignalKind::CompactionObserved {
            compaction: Compaction {
                at_ms: 1_900,
                trigger: Some("auto".into()),
                pre_tokens: Some(180_000),
            },
            context_before: ContextState {
                tokens: 180_000,
                window: Some(200_000),
            },
            context_after: ContextState {
                tokens: 80_000,
                window: Some(200_000),
            },
            cleared_alert: Some(ContextAlertLevel::Critical),
        }
    );
}

#[test]
fn only_the_five_newest_signals_are_retained_newest_first() {
    let mut engine = RecentSignalsEngine::default();
    let mut sessions = (0..6)
        .map(|index| session(&format!("session-{index}"), SessionState::Busy, 80_000))
        .collect::<Vec<_>>();
    engine.update(&sessions, 1_000);

    for index in 0..6 {
        sessions[index].state = SessionState::Idle;
        engine.update(&sessions, 2_000 + index as i64);
    }

    let signals = engine.update(&sessions, 3_000);
    assert_eq!(signals.len(), 5);
    assert_eq!(
        signals
            .iter()
            .map(|signal| session_signal(signal).2)
            .collect::<Vec<_>>(),
        vec![
            "session-5",
            "session-4",
            "session-3",
            "session-2",
            "session-1"
        ]
    );
    assert_eq!(
        signals
            .iter()
            .map(RecentSignal::observed_at_ms)
            .collect::<Vec<_>>(),
        vec![2_005, 2_004, 2_003, 2_002, 2_001]
    );
}

#[test]
fn context_alerts_emit_when_warning_or_critical_clears() {
    for (before_tokens, after_tokens, from, to) in [
        (150_000, 100_000, ContextAlertLevel::Warning, None),
        (
            180_000,
            150_000,
            ContextAlertLevel::Critical,
            Some(ContextAlertLevel::Warning),
        ),
    ] {
        let mut engine = RecentSignalsEngine::default();
        engine.update(
            &[session("cleared", SessionState::Busy, before_tokens)],
            1_000,
        );

        let signals = engine.update(
            &[session("cleared", SessionState::Busy, after_tokens)],
            2_000,
        );

        assert_eq!(signals.len(), 1);
        assert!(matches!(
            session_signal(&signals[0]).3,
            RecentSignalKind::ContextAlertChanged {
                from: Some(observed_from),
                to: observed_to,
                ..
            } if *observed_from == from && *observed_to == to
        ));
    }
}

#[test]
fn non_signal_session_updates_are_ignored() {
    let mut engine = RecentSignalsEngine::default();
    engine.update(&[session("routine", SessionState::Idle, 100_000)], 1_000);

    let mut changed = session("routine", SessionState::Busy, 110_000);
    changed.usage.input = 50_000;
    changed.cost = Some(12.34);
    changed.last_activity_ms = 1_900;
    assert!(engine.update(&[changed.clone()], 2_000).is_empty());

    changed.context.tokens = 120_000;
    changed.usage.output = 10_000;
    changed.last_activity_ms = 2_900;
    assert!(engine.update(&[changed], 3_000).is_empty());
}

#[test]
fn repeated_polling_does_not_duplicate_retained_signals() {
    let mut engine = RecentSignalsEngine::default();
    engine.update(&[session("stable", SessionState::Busy, 100_000)], 1_000);
    let mut observed = session("stable", SessionState::Idle, 90_000);
    observed.compactions.push(Compaction {
        at_ms: 1_900,
        trigger: Some("manual".into()),
        pre_tokens: Some(100_000),
    });

    assert_eq!(engine.update(&[observed.clone()], 2_000).len(), 2);
    let unchanged = engine.update(&[observed], 3_000);

    assert_eq!(unchanged.len(), 2);
    assert!(unchanged
        .iter()
        .all(|signal| signal.observed_at_ms() == 2_000));
}

#[test]
fn multiple_new_compactions_are_ordered_by_event_time_newest_first() {
    let mut engine = RecentSignalsEngine::default();
    engine.update(&[session("several", SessionState::Busy, 120_000)], 1_000);
    let mut compacted = session("several", SessionState::Busy, 80_000);
    compacted.compactions = vec![
        Compaction {
            at_ms: 1_900,
            trigger: Some("newer".into()),
            pre_tokens: Some(100_000),
        },
        Compaction {
            at_ms: 1_800,
            trigger: Some("older".into()),
            pre_tokens: Some(120_000),
        },
    ];

    let signals = engine.update(&[compacted], 2_000);

    assert_eq!(signals.len(), 2);
    assert_eq!(
        signals
            .iter()
            .map(|signal| match session_signal(signal).3 {
                RecentSignalKind::CompactionObserved { compaction, .. } => compaction.at_ms,
                _ => panic!("expected Compaction Signal"),
            })
            .collect::<Vec<_>>(),
        vec![1_900, 1_800]
    );
}
