# TokenPlay

A fully local, read-only **macOS TUI** for observing **Claude Code** and **Codex CLI**
coding sessions in one terminal — current context pressure, live Busy/Idle/exited state,
historical usage with API-rate dollar estimates, and compaction history.

> Metadata only. TokenPlay never reads prompts, tool calls, or file content into storage.

`tokenplay` is a working name; unrelated Web3 products already use "TokenPlay". Revisit
before any public release.

## Highlights

- **Three numeric pages:** `[1]` All, `[2]` Claude Code, `[3]` Codex CLI.
- **First-use Help:** opens once per local Profile and remains available through `?`.
- **Six semantic Themes:** `tokenplay`, `catppuccin`, `dracula`, `gruvbox`, `light`, and
  `high-contrast`, selectable without changing Session semantics.
- **Accurate context semantics** per provider (see `CONTEXT.md`).
- **Live lifecycle:** Busy / Idle / exited, with PID-reuse-safe detection on macOS.
- **Historical usage + API-rate dollar estimates** (bundled, versioned, overridable).
- **Compaction history** (count badge + detail timeline).
- **Context Alerts** at 70% (warning) / 85% (critical) with macOS notifications.
- **Disposable SQLite index**, rebuildable from source logs; incremental, no full rescans.

## Build & run

```sh
cargo run --release        # opens the TUI
cargo run --release -- --theme dracula  # one-process Theme override
```

Keys: `1-3` open pages · `Tab`/`Shift+Tab` move focus within a page · `f` cycles the
time filter on top-level pages · `t` cycles Themes · `Up/Down` or `j/k` move selection
or scroll Detail · `Enter` opens Detail · `?` opens Help · `Esc` returns from Detail or
closes the current layer · `r` rescans · `q` quits.

### Non-interactive checks

```sh
cargo test                                   # unit + render smoke tests
TOKENPLAY_DATA_DIR=$(mktemp -d) cargo run --example scan     # pipeline summary
TOKENPLAY_DATA_DIR=$(mktemp -d) cargo run --example render   # off-screen TUI render
```

## Configuration

Standard paths with environment overrides:

| Variable | Default | Purpose |
|---|---|---|
| `TOKENPLAY_CLAUDE_DIR` | `~/.claude` | Claude data root |
| `TOKENPLAY_CODEX_DIR` | `~/.codex` | Codex data root |
| `TOKENPLAY_DATA_DIR` | `~/Library/Application Support/tokenplay` | index, Profile, and pricing files |
| `TOKENPLAY_EXTRA_DIRS` | — | colon-separated extra log dirs |

Local pricing override: drop a `pricing.json` (same shape as `pricing/models.json`) in
`TOKENPLAY_DATA_DIR`.

The disposable Session index is `index.db`. Local Profile preferences are stored
separately in `profile.json`; deleting that file restores first-use Help and the default
`tokenplay` Theme. `--theme <name>` overrides the saved Theme for one process without
changing the Profile; unknown names fall back to `tokenplay`.

## Layout

```
src/
  domain.rs         normalized types (Session, Usage, ContextState, …)
  config.rs         paths + env overrides + thresholds
  pricing.rs        bundled rate card + estimator (N/A, never substituted)
  providers/        deep adapters behind one interface
    claude.rs         transcript parser (dedup streamed records)
    codex.rs          rollout parser (cumulative deltas, never summed)
    state.rs          macOS Busy/Idle/exited detection
  index/            disposable, incremental SQLite index
  collector.rs      discover → index → build normalized snapshot
  alerts.rs         threshold crossings + macOS notifications
  app.rs            tabs, filters, snapshot, formatting
  ui/               Ratatui rendering (All / provider / detail)
  main.rs           tokio background loop + crossterm event loop
```

See `CONTEXT.md` for domain vocabulary and `docs/` for the spec and plan.

## Scope (v1)

macOS only; read-only; no background daemon after exit; no subscription quota; no
prompt/tool/file analysis; no JSON output; Claude Code + Codex CLI only.

## License

MIT
