use super::*;
use crate::app::Snapshot;
use crate::domain::{ContextState, Provider, Session, SessionState, SourceMetadata, Usage};
use crate::tasks::{OperationTask, OperationTaskState};
use std::path::PathBuf;

fn session() -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: "selected".into(),
        source: SourceMetadata::default(),
        project: None,
        worktree: None,
        cwd: None,
        latest_model: Some("test-model".into()),
        state: SessionState::Busy,
        context: ContextState {
            tokens: 100,
            window: Some(200),
        },
        usage: Usage::default(),
        per_model: vec![],
        current_run_usage: None,
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

#[test]
fn compact_inspector_names_all_navigation_facts() {
    let session = session();
    let ui = Ui::default();
    let lines = selected_session_lines(
        &session,
        &Snapshot {
            sessions: vec![session.clone()],
            ..Snapshot::default()
        },
        &ui,
        48,
    );
    let rendered = lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    for label in [
        "Session Label",
        "Provider / Lifecycle",
        "Project",
        "Worktree",
        "Latest model",
        "Current Context",
        "Current Run",
        "Operation Task",
    ] {
        assert!(rendered.contains(label), "missing {label}");
    }
}
