# SessionMesh

A fully local **macOS TUI** for observing **Claude Code** and **Codex CLI** coding
sessions in one terminal — current context pressure, live Busy/Idle/exited state,
meaningful recent transitions, Current Run and Transcript Lifetime Usage with API-rate
dollar estimates, compaction history, and explicitly authorized Assisted Handoff.

> The Session index and Task Store contain metadata only. SessionMesh never stores Prompt,
> raw Provider output, Transcript, tool, or source-file content in either database. A validated
> Handoff Artifact is stored separately only after explicit user authorization.

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
- **Assisted Handoff:** an eligible Busy, Idle, or exited Session can generate a private
  validated Artifact through a detached, source-read-only Provider Worker after explicit
  confirmation. Open Sessions wait for Idle, revalidate the exact Current Run, request only
  its graceful exit, and never force-kill it or close its Terminal or shell.
- **Same- or cross-Provider continuation:** each authorization chooses Claude Code or Codex CLI
  as the Successor Provider, defaulting to the source Provider. Ready Results expose both a
  safely quoted Copy Command and a Terminal.app launch backed by the same validated, structured
  Launch Plan. A foreground helper inherits the interactive standard streams and stores no
  conversation content. Launch intent alone never claims linkage: the helper records a Successor
  only when Provider-native source evidence matches the exact launched process and start identity.
- **Durable Operation Tasks:** a focusable, scrollable TASKS timeline survives index rebuilds
  and TUI restarts; opening terminal outcomes acknowledges them without removing their Results.
  Every non-terminal Task can be explicitly cancelled. Failed and Cancelled Tasks can be
  retried with fresh authorization; Interrupted Tasks become retryable only after reconciliation
  proves their prior Worker and Provider are gone. Ready Tasks are never retried in place. Tasks
  and Artifacts never expire automatically; an irreversible confirmation can delete one terminal
  Task and its exact owner-only Artifact. Interrupted deletion resumes safely after restart.
- **Evidence-driven recovery:** Worker startup has a 30-second deadline and Running generation
  has a 30-minute deadline. TUI and Worker startup reconcile exact process identities and the
  declared Artifact path without accepting partial/newest output, resending a Prompt, or starting
  a Retry automatically.
- **Managed Handoff Skill:** the first explicitly authorized Handoff installs SessionMesh's
  versioned `sessionmesh-handoff` Skill at `~/.agents/skills/sessionmesh-handoff/SKILL.md`
  for Codex CLI and `~/.claude/skills/sessionmesh-handoff/SKILL.md` for Claude Code. Only a
  SessionMesh-marked copy is updated; an unrelated same-name Skill makes Handoff ineligible
  without being overwritten.

## Build & run

```sh
cargo run --release        # opens the TUI
cargo run --release -- --theme dracula  # one-process Theme override
```

Keys: `1-3` open pages · `Tab`/`Shift+Tab` move focus within a page · `Up/Down` or `j/k`
move selection or scroll Detail · `Enter` inspects a Session or Operation Task, or toggles
`OLDER SESSIONS`; from a linked Task it opens the Successor after that Session reaches the index ·
`h` opens Handoff confirmation from a Session Detail · `?` opens
Guide · `Esc` returns from Detail, closes the current layer, or closes a Ready Task for later ·
`x` opens Cancel confirmation for a non-terminal Task · `r` opens Retry authorization for an
eligible terminal Task, otherwise rescans · `Left/Right` chooses the Successor Provider during
authorization · `o` opens a Ready Successor in Terminal.app · `c` copies its validated command ·
`d` opens irreversible Delete Task and Artifact confirmation for a terminal Task · `q` quits.

Terminal.app is the v1 Terminal Launcher. If it is unavailable or launch fails, the Handoff
Result remains Ready and Copy Command stays available. Both launch paths reject missing,
malformed, or tampered Plan data; arguments are preserved across spaces, quotes, shell
metacharacters, and Unicode paths.

The foreground helper links Claude Code through the launched PID's matching state record and
links Codex CLI through the rollout held by that exact process. PID reuse, conflicting or missing
records, early exit, Copy Command, and launch paths that bypass the helper leave the Handoff Result
Ready and show `Successor Session: Not linked`; they never guess from cwd, timestamps, or the newest
source log. Once a confirmed Successor appears through the normal Session index refresh, Task Detail
offers direct navigation to its distinct Provider-native Session.

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
| `SESSIONMESH_DATA_DIR` | `~/Library/Application Support/sessionmesh` | index, Task Store, Handoff Artifacts, Profile, and model override files |
| `SESSIONMESH_EXTRA_DIRS` | — | colon-separated extra log dirs |
| `SESSIONMESH_CLAUDE_EXECUTABLE` | discovered from `PATH` | Claude CLI used by Handoff Workers |
| `SESSIONMESH_CODEX_EXECUTABLE` | discovered from `PATH` | Codex CLI used by Handoff Workers |

Local Model Catalog override: drop a `models.json` (same shape as `models/catalog.json`) in
`SESSIONMESH_DATA_DIR`. Entries are scoped by exact Model Provider and model identifier. The
bundled catalog currently includes 80 model identifiers across 9 Model Providers; its 65
mainstream source records come from the structured table at `llmcalcs.com/pricing-comparison`
(source data last updated 2026-06-11). The catalog records direct and batch API prices plus
Context Window; batch prices are metadata and are not used for interactive Session estimates.

The disposable Session index is `index.db`. The authoritative Operation Task Store is the
separate owner-only `tasks.db`; validated Handoff Artifacts are published under the owner-only
`handoff-artifacts/` directory. Local Profile preferences are stored separately in
`profile.json`; deleting that file restores first-use Guide and the default `sessionmesh`
Theme. `--theme <name>` overrides the saved Theme for one process without changing the
Profile; unknown names fall back to `sessionmesh`. Runtime Theme switching is deferred to a
future Settings feature.

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
  tasks.rs          authoritative Operation Task Store
  handoff.rs        explicit authorization + detached Handoff Worker
  successor.rs      validated Launch Plans, commands, helper, and Terminal adapter
  collector.rs      discover → index → build normalized snapshot
  alerts.rs         threshold crossings + macOS notifications
  app.rs            tabs, snapshot, formatting
  ui/               Ratatui rendering (All / provider / detail)
  main.rs           tokio background loop + crossterm event loop
```

See `CONTEXT.md` for domain vocabulary and `docs/` for the spec and plan.

## Scope (v1)

macOS only; source Projects and Worktrees stay read-only; no always-on background daemon
(an explicitly authorized Handoff Worker may outlive the TUI); no subscription quota; no
prompt/tool/file analysis in either database; no JSON output; Claude Code + Codex CLI only.

## License

MIT
