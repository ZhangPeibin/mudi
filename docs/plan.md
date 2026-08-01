# SessionMesh — implementation plan (v1)

## Architecture

Deep provider adapters behind one normalized interface. Provider-specific JSONL semantics
stay inside adapters; the UI never sees field names.

```
source JSONL (truth)
   │  collectors (walk dirs, discover session files)
   ▼
parsers (claude.rs / codex.rs)  ──►  normalized domain types (domain.rs)
   │                                        │
   ▼                                        ▼
state detector (state.rs, macOS)      SQLite index (index/) — disposable, rebuildable
   │                                        │
   └──────────────►  App snapshot  ◄────────┘
                          │
              alerts.rs (thresholds + macOS notify)
                          │
                    TUI (ui/*, ratatui)
```

- `main.rs` — tokio runtime; spawn background collector loop (~2s); run TUI event/render
  loop on the main thread. Shared state via `Arc<Mutex<AppState>>`.
- Background loop: incremental scan → parse changed files → update index → recompute
  snapshot → run alert engine → post notifications.
- TUI: reads the latest snapshot each frame; never blocks on IO.

## Modules

- `config.rs` — data dirs, env overrides (`SESSIONMESH_CLAUDE_DIR`, `SESSIONMESH_CODEX_DIR`,
  `SESSIONMESH_DATA_DIR`, `SESSIONMESH_EXTRA_DIRS`), thresholds.
- `domain.rs` — `Provider`, `SessionState`, `Usage`, `ContextState`, `ModelUsage`,
  `Compaction`, `Session`, `Snapshot`.
- `pricing.rs` — bundled rate card (embedded JSON) + local override + estimator.
- `providers/claude.rs`, `providers/codex.rs` — collectors + parsers.
- `providers/state.rs` — macOS process detection (`ps`, `lsof`).
- `index/mod.rs` — SQLite schema, upsert, reconcile, rebuild.
- `alerts.rs` — threshold crossing + de-dup + `osascript` notifications.
- `signals.rs` — process-lifetime transition detection, de-duplication, and five-item
  retention.
- `app.rs` — tabs, selection, snapshot assembly.
- `ui/` — `all.rs`, `provider.rs`, `detail.rs`, shared widgets.

## Build order (test-first where it pays)

1. Scaffold + docs + pricing. ✔
2. Domain + pricing + config (unit tests on estimator).
3. Parsers with fixtures: duplicated Claude assistant records (dedup by requestId/uuid),
   Codex cumulative counters (use latest, don't sum), compaction extraction.
4. macOS state detection.
5. SQLite incremental index + reconcile tests.
6. TUI views; alerts; notifications; compaction detail.
7. Build/test green; verify against real logs without persisting content.

## Key correctness rules

- Claude current context = `input_tokens + cache_read_input_tokens` (latest only).
- Codex current context = latest `last_token_usage` (`input + output`) vs
  `model_context_window`.
- Processed usage is cumulative and separate from current context.
- Dedup duplicated Claude records; never sum Codex cumulative snapshots.
- Unknown model price → `N/A`, never substitute.
- Never persist prompt/tool/file content.
