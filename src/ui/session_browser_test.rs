use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn browser_uses_compact_headers_and_an_internal_details_divider() {
    let backend = TestBackend::new(48, 14);
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
        .chunks(48)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>();
    let rendered = rows.join("\n");

    assert!(rows[0].contains("PROJECT 0"));
    assert!(rows[0].contains("0 OPEN"));
    assert!(rendered.contains("DETAILS"));
    assert!(rendered.contains("No Open Session selected."));
    assert!(!rendered.contains("Selected Session"));
}
