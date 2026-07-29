//! Render the TUI to an off-screen buffer against real logs and print it as text.
//! Lets us eyeball the layout without a TTY. Respects TOKENPLAY_* overrides.

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tokenplay::app::{Snapshot, Tab, TimeFilter};
use tokenplay::collector;
use tokenplay::config::Config;
use tokenplay::index::Index;
use tokenplay::pricing::RateCard;
use tokenplay::ui::{draw, Ui};
use tokenplay::alerts;

fn main() -> anyhow::Result<()> {
    let cfg = Config::load();
    let mut index = Index::open(&cfg.index_path())?;
    let rates = RateCard::load(Some(&cfg.pricing_override_path()));
    collector::refresh(&cfg, &mut index);
    let mut sessions = collector::build_sessions(&cfg, &index, &rates);
    collector::sort_for_display(&mut sessions);
    let alerts = alerts::counts(&sessions);

    let snap = Snapshot {
        sessions,
        alerts,
        status: "watching".into(),
        generated_ms: chrono::Local::now().timestamp_millis(),
        ready: true,
        ..Default::default()
    };

    for (tab, filter) in [(Tab::All, TimeFilter::Last30), (Tab::Claude, TimeFilter::Last30), (Tab::Codex, TimeFilter::Last30)] {
        let ui = Ui { tab, filter, selected: 0, detail: false };
        print_frame(&snap, &ui, tab, filter)?;
    }
    Ok(())
}

fn print_frame(snap: &Snapshot, ui: &Ui, tab: Tab, filter: TimeFilter) -> anyhow::Result<()> {
    let (w, h) = (110u16, 24u16);
    let mut terminal = Terminal::new(TestBackend::new(w, h))?;
    terminal.draw(|f| draw(f, snap, ui))?;
    let buf = terminal.backend().buffer().clone();

    println!("\n┌── tab: {} · filter: {} {}", tab.title(), filter.title(), "─".repeat(40));
    for y in 0..h {
        let mut line = String::new();
        for x in 0..w {
            line.push_str(buf[(x, y)].symbol());
        }
        println!("│{}", line.trim_end());
    }
    println!("└{}", "─".repeat(70));
    Ok(())
}
