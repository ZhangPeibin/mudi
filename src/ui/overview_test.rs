use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn compact_overview_reserves_left_third_and_composes_the_session_editor() {
    let backend = TestBackend::new(80, 22);
    let mut terminal = Terminal::new(backend).unwrap();
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };
    let ui = Ui::default();

    terminal
        .draw(|frame| draw(frame, frame.area(), &snapshot, &ui))
        .unwrap();
    let rows = terminal
        .backend()
        .buffer()
        .content()
        .chunks(80)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>();

    assert!(rows[0].chars().take(26).all(|character| character == ' '));
    assert!(rows[0].contains("RECENT SIGNALS"));
    assert!(rows[0].contains("TASKS"));
    assert!(rows[8].starts_with(" PROJECT TREE"));
    assert!(rows[8].contains("Session Workspace"));
    assert_eq!(rows[8].chars().nth(31), Some('│'));
    assert_ne!(rows[8].chars().last(), Some('┐'));
    assert!(rows.iter().any(|row| row.contains("DETAILS")));
    assert!(!rows.iter().any(|row| row.contains("Selected Session")));
}
