# SessionMesh — product specification (v1)

## What it is

A fully local, read-only macOS terminal UI that observes Claude Code and Codex CLI
sessions side by side: current context pressure, live Busy/Idle/exited state, Current Run
and Transcript Lifetime Usage with API-rate dollar estimates, and compaction history.
Metadata only — no prompts, tools, or file content are ever read into storage.

See `CONTEXT.md` for the authoritative domain vocabulary.

## In scope (v1)

- Rust + Ratatui + Crossterm + Tokio, macOS only.
- Read-only access to `~/.claude` and `~/.codex` logs.
- Three directly addressable pages: `[1]` All, `[2]` Claude Code, `[3]` Codex CLI.
- First-use contextual Guide persisted per local Profile outside the disposable Session
  index.
- Six semantic Themes with Profile persistence and a process-only `--theme <name>`
  override. Interactive Theme selection is deferred to Settings.
- Live polling ~every 2s; incremental updates, no full rescans on append.
- Native session IDs; providers never merged.
- Current Context per session; Busy/Idle/exited state.
- Provider pages group Sessions by Project with direct rolling-seven-day rows and
  independent per-Project `OLDER SESSIONS` disclosures.
- Current Run and Transcript Lifetime token categories + estimated dollars (API rates).
- Compaction count badge + detail timeline.
- SQLite incremental index, rebuildable from source.
- macOS notifications while the TUI runs.
- Bundled, Model-Provider-scoped Model Catalog with sourced direct/Batch API prices, Context
  Windows, and local overrides.
- Standard dirs + env overrides + configurable extra directories.

## Out of scope (v1)

Subscription quota/remaining; prompt/summary/tool/file analysis; background daemon after
exit; Linux/Windows; JSON/non-interactive output; other agents; predicting next compaction.

## Screens

- **All** — cross-provider `NOW`, grouped by Project and Worktree, beside a bounded
  process-lifetime `RECENT SIGNALS` list. `NOW` contains Open Sessions and current
  Attention evidence; Signals contain only meaningful Session, Context, and Compaction
  transitions observed after the silent initial baseline.
- **Provider page** — Sessions grouped by canonical Project identity. Project groups and
  their Sessions sort by latest activity. Sessions from the rolling seven-day window render
  directly; older Sessions live in an independent collapsed disclosure under their Project.
  Every Session row retains complete transcript-lifetime Usage and opens shared Detail.
- **Detail** — full-page child view addressed by Provider plus native Session ID while
  keeping numeric navigation visible. Open Sessions show separately labelled Current Run
  Usage and Transcript Lifetime Usage, including every provider-reported category,
  per-model splits, and API price estimates. An unavailable validated Run boundary shows
  Current Run Usage as `N/A`; Exited Sessions omit Current Run Usage. Detail also shows
  Current Context and Compactions, with no Recent Signals, prompt, or tool content.
- **Guide** — centered two-pane reference opened globally with `?`. Its sidebar exposes
  Overview, Navigation, Sessions, Signals, and Theme. It initially selects Signals from
  `RECENT SIGNALS`, Sessions from Provider pages or Detail, and Overview from `NOW`, first
  use, or an unknown context.

Footer note when prices shown: `Estimated using API rates`. Unknown price → `N/A`.
Unknown context window → show token count, limit/percent `N/A`.

## Keys

`1/2/3` open All, Claude Code, and Codex CLI · `Tab`/`Shift+Tab` move focus within the
current page and are no-ops when only one region is focusable · `Up/Down` or `j/k` move
selection or scroll Detail · `Enter` opens Detail or toggles a Project's `OLDER SESSIONS` ·
`?` opens Guide · `Esc` returns from Detail or closes the current layer · `r` forces a
rescan · `q` quits.

Inside Guide, `Up/Down` or `j/k` changes the selected category, `Tab` or Right enters the
content pane, and `Shift+Tab` or Left returns to categories. The content pane uses the same
vertical movement keys to scroll. Each opening starts at its contextual category and the
top of its content.

Built-in Themes are `sessionmesh`, `catppuccin`, `dracula`, `gruvbox`, `light`, and
`high-contrast`; `sessionmesh` is the default. The saved local Profile Theme is loaded at
startup. `--theme <name>` applies a process-local override without changing that Profile;
unknown names fall back to `sessionmesh` without preventing startup.
