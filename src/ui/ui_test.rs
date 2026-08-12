//! UI 单元测试覆盖状态转换、响应式布局、语义颜色和稳定身份导航。
//!
//! 测试函数名描述预期行为；下面的辅助构造负责复用快照与 TestBackend，
//! 各测试中的断言则直接对应用户可观察的文字、位置或样式证据。

use super::*;
use crate::alerts::AlertCounts;
use crate::attention::AttentionEngine;
use crate::domain::{
    Compaction, ContextState, ModelUsage, Project, ProjectIdentity, Provider, Session,
    SessionState, Usage, UsageSummary, Worktree,
};
use crate::signals::{ContextAlertLevel, RecentSignal, RecentSignalKind, SharedWorktreeRiskChange};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 构造具有稳定测试路径的 Project。
fn test_project(label: &str) -> Project {
    Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from(format!("/test/{label}"))),
        label: label.into(),
    }
}

/// 返回不会与并行测试冲突的临时 Profile 路径。
fn unused_profile_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "sessionmesh-{name}-{}-{nonce}-profile.json",
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
fn operation_task(state: crate::tasks::OperationTaskState) -> OperationTask {
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
        crate::tasks::OperationTaskState::Queued,
        crate::tasks::OperationTaskState::Waiting,
        crate::tasks::OperationTaskState::Running,
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
    let task = operation_task(crate::tasks::OperationTaskState::Running);
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
fn declining_irreversible_task_deletion_preserves_the_terminal_task() {
    let mut task = operation_task(crate::tasks::OperationTaskState::Ready);
    let artifact_path = PathBuf::from("/private/artifacts/task-ready/handoff.md");
    task.artifact_path = Some(artifact_path.clone());
    task.successor_launch_plan = Some(crate::tasks::SuccessorLaunchPlan {
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
    let task = operation_task(crate::tasks::OperationTaskState::Cancelled);
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
    let mut task = operation_task(crate::tasks::OperationTaskState::Ready);
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
    let mut retry = operation_task(crate::tasks::OperationTaskState::Failed);
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
        crate::tasks::OperationTaskState::Ready,
        crate::tasks::OperationTaskState::Failed,
        crate::tasks::OperationTaskState::Interrupted,
        crate::tasks::OperationTaskState::Cancelled,
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
        crate::tasks::OperationTaskState::Queued,
        crate::tasks::OperationTaskState::Waiting,
        crate::tasks::OperationTaskState::Running,
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
        crate::tasks::OperationTaskState::Failed,
        crate::tasks::OperationTaskState::Cancelled,
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

    let mut interrupted = operation_task(crate::tasks::OperationTaskState::Interrupted);
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

    snapshot.tasks = vec![operation_task(crate::tasks::OperationTaskState::Ready)];
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("r Retry"));
}

#[test]
fn retry_requires_fresh_confirmation_before_emitting_an_action() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].state = SessionState::Exited;
    let mut task = operation_task(crate::tasks::OperationTaskState::Failed);
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
    let mut task = operation_task(crate::tasks::OperationTaskState::Failed);
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
    let mut task = operation_task(crate::tasks::OperationTaskState::Ready);
    let artifact_path = PathBuf::from("/private/artifacts/task/handoff.md");
    task.artifact_path = Some(artifact_path.clone());
    task.successor_launch_plan = Some(crate::tasks::SuccessorLaunchPlan {
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
    let mut task = operation_task(crate::tasks::OperationTaskState::Ready);
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
        crate::tasks::OperationTaskState::Queued,
        crate::tasks::OperationTaskState::Waiting,
        crate::tasks::OperationTaskState::Running,
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
            identity: ProjectIdentity::GitCommonDir(PathBuf::from(
                "/Users/x/code/sessionmesh/.git",
            )),
            label: "sessionmesh".into(),
        }),
        worktree: Some(Worktree {
            root: PathBuf::from("/Users/x/code/sessionmesh"),
            branch: Some("main".into()),
            writable: true,
        }),
        cwd: Some("/Users/x/code/sessionmesh".into()),
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
    feature.worktree = Some(Worktree {
        root: PathBuf::from("/Users/x/code/sessionmesh-feature"),
        branch: Some("feature".into()),
        writable: true,
    });
    feature.cwd = Some("/Users/x/code/sessionmesh-feature".into());
    feature.last_activity_ms = 1_000;
    snapshot.sessions.push(feature);
    snapshot
}

#[test]
fn project_context_new_defaults_to_main_and_emits_the_switched_worktree() {
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
    assert!(initial.contains("New Session"));
    assert!(initial.contains("/Users/x/code/sessionmesh"));
    assert!(initial.contains("Choose Provider"));
    assert!(initial.contains("[ ] Off"));
    let compact = rendered_rows_at(&ui, &snapshot, 80, 24);
    for visible_control in [
        "New Session",
        "Provider",
        "Worktree",
        "Title",
        "Description",
        "Binding Initializer",
        "Start New",
    ] {
        assert!(compact.contains(visible_control));
    }

    ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    for character in "Investigate startup".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    for character in "Trace launch".chars() {
        ui.handle_key(key(KeyCode::Char(character)), &snapshot)
            .unwrap();
    }
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char(' ')), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::StartManagedNew {
            request: crate::managed_sessions::ManagedNewRequest {
                provider: Provider::CodexCli,
                target: crate::managed_sessions::ManagedLaunchTarget {
                    project: snapshot.sessions[0]
                        .project
                        .as_ref()
                        .unwrap()
                        .identity
                        .clone(),
                    project_label: "sessionmesh".into(),
                    worktree_root: Some(PathBuf::from("/Users/x/code/sessionmesh-feature")),
                    working_directory: PathBuf::from("/Users/x/code/sessionmesh-feature"),
                },
                title: "Investigate startup".into(),
                description: Some("Trace launch".into()),
                binding_initializer: true,
            }
        }
    );
}

#[test]
fn selected_session_new_inherits_its_worktree_context() {
    let snapshot = managed_new_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);

    assert_eq!(
        ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap(),
        UiAction::None
    );
    let form = ui.new_session.as_ref().unwrap();
    assert!(!form.directory_context);
    assert_eq!(
        form.targets[form.target_index].working_directory,
        PathBuf::from("/Users/x/code/sessionmesh")
    );
}

#[test]
fn new_without_project_tree_context_opens_the_form_before_the_directory_picker() {
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
    let form = ui.new_session.as_ref().unwrap();
    assert!(form.directory_context);
    assert!(form.targets.is_empty());

    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::ChooseManagedNewDirectory
    );
}

#[test]
fn repeated_new_shortcuts_never_request_the_directory_picker() {
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let key = || KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);

    for _ in 0..3 {
        assert_eq!(ui.handle_key(key(), &snapshot).unwrap(), UiAction::None);
    }
    assert!(ui.new_session.is_some());
}

#[test]
fn directory_context_can_be_reselected_before_new_is_confirmed() {
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.finish_managed_new_directory_selection(Ok(Some(
        crate::managed_sessions::ManagedLaunchTarget {
            project: ProjectIdentity::CanonicalCwd(PathBuf::from("/Users/x/code/notes")),
            project_label: "notes".into(),
            worktree_root: None,
            working_directory: PathBuf::from("/Users/x/code/notes"),
        },
    )));
    ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();

    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::ChooseManagedNewDirectory
    );

    ui.finish_managed_new_directory_selection(Err("ambiguous Git identity".into()));
    let rendered = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(rendered.contains("Choose Directory"));
    assert!(rendered.contains("ambiguous Git identity"));
    assert!(!rendered.contains("/Users/x/code/notes"));
}

#[test]
fn selected_worktree_new_requires_title_and_does_not_remember_provider() {
    let snapshot = managed_new_snapshot();
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    ui.clamp_selections(&snapshot);
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();

    let selected = rendered_rows_at(&ui, &snapshot, 120, 36);
    assert!(selected.contains("/Users/x/code/sessionmesh-feature"));
    assert!(selected.contains("Choose Provider"));
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    for _ in 0..5 {
        ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
    }
    assert_eq!(
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
        UiAction::None
    );
    assert!(rendered_rows_at(&ui, &snapshot, 120, 36).contains("Title is required"));

    ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Char('n')), &snapshot).unwrap();
    assert!(rendered_rows_at(&ui, &snapshot, 120, 36).contains("Choose Provider"));
}

/// 对指定顶层页签及可选详情状态执行一次无崩溃渲染。
fn render_tab(tab: Tab, detail: bool) {
    let snap = demo_snapshot();
    let mut ui = Ui {
        tab,
        selected: 0,
        ..Ui::default()
    };
    if detail {
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snap)
            .unwrap();
    }
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    // 无论当前状态如何，布局计算和绘制过程都不能崩溃。
    terminal.draw(|f| draw(f, &snap, &ui)).unwrap();
}

#[test]
fn t_key_does_not_change_the_active_theme_or_show_a_notice() {
    let mut ui = Ui::default();
    let snapshot = Snapshot {
        ..Snapshot::default()
    };

    assert_eq!(ui.active_theme_name(), "sessionmesh");
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();

    assert_eq!(ui.active_theme_name(), "sessionmesh");
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Theme:"));
}

#[test]
fn each_theme_supplies_the_selected_page_structural_role() {
    for (name, selected_background) in [
        ("sessionmesh", Color::Rgb(32, 35, 38)),
        ("catppuccin", Color::Rgb(35, 36, 52)),
        ("dracula", Color::Rgb(34, 38, 62)),
        ("gruvbox", Color::Rgb(47, 36, 29)),
        ("light", Color::Rgb(230, 233, 235)),
        ("high-contrast", Color::Rgb(20, 20, 20)),
    ] {
        let profile_path = unused_profile_path(name);
        std::fs::write(
            &profile_path,
            format!(r#"{{"help_seen":true,"theme":"{name}"}}"#),
        )
        .unwrap();
        let ui = Ui::load(&profile_path, None).unwrap();
        let terminal = render_terminal(&ui);
        assert_eq!(
            rendered_style_at(&terminal, "1 2 3").bg,
            Some(selected_background)
        );
        let _ = std::fs::remove_file(profile_path);
    }
}

#[test]
fn sessionmesh_uses_semantic_busy_and_critical_context_roles() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].context.tokens = 180_000;
    let now = Local::now().timestamp_millis();
    snapshot.sessions[0].last_activity_ms = now;
    let mut selected = snapshot.sessions[0].clone();
    selected.id = "selected-session".into();
    selected.project = Some(test_project("selected-project"));
    selected.context.tokens = 50_000;
    selected.last_activity_ms = now - 1_000;
    snapshot.sessions.push(selected);
    let ui = Ui {
        tab: Tab::Claude,
        selected: 1,
        ..Ui::default()
    };

    let terminal = render_terminal_with_snapshot(&ui, &snapshot);

    assert_eq!(
        rendered_style_at(&terminal, "Busy").fg,
        Some(Color::Rgb(120, 205, 120))
    );
    assert_eq!(
        rendered_style_at(&terminal, "180.0k (90%)").fg,
        Some(Color::Rgb(255, 95, 109))
    );
}

#[test]
fn sessionmesh_uses_semantic_idle_exited_and_warning_roles() {
    let mut snapshot = demo_snapshot();
    let now = Local::now().timestamp_millis();
    snapshot.sessions[0].last_activity_ms = now;
    snapshot.sessions[0].context.tokens = 50_000;

    let mut idle = snapshot.sessions[0].clone();
    idle.id = "idle-session".into();
    idle.project = Some(test_project("idle-project"));
    idle.state = SessionState::Idle;
    idle.context.tokens = 150_000;
    idle.last_activity_ms = now - 1_000;

    let mut exited = snapshot.sessions[0].clone();
    exited.id = "exited-session".into();
    exited.project = Some(test_project("exited-project"));
    exited.state = SessionState::Exited;
    exited.context.tokens = 50_000;
    exited.last_activity_ms = now - 2_000;
    snapshot.sessions.extend([idle, exited]);

    let ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
    let terminal = render_terminal_with_snapshot(&ui, &snapshot);

    assert_eq!(
        rendered_style_at(&terminal, "Idle").fg,
        Some(Color::Rgb(105, 184, 220))
    );
    assert_eq!(
        rendered_style_at(&terminal, "150.0k (75%)").fg,
        Some(Color::Rgb(238, 190, 70))
    );
    assert_eq!(
        rendered_style_at(&terminal, "· —").fg,
        Some(Color::Rgb(112, 116, 120))
    );
}

#[test]
fn light_theme_supplies_background_border_accent_and_muted_roles() {
    let profile_path = unused_profile_path("light-roles");
    std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();
    let snapshot = Snapshot::default();
    ui.tab = Tab::Claude;

    let terminal = render_terminal_with_snapshot(&ui, &snapshot);
    let top_left = &terminal.backend().buffer()[(0, 0)];

    assert_eq!(top_left.style().bg, Some(Color::Rgb(250, 249, 246)));
    assert_eq!(top_left.style().fg, Some(Color::Rgb(126, 130, 133)));
    assert_eq!(
        rendered_style_at(&terminal, "SessionMesh").fg,
        Some(Color::Rgb(25, 91, 130))
    );
    assert_eq!(
        rendered_style_at(&terminal, "Indexing… sessions will appear shortly.").fg,
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
    let project = page.find("▾ sessionmesh").unwrap();
    let claude = page.find("✳ -session").unwrap();
    let codex = page.find("⬡ dex-open").unwrap();

    assert!(project < claude && claude < codex);
    assert_eq!(
        page.lines()
            .filter(|line| line.contains("▾ sessionmesh"))
            .count(),
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
fn now_sorts_projects_by_activity_and_their_sessions_by_attention() {
    let mut healthy = demo_snapshot().sessions.remove(0);
    healthy.id = "healthy".into();
    healthy.project = Some(test_project("healthy"));
    healthy.worktree.as_mut().unwrap().root = PathBuf::from("/worktrees/healthy");
    healthy.latest_model = Some("healthy-model".into());
    healthy.context.tokens = 80_000;
    healthy.last_activity_ms = 2_000;

    let mut warning = healthy.clone();
    warning.id = "warning".into();
    warning.project = Some(test_project("priority"));
    warning.worktree.as_mut().unwrap().root = PathBuf::from("/worktrees/a-warning");
    warning.latest_model = Some("warning-model".into());
    warning.context.tokens = 150_000;
    warning.last_activity_ms = 1_000;

    let mut shared = warning.clone();
    shared.provider = Provider::CodexCli;
    shared.id = "shared".into();
    shared.worktree.as_mut().unwrap().root = PathBuf::from("/worktrees/b-critical");
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
    assert_eq!(session_ids, ["healthy", "critical", "shared", "warning"]);

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    assert!(page.contains("✳ critical"));
    assert!(page.contains("⬡ shared"));
    assert!(page.contains("✳ warning"));
}

#[test]
fn now_shows_shared_worktree_risk_and_every_contributing_session() {
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
    assert!(critical.contains('●'));
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
    idle.worktree = Some(Worktree {
        root: PathBuf::from("/Users/x/code/idle-warning"),
        branch: Some("main".into()),
        writable: true,
    });
    idle.cwd = Some("/Users/x/code/idle-warning".into());
    idle.state = SessionState::Idle;
    snapshot.sessions.push(idle);
    let ui = Ui {
        selected: usize::MAX,
        ..Ui::default()
    };

    let terminal = render_terminal_at(&ui, &snapshot, 120, 30);
    let busy = rendered_style_at(&terminal, "●");
    let idle = rendered_style_at(&terminal, "○");
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
    fallback.worktree.as_mut().unwrap().root = PathBuf::from("/test/fallback");
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

    assert!(observed.contains('○'));
    assert!(observed.contains("ved-idle"));
    assert!(fallback.contains('○'));
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
    assert!(collapsed_project.contains(">▸ sessionmesh (2)"));
    assert!(!collapsed_project.contains("✳ -session"));
    assert!(!collapsed_project.contains("⬡ ond-open"));
    assert!(ui.selected_session(&snapshot).is_none());

    ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
    assert_eq!(ui.selected_session(&snapshot).unwrap().id, "demo-session");

    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Left), &snapshot).unwrap();
    let collapsed_again = rendered_rows_at(&ui, &snapshot, 120, 30);
    assert!(collapsed_again.contains(">▸ sessionmesh (2)"));
    assert!(!collapsed_again.contains("/Users/x/code/sessionmesh"));
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
        ui.edit_session.as_ref().map(|editor| &editor.session),
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
        ui.edit_session.as_ref().map(|editor| &editor.session),
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
fn now_flattens_a_single_worktree_and_moves_its_location_to_details() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].worktree.as_mut().unwrap().branch = Some("feature/shared".into());
    let ui = Ui::default();

    let page = rendered_rows_at(&ui, &snapshot, 120, 30);
    let project = page
        .lines()
        .find(|line| line.contains("▾ sessionmesh"))
        .unwrap();

    assert!(project.contains("(1)"));
    assert!(!page.contains("▾ feature/shared"));
    assert!(page.contains("Worktree"));
    assert!(page.contains("@ feature/shared"));
    assert!(!project.contains("Context"));
}

#[test]
fn now_keeps_multiple_worktrees_below_the_project_without_context() {
    let mut snapshot = demo_snapshot();
    let mut feature = snapshot.sessions[0].clone();
    feature.id = "feature-session".into();
    feature.worktree = Some(Worktree {
        root: PathBuf::from("/Users/x/worktrees/sessionmesh-feature"),
        branch: Some("feature".into()),
        writable: true,
    });
    feature.cwd = Some("/Users/x/worktrees/sessionmesh-feature".into());
    snapshot.sessions.push(feature);

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
    let mut lines = page.lines();
    let project = lines.find(|line| line.contains("▾ sessionmesh")).unwrap();
    let main = lines.find(|line| line.contains("▾ main")).unwrap();
    let feature = lines.find(|line| line.contains("▾ feature")).unwrap();

    assert!(project.contains("(2)"));
    assert!(main.starts_with("  "));
    assert!(feature.starts_with("  "));
    assert!(!main.starts_with('│'));
    assert!(!feature.starts_with('│'));
    assert!(!main.contains("/Users"));
    assert!(!feature.contains("/Users"));
    assert!(!main.contains("Context"));
    assert!(!feature.contains("Context"));
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
fn shared_worktree_signal_renders_canonical_root_and_membership_evidence() {
    let mut snapshot = demo_snapshot();
    snapshot.recent_signals = vec![RecentSignal::Worktree {
        observed_at_ms: 2_000,
        worktree_root: "/wt/shared".into(),
        change: SharedWorktreeRiskChange::Detected,
        before: vec![crate::attention::SessionIdentity {
            provider: Provider::ClaudeCode,
            native_id: "claude-a".into(),
        }],
        after: vec![
            crate::attention::SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "claude-a".into(),
            },
            crate::attention::SessionIdentity {
                provider: Provider::CodexCli,
                native_id: "codex-b".into(),
            },
        ],
    }];

    let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);

    for evidence in [
        "/wt/shared",
        "Risk detected",
        "Before 1: ✳:claude-a",
        "After 2: ✳:claude-a, ⬡:codex-b",
    ] {
        assert!(page.contains(evidence), "missing evidence {evidence:?}");
    }
    assert!(!page.to_lowercase().contains("conflict"));
}

#[test]
fn worktree_signal_locates_the_recommended_now_session_not_its_first_member() {
    let mut snapshot = demo_snapshot();
    let worktree_root = PathBuf::from("/wt/shared");
    let healthy = &mut snapshot.sessions[0];
    healthy.id = "arbitrary-first".into();
    healthy.latest_model = Some("arbitrary-model".into());
    healthy.context.tokens = 80_000;
    healthy.worktree.as_mut().unwrap().root = worktree_root.clone();

    let mut critical = healthy.clone();
    critical.provider = Provider::CodexCli;
    critical.id = "recommended".into();
    critical.latest_model = Some("recommended-model".into());
    critical.context.tokens = 180_000;
    snapshot.sessions.push(critical);
    snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));
    snapshot.recent_signals = vec![RecentSignal::Worktree {
        observed_at_ms: 2_000,
        worktree_root,
        change: SharedWorktreeRiskChange::Detected,
        before: vec![crate::attention::SessionIdentity {
            provider: Provider::ClaudeCode,
            native_id: "arbitrary-first".into(),
        }],
        after: vec![
            crate::attention::SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "arbitrary-first".into(),
            },
            crate::attention::SessionIdentity {
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

    assert_eq!(ui.active_tab(), Tab::All);
    let cursor = now_cursor_style_at(&located, "ommended").unwrap();
    assert_eq!(cursor.fg, Some(Color::Rgb(230, 180, 80)));
    assert_eq!(now_cursor_style_at(&located, "ry-first"), None);
    assert_eq!(
        rendered_style_at(&located, "Risk detected").bg,
        Some(ThemeName::SessionMesh.palette().selected_background)
    );
}

#[test]
fn cleared_worktree_signal_stays_selected_when_the_worktree_is_no_longer_open() {
    let mut snapshot = demo_snapshot();
    let worktree_root = PathBuf::from("/wt/closed");
    snapshot.sessions[0].state = SessionState::Exited;
    snapshot.sessions[0].worktree.as_mut().unwrap().root = worktree_root.clone();
    snapshot.recent_signals = vec![RecentSignal::Worktree {
        observed_at_ms: 2_000,
        worktree_root,
        change: SharedWorktreeRiskChange::Cleared,
        before: vec![
            crate::attention::SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "claude-a".into(),
            },
            crate::attention::SessionIdentity {
                provider: Provider::CodexCli,
                native_id: "codex-b".into(),
            },
        ],
        after: vec![],
    }];
    let selected_background = Some(ThemeName::SessionMesh.palette().selected_background);
    let mut ui = Ui::default();

    ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let unchanged = render_terminal_at(&ui, &snapshot, 120, 30);
    let page = rendered_rows_at(&ui, &snapshot, 120, 30);

    assert_eq!(ui.active_tab(), Tab::All);
    assert_eq!(
        rendered_style_at(&unchanged, "Risk cleared").bg,
        selected_background
    );
    assert!(page.contains("Worktree is no longer open"));
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
    let selected_background = Some(ThemeName::SessionMesh.palette().selected_background);
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
    let selected_background = Some(ThemeName::SessionMesh.palette().selected_background);
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

    assert!(session.contains('●'));
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
    snapshot.sessions[0].worktree = Some(Worktree {
        root: PathBuf::from("/Users/x/code/sessionmesh-worktrees/attention-layout-prototype"),
        branch: Some("prototype/q-007-responsive-terminal-layout".into()),
        writable: true,
    });
    snapshot.sessions[0].latest_model =
        Some("claude-sonnet-with-an-intentionally-long-model-name".into());
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    let ui = Ui::default();

    let page = rendered_rows_at(&ui, &snapshot, 80, 24);
    let session = page
        .lines()
        .find(|line| line.contains("✳ -session"))
        .unwrap();

    assert!(page.contains("Worktree"));
    assert!(page.contains("Provider"));
    for evidence in ["!", "●", "✳", "-session", "75%"] {
        assert!(
            session.contains(evidence),
            "missing compact evidence: {evidence}"
        );
    }
}

#[test]
fn changing_theme_does_not_change_rendered_product_semantics() {
    let sessionmesh_path = unused_profile_path("sessionmesh-semantics");
    let light_path = unused_profile_path("light-semantics");
    std::fs::write(
        &sessionmesh_path,
        br#"{"help_seen":true,"theme":"sessionmesh"}"#,
    )
    .unwrap();
    std::fs::write(&light_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
    let mut sessionmesh = Ui::load(&sessionmesh_path, None).unwrap();
    let mut light = Ui::load(&light_path, None).unwrap();
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    snapshot.alerts = AlertCounts {
        warning: 3,
        critical: 2,
    };

    assert_eq!(
        rendered_with_snapshot(&sessionmesh, &snapshot),
        rendered_with_snapshot(&light, &snapshot)
    );

    sessionmesh.tab = Tab::Claude;
    light.tab = Tab::Claude;
    assert_eq!(
        rendered_with_snapshot(&sessionmesh, &snapshot),
        rendered_with_snapshot(&light, &snapshot)
    );

    let _ = std::fs::remove_file(sessionmesh_path);
    let _ = std::fs::remove_file(light_path);
}

#[test]
fn provider_inventory_and_detail_use_theme_structural_roles() {
    let mut snapshot = demo_snapshot();
    let now = Local::now().timestamp_millis();
    snapshot.sessions[0].last_activity_ms = now;
    let mut secondary = snapshot.sessions[0].clone();
    secondary.id = "secondary-session".into();
    secondary.project = Some(test_project("second"));
    secondary.latest_model = Some("model-two".into());
    secondary.state = SessionState::Exited;
    secondary.last_activity_ms = now - 1_000;
    snapshot.sessions.push(secondary);
    let profile_path = unused_profile_path("light-provider-roles");
    std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();
    ui.tab = Tab::Claude;

    let inventory = render_terminal_with_snapshot(&ui, &snapshot);
    assert_eq!(
        rendered_style_at(&inventory, "State").fg,
        Some(Color::Rgb(25, 91, 130))
    );
    assert_eq!(
        rendered_style_at(&inventory, "second").fg,
        Some(Color::Rgb(25, 91, 130))
    );
    assert_eq!(
        rendered_style_at(&inventory, "model-two").fg,
        Some(Color::Rgb(96, 101, 106))
    );
    assert_eq!(
        inventory.backend().buffer()[(0, 3)].style().fg,
        Some(Color::Rgb(126, 130, 133))
    );

    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let detail = render_terminal_with_snapshot(&ui, &snapshot);
    assert_eq!(
        rendered_style_at(&detail, "Attention Conditions").fg,
        Some(Color::Rgb(25, 91, 130))
    );
    assert_eq!(
        rendered_style_at(&detail, "Provider").fg,
        Some(Color::Rgb(96, 101, 106))
    );
    assert_eq!(
        rendered_style_at(&detail, "demo-session").fg,
        Some(Color::Rgb(28, 31, 34))
    );
    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn guide_uses_the_active_theme_roles() {
    let profile_path = unused_profile_path("themed-guide");
    let ui = Ui::load(&profile_path, Some("light")).unwrap();

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
    let mut ui = Ui::load(&profile_path, None).unwrap();
    ui.tab = Tab::Claude;
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

    let reloaded = Ui::load(&profile_path, None).unwrap();
    assert_eq!(reloaded.active_theme_name(), "catppuccin");

    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn process_theme_override_takes_precedence_without_changing_the_profile() {
    let profile_path = unused_profile_path("theme-override");
    std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"catppuccin"}"#).unwrap();

    let overridden = Ui::load(&profile_path, Some("dracula")).unwrap();
    assert_eq!(overridden.active_theme_name(), "dracula");

    let reloaded = Ui::load(&profile_path, None).unwrap();
    assert_eq!(reloaded.active_theme_name(), "catppuccin");

    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn unknown_process_theme_falls_back_without_changing_the_profile() {
    let profile_path = unused_profile_path("unknown-theme-override");
    std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"catppuccin"}"#).unwrap();

    let overridden = Ui::load(&profile_path, Some("neon")).unwrap();
    assert_eq!(overridden.active_theme_name(), "sessionmesh");

    let reloaded = Ui::load(&profile_path, None).unwrap();
    assert_eq!(reloaded.active_theme_name(), "catppuccin");

    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn unknown_profile_theme_falls_back_to_sessionmesh_without_blocking_startup() {
    let profile_path = unused_profile_path("unknown-profile-theme");
    std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"neon"}"#).unwrap();

    let ui = Ui::load(&profile_path, None).unwrap();

    assert_eq!(ui.active_theme_name(), "sessionmesh");
    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn first_use_profile_opens_guide_on_overview() {
    let ui = Ui::load(unused_profile_path("first-use"), None).unwrap();

    assert!(rendered(&ui).contains("> Overview"));
}

#[test]
fn dismissing_first_use_guide_is_remembered_by_the_profile() {
    let profile_path = unused_profile_path("dismissed-help");
    let mut ui = Ui::load(&profile_path, None).unwrap();

    ui.handle_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &Snapshot::default(),
    )
    .unwrap();

    let reloaded = Ui::load(&profile_path, None).unwrap();
    assert!(!rendered(&reloaded).contains("> Overview"));

    let _ = std::fs::remove_file(profile_path);
}

#[test]
fn deleting_help_preference_restores_first_use_guide() {
    let profile_path = unused_profile_path("deleted-help-preference");
    let mut ui = Ui::load(&profile_path, None).unwrap();
    ui.handle_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &Snapshot::default(),
    )
    .unwrap();
    std::fs::remove_file(&profile_path).unwrap();

    let reloaded = Ui::load(&profile_path, None).unwrap();

    assert!(rendered(&reloaded).contains("> Overview"));
}

#[test]
fn guide_can_be_reopened_and_closed_without_changing_the_active_page() {
    let profile_path = unused_profile_path("reopened-guide");
    let mut ui = Ui::load(&profile_path, None).unwrap();
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
fn guide_prevents_page_navigation_from_changing_the_underlying_page() {
    let mut ui = Ui::load(unused_profile_path("numeric-pages"), None).unwrap();
    let snapshot = Snapshot::default();

    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    assert_eq!(ui.active_tab(), Tab::All);

    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    assert_eq!(ui.active_tab(), Tab::Claude);
}

#[test]
fn tab_and_shift_tab_are_no_ops_on_a_single_region_page() {
    let mut ui = Ui::default();
    let snapshot = Snapshot::default();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    let page_before_focus_keys = rendered(&ui);

    ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        &snapshot,
    )
    .unwrap();

    assert_eq!(rendered(&ui), page_before_focus_keys);
}

#[test]
fn persistent_navigation_is_numeric_and_highlights_the_active_page() {
    let snapshot = Snapshot::default();
    let mut ui = Ui::default();
    let selected_background = Some(ThemeName::SessionMesh.palette().selected_background);
    let terminal = render_terminal(&ui);
    let navigation_cells = &terminal.backend().buffer().content()[..80];
    let navigation: String = navigation_cells.iter().map(|cell| cell.symbol()).collect();

    assert_eq!(navigation.matches("1 2 3").count(), 1);
    for page_name in ["All", "Claude Code", "Codex CLI"] {
        assert!(!navigation.contains(page_name));
    }
    assert_eq!(
        rendered_style_at(&terminal, "1 2 3").bg,
        selected_background
    );
    assert_ne!(rendered_style_at(&terminal, "2 3").bg, selected_background);

    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    let terminal = render_terminal(&ui);
    assert_eq!(rendered_style_at(&terminal, "2 3").bg, selected_background);
}

#[test]
fn guide_navigation_explains_page_mapping_and_essential_controls() {
    let mut ui = Ui::load(unused_profile_path("guide-content"), None).unwrap();
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
        "1 / 2 / 3",
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
    assert!(!guide.contains("t Theme"));
}

#[test]
fn provider_session_detail_remains_keyboard_accessible() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    let mut ui = Ui::default();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();

    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session Detail"));

    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Session Detail"));
}

#[test]
fn provider_page_groups_its_sessions_by_project_in_newest_project_order() {
    let now = Local::now().timestamp_millis();
    let mut snapshot = demo_snapshot();
    snapshot.generated_ms = now;
    snapshot.sessions[0].state = SessionState::Exited;
    snapshot.sessions[0].id = "newest-project-recent".into();
    snapshot.sessions[0].project = Some(test_project("newest-project"));
    snapshot.sessions[0].latest_model = Some("new-project-model".into());
    snapshot.sessions[0].last_activity_ms = now - 1_000;

    let mut older_in_same_project = snapshot.sessions[0].clone();
    older_in_same_project.id = "newest-project-second".into();
    older_in_same_project.latest_model = Some("same-project-model".into());
    older_in_same_project.last_activity_ms = now - 3_000;

    let mut older_project = snapshot.sessions[0].clone();
    older_project.id = "older-project-session".into();
    older_project.project = Some(test_project("older-project"));
    older_project.latest_model = Some("older-model".into());
    older_project.last_activity_ms = now - 2_000;

    let mut other_provider = snapshot.sessions[0].clone();
    other_provider.provider = Provider::CodexCli;
    other_provider.id = "codex-must-not-render".into();
    other_provider.project = Some(test_project("codex-project"));
    other_provider.latest_model = Some("codex-must-not-render".into());

    snapshot
        .sessions
        .extend([older_in_same_project, older_project, other_provider]);
    let ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };

    let page = rendered_rows_at(&ui, &snapshot, 120, 30);
    let newest_header = page.find("PROJECT  newest-project").unwrap();
    let older_header = page.find("PROJECT  older-project").unwrap();

    assert!(newest_header < older_header);
    assert_eq!(page.matches("PROJECT  newest-project").count(), 1);
    assert!(page.contains("new-project-model"));
    assert!(page.contains("same-project-model"));
    assert!(page.contains("older-model"));
    assert!(!page.contains("codex-must-not-render"));
}

#[test]
fn provider_project_folds_recent_sessions_after_five_and_enter_expands() {
    let now = Local::now().timestamp_millis();
    let mut snapshot = demo_snapshot();
    snapshot.generated_ms = now;
    snapshot.sessions = (0..7)
        .map(|position| {
            let mut session = snapshot.sessions[0].clone();
            session.id = format!("recent-{position}");
            session.latest_model = Some(format!("recent-model-{position}"));
            session.last_activity_ms = now - position * 1_000;
            session
        })
        .collect();
    let mut ui = Ui {
        tab: Tab::Claude,
        selected: 5,
        ..Ui::default()
    };

    let collapsed = rendered_rows_at(&ui, &snapshot, 120, 30);
    for visible in 0..5 {
        assert!(collapsed.contains(&format!("recent-model-{visible}")));
    }
    assert!(!collapsed.contains("recent-model-5"));
    assert!(!collapsed.contains("recent-model-6"));
    assert!(collapsed.contains("+ MORE SESSIONS (2)"));

    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let expanded = rendered_rows_at(&ui, &snapshot, 120, 30);

    assert!(expanded.contains("recent-model-5"));
    assert!(expanded.contains("recent-model-6"));
    assert!(expanded.contains("- MORE SESSIONS (2)"));
}

#[test]
fn provider_page_collapses_unavailable_cwds_and_hides_only_system_temp_sessions() {
    let now = Local::now().timestamp_millis();
    let mut snapshot = demo_snapshot();
    snapshot.generated_ms = now;
    snapshot.sessions[0].last_activity_ms = now;

    let mut real_private = snapshot.sessions[0].clone();
    real_private.id = "real-private".into();
    real_private.project = Some(test_project("real-private"));
    real_private.cwd = Some("/private/repos/real-project".into());
    real_private.latest_model = Some("real-private-model".into());
    real_private.state = SessionState::Exited;
    real_private.last_activity_ms = now - 1_000;

    let mut missing = snapshot.sessions[0].clone();
    missing.id = "missing-cwd".into();
    missing.project = None;
    missing.worktree = None;
    missing.cwd = Some("/deleted/worktree".into());
    missing.latest_model = Some("missing-cwd-model".into());
    missing.state = SessionState::Exited;
    missing.last_activity_ms = now - 2_000;

    let mut no_cwd = missing.clone();
    no_cwd.id = "no-cwd".into();
    no_cwd.cwd = None;
    no_cwd.latest_model = Some("no-cwd-model".into());
    no_cwd.last_activity_ms = now - 3_000;

    let mut system_temp = snapshot.sessions[0].clone();
    system_temp.id = "system-temp".into();
    system_temp.project = Some(test_project("system-temp"));
    system_temp.cwd = Some("/private/var/folders/zz/agent-workspace".into());
    system_temp.latest_model = Some("system-temp-model".into());
    system_temp.last_activity_ms = now - 4_000;

    snapshot
        .sessions
        .extend([real_private, missing, no_cwd, system_temp]);
    let mut ui = Ui {
        tab: Tab::Claude,
        selected: 2,
        ..Ui::default()
    };

    let collapsed = rendered_rows_at(&ui, &snapshot, 120, 30);
    assert!(collapsed.contains("real-private-model"));
    assert!(collapsed.contains("+ MISSING CWD (2)"));
    assert!(!collapsed.contains("missing-cwd-model"));
    assert!(!collapsed.contains("no-cwd-model"));
    assert!(!collapsed.contains("system-temp-model"));

    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let expanded = rendered_rows_at(&ui, &snapshot, 120, 30);

    assert!(expanded.contains("- MISSING CWD (2)"));
    assert!(expanded.contains("missing-cwd-model"));
    assert!(expanded.contains("no-cwd-model"));
    assert!(!expanded.contains("system-temp-model"));
}

#[test]
fn provider_projects_partition_sessions_at_the_rolling_seven_day_cutoff() {
    const DAY_MS: i64 = 86_400_000;
    let now = 1_800_000_000_000;
    let cutoff = now - 7 * DAY_MS;
    let mut snapshot = demo_snapshot();
    snapshot.generated_ms = now;
    snapshot.sessions[0].state = SessionState::Exited;
    snapshot.sessions[0].id = "cutoff-session".into();
    snapshot.sessions[0].project = Some(test_project("mixed-project"));
    snapshot.sessions[0].latest_model = Some("cutoff-visible".into());
    snapshot.sessions[0].last_activity_ms = cutoff;

    let mut older = snapshot.sessions[0].clone();
    older.id = "older-hidden".into();
    older.latest_model = Some("older-hidden".into());
    older.last_activity_ms = cutoff - 1;

    let mut older_only = snapshot.sessions[0].clone();
    older_only.id = "older-only-hidden".into();
    older_only.project = Some(test_project("older-only-project"));
    older_only.latest_model = Some("older-only-hidden".into());
    older_only.last_activity_ms = cutoff - DAY_MS;

    snapshot.sessions.extend([older, older_only]);
    let ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };

    let page = rendered_rows_at(&ui, &snapshot, 120, 30);

    assert!(page.contains("PROJECT  mixed-project"));
    assert!(page.contains("cutoff-visible"));
    assert!(!page.contains("older-hidden"));
    assert!(page.contains("PROJECT  older-only-project"));
    assert!(!page.contains("older-only-hidden"));
    assert_eq!(page.matches("OLDER SESSIONS (1)").count(), 2);
}

#[test]
fn provider_project_older_disclosures_expand_independently() {
    const DAY_MS: i64 = 86_400_000;
    let now = 1_800_000_000_000;
    let mut snapshot = demo_snapshot();
    snapshot.generated_ms = now;
    snapshot.sessions[0].state = SessionState::Exited;
    snapshot.sessions[0].id = "alpha-older".into();
    snapshot.sessions[0].project = Some(test_project("alpha"));
    snapshot.sessions[0].latest_model = Some("alpha-older-model".into());
    snapshot.sessions[0].last_activity_ms = now - 8 * DAY_MS;

    let mut beta = snapshot.sessions[0].clone();
    beta.id = "beta-older".into();
    beta.project = Some(test_project("beta"));
    beta.latest_model = Some("beta-older-model".into());
    beta.last_activity_ms = now - 9 * DAY_MS;
    snapshot.sessions.push(beta);

    let mut ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let alpha_expanded = rendered_rows_at(&ui, &snapshot, 120, 30);
    assert!(alpha_expanded.contains("alpha-older-model"));
    assert!(!alpha_expanded.contains("beta-older-model"));

    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let both_expanded = rendered_rows_at(&ui, &snapshot, 120, 30);
    assert!(both_expanded.contains("alpha-older-model"));
    assert!(both_expanded.contains("beta-older-model"));

    ui.handle_key(key(KeyCode::Up), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Up), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let only_beta_expanded = rendered_rows_at(&ui, &snapshot, 120, 30);
    assert!(!only_beta_expanded.contains("alpha-older-model"));
    assert!(only_beta_expanded.contains("beta-older-model"));
}

#[test]
fn expanding_older_sessions_preserves_an_exact_partition_and_lifetime_usage() {
    const DAY_MS: i64 = 86_400_000;
    let now = 1_800_000_000_000;
    let mut snapshot = demo_snapshot();
    snapshot.generated_ms = now;
    snapshot.sessions[0].id = "recent-session".into();
    snapshot.sessions[0].project = Some(test_project("partition-project"));
    snapshot.sessions[0].latest_model = Some("recent-model".into());
    snapshot.sessions[0].last_activity_ms = now - DAY_MS;
    snapshot.sessions[0].usage = Usage {
        input: 11,
        output: 12,
        cache_read: 13,
        cache_write: 14,
    };

    let mut older_a = snapshot.sessions[0].clone();
    older_a.id = "older-a".into();
    older_a.latest_model = Some("older-a-model".into());
    older_a.last_activity_ms = now - 8 * DAY_MS;
    older_a.usage = Usage {
        input: 21,
        output: 22,
        cache_read: 23,
        cache_write: 24,
    };

    let mut older_b = snapshot.sessions[0].clone();
    older_b.id = "older-b".into();
    older_b.latest_model = Some("older-b-model".into());
    older_b.last_activity_ms = now - 9 * DAY_MS;
    older_b.usage = Usage {
        input: 31,
        output: 32,
        cache_read: 33,
        cache_write: 34,
    };
    snapshot.sessions.extend([older_a, older_b]);

    let mut ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let collapsed = rendered_rows_at(&ui, &snapshot, 160, 30);
    assert_eq!(collapsed.matches("recent-model").count(), 1);
    assert!(collapsed.contains("I 11  O 12  CR 13  CW 14"));
    assert_eq!(collapsed.matches("OLDER SESSIONS (2)").count(), 1);
    assert!(!collapsed.contains("older-a-model"));
    assert!(!collapsed.contains("older-b-model"));

    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    let expanded = rendered_rows_at(&ui, &snapshot, 160, 30);

    for session in ["recent-model", "older-a-model", "older-b-model"] {
        assert_eq!(expanded.matches(session).count(), 1);
    }
    for usage in [
        "I 11  O 12  CR 13  CW 14",
        "I 21  O 22  CR 23  CW 24",
        "I 31  O 32  CR 33  CW 34",
    ] {
        assert_eq!(expanded.matches(usage).count(), 1);
    }
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
fn provider_pages_have_no_time_filter_and_ignore_the_old_filter_key() {
    let mut ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
    let snapshot = Snapshot::default();
    let before = rendered_with_snapshot(&ui, &snapshot);

    ui.handle_key(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();

    let after = rendered_with_snapshot(&ui, &snapshot);
    assert_eq!(after, before);
    for removed in [
        "Time",
        "Active Today",
        "Last 7 Days",
        "Last 30 Days",
        "All Sessions",
    ] {
        assert!(!after.contains(removed));
    }
}

#[test]
fn provider_session_row_preserves_complete_session_facts_and_lifetime_usage() {
    let now = Local::now().timestamp_millis();
    let mut snapshot = demo_snapshot();
    snapshot.generated_ms = now;
    snapshot.sessions[0].last_activity_ms = now;
    snapshot.sessions[0].compactions = vec![
        Compaction {
            at_ms: now - 2_000,
            trigger: Some("auto".into()),
            pre_tokens: Some(180_000),
        },
        Compaction {
            at_ms: now - 1_000,
            trigger: Some("manual".into()),
            pre_tokens: Some(170_000),
        },
    ];
    let ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };

    let page = rendered_rows_at(&ui, &snapshot, 160, 30);

    for fact in [
        "PROJECT  sessionmesh",
        "-session",
        "Busy",
        "claude-opus-4-8",
        "150.0k (75%)",
        "I 1.0k  O 500  CR 200  CW 100",
        "$0.42",
        "0s",
        "⤳2",
    ] {
        assert!(page.contains(fact), "Provider row should contain {fact:?}");
    }
}

#[test]
fn compact_provider_session_row_keeps_every_fact_visible() {
    let now = Local::now().timestamp_millis();
    let mut snapshot = demo_snapshot();
    snapshot.generated_ms = now;
    snapshot.sessions[0].last_activity_ms = now;
    snapshot.sessions[0].compactions = vec![Compaction {
        at_ms: now - 1_000,
        trigger: Some("auto".into()),
        pre_tokens: Some(180_000),
    }];
    let ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };

    let page = rendered_rows_at(&ui, &snapshot, 80, 24);

    for fact in [
        "PROJECT  sessionmesh",
        "-session",
        "Busy",
        "claude-opus-4-8",
        "150.0k (75%)",
        "I 1.0k  O 500  CR 200  CW 100",
        "$0.42",
        "0s",
        "⤳1",
    ] {
        assert!(
            page.contains(fact),
            "compact Provider row should contain {fact:?}"
        );
    }
}

#[test]
fn old_open_session_remains_now_and_restores_its_provider_disclosure_after_detail() {
    const DAY_MS: i64 = 86_400_000;
    let now = Local::now().timestamp_millis();
    let mut snapshot = demo_snapshot();
    snapshot.generated_ms = now;
    snapshot.sessions[0].id = "old-open-session".into();
    snapshot.sessions[0].project = Some(test_project("old-open-project"));
    snapshot.sessions[0].latest_model = Some("old-open-model".into());
    snapshot.sessions[0].state = SessionState::Busy;
    snapshot.sessions[0].last_activity_ms = now - 8 * DAY_MS;
    let mut ui = Ui::default();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    assert!(rendered_rows_at(&ui, &snapshot, 80, 24).contains("✳ -session"));

    ui.handle_key(key(KeyCode::Char('2')), &snapshot).unwrap();
    let collapsed = rendered_rows_at(&ui, &snapshot, 80, 24);
    assert!(collapsed.contains("OLDER SESSIONS (1)"));
    assert!(!collapsed.contains("old-open-model"));

    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
    assert!(rendered_rows_at(&ui, &snapshot, 80, 24).contains("old-open-model"));
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("old-open-session"));

    ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
    let restored = rendered_rows_at(&ui, &snapshot, 80, 24);
    assert!(restored.contains("- OLDER SESSIONS (1)  Collapse"));
    assert!(restored.contains("old-open-model"));
    ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
    assert!(rendered_with_snapshot(&ui, &snapshot).contains("old-open-session"));
}

#[test]
fn moving_selection_changes_which_session_opens_in_detail() {
    let mut snapshot = demo_snapshot();
    let now = Local::now().timestamp_millis();
    snapshot.sessions[0].last_activity_ms = now;
    let mut second = snapshot.sessions[0].clone();
    second.id = "chosen-second-session".into();
    second.project = Some(test_project("second-project"));
    second.last_activity_ms = now - 1_000;
    snapshot.sessions.push(second);
    let mut ui = Ui::default();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();

    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    assert!(rendered_with_snapshot(&ui, &snapshot).contains("chosen-second-session"));
}

#[test]
fn detail_keeps_provider_and_native_session_identity_when_rows_reorder() {
    let mut snapshot = demo_snapshot();
    let now = Local::now().timestamp_millis();
    snapshot.sessions[0].last_activity_ms = now;
    let mut chosen = snapshot.sessions[0].clone();
    chosen.id = "stable-detail-target".into();
    chosen.project = Some(test_project("chosen-project"));
    chosen.state = SessionState::Exited;
    chosen.last_activity_ms = now - 1_000;
    snapshot.sessions.push(chosen);
    let mut ui = Ui::default();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    snapshot.sessions[0].state = SessionState::Exited;
    snapshot.sessions[1].state = SessionState::Busy;
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_with_snapshot(&ui, &snapshot);
    assert!(detail.contains("stable-detail-target"));
    assert!(!detail.contains("demo-session"));

    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let reopened = rendered_with_snapshot(&ui, &snapshot);
    assert!(reopened.contains("stable-detail-target"));
    assert!(!reopened.contains("demo-session"));
}

#[test]
fn session_detail_is_a_centered_modal_above_a_dimmed_origin_page() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    let mut ui = Ui::default();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
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
fn escape_returns_to_the_originating_page_with_its_selection_restored() {
    let mut snapshot = demo_snapshot();
    let now = Local::now().timestamp_millis();
    snapshot.sessions[0].last_activity_ms = now;
    let mut origin = snapshot.sessions[0].clone();
    origin.id = "origin-session".into();
    origin.project = Some(test_project("origin-project"));
    origin.last_activity_ms = now - 1_000;
    let mut third = snapshot.sessions[0].clone();
    third.id = "third-session".into();
    third.project = Some(test_project("third-project"));
    third.last_activity_ms = now - 2_000;
    snapshot.sessions.extend([origin, third]);
    let mut ui = Ui::default();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    ui.handle_key(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_with_snapshot(&ui, &snapshot);
    assert!(detail.contains("origin-session"));
    assert!(!detail.contains("third-session"));
}

#[test]
fn number_keys_leave_detail_for_the_requested_top_level_page() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();

    for (key, expected_page) in [('1', Tab::All), ('2', Tab::Claude), ('3', Tab::Codex)] {
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session Detail"));

        ui.handle_key(
            KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        assert_eq!(ui.active_tab(), expected_page);
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Session Detail"));
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
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
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
    let mut ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
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
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
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
        "sessionmesh",
        "Worktree",
        "/Users/x/code/sessionmesh",
        "Branch",
        "main",
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
    let mut ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
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
        let mut ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
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
    let mut unknown_ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
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
    let mut exited_ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
    exited_ui
        .handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &exited_snapshot,
        )
        .unwrap();
    let exited_terminal = render_terminal_at(&exited_ui, &exited_snapshot, 160, 40);

    assert_eq!(
        rendered_style_at(&exited_terminal, "90.0%").fg,
        Some(theme.muted)
    );

    let mut narrow_ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
    narrow_ui
        .handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &demo_snapshot(),
        )
        .unwrap();
    for _ in 0..11 {
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
    let mut ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

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
    let mut ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

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
    let mut ui = Ui {
        tab: Tab::Claude,
        ..Ui::default()
    };
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_rows_at(&ui, &snapshot, 120, 80);

    assert!(detail.contains("Transcript Lifetime Usage"));
    assert!(!detail.contains("Current Run Usage"));
    assert!(!detail.contains("Last Run Usage"));
}

#[test]
fn non_git_detail_marks_git_only_fields_unavailable() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    snapshot.sessions[0].project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/Users/x/Documents/research-notes")),
        label: "research-notes".into(),
    });
    snapshot.sessions[0].worktree = None;
    snapshot.sessions[0].cwd = Some("/Users/x/Documents/research-notes".into());
    let mut ui = Ui::default();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_with_snapshot(&ui, &snapshot);

    assert!(detail.contains("research-notes"));
    assert!(detail.contains("N/A (non-Git Project)"));
}

#[test]
fn unresolved_detail_preserves_cwd_and_marks_identity_unavailable() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    snapshot.sessions[0].project = None;
    snapshot.sessions[0].worktree = None;
    snapshot.sessions[0].cwd = Some("/missing/session-cwd".into());
    let mut ui = Ui::default();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_with_snapshot(&ui, &snapshot);

    assert!(detail.contains("N/A (identity unavailable)"));
    assert!(detail.contains("/missing/session-cwd"));
}

#[test]
fn detached_worktree_detail_marks_only_the_branch_unavailable() {
    let mut snapshot = demo_snapshot();
    snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
    snapshot.sessions[0].worktree.as_mut().unwrap().branch = None;
    let mut ui = Ui::default();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = rendered_with_snapshot(&ui, &snapshot);

    assert!(detail.contains("N/A (detached HEAD)"));
    assert!(detail.contains("/Users/x/code/sessionmesh"));
}

#[test]
fn renders_all_tab() {
    render_tab(Tab::All, false);
}

#[test]
fn renders_provider_tab_and_detail() {
    render_tab(Tab::Claude, false);
    render_tab(Tab::Claude, true);
}
