//! Render the TUI to an off-screen buffer against real logs and print it as text.
//! Lets us eyeball the layout without a TTY. Respects MUDI_* overrides.

use mudi::alerts;
use mudi::app::Snapshot;
use mudi::collector;
use mudi::config::Config;
use mudi::index::Index;
use mudi::models::ModelCatalog;
use mudi::ui::{draw, Ui};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn main() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    let mut index = Index::open(&cfg.index_path())?;
    let models = ModelCatalog::load(Some(&cfg.model_catalog_override_path()));
    collector::refresh(&cfg, &mut index);
    let mut sessions = collector::build_sessions(&cfg, &index, &models);
    collector::sort_for_display(&mut sessions);
    let alerts = alerts::counts(&sessions);

    let snap = Snapshot {
        sessions,
        alerts,
        generated_ms: chrono::Local::now().timestamp_millis(),
        ready: true,
        ..Default::default()
    };

    print_frame(&snap, &Ui::default())
}

fn print_frame(snap: &Snapshot, ui: &Ui) -> anyhow::Result<()> {
    let (w, h) = (110u16, 24u16);
    let mut terminal = Terminal::new(TestBackend::new(w, h))?;
    terminal.draw(|f| draw(f, snap, ui))?;
    let buf = terminal.backend().buffer().clone();

    println!("\n┌── Mudi {}", "─".repeat(40));
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
