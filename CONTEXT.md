# CONTEXT.md — SessionMesh domain model

SessionMesh is a fully local macOS TUI for observing **Claude Code** and **Codex CLI**
coding sessions and performing explicitly authorized local operations on them. Passive
observation is the default; an alert or threshold never authorizes an operation. It is
not a provider billing dashboard. Source JSONL logs are the source of truth for observed
session state; everything derived from them is disposable and rebuildable.

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

## Session Label

A stable user-facing name used to recognize one Session. It does not claim to describe the
Session's current work, lifecycle state, or latest goal; the UI presents it as `Title`.
Provider-native or Provider-generated titles are not Session Label sources.
Session Labels are not unique and never participate in Session identity. When visible
labels collide, the shortened native Session ID disambiguates them.

The displayed Session Label uses this precedence: Session Alias, Generated Session Label,
then shortened native Session ID.

_Avoid:_ Summary, Intent, Current Task, when referring to Session display identity.

## Session Alias

A SessionMesh-owned Session Label explicitly assigned by the user to one Session. It is a
local nickname and never renames or modifies the provider-native Session. It remains
associated across Current Runs, SessionMesh restarts, and temporary source-log absence
until the user explicitly clears it.

_Avoid:_ Provider title, Project label, Current Task.

## Generated Session Label

A SessionMesh-owned Session Label created only when the user explicitly invokes
`Generate title`. It is never generated in the background, remains distinct from a
user-written Session Alias, and is ignored whenever an Alias exists for the same Session.

_Avoid:_ Automatic summary, Provider title, Current Task.

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

## Control Operation

A bounded, state-changing action that SessionMesh performs on a selected Session only
after the user explicitly authorizes it. Attention Conditions, Alerts, thresholds, and
elapsed time may offer a Control Operation but never authorize one. One authorization
covers one declared operation workflow only until its declared deadline, and expires
earlier when that workflow succeeds, fails, or is cancelled; it grants no persistent
control over a Session or Project.

_Avoid:_ Automatic remediation, background action, when no user authorization exists.

## Operation Task

One execution instance of an authorized Control Operation. Multiple Operation Tasks may
coexist; each identifies its operation type, selected target, execution state, timestamps,
and result reference without storing provider conversation content. Once started, an
authorized Operation Task continues independently of whether the SessionMesh TUI remains
open. A Session may have at most one non-terminal Operation Task; tasks targeting different
Sessions may execute concurrently, but a second authorization is never silently queued
behind an existing task. Losing the executing process triggers reconciliation against live
process and declared-result evidence; an uncertain outcome becomes Interrupted and is never
retried without new user authorization. A terminal Operation Task never reopens; a manual
retry is a distinct Task linked to its predecessor.

_Avoid:_ Provider task, when referring to work managed by SessionMesh; Control Operation,
when referring to one execution rather than the authorized action type.

## Operation Task State

The execution outcome of an Operation Task. `Queued`, `Waiting`, and `Running` are
non-terminal; `Ready`, `Failed`, `Interrupted`, and `Cancelled` are terminal. `Ready`
means a valid declared result exists, `Failed` means failure is known, `Interrupted`
means the outcome is uncertain, and `Cancelled` means execution stopped before uncertain
side effects occurred, including when an unused authorization reaches its deadline. A
cancellation request expresses user intent; it becomes `Cancelled` only when process and
result evidence make that outcome certain.

## Task Acknowledgement

Metadata recording whether the user has seen a terminal Operation Task or its result.
It is created only by deliberate result or notice interaction, never inferred from
notification delivery, panel rendering, or TUI startup. Acknowledging, viewing, copying,
launching, or dismissing a notice never changes the Operation Task State.

_Avoid:_ Completed, dismissed, when referring to whether a Task result has been seen.

## Handoff Operation

A Control Operation whose Operation Task asks one source Session to produce a Handoff
Result for a future Successor Session. It is complete when the result is ready; launching
the successor is a separate user choice. For an Open source Session, its declared workflow
may include ending the Current Run only with explicit authorization; it waits for Idle,
revalidates the target process, and never force-kills a Busy or unverified process. Handoff
generation is strictly read-only with respect to the source Project and Worktree. Context
Alerts may recommend a Handoff but never determine its eligibility or authorization.

_Avoid:_ Task, automatic handoff.

## Handoff Result

The pair produced by a successful Handoff Operation: one local Handoff Artifact and one
Successor Launch Plan that yields a provider-specific Successor Launch Command. A result
can be launched, copied, or left for later without changing whether the Handoff Operation
completed successfully. Dismissing its completion notice preserves the result.

## Handoff Artifact

A local, structured Markdown document containing the working context needed by a Successor
Session. SessionMesh owns it in the private SessionMesh data directory, tracks its location
and readiness, and does not place its contents in either the disposable Session index or
authoritative Task Store. It remains until its terminal Operation Task is explicitly
deleted; exporting an Artifact into a Project is a separate user choice.

_Avoid:_ Transcript, generated summary, when referring to the persisted handoff document.

## Successor Launch Plan

A platform-neutral description of how to start the selected Successor Provider in the
source working directory with access to the Handoff Artifact. It can be rendered as a
copyable Successor Launch Command or executed by a platform-specific Terminal Launcher;
either interaction is independent of Handoff completion.

## Successor Session

A new provider-native Session started from a Handoff Result. It is related to its source
Session but has a distinct native Session ID and does not inherit the source transcript.
Its Provider may differ from the source Provider; the Handoff Artifact is their only
working-context bridge. SessionMesh records the relationship only after Provider evidence
ties the launched process to the new native Session ID; launch intent alone is insufficient.

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

- Provider source JSONL files are the source of truth for observed Session data.
- The Session SQLite index is a **disposable incremental index**, rebuildable from Provider
  sources. It is not authoritative.
- The Task Store is authoritative only for user-authorized Operation Task metadata and
  result references; it is separate from the disposable Session index.
- A Handoff Artifact is the authoritative local content of its Handoff Result. Its contents
  are never copied into either store.
- Operation Tasks and Handoff Artifacts do not expire automatically; explicit deletion of
  a terminal Task removes its Task Store record and owned Artifact without affecting either
  Provider Session.
- Stores contain only the metadata required by their declared responsibilities. They never
  contain prompt bodies, generated summaries, tool arguments, tool outputs, or file contents.
- Handle appended, truncated, deleted, and malformed source files without crashing.
- A deleted source log disappears after reconciliation, because the index is rebuildable.
