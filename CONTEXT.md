# CONTEXT.md — TokenPlay domain model

TokenPlay is a fully local, read-only macOS TUI for observing **Claude Code** and
**Codex CLI** coding sessions. It is an AI coding-session *observability* tool, not a
provider billing dashboard. Source JSONL logs are the source of truth; everything else
is a disposable, rebuildable derivation.

This file is the shared vocabulary. Provider-specific field names live inside adapters;
the rest of the codebase speaks only in the terms below.

## Session

A provider-native Claude Code or Codex CLI conversation, identified by its **native
Session ID**. Sessions are **never merged across providers** — a Claude session and a
Codex session are always distinct, even if related in time or directory.

- `Provider` ∈ { ClaudeCode, CodexCli }.
- `Project` label = basename of the session working directory. If two visible sessions
  collide on basename, prepend enough parent path to disambiguate (e.g. `dex/backend`).
- A session lists its **latest** model. Mixed-model sessions split usage in the detail view.

## Current Context

The **latest known** amount of content occupying the model's context window for a
session. It is *not* cumulative token processing.

- **Claude Code:** `input_tokens + cache_read_input_tokens` of the latest assistant
  message. **Never** add `cache_creation_input_tokens` — that is write-side billing data
  and double-counts current context.
- **Codex CLI:** derived from the latest `token_count` event's `last_token_usage`
  (`input_tokens + output_tokens`, cached input is already a subset of input) together
  with `model_context_window`.
- If the model's context window is unknown, show the raw token count but render the limit
  and percentage as `N/A`.

## Processed Usage

Cumulative, provider-reported token processing. Used for the Usage detail view and price
estimates. Categories, kept separate when the provider exposes them:

- input, output, cache read, cache creation.

Processed Usage is **kept strictly separate** from Current Context.

## API Price Estimate

An API-equivalent dollar estimate from a bundled, versioned public rate card. It is **not**
the user's subscription bill. UI shows the bare dollar amount; a single footer note reads
`Estimated using API rates`. Unknown model price → `N/A`. Never silently substitute a
nearby model.

## Session State (lifecycle)

- **Busy** — process is open and a request is running.
- **Idle** — process is open and awaiting input.
- **(no marker)** — process has exited; historical session.

Detection (macOS):

- **Claude:** `~/.claude/sessions/<pid>.json` gives `pid`, `sessionId`, `status`,
  `updatedAt` + process-start info. `status = busy` → Busy; other open statuses → Idle.
  Validate process start time against the live PID to defend against PID reuse. Missing,
  stale, or non-matching → exited.
- **Codex:** rollout metadata has no PID. Find the process holding the rollout file open
  (open fd). Combine with the latest unmatched task boundary:
  - held file + unmatched `task_started` → Busy
  - held file + `task_complete`/`turn_aborted` → Idle
  - no holder → exited (no marker)

## Context Alert

An **open** session crossing a raw Current-Context threshold:

- **Warning** at 70%.
- **Critical** at 85%.

Notify once when crossing each threshold. Reset notification state when context drops
below the threshold (including after Compaction). Only Busy/Idle sessions contribute to
`Context Alerts`; exited historical sessions never do.

## Compaction

A provider-recorded context-compression event.

- **Claude:** `type: system`, `subtype: compact_boundary`, with
  `compactMetadata.trigger` and `preTokens`.
- **Codex:** top-level `type: compacted` and/or `event_msg` payload `context_compacted`.

The first version **records observed events** (count badge + detail timeline). It does
**not** predict the next compaction.

## Time filters

`Active Today`, `Last 7 Days`, `Last 30 Days`, `All Sessions`. These select sessions by
**activity time**. A matched session always displays its **complete lifetime Usage** —
token data is never split across days. The UI states plainly that shown Usage is the full
usage of matching sessions.

## Storage & privacy (non-negotiable)

- Source JSONL files are the source of truth.
- SQLite is a **disposable incremental index**, rebuildable from source. Not authoritative.
- Store only usage metadata needed by the UI.
- **Never** store prompt bodies, generated summaries, tool arguments, tool outputs, or
  file contents.
- Handle appended, truncated, deleted, and malformed source files without crashing.
- A deleted source log disappears after reconciliation, because the index is rebuildable.
