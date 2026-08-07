use super::*;
use crate::domain::{ContextState, Provider, SessionState, SourceMetadata, Usage, UsageSummary};
use crate::tasks::OperationTaskState;
use std::path::PathBuf;

fn session() -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: "selected".into(),
        source: SourceMetadata::default(),
        project: None,
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

    for label in ["Provider", "Worktree", "Model", "Run usage"] {
        assert!(rendered.contains(label), "missing {label}");
    }
    for duplicate in ["Session Label", "Project", "Current Context", "Lifecycle"] {
        assert!(!rendered.contains(duplicate), "repeated {duplicate}");
    }
    assert!(!rendered.contains("N/A"));
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
