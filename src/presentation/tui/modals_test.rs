use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 收集测试终端中的所有字符，保持断言只依赖用户可见结果。
fn rendered(terminal: &Terminal<TestBackend>) -> String {
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn cancellation_confirmation_identifies_the_selected_task() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut ui = Ui::default();
    ui.push_confirmation(super::super::overlay::ConfirmationState::TaskCancellation(
        super::super::TaskCancellationConfirmation {
            task_id: "task-to-cancel".into(),
        },
    ));

    terminal
        .draw(|frame| draw_overlays(frame, &Snapshot::default(), &ui))
        .unwrap();

    let output = rendered(&terminal);
    assert!(output.contains("Cancel Task?"));
    assert!(output.contains("task-to-cancel"));
    assert!(output.contains("Esc Keep Task"));
}

#[test]
fn too_small_notice_reports_required_and_current_dimensions() {
    let backend = TestBackend::new(70, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = Ui::default().theme.palette();

    terminal
        .draw(|frame| draw_terminal_too_small(frame, theme))
        .unwrap();

    let output = rendered(&terminal);
    assert!(output.contains("Terminal too small"));
    assert!(output.contains("Required: 80x24"));
    assert!(output.contains("Current: 70x20"));
}
