//! TokenPlay entry point.
//!
//! The TUI opens immediately. A background task performs the initial (potentially long)
//! historical index with visible progress, then keeps the index fresh with cheap
//! incremental scans ~every 2s. The render loop never blocks on IO — it reads the latest
//! snapshot from shared state each frame.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

use tokenplay::alerts::{self, AlertEngine};
use tokenplay::app::{Snapshot, Tab};
use tokenplay::collector;
use tokenplay::config::Config;
use tokenplay::index::Index;
use tokenplay::pricing::RateCard;
use tokenplay::ui::{draw, Ui};

/// Shared snapshot updated by the collector, read by the UI.
type Shared = Arc<Mutex<Snapshot>>;

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = Config::load();
    let shared: Shared = Arc::new(Mutex::new(Snapshot {
        status: "Starting…".to_string(),
        ..Default::default()
    }));

    // Channel to request an out-of-band rescan (the `r` key).
    let (rescan_tx, rescan_rx) = mpsc::unbounded_channel::<()>();

    // Background collector task.
    let bg = tokio::spawn(collector_loop(cfg.clone(), shared.clone(), rescan_rx));

    // Run the TUI on the current thread.
    let result = run_tui(shared.clone(), rescan_tx);

    bg.abort();
    result
}

/// Background loop: scan → build → alert → publish, repeating ~every 2s.
async fn collector_loop(cfg: Config, shared: Shared, mut rescan_rx: mpsc::UnboundedReceiver<()>) {
    // Index and alert engine live only here (single-owner, off the UI thread).
    let mut index = match Index::open(&cfg.index_path()) {
        Ok(i) => i,
        Err(e) => {
            if let Ok(mut s) = shared.lock() {
                s.status = format!("index error: {e}");
            }
            return;
        }
    };
    let rates = RateCard::load(Some(&cfg.pricing_override_path()));
    let mut alert_engine = AlertEngine::new();
    let mut first = true;

    loop {
        {
            if let Ok(mut s) = shared.lock() {
                s.status = if first { "Indexing…".into() } else { "Refreshing…".into() };
            }
        }

        let stats = collector::refresh(&cfg, &mut index);
        let mut sessions = collector::build_sessions(&cfg, &index, &rates);
        collector::sort_for_display(&mut sessions);

        // Alerts + notifications run off the raw context of open sessions.
        alert_engine.update(&sessions);
        let alert_counts = alerts::counts(&sessions);

        let now = chrono::Local::now().timestamp_millis();
        let snap = Snapshot {
            sessions,
            stats,
            alerts: alert_counts,
            status: format!(
                "watching · {} sessions · {} parsed / {} skipped{}",
                stats.discovered,
                stats.parsed,
                stats.skipped,
                if stats.pruned > 0 { format!(" · {} pruned", stats.pruned) } else { String::new() },
            ),
            generated_ms: now,
            ready: true,
        };
        if let Ok(mut s) = shared.lock() {
            *s = snap;
        }
        first = false;

        // Wait ~2s, but wake early on an explicit rescan request.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = rescan_rx.recv() => {}
        }
    }
}

fn run_tui(shared: Shared, rescan_tx: mpsc::UnboundedSender<()>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut ui = Ui::default();
    let res = event_loop(&mut terminal, &mut ui, &shared, &rescan_tx);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    ui: &mut Ui,
    shared: &Shared,
    rescan_tx: &mpsc::UnboundedSender<()>,
) -> Result<()> {
    loop {
        // Snapshot the shared state (cheap clone) so rendering holds no lock.
        let snap = shared.lock().map(|s| s.clone()).unwrap_or_default();
        let visible_len = ui.visible(&snap).len();
        ui.clamp_selection(visible_len);

        terminal.draw(|f| draw(f, &snap, ui))?;

        // Poll for input with a timeout so the screen refreshes even without keys.
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('q') => return Ok(()),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),

            KeyCode::Tab | KeyCode::Right => switch_tab(ui, 1),
            KeyCode::BackTab | KeyCode::Left => switch_tab(ui, -1),
            KeyCode::Char('1') => set_tab(ui, Tab::All),
            KeyCode::Char('2') => set_tab(ui, Tab::Claude),
            KeyCode::Char('3') => set_tab(ui, Tab::Codex),

            KeyCode::Char('f') => {
                ui.filter = ui.filter.next();
                ui.selected = 0;
            }
            KeyCode::Char('r') => {
                let _ = rescan_tx.send(());
            }

            KeyCode::Down | KeyCode::Char('j') if ui.tab != Tab::All => {
                if visible_len > 0 && ui.selected + 1 < visible_len {
                    ui.selected += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') if ui.tab != Tab::All => {
                ui.selected = ui.selected.saturating_sub(1);
            }
            KeyCode::Enter if ui.tab != Tab::All && visible_len > 0 => ui.detail = true,
            KeyCode::Esc => ui.detail = false,

            _ => {}
        }
    }
}

fn set_tab(ui: &mut Ui, tab: Tab) {
    ui.tab = tab;
    ui.selected = 0;
    ui.detail = false;
}

fn switch_tab(ui: &mut Ui, dir: i32) {
    let i = ui.tab.index() as i32;
    let n = Tab::ALL.len() as i32;
    let next = ((i + dir) % n + n) % n;
    set_tab(ui, Tab::ALL[next as usize]);
}
