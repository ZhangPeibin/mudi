use super::*;
use crate::domain::{
    ContextState, Project, ProjectIdentity, Provider, Session, SessionState, SourceMetadata, Usage,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::PathBuf;

fn session() -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: "selected".into(),
        provenance: crate::domain::SessionProvenance::Observed,
        source: SourceMetadata::default(),
        project: Some(Project {
            identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/test/mudi")),
            label: "mudi".into(),
        }),
        worktree: None,
        cwd: None,
        latest_model: Some("test-model".into()),
        state: SessionState::Busy,
        context: ContextState::default(),
        usage: Usage::default(),
        per_model: vec![],
        current_run_usage: None,
        compactions: vec![],
        last_activity_ms: 0,
        cost: None,
    }
}

#[test]
fn workspace_remains_independent_from_selected_session_details() {
    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let snapshot = Snapshot {
        sessions: vec![session()],
        ..Snapshot::default()
    };
    let ui = Ui::default();

    terminal
        .draw(|frame| draw_workspace(frame, frame.area(), &snapshot, &ui))
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("Session Workspace"));
    assert!(rendered.contains("Session Claude Code / selected"));
    assert!(rendered.contains("Conversation unavailable."));
    assert!(!rendered.contains("Worktree"));
}
