//! UI 单元测试覆盖状态转换、响应式布局、语义颜色和稳定身份导航。
//!
//! 测试函数名描述预期行为；下面的辅助构造负责复用快照与 TestBackend，
//! 各测试中的断言则直接对应用户可观察的文字、位置或样式证据。

use super::*;
use crate::application::alerts::AlertCounts;
use crate::application::attention::AttentionEngine;
use crate::application::signals::{
    ContextAlertLevel, RecentSignal, RecentSignalKind, SharedProjectRiskChange,
};
use crate::domain::{
    Compaction, ContextState, ModelUsage, Project, ProjectIdentity, Provider, Session,
    SessionState, Usage, UsageSummary,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 构造具有稳定测试路径的 Project。
fn test_project(label: &str) -> Project {
    Project {
        identity: ProjectIdentity::new(PathBuf::from(format!("/test/{label}"))),
        label: label.into(),
    }
}

/// 返回不会与并行测试冲突的临时 Profile 路径。
fn unused_profile_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let data_dir = std::env::temp_dir().join(format!("mudi-ui-tests-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).unwrap();
    data_dir.join(format!(
        "mudi-{name}-{}-{nonce}-profile.json",
        std::process::id(),
    ))
}

/// 使用产品支持的最小尺寸渲染测试终端。
fn render_terminal_with_snapshot(ui: &Ui, snapshot: &Snapshot) -> Terminal<TestBackend> {
    render_terminal_at(ui, snapshot, 80, 24)
}

/// 按指定终端尺寸渲染一次快照，供位置和样式断言读取缓冲区。
fn render_terminal_at(
    ui: &Ui,
    snapshot: &Snapshot,
    width: u16,
    height: u16,
) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| draw(f, snapshot, ui)).unwrap();
    terminal
}

/// 使用空快照渲染最小测试终端。
fn render_terminal(ui: &Ui) -> Terminal<TestBackend> {
    render_terminal_with_snapshot(ui, &Snapshot::default())
}

/// 将空快照的整个终端缓冲区拼接为文本。
fn rendered(ui: &Ui) -> String {
    render_terminal(ui)
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// 将给定快照的整个终端缓冲区拼接为文本。
fn rendered_with_snapshot(ui: &Ui, snapshot: &Snapshot) -> String {
    render_terminal_with_snapshot(ui, snapshot)
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// 构造处于指定状态且不含额外执行证据的 Operation Task。
fn operation_task(state: crate::application::tasks::OperationTaskState) -> OperationTask {
    OperationTask {
        id: format!("task-{}", state.label().to_lowercase()),
        state,
        source_provider: Provider::ClaudeCode,
        source_session_id: "source-session".into(),
        source_record_path: PathBuf::from("/private/source.jsonl"),
        source_working_directory: PathBuf::from("/private/project"),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::from("/usr/local/bin/claude"),
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

#[test]
fn dismissing_cancel_confirmation_changes_no_non_terminal_task() {
    for state in [
        crate::application::tasks::OperationTaskState::Queued,
        crate::application::tasks::OperationTaskState::Waiting,
        crate::application::tasks::OperationTaskState::Running,
    ] {
        let task = operation_task(state);
        let snapshot = Snapshot {
            tasks: vec![task.clone()],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

        assert_eq!(
            ui.handle_key(key(KeyCode::Char('x')), &snapshot).unwrap(),
            UiAction::None
        );
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("Cancel Task?"));
        assert_eq!(
            ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap(),
            UiAction::None
        );
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Cancel Task?"));
        assert_eq!(snapshot.tasks, vec![task]);
    }
}

#[test]
fn confirming_cancel_emits_the_selected_non_terminal_task_identity() {
    let task = operation_task(crate::application::tasks::OperationTaskState::Running);
    let snapshot = Snapshot {
        tasks: vec![task.clone()],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::None
    );
    ui.handle_key(key(KeyCode::Char('x')), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::CancelTask { task_id: task.id }
    );
}

#[test]
fn overlay_owner_blocks_background_navigation_and_disappears_with_its_target() {
    let task = operation_task(crate::application::tasks::OperationTaskState::Running);
    let snapshot = Snapshot {
        tasks: vec![task],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('x')), &snapshot).unwrap();
    let detail_scroll = ui.overlay.detail().unwrap().scroll.get();

    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();

    assert!(ui.overlay.confirmation().is_some());
    assert_eq!(ui.overlay.detail().unwrap().scroll.get(), detail_scroll);

    ui.handle_key(key(KeyCode::Down), &Snapshot::default())
        .unwrap();

    assert!(ui.overlay.confirmation().is_none());
    assert!(ui.overlay.detail().is_none());
}

#[test]
fn guide_owns_navigation_without_moving_the_background_task_selection() {
    let mut second = operation_task(crate::application::tasks::OperationTaskState::Waiting);
    second.id = "second-task".into();
    let snapshot = Snapshot {
        tasks: vec![
            operation_task(crate::application::tasks::OperationTaskState::Queued),
            second,
        ],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    ui.handle_key(key(KeyCode::Char('?')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();

    assert_eq!(ui.task_selected, 0);
    assert_eq!(
        ui.overlay.guide().map(|guide| guide.category),
        Some(GuideCategory::Navigation)
    );
}

#[test]
fn guide_and_session_edit_restore_their_detail_underlay() {
    let snapshot = demo_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    ui.handle_key(key(KeyCode::Char('?')), &snapshot).unwrap();
    assert!(ui.overlay.guide().is_some());
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session Detail"));
    ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
    assert!(ui.overlay.detail().is_some());

    ui.handle_key(key(KeyCode::Char('e')), &snapshot).unwrap();
    assert!(ui.overlay.session_editor().is_some());
    ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
    assert!(ui.overlay.detail().is_some());
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session Detail"));
}

#[test]
fn declining_irreversible_task_deletion_preserves_the_terminal_task() {
    let mut task = operation_task(crate::application::tasks::OperationTaskState::Ready);
    let artifact_path = PathBuf::from("/private/artifacts/task-ready/handoff.md");
    task.artifact_path = Some(artifact_path.clone());
    task.successor_launch_plan = Some(crate::application::tasks::SuccessorLaunchPlan {
        successor_provider: Provider::ClaudeCode,
        working_directory: PathBuf::from("/private/project"),
        executable: PathBuf::from("/usr/local/bin/claude"),
        arguments: vec!["continue".into()],
        artifact_path,
    });
    task.acknowledged_at_ms = Some(2_500);
    let snapshot = Snapshot {
        tasks: vec![task.clone()],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Char('d')), &snapshot).unwrap(),
        UiAction::None
    );
    let confirmation = rendered_with_snapshot(&ui, &snapshot);
    assert!(confirmation.contains("Delete Task and Artifact?"));
    assert!(confirmation.contains(&task.id));
    assert!(confirmation.contains("irreversible"));
    assert!(confirmation.contains("Handoff Artifact"));
    assert!(confirmation.contains("Successor Launch Plan"));

    assert_eq!(
        ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap(),
        UiAction::None
    );
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Delete Task and Artifact?"));
    assert_eq!(snapshot.tasks, vec![task]);
}

#[test]
fn confirming_irreversible_task_deletion_emits_the_selected_task_identity() {
    let task = operation_task(crate::application::tasks::OperationTaskState::Cancelled);
    let snapshot = Snapshot {
        tasks: vec![task.clone()],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('d')), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::DeleteTask {
            task_id: task.id.clone()
        }
    );
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Delete Task and Artifact?"));
    assert_eq!(snapshot.tasks, vec![task]);
}

#[test]
fn deleting_task_detail_shows_progress_or_recoverable_failure_without_conflicting_actions() {
    let mut task = operation_task(crate::application::tasks::OperationTaskState::Ready);
    task.deletion_requested_at_ms = Some(3_000);
    let mut snapshot = Snapshot {
        tasks: vec![task.clone()],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::None
    );

    let in_progress = rendered_with_snapshot(&ui, &snapshot);
    assert!(in_progress.contains("Deletion                In progress"));
    for unavailable in ["d Delete", "r Retry", "o Open Terminal", "c Copy Command"] {
        assert!(!in_progress.contains(unavailable));
    }

    task.deletion_failure_code = Some("artifact_remove_failed".into());
    snapshot.tasks = vec![task];
    let failed = rendered_with_snapshot(&ui, &snapshot);
    assert!(failed.contains("Deletion                Failed; will retry"));
    assert!(failed.contains("Deletion failure        artifact_remove_failed"));
    assert!(!failed.contains("d Delete"));
}

#[test]
fn retained_retry_marks_its_deleted_predecessor_identity_as_unavailable() {
    let mut retry = operation_task(crate::application::tasks::OperationTaskState::Failed);
    retry.id = "retry-task".into();
    retry.retry_of_task_id = Some("deleted-predecessor-uuid".into());
    let snapshot = Snapshot {
        tasks: vec![retry],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    assert!(
        rendered_with_snapshot(&ui, &snapshot).contains("deleted-predecessor-uuid (unavailable)")
    );
}

#[test]
fn task_deletion_is_available_only_for_terminal_states() {
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    for state in [
        crate::application::tasks::OperationTaskState::Ready,
        crate::application::tasks::OperationTaskState::Failed,
        crate::application::tasks::OperationTaskState::Interrupted,
        crate::application::tasks::OperationTaskState::Cancelled,
    ] {
        let snapshot = Snapshot {
            tasks: vec![operation_task(state)],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("d Delete"));
        ui.handle_key(key(KeyCode::Char('d')), &snapshot).unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("Delete Task and Artifact?"));
    }

    for state in [
        crate::application::tasks::OperationTaskState::Queued,
        crate::application::tasks::OperationTaskState::Waiting,
        crate::application::tasks::OperationTaskState::Running,
    ] {
        let snapshot = Snapshot {
            tasks: vec![operation_task(state)],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("d Delete"));
        ui.handle_key(key(KeyCode::Char('d')), &snapshot).unwrap();
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Delete Task and Artifact?"));
    }
}

#[test]
fn task_detail_offers_retry_only_for_eligible_terminal_outcomes() {
    for state in [
        crate::application::tasks::OperationTaskState::Failed,
        crate::application::tasks::OperationTaskState::Cancelled,
    ] {
        let snapshot = Snapshot {
            tasks: vec![operation_task(state)],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("r Retry"));
    }

    let mut interrupted =
        operation_task(crate::application::tasks::OperationTaskState::Interrupted);
    let mut snapshot = Snapshot {
        tasks: vec![interrupted.clone()],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("r Retry"));

    interrupted.retry_ready_at_ms = Some(3_000);
    snapshot.tasks = vec![interrupted];
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("r Retry"));

    snapshot.tasks = vec![operation_task(
        crate::application::tasks::OperationTaskState::Ready,
    )];
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("r Retry"));
}

#[test]
fn retry_requires_fresh_confirmation_before_emitting_an_action() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].state = SessionState::Exited;
    let mut task = operation_task(crate::application::tasks::OperationTaskState::Failed);
    task.source_provider = snapshot.sessions[0].provider;
    task.source_session_id = snapshot.sessions[0].id.clone();
    snapshot.tasks = vec![task.clone()];
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Char('r')), &snapshot).unwrap(),
        UiAction::None
    );
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Authorize Retry?"));
    assert_eq!(
        ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap(),
        UiAction::None
    );

    ui.handle_key(key(KeyCode::Char('r')), &snapshot).unwrap();
    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::RetryTask {
            predecessor_task_id: task.id,
            source_provider: snapshot.sessions[0].provider,
            source_session_id: snapshot.sessions[0].id.clone(),
            successor_provider: snapshot.sessions[0].provider,
            end_current_run: false,
        }
    );
}

#[test]
fn retry_defaults_to_the_source_provider_and_emits_a_fresh_successor_choice() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].state = SessionState::Exited;
    let mut task = operation_task(crate::application::tasks::OperationTaskState::Failed);
    task.source_provider = snapshot.sessions[0].provider;
    task.source_session_id = snapshot.sessions[0].id.clone();
    task.successor_provider = Provider::CodexCli;
    snapshot.tasks = vec![task.clone()];
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('r')), &snapshot).unwrap();

    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Successor  [Claude Code]  Codex CLI"));
    ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Successor  Claude Code  [Codex CLI]"));
    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::RetryTask {
            predecessor_task_id: task.id,
            source_provider: snapshot.sessions[0].provider,
            source_session_id: snapshot.sessions[0].id.clone(),
            successor_provider: Provider::CodexCli,
            end_current_run: false,
        }
    );
}

#[test]
fn ready_task_offers_open_terminal_and_copy_command_for_the_same_result() {
    let mut task = operation_task(crate::application::tasks::OperationTaskState::Ready);
    let artifact_path = PathBuf::from("/private/artifacts/task/handoff.md");
    task.artifact_path = Some(artifact_path.clone());
    task.successor_launch_plan = Some(crate::application::tasks::SuccessorLaunchPlan {
        successor_provider: Provider::ClaudeCode,
        working_directory: PathBuf::from("/private/project"),
        executable: PathBuf::from("/usr/local/bin/claude"),
        arguments: vec!["Read the Handoff Artifact and continue.".into()],
        artifact_path,
    });
    let snapshot = Snapshot {
        tasks: vec![task.clone()],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    let detail = rendered_with_snapshot(&ui, &snapshot);
    assert!(detail.contains("o Open Terminal"));
    assert!(detail.contains("c Copy Command"));
    assert_eq!(
        ui.handle_key(key(KeyCode::Char('c')), &snapshot).unwrap(),
        UiAction::CopySuccessorCommand {
            task_id: task.id.clone()
        }
    );
    assert_eq!(
        ui.handle_key(key(KeyCode::Char('o')), &snapshot).unwrap(),
        UiAction::OpenSuccessorTerminal { task_id: task.id }
    );
}

#[test]
fn linked_successor_becomes_navigable_after_the_session_reaches_the_index() {
    let mut task = operation_task(crate::application::tasks::OperationTaskState::Ready);
    task.successor_provider = Provider::CodexCli;
    task.successor_session_id = Some("indexed-codex-successor".into());
    task.successor_linked_at_ms = Some(3_000);
    let mut snapshot = Snapshot {
        tasks: vec![task],
        ..Snapshot::default()
    };
    let mut ui = Ui {
        all_region: AllRegion::Tasks,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    let waiting_for_index = rendered_with_snapshot(&ui, &snapshot);
    assert!(waiting_for_index.contains("Successor Session       indexed-codex-successor"));
    assert!(waiting_for_index.contains("Linked, not indexed"));
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Task ID"));

    let mut successor = demo_snapshot().sessions.remove(0);
    successor.provider = Provider::CodexCli;
    successor.id = "indexed-codex-successor".into();
    snapshot.sessions.push(successor);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    let successor_detail = rendered_with_snapshot(&ui, &snapshot);
    assert!(successor_detail.contains("Session Detail"));
    assert!(successor_detail.contains("indexed-codex-successor"));
    assert!(!successor_detail.contains("Task ID"));
}

#[test]
fn non_terminal_task_detail_shows_the_authorized_cross_provider_successor() {
    for state in [
        crate::application::tasks::OperationTaskState::Queued,
        crate::application::tasks::OperationTaskState::Waiting,
        crate::application::tasks::OperationTaskState::Running,
    ] {
        let mut task = operation_task(state);
        task.successor_provider = Provider::CodexCli;
        task.successor_executable = PathBuf::from("/usr/local/bin/codex");
        let snapshot = Snapshot {
            tasks: vec![task],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(detail.contains("Successor Provider      Codex CLI"));
    }
}

/// 按行保留终端右侧空格，便于精确断言面板边界和留白。
fn rendered_rows_at(ui: &Ui, snapshot: &Snapshot, width: u16, height: u16) -> String {
    let terminal = render_terminal_at(ui, snapshot, width, height);
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn rendered_modal_height(rendered: &str, title: &str) -> usize {
    let rows = rendered.lines().collect::<Vec<_>>();
    let top = rows.iter().position(|row| row.contains(title)).unwrap();
    let modal_x = rows[top].chars().position(|cell| cell == '┌').unwrap();
    let bottom = rows
        .iter()
        .enumerate()
        .skip(top + 1)
        .find(|(_, row)| row.chars().nth(modal_x) == Some('└'))
        .map(|(index, _)| index)
        .unwrap();
    bottom - top + 1
}

/// 返回指定可见文本首个字符使用的 Ratatui 样式。
fn rendered_style_at(terminal: &Terminal<TestBackend>, text: &str) -> Style {
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    let cell_count = text.chars().count();
    for row in buffer.content().chunks(width) {
        if let Some(cells) = row
            .windows(cell_count)
            .find(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == text)
        {
            return cells[0].style();
        }
    }
    panic!("rendered text not found: {text}");
}

/// 在 Project 区定位目标 Session 行，并返回该行选择光标的样式。
fn now_cursor_style_at(terminal: &Terminal<TestBackend>, session_text: &str) -> Option<Style> {
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    let tree_width = width * 46 / 100;
    let cell_count = session_text.chars().count();
    for row in buffer
        .content()
        .chunks(width)
        .skip(1 + overview::STATUS_HEIGHT as usize)
    {
        let row = &row[..tree_width];
        let Some(session_start) = row.windows(cell_count).position(|cells| {
            cells.iter().map(|cell| cell.symbol()).collect::<String>() == session_text
        }) else {
            continue;
        };
        return row[..session_start]
            .iter()
            .find(|cell| cell.symbol() == ">")
            .map(|cell| cell.style());
    }
    panic!("rendered Session not found: {session_text}");
}

/// 构造覆盖首页主要状态和 Usage 字段的标准演示快照。
fn demo_snapshot() -> Snapshot {
    let session = Session {
        provider: Provider::ClaudeCode,
        id: "demo-session".into(),
        provenance: crate::domain::SessionProvenance::Observed,
        source: Default::default(),
        project: Some(Project {
            identity: ProjectIdentity::new(PathBuf::from("/Users/x/code/mudi")),
            label: "mudi".into(),
        }),
        cwd: Some("/Users/x/code/mudi".into()),
        latest_model: Some("claude-opus-4-8".into()),
        state: SessionState::Busy,
        context: ContextState {
            tokens: 150_000,
            window: Some(200_000),
        },
        usage: Usage {
            input: 1000,
            output: 500,
            cache_read: 200,
            cache_write: 100,
        },
        per_model: vec![ModelUsage {
            model: "claude-opus-4-8".into(),
            usage: Usage {
                input: 1000,
                output: 500,
                cache_read: 200,
                cache_write: 100,
            },
            cost: Some(0.42),
        }],
        current_run_usage: None,
        compactions: vec![],
        last_activity_ms: 0,
        cost: Some(0.42),
    };
    Snapshot {
        sessions: vec![session],
        alerts: AlertCounts {
            warning: 1,
            critical: 0,
        },
        ready: true,
        ..Default::default()
    }
}

fn managed_new_snapshot() -> Snapshot {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = 2_000;
    let mut feature = snapshot.sessions[0].clone();
    feature.id = "feature-session".into();
    feature.cwd = Some("/Users/x/code/mudi-feature".into());
    feature.last_activity_ms = 1_000;
    snapshot.sessions.push(feature);
    snapshot
}

#[test]
fn project_context_new_defaults_to_project_root_and_keeps_location_editable() {
    let snapshot = managed_new_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap(),
        UiAction::None
    );
    let initial = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(initial.contains("New"));
    assert!(initial.contains("/Users/x/code/mudi"));
    assert!(initial.contains("Provider *    Not set"));
    assert!(initial.contains("Location"));
    assert!(initial.contains("Title *       Not set"));
    assert!(initial.contains("Description   Not set"));
    assert!(!initial.contains("New Session"));
    assert!(!initial.contains("Create in"));
    assert!(!initial.contains("Initialize"));
    assert!(initial.contains("› Provider *"));
    assert!(initial.contains("↑/↓ or j/k Move · Enter Edit · y Start · Esc/q Close"));
    assert!(!initial.contains("Full access · No sandbox · No approvals"));
    assert!(!initial.contains("Start New"));
    assert_eq!(rendered_modal_height(&initial, "New"), 9);
    let initial_rows = initial.lines().collect::<Vec<_>>();
    let modal_top = initial_rows
        .iter()
        .position(|row| row.contains("New"))
        .unwrap();
    let property_rows = ["Provider *", "Location", "Title *", "Description"].map(|label| {
        initial_rows
            .iter()
            .position(|row| row.contains(label))
            .unwrap()
    });
    assert_eq!(property_rows[0], modal_top + 2);
    assert_eq!(property_rows[1], property_rows[0] + 1);
    assert_eq!(property_rows[2], property_rows[1] + 1);
    assert_eq!(property_rows[3], property_rows[2] + 1);

    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
    assert_eq!(
        ui.test_new_form().unwrap().field(),
        NewSessionField::Provider
    );
    let initial_terminal = render_terminal_at(&ui, &snapshot, 120, 36);
    assert!(rendered_style_at(&initial_terminal, "PROJECT")
        .add_modifier
        .contains(Modifier::DIM));
    assert!(!rendered_style_at(&initial_terminal, "New")
        .add_modifier
        .contains(Modifier::DIM));
    let compact = rendered_rows_at(&ui, &snapshot, 80, 24);
    for visible_control in ["New", "Provider *", "Location", "Title *", "Description"] {
        assert!(compact.contains(visible_control));
    }
    assert!(!compact.contains("Start New"));
    assert_eq!(rendered_modal_height(&compact, "New"), 9);

    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let provider_choice = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(provider_choice.contains("Select Provider"));
    assert!(provider_choice.contains("› ◦ Codex CLI"));
    assert!(provider_choice.contains("Full access · No sandbox · No approvals"));
    assert_eq!(
        rendered_modal_height(&provider_choice, "Select Provider"),
        7
    );
    let provider_rows = provider_choice.lines().collect::<Vec<_>>();
    let provider_top = provider_rows
        .iter()
        .position(|row| row.contains("Select Provider"))
        .unwrap();
    let codex_row = provider_rows
        .iter()
        .position(|row| row.contains("Codex CLI"))
        .unwrap();
    let notice_row = provider_rows
        .iter()
        .position(|row| row.contains("Full access · No sandbox · No approvals"))
        .unwrap();
    assert_eq!(codex_row, provider_top + 2);
    assert_eq!(notice_row, provider_top + 4);
    let compact_provider_choice = rendered_rows_at(&ui, &snapshot, 80, 24);
    assert!(compact_provider_choice.contains("Select Provider"));
    assert!(compact_provider_choice.contains("Enter Select · Esc Back"));
    assert_eq!(
        rendered_modal_height(&compact_provider_choice, "Select Provider"),
        7
    );
    assert_eq!(ui.test_new_form().unwrap().provider, None);
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let codex_selected = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(codex_selected.contains("Codex CLI"));
    assert!(!codex_selected.contains("Full access · No sandbox · No approvals"));
    let codex_terminal = render_terminal_at(&ui, &snapshot, 120, 36);
    assert_eq!(
        rendered_style_at(&codex_terminal, "Codex CLI").fg,
        Some(ui.theme.palette().selected_foreground)
    );

    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let locations = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(locations.contains("Select Location"));
    assert!(locations.contains("/Users/x/code/mudi"));
    assert!(locations.contains("› • /Users/x/code/mudi"));
    assert!(!locations.contains("/Users/x/code/mudi-feature"));
    assert!(locations.contains("Choose Directory..."));
    assert_eq!(rendered_modal_height(&locations, "Select Location"), 6);
    let location_rows = locations.lines().collect::<Vec<_>>();
    let location_top = location_rows
        .iter()
        .position(|row| row.contains("Select Location"))
        .unwrap();
    let current_location_row = location_rows
        .iter()
        .position(|row| row.contains("› • /Users/x/code/mudi"))
        .unwrap();
    let choose_directory_row = location_rows
        .iter()
        .position(|row| row.contains("Choose Directory..."))
        .unwrap();
    assert_eq!(current_location_row, location_top + 2);
    assert_eq!(choose_directory_row, location_top + 3);
    let compact_locations = rendered_rows_at(&ui, &snapshot, 80, 24);
    assert!(compact_locations.contains("Select Location"));
    assert!(compact_locations.contains("Choose Directory..."));
    assert_eq!(
        rendered_modal_height(&compact_locations, "Select Location"),
        6
    );
    ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
    assert_eq!(
        ui.test_new_form().unwrap().targets[ui.test_new_form().unwrap().target_index]
            .working_directory,
        PathBuf::from("/Users/x/code/mudi")
    );
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    for character in "Investigate startup".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    for character in "Trace launch".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Char('y')), &snapshot).unwrap(),
        UiAction::StartManagedNew {
            request: crate::application::managed_session::ManagedNewRequest {
                provider: Provider::CodexCli,
                target: crate::application::managed_session::ManagedLaunchTarget {
                    working_directory: PathBuf::from("/Users/x/code/mudi"),
                },
                title: "Investigate startup".into(),
                description: Some("Trace launch".into()),
                binding_initializer: true,
            }
        }
    );
}

#[test]
fn single_project_location_omits_a_meaningless_count() {
    let snapshot = demo_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();

    let rendered = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(rendered.contains("/Users/x/code/mudi"));
    assert!(!rendered.contains("1/1"));
    assert!(!rendered.contains("1 of 1"));
}

#[test]
fn new_session_shortcut_letters_are_text_only_while_inline_editing() {
    let snapshot = managed_new_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    for character in "yq文".chars() {
        assert_eq!(
            ui.handle_key(key(KeyCode::Char(character)), &snapshot)
                .unwrap(),
            UiAction::None
        );
    }
    assert_eq!(ui.test_new_form().unwrap().title, "");
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Backspace), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Delete), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('好')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert_eq!(ui.test_new_form().unwrap().title, "y好");

    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    for character in "q and y".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
    assert_eq!(ui.test_new_form().unwrap().description, "");
    assert!(ui.test_new_form().is_some());
}

#[test]
fn new_inline_editing_keeps_the_cursor_on_the_spaced_property_row() {
    let snapshot = managed_new_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    let mut title_terminal = render_terminal_at(&ui, &snapshot, 80, 24);
    let title_cursor = title_terminal.get_cursor_position().unwrap();
    let title_rows = rendered_rows_at(&ui, &snapshot, 80, 24);
    let title_row = title_rows
        .lines()
        .position(|row| row.contains("Title *"))
        .unwrap() as u16;
    assert_eq!(title_cursor.y, title_row);

    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let mut description_terminal = render_terminal_at(&ui, &snapshot, 80, 24);
    let description_cursor = description_terminal.get_cursor_position().unwrap();
    let description_rows = rendered_rows_at(&ui, &snapshot, 80, 24);
    let description_row = description_rows
        .lines()
        .position(|row| row.contains("Description"))
        .unwrap() as u16;
    assert_eq!(description_cursor.y, description_row);
    assert_eq!(description_cursor.y, title_cursor.y + 1);
}

#[test]
fn selected_session_new_inherits_its_exact_cwd() {
    let mut snapshot = managed_new_snapshot();
    snapshot.sessions[0].cwd = Some("/Users/x/code/mudi/src/domain".into());
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);

    assert_eq!(
        ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap(),
        UiAction::None
    );
    let form = ui.test_new_form().unwrap();
    assert_eq!(
        form.targets[form.target_index].working_directory,
        PathBuf::from("/Users/x/code/mudi/src/domain")
    );
}

#[test]
fn inherited_new_target_opens_an_in_tui_directory_browser() {
    let project_root = std::env::current_dir().unwrap();
    let snapshot = Snapshot {
        sessions: vec![Session {
            provider: Provider::ClaudeCode,
            id: "local-session".into(),
            provenance: crate::domain::SessionProvenance::Observed,
            source: Default::default(),
            project: Some(Project {
                identity: ProjectIdentity::new(project_root.clone()),
                label: "mudi".into(),
            }),
            cwd: Some(project_root.to_string_lossy().into_owned()),
            latest_model: None,
            state: SessionState::Idle,
            context: ContextState::default(),
            usage: Usage::default(),
            per_model: Vec::new(),
            current_run_usage: None,
            compactions: Vec::new(),
            last_activity_ms: 1,
            cost: None,
        }],
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);

    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::None
    );
    let rendered = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(rendered.contains("Choose Directory"));
    assert!(rendered.contains(project_root.to_string_lossy().as_ref()));
    assert!(rendered.contains("↑/↓ or j/k Move"));
    assert!(rendered.contains("h/l Navigate"));
    assert!(rendered.contains("y Select"));
    assert!(rendered.contains("Esc Cancel"));
}

#[test]
fn confirming_the_directory_browser_requests_resolution_of_its_current_directory() {
    let project_root = std::env::current_dir().unwrap();
    let snapshot = Snapshot {
        sessions: vec![Session {
            provider: Provider::ClaudeCode,
            id: "local-session".into(),
            provenance: crate::domain::SessionProvenance::Observed,
            source: Default::default(),
            project: Some(Project {
                identity: ProjectIdentity::new(project_root.clone()),
                label: "mudi".into(),
            }),
            cwd: Some(project_root.to_string_lossy().into_owned()),
            latest_model: None,
            state: SessionState::Idle,
            context: ContextState::default(),
            usage: Usage::default(),
            per_model: Vec::new(),
            current_run_usage: None,
            compactions: Vec::new(),
            last_activity_ms: 1,
            cost: None,
        }],
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Char('y')), &snapshot).unwrap(),
        UiAction::ResolveManagedNewDirectory {
            directory: project_root,
        }
    );
    assert!(ui.test_directory_browser().is_none());
    assert!(ui.test_new_form().is_some());
}

#[test]
fn directory_browser_navigates_with_arrow_and_vim_keys_and_cancel_preserves_the_new_form() {
    let root = unused_profile_path("directory-browser-root");
    fs::create_dir_all(root.join("child")).unwrap();
    let project_root = std::env::current_dir().unwrap();
    let snapshot = Snapshot {
        sessions: vec![Session {
            provider: Provider::ClaudeCode,
            id: "local-session".into(),
            provenance: crate::domain::SessionProvenance::Observed,
            source: Default::default(),
            project: Some(Project {
                identity: ProjectIdentity::new(project_root.clone()),
                label: "mudi".into(),
            }),
            cwd: Some(project_root.to_string_lossy().into_owned()),
            latest_model: None,
            state: SessionState::Idle,
            context: ContextState::default(),
            usage: Usage::default(),
            per_model: Vec::new(),
            current_run_usage: None,
            compactions: Vec::new(),
            last_activity_ms: 1,
            cost: None,
        }],
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.finish_managed_new_directory_selection(Ok(Some(ManagedLaunchTarget {
        working_directory: root.clone(),
    })));
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    for character in "Preserve this title".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Up), &snapshot).unwrap();
    let form_before = ui.test_new_form().cloned();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    ui.handle_key(key(KeyCode::Char('j')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('k')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Up), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
    assert_eq!(
        ui.test_directory_browser().unwrap().explorer.cwd(),
        &root.join("child")
    );
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    assert_eq!(ui.test_directory_browser().unwrap().explorer.cwd(), &root);
    ui.handle_key(key(KeyCode::Char('j')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('l')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('h')), &snapshot).unwrap();
    assert_eq!(ui.test_directory_browser().unwrap().explorer.cwd(), &root);

    assert_eq!(
        ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap(),
        UiAction::None
    );
    assert!(ui.test_directory_browser().is_none());
    assert_eq!(ui.test_new_form().cloned(), form_before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_target_directory_browser_starts_from_mudi_working_directory() {
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let location_choice = rendered_rows_at(&ui, &snapshot, 80, 24);
    assert!(location_choice.contains("Select Location"));
    assert!(location_choice.contains("› ◦ Choose Directory..."));
    assert_eq!(
        rendered_modal_height(&location_choice, "Select Location"),
        5
    );
    let location_rows = location_choice.lines().collect::<Vec<_>>();
    let location_top = location_rows
        .iter()
        .position(|row| row.contains("Select Location"))
        .unwrap();
    let choose_directory_row = location_rows
        .iter()
        .position(|row| row.contains("Choose Directory..."))
        .unwrap();
    assert_eq!(choose_directory_row, location_top + 2);
    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::None
    );

    assert_eq!(
        ui.test_directory_browser().unwrap().explorer.cwd(),
        &std::env::current_dir().unwrap()
    );
}

#[test]
fn sessions_in_one_project_do_not_create_location_options() {
    let mut snapshot = managed_new_snapshot();
    for index in 2..=9 {
        let mut session = snapshot.sessions[0].clone();
        session.id = format!("subdirectory-session-{index}");
        session.cwd = Some(format!("/Users/x/code/mudi/subdirectory-{index}"));
        session.last_activity_ms = 1_000 - index;
        snapshot.sessions.push(session);
    }
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    let initial = rendered_rows_at(&ui, &snapshot, 80, 24);
    assert_eq!(rendered_modal_height(&initial, "Select Location"), 6);
    let initial_rows = initial.lines().collect::<Vec<_>>();
    let initial_top = initial_rows
        .iter()
        .position(|row| row.contains("Select Location"))
        .unwrap();
    let current_location_row = initial_rows
        .iter()
        .position(|row| row.contains("› • /Users/x/code/mudi"))
        .unwrap();
    assert_eq!(current_location_row, initial_top + 2);
    assert!(!initial.contains("subdirectory-9"));
    let choose_directory_row = initial_rows
        .iter()
        .position(|row| row.contains("Choose Directory..."))
        .unwrap();
    assert_eq!(choose_directory_row, initial_top + 3);
}

#[test]
fn unreadable_directory_browser_start_preserves_the_new_form_and_shows_an_error() {
    let snapshot = managed_new_snapshot();
    let missing = unused_profile_path("missing-directory-browser-root");
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    for character in "Preserve on browser error".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.finish_managed_new_directory_selection(Ok(Some(ManagedLaunchTarget {
        working_directory: missing,
    })));
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::None
    );

    assert!(ui.test_directory_browser().is_none());
    let form = ui.test_new_form().unwrap();
    assert_eq!(form.provider, Some(Provider::CodexCli));
    assert_eq!(form.title, "Preserve on browser error");
    assert!(form
        .error
        .as_deref()
        .unwrap()
        .contains("Directory browser failed"));
    assert!(rendered_rows_at(&ui, &snapshot, 80, 24).contains("Directory browser failed"));
}

#[test]
fn directory_browser_keeps_essential_controls_visible_at_minimum_terminal_size() {
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    let rendered = rendered_rows_at(&ui, &snapshot, 80, 24);
    assert!(rendered.contains("Choose Directory"));
    assert!(rendered.contains("↑/↓ or j/k Move"));
    assert!(rendered.contains("h/l Navigate"));
    assert!(rendered.contains("y Select"));
    assert!(rendered.contains("Esc Cancel"));
}

#[test]
fn replacing_an_inherited_new_target_preserves_form_values() {
    let snapshot = managed_new_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);

    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    for character in "Keep title".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    for character in "Keep description".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

    ui.finish_managed_new_directory_selection(Ok(Some(
        crate::application::managed_session::ManagedLaunchTarget {
            working_directory: PathBuf::from("/Users/x/code/notes"),
        },
    )));

    let form = ui.test_new_form().unwrap();
    assert_eq!(form.provider, Some(Provider::CodexCli));
    assert_eq!(form.title, "Keep title");
    assert_eq!(form.description, "Keep description");
    assert!(form.request().unwrap().binding_initializer);
    assert_eq!(
        form.targets[form.target_index].working_directory,
        PathBuf::from("/Users/x/code/notes")
    );
}

#[test]
fn cancelling_inherited_target_replacement_preserves_the_entire_form() {
    let snapshot = managed_new_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);

    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    for character in "Keep everything".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let before = ui.test_new_form().cloned();

    ui.finish_managed_new_directory_selection(Ok(None));

    assert_eq!(ui.test_new_form().cloned(), before);
}

#[test]
fn failed_inherited_target_replacement_preserves_the_valid_location_and_inputs() {
    let snapshot = managed_new_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);

    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    for character in "Keep title".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.finish_managed_new_directory_selection(Err("ambiguous Git identity".into()));

    let form = ui.test_new_form().unwrap();
    assert_eq!(form.provider, Some(Provider::ClaudeCode));
    assert_eq!(form.title, "Keep title");
    assert!(!form.targets.is_empty());
    assert_eq!(form.field(), NewSessionField::Location);
    assert_eq!(form.error.as_deref(), Some("ambiguous Git identity"));
    assert!(rendered_rows_at(&ui, &snapshot, 120, 36).contains("ambiguous Git identity"));

    assert!(matches!(
        ui.handle_key(key(KeyCode::Char('y')), &snapshot).unwrap(),
        UiAction::StartManagedNew { .. }
    ));
}

#[test]
fn new_without_project_tree_context_opens_the_form_before_the_directory_browser() {
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    assert_eq!(
        ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap(),
        UiAction::None
    );
    let form = ui.test_new_form().unwrap();
    assert!(form.targets.is_empty());

    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::None
    );
    assert!(ui.test_directory_browser().is_some());
}

#[test]
fn repeated_new_shortcuts_never_open_the_directory_browser() {
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let key = || KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);

    for _ in 0..3 {
        assert_eq!(ui.handle_key(key(), &snapshot).unwrap(), UiAction::None);
    }
    assert!(ui.test_new_form().is_some());
}

#[test]
fn directory_context_can_be_reselected_before_new_is_confirmed() {
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let directory = std::env::current_dir().unwrap();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.finish_managed_new_directory_selection(Ok(Some(
        crate::application::managed_session::ManagedLaunchTarget {
            working_directory: directory.clone(),
        },
    )));
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::None
    );
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert!(ui.test_directory_browser().is_some());
    ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();

    ui.finish_managed_new_directory_selection(Err("ambiguous Git identity".into()));
    let rendered = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(rendered.contains("ambiguous Git identity"));
    assert!(rendered.contains(directory.to_string_lossy().as_ref()));

    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::None
    );
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert!(ui.test_directory_browser().is_some());
}

#[test]
fn selected_session_new_requires_title_and_does_not_remember_provider() {
    let snapshot = managed_new_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();

    let selected = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(selected.contains("/Users/x/code/mudi-feature"));
    assert!(selected.contains("Provider *    Not set"));
    assert_eq!(
        ui.handle_key(key(KeyCode::Char('y')), &snapshot).unwrap(),
        UiAction::None
    );
    assert!(rendered_rows_at(&ui, &snapshot, 120, 36).contains("Provider is required"));
    assert_eq!(
        ui.test_new_form().unwrap().field(),
        NewSessionField::Provider
    );
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert_eq!(
        ui.test_new_form().unwrap().provider,
        Some(Provider::ClaudeCode)
    );
    assert_eq!(
        ui.handle_key(key(KeyCode::Char('y')), &snapshot).unwrap(),
        UiAction::None
    );
    let invalid = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(invalid.contains("Title is required"));
    assert_eq!(ui.test_new_form().unwrap().field(), NewSessionField::Title);
    assert_eq!(rendered_modal_height(&invalid, "New"), 9);
    let invalid_rows = invalid.lines().collect::<Vec<_>>();
    let modal_top = invalid_rows
        .iter()
        .position(|row| row.contains("New"))
        .unwrap();
    let description_row = invalid_rows
        .iter()
        .position(|row| row.contains("Description"))
        .unwrap();
    let error_row = invalid_rows
        .iter()
        .position(|row| row.contains("Title is required"))
        .unwrap();
    assert_eq!(error_row, description_row + 1);
    assert_eq!(modal_top + 8, error_row + 2);

    ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('q')), &snapshot).unwrap();
    assert!(!rendered_rows_at(&ui, &snapshot, 120, 36).contains("Provider *"));
}

/// 对总览及可选详情状态执行一次无崩溃渲染。
fn render_overview(detail: bool) {
    let snap = demo_snapshot();
    let mut ui = Ui::default();
    if detail {
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snap)
            .unwrap();
    }
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    // 无论当前状态如何，布局计算和绘制过程都不能崩溃。
    terminal.draw(|f| draw(f, &snap, &ui)).unwrap();
}

fn open_first_session_detail(ui: &mut Ui, snapshot: &Snapshot) {
    let session = snapshot.sessions.first().expect("test Session");
    ui.test_set_detail(DetailView {
        session: SessionIdentity::from(session),
        task_id: None,
        scroll: StateCell::new(0),
        selected_field: 0,
        origin_signal: None,
    });
}

#[test]
fn t_key_does_not_change_the_active_theme_or_show_a_notice() {
    let mut ui = Ui::default();
    let snapshot = Snapshot {
        ..Snapshot::default()
    };

    assert_eq!(ui.active_theme_name(), "mudi");
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();

    assert_eq!(ui.active_theme_name(), "mudi");
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Theme:"));
}

#[test]
fn light_theme_supplies_background_border_accent_and_muted_roles() {
    let profile_path = unused_profile_path("light-roles");
    std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
    let ui = Ui::load(&profile_path).unwrap();
    let snapshot = Snapshot::default();

    let terminal = render_terminal_with_snapshot(&ui, &snapshot);
    let top_left = &terminal.backend().buffer()[(0, 0)];

    assert_eq!(top_left.style().bg, Some(Color::Rgb(250, 249, 246)));
    assert_eq!(top_left.style().fg, Some(Color::Rgb(126, 130, 133)));
    assert_eq!(
        rendered_style_at(&terminal, "Mudi").fg,
        Some(Color::Rgb(25, 91, 130))
    );
    assert_eq!(
        rendered_style_at(&terminal, &format!("v{}", env!("CARGO_PKG_VERSION")),).fg,
        Some(Color::Rgb(96, 101, 106))
    );
    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn now_groups_open_sessions_across_providers_and_omits_exited_sessions() {
    let ui = Ui::default();
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = Some("claude-open-model".into());

    let mut codex = snapshot.sessions[0].clone();
    codex.provider = Provider::CodexCli;
    codex.id = "codex-open".into();
    codex.latest_model = Some("codex-open-model".into());
    codex.state = SessionState::Idle;

    let mut exited = snapshot.sessions[0].clone();
    exited.id = "exited-session".into();
    exited.latest_model = Some("exited-model".into());
    exited.state = SessionState::Exited;
    snapshot.sessions.extend([codex, exited]);

    let page = rendered_rows_at(&ui, &snapshot, 120, 30);
    let project = page.find("▾ mudi").unwrap();
    let claude = page.find("✳ -session").unwrap();
    let codex = page.find("⬡ dex-open").unwrap();

    assert!(project < claude && claude < codex);
    assert_eq!(
        page.lines().filter(|line| line.contains("▾ mudi")).count(),
        1
    );
    let session_ids = project_tree::open_sessions(&snapshot)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(session_ids, ["demo-session", "codex-open"]);
    assert!(!page.contains("exited-model"));
}

#[test]
fn now_sorts_initial_projects_by_activity_and_their_sessions_by_attention() {
    let mut healthy = demo_snapshot().sessions.remove(0);
    healthy.id = "healthy".into();
    healthy.project = Some(test_project("zulu"));
    healthy.latest_model = Some("healthy-model".into());
    healthy.context.tokens = 80_000;
    healthy.last_activity_ms = 2_000;

    let mut warning = healthy.clone();
    warning.id = "warning".into();
    warning.project = Some(test_project("alpha"));
    warning.latest_model = Some("warning-model".into());
    warning.context.tokens = 150_000;
    warning.last_activity_ms = 1_000;

    let mut shared = warning.clone();
    shared.provider = Provider::CodexCli;
    shared.id = "shared".into();
    shared.latest_model = Some("shared-model".into());
    shared.context.tokens = 80_000;

    let mut critical = shared.clone();
    critical.provider = Provider::ClaudeCode;
    critical.id = "critical".into();
    critical.latest_model = Some("critical-model".into());
    critical.state = SessionState::Idle;
    critical.context.tokens = 180_000;

    let mut snapshot = Snapshot {
        sessions: vec![healthy, warning, shared, critical],
        generated_ms: 3_000,
        ready: true,
        ..Snapshot::default()
    };
    snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

    let session_ids = project_tree::open_sessions(&snapshot)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(session_ids, ["healthy", "critical", "warning", "shared"]);

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    assert!(page.contains("✳ critical"));
    assert!(page.contains("⬡ shared"));
    assert!(page.contains("✳ warning"));
}

#[test]
fn now_shows_shared_project_risk_and_every_contributing_session() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].context.tokens = 80_000;
    snapshot.sessions[0].latest_model = Some("claude-member".into());
    let mut codex = snapshot.sessions[0].clone();
    codex.provider = Provider::CodexCli;
    codex.id = "codex-contributor".into();
    codex.latest_model = Some("codex-member".into());
    codex.state = SessionState::Idle;
    snapshot.sessions.push(codex);
    snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    let claude = page
        .lines()
        .find(|line| line.contains("✳ -session"))
        .unwrap();
    let codex = page
        .lines()
        .find(|line| line.contains("⬡ tributor"))
        .unwrap();

    assert!(claude.contains('!'));
    assert!(codex.contains('!'));
}

#[test]
fn now_shows_critical_context_label_and_exact_percentage_in_the_editor_tree() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = Some("critical-model".into());
    snapshot.sessions[0].context.tokens = 180_000;
    snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    let critical = page
        .lines()
        .find(|line| line.contains("✳ -session"))
        .unwrap();

    assert!(critical.contains('!'));
    assert!(critical.contains("90%"));
    assert!(critical.contains('•'));
    assert!(critical.contains('✳'));
    assert!(critical.contains("-session"));
}

#[test]
fn now_shows_warning_context_label_and_exact_percentage_in_the_editor_tree() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = Some("warning-model".into());
    snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    let warning = page
        .lines()
        .find(|line| line.contains("✳ -session"))
        .unwrap();

    assert!(warning.contains('!'));
    assert!(warning.contains("75%"));
}

#[test]
fn now_shows_the_last_eight_native_session_id_characters() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].id = "12345678remaining-id".into();
    snapshot.sessions[0].latest_model = Some("session-id-model".into());

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    let session = page
        .lines()
        .find(|line| line.contains("✳ ining-id"))
        .unwrap();

    assert!(session.contains("✳ ining-id"));
    assert!(page.contains("session-id-model"));
    assert!(!session.contains("12345678"));
}

#[test]
fn now_preserves_lifecycle_colors_when_attention_overrides_rank() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].context.tokens = 150_000;
    let mut idle = snapshot.sessions[0].clone();
    idle.id = "idle-warning".into();
    idle.project = Some(test_project("idle-warning-project"));
    idle.cwd = Some("/Users/x/code/idle-warning".into());
    idle.state = SessionState::Idle;
    snapshot.sessions.push(idle);
    let ui = Ui::default();

    let terminal = render_terminal_at(&ui, &snapshot, 120, 30);
    let busy = rendered_style_at(&terminal, "•");
    let idle = rendered_style_at(&terminal, "◦");
    let warning = rendered_style_at(&terminal, "!");

    assert_eq!(busy.fg, Some(Color::Rgb(120, 205, 120)));
    assert!(busy.add_modifier.contains(Modifier::BOLD));
    assert_eq!(idle.fg, Some(Color::Rgb(105, 184, 220)));
    assert_eq!(warning.fg, Some(Color::Rgb(238, 190, 70)));
    assert_ne!(busy, warning);
    assert_ne!(idle, warning);
}

#[test]
fn now_shows_idle_elapsed_from_transition_and_last_activity_fallback() {
    let now_ms = Local::now().timestamp_millis();
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].id = "observed-idle".into();
    snapshot.sessions[0].latest_model = Some("observed-idle".into());
    snapshot.sessions[0].context.tokens = 80_000;
    snapshot.sessions[0].last_activity_ms = now_ms - 20 * 60_000;
    let mut engine = AttentionEngine::default();
    engine.update(&snapshot.sessions, now_ms - 5 * 60_000);

    snapshot.sessions[0].state = SessionState::Idle;
    let mut fallback = snapshot.sessions[0].clone();
    fallback.provider = Provider::CodexCli;
    fallback.id = "fallback-idle".into();
    fallback.latest_model = Some("fallback-idle".into());
    fallback.project = Some(test_project("fallback"));
    fallback.last_activity_ms = now_ms - 11 * 60_000;
    snapshot.sessions.push(fallback);
    snapshot.attention = Some(engine.update(&snapshot.sessions, now_ms - 4 * 60_000));

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    let observed = page
        .lines()
        .find(|line| line.contains("✳ ved-idle"))
        .unwrap();
    let fallback = page
        .lines()
        .find(|line| line.contains("⬡ ack-idle"))
        .unwrap();

    assert!(observed.contains('◦'));
    assert!(observed.contains("ved-idle"));
    assert!(fallback.contains('◦'));
    assert!(fallback.contains("ack-idle"));
}

#[test]
fn all_page_selection_round_trips_through_session_detail() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = Some("first-open-model".into());
    let mut second = snapshot.sessions[0].clone();
    second.provider = Provider::CodexCli;
    second.id = "second-open".into();
    second.latest_model = Some("second-open-model".into());
    snapshot.sessions.push(second);
    let mut ui = Ui::default();

    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_with_snapshot(&ui, &snapshot);
    assert!(detail.contains("second-open"));
    assert!(!detail.contains("first-open-model"));

    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let all = render_terminal_with_snapshot(&ui, &snapshot);
    let cursor = now_cursor_style_at(&all, "ond-open").unwrap();
    assert_eq!(cursor.fg, Some(Color::Rgb(230, 180, 80)));
    assert!(cursor.add_modifier.contains(Modifier::BOLD));
    assert_eq!(now_cursor_style_at(&all, "-session"), None);
}

#[test]
fn session_tree_keyboard_traverses_visible_nodes_and_never_hidden_sessions() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = Some("first-open-model".into());
    let mut second = snapshot.sessions[0].clone();
    second.provider = Provider::CodexCli;
    second.id = "second-open".into();
    second.latest_model = Some("second-open-model".into());
    snapshot.sessions.push(second);
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    ui.handle_key(key(KeyCode::Up), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let collapsed_project = rendered_rows_at(&ui, &snapshot, 120, 30);
    assert!(collapsed_project.contains(">▸ mudi (2)"));
    assert!(!collapsed_project.contains("✳ -session"));
    assert!(!collapsed_project.contains("⬡ ond-open"));
    assert!(ui.selected_session(&snapshot).is_none());

    ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
    assert_eq!(ui.selected_session(&snapshot).unwrap().id, "demo-session");

    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    let collapsed_again = rendered_rows_at(&ui, &snapshot, 120, 30);
    assert!(collapsed_again.contains(">▸ mudi (2)"));
    assert!(!collapsed_again.contains("/Users/x/code/mudi"));
}

#[test]
fn all_page_session_selection_survives_attention_reordering_by_provider_and_native_id() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].id = "stable-claude".into();
    snapshot.sessions[0].latest_model = Some("claude-model".into());
    snapshot.sessions[0].context.tokens = 60_000;
    let mut codex = snapshot.sessions[0].clone();
    codex.provider = Provider::CodexCli;
    codex.id = "stable-codex".into();
    codex.latest_model = Some("codex-model".into());
    codex.context.tokens = 150_000;
    snapshot.sessions.push(codex);
    snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 1_000));
    let mut ui = Ui::default();
    ui.project_tree.select_session(SessionIdentity {
        provider: Provider::ClaudeCode,
        native_id: "stable-claude".into(),
    });

    snapshot.sessions[0].context.tokens = 180_000;
    snapshot.sessions[1].context.tokens = 20_000;
    snapshot.sessions.reverse();
    snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

    assert_eq!(
        ui.selected_session(&snapshot)
            .map(|session| (session.provider, session.id.as_str())),
        Some((Provider::ClaudeCode, "stable-claude"))
    );
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    assert_eq!(
        ui.test_session_editor().map(|editor| &editor.session),
        Some(&SessionIdentity {
            provider: Provider::ClaudeCode,
            native_id: "stable-claude".into(),
        })
    );
    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let detail = rendered_with_snapshot(&ui, &snapshot);
    assert!(detail.contains("stable-claude"));
    assert!(detail.contains("Claude Code"));
    assert!(!detail.contains("codex-model"));
}

#[test]
fn edit_session_targets_the_project_session_selected_with_vertical_navigation() {
    let mut snapshot = demo_snapshot();
    let mut second = snapshot.sessions[0].clone();
    snapshot.sessions[0].id = "session-a".into();
    second.id = "session-b".into();
    snapshot.sessions = vec![snapshot.sessions[0].clone(), second];
    snapshot.attention = None;
    let mut ui = Ui::default();

    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();

    assert_eq!(
        ui.test_session_editor().map(|editor| &editor.session),
        Some(&SessionIdentity {
            provider: Provider::ClaudeCode,
            native_id: "session-b".into(),
        })
    );
}

#[test]
fn all_page_selection_does_not_merge_matching_native_ids_across_providers() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].id = "shared-native-id".into();
    snapshot.sessions[0].latest_model = Some("claude-collision-model".into());
    let mut codex = snapshot.sessions[0].clone();
    codex.provider = Provider::CodexCli;
    codex.latest_model = Some("codex-collision-model".into());
    snapshot.sessions.push(codex);
    let mut ui = Ui::default();
    ui.project_tree.select_session(SessionIdentity {
        provider: Provider::CodexCli,
        native_id: "shared-native-id".into(),
    });

    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let detail = rendered_with_snapshot(&ui, &snapshot);
    assert!(detail.contains("Codex CLI"));
    assert!(detail.contains("codex-collision-model"));
    assert!(!detail.contains("claude-collision-model"));
}

#[test]
fn disappearing_tree_selection_falls_back_to_first_attention_session() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].id = "remaining-session".into();
    let mut disappearing = snapshot.sessions[0].clone();
    disappearing.provider = Provider::CodexCli;
    disappearing.id = "disappearing-session".into();
    snapshot.sessions.push(disappearing);
    let mut ui = Ui::default();
    ui.project_tree.select_session(SessionIdentity {
        provider: Provider::CodexCli,
        native_id: "disappearing-session".into(),
    });

    snapshot
        .sessions
        .retain(|session| session.id == "remaining-session");
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_with_snapshot(&ui, &snapshot);
    assert!(detail.contains("remaining-session"));
    assert!(!detail.contains("disappearing-session"));
}

#[test]
fn now_keeps_known_and_unknown_current_context_evidence_visible() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = Some("known-model".into());
    snapshot.sessions[0].context = ContextState {
        tokens: 140_000,
        window: Some(200_000),
    };
    let mut unknown = snapshot.sessions[0].clone();
    unknown.provider = Provider::CodexCli;
    unknown.id = "unknown-context".into();
    unknown.latest_model = Some("unknown-model".into());
    unknown.context = ContextState {
        tokens: 42_000,
        window: None,
    };
    snapshot.sessions.push(unknown);
    let ui = Ui::default();

    let terminal = render_terminal_at(&ui, &snapshot, 120, 30);
    let page = terminal
        .backend()
        .buffer()
        .content()
        .chunks(120)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>();
    let known = page
        .iter()
        .find(|line| line.contains("✳ -session"))
        .unwrap();
    let unknown = page
        .iter()
        .find(|line| line.contains("⬡ -context"))
        .unwrap();

    assert!(known.contains("70%"));
    assert!(unknown.contains("N/A"));
    assert_eq!(
        rendered_style_at(&terminal, "N/A").fg,
        Some(Color::Rgb(132, 136, 141))
    );
}

#[test]
fn now_renders_a_session_directly_beneath_its_project() {
    let snapshot = demo_snapshot();
    let ui = Ui::default();

    let page = rendered_rows_at(&ui, &snapshot, 120, 30);
    let project = page.lines().find(|line| line.contains("▾ mudi")).unwrap();

    assert!(project.contains("(1)"));
    assert!(page.contains("Directory"));
    assert!(page.contains("/Users/x/code/mudi"));
    assert!(!project.contains("Context"));
}

#[test]
fn now_renders_multiple_sessions_directly_below_the_project() {
    let mut snapshot = demo_snapshot();
    let mut feature = snapshot.sessions[0].clone();
    feature.id = "feature-session".into();
    feature.cwd = Some("/Users/x/code/mudi/subdirectory".into());
    snapshot.sessions.push(feature);

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    let project = page.lines().find(|line| line.contains("▾ mudi")).unwrap();
    let session_rows = page
        .lines()
        .filter(|line| line.contains('✳'))
        .collect::<Vec<_>>();

    assert!(project.contains("(2)"));
    assert!(session_rows.len() >= 2);
}

#[test]
fn all_page_has_global_usage_without_a_time_filter_or_cost_surface() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].cost = Some(123.45);
    snapshot.sessions[0].usage = Usage {
        input: 987_654,
        output: 123_456,
        cache_read: 456_789,
        cache_write: 12_345,
    };
    let mut ui = Ui::default();

    ui.handle_key(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    let page = rendered_rows_at(&ui, &snapshot, 120, 30);

    for excluded in [
        "Active Today",
        "Last 7 Days",
        "All Sessions",
        "Estimated using API rates",
        "$123",
    ] {
        assert!(
            !page.contains(excluded),
            "unexpected All-page text: {excluded}"
        );
    }
    let header = page.lines().next().unwrap();
    assert!(header.contains("In 987.7k"));
    assert!(header.contains("Out 123.5k"));
}

#[test]
fn all_page_places_status_panels_side_by_side_in_the_upper_right() {
    let snapshot = demo_snapshot();
    let ui = Ui::default();

    let wide = rendered_rows_at(&ui, &snapshot, 120, 30);
    let wide_lines = wide.lines().collect::<Vec<_>>();
    assert!(wide_lines[1].contains("RECENT SIGNALS"));
    assert!(wide_lines[1].contains("TASKS"));
    assert!(wide_lines[9].contains("PROJECT"));
    assert!(wide_lines[9].contains("Session Workspace"));
    for line in &wide_lines[1..9] {
        assert!(line.chars().take(40).all(|character| character == ' '));
    }

    let compact = rendered_rows_at(&ui, &snapshot, 80, 24);
    let compact_lines = compact.lines().collect::<Vec<_>>();
    assert!(compact_lines[1].contains("RECENT SIGNALS"));
    assert!(compact_lines[1].contains("TASKS"));
    assert!(compact_lines[9].contains("PROJECT"));
    assert!(compact_lines[9].contains("Session Workspace"));
    for line in &compact_lines[1..9] {
        assert!(line.chars().take(26).all(|character| character == ' '));
    }

    assert!(wide.contains("RECENT SIGNALS"));
    assert!(compact.contains("RECENT SIGNALS"));
    assert!(wide.contains("TASKS"));
    assert!(compact.contains("TASKS"));
    for speculative_label in ["FOCUS", "PROJECT PULSE"] {
        assert!(!wide.contains(speculative_label));
        assert!(!compact.contains(speculative_label));
    }
}

#[test]
fn recent_signals_render_newest_first_without_history_controls() {
    let mut snapshot = demo_snapshot();
    snapshot.recent_signals = vec![
        RecentSignal::Session {
            observed_at_ms: 2_000,
            provider: Provider::ClaudeCode,
            session_id: "newest-signal".into(),
            kind: RecentSignalKind::SessionStateChanged {
                from: SessionState::Busy,
                to: SessionState::Idle,
            },
        },
        RecentSignal::Session {
            observed_at_ms: 1_000,
            provider: Provider::CodexCli,
            session_id: "older-signal".into(),
            kind: RecentSignalKind::SessionStateChanged {
                from: SessionState::Idle,
                to: SessionState::Exited,
            },
        },
    ];

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);

    assert!(page.contains("RECENT SIGNALS"));
    assert!(page.find("t-signal").unwrap() < page.find("r-signal").unwrap());
    assert!(page.contains("Busy -> Idle"));
    assert!(page.contains("Idle -> Exited"));
    for excluded in ["View More", "Filter", "Scroll"] {
        assert!(!page.contains(excluded));
    }
}

#[test]
fn shared_project_signal_renders_canonical_path_and_membership_evidence() {
    let mut snapshot = demo_snapshot();
    snapshot.recent_signals = vec![RecentSignal::Project {
        observed_at_ms: 2_000,
        project_path: "/projects/shared".into(),
        change: SharedProjectRiskChange::Detected,
        before: vec![crate::application::attention::SessionIdentity {
            provider: Provider::ClaudeCode,
            native_id: "claude-a".into(),
        }],
        after: vec![
            crate::application::attention::SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "claude-a".into(),
            },
            crate::application::attention::SessionIdentity {
                provider: Provider::CodexCli,
                native_id: "codex-b".into(),
            },
        ],
    }];

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);

    for evidence in [
        "/projects/shared",
        "Risk detected",
        "Before 1: ✳:claude-a",
        "After 2: ✳:claude-a, ⬡:codex-b",
    ] {
        assert!(page.contains(evidence), "missing evidence {evidence:?}");
    }
    assert!(!page.to_lowercase().contains("conflict"));
}

#[test]
fn project_signal_locates_the_recommended_now_session_not_its_first_member() {
    let mut snapshot = demo_snapshot();
    let project_path = PathBuf::from("/projects/shared");
    let healthy = &mut snapshot.sessions[0];
    healthy.id = "arbitrary-first".into();
    healthy.latest_model = Some("arbitrary-model".into());
    healthy.context.tokens = 80_000;
    healthy.project.as_mut().unwrap().identity = ProjectIdentity::new(project_path.clone());

    let mut critical = healthy.clone();
    critical.provider = Provider::CodexCli;
    critical.id = "recommended".into();
    critical.latest_model = Some("recommended-model".into());
    critical.context.tokens = 180_000;
    snapshot.sessions.push(critical);
    snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));
    snapshot.recent_signals = vec![RecentSignal::Project {
        observed_at_ms: 2_000,
        project_path,
        change: SharedProjectRiskChange::Detected,
        before: vec![crate::application::attention::SessionIdentity {
            provider: Provider::ClaudeCode,
            native_id: "arbitrary-first".into(),
        }],
        after: vec![
            crate::application::attention::SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "arbitrary-first".into(),
            },
            crate::application::attention::SessionIdentity {
                provider: Provider::CodexCli,
                native_id: "recommended".into(),
            },
        ],
    }];
    let mut ui = Ui::default();

    ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let located = render_terminal_at(&ui, &snapshot, 120, 30);

    let cursor = now_cursor_style_at(&located, "ommended").unwrap();
    assert_eq!(cursor.fg, Some(Color::Rgb(230, 180, 80)));
    assert_eq!(now_cursor_style_at(&located, "ry-first"), None);
    assert_eq!(
        rendered_style_at(&located, "Risk detected").bg,
        Some(ThemeName::Mudi.palette().selected_background)
    );
}

#[test]
fn cleared_project_signal_stays_selected_when_the_project_is_no_longer_open() {
    let mut snapshot = demo_snapshot();
    let project_path = PathBuf::from("/projects/closed");
    snapshot.sessions[0].state = SessionState::Exited;
    snapshot.sessions[0].project.as_mut().unwrap().identity =
        ProjectIdentity::new(project_path.clone());
    snapshot.recent_signals = vec![RecentSignal::Project {
        observed_at_ms: 2_000,
        project_path,
        change: SharedProjectRiskChange::Cleared,
        before: vec![
            crate::application::attention::SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "claude-a".into(),
            },
            crate::application::attention::SessionIdentity {
                provider: Provider::CodexCli,
                native_id: "codex-b".into(),
            },
        ],
        after: vec![],
    }];
    let selected_background = Some(ThemeName::Mudi.palette().selected_background);
    let mut ui = Ui::default();

    ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let unchanged = render_terminal_at(&ui, &snapshot, 120, 30);
    let page = rendered_rows_at(&ui, &snapshot, 120, 30);

    assert_eq!(
        rendered_style_at(&unchanged, "Risk cleared").bg,
        selected_background
    );
    assert!(page.contains("Project is no longer open"));
}

#[test]
fn all_page_focus_moves_between_independently_selected_regions() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = Some("first-now-model".into());
    let mut second = snapshot.sessions[0].clone();
    second.provider = Provider::CodexCli;
    second.id = "second-now".into();
    second.latest_model = Some("second-now-model".into());
    snapshot.sessions.push(second);
    snapshot.recent_signals = vec![
        RecentSignal::Session {
            observed_at_ms: 2_000,
            provider: Provider::ClaudeCode,
            session_id: "first-signal".into(),
            kind: RecentSignalKind::SessionStateChanged {
                from: SessionState::Busy,
                to: SessionState::Idle,
            },
        },
        RecentSignal::Session {
            observed_at_ms: 1_000,
            provider: Provider::CodexCli,
            session_id: "second-signal".into(),
            kind: RecentSignalKind::SessionStateChanged {
                from: SessionState::Idle,
                to: SessionState::Exited,
            },
        },
    ];
    let selected_background = Some(ThemeName::Mudi.palette().selected_background);
    let mut ui = Ui::default();

    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let signals_focused = render_terminal_at(&ui, &snapshot, 120, 30);
    assert_eq!(
        rendered_style_at(&signals_focused, "d-signal").bg,
        selected_background
    );
    assert_eq!(
        now_cursor_style_at(&signals_focused, "cond-now").and_then(|style| style.bg),
        selected_background
    );

    ui.handle_key(
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        &snapshot,
    )
    .unwrap();
    let now_focused = render_terminal_at(&ui, &snapshot, 120, 30);
    let cursor = now_cursor_style_at(&now_focused, "cond-now").unwrap();
    assert_eq!(cursor.fg, Some(Color::Rgb(230, 180, 80)));
    assert_eq!(cursor.bg, selected_background);
    assert_eq!(
        rendered_style_at(&now_focused, "d-signal").bg,
        selected_background
    );
}

#[test]
fn signal_detail_returns_to_the_signal_and_preserves_now_selection() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = Some("first-now-model".into());
    let mut target = snapshot.sessions[0].clone();
    target.provider = Provider::CodexCli;
    target.id = "signal-detail-target".into();
    target.latest_model = Some("target-now-model".into());
    snapshot.sessions.push(target);
    snapshot.recent_signals = vec![
        RecentSignal::Session {
            observed_at_ms: 2_000,
            provider: Provider::ClaudeCode,
            session_id: "demo-session".into(),
            kind: RecentSignalKind::SessionStateChanged {
                from: SessionState::Busy,
                to: SessionState::Idle,
            },
        },
        RecentSignal::Session {
            observed_at_ms: 1_000,
            provider: Provider::CodexCli,
            session_id: "signal-detail-target".into(),
            kind: RecentSignalKind::SessionStateChanged {
                from: SessionState::Idle,
                to: SessionState::Exited,
            },
        },
    ];
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let selected_background = Some(ThemeName::Mudi.palette().selected_background);
    let mut ui = Ui::default();

    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let detail = rendered_with_snapshot(&ui, &snapshot);
    assert!(detail.contains("signal-detail-target"));
    assert!(detail.contains("target-now-model"));
    assert!(detail.contains("RECENT SIGNALS"));

    ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
    let restored_signal = render_terminal_at(&ui, &snapshot, 120, 30);
    assert_eq!(
        rendered_style_at(&restored_signal, "l-target").bg,
        selected_background
    );

    ui.handle_key(
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        &snapshot,
    )
    .unwrap();
    let restored_now = render_terminal_at(&ui, &snapshot, 120, 30);
    let cursor = now_cursor_style_at(&restored_now, "l-target").unwrap();
    assert_eq!(cursor.fg, Some(Color::Rgb(230, 180, 80)));
}

#[test]
fn compound_signal_keeps_compaction_and_context_clear_evidence_visible() {
    let mut snapshot = demo_snapshot();
    snapshot.recent_signals = vec![RecentSignal::Session {
        observed_at_ms: 2_000,
        provider: Provider::ClaudeCode,
        session_id: "s".into(),
        kind: RecentSignalKind::CompactionObserved {
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
        },
    }];

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    let signal = page.lines().find(|line| line.contains("Cmp")).unwrap();

    for evidence in ["auto", "180.0k", "80.0k", "Critical"] {
        assert!(signal.contains(evidence), "missing evidence {evidence:?}");
    }
}

#[test]
fn compact_status_strip_limits_signal_rows_while_the_editor_remains_visible() {
    let mut snapshot = demo_snapshot();
    snapshot.recent_signals = (1..=7)
        .map(|position| RecentSignal::Session {
            observed_at_ms: 3_000 - position,
            provider: Provider::ClaudeCode,
            session_id: format!("signal-{position}"),
            kind: RecentSignalKind::SessionStateChanged {
                from: SessionState::Busy,
                to: SessionState::Idle,
            },
        })
        .collect();

    let page = rendered_rows_at(&Ui::default(), &snapshot, 80, 24);

    assert!(page.contains("PROJECT"));
    assert!(page.contains("DETAILS"));
    assert!(page.contains("Session Workspace"));
    for visible in [
        "signal-1", "signal-2", "signal-3", "signal-4", "signal-5", "signal-6",
    ] {
        assert!(page.contains(visible));
    }
    assert!(!page.contains("signal-7"));
}

#[test]
fn now_empty_state_explains_that_there_are_no_open_sessions() {
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let ui = Ui::default();

    let page = rendered_rows_at(&ui, &snapshot, 80, 24);

    assert!(page.contains("No Open Sessions."));
    for manufactured_surface in ["Alert", "ATTENTION", "Warning", "Critical"] {
        assert!(!page.contains(manufactured_surface));
    }
}

#[test]
fn now_keeps_healthy_busy_sessions_visible_without_manufacturing_attention() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = Some("healthy-busy-model".into());
    snapshot.sessions[0].context.tokens = 80_000;
    let ui = Ui::default();

    let page = rendered_rows_at(&ui, &snapshot, 120, 30);
    let session = page
        .lines()
        .find(|line| line.contains("✳ -session"))
        .unwrap();

    assert!(session.contains('•'));
    assert!(session.contains('✳'));
    assert!(session.contains("40%"));
    assert!(session.contains("-session"));
    assert!(!session.contains('!'));
    assert_eq!(page.matches("✳ -session").count(), 1);
    for manufactured_condition in ["Alert", "ATTENTION", "Warning", "Critical"] {
        assert!(!page.contains(manufactured_condition));
    }
}

#[test]
fn compact_now_shortens_long_identity_before_operational_evidence() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].cwd = Some("/Users/x/code/mudi/attention-layout-prototype".into());
    snapshot.sessions[0].latest_model =
        Some("claude-sonnet-with-an-intentionally-long-model-name".into());
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    let ui = Ui::default();

    let page = rendered_rows_at(&ui, &snapshot, 80, 24);
    let session = page
        .lines()
        .find(|line| line.contains("✳ -session"))
        .unwrap();

    assert!(page.contains("Directory"));
    assert!(page.contains("Provider"));
    for evidence in ["!", "•", "✳", "-session", "75%"] {
        assert!(
            session.contains(evidence),
            "missing compact evidence: {evidence}"
        );
    }
}

#[test]
fn changing_theme_does_not_change_rendered_product_semantics() {
    let mudi_path = unused_profile_path("mudi-semantics");
    let light_path = unused_profile_path("light-semantics");
    std::fs::write(&mudi_path, br#"{"help_seen":true,"theme":"mudi"}"#).unwrap();
    std::fs::write(&light_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
    let mudi = Ui::load(&mudi_path).unwrap();
    let light = Ui::load(&light_path).unwrap();
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    snapshot.alerts = AlertCounts {
        warning: 3,
        critical: 2,
    };

    assert_eq!(
        rendered_with_snapshot(&mudi, &snapshot),
        rendered_with_snapshot(&light, &snapshot)
    );

    let _ = std::fs::remove_file(mudi_path);
    let _ = std::fs::remove_file(light_path);
}

#[test]
fn guide_uses_the_active_theme_roles() {
    let profile_path = unused_profile_path("themed-guide");
    std::fs::write(&profile_path, br#"{"theme":"light"}"#).unwrap();
    let ui = Ui::load(&profile_path).unwrap();

    let terminal = render_terminal(&ui);

    assert_eq!(
        rendered_style_at(&terminal, "> Overview").bg,
        Some(Color::Rgb(230, 233, 235))
    );
    assert_eq!(
        rendered_style_at(&terminal, "Guide").fg,
        Some(Color::Rgb(25, 91, 130))
    );
    assert_eq!(
        rendered_style_at(&terminal, "Passive observation is the default.").fg,
        Some(Color::Rgb(28, 31, 34))
    );

    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn footer_uses_theme_roles_and_exposes_the_global_guide() {
    let profile_path = unused_profile_path("light-footer");
    std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
    let ui = Ui::load(&profile_path).unwrap();
    let snapshot = Snapshot::default();
    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| draw(f, &snapshot, &ui)).unwrap();

    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(!rendered.contains("Active Today"));
    assert_eq!(
        rendered_style_at(&terminal, "? Guide").fg,
        Some(Color::Rgb(96, 101, 106))
    );
    assert!(!rendered.contains("t Theme"));
    assert!(!rendered.contains("Theme: light"));
    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn saved_theme_selection_is_restored_from_the_profile() {
    let profile_path = unused_profile_path("persisted-theme");
    std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"catppuccin"}"#).unwrap();

    let reloaded = Ui::load(&profile_path).unwrap();
    assert_eq!(reloaded.active_theme_name(), "catppuccin");

    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn unknown_profile_theme_falls_back_to_mudi_without_blocking_startup() {
    let profile_path = unused_profile_path("unknown-profile-theme");
    std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"neon"}"#).unwrap();

    let ui = Ui::load(&profile_path).unwrap();

    assert_eq!(ui.active_theme_name(), "mudi");
    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn first_use_profile_opens_guide_on_overview() {
    let ui = Ui::load(unused_profile_path("first-use")).unwrap();

    assert!(rendered(&ui).contains("> Overview"));
}

#[test]
fn dismissing_first_use_guide_is_remembered_by_the_profile() {
    let profile_path = unused_profile_path("dismissed-help");
    let mut ui = Ui::load(&profile_path).unwrap();

    ui.handle_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &Snapshot::default(),
    )
    .unwrap();

    let reloaded = Ui::load(&profile_path).unwrap();
    assert!(!rendered(&reloaded).contains("> Overview"));

    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn deleting_help_preference_restores_first_use_guide() {
    let profile_path = unused_profile_path("deleted-help-preference");
    let mut ui = Ui::load(&profile_path).unwrap();
    ui.handle_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &Snapshot::default(),
    )
    .unwrap();
    std::fs::remove_file(&profile_path).unwrap();

    let reloaded = Ui::load(&profile_path).unwrap();

    assert!(rendered(&reloaded).contains("> Overview"));
}

#[test]
fn guide_can_be_reopened_and_closed_without_changing_the_active_page() {
    let profile_path = unused_profile_path("reopened-guide");
    let mut ui = Ui::load(&profile_path).unwrap();
    let snapshot = Snapshot::default();
    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let page_before_guide = rendered(&ui);

    ui.handle_key(
        KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    assert!(rendered(&ui).contains("> Overview"));

    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert_eq!(rendered(&ui), page_before_guide);

    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn header_has_no_numeric_page_navigation_and_number_keys_do_not_change_overview() {
    let snapshot = demo_snapshot();
    let mut ui = Ui::default();
    let before = rendered_with_snapshot(&ui, &snapshot);

    assert!(!before.lines().next().unwrap_or_default().contains("1 2 3"));
    for key in ['1', '2', '3'] {
        ui.handle_key(
            KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        assert_eq!(rendered_with_snapshot(&ui, &snapshot), before);
    }
}

#[test]
fn guide_navigation_explains_essential_controls_without_page_mapping() {
    let mut ui = Ui::load(unused_profile_path("guide-content")).unwrap();
    ui.handle_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &Snapshot::default(),
    )
    .unwrap();
    let mut guide = rendered(&ui);
    ui.handle_key(
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        &Snapshot::default(),
    )
    .unwrap();
    for _ in 0..20 {
        ui.handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &Snapshot::default(),
        )
        .unwrap();
        guide.push_str(&rendered(&ui));
    }

    for expected in [
        "Tab / Shift+Tab",
        "Up/Down or j/k",
        "Enter",
        "View an indexed linked Successor",
        "Exact launched-process evidence",
        "Refresh",
        "?  Guide",
        "Esc",
        "Quit",
    ] {
        assert!(
            guide.contains(expected),
            "Navigation Guide should contain {expected:?}"
        );
    }
    assert!(!guide.contains("Open a page"));
    assert!(!guide.contains("t Theme"));
}

#[test]
fn overview_session_detail_remains_keyboard_accessible() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    let mut ui = Ui::default();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session Detail"));

    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Session Detail"));
}

#[test]
fn global_keys_return_actions_for_the_event_loop() {
    let mut ui = Ui::default();
    let snapshot = Snapshot::default();

    assert_eq!(
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            &snapshot
        )
        .unwrap(),
        UiAction::Rescan
    );
    assert_eq!(
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &snapshot
        )
        .unwrap(),
        UiAction::Quit
    );
    assert_eq!(
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &snapshot,
        )
        .unwrap(),
        UiAction::Quit
    );
}

#[test]
fn session_detail_is_a_centered_modal_above_a_dimmed_origin_page() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    let mut ui = Ui::default();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let terminal = render_terminal_at(&ui, &snapshot, 120, 30);
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    let (modal_x, modal_y) = buffer
        .content()
        .chunks(width)
        .enumerate()
        .find_map(|(y, row)| {
            row.windows("Session Detail".len())
                .position(|cells| {
                    cells.iter().map(|cell| cell.symbol()).collect::<String>() == "Session Detail"
                })
                .map(|x| (x, y))
        })
        .expect("Detail modal title should render");
    let top = &buffer.content()[modal_y * width..(modal_y + 1) * width];
    let left = top[..modal_x]
        .iter()
        .rposition(|cell| cell.symbol() == "┌")
        .expect("Detail modal should have a left border");
    let right = top[modal_x..]
        .iter()
        .position(|cell| cell.symbol() == "┐")
        .map(|offset| modal_x + offset)
        .expect("Detail modal should have a right border");

    assert!(right - left < 101);
    assert!((left + right + 1).abs_diff(width) <= 1);
    assert!(rendered_style_at(&terminal, "PROJECT")
        .add_modifier
        .contains(Modifier::DIM));
    assert!(!rendered_style_at(&terminal, "Session Detail")
        .add_modifier
        .contains(Modifier::DIM));
}

#[test]
fn number_keys_are_no_ops_in_session_detail() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();

    for key in ['1', '2', '3'] {
        let mut ui = Ui::default();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let before = rendered_with_snapshot(&ui, &snapshot);
        assert!(before.contains("Session Detail"));

        ui.handle_key(
            KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        assert_eq!(rendered_with_snapshot(&ui, &snapshot), before);
    }
}

#[test]
fn field_navigation_auto_scrolls_without_changing_the_origin_session() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    snapshot.sessions[0].id = format!("scroll-session-{}", "x".repeat(260));
    snapshot.sessions[0].project = Some(test_project(&"project".repeat(40)));
    snapshot.sessions[0].latest_model = Some("long-model".repeat(30));
    snapshot.sessions[0].compactions = (0..8)
        .map(|index| Compaction {
            at_ms: 1_700_000_000_000 + index * 1_000,
            trigger: Some(if index == 7 {
                "scroll-target-trigger".into()
            } else {
                format!("trigger-{index}")
            }),
            pre_tokens: Some(100_000 + index as u64),
        })
        .collect();
    let mut ui = Ui::default();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

    for _ in 0..100 {
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
    }
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

    for _ in 0..100 {
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
    }
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Claude Code ->"));
}

#[test]
fn enter_is_a_no_op_and_h_still_opens_explicit_handoff_authorization() {
    let snapshot = demo_snapshot();
    let mut ui = Ui::default();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let before = rendered_rows_at(&ui, &snapshot, 120, 30);

    assert_eq!(
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap(),
        UiAction::None
    );
    assert_eq!(rendered_rows_at(&ui, &snapshot, 120, 30), before);

    ui.handle_key(
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Authorize one Handoff Operation?"));
    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session Detail"));
}

#[test]
fn detail_keeps_required_session_evidence_without_reading_source_content() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    snapshot.sessions[0].compactions = vec![Compaction {
        at_ms: 1_700_000_000_000,
        trigger: Some("manual".into()),
        pre_tokens: Some(146_000),
    }];
    let mut ui = Ui::default();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let backend = TestBackend::new(120, 100);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, &snapshot, &ui)).unwrap();
    let detail: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    for expected in [
        "Provider",
        "Claude Code -> claude-opus-4-8",
        "Project",
        "mudi",
        "/Users/x/code/mudi",
        "cwd",
        "Title",
        "-session",
        "Session ID",
        "demo-session",
        "Busy",
        "Current Context",
        "150.0k / 200.0k",
        "75.0%",
        "Transcript Lifetime Usage",
        "Input",
        "Output",
        "Est. API cost",
        "$0.42",
        "Compactions (1)",
        "manual",
        "146.0k",
    ] {
        assert!(
            detail.contains(expected),
            "Detail should contain {expected:?}"
        );
    }
    assert!(!detail.contains("Cache read"));
    assert!(!detail.contains("Cache write"));
}

#[test]
fn full_detail_keeps_provider_when_the_latest_model_is_unavailable() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].latest_model = None;
    let mut ui = Ui::default();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_rows_at(&ui, &snapshot, 120, 80);

    assert!(detail.contains("Provider        Claude Code"));
    assert!(!detail.contains("Latest model"));
    assert!(!detail.contains("->"));
}

#[test]
fn session_detail_current_context_progress_uses_threshold_colors_and_full_overflow_bar() {
    let theme = Ui::default().theme.palette();
    let cases = [
        (699, "69.9%", theme.context_normal),
        (700, "70.0%", theme.context_warning),
        (849, "84.9%", theme.context_warning),
        (850, "85.0%", theme.context_critical),
        (1_000, "100.0%", theme.context_critical),
        (1_125, "112.5%", theme.context_critical),
    ];

    for (tokens, percentage, color) in cases {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].context = ContextState {
            tokens,
            window: Some(1_000),
        };
        let mut ui = Ui::default();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let terminal = render_terminal_at(&ui, &snapshot, 160, 40);
        let rendered = rendered_rows_at(&ui, &snapshot, 160, 40);
        let occupancy = rendered
            .lines()
            .find(|line| line.contains("Occupancy"))
            .expect("Session Detail should render Current Context occupancy");

        assert!(occupancy.contains('['));
        assert!(occupancy.contains(percentage));
        assert_eq!(rendered_style_at(&terminal, percentage).fg, Some(color));
        if tokens >= 1_000 {
            let bar = occupancy
                .split_once('[')
                .and_then(|(_, rest)| rest.split_once(']'))
                .map(|(bar, _)| bar)
                .expect("wide Session Detail should render a progress bar");
            assert!(bar.chars().all(|cell| cell == '#'));
        }
    }
}

#[test]
fn session_detail_context_preserves_unknown_tokens_and_degrades_on_narrow_surfaces() {
    let theme = Ui::default().theme.palette();
    let mut unknown_snapshot = demo_snapshot();
    unknown_snapshot.sessions[0].context = ContextState {
        tokens: 123_456,
        window: None,
    };
    let mut unknown_ui = Ui::default();
    unknown_ui
        .handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &unknown_snapshot,
        )
        .unwrap();
    let unknown_terminal = render_terminal_at(&unknown_ui, &unknown_snapshot, 160, 40);
    let unknown_rendered = rendered_rows_at(&unknown_ui, &unknown_snapshot, 160, 40);
    let unknown_occupancy = unknown_rendered
        .lines()
        .find(|line| line.contains("Occupancy"))
        .expect("Session Detail should retain unknown Current Context tokens");

    assert!(unknown_occupancy.contains("123.5k / N/A"));
    assert!(unknown_occupancy.contains("[--------------------------------]"));
    assert!(unknown_occupancy.contains("(N/A)"));
    assert_eq!(
        rendered_style_at(&unknown_terminal, "(N/A)").fg,
        Some(theme.muted)
    );

    let mut exited_snapshot = demo_snapshot();
    exited_snapshot.sessions[0].state = SessionState::Exited;
    exited_snapshot.sessions[0].context = ContextState {
        tokens: 900,
        window: Some(1_000),
    };
    let mut exited_ui = Ui::default();
    open_first_session_detail(&mut exited_ui, &exited_snapshot);
    let exited_terminal = render_terminal_at(&exited_ui, &exited_snapshot, 160, 40);

    assert_eq!(
        rendered_style_at(&exited_terminal, "90.0%").fg,
        Some(theme.muted)
    );

    let mut narrow_ui = Ui::default();
    narrow_ui
        .handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &demo_snapshot(),
        )
        .unwrap();
    for _ in 0..9 {
        narrow_ui
            .handle_key(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                &demo_snapshot(),
            )
            .unwrap();
    }
    let narrow = rendered_rows_at(&narrow_ui, &demo_snapshot(), 80, 24);
    let context = narrow
        .lines()
        .find(|line| line.contains("Occupancy"))
        .expect("narrow Session Detail should retain Current Context evidence");

    assert!(context.contains("150.0k / 200.0k"));
    assert!(context.contains("75.0%"));
    assert!(!context.contains('['));
}

#[test]
fn open_session_detail_labels_current_run_and_transcript_lifetime_usage() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].current_run_usage = Some(UsageSummary {
        usage: Usage {
            input: 111,
            output: 22,
            cache_read: 33,
            cache_write: 44,
        },
        per_model: vec![ModelUsage {
            model: "run-only-model".into(),
            usage: Usage {
                input: 111,
                output: 22,
                cache_read: 33,
                cache_write: 44,
            },
            cost: Some(0.07),
        }],
        cost: Some(0.07),
    });
    let mut ui = Ui::default();
    open_first_session_detail(&mut ui, &snapshot);

    let detail = rendered_rows_at(&ui, &snapshot, 160, 80);

    for expected in [
        "Current Run Usage",
        "Current Run per-model Usage",
        "run-only-model",
        "$0.07",
        "Transcript Lifetime Usage",
        "Transcript Lifetime per-model Usage",
        "$0.42",
    ] {
        assert!(
            detail.contains(expected),
            "Detail should contain {expected:?}"
        );
    }
}

#[test]
fn open_session_without_a_run_boundary_renders_current_run_usage_as_na() {
    let snapshot = demo_snapshot();
    let mut ui = Ui::default();
    open_first_session_detail(&mut ui, &snapshot);

    let detail = rendered_rows_at(&ui, &snapshot, 160, 40);

    assert!(detail.contains("Current Run Usage"));
    assert!(detail.contains("Usage           N/A"));
    assert!(!detail.contains("Current Run per-model Usage"));
}

#[test]
fn exited_session_detail_shows_lifetime_usage_without_current_or_last_run_usage() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].state = SessionState::Exited;
    snapshot.sessions[0].current_run_usage = Some(UsageSummary {
        usage: Usage {
            input: 111,
            ..Usage::default()
        },
        per_model: vec![],
        cost: None,
    });
    let mut ui = Ui::default();
    open_first_session_detail(&mut ui, &snapshot);

    let detail = rendered_rows_at(&ui, &snapshot, 120, 80);

    assert!(detail.contains("Transcript Lifetime Usage"));
    assert!(!detail.contains("Current Run per-model Usage"));
    assert!(!detail.contains("Last Run Usage"));
}

#[test]
fn project_detail_uses_the_exact_working_directory_without_git_fields() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    snapshot.sessions[0].project = Some(Project {
        identity: ProjectIdentity::new(PathBuf::from("/Users/x/Documents/research-notes")),
        label: "research-notes".into(),
    });
    snapshot.sessions[0].cwd = Some("/Users/x/Documents/research-notes".into());
    let mut ui = Ui::default();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_rows_at(&ui, &snapshot, 120, 80);

    assert!(detail.contains("research-notes"));
    assert!(detail.contains("/Users/x/Documents/research-notes"));
    assert!(!detail.contains("Branch"));
}

#[test]
fn unresolved_detail_preserves_cwd_and_marks_identity_unavailable() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    snapshot.sessions[0].project = None;
    snapshot.sessions[0].cwd = Some("/missing/session-cwd".into());
    let mut ui = Ui::default();
    open_first_session_detail(&mut ui, &snapshot);

    let detail = rendered_with_snapshot(&ui, &snapshot);

    assert!(detail.contains("N/A (identity unavailable)"));
    assert!(detail.contains("/missing/session-cwd"));
}

#[test]
fn renders_overview_and_detail() {
    render_overview(false);
    render_overview(true);
}
