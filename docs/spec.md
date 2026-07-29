# TokenPlay — product specification (v1)

## What it is

A fully local, read-only macOS terminal UI that observes Claude Code and Codex CLI
sessions side by side: current context pressure, live Busy/Idle/exited state, historical
usage with API-rate dollar estimates, and compaction history. Metadata only — no prompts,
tools, or file content are ever read into storage.

See `CONTEXT.md` for the authoritative domain vocabulary.

## In scope (v1)

- Rust + Ratatui + Crossterm + Tokio, macOS only.
- Read-only access to `~/.claude` and `~/.codex` logs.
- Three tabs: `All`, `Claude Code`, `Codex CLI`.
- Live polling ~every 2s; incremental updates, no full rescans on append.
- Native session IDs; providers never merged.
- Current Context per session; Busy/Idle/exited state.
- Time filters over activity time; matched sessions show full lifetime usage.
- Token categories + estimated dollars (API rates).
- Compaction count badge + detail timeline.
- SQLite incremental index, rebuildable from source.
- macOS notifications while the TUI runs.
- Bundled versioned pricing with local overrides.
- Standard dirs + env overrides + configurable extra directories.

## Out of scope (v1)

Subscription quota/remaining; prompt/summary/tool/file analysis; background daemon after
exit; Linux/Windows; JSON/non-interactive output; other agents; predicting next compaction.

## Screens

- **All** — compact comparison dashboard. Claude and Codex totals kept separate, plus a
  combined dollar estimate and session count. No per-session context bars. A
  `Context Alerts` panel counting open sessions over Warning/Critical thresholds.
- **Provider tab** — dense session list. Columns: status, project, latest model, context,
  dollars, last update, compaction badge. Sort Busy first, Idle second, then remaining by
  latest update. `Enter` opens Usage detail.
- **Detail** — context; input/output; cache read/write; dollar estimate; per-model
  breakdown; compaction timeline. No prompt or tool content.

Footer note when prices shown: `Estimated using API rates`. Unknown price → `N/A`.
Unknown context window → show token count, limit/percent `N/A`.

## Keys

`Tab`/`Shift+Tab` switch tabs · `1/2/3` jump to tab · `f` cycle time filter ·
`↑/↓` or `j/k` move selection · `Enter` open detail · `Esc` close detail · `r` force
rescan · `q` quit.
