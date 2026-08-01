# CONTEXT.md — SessionMesh domain model

SessionMesh is a fully local, read-only macOS TUI for observing **Claude Code** and
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
- A Session belongs to one Project and, for a Git Project, one Worktree according to its
  working directory.
- A session lists its **latest** model. Mixed-model sessions split usage in the detail view.

## Project

A local repository-scale unit of related coding work. Git working directories with the
same canonical Git common directory belong to one Project; independent clones remain
distinct even when they use the same remote. A non-Git canonical working directory is a
standalone Project.

The display label uses the working directory or repository basename and adds parent-path
context when visible labels collide.

_Avoid:_ Remote repository, project label, when referring to Project identity.

## Worktree

A specific Git working copy within a Project, identified by its canonical Git top-level
directory. Linked Worktrees share a Project but remain distinct working copies.

_Avoid:_ Project, repository, branch.

## Current Run

The currently active provider CLI invocation attached to a Session. Resuming the same
native Session in a new process creates a new Run without creating a new Session.

_Avoid:_ Current Session, when referring only to usage since the latest invocation began.

## Open Session

A Session currently attached to a validated live provider CLI process. An Open Session is
either Busy or Idle; a Session without a live process is Exited even if its source log
still exists and the Session can later be resumed.

_Avoid:_ Active Session, live log, closed Session.

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

Cumulative, provider-reported token processing. Every displayed value has an explicit
scope and keeps provider categories separate when available:

- input, output, cache read, cache creation.

**Current Run Usage** is Processed Usage belonging to the Current Run. It exists only
while the Session is Open and is the scope users compare with the provider's in-session
usage display; an Exited Session has no Current Run Usage or Last Run Usage.

**Transcript Lifetime Usage** is Processed Usage across the complete provider-native
Session transcript, including history inherited when the Session is resumed.

_Avoid:_ Session Usage or Total Usage without an explicit scope; Last Run Usage.

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

## Shared Worktree Risk

A structural concurrency risk that exists when two or more distinct Open Sessions resolve
to the same writable Git worktree. It indicates possible interference in one working copy,
not an observed file-level or Git merge conflict. It has Warning severity and does not
become Critical solely because more Sessions share the worktree.

_Avoid:_ Worktree conflict, candidate conflict.

## Attention Condition

An evidence-backed current condition on a Session or worktree that may warrant user
inspection. An Idle Session is a neutral Attention Condition; Attention Conditions are not
Alerts unless they carry Warning or Critical severity.

_Avoid:_ Attention state, when referring to a condition independent of Session lifecycle.

## Recent Signal

A deduplicated, metadata-only record that a meaningful Session, Context, or Worktree
condition changed recently. A Recent Signal describes a transition, not current state and
never provider conversation content.

_Avoid:_ Session history, transcript history, prompt history.

## Compaction

A provider-recorded context-compression event.

- **Claude:** `type: system`, `subtype: compact_boundary`, with
  `compactMetadata.trigger` and `preTokens`.
- **Codex:** top-level `type: compacted` and/or `event_msg` payload `context_compacted`.

The first version **records observed events** (count badge + detail timeline). It does
**not** predict the next compaction.

## Session recency

A Session is **Recent** when its last activity falls within the rolling seven-day window
and **Older** when it predates that cutoff, regardless of lifecycle state. Recency never
changes the Session's complete lifetime Usage scope.

_Avoid:_ History, when referring to Older Sessions rather than Signal History.

## Storage & privacy (non-negotiable)

- Source JSONL files are the source of truth.
- SQLite is a **disposable incremental index**, rebuildable from source. Not authoritative.
- Store only usage metadata needed by the UI.
- **Never** store prompt bodies, generated summaries, tool arguments, tool outputs, or
  file contents.
- Handle appended, truncated, deleted, and malformed source files without crashing.
- A deleted source log disappears after reconciliation, because the index is rebuildable.
