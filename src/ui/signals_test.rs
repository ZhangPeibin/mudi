use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn signal() -> RecentSignal {
    RecentSignal::Session {
        observed_at_ms: 2_000,
        provider: Provider::CodexCli,
        session_id: "signal-session".into(),
        kind: RecentSignalKind::SessionStateChanged {
            from: SessionState::Busy,
            to: SessionState::Idle,
        },
    }
}

fn render(width: u16, signals: &[RecentSignal]) -> String {
    let backend = TestBackend::new(width, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    let ui = Ui::default();
    terminal
        .draw(|frame| draw_panel(frame, frame.area(), signals, 3_000, &ui))
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
fn empty_panel_explains_that_no_recent_signals_exist() {
    assert!(render(27, &[]).contains("No recent Signals."));
}

#[test]
fn compact_panel_hides_latest_title_but_keeps_signal_evidence() {
    let rendered = render(27, &[signal()]);

    assert!(!rendered.contains("latest"));
    assert!(rendered.contains("Busy -> Idle"));
}

#[test]
fn wide_panel_shows_latest_signal_age() {
    assert!(render(40, &[signal()]).contains("latest"));
}
