//! Render the TUI to an off-screen buffer against real logs and print it as text.
//! Lets us eyeball the layout without a TTY. Respects SESSIONMESH_* overrides.

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sessionmesh::alerts;
use sessionmesh::app::{Snapshot, Tab};
use sessionmesh::collector;
use sessionmesh::config::Config;
use sessionmesh::index::Index;
use sessionmesh::models::ModelCatalog;
use sessionmesh::ui::{draw, Ui};

fn main() -> anyhow::Result<()> {
    let cfg = Config::load();
    let mut index = Index::open(&cfg.index_path())?;
    let models = ModelCatalog::load(Some(&cfg.model_catalog_override_path()));
    collector::refresh(&cfg, &mut index);
    let mut sessions = collector::build_sessions(&cfg, &index, &models);
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

    for tab in Tab::ALL {
        let mut ui = Ui::default();
        ui.tab = tab;
        print_frame(&snap, &ui, tab)?;
    }
    Ok(())
}

fn print_frame(snap: &Snapshot, ui: &Ui, tab: Tab) -> anyhow::Result<()> {
    let (w, h) = (110u16, 24u16);
    let mut terminal = Terminal::new(TestBackend::new(w, h))?;
    terminal.draw(|f| draw(f, snap, ui))?;
    let buf = terminal.backend().buffer().clone();

    println!("\n┌── tab: {} {}", tab.title(), "─".repeat(40));
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
