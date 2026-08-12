use super::model::{
    rows, rows_with_order, tree_counts, ProjectOrder, ProjectTreeKey, ProjectTreeRow,
};
use super::view::{draw, group_prefix, open_session_line, worktree_line, SessionTreePosition};
use super::State;
use crate::app::Snapshot;
use crate::attention::{AttentionEngine, AttentionSnapshot};
use crate::domain::{
    ContextState, Project, ProjectIdentity, Provider, Session, SessionProvenance, SessionState,
    SourceMetadata, Usage, Worktree,
};
use crate::session_labels::SessionLabels;
use crate::ui::ThemeName;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::HashSet;
use std::path::PathBuf;

fn unresolved_open_session(provider: Provider, id: &str) -> Session {
    Session {
        provider,
        id: id.into(),
        provenance: crate::domain::SessionProvenance::Observed,
        source: SourceMetadata::default(),
        project: None,
        worktree: None,
        cwd: None,
        latest_model: Some("test-model".into()),
        state: SessionState::Busy,
        context: ContextState::default(),
        usage: Usage::default(),
        per_model: vec![],
        current_run_usage: None,
        compactions: vec![],
        last_activity_ms: 1_000,
        cost: None,
    }
}

fn open_session_in_worktree(id: &str, root: &str, branch: &str) -> Session {
    let mut session = unresolved_open_session(Provider::ClaudeCode, id);
    session.project = Some(Project {
        identity: ProjectIdentity::GitCommonDir(PathBuf::from("/repo/.git")),
        label: "repo".into(),
    });
    session.worktree = Some(Worktree {
        root: PathBuf::from(root),
        branch: Some(branch.into()),
        writable: true,
    });
    session.cwd = Some(root.into());
    session
}

#[test]
fn empty_tree_content_explains_that_no_open_sessions_exist() {
    let backend = TestBackend::new(80, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let state = State::default();
    let labels = SessionLabels::default();
    let theme = ThemeName::SessionMesh.palette();

    terminal
        .draw(|frame| draw(frame, frame.area(), &snapshot, &state, &labels, theme))
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("No Open Sessions."));
}

#[test]
fn single_worktree_is_flattened_beneath_its_project() {
    let sessions = vec![open_session_in_worktree("one", "/repo", "main")];
    let tree = rows(
        &sessions,
        &AttentionSnapshot::default(),
        1_000,
        &HashSet::new(),
        &HashSet::new(),
    );

    assert!(!tree
        .iter()
        .any(|row| matches!(row, ProjectTreeRow::Worktree { .. })));
    let session = tree
        .iter()
        .find(|row| matches!(row, ProjectTreeRow::Session { .. }))
        .unwrap();
    assert_eq!(session.depth(), 1);
    assert!(matches!(
        session.parent_key(),
        Some(ProjectTreeKey::Project(_))
    ));
}

#[test]
fn multiple_worktrees_keep_short_branch_groups() {
    let sessions = vec![
        open_session_in_worktree("main", "/repo", "main"),
        open_session_in_worktree("feature", "/repo-feature", "feature"),
    ];
    let tree = rows(
        &sessions,
        &AttentionSnapshot::default(),
        1_000,
        &HashSet::new(),
        &HashSet::new(),
    );

    assert_eq!(
        tree.iter()
            .filter(|row| matches!(row, ProjectTreeRow::Worktree { .. }))
            .count(),
        2
    );
    let line = worktree_line(
        sessions[0].worktree.as_ref().unwrap(),
        &AttentionSnapshot::default(),
        1,
        true,
        false,
        40,
        ThemeName::SessionMesh.palette(),
    )
    .to_string();
    assert!(line.contains("main"));
    assert!(!line.contains("/repo"));
}

#[test]
fn tree_rows_use_disclosure_triangles_provider_icons_and_lifecycle_dots() {
    assert_eq!(group_prefix(0, false, true), " ▾ ");
    assert_eq!(group_prefix(0, false, false), " ▸ ");

    let session = unresolved_open_session(Provider::ClaudeCode, "one");
    let theme = ThemeName::SessionMesh.palette();
    let line = open_session_line(
        &session,
        "one",
        &AttentionSnapshot::default(),
        1_000,
        40,
        SessionTreePosition {
            depth: 1,
            selected: true,
        },
        theme,
    );
    let mark = line.spans.iter().find(|span| span.content == "✳").unwrap();
    assert_eq!(mark.style.fg, Some(theme.provider_claude));
    let line = line.to_string();

    assert!(line.contains("✳"));
    assert!(line.contains("•"));
    assert!(!line.contains("Claude"));
}

#[test]
fn only_observed_rows_show_an_external_suffix_after_the_title() {
    let observed = unresolved_open_session(Provider::ClaudeCode, "observed");
    let mut managed = observed.clone();
    managed.id = "managed".into();
    managed.provenance = SessionProvenance::Managed;
    let theme = ThemeName::SessionMesh.palette();
    let position = SessionTreePosition {
        depth: 1,
        selected: false,
    };

    let observed_line = open_session_line(
        &observed,
        "observed",
        &AttentionSnapshot::default(),
        1_000,
        50,
        position,
        theme,
    );
    let managed_line = open_session_line(
        &managed,
        "managed",
        &AttentionSnapshot::default(),
        1_000,
        50,
        position,
        theme,
    );

    let observed_title = observed_line
        .spans
        .iter()
        .position(|span| span.content.starts_with("observed"))
        .unwrap();
    let managed_title = managed_line
        .spans
        .iter()
        .position(|span| span.content.starts_with("managed"))
        .unwrap();
    let observed_suffix = &observed_line.spans[observed_title + 1];
    let managed_suffix = &managed_line.spans[managed_title + 1];

    assert_eq!(observed_suffix.content, " ↗");
    assert_eq!(managed_suffix.content, "  ");
    assert!(!managed_line.to_string().contains('↗'));
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(observed_suffix.content.as_ref()),
        2
    );
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(managed_suffix.content.as_ref()),
        2
    );
    assert_eq!(observed_suffix.style.fg, Some(theme.muted));
}

#[test]
fn open_session_operational_evidence_uses_stable_columns() {
    const NOW: i64 = 1_700_000_000_000;
    let mut warning = unresolved_open_session(Provider::ClaudeCode, "warning");
    warning.context = ContextState {
        tokens: 150_000,
        window: Some(200_000),
    };
    warning.last_activity_ms = NOW - 30_000;
    let mut critical = warning.clone();
    critical.id = "critical".into();
    critical.context.tokens = 200_000;
    critical.last_activity_ms = NOW - 8 * 86_400_000;
    let sessions = [warning.clone(), critical.clone()];
    let attention = AttentionEngine::default().update(&sessions, NOW);
    let position = SessionTreePosition {
        depth: 1,
        selected: false,
    };
    let theme = ThemeName::SessionMesh.palette();
    let warning =
        open_session_line(&warning, "short", &attention, NOW, 50, position, theme).to_string();
    let critical = open_session_line(
        &critical,
        "a much longer title",
        &attention,
        NOW,
        50,
        position,
        theme,
    )
    .to_string();
    let column = |line: &str, marker: char| {
        line[..line.find(marker).expect("marker should render")]
            .chars()
            .count()
    };

    for marker in ['!', '•', '%'] {
        assert_eq!(column(&warning, marker), column(&critical, marker));
    }
    assert!(warning.contains("75%"));
    assert!(critical.contains("100%"));
    assert!(!warning.contains('['));
    assert!(!critical.contains('['));
}

#[test]
fn unresolved_sessions_are_excluded_from_the_project_tree() {
    let sessions = vec![
        unresolved_open_session(Provider::ClaudeCode, "same-native-id"),
        unresolved_open_session(Provider::CodexCli, "same-native-id"),
    ];
    let tree = rows(
        &sessions,
        &AttentionSnapshot::default(),
        1_000,
        &HashSet::new(),
        &HashSet::new(),
    );

    assert!(tree.is_empty());
}

fn project_labels(state: &State, snapshot: &Snapshot) -> Vec<String> {
    state
        .rows(snapshot)
        .into_iter()
        .filter_map(|row| match row {
            ProjectTreeRow::Project { project, .. } => Some(project.label.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn projects_keep_their_startup_activity_order_when_activity_changes() {
    let mut alpha = open_session_in_worktree("alpha", "/alpha", "main");
    alpha.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/alpha")),
        label: "alpha".into(),
    });
    alpha.last_activity_ms = 100;
    let mut zulu = open_session_in_worktree("zulu", "/zulu", "main");
    zulu.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/zulu")),
        label: "zulu".into(),
    });
    zulu.last_activity_ms = 200;

    let mut snapshot = Snapshot {
        sessions: vec![alpha, zulu],
        generated_ms: 300,
        ..Snapshot::default()
    };
    let state = State::default();

    assert_eq!(project_labels(&state, &snapshot), ["zulu", "alpha"]);
    snapshot.sessions[0].last_activity_ms = 300;
    snapshot.sessions[1].last_activity_ms = 50;
    assert_eq!(project_labels(&state, &snapshot), ["zulu", "alpha"]);
}

#[test]
fn newly_discovered_project_is_first_and_keeps_its_rank_after_reappearing() {
    let mut alpha = open_session_in_worktree("alpha", "/alpha", "main");
    alpha.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/alpha")),
        label: "alpha".into(),
    });
    alpha.last_activity_ms = 100;
    let mut zulu = open_session_in_worktree("zulu", "/zulu", "main");
    zulu.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/zulu")),
        label: "zulu".into(),
    });
    zulu.last_activity_ms = 200;
    let mut beta = open_session_in_worktree("beta", "/beta", "main");
    beta.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/beta")),
        label: "beta".into(),
    });
    beta.last_activity_ms = 50;

    let mut snapshot = Snapshot {
        sessions: vec![alpha, zulu],
        generated_ms: 300,
        ..Snapshot::default()
    };
    let state = State::default();
    assert_eq!(project_labels(&state, &snapshot), ["zulu", "alpha"]);

    snapshot.sessions.push(beta.clone());
    assert_eq!(project_labels(&state, &snapshot), ["beta", "zulu", "alpha"]);

    snapshot.sessions.retain(|session| session.id != beta.id);
    assert_eq!(project_labels(&state, &snapshot), ["zulu", "alpha"]);

    snapshot.sessions.push(beta);
    assert_eq!(project_labels(&state, &snapshot), ["beta", "zulu", "alpha"]);
}

#[test]
fn projects_with_matching_labels_use_canonical_path_order() {
    let mut zulu = open_session_in_worktree("zulu", "/projects/zulu", "main");
    zulu.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/projects/zulu")),
        label: "project".into(),
    });
    let mut alpha = open_session_in_worktree("alpha", "/projects/alpha", "main");
    alpha.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/projects/alpha")),
        label: "project".into(),
    });

    let paths = rows(
        &[zulu, alpha],
        &AttentionSnapshot::default(),
        300,
        &HashSet::new(),
        &HashSet::new(),
    )
    .into_iter()
    .filter_map(|row| match row {
        ProjectTreeRow::Project { project, .. } => {
            Some(project.identity.display_path().to_path_buf())
        }
        _ => None,
    })
    .collect::<Vec<_>>();

    assert_eq!(
        paths,
        [
            PathBuf::from("/projects/alpha"),
            PathBuf::from("/projects/zulu")
        ]
    );
}

#[test]
fn newly_discovered_older_project_is_not_added_to_the_tree() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let mut alpha = open_session_in_worktree("alpha", "/alpha", "main");
    alpha.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/alpha")),
        label: "alpha".into(),
    });
    alpha.state = SessionState::Exited;
    alpha.last_activity_ms = NOW - 10 * DAY_MS;
    let mut zulu = open_session_in_worktree("zulu", "/zulu", "main");
    zulu.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/zulu")),
        label: "zulu".into(),
    });
    zulu.state = SessionState::Exited;
    zulu.last_activity_ms = NOW - 8 * DAY_MS;
    let mut beta = open_session_in_worktree("beta", "/beta", "main");
    beta.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/beta")),
        label: "beta".into(),
    });
    beta.state = SessionState::Exited;
    beta.last_activity_ms = NOW - 30 * DAY_MS;

    let mut sessions = vec![alpha, zulu];
    let mut project_order = ProjectOrder::default();
    project_order.observe(&sessions);
    sessions.push(beta);
    project_order.observe(&sessions);
    let tree = rows_with_order(
        &sessions,
        &AttentionSnapshot::default(),
        NOW,
        &HashSet::new(),
        &HashSet::new(),
        &project_order,
    );
    assert!(tree.is_empty());
}

#[test]
fn project_with_only_recent_and_older_sessions_is_not_in_the_tree() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let project = Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/history")),
        label: "history-only-project".into(),
    };
    let mut recent = open_session_in_worktree("recent", "/history", "main");
    recent.project = Some(project.clone());
    recent.state = SessionState::Exited;
    recent.last_activity_ms = NOW - DAY_MS;
    let mut older = open_session_in_worktree("older", "/history", "main");
    older.project = Some(project);
    older.state = SessionState::Exited;
    older.last_activity_ms = NOW - 4 * DAY_MS;

    let snapshot = Snapshot {
        sessions: vec![recent, older],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };
    let tree = rows(
        &snapshot.sessions,
        &AttentionSnapshot::default(),
        NOW,
        &HashSet::new(),
        &HashSet::new(),
    );
    assert!(tree.is_empty());
    assert_eq!(tree_counts(&snapshot), (0, 0));

    let backend = TestBackend::new(80, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = State::default();
    let labels = SessionLabels::default();
    let theme = ThemeName::SessionMesh.palette();
    terminal
        .draw(|frame| draw(frame, frame.area(), &snapshot, &state, &labels, theme))
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("No Open Sessions."));
    assert!(!rendered.contains("history-only-project"));
}

#[test]
fn project_count_includes_only_projects_with_open_sessions() {
    let mut open = open_session_in_worktree("open", "/open", "main");
    open.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/open")),
        label: "open-project".into(),
    });
    let mut recent = open_session_in_worktree("recent", "/recent", "main");
    recent.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/recent")),
        label: "recent-only-project".into(),
    });
    recent.state = SessionState::Exited;
    let snapshot = Snapshot {
        sessions: vec![open, recent],
        generated_ms: 1_000,
        ready: true,
        ..Snapshot::default()
    };
    assert_eq!(tree_counts(&snapshot), (1, 1));
}

#[test]
fn an_open_project_remains_current_even_when_its_last_activity_is_old() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let mut open = open_session_in_worktree("open", "/open", "main");
    open.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/open")),
        label: "open".into(),
    });
    open.last_activity_ms = NOW - 8 * DAY_MS;

    let sessions = [open];
    let tree = rows(
        &sessions,
        &AttentionSnapshot::default(),
        NOW,
        &HashSet::new(),
        &HashSet::new(),
    );

    assert!(tree.iter().any(|row| matches!(
        row,
        ProjectTreeRow::Project { project, depth: 0, .. } if project.label == "open"
    )));
}
