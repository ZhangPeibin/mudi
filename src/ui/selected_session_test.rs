use super::*;
use crate::attention::AttentionEngine;
use crate::domain::{
    ContextState, Project, ProjectIdentity, Provider, SessionIdentity, SessionState,
    SourceMetadata, Usage, UsageSummary,
};
use crate::tasks::OperationTaskState;
use std::path::PathBuf;

fn session() -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: "selected".into(),
        source: SourceMetadata::default(),
        project: Some(Project {
            identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/Users/test/code/sessionmesh")),
            label: "sessionmesh".into(),
        }),
        worktree: Some(crate::domain::Worktree {
            root: PathBuf::from("/Users/test/code/sessionmesh"),
            branch: Some("main".into()),
            writable: true,
        }),
        cwd: Some("/Users/test/code/sessionmesh".into()),
        latest_model: Some("test-model".into()),
        state: SessionState::Busy,
        context: ContextState {
            tokens: 100,
            window: Some(200),
        },
        usage: Usage::default(),
        per_model: vec![],
        current_run_usage: Some(UsageSummary {
            usage: Usage {
                input: 1_200,
                output: 230,
                cache_read: 3_400,
                cache_write: 45,
            },
            per_model: vec![],
            cost: None,
        }),
        compactions: vec![],
        last_activity_ms: 0,
        cost: None,
    }
}

fn task(state: OperationTaskState, changed_at: i64) -> OperationTask {
    OperationTask {
        id: format!("task-{changed_at}"),
        state,
        source_provider: Provider::ClaudeCode,
        source_session_id: "selected".into(),
        source_record_path: PathBuf::new(),
        source_working_directory: PathBuf::new(),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::new(),
        created_at_ms: changed_at,
        state_changed_at_ms: changed_at,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: None,
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: None,
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    }
}

fn snapshot_with_attention(sessions: Vec<Session>, observed_at_ms: i64) -> Snapshot {
    let attention = AttentionEngine::default().update(&sessions, observed_at_ms);
    Snapshot {
        sessions,
        attention: Some(attention),
        generated_ms: observed_at_ms,
        ..Snapshot::default()
    }
}

fn ui_selecting(session: &Session) -> Ui {
    let mut ui = Ui::default();
    ui.project_tree.select_session(SessionIdentity {
        provider: session.provider,
        native_id: session.id.clone(),
    });
    ui
}

fn attention_line(snap: &Snapshot, ui: &Ui) -> Option<Line<'static>> {
    detail_lines(snap, ui, 80)
        .into_iter()
        .find(|line| line.to_string().starts_with("Attention"))
}

#[test]
fn preferred_height_is_the_visible_rows_plus_the_heading_rule() {
    let snapshot = Snapshot {
        sessions: vec![session()],
        ..Snapshot::default()
    };
    let ui = Ui::default();

    assert_eq!(detail_lines(&snapshot, &ui, 60).len(), 7);
    assert_eq!(preferred_height(&snapshot, &ui, 61), 8);
    assert_eq!(detail_lines(&snapshot, &ui, 35).len(), 6);
    assert_eq!(preferred_height(&snapshot, &ui, 36), 7);
    assert_eq!(preferred_height(&Snapshot::default(), &ui, 60), 2);

    let mut selected = session();
    selected.context = ContextState {
        tokens: 90,
        window: Some(100),
    };
    let ui = ui_selecting(&selected);
    let mut configured = snapshot_with_attention(vec![selected], 1_000);
    configured.tasks = vec![task(OperationTaskState::Running, 1_000)];
    let configured_lines = detail_lines(&configured, &ui, 35);

    for required in ["Title", "Description", "Context", "Attention", "Task"] {
        assert!(configured_lines
            .iter()
            .any(|line| line.to_string().starts_with(required)));
    }
    assert_eq!(configured_lines.len(), 8);
    assert_eq!(preferred_height(&configured, &ui, 36), 9);
}

#[test]
fn compact_details_render_the_first_field_directly_below_the_rule() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let snapshot = Snapshot {
        sessions: vec![session()],
        ..Snapshot::default()
    };
    let ui = Ui::default();
    let height = preferred_height(&snapshot, &ui, 60);
    let backend = TestBackend::new(60, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| draw(frame, frame.area(), &snapshot, &ui))
        .unwrap();
    let rows = terminal
        .backend()
        .buffer()
        .content()
        .chunks(60)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>();

    assert!(rows[0].contains("DETAILS"));
    assert!(rows[1].starts_with(" Title"), "{:?}", rows[1]);
    assert!(rows.iter().any(|row| row.contains("Run usage")));
}

#[test]
fn compact_detail_fields_use_normal_weight_with_semantic_colors() {
    let mut selected = session();
    selected.context = ContextState {
        tokens: 90,
        window: Some(100),
    };
    let ui = ui_selecting(&selected);
    let snapshot = snapshot_with_attention(vec![selected], 1_000);
    let lines = detail_lines(&snapshot, &ui, 60);

    assert!(lines
        .iter()
        .flat_map(|line| &line.spans)
        .all(|span| !span.style.add_modifier.contains(Modifier::BOLD)));
    let attention = lines
        .iter()
        .find(|line| line.to_string().starts_with("Attention"))
        .unwrap();
    assert_eq!(
        attention.spans[1].style.fg,
        Some(ui.theme.palette().critical)
    );
}

#[test]
fn details_only_show_facts_not_already_visible_in_the_tree() {
    let session = session();
    let snapshot = Snapshot {
        sessions: vec![session],
        ..Snapshot::default()
    };
    let rendered = detail_lines(&snapshot, &Ui::default(), 60)
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    for label in [
        "Title",
        "Description",
        "Provider",
        "Worktree",
        "Model",
        "Context",
        "Run usage",
    ] {
        assert!(rendered.contains(label), "missing {label}");
    }
    for duplicate in ["Project", "Lifecycle"] {
        assert!(!rendered.contains(duplicate), "repeated {duplicate}");
    }
    assert!(rendered.contains("Description N/A"));
    assert!(!rendered.contains("Attention"));
    assert!(!rendered.contains("Task"));
}

#[test]
fn compact_details_prioritize_tree_height_over_usage_summary() {
    let session = session();
    let snapshot = Snapshot {
        sessions: vec![session],
        ..Snapshot::default()
    };
    let rendered = detail_lines(&snapshot, &Ui::default(), 36)
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Provider"));
    assert!(rendered.contains("Worktree"));
    assert!(rendered.contains("Model"));
    assert!(!rendered.contains("Run"));
}

#[test]
fn details_render_shared_context_bar_and_degrade_to_percentage_when_narrow() {
    let mut session = session();
    session.context = ContextState {
        tokens: 699,
        window: Some(1_000),
    };
    let snapshot = Snapshot {
        sessions: vec![session],
        ..Snapshot::default()
    };
    let ui = Ui::default();
    let wide = detail_lines(&snapshot, &ui, 60)
        .into_iter()
        .find(|line| line.to_string().starts_with("Context"))
        .expect("Current Context should render");
    let narrow = detail_lines(&snapshot, &ui, 20)
        .into_iter()
        .find(|line| line.to_string().starts_with("Context"))
        .expect("Current Context should render");

    assert!(wide.to_string().contains("["));
    assert!(wide.to_string().contains("69.9%"));
    assert_eq!(
        wide.spans.last().unwrap().style.fg,
        Some(ui.theme.palette().context_normal)
    );
    assert!(!narrow.to_string().contains('['));
    assert!(narrow.to_string().contains("69.9%"));
}

#[test]
fn details_render_saved_metadata_and_truncate_description_without_changing_it() {
    let session = session();
    let snapshot = Snapshot {
        sessions: vec![session],
        ..Snapshot::default()
    };
    let description = "A deliberately long Unicode description for compact DETAILS 登录修复";
    let mut ui = Ui::default();
    ui.labels
        .set_metadata(
            Provider::ClaudeCode,
            "selected",
            "Readable title",
            description,
        )
        .unwrap();

    let rendered = detail_lines(&snapshot, &ui, 34)
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Readable title"));
    assert!(rendered.contains("Description"));
    assert!(!rendered.contains(description));
    assert_eq!(
        ui.labels.description(Provider::ClaudeCode, "selected"),
        Some(description)
    );
}

#[test]
fn details_style_an_empty_description_as_muted_na() {
    let snapshot = Snapshot {
        sessions: vec![session()],
        ..Snapshot::default()
    };
    let ui = Ui::default();
    let lines = detail_lines(&snapshot, &ui, 60);
    let description = lines
        .iter()
        .find(|line| line.to_string().starts_with("Description"))
        .expect("Description row should render");

    assert_eq!(description.spans[1].content.trim_end(), "N/A");
    assert_eq!(
        description.spans[1].style.fg,
        Some(ui.theme.palette().muted)
    );
}

#[test]
fn details_explain_one_critical_context_marker() {
    let mut selected = session();
    selected.context = ContextState {
        tokens: 90,
        window: Some(100),
    };
    let ui = ui_selecting(&selected);
    let snapshot = snapshot_with_attention(vec![selected], 1_000);
    let line = attention_line(&snapshot, &ui).expect("marker should have an explanation");

    assert_eq!(
        line.to_string().trim_end(),
        "Attention   Critical Context · 90%"
    );
    assert_eq!(line.spans[1].style.fg, Some(ui.theme.palette().critical));
}

#[test]
fn details_count_only_additional_warning_or_critical_causes() {
    let mut selected = session();
    selected.state = SessionState::Idle;
    selected.context = ContextState {
        tokens: 90,
        window: Some(100),
    };
    let mut peer = selected.clone();
    peer.id = "peer".into();
    peer.state = SessionState::Busy;
    peer.context = ContextState {
        tokens: 10,
        window: Some(100),
    };
    let ui = ui_selecting(&selected);
    let snapshot = snapshot_with_attention(vec![selected, peer], 1_000);
    let line = attention_line(&snapshot, &ui).expect("marker should have an explanation");

    assert_eq!(
        line.to_string().trim_end(),
        "Attention   Critical Context · 90% · +1"
    );

    let compact = detail_lines(&snapshot, &ui, 30)
        .into_iter()
        .find(|line| line.to_string().starts_with("Attention"))
        .expect("compact marker should keep its explanation");
    assert_eq!(
        compact.to_string().trim_end(),
        "Attention   Crit Ctx · 90% +1"
    );
}

#[test]
fn details_remove_attention_when_refresh_clears_the_condition() {
    let mut selected = session();
    selected.context = ContextState {
        tokens: 75,
        window: Some(100),
    };
    let ui = ui_selecting(&selected);
    let mut engine = AttentionEngine::default();
    let initial = Snapshot {
        attention: Some(engine.update(std::slice::from_ref(&selected), 1_000)),
        sessions: vec![selected.clone()],
        generated_ms: 1_000,
        ..Snapshot::default()
    };
    assert_eq!(
        attention_line(&initial, &ui)
            .unwrap()
            .to_string()
            .trim_end(),
        "Attention   Warning Context · 75%"
    );

    selected.context.tokens = 50;
    let refreshed = Snapshot {
        attention: Some(engine.update(std::slice::from_ref(&selected), 2_000)),
        sessions: vec![selected],
        generated_ms: 2_000,
        ..Snapshot::default()
    };

    assert!(attention_line(&refreshed, &ui).is_none());
    assert_eq!(ui.editor_session(&refreshed).unwrap().id, "selected");
}

#[test]
fn details_explain_shared_worktree_risk_with_contributor_evidence() {
    let mut selected = session();
    selected.context = ContextState {
        tokens: 75,
        window: Some(100),
    };
    let mut peer = selected.clone();
    peer.id = "peer".into();
    peer.context = ContextState {
        tokens: 10,
        window: Some(100),
    };
    let ui = ui_selecting(&selected);
    let snapshot = snapshot_with_attention(vec![selected, peer], 1_000);
    let line = attention_line(&snapshot, &ui).expect("marker should have an explanation");

    assert_eq!(
        line.to_string().trim_end(),
        "Attention   Shared Worktree Risk · 2 Sessions · +1"
    );
    assert_eq!(line.spans[1].style.fg, Some(ui.theme.palette().warning));
}

#[test]
fn non_terminal_task_takes_priority_over_later_terminal_task() {
    let session = session();
    let tasks = vec![
        task(OperationTaskState::Running, 10),
        task(OperationTaskState::Ready, 20),
    ];

    assert_eq!(
        task_for_session(&session, &tasks).unwrap().state,
        OperationTaskState::Running
    );
}

#[test]
fn latest_terminal_task_is_used_when_no_task_is_running() {
    let session = session();
    let tasks = vec![
        task(OperationTaskState::Failed, 10),
        task(OperationTaskState::Ready, 20),
    ];

    assert_eq!(
        task_for_session(&session, &tasks).unwrap().state,
        OperationTaskState::Ready
    );
}
