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
- Three directly addressable pages: `[1]` All, `[2]` Claude Code, `[3]` Codex CLI.
- First-use Help persisted per local Profile outside the disposable Session index.
- Six semantic Themes with runtime cycling, Profile persistence, and a process-only
  `--theme <name>` override.
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
- **Provider page** — dense session list. Columns: status, project, latest model, context,
  dollars, last update, compaction badge. Sort Busy first, Idle second, then remaining by
  latest update. `Enter` opens Usage detail.
- **Detail** — full-page child view addressed by Provider plus native Session ID while
  keeping numeric navigation visible. Shows Current Context, transcript-lifetime
  input/output/cache Usage, dollar estimate, per-model Usage, and Compactions. It has no
  Recent Signals, prompt, or tool content.

Footer note when prices shown: `Estimated using API rates`. Unknown price → `N/A`.
Unknown context window → show token count, limit/percent `N/A`.

## Keys

`1/2/3` open All, Claude Code, and Codex CLI · `Tab`/`Shift+Tab` move focus within the
current page and are no-ops when only one region is focusable · `f` cycles the time
filter on top-level pages · `t` cycles Themes · `Up/Down` or `j/k` move selection or
scroll Detail · `Enter` opens Detail · `?` opens Help · `Esc` returns from Detail or
closes the current layer · `r` forces a rescan · `q` quits.

Theme cycle order: `tokenplay`, `catppuccin`, `dracula`, `gruvbox`, `light`, then
`high-contrast`. `tokenplay` is the default. Runtime selection is saved in the local
Profile unless `--theme <name>` supplied the process-local override. Unknown Theme names
fall back to `tokenplay` without preventing startup.
