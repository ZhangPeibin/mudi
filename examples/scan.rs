//! Non-interactive verification harness: run the real collector pipeline against
//! local logs and print a summary. Used to validate parsing/state/pricing without the
//! TUI. Respects SESSIONMESH_* env overrides (point SESSIONMESH_DATA_DIR at a temp dir to
//! keep the real index untouched).

use sessionmesh::app::{fmt_context, fmt_cost, fmt_tokens};
use sessionmesh::collector;
use sessionmesh::config::Config;
use sessionmesh::domain::{Provider, SessionState};
use sessionmesh::index::Index;
use sessionmesh::models::ModelCatalog;

fn main() -> anyhow::Result<()> {
    let cfg = Config::load();
    println!("claude_dir: {}", cfg.claude_dir.display());
    println!("codex_dir:  {}", cfg.codex_dir.display());
    println!("index:      {}", cfg.index_path().display());

    let mut index = Index::open(&cfg.index_path())?;
    let models = ModelCatalog::load(Some(&cfg.model_catalog_override_path()));
    println!("model catalog:  {}\n", models.version);

    let t0 = std::time::Instant::now();
    let stats = collector::refresh(&cfg, &mut index);
    let elapsed1 = t0.elapsed();

    // Second pass should skip everything (incremental / no re-parse).
    let t1 = std::time::Instant::now();
    let stats2 = collector::refresh(&cfg, &mut index);
    let elapsed2 = t1.elapsed();

    println!(
        "scan 1: discovered={} parsed={} skipped={} pruned={} in {:?}",
        stats.discovered, stats.parsed, stats.skipped, stats.pruned, elapsed1
    );
    println!(
        "scan 2: discovered={} parsed={} skipped={} pruned={} in {:?}  (incremental should re-parse ~0)\n",
        stats2.discovered, stats2.parsed, stats2.skipped, stats2.pruned, elapsed2
    );

    let mut sessions = collector::build_sessions(&cfg, &index, &models);
    collector::sort_for_display(&mut sessions);

    for p in [Provider::ClaudeCode, Provider::CodexCli] {
        let ps: Vec<_> = sessions.iter().filter(|s| s.provider == p).collect();
        let busy = ps.iter().filter(|s| s.state == SessionState::Busy).count();
        let idle = ps.iter().filter(|s| s.state == SessionState::Idle).count();
        println!(
            "== {} ==  {} sessions  ({} busy, {} idle)",
            p.label(),
            ps.len(),
            busy,
            idle
        );
        for s in ps.iter().take(8) {
            let state = match s.state {
                SessionState::Busy => "BUSY",
                SessionState::Idle => "idle",
                SessionState::Exited => "—",
            };
            println!(
                "  {:<4} {:<24} {:<16} ctx {:<14} {:<8} cmp={}",
                state,
                truncate(s.project_label(), 24),
                truncate(s.latest_model.as_deref().unwrap_or("—"), 16),
                fmt_context(s),
                fmt_cost(s.cost),
                s.compaction_count(),
            );
        }
        let total: u64 = ps.iter().map(|s| s.usage.total()).sum();
        println!(
            "  lifetime tokens across shown provider: {}\n",
            fmt_tokens(total)
        );
    }

    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}
