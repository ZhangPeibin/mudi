# SessionMesh

A fully local, read-only **macOS TUI** for observing **Claude Code** and **Codex CLI**
coding sessions in one terminal — current context pressure, live Busy/Idle/exited state,
meaningful recent transitions, Current Run and Transcript Lifetime Usage with API-rate
dollar estimates, and compaction history.

> Metadata only. SessionMesh never reads prompts, tool calls, or file content into storage.

## Highlights

- **Three numeric pages:** `[1]` All, `[2]` Claude Code, `[3]` Codex CLI.
- **Contextual Guide:** opens once on first use and remains globally available through
  `?`, with Overview, Navigation, Sessions, Signals, and Theme references.
- **Six semantic Themes:** `sessionmesh`, `catppuccin`, `dracula`, `gruvbox`, `light`, and
  `high-contrast`, loaded from the local Profile or overridden for one process.
- **Accurate context semantics** per provider (see `CONTEXT.md`).
- **Live lifecycle:** Busy / Idle / exited, with PID-reuse-safe detection on macOS.
- **Recent Signals:** five process-lifetime Session, Context, and Compaction transitions.
- **Scoped Usage:** Current Run for Open Sessions beside complete Transcript Lifetime
  Usage, with API-rate dollar estimates (bundled, versioned, overridable).
- **Compaction history** (count badge + detail timeline).
- **Context Alerts** at 70% (warning) / 85% (critical) with macOS notifications.
- **Disposable SQLite index**, rebuildable from source logs; incremental, no full rescans.

## Build & run

```sh
cargo run --release        # opens the TUI
cargo run --release -- --theme dracula  # one-process Theme override
```

Keys: `1-3` open pages · `Tab`/`Shift+Tab` move focus within a page · `Up/Down` or `j/k`
move selection or scroll Detail · `Enter` inspects a Session or toggles `OLDER SESSIONS` ·
`?` opens Guide · `Esc` returns from Detail or closes the current layer · `r` rescans ·
`q` quits.

Guide opens on the category relevant to the current page or region while keeping every
category available in its sidebar. Use `Up/Down` or `j/k` to choose a category, `Tab` or
Right to enter its content, and `Shift+Tab` or Left to return to the sidebar.

### Non-interactive checks

```sh
cargo test                                   # unit + render smoke tests
SESSIONMESH_DATA_DIR=$(mktemp -d) cargo run --example scan     # pipeline summary
SESSIONMESH_DATA_DIR=$(mktemp -d) cargo run --example render   # off-screen TUI render
```

## Configuration

Standard paths with environment overrides:

| Variable | Default | Purpose |
|---|---|---|
| `SESSIONMESH_CLAUDE_DIR` | `~/.claude` | Claude data root |
| `SESSIONMESH_CODEX_DIR` | `~/.codex` | Codex data root |
| `SESSIONMESH_DATA_DIR` | `~/Library/Application Support/sessionmesh` | index, Profile, and model override files |
| `SESSIONMESH_EXTRA_DIRS` | — | colon-separated extra log dirs |

Local Model Catalog override: drop a `models.json` (same shape as `models/catalog.json`) in
`SESSIONMESH_DATA_DIR`. Entries are scoped by exact Model Provider and model identifier. The
bundled catalog currently includes 80 model identifiers across 9 Model Providers; its 65
mainstream source records come from the structured table at `llmcalcs.com/pricing-comparison`
(source data last updated 2026-06-11). The catalog records direct and batch API prices plus
Context Window; batch prices are metadata and are not used for interactive Session estimates.

The disposable Session index is `index.db`. Local Profile preferences are stored
separately in `profile.json`; deleting that file restores first-use Guide and the default
`sessionmesh` Theme. `--theme <name>` overrides the saved Theme for one process without
changing the Profile; unknown names fall back to `sessionmesh`. Runtime Theme switching is
deferred to a future Settings feature.

## Layout

```
src/
  domain.rs         normalized types (Session, Usage, ContextState, …)
  config.rs         paths + env overrides + thresholds
  models.rs         Model-Provider-scoped prices + Context Window catalog
  providers/        deep adapters behind one interface
    claude.rs         transcript parser (dedup streamed records)
    codex.rs          rollout parser (cumulative deltas, never summed)
    state.rs          macOS Busy/Idle/exited detection
  index/            disposable, incremental SQLite index
  collector.rs      discover → index → build normalized snapshot
  alerts.rs         threshold crossings + macOS notifications
  app.rs            tabs, snapshot, formatting
  ui/               Ratatui rendering (All / provider / detail)
  main.rs           tokio background loop + crossterm event loop
```

See `CONTEXT.md` for domain vocabulary and `docs/` for the spec and plan.

## Scope (v1)

macOS only; read-only; no background daemon after exit; no subscription quota; no
prompt/tool/file analysis; no JSON output; Claude Code + Codex CLI only.

## License

MIT
