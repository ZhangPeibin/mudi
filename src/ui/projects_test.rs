use super::*;
use crate::domain::{ContextState, SourceMetadata, Usage};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

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

#[test]
fn empty_projects_panel_explains_that_no_open_sessions_exist() {
    let backend = TestBackend::new(80, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let ui = Ui::default();

    terminal
        .draw(|frame| draw(frame, frame.area(), &snapshot, &ui))
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("SESSION TREE 0"));
    assert!(rendered.contains("No Open Sessions."));
}

#[test]
fn compact_column_widths_preserve_title_context_and_model_evidence() {
    let widths = column_widths(80, 78);

    assert_eq!(widths.title, MIN_TITLE_WIDTH);
    assert_eq!(widths.context_bar, MIN_CONTEXT_BAR_WIDTH);
    assert_eq!(widths.model, MIN_MODEL_WIDTH);
}

#[test]
fn unresolved_sessions_share_one_project_node_but_keep_provider_scoped_identities() {
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

    assert_eq!(
        tree.iter()
            .filter(|row| matches!(row, ProjectTreeRow::Project { .. }))
            .count(),
        1
    );
    let session_keys = tree
        .iter()
        .filter_map(ProjectTreeRow::session_key)
        .collect::<HashSet<_>>();
    assert_eq!(session_keys.len(), 2);
}
