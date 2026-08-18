use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 将页头和页脚渲染到同一个缓冲区，便于断言全局框架信息。
fn render(snapshot: &Snapshot, ui: &Ui) -> String {
    let backend = TestBackend::new(120, 2);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            draw_header(frame, Rect::new(0, 0, 120, 1), snapshot, ui);
            draw_footer(frame, Rect::new(0, 1, 120, 1), snapshot, ui);
        })
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
fn global_chrome_shows_product_navigation_inventory_and_status() {
    let snapshot = Snapshot {
        ready: true,
        ..Snapshot::default()
    };

    let rendered = render(&snapshot, &Ui::default());

    assert!(rendered.contains("Mudi"));
    assert!(rendered.contains("0 Projects"));
    assert!(rendered.contains("watching · 0 sessions · 0 parsed / 0 skipped"));
}
