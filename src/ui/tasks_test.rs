use super::*;
use crate::tasks::OperationTaskState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::PathBuf;

fn task(state: OperationTaskState) -> OperationTask {
    OperationTask {
        id: "task-test".into(),
        state,
        source_provider: Provider::ClaudeCode,
        source_session_id: "source-session".into(),
        source_record_path: PathBuf::from("/private/source.jsonl"),
        source_working_directory: PathBuf::from("/private/project"),
        successor_provider: Provider::CodexCli,
        successor_executable: PathBuf::from("/usr/local/bin/codex"),
        created_at_ms: 1_000,
        state_changed_at_ms: 2_000,
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

fn render(width: u16, tasks: &[OperationTask]) -> String {
    let backend = TestBackend::new(width, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    let ui = Ui::default();
    terminal
        .draw(|frame| draw_panel(frame, frame.area(), tasks, 3_000, &ui))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn compact_panel_prioritizes_task_identity_and_state() {
    let rendered = render(27, &[task(OperationTaskState::Queued)]);

    assert!(rendered.contains("CL:"));
    assert!(rendered.contains("Queued"));
    assert!(!rendered.contains("Handoff"));
}

#[test]
fn wide_panel_keeps_the_handoff_label() {
    let rendered = render(40, &[task(OperationTaskState::Ready)]);

    assert!(rendered.contains("NEW Handoff CL:"));
    assert!(rendered.contains("Ready"));
}
