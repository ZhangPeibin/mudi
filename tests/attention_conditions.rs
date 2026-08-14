use std::path::PathBuf;

use mudi::attention::{AttentionCondition, AttentionEngine, AttentionRank, AttentionSeverity};
use mudi::domain::{
    ContextState, Project, ProjectIdentity, Provider, Session, SessionState, Worktree,
};

fn session(
    id: &str,
    provider: Provider,
    state: SessionState,
    context_percent: u64,
    project_path: &str,
    worktree_path: Option<&str>,
) -> Session {
    Session {
        provider,
        id: id.into(),
        provenance: mudi::domain::SessionProvenance::Observed,
        source: Default::default(),
        project: Some(Project {
            identity: ProjectIdentity::GitCommonDir(PathBuf::from(project_path).join(".git")),
            label: PathBuf::from(project_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        }),
        worktree: worktree_path.map(|root| Worktree {
            root: PathBuf::from(root),
            branch: Some("main".into()),
            writable: true,
        }),
        cwd: worktree_path.map(str::to_string),
        latest_model: Some("test-model".into()),
        state,
        context: ContextState {
            tokens: context_percent,
            window: Some(100),
        },
        usage: Default::default(),
        per_model: vec![],
        current_run_usage: None,
        compactions: vec![],
        last_activity_ms: 1_000,
        cost: None,
    }
}

#[test]
fn now_orders_critical_shared_warning_idle_then_healthy_busy() {
    let shared_root = "/projects/shared";
    let sessions = vec![
        session(
            "healthy",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            "/projects/healthy",
            Some("/projects/healthy"),
        ),
        session(
            "idle",
            Provider::ClaudeCode,
            SessionState::Idle,
            40,
            "/projects/idle",
            Some("/projects/idle"),
        ),
        session(
            "warning",
            Provider::ClaudeCode,
            SessionState::Busy,
            75,
            "/projects/warning",
            Some("/projects/warning"),
        ),
        session(
            "shared-claude",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            shared_root,
            Some(shared_root),
        ),
        session(
            "shared-codex",
            Provider::CodexCli,
            SessionState::Idle,
            40,
            shared_root,
            Some(shared_root),
        ),
        session(
            "critical",
            Provider::CodexCli,
            SessionState::Busy,
            90,
            "/projects/critical",
            Some("/projects/critical"),
        ),
    ];

    let attention = AttentionEngine::default().update(&sessions, 2_000);
    let ordered_ids = attention
        .ordered_open_sessions(&sessions)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ordered_ids,
        [
            "critical",
            "shared-claude",
            "shared-codex",
            "warning",
            "idle",
            "healthy",
        ]
    );
}

#[test]
fn shared_worktree_risk_identifies_the_worktree_and_every_contributor() {
    let shared_root = "/projects/shared";
    let sessions = vec![
        session(
            "codex-session",
            Provider::CodexCli,
            SessionState::Idle,
            40,
            shared_root,
            Some(shared_root),
        ),
        session(
            "claude-session",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            shared_root,
            Some(shared_root),
        ),
    ];

    let attention = AttentionEngine::default().update(&sessions, 2_000);
    let [risk] = attention.shared_worktree_risks() else {
        panic!("expected exactly one Shared Worktree Risk");
    };

    assert_eq!(risk.worktree_root, PathBuf::from(shared_root));
    assert_eq!(risk.severity, AttentionSeverity::Warning);
    assert_eq!(
        risk.contributing_sessions
            .iter()
            .map(|identity| (identity.provider, identity.native_id.as_str()))
            .collect::<Vec<_>>(),
        [
            (Provider::ClaudeCode, "claude-session"),
            (Provider::CodexCli, "codex-session"),
        ]
    );
}

#[test]
fn shared_worktree_risk_clears_when_only_one_open_session_remains() {
    let shared_root = "/projects/shared";
    let sessions = vec![
        session(
            "claude-session",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            shared_root,
            Some(shared_root),
        ),
        session(
            "codex-session",
            Provider::CodexCli,
            SessionState::Idle,
            40,
            shared_root,
            Some(shared_root),
        ),
    ];
    let mut engine = AttentionEngine::default();

    assert_eq!(
        engine
            .update(&sessions, 2_000)
            .shared_worktree_risks()
            .len(),
        1
    );
    assert!(engine
        .update(&sessions[..1], 3_000)
        .shared_worktree_risks()
        .is_empty());
}

#[test]
fn shared_worktree_risk_requires_distinct_open_sessions_on_one_writable_resolved_worktree() {
    let duplicate = session(
        "same-native-id",
        Provider::ClaudeCode,
        SessionState::Busy,
        40,
        "/projects/duplicate",
        Some("/projects/duplicate"),
    );
    let mut exited = session(
        "exited",
        Provider::CodexCli,
        SessionState::Exited,
        40,
        "/projects/exited",
        Some("/projects/exited"),
    );
    exited.last_activity_ms = 2_000;
    let mut read_only_a = session(
        "read-only-a",
        Provider::ClaudeCode,
        SessionState::Busy,
        40,
        "/projects/read-only",
        Some("/projects/read-only"),
    );
    let mut read_only_b = session(
        "read-only-b",
        Provider::CodexCli,
        SessionState::Idle,
        40,
        "/projects/read-only",
        Some("/projects/read-only"),
    );
    read_only_a.worktree.as_mut().unwrap().writable = false;
    read_only_b.worktree.as_mut().unwrap().writable = false;

    let sessions = vec![
        duplicate.clone(),
        duplicate,
        session(
            "only-open",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            "/projects/exited",
            Some("/projects/exited"),
        ),
        exited,
        session(
            "unresolved-a",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            "/projects/unresolved-a",
            None,
        ),
        session(
            "unresolved-b",
            Provider::CodexCli,
            SessionState::Idle,
            40,
            "/projects/unresolved-b",
            None,
        ),
        read_only_a,
        read_only_b,
        session(
            "worktree-a",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            "/projects/one-project",
            Some("/projects/one-project/a"),
        ),
        session(
            "worktree-b",
            Provider::CodexCli,
            SessionState::Idle,
            40,
            "/projects/one-project",
            Some("/projects/one-project/b"),
        ),
    ];

    let attention = AttentionEngine::default().update(&sessions, 3_000);

    assert!(attention.shared_worktree_risks().is_empty());
}

#[test]
fn shared_worktree_risk_does_not_attach_to_a_non_contributing_session() {
    let shared_root = "/projects/mixed-writability";
    let writable_a = session(
        "writable-a",
        Provider::ClaudeCode,
        SessionState::Busy,
        40,
        shared_root,
        Some(shared_root),
    );
    let writable_b = session(
        "writable-b",
        Provider::CodexCli,
        SessionState::Busy,
        40,
        shared_root,
        Some(shared_root),
    );
    let mut read_only = session(
        "read-only",
        Provider::ClaudeCode,
        SessionState::Busy,
        40,
        shared_root,
        Some(shared_root),
    );
    read_only.worktree.as_mut().unwrap().writable = false;
    let sessions = vec![writable_a, writable_b, read_only.clone()];

    let attention = AttentionEngine::default().update(&sessions, 3_000);

    assert_eq!(attention.rank_for(&read_only), AttentionRank::HealthyBusy);
    assert!(!attention
        .conditions_for(&read_only)
        .iter()
        .any(|condition| matches!(condition, AttentionCondition::SharedWorktreeRisk { .. })));
}

#[test]
fn idle_condition_keeps_the_observed_busy_to_idle_transition_time() {
    let mut current = session(
        "transitioning",
        Provider::ClaudeCode,
        SessionState::Busy,
        40,
        "/projects/transitioning",
        Some("/projects/transitioning"),
    );
    current.last_activity_ms = 500;
    let mut engine = AttentionEngine::default();
    engine.update(std::slice::from_ref(&current), 1_000);

    current.state = SessionState::Idle;
    engine.update(std::slice::from_ref(&current), 4_000);
    let later = engine.update(std::slice::from_ref(&current), 9_000);

    assert!(later
        .conditions_for(&current)
        .contains(&AttentionCondition::Idle {
            since_ms: 4_000,
            transition_observed: true,
        }));
}

#[test]
fn initial_idle_condition_falls_back_to_last_activity() {
    let mut idle = session(
        "already-idle",
        Provider::CodexCli,
        SessionState::Idle,
        40,
        "/projects/already-idle",
        Some("/projects/already-idle"),
    );
    idle.last_activity_ms = 1_234;

    let attention = AttentionEngine::default().update(std::slice::from_ref(&idle), 5_000);

    assert!(attention
        .conditions_for(&idle)
        .contains(&AttentionCondition::Idle {
            since_ms: 1_234,
            transition_observed: false,
        }));
}

#[test]
fn session_retains_every_condition_while_ranking_by_the_highest_priority() {
    let shared_root = "/projects/multiple";
    let target = session(
        "critical-idle",
        Provider::ClaudeCode,
        SessionState::Idle,
        90,
        shared_root,
        Some(shared_root),
    );
    let contributor = session(
        "healthy-busy",
        Provider::CodexCli,
        SessionState::Busy,
        40,
        shared_root,
        Some(shared_root),
    );
    let sessions = vec![target.clone(), contributor];

    let attention = AttentionEngine::default().update(&sessions, 5_000);
    let conditions = attention.conditions_for(&target);

    assert_eq!(attention.rank_for(&target), AttentionRank::CriticalContext);
    assert!(conditions.iter().any(|condition| matches!(
        condition,
        AttentionCondition::ContextAlert {
            severity: AttentionSeverity::Critical,
            threshold_percent: 85,
            ..
        }
    )));
    assert!(conditions.iter().any(|condition| matches!(
        condition,
        AttentionCondition::SharedWorktreeRisk {
            contributing_sessions: 2,
            ..
        }
    )));
    assert!(conditions
        .iter()
        .any(|condition| matches!(condition, AttentionCondition::Idle { .. })));
}

#[test]
fn context_conditions_expose_exact_percentage_threshold_and_severity() {
    let warning = session(
        "warning",
        Provider::ClaudeCode,
        SessionState::Busy,
        75,
        "/projects/warning",
        Some("/projects/warning"),
    );
    let critical = session(
        "critical",
        Provider::CodexCli,
        SessionState::Busy,
        90,
        "/projects/critical",
        Some("/projects/critical"),
    );
    let sessions = vec![warning.clone(), critical.clone()];

    let attention = AttentionEngine::default().update(&sessions, 5_000);

    assert!(attention
        .conditions_for(&warning)
        .contains(&AttentionCondition::ContextAlert {
            severity: AttentionSeverity::Warning,
            percentage: 75.0,
            threshold_percent: 70,
            since_ms: 1_000,
        }));
    assert!(attention
        .conditions_for(&critical)
        .contains(&AttentionCondition::ContextAlert {
            severity: AttentionSeverity::Critical,
            percentage: 90.0,
            threshold_percent: 85,
            since_ms: 1_000,
        }));
}

#[test]
fn warning_ties_use_strength_recency_paths_provider_then_native_id() {
    let warning = |id, provider, percent, last_activity_ms, project, worktree| {
        let mut value = session(
            id,
            provider,
            SessionState::Busy,
            percent,
            project,
            Some(worktree),
        );
        value.last_activity_ms = last_activity_ms;
        value.worktree.as_mut().unwrap().writable = false;
        value
    };
    let sessions = vec![
        warning(
            "project-b",
            Provider::ClaudeCode,
            75,
            2_000,
            "/projects/b",
            "/worktrees/a",
        ),
        warning(
            "codex-a",
            Provider::CodexCli,
            75,
            2_000,
            "/projects/a",
            "/worktrees/a",
        ),
        warning(
            "stronger",
            Provider::CodexCli,
            80,
            1_000,
            "/projects/z",
            "/worktrees/z",
        ),
        warning(
            "worktree-b",
            Provider::ClaudeCode,
            75,
            2_000,
            "/projects/a",
            "/worktrees/b",
        ),
        warning(
            "claude-b",
            Provider::ClaudeCode,
            75,
            2_000,
            "/projects/a",
            "/worktrees/a",
        ),
        warning(
            "recent",
            Provider::CodexCli,
            75,
            3_000,
            "/projects/z",
            "/worktrees/z",
        ),
        warning(
            "claude-a",
            Provider::ClaudeCode,
            75,
            2_000,
            "/projects/a",
            "/worktrees/a",
        ),
    ];

    let attention = AttentionEngine::default().update(&sessions, 5_000);
    let ordered_ids = attention
        .ordered_open_sessions(&sessions)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ordered_ids,
        [
            "stronger",
            "recent",
            "claude-a",
            "claude-b",
            "codex-a",
            "worktree-b",
            "project-b",
        ]
    );
}

#[test]
fn shared_worktree_risk_ties_use_contributor_count_first() {
    let sessions = vec![
        session(
            "two-claude",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            "/projects/a-two",
            Some("/projects/a-two"),
        ),
        session(
            "three-codex",
            Provider::CodexCli,
            SessionState::Busy,
            40,
            "/projects/z-three",
            Some("/projects/z-three"),
        ),
        session(
            "three-claude-b",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            "/projects/z-three",
            Some("/projects/z-three"),
        ),
        session(
            "two-codex",
            Provider::CodexCli,
            SessionState::Busy,
            40,
            "/projects/a-two",
            Some("/projects/a-two"),
        ),
        session(
            "three-claude-a",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            "/projects/z-three",
            Some("/projects/z-three"),
        ),
    ];

    let attention = AttentionEngine::default().update(&sessions, 5_000);
    let ordered_ids = attention
        .ordered_open_sessions(&sessions)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ordered_ids,
        [
            "three-claude-a",
            "three-claude-b",
            "three-codex",
            "two-claude",
            "two-codex",
        ]
    );
}

#[test]
fn idle_ties_use_the_newest_idle_transition_first() {
    let mut older = session(
        "older-idle",
        Provider::ClaudeCode,
        SessionState::Busy,
        40,
        "/projects/a-older",
        Some("/projects/a-older"),
    );
    let mut newer = session(
        "newer-idle",
        Provider::CodexCli,
        SessionState::Busy,
        40,
        "/projects/z-newer",
        Some("/projects/z-newer"),
    );
    let mut engine = AttentionEngine::default();
    engine.update(&[older.clone(), newer.clone()], 1_000);

    older.state = SessionState::Idle;
    engine.update(&[older.clone(), newer.clone()], 2_000);
    newer.state = SessionState::Idle;
    let sessions = vec![older, newer];
    let attention = engine.update(&sessions, 3_000);
    let ordered_ids = attention
        .ordered_open_sessions(&sessions)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ordered_ids, ["newer-idle", "older-idle"]);
}

#[test]
fn healthy_busy_ties_use_last_activity_first() {
    let mut older = session(
        "older-busy",
        Provider::ClaudeCode,
        SessionState::Busy,
        40,
        "/projects/a-older",
        Some("/projects/a-older"),
    );
    older.last_activity_ms = 1_000;
    let mut newer = session(
        "newer-busy",
        Provider::CodexCli,
        SessionState::Busy,
        40,
        "/projects/z-newer",
        Some("/projects/z-newer"),
    );
    newer.last_activity_ms = 2_000;
    let sessions = vec![older, newer];

    let attention = AttentionEngine::default().update(&sessions, 5_000);
    let ordered_ids = attention
        .ordered_open_sessions(&sessions)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ordered_ids, ["newer-busy", "older-busy"]);
}

#[test]
fn ordering_is_deterministic_when_snapshot_input_order_changes() {
    let shared_root = "/projects/shared";
    let sessions = vec![
        session(
            "healthy",
            Provider::CodexCli,
            SessionState::Busy,
            40,
            "/projects/z-healthy",
            Some("/projects/z-healthy"),
        ),
        session(
            "shared-codex",
            Provider::CodexCli,
            SessionState::Idle,
            40,
            shared_root,
            Some(shared_root),
        ),
        session(
            "critical",
            Provider::ClaudeCode,
            SessionState::Busy,
            90,
            "/projects/critical",
            Some("/projects/critical"),
        ),
        session(
            "shared-claude",
            Provider::ClaudeCode,
            SessionState::Busy,
            40,
            shared_root,
            Some(shared_root),
        ),
    ];
    let mut reversed = sessions.clone();
    reversed.reverse();

    let forward_attention = AttentionEngine::default().update(&sessions, 5_000);
    let reverse_attention = AttentionEngine::default().update(&reversed, 5_000);
    let forward = forward_attention
        .ordered_open_sessions(&sessions)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();
    let reverse = reverse_attention
        .ordered_open_sessions(&reversed)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(forward, reverse);
}
