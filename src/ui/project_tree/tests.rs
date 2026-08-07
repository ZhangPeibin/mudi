use super::model::{rows, ProjectTreeKey, ProjectTreeRow, ProjectTreeSection};
use super::view::{draw, group_prefix, open_session_line, worktree_line, SessionTreePosition};
use super::State;
use crate::app::Snapshot;
use crate::attention::{AttentionEngine, AttentionSnapshot};
use crate::domain::{
    ContextState, Project, ProjectIdentity, Provider, Session, SessionState, SourceMetadata, Usage,
    Worktree,
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
        .draw(|frame| draw(frame, frame.area(), &snapshot, &state, &labels, true, theme))
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
fn tree_rows_use_disclosure_triangles_and_provider_icons() {
    assert_eq!(group_prefix(0, false, true), " ▾ ");
    assert_eq!(group_prefix(0, false, false), " ▸ ");

    let session = unresolved_open_session(Provider::ClaudeCode, "one");
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
        ThemeName::SessionMesh.palette(),
    )
    .to_string();

    assert!(line.contains("✦"));
    assert!(line.contains("●"));
    assert!(!line.contains("Claude"));
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

    for marker in ['!', '●', '%'] {
        assert_eq!(column(&warning, marker), column(&critical, marker));
    }
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

#[test]
fn projects_are_sorted_by_their_newest_session_activity() {
    let mut older = open_session_in_worktree("older", "/older", "main");
    older.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/older")),
        label: "older".into(),
    });
    older.last_activity_ms = 100;
    let mut newer = open_session_in_worktree("newer", "/newer", "main");
    newer.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/newer")),
        label: "newer".into(),
    });
    newer.last_activity_ms = 200;

    let sessions = [older, newer];
    let tree = rows(
        &sessions,
        &AttentionSnapshot::default(),
        300,
        &HashSet::new(),
        &HashSet::new(),
    );
    let projects = tree
        .iter()
        .filter_map(|row| match row {
            ProjectTreeRow::Project { project, .. } => Some(project.label.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(projects, ["newer", "older"]);
}

#[test]
fn inactive_projects_older_than_seven_days_are_collapsed_together() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let mut old = open_session_in_worktree("old", "/old", "main");
    old.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/old")),
        label: "old".into(),
    });
    old.state = SessionState::Exited;
    old.last_activity_ms = NOW - 8 * DAY_MS;

    let sessions = [old];
    let collapsed = rows(
        &sessions,
        &AttentionSnapshot::default(),
        NOW,
        &HashSet::new(),
        &HashSet::new(),
    );
    assert!(collapsed.iter().any(|row| matches!(
        row,
        ProjectTreeRow::Section {
            section: ProjectTreeSection::OlderProjects,
            item_count: 1,
            expanded: false,
            ..
        }
    )));
    assert!(!collapsed
        .iter()
        .any(|row| matches!(row, ProjectTreeRow::Project { .. })));

    let expanded_key = ProjectTreeKey::Section(ProjectTreeSection::OlderProjects);
    let expanded = rows(
        &sessions,
        &AttentionSnapshot::default(),
        NOW,
        &HashSet::new(),
        &HashSet::from([expanded_key]),
    );
    assert!(expanded.iter().any(|row| matches!(
        row,
        ProjectTreeRow::Project { project, depth: 1, .. } if project.label == "old"
    )));
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
    assert!(!tree.iter().any(|row| matches!(
        row,
        ProjectTreeRow::Section {
            section: ProjectTreeSection::OlderProjects,
            ..
        }
    )));
}
