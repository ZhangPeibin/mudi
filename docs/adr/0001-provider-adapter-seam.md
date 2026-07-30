# ADR 0001 — Express the `ProviderAdapter` seam

- **Status:** Accepted and implemented (2026-07-30).
- **Scope:** Architecture deepening candidates **1 + 2 + 4** as one unit. Candidates 3
  (pricing policy) and 5 (probe injection / testability) are explicitly out of scope.

## Context

"What a provider is" is currently smeared across ~6 `match provider` sites instead of
living behind one type:

- `collector::refresh` — separate `discover_claude_files` / `discover_codex_files`.
- `collector::index_one` — `match provider` to pick `claude::parse_file` / `codex::parse_file`.
- `collector::to_session` — `match provider` to pick `detector.claude_state(id)` vs
  `detector.codex_state(source_path, codex_open_turn)`. Only consumer of `codex_open_turn`.
- `providers::known_context_window` — a disguised switch (`model.starts_with("claude")` →
  hardcoded `200_000`). This is the real `106%` over-report bug (candidate 4).
- `ui::draw_provider_card` accent colour + `app::Tab` enumerations.

Two symptoms follow from the missing type:

- **Leak (candidate 2):** `ParsedSession.codex_open_turn: Option<bool>` — a Codex-only field
  on the normalized struct — also persists as a SQLite column (`index` `codex_open_turn`),
  polluting a cache that should hold pure usage metadata.
- **Bug (candidate 4):** the hardcoded 200k window makes Claude sessions render `>100%`
  (observed `212.7k (106%)`).

## Decision

Introduce a `ProviderAdapter` — one "manager" per provider — and route every
provider-specific decision through it. Five questions were resolved:

### Q1 — Scope
Design **1 + 2 + 4** as a single unit; keep **3** and **5** separate. The `106%` bug is
fixed **through** the seam (via `context_window`), not as a standalone patch.

### Q2 — Trait shape
The adapter owns four data/behaviour concerns; presentation stays out.

```
trait ProviderAdapter {
    fn provider(&self) -> Provider;
    fn discover(&self, cfg: &Config) -> Vec<PathBuf>;            // replaces the two discover fns
    fn parse_file(&self, path: &Path) -> Option<ParsedSession>; // replaces index_one match
    fn context_window(&self, model: &str) -> Option<u64>;       // replaces known_context_window → fixes 106%
    fn live_state(&self, probe: &StateProbe, path: &Path) -> SessionState; // replaces to_session match
}
```

- **`card_accent` is NOT on the trait.** Colour is pure presentation. A dedicated *theme /
  colour manager* is planned for the future (out of scope here); until then the
  `Provider → Color` map stays in `ui/`. This keeps the domain-ish adapter from depending on
  `ratatui::Color`.
- **`Tab`, `Provider::label`, `Provider::as_str` stay put.** `Provider` is a `Copy` enum used
  as a SQLite key and serde tag; the adapter *has a* `Provider`, it is not one. `Tab` is UI
  navigation, not provider behaviour.

### Q3 — The index is the read path
`build_sessions` reads from SQLite (`load_all`), not source files, so Codex live-state needs
the task-boundary bit at build time. **Decision: option (b).** Drop the `codex_open_turn`
column entirely — the index returns to pure usage metadata — and **re-read only the tails of
currently-held rollout files** at build time to recover the open-turn bit.

Key fact that makes (b) cheap: `build_sessions` already gathers the `lsof`-held set before
calling `to_session`. Only held files can be Busy/Idle; everything else is Exited regardless
of the boundary. So the re-read touches only the tiny set of live sessions (typically 0–2),
not all logs. The previously-feared cost does not exist.

Rejected: (a) rename the column to a generic `provider_state_hint` — merely relocates the
leak; (c) status quo.

### Q4 — Dispatch mechanism
**A registry (slice) of adapters** (`&[&dyn ProviderAdapter]` or equivalent), iterated to
find the one matching a session's `Provider`. This drives every `match provider` site to
zero and makes adding a provider a **pure addition** (one new impl, no edits to old code).
Chosen over a single centralized match (still fine, but keeps one switch) and over static
generics (over-engineered; perf is a non-issue here).

### Q5 — Liveness probe
**One shared probe per scan.** The OS calls (`ps` for Claude, `pgrep`/`lsof` for Codex) are
the only expensive step; run them **once** per refresh and pass the resulting report into
each adapter's `live_state`. Rejected: each adapter gathering independently (re-runs the slow
commands per session).

Note on why the OS probe cannot be removed: log flags answer *"what was it doing"*
(Codex `task_started`/`task_complete`; Claude state-file `status`) but **not** *"is it still
alive"*. A crashed/killed session freezes its last flag (Codex fake-busy on `task_started`,
Claude fake-idle in a stale state file). Liveness is ground-truth only from the OS. State =
**OS liveness AND log flag**, combined: not alive → Exited (ignore flag); alive → trust flag.

## Consequences

- `providers/mod.rs` gains the `ProviderAdapter` trait; `claude.rs` / `codex.rs` each become
  an `impl`. `collector.rs` loses its `match provider` sites and iterates the registry.
- The SQLite schema drops the `codex_open_turn` column; `ParsedSession.codex_open_turn` is
  removed. A Codex-tail re-reader (returning the open-turn bit for a given held path) is
  added and called from `live_state`.
- `known_context_window` is deleted; the Claude adapter returns the correct window (fixing
  `106%`), the Codex adapter reports `None` (window comes from its own log, as today).
- `StateDetector` becomes the shared `StateProbe` passed into `live_state`. Making it
  injectable for tests (candidate 5) is a separate, later change.
- All 28 tests must stay green; migrate slice-by-slice (`/tdd`).

## Follow-ups (not in this ADR)
- Candidate 3 — pricing policy (already model-keyed in `RateCard`, provider-agnostic).
- Candidate 5 — inject the probe to make `ps`/`lsof` shell-outs testable.
- Theme / colour manager — own the `Provider → Color` map and future theming.
