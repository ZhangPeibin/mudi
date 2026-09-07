use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn compact_overview_uses_the_full_status_band_without_an_actionable_managed_start() {
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

    assert_eq!(rows[0].find("RECENT SIGNALS"), Some(4));
    assert!(rows[0].contains("TASKS"));
    assert!(!rows.iter().any(|row| row.contains("MANAGED START")));
    assert!(rows[8].starts_with(" PROJECT"));
    assert!(rows[8].contains("Session Workspace"));
    assert_eq!(rows[8].chars().nth(31), Some('│'));
    assert_ne!(rows[8].chars().last(), Some('┐'));
    assert!(rows.iter().any(|row| row.contains("DETAILS")));
    assert!(!rows.iter().any(|row| row.contains("Selected Session")));
}
