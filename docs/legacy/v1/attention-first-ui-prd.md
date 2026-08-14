# Mudi Attention-First UI PRD

> Legacy local record. It captures the proposed post-v1 product direction at the time of
> writing. Formal PRDs and delivery tickets now live in GitHub Issues; accepted version
> scope and architecture decisions live in `feature/` and `docs/adr/`.

| Field | Value |
|---|---|
| Status | Legacy local record |
| Last updated | 2026-08-09 |
| Product phase | Post-v1 exploration |
| Related baseline | `product-specification.md` |
| Assisted Handoff | `assisted-handoff-prd.md` |
| Production render evidence | `tests/fixtures/production-renders/` |

## 1. Document contract

This document is the working source for the attention-first product direction. It does
not supersede the v1 specification until the relevant requirements are confirmed.

Decision statuses:

- **Proposed** - captured for discussion; not implementation scope yet.
- **Confirmed** - agreed product behavior; eligible for planning and implementation.
- **Deferred** - valuable, but intentionally postponed.
- **Rejected** - considered and explicitly excluded, with the reason recorded.

When a new requirement is confirmed, update its status, acceptance criteria, affected
screens, and the decision log in the same change.

## 2. Background

The v1 UI is primarily a monitoring dashboard. It shows provider totals, session state,
Current Context occupancy, historical usage, estimated cost, and compaction history.

The higher-value user problem is not merely observing those metrics. A user running
several Claude Code and Codex CLI sessions needs to answer:

> Which coding session needs my attention now, why, and what should I inspect next?

Provider-centric navigation makes it harder to see cross-provider relationships. In
particular, two sessions from different providers may affect the same project or writable
Git worktree. A metrics-first dashboard also makes the user interpret raw state before
deciding what to do.

## 3. Product position

Mudi should evolve from a token and session monitor into a local attention console
for concurrent AI coding sessions.

The product promise is:

> See where intervention is needed, where concurrent agents create Shared Worktree Risk,
> and what changed while you were away, without storing prompt or source-code content.

## 4. Goals

- Make the most important current issue visible immediately after launch.
- Explain why every Attention Condition exists using observable evidence.
- Reveal project and worktree relationships across providers.
- Support investigation of recent state changes without reading prompt content.
- Preserve local, metadata-minimizing observation while requiring explicit authorization
  for every bounded Control Operation.
- Keep the interface dense, keyboard-first, and suitable for repeated terminal use.

## 5. Non-goals

- Resume, terminate, compact, or modify provider sessions without one explicit,
  task-scoped user authorization.
- Persist source Prompt bodies, raw Transcript responses, tool content, or source-code
  contents in the Session index or authoritative Task Store.
- Present heuristic classifications as facts without evidence.
- Replace the complete session list or usage detail views.
- Turn the default screen into a cost analytics dashboard.
- Add remote telemetry as a dependency for measuring product usefulness.

## 6. Primary navigation and Guide

### NAV-001 - Three numeric primary pages

**Status: Confirmed**

The persistent top-level navigation contains exactly three numeric targets:

```text
[1]  [2]  [3]
```

| Key | Page |
|---:|---|
| `1` | All - the current main/global page |
| `2` | Claude Code sessions |
| `3` | Codex CLI sessions |

The persistent navigation must not render `All`, `Claude Code`, or `Codex CLI` beside the
numbers. The active number is visually highlighted. Page content may identify its
provider where needed; the restriction applies to the persistent numeric navigation.

Project and Worktree organization is part of `NOW`, not a separate navigation target.
Full Signal History is Deferred and is not a top-level page, detail section, overlay, or
main-page region in the first attention-first implementation.

### NAV-002 - Contextual Guide modal

**Status: Confirmed**

`?` opens one global, centered Guide modal at 80% of terminal width and 76% of terminal
height. Its left 24% sidebar lists Overview, Navigation, Sessions, Signals, and Theme; the
right pane shows the selected category. Category selection updates content immediately.

The same shortcut and complete category list are available everywhere. Context only sets
the initial category: `RECENT SIGNALS` opens Signals, Provider pages and Session Detail
open Sessions, and `NOW` or an unknown context opens Overview. Every explicit opening
starts at the contextual category and the top of its content.

`Up/Down` or `j/k` changes the selected category. `Tab` or Right enters the content pane,
where the same vertical keys scroll; `Shift+Tab` or Left returns to the sidebar. Overflowing
content shows a scrollbar and position indicator, while content that fits shows neither.

Guide opens automatically on Overview only on the first launch for a local Profile.
Dismissing it stores the existing compatible `help_seen` preference outside the disposable
Session index; later launches do not auto-open it, and deleting that local preference
restores first-launch behavior.

## 7. Global interaction principles

### UI-001 - Decision before metrics

**Status: Confirmed**

The first viewport prioritizes current Attention Conditions and evidence. The main screen
does not display Processed Usage, Estimated Cost, or Provider aggregate usage/cost.
Current Context remains visible because it is current operational state, not cumulative
Processed Usage. Usage and cost remain available on Provider pages and in Session detail.

### UI-002 - Progressive disclosure

**Status: Confirmed**

Information is revealed in this order:

```text
Attention Condition -> reason/evidence -> Session detail -> complete Usage
```

The list must contain enough evidence to trust the classification without requiring the
detail view for every item:

- a Context Alert shows the stable-width Context progress bar, exact percentage, and the
  threshold it crossed;
- Shared Worktree Risk appears at the Worktree level with the count of contributing Open
  Sessions; the contributing Session rows immediately below are the member evidence and
  are not repeated in a second list;
- Idle shows elapsed time since the observed Busy-to-Idle transition, falling back to last
  activity when that transition time is unavailable;
- healthy Busy shows Current Context and last activity with lower visual emphasis.

### UI-003 - One primary action per selected item

**Status: Confirmed**

The persistent contextual Footer exposes one primary command for the selected item,
normally `Enter Inspect` for a Session. The selected-row cursor identifies the command's
target; do not repeat the same command text on every Session row. Secondary commands such
as filtering must not compete visually with it. `Acknowledge` and `Snooze` are Deferred
and do not appear as commands in the first attention-first implementation.

### UI-004 - Healthy state stays quiet

**Status: Confirmed**

Healthy running sessions remain visible in a lower-emphasis roster or inventory, but must
not compete with critical or warning items.

### UI-010 - Idle is neutral Attention

**Status: Confirmed**

Every Idle Open Session has a neutral Attention Condition because its provider process is
awaiting input. Idle is not by itself a Warning or Critical Alert: it does not use the alert
palette and does not trigger a system notification. It ranks below Warning conditions and
above healthy Busy Sessions according to `UI-011`.

### UI-011 - Cross-kind Attention priority

**Status: Confirmed**

Current conditions use the following descending priority:

1. Critical Context Alert (`Current Context >= 85%`);
2. Shared Worktree Risk;
3. Warning Context Alert (`Current Context` from 70% through 84%);
4. neutral Idle Attention;
5. healthy Busy with no Attention Condition.

A Session or Project may carry multiple conditions. Its ordering rank is determined by
the highest-priority current condition while the UI retains the lower-priority evidence.
Lower-priority conditions do not alter the ordering rank.

Items with the same rank use condition-specific evidence strength first:

- Context Alerts sort by Current Context percentage, highest first;
- Shared Worktree Risks sort by contributing Open Session count, highest first;
- Idle Sessions sort by the time they most recently entered Idle, newest first;
- healthy Busy Sessions sort by last activity, newest first.

Remaining ties use the most recent relevant transition or activity, then canonical Project
and Worktree path, then Provider plus native Session ID. If an Idle transition timestamp
is unavailable, last activity is the permitted ordering fallback. These rules form a
visible, deterministic lexicographic order; Mudi must not replace them with an opaque
weighted score.

## 8. Main screen (`1`)

### 8.1 Purpose

The All screen (`1`) is a compact terminal workspace rather than a full-screen Session
table or a large summary surface. It gives the user a current-session inventory and a
small record of meaningful recent changes without duplicating the same Sessions in a
separate Attention queue.

### 8.2 Layout

**Status: Confirmed at region level**

The terminal may be divided into multiple regions. The observation surface has two
existing regions, and the explicitly authorized control extension adds a third:

| Region | Status | Responsibility |
|---|---|---|
| `NOW` | Confirmed | Open Sessions grouped by Project, with compact facts for the selected Session |
| `RECENT SIGNALS` | Confirmed | A short list of meaningful recent metadata changes |
| `TASKS` | Confirmed | Time-ordered Operation Tasks and their current execution states |
| Reserved upper-left region | Confirmed empty | Remains unlabelled and unassigned |

The page has a compact upper status strip and a lower workspace that owns all remaining
height and the complete page width. The status strip remains eight rows high at the
supported full and compact target sizes. Its left third stays empty by design; the
remaining right side places `RECENT SIGNALS` and `TASKS` next to each other with equal
widths. Neither status panel expands leftward merely because it has more items.

One vertical divider splits the lower workspace into a left inventory and the Session
Workspace. The left inventory places `PROJECT` above the compact selected-session
`DETAILS`, separated by one horizontal rule. `DETAILS` sizes to its content at roughly the
lower third of the inventory, and `PROJECT` receives the remaining height. Compact-width
degradation and Task navigation are specified separately.

### 8.3 Operation Task state model

**Status: Confirmed**

Every Operation Task has exactly one execution state:

| State | Terminal | Meaning |
|---|---:|---|
| `Queued` | No | Authorization is stored, but the Worker has not started. |
| `Waiting` | No | Execution is waiting for a declared external condition, such as the source Current Run becoming Idle and exiting safely. |
| `Running` | No | Worker or Provider execution is active. |
| `Ready` | Yes | The declared result exists and has passed validation. |
| `Failed` | Yes | A known failure occurred and no valid result exists. |
| `Interrupted` | Yes | The outcome cannot be established safely and requires a new user decision. |
| `Cancelled` | Yes | Execution stopped before any uncertain external side effect occurred. |

The normal successful path is `Queued -> Waiting -> Running -> Ready`; a Task may omit
`Waiting` when no external condition blocks it. A known error may move a non-terminal Task
to `Failed`. Cancellation may produce `Cancelled` only when Mudi can establish that
no uncertain external side effect occurred; otherwise an uncertain outcome is
`Interrupted`. Worker loss invokes reconciliation: a valid declared result becomes
`Ready`, confirmed live execution remains in its current non-terminal state, and an
unresolvable outcome becomes `Interrupted`. Mudi never silently retries or
resubmits an `Interrupted` Task.

Execution state is independent of Task Acknowledgement. Closing a completion notice,
opening the Artifact, copying the Successor Launch Command, or starting a Successor
Session does not move a `Ready` Task to another state. The Task Store records separate
acknowledgement and interaction timestamps where required by the UI.

### 8.4 `TASKS` ordering

**Status: Confirmed**

`TASKS` is one unified timeline ordered by each Task's most recent execution-state
transition, newest first. It does not partition or group rows by state. Creating a Task
and every transition among `Queued`, `Waiting`, `Running`, and a terminal state updates
that ordering timestamp; Worker heartbeats, progress observations, notice delivery, and
user interaction metadata do not. Consequently, a newly completed, failed, cancelled, or
interrupted Task moves to the top, while routine refreshes do not make rows jump.

Remaining ties use Task creation time, then the stable Task ID, both newest first.

### 8.5 Task Acknowledgement

**Status: Confirmed**

A terminal Task receives `acknowledged_at` only after deliberate user interaction:

- opening its Task detail;
- choosing `Open Terminal`, `Copy Command`, or `Close for Later` for a Ready result;
- viewing or dismissing a `Failed`, `Interrupted`, or `Cancelled` notice.

Delivering a macOS notification, rendering a row in `TASKS`, or loading the Task when the
TUI starts does not acknowledge it because none proves that the user saw the outcome.
Acknowledgement controls only unread presentation; it neither changes execution state nor
implies that a Handoff Result was launched, copied, or deleted. Those interactions retain
their own timestamps when needed.

### 8.6 `TASKS` visibility and panel retention

**Status: Confirmed**

`TASKS` displays every Operation Task still retained in the Task Store, including
acknowledged terminal Tasks. Acknowledgement removes only the unread treatment; it never
hides or removes a row. The panel is keyboard-scrollable when the retained timeline
exceeds its visible height, and compact layouts may reduce the viewport without dropping
Tasks from the timeline.

A Task leaves the panel only when a separately confirmed retention or explicit-deletion
rule removes it from the Task Store. The first implementation does not require a separate
Task History page merely to recover a Ready Handoff Result after `Close for Later`.

### 8.7 Operation Task timeouts

**Status: Confirmed**

The first implementation uses fixed stage-specific deadlines:

| State | Deadline | Outcome at deadline |
|---|---:|---|
| `Queued` | 30 seconds | If Worker startup cannot be established, become `Failed`. |
| `Waiting` | 2 hours | Expire the unused authorization and become `Cancelled`. |
| `Running` | 30 minutes | Request graceful termination of the Task-owned Provider process, then reconcile. |

The `Running` deadline begins graceful termination rather than escalating to `SIGKILL`.
While a verified process is still shutting down, it remains `Running`. Reconciliation
makes a valid declared result `Ready`, a known failure without a valid result `Failed`,
and an outcome that cannot be established `Interrupted`. Timeout never resubmits the
Prompt or creates a retry automatically.

The `Waiting` deadline prevents a stale authorization from ending a Current Run long
after the user's original intent. A later retry is a new Operation Task and requires new
authorization. Configurable deadlines are deferred beyond the first implementation.

### 8.8 Operation Task cancellation

**Status: Confirmed**

Every non-terminal Task offers `Cancel Task` and requires confirmation before Mudi
acts. Cancellation is a requested intent, not a forced terminal-state assignment:

- if no Provider Prompt or control action was sent, cancellation becomes `Cancelled`;
- if a Task-owned Provider process started, Mudi requests graceful termination and
  never escalates automatically to `SIGKILL`;
- after termination, a valid declared result becomes `Ready`, a verified stop without a
  valid result becomes `Cancelled`, and an outcome that cannot be established becomes
  `Interrupted`.

Cancellation never closes the user's Terminal or shell, never targets an unverified
process, and never creates or starts a retry. While a verified child is still shutting
down, the Task remains non-terminal so the same Session cannot start another Operation
Task concurrently.

### 8.9 Manual retry

**Status: Confirmed**

Retry never reopens or mutates a terminal Task. `Retry` on a `Failed` or `Cancelled` Task
creates a new Operation Task with a new Task ID, fresh authorization, new Artifact path,
and `retry_of_task_id` pointing to its predecessor. The predecessor and any evidence or
partial metadata remain unchanged.

Before creating the new Task, Mudi revalidates the source Session, process,
Project or Worktree, and the one-non-terminal-Task concurrency rule. An `Interrupted`
Task offers Retry only after reconciliation proves that its previous Worker and Provider
process are no longer running; uncertainty must not create concurrent transcript writers.
A `Ready` Task does not offer Retry. Regenerating its result is a new Handoff Operation.

No manual retry reuses a previous authorization or overwrites an existing Artifact.

### 8.10 Handoff eligibility

**Status: Confirmed**

Current Context and Context Alerts may recommend a Handoff but are never eligibility gates.
A user may authorize Handoff at any Context percentage or when Current Context is unknown.
Session lifecycle changes the workflow rather than eligibility: Busy, Idle, and Exited
Sessions are all eligible when the following preconditions hold:

- the source Provider is supported and its CLI executable is available;
- the native Session ID and provider source Session record still exist;
- the original working directory exists and is readable;
- the Session has no other non-terminal Operation Task.

An Open Session additionally requires the confirmation flow to disclose and obtain
authorization to end its Current Run safely. Without that authorization, Mudi does
not create the Handoff Task. Provider authentication and similar conditions that cannot be
reliably established by preflight are execution failures, not false eligibility claims.

Attention Conditions, thresholds, and notifications never create a Task or satisfy any
authorization requirement.

### 8.11 Successor Provider

**Status: Confirmed**

The Handoff confirmation flow requires a Successor Provider and defaults it to the source
Session's Provider. The user may instead select another supported Provider whose local CLI
executable is available. Same-Provider and cross-Provider Handoffs use the same Artifact
contract.

The source Provider always resumes the source Session and produces the candidate Artifact.
The selected Successor Provider determines the Successor Launch Command only. A
cross-Provider Handoff never converts or merges Provider-native Session IDs, Transcripts,
or model context; the structured Handoff Artifact is the sole working-context bridge to a
new independent Successor Session.

The selected Successor Provider is stored as Task metadata and is revalidated before a
launch attempt.

### 8.12 Successor launch

**Status: Confirmed**

Handoff completion produces a platform-neutral Successor Launch Plan containing the
working directory, selected Provider executable, structured argument list, and Handoff
Artifact reference. Core Handoff and Task logic never embeds AppleScript or constructs an
unescaped shell command. Platform-specific launch and copy renderers consume the same Plan.

The first implementation provides:

- a macOS `Terminal.app` launcher that opens a new window in the source working directory;
- a shell-appropriate `Copy Command` fallback derived from the same structured Plan;
- preservation of the new shell after the Successor Provider CLI exits.

Failure to open a terminal or start its command does not change a `Ready` Task because
launching is a later user interaction, not Handoff execution. The UI reports the launch
failure and keeps `Copy Command` available.

The Terminal Launcher is an explicit module seam. Future Linux, Windows, iTerm2, Ghostty,
Warp, or other launch adapters may be added without changing Handoff generation, Artifact
validation, Task state, or Provider command selection. The first implementation does not
ship untested non-macOS adapters; unsupported terminals use `Copy Command`.

### 8.13 Successor Session linkage

**Status: Confirmed**

Both `Open Terminal` and `Copy Command` use a foreground Mudi launch helper that
loads the Task's validated Successor Launch Plan, starts the selected Provider CLI with
the Terminal's inherited standard input and output, and records only process and Task
metadata. It does not proxy or persist interactive Provider content.

Claude and Codex use the same external linkage contract: the helper records the launched
process identity, and the target Provider adapter resolves that process to a newly observed
native Session ID. Claude validates the PID-associated state record; Codex validates the
rollout file held by the launched process. Both paths apply process-start validation to
protect against PID reuse and confirm that the Provider source record exists before storing
`successor_session_id` and `successor_linked_at`.

Mudi does not preassign a Claude Session ID and never correlates either Provider by
choosing the newest Session in a working directory. If the user changes the copied command,
bypasses the helper, or linkage evidence never becomes conclusive, the Handoff remains
`Ready` and displays `Successor Session not linked`; uncertainty never creates a guessed
relationship or changes Task execution state.

### 8.14 Task and Artifact retention

**Status: Confirmed**

The first implementation applies no automatic expiry to Operation Tasks or Handoff
Artifacts. Acknowledgement, command copying, Successor launch, and Successor linkage do not
start a retention countdown. This preserves the promise made by `Close for Later`.

Only a terminal Task offers `Delete Task and Artifact`, and the action requires an explicit
irreversible-deletion confirmation. A non-terminal Task must first complete or pass through
the confirmed cancellation workflow. Successful deletion removes the Task Store record,
owned Artifact, Successor Launch Plan, and Task interaction metadata, after which the Task
leaves `TASKS`.

Deleting a Task never deletes or modifies its source Session, linked Successor Session,
Provider Transcript, Project, Worktree, or open Terminal. A retained retry may preserve
only the deleted predecessor's Task ID as an unavailable historical reference.

Automatic retention periods, bulk deletion, and storage-usage management are deferred.

### 8.15 Accepted failure walkthrough

**Status: Confirmed**

The failure contract is exercised by this accepted scenario:

1. A Busy Codex source Session starts a Handoff to a Claude Successor, with explicit
   authorization to end the Current Run.
2. The Task waits for Idle, ends the validated source process, resumes the source Session,
   and sends the Handoff Prompt.
3. The Worker then disappears before atomically publishing a valid Artifact. Reconciliation
   proves that its processes exited but cannot establish the Prompt outcome, so the Task
   becomes `Interrupted`; temporary output is never promoted and the Prompt is not resent.
4. The unread terminal Task remains in `TASKS` and produces a macOS notification. Retry is
   available only after liveness is conclusive, requires fresh authorization, and creates a
   new Task ID and Artifact.
5. If the retry becomes `Ready` but `Open Terminal` later fails, the Task remains `Ready`
   and `Copy Command` remains available.
6. If the Claude Successor starts but its native ID cannot be linked conclusively, the Task
   remains `Ready` with `Successor Session not linked` rather than a guessed relationship.

This keeps Handoff execution, terminal launch, and Successor linkage as three independent
facts with independent evidence.

### 8.16 `NOW`

**Status: Confirmed**

- Show only Projects that contain at least one Open Session.
- Organize Open Sessions as `Project -> Worktree -> Session` while preserving one
  selectable row per Session.
- Identify a Git Project by canonical Git common directory and a Worktree by canonical
  Git top-level path. Linked Worktrees belong to one Project; independent clones remain
  distinct even when they share a remote URL.
- Place non-Git Sessions directly in a standalone Project identified by canonical `cwd`,
  with Git-only fields unavailable.
- Show branch, Worktree root, Shared Worktree Risk, and highest Current Context role at the
  Worktree level. Keep state, Provider, model, Current Context progress, and last activity
  scannable in each Session row.
- Render Current Context as the stable-width progress bar plus percentage defined by
  `UI-008`; only compact-width degradation may reduce it to the percentage alone.
- Order Projects by current Attention priority, with the highest-priority Project first.
- Apply the same confirmed Attention order within each Project and Worktree; the first
  Session row is the recommended inspection target.
- Do not apply a time filter on the main screen.
- Do not duplicate Sessions in a separate Attention list.
- Do not display Processed Usage, Estimated Cost, or Provider aggregate usage/cost.
- Context threshold colors follow `UI-008`.

### 8.17 Shared Worktree Risk

**Status: Confirmed**

A Shared Worktree Risk exists when two or more distinct Open Sessions resolve to the same
writable Git Worktree. Busy and Idle Sessions are Open; Exited Sessions do not contribute.
The count uses provider-native Session identity, not the number of discovered source-log
paths.

Sharing only a Project while using separate Worktrees does not create this risk. If
Worktree identity cannot be resolved, the state is unknown and Mudi must not infer a
risk. Shared Worktree Risk reports structural exposure only; it does not claim that the
Sessions edit the same files or that a file-level or Git merge conflict has occurred.

Shared Worktree Risk always has Warning severity in the first version. The number of
contributing Open Sessions is displayed as evidence but does not by itself escalate the
risk to Critical.

### 8.18 `RECENT SIGNALS`

**Status: Confirmed**

`RECENT SIGNALS` is a compact, newest-first list answering "what just changed?" It is
the only event-list capability in the first attention-first implementation. Full Signal
History, filters, search, a timeline inspector, and long-term retention are Deferred.

Recent Signals are process-lifetime only. The initial scan establishes a baseline and
does not emit Signals for conditions that already exist. Only changes observed after that
baseline are recorded; quitting Mudi clears the list, and the next launch starts with
a new empty list. Current conditions discovered at launch still appear immediately in
`NOW`.

The first version records these Recent Signals:

- Busy changed to Idle;
- Busy or Idle changed to Exited;
- Exited changed to Busy or Idle when a Session is opened or resumed;
- Current Context entered or cleared Warning or Critical;
- Compaction observed;
- Shared Worktree Risk detected or cleared.

It does not record Idle-to-Busy changes, Busy heartbeats, ordinary Current Context
percentage changes, Usage or Cost changes, source-file timestamp changes, unchanged polling
observations, or `Possible Stall`.

When one Compaction observation also clears a Context Alert, Mudi emits one compound
Recent Signal containing the Compaction evidence, Context before/after values, and cleared
Alert rather than occupying multiple rows.

`RECENT SIGNALS` retains at most the five newest Signals for the current Mudi process
and renders them newest first. A compact viewport may render fewer than five when required
to preserve `NOW`; the region does not scroll, expand, or link to additional history. A
new Signal is inserted at the top and evicts the oldest retained Signal when the limit is
already reached.

`Idle` is a current Session state meaning that the provider process is open and awaiting
input, and it carries a neutral Attention Condition. A transition from Busy to Idle is a
Recent Signal; repeated observations of an unchanged Idle Session are not new signals.

### 8.19 Selected Session Details

**Status: Confirmed**

The left inventory keeps a compact `DETAILS` section below `PROJECT` for the currently
selected Open Session. It complements the Tree row instead of becoming a second Session
Detail view.

- Size the section from its configured content at roughly one third of the left inventory
  height. Give the remaining height to `PROJECT`, but do not remove configured detail rows
  merely to preserve the approximate ratio.
- Put the first field immediately below the titled top rule; do not reserve a blank row
  below `DETAILS`.
- Render field labels and values at the terminal's normal text size and weight. The title
  may remain emphasized; field labels are not bold.
- Keep Provider, Worktree or Directory, model, eligible Current Run Usage, and active Task
  facts where available. Long horizontal values may use predictable truncation because
  their complete values remain available in Session Detail.
- When the selected Session's Tree row displays `!`, add an `Attention` row that explains
  the highest-priority Warning or Critical condition that produced the marker. Use the
  confirmed Attention ordering rather than a separate display-only priority.
- Format the primary cause compactly, for example `Critical Context · 90%` or
  `Shared Worktree Risk · 2 Sessions`. When more Warning or Critical causes exist, append
  `+N`, where `N` counts the remaining causes. Session Detail lists every current Attention
  Condition with its evidence.
- Render Current Context with the compact shared progress component from `UI-008`.

The `!` marker and its explanation are informational. Neither authorizes a Control
Operation or changes the explicit authorization boundary.

### 8.20 Main-screen acceptance criteria

- `NOW` does not consume the entire terminal when sufficient width and height are
  available for multiple regions.
- Every Open Session appears exactly once in `NOW`.
- Exited Sessions do not appear in `NOW`.
- A transition to Exited may appear in `RECENT SIGNALS` but is not a current Attention
  Condition.
- Sessions from different Providers appear together when they belong to the same Project.
- Linked Worktrees appear under one Project and remain distinguishable.
- Independent clones remain separate Projects even when their remote URL is the same.
- Shared Worktree Risk evidence identifies the Worktree and every contributing Open
  Session without reading source-code content.
- Shared Worktree Risk clears when fewer than two distinct Open Sessions resolve to the
  Worktree.
- Attention ordering and Context colors make the highest-priority current Project
  scannable.
- The main screen contains no Processed Usage or Estimated Cost values.
- `RECENT SIGNALS` is visually and semantically distinct from current Session state.
- `RECENT SIGNALS` retains at most five entries, orders them newest first, and provides no
  scrolling, expansion, or `View more` action.
- The compact status strip shortens status evidence and reduces visible Tasks before
  reducing the space required to understand `NOW`.
- `Tab` and `Shift+Tab` move focus among `TASKS`, `NOW`, and `RECENT SIGNALS`; the focused
  region is visibly identified and each list retains its own selected row.
- `Enter` on a Task row opens its Task detail and state-appropriate actions.
- `Enter` on a Session-scoped Recent Signal opens that Session's Detail modal.
- `Enter` on a Shared Worktree Risk Signal switches to page `1` and focuses the matching
  Worktree in `NOW`. If the risk has cleared and the Worktree no longer contains an Open
  Session, Mudi stays on the Signal and briefly reports
  `Worktree is no longer open` in the status line.
- Duplicate polling observations do not produce duplicate Recent Signals.
- A threshold crossing produces one Recent Signal per crossing and may produce another
  when the condition clears.
- A Compaction and the Context Alert it clears appear as one compound Recent Signal.
- Recent Signals never contain prompt, response, tool, or source-code content.
- Restarting Mudi starts `RECENT SIGNALS` empty without affecting current conditions
  shown in `NOW`.
- The reserved upper-left region remains unlabelled, empty, and non-focusable.

## 9. Provider session pages

### 9.1 Purpose

Pages `2` and `3` preserve the complete v1 inventory and detail workflow for Claude Code
and Codex CLI respectively. They are the factual views behind attention classifications.

### 9.2 Navigation behavior

**Status: Confirmed**

- `2` always opens the Claude Code session list.
- `3` always opens the Codex CLI session list.
- Sessions are grouped by Project identity within each Provider page.
- Each session is rendered as exactly one selectable row inside its project group.
- Project is the organization unit; Session remains the inspection and detail unit.
- Order Project groups by the most recent `last_activity` of any contained Session,
  newest first.
- Preserve state, project, model, context, estimated cost, update time, and compaction
  columns.
- Allow entry from `NOW` and Session-scoped Recent Signals with the relevant Session
  selected. Worktree-scoped Signals locate their Worktree on page `1` instead of choosing
  an arbitrary contributing Session.
- Do not render a Time Filter control.
- Inside each Project, render Sessions with last activity inside the rolling seven-day
  window directly, ordered by `last_activity` newest first.
- Under each Project, render Sessions before that cutoff in that Project's own
  `OLDER SESSIONS` disclosure, collapsed by default, showing its Session count, and ordered
  by `last_activity` newest first when expanded.
- Do not create global `LAST 7 DAYS` or `OLDER SESSIONS` sections outside Project groups.
- Apply the cutoff strictly by last activity. An Open Session older than seven days remains
  visible in `NOW` and belongs to `OLDER SESSIONS` on its Provider page.
- Keep a Project visible when it contains no Recent Sessions; its collapsed
  `OLDER SESSIONS` disclosure is the Project's only child row.
- Every row displays the Session's complete lifetime Usage; the seven-day boundary never
  splits or limits Usage.

### 9.3 Row-organization acceptance criteria

- Every visible session appears once and opens its own Session detail.
- Sessions belonging to the same Project are visually grouped.
- Distinct Projects with the same display basename remain separate and receive
  disambiguated labels.
- A project group may contain multiple sessions without merging their state, context,
  model, usage, or provider-native identity.
- Direct Recent rows and per-Project `OLDER SESSIONS` disclosures partition the Provider's
  Sessions without omission or duplication.
- Each Project's `OLDER SESSIONS` is collapsed on first entry to a Provider page and can
  be expanded independently without navigating away.
- Expanding or collapsing `OLDER SESSIONS` does not change Session data or Usage scope.

## 10. Session detail

**Status: Confirmed**

Session detail remains the evidence surface and should contain:

- provider, native session ID, project, cwd/worktree, model, and current state;
- Current Context occupancy and threshold state;
- Current Run Usage and Transcript Lifetime Usage as separate, explicitly labelled scopes;
- per-model usage within the selected scope;
- compaction timeline;
- current Attention Conditions and the evidence behind each.

Session Detail does not repeat `RECENT SIGNALS`, add a Recent Transitions list, or expose
a Signal timeline. Compactions remain because they are provider-recorded transcript
metadata that can be rebuilt independently of Mudi's process-lifetime Signals.

### UI-013 - Session Detail modal

**Status: Confirmed**

`Inspect` opens Session Detail as a centered modal over the originating page. The modal is
approximately 80% of the terminal width and height, with a maximum width of about 100
columns. The originating page remains visible behind a visually dimmed backdrop so the
user retains location context.

- `Enter` from a Session row in `NOW`, a Session-scoped Recent Signal, or either Provider
  page opens the same Session Detail identified by Provider plus native Session ID.
- `Esc` returns to the originating page with its selection, scroll position, and expanded
  `OLDER SESSIONS` disclosures restored.
- `1`, `2`, or `3` leaves Detail and navigates directly to that top-level page.
- `Up/Down` or `j/k` selects value-bearing fields and automatically scrolls to keep the
  selected field visible. This selection stays within the current modal and never switches
  Session. Section headings, separators, progress visuals, and narrative text are not
  selectable.
- `c` copies only the selected field's complete underlying value. It does not include the
  field label or a trailing newline and does not open another modal or menu.
- `Enter` has no action inside Session Detail.
- `h` starts the separately specified explicit Handoff authorization flow for the Session;
  opening Detail or selecting a field never authorizes that operation.
- The content identifies the Provider, Project, and Session and renders all current
  Attention Conditions with their evidence.

### UI-009 - Current-run and transcript-lifetime usage

**Status: Confirmed**

Session detail must distinguish the usage produced by the current provider invocation
from usage accumulated across the complete native Session transcript:

- `Current Run Usage` is the primary comparison surface for an open Claude Code Session
  and should match Claude Code `/usage` within the provider's display rounding.
- `Current Run Usage` exists only for an Open Session. An Exited Session displays no
  `Current Run Usage` or `Last Run Usage` because it has no current provider invocation.
- `Transcript Lifetime Usage` includes inherited history from earlier Runs of the same
  native Session. It remains available for both Open and Exited Sessions because the
  complete JSONL transcript remains reconstructable across resume operations.
- The UI must never add the two scopes together or present either one as an unqualified
  `Total Usage` or `Session Usage`.
- Current Run Usage is bounded by the provider Run boundary. For an open Claude Code
  Session, the authoritative boundary is the live state record's process `startedAt`.
- Current Run Usage includes every model used after that boundary. Filtering by the
  latest model is not an acceptable substitute for identifying the Run boundary.
- Input, output, cache read, and cache creation remain separate in both scopes.
- Estimated API cost must identify which usage scope it estimates and must not imply that
  it equals the provider's subscription charge.
- If the Current Run boundary cannot be established, display Current Run Usage as `N/A`
  rather than guessing from model names, file modification time, or the latest records.
- Do not persist a completed Run boundary solely to retain its Usage after exit. Do not
  infer a Last Run boundary from timestamp gaps, file offsets, or task/turn events.
- Provider model names continue to come from source metadata. A model alias shown by a
  provider command must not be silently substituted without a confirmed mapping.

### UI-007 - Source metadata

**Status: Confirmed**

Session detail should expose the exact local source metadata needed to locate a session:

- working directory (`cwd` / process `pwd`) when known;
- provider source-log path;
- source format (`JSONL` for current Claude Code and Codex CLI adapters);
- native provider session ID.

The current implementation already indexes the source-log path and carries `cwd`, but the
normalized UI `Session` does not retain the source path. Session Detail displays complete
paths even when compact lists truncate them. Mudi must not read or display raw JSONL
contents, prompt/response content, tool content, or source-code content through this view.

Source-log path, Worktree path, `cwd`, and native Session ID are value-bearing fields in
Session Detail when available. `Up/Down` or `j/k` includes them in the normal field
selection order, and `c` writes the selected field's complete untruncated value directly
to the macOS clipboard. Unavailable values are not selectable. The copied value contains
no field label or trailing newline; success or failure appears briefly in the status line.
The Copy command never reads or copies raw JSONL or source-code content.

Session Detail has no Copy menu, Copy-menu state, nested Copy overlay, or Enter-to-copy
interaction.

## 11. Visual language

### UI-005 - Theme-driven semantic color

**Status: Confirmed**

Components consume semantic Theme roles rather than hard-coded colors. Every built-in
Theme defines at least:

- structural roles: background, foreground, muted text, border, accent, selected
  background, and selected foreground;
- lifecycle and attention roles: Critical, Warning, Idle, healthy Busy, and
  Exited/unknown;
- Context roles: normal, Warning, and Critical.

Mudi ships with a built-in `mudi` Theme as the default and restores an
existing Theme choice from the local Profile. There is no runtime Theme-cycling shortcut;
interactive Theme selection belongs to a later Settings feature.

The first implementation includes six built-in Themes:

1. `mudi` - the default dark Theme;
2. `catppuccin` - Catppuccin Mocha;
3. `dracula`;
4. `gruvbox` - Gruvbox Dark;
5. `light` - for bright terminal environments;
6. `high-contrast` - maximum foreground/background separation.

User-authored color palettes and per-role RGB configuration are outside the first
implementation. Additional built-in Themes may be added later without changing Theme
semantics or Guide behavior.

`--theme <name>` overrides the saved Theme for the current process without changing the
saved preference. An unknown or unavailable Theme name falls back to `mudi` instead
of preventing startup.

Theme changes affect presentation only. They never change thresholds, severity, ordering,
Session state, or Provider identity. Provider identity remains a text label and does not
own the alert palette. Required state and severity labels remain visible so color is never
the only evidence.

### UI-006 - Dense operational layout

**Status: Confirmed**

- Prefer rows, tables, lanes, and inspectors over a card-heavy dashboard.
- Use indentation, compact headings, and aligned columns for the
  `Project -> Worktree -> Session` hierarchy. Borders frame major regions such as `NOW`,
  `RECENT SIGNALS`, `TASKS`, and the Session Detail modal, not each Project, Worktree, or
  Session. One horizontal rule separates `PROJECT` from inline `DETAILS`; one vertical
  divider separates the lower inventory from the Session Workspace.
- Keep headings compact and optimized for scanning.
- Do not hide required evidence behind color alone.
- Truncate long project/model/session values predictably and expose full values in detail.
- Narrow terminals may collapse secondary fields, but must preserve attention kind,
  project, severity, and primary action.

### UI-012 - Responsive terminal bounds

**Status: Confirmed**

Use `120x30` as the full-layout target and `80x24` as the hard minimum. At both targets,
the compact upper status strip keeps its left third empty. The right two-thirds are split
equally between `RECENT SIGNALS` on the left and `TASKS` on the right. The lower workspace
owns the remaining height and places `PROJECT` plus inline `DETAILS` to the left of the
Session Workspace. Narrow layouts degrade fields in this order: shorten Worktree path,
hide model, hide last activity, reduce the applicable Context presentation to its text
percentage, then shorten status-row evidence.

Attention kind and severity, Project and Worktree identity, Session State, Provider,
Current Context percentage, selection, and access to Inspect through the persistent
contextual Footer remain available in the compact layout. The Session row does not repeat
the Footer's `Inspect` command text.
Below the hard minimum, render a terminal-too-small state with the required and current
dimensions plus Quit; do not attempt the normal layout.

The bounds are locked by deterministic production Ratatui renders under
`tests/fixtures/production-renders/`, asserted through the public production UI in
`tests/production_renders.rs`. Both sizes preserve the required hierarchy, state, Context
percentage, selection, and primary action without overlap. Session Detail keeps identity,
Usage, and complete wrapped source metadata reachable through selected-field navigation
and automatic scrolling.

### UI-008 - Context progress bar

**Status: Confirmed**

One shared component renders Current Context occupancy from
`ContextState.tokens / ContextState.window`; it must never represent cumulative processed
Usage. It has two progress-bar forms:

- inline `DETAILS` uses a compact bar plus percentage;
- the Session Detail modal uses a wider bar plus token count, context window, and
  percentage.

The `PROJECT` Tree keeps its existing aligned text percentage so the hierarchy remains
dense. It uses the same threshold semantics and truthful percentage as the shared
component.

Theme roles for Open Sessions follow the Context thresholds:

- below 70% - Context normal;
- 70% or above but below 85% - Context Warning;
- 85% and above - Context Critical.

The default `mudi` Theme maps these roles to green, amber/yellow, and red. Other
Themes may use different accessible colors without changing the thresholds or labels.

The percentage remains visible so color is not the only signal. Values above 100% render a
full bar while retaining the truthful percentage. When the context window is unknown, the
UI renders a neutral empty bar with `N/A` and does not invent a ratio; if the token count is
known, the Session Detail modal still displays it. An Exited session retains its last
known percentage but uses neutral/gray presentation so historical context does not look
like a current alert.

When inline `DETAILS` or the modal is too narrow for its stable-width bar, that Context
presentation may degrade to the text percentage or `N/A` while preserving the same
threshold semantics.

### UI-014 - Explicit region focus

**Status: Confirmed**

`PROJECT`, `RECENT SIGNALS`, and `TASKS` use one shared focus presentation:

- the focused region title uses the selected background, highlighted foreground, and bold
  text, and its top rule or border uses the accent color;
- an unfocused region title uses muted text with no title background;
- each region retains its own selected row, whose shared dark selection background does
  not change merely because the region gains or loses focus.

The same rules apply to all three regions so `Tab` and `Shift+Tab` always produce a clear,
consistent focus change.

## 12. Keyboard model

**Status: Confirmed**

Global commands:

- `1/2/3` - All, Claude Code, Codex CLI;
- `Tab` / `Shift+Tab` - move forward/backward among focusable regions on the current page;
  they never change the `1/2/3` page. On the All page they traverse `TASKS`, `NOW`, and
  `RECENT SIGNALS`; a page with only one focusable region leaves them as no-ops;
- `?` - open the contextual Guide;
- `r` - refresh;
- `q` - quit;
- `Esc` - close Session Detail and restore its origin state, or close the current
  inspector/Guide layer.

List and timeline commands:

- `Up/Down` or `j/k` - move selection;
- `Enter` - activate the selected item's single primary action: inspect a Session or
  Task, or locate the Worktree belonging to a Shared Worktree Risk Signal.

Session Detail commands:

- `Up/Down` or `j/k` - select the previous/next value-bearing field and automatically
  scroll it into view;
- `c` - copy the selected field's complete value directly, without its label or a trailing
  newline;
- `h` - start the explicit Handoff authorization flow;
- `Enter` - no action;
- `1/2/3` - leave Detail for a top-level page;
- `Esc` - return to the originating page and restore its view state.

## 13. Privacy and action boundaries

The existing privacy contract remains non-negotiable:

- metadata-only Session and Task stores;
- no source Prompt, raw Transcript response, tool, or source-code content in either store;
- Handoff Artifact content exists only in its explicitly authorized owner-only result file;
- source logs remain the source of truth for provider session metadata;
- local processing only;
- provider sessions remain distinct even when grouped by project.

Passive observation remains the default. Attention Conditions, Alerts, thresholds, and
elapsed time never authorize action. Mudi may perform only a declared, bounded
Control Operation after the user selects its target and explicitly authorizes that one
workflow attempt. Handoff generation remains read-only toward the source Project and
Worktree even when its authorized workflow safely ends a Current Run or launches a later
Successor Session.

## 14. Technical capability gaps

These are product dependencies, not an implementation plan:

1. **Project and Worktree identity** - current `cwd` metadata must be resolved to a
   canonical Git common directory, Worktree root, and branch without reading source
   contents. Non-Git directories use canonical `cwd` as standalone Project identity.
2. **State transitions** - current snapshots show latest state; Recent Signals require
   de-duplicated transition events or equivalent temporal metadata.
3. **Detail navigation** - `NOW`, `RECENT SIGNALS`, and Provider pages need a stable
   Provider-plus-native-Session-ID route into the same centered Detail modal, selected-field
   navigation with automatic scrolling, and restoration of the originating view state on
   return.
4. **Clipboard integration** - Session Detail needs an explicit user-triggered macOS
   clipboard write for selected source metadata, including status feedback on failure.
5. **Run boundaries** - Current Run Usage needs a provider-native live invocation
   boundary. Claude exposes `startedAt` for its live process; Codex requires the start
   time of the live process holding the rollout file. Completed Run boundaries are not
   retained, and Exited Sessions expose only Transcript Lifetime Usage.
6. **Operation Task execution** - authorized work needs an authoritative Task Store,
   detached Workers, process reconciliation, private Artifact publication, completion
   notification, terminal launch adapters, and Provider-specific Successor linkage.
### 14.1 Deferred capability

`Possible Stall` detection is Deferred from the first attention-first implementation. It
must not create an Attention Condition, Recent Signal, future Signal History event, or
notification until provider-specific evidence can distinguish a likely stall from
legitimate Busy work with an acceptable false-positive rate.

`Acknowledge` and `Snooze` for Attention Conditions are Deferred to a later requirement.
The first implementation does not provide their commands, key bindings, persistence,
expiry, hiding, or ordering behavior; this is distinct from confirmed Task Acknowledgement
metadata for terminal Operation Tasks.

Full Signal History is Deferred to a later requirement. The first implementation provides
only the bounded `RECENT SIGNALS` list and does not include a History page or overlay,
global/Project/Session filters, search, a timeline inspector, or long-term event retention.
Recent Signals exist only for the current Mudi process and require no persistent event
store.

## 15. Product success criteria

The direction is successful when:

- a user with several Open Sessions can identify the next Session to inspect immediately;
- cross-provider worktree risk is visible without manually comparing provider tabs;
- users can understand an alert from its displayed evidence;
- returning users can reconstruct the important recent metadata changes retained in
  `RECENT SIGNALS`;
- the interface remains useful with zero Attention Conditions and does not manufacture
  urgency;
- authorized Handoff continues after the TUI exits, produces a valid recoverable Result,
  and never conflates execution, terminal launch, or Successor linkage outcomes;
- all observation remains metadata-minimizing and every Control Operation remains local,
  bounded, and explicitly authorized.

No remote analytics are required. Evaluation may use scripted scenarios, render fixtures,
and direct usability observation.

## 16. Open questions

| ID | Question | Status |
|---|---|---|
| Q-007 | Do the provisional `80x24` minimum and `120x30` full target work in the running TUI without overlap or unreadable truncation? | Confirmed |

## 17. Decision log

| Date | Decision | Status | Rationale |
|---|---|---|---|
| 2026-07-30 | Maintain this as a living local PRD alongside the v1 spec. | Confirmed | New requirement points will be reviewed and incorporated incrementally. |
| 2026-07-30 | Use exactly `1/2/3` as persistent top-level navigation for All, Claude Code, and Codex CLI. | Confirmed | Keeps the terminal navigation compact and easy to operate by keyboard. |
| 2026-07-30 | Omit text descriptions beside the persistent `1/2/3` navigation. | Confirmed | The mapping is introduced through Guide instead of repeated in the chrome. |
| 2026-07-30 | Introduce the numeric mapping and essential keys in a small startup Help overlay. | Superseded | Replaced on 2026-08-01 by the complete contextual Guide. |
| 2026-07-30 | Defer full Signal History and keep only `RECENT SIGNALS` in the first implementation. | Deferred | A bounded recent-change list covers the immediate return-to-terminal job without adding another view, filters, search, and long-term event retention. |
| 2026-07-30 | Do not use a separate Attention Inbox as the default-screen structure. | Rejected | The main screen uses one non-duplicated current Session inventory instead. |
| 2026-07-30 | Keep Mudi read-only and use `Inspect` as the primary action. | Superseded | ADR 0002 preserves passive observation as the default while permitting one explicitly authorized, bounded Control Operation. |
| 2026-07-30 | Use Theme-defined semantic colors, with severity and state rather than Provider owning alert meaning. | Confirmed | A default Theme gives Mudi a coherent identity while Profile loading, command-line overrides, and accessible alternatives can change presentation without changing behavior. |
| 2026-07-30 | Ship six built-in Themes and defer user-authored palettes. | Confirmed | Mudi, Catppuccin, Dracula, Gruvbox, Light, and High Contrast cover mainstream dark, bright-terminal, and accessibility needs without expanding first-version configuration and validation across a large palette catalog. |
| 2026-07-30 | Show complete working-directory and source-log metadata in Session Detail without exposing raw contents. | Confirmed | Local paths and native identity support investigation, while raw JSONL, prompt/response, tool, and source-code content remain outside the privacy boundary. |
| 2026-07-30 | Group provider-page sessions by Project while retaining one row per Session. | Confirmed | Project supplies context without losing session-specific state or detail navigation. |
| 2026-07-30 | Render Current Context as a percentage progress bar with threshold colors. | Confirmed | Makes Current Context occupancy scannable while retaining precise text and unknown-state handling. |
| 2026-07-30 | Divide the main terminal screen into regions, with `NOW` and `RECENT SIGNALS` as the only assigned regions. | Superseded | `RECENT SIGNALS` and `TASKS` now share the compact upper-right status row, while `PROJECTS` owns the full-width lower region. |
| 2026-07-30 | Leave the other two proposed homepage regions unassigned. | Superseded | The upper-right region now contains the two side-by-side status panels; only the upper-left region remains deliberately empty. |
| 2026-07-30 | Show Current Run Usage separately from Transcript Lifetime Usage in Session detail. | Confirmed | Current Run supports comparison with Claude Code `/usage`; transcript lifetime preserves inherited usage without presenting it as the same total. |
| 2026-07-30 | Use the provider Run boundary, not the latest model, to calculate Current Run Usage. | Confirmed | A single Run may use multiple models, while a resumed Session may retain earlier transcript usage. |
| 2026-07-30 | Show Current Run Usage only while a Session is Open; show only Transcript Lifetime Usage after exit. | Confirmed | JSONL timestamps preserve lifetime Usage but not a durable provider Run boundary. Avoiding inferred or persisted Last Run Usage keeps the index rebuildable and the lifecycle language precise. |
| 2026-07-30 | Expose same-ID JSONL copies as a Transcript Lineage in Session Detail. | Rejected | The observed duplicate files were manually copied and do not establish normal Provider behavior; Session Detail retains one Source Log. |
| 2026-07-30 | Name structural same-worktree exposure `Shared Worktree Risk`, not worktree conflict. | Confirmed | Metadata can prove that Open Sessions share one working copy, but cannot prove that they edit the same files or have produced a Git conflict. |
| 2026-07-30 | Treat Idle as a current state and Busy-to-Idle as a transition event. | Confirmed | The domain model already defines every Open Idle Session as awaiting input; only a state change belongs in Recent Signals. |
| 2026-07-30 | Define an Open Session by attachment to a validated live provider CLI process. | Confirmed | Busy and Idle describe a live process; source-log existence or recent activity alone does not prove that a Session remains open. |
| 2026-07-30 | Treat every Idle Open Session as neutral Attention, not an Alert. | Confirmed | Awaiting input may require user action, but it is neither a risk nor an abnormal condition and must not manufacture urgency. |
| 2026-07-30 | Defer `Possible Stall` detection from the first attention-first implementation. | Deferred | Busy state plus absent source-log growth cannot reliably distinguish a stall from legitimate work within the metadata-only boundary. |
| 2026-07-30 | Keep Exited Sessions out of current Attention and record only their transition. | Confirmed | Exited Sessions are not Open and therefore do not belong in `NOW`; the transition remains useful in Recent Signals. |
| 2026-07-30 | Give Shared Worktree Risk fixed Warning severity. | Confirmed | Shared-worktree evidence is important but does not prove an actual conflict; Session count strengthens the evidence display without justifying Critical severity. |
| 2026-07-30 | Identify a Git Project by canonical Git common directory and a Worktree by canonical top-level path. | Confirmed | Linked Worktrees must group under one local Project, while independent clones remain separate even when they share a remote URL. |
| 2026-07-30 | Rank current conditions as Critical Context, Shared Worktree Risk, Warning Context, Idle, then healthy Busy. | Confirmed | Critical outranks Warning; shared-worktree exposure has broader impact than an early Context warning; neutral Idle remains actionable without being presented as risk. |
| 2026-07-30 | Break equal Attention ranks with condition-specific evidence, recency, then stable identity. | Confirmed | The order stays deterministic and explainable without introducing an opaque weighted score; recently Idle work is more relevant than indefinitely pinning stale Idle Sessions. |
| 2026-07-30 | Integrate Project and Worktree organization into `NOW` instead of creating a separate Projects capability. | Confirmed | The main page already owns the cross-Provider Open Session inventory; a second view would duplicate the same Sessions and conflict with the fixed three-page navigation. |
| 2026-07-30 | Keep Processed Usage and Estimated Cost off the main screen. | Confirmed | `NOW` prioritizes current action and evidence; Current Context remains because it is operational state, while cumulative usage and cost stay in Provider pages and Session detail. |
| 2026-07-30 | Defer `Acknowledge` and `Snooze` to a later requirement. | Deferred | A factual current-state view does not yet need user-authored suppression state, expiry rules, persistence, or dedicated commands. |
| 2026-07-30 | Keep `RECENT SIGNALS` process-lifetime only. | Confirmed | Live state transitions cannot be reconstructed reliably after Mudi was closed; an initial snapshot is a baseline, not a set of newly observed events. |
| 2026-07-30 | Limit Recent Signals to actionable lifecycle, Context, Compaction, and Shared Worktree Risk changes. | Confirmed | Excluding Idle-to-Busy, heartbeats, ordinary metric changes, and unchanged polls keeps the bounded list focused; a Compaction and its resulting Alert clear are one compound Signal. |
| 2026-07-30 | Retain at most the five newest Recent Signals without scrolling or expansion. | Confirmed | Five changes provide short return-to-terminal context without recreating the Deferred Signal History capability; compact layouts yield space to `NOW` first. |
| 2026-07-30 | Use `80x24` as the hard minimum and `120x30` as the full-layout target. | Confirmed | Deterministic interactive Ratatui renders at both sizes preserved the required hierarchy, state, Context, selection, actions, and complete Detail metadata without overlap; `79x23` correctly falls back to the terminal-too-small state. |
| 2026-07-30 | Name the collapsed older provider inventory `OLDER SESSIONS`, not History. | Confirmed | It contains Session rows outside the primary recency scope, while Signal History means a chronological event capability and is Deferred. |
| 2026-07-30 | Organize Provider pages by recent-first Projects with a per-Project `OLDER SESSIONS` disclosure. | Confirmed | Project groups preserve work context while their latest child orders the groups; Recent and Older Sessions remain time-ordered inside each Project without creating a separate global archive. |
| 2026-07-30 | Auto-open Help once per local Profile and keep it available through `?`. | Superseded | The first-use persistence remains, but the small Help overlay is replaced by Guide. |
| 2026-08-01 | Replace small Help with one global, contextual, two-pane Guide. | Confirmed | Every category remains available while the initial selection follows the user's current page or region. |
| 2026-08-01 | Remove runtime `t` Theme cycling while preserving Theme loading and `--theme`. | Confirmed | Theme changes are low-frequency settings; removing the shortcut prevents accidental changes and frees footer space. |
| 2026-08-02 | Expose the selected item's primary action in the persistent contextual Footer without repeating it on every Session row. | Confirmed | The selection cursor identifies the target and `Enter Inspect` teaches the command without consuming row width with repeated action text. |
| 2026-07-30 | Render Session Detail as a full-page child view instead of a centered overlay. | Superseded | Replaced on 2026-08-09 by a centered modal that preserves origin context while keeping complete evidence reachable through selected-field navigation and scrolling. |
| 2026-07-30 | Add a `c` Copy menu for source paths and native Session ID in Session Detail. | Superseded | Replaced on 2026-08-09 by direct copy of the selected value-bearing field; the extra menu and Enter-to-copy interaction are removed. |
| 2026-07-30 | Reserve `1/2/3` for top-level pages and use `Tab`/`Shift+Tab` for focus within the current page. | Confirmed | Numeric page navigation is already direct; local focus traversal makes the independent `NOW` and `RECENT SIGNALS` selections reachable without duplicating page navigation. |
| 2026-07-30 | Show condition-specific evidence directly in `NOW`, including a Context progress bar plus percentage. | Confirmed | Users should be able to trust Context, Shared Worktree Risk, Idle, and healthy Busy classifications before opening Detail; compact layouts may degrade the bar to percentage text. |
| 2026-07-30 | Use compact hierarchical rows and reserve borders for major regions. | Confirmed | Dense alignment keeps multiple concurrent Sessions comparable in one terminal viewport without turning Projects, Worktrees, and Sessions into nested cards. |
| 2026-07-30 | Keep Recent Signals out of Session Detail. | Confirmed | The main page owns the bounded process-lifetime change list; Detail owns current evidence and provider-reconstructable Usage, Compaction, and source metadata without introducing a second Signal History surface. |
| 2026-07-30 | Route Recent Signal actions by scope: Session Signals inspect Detail and Shared Worktree Risk Signals locate the Worktree in `NOW`. | Confirmed | A worktree-level condition may involve multiple Sessions, so selecting one arbitrarily would misrepresent the Signal; cleared, no-longer-open Worktrees report that navigation is unavailable. |
| 2026-08-02 | Use seven explicit Operation Task states and keep Task Acknowledgement separate from execution state. | Confirmed | Execution outcomes must remain stable and recoverable; viewing, copying, launching, or dismissing a result is UI interaction metadata rather than a new execution outcome. |
| 2026-08-02 | Order one unified Task timeline by its most recent execution-state transition. | Confirmed | Meaningful transitions, especially new terminal outcomes, become immediately visible without heartbeats causing row movement or state grouping fragmenting time order. |
| 2026-08-02 | Record Task Acknowledgement only after deliberate user interaction. | Confirmed | Notification delivery, panel rendering, and startup loading cannot prove that the user saw a terminal outcome; unread presentation must not alter execution truth. |
| 2026-08-02 | Keep every retained Task in the scrollable `TASKS` panel after acknowledgement. | Confirmed | `Close for Later` must leave the result discoverable without creating a separate history surface; only retention or explicit deletion removes a Task. |
| 2026-08-04 | Rebalance the home page into side-by-side Signals and Tasks in a compact upper-right status area above a full-width `PROJECTS` region. | Superseded | The upper status layout remains, but the lower area was replaced by the split inventory and Session Workspace confirmed on 2026-08-09. |
| 2026-08-02 | Apply fixed deadlines of 30 seconds in `Queued`, 2 hours in `Waiting`, and 30 minutes in `Running`. | Confirmed | Startup failures should surface quickly, stale control authorization must expire, and runaway Provider execution must begin graceful termination without force killing or automatic retry. |
| 2026-08-02 | Treat Task cancellation as an intent whose outcome is determined by reconciliation evidence. | Confirmed | A valid result, verified cancellation, and uncertain outcome must remain distinguishable; cancellation must not force-kill processes, close the user's shell, or conceal uncertainty. |
| 2026-08-02 | Implement manual retry as a newly authorized Task linked to an immutable terminal predecessor. | Confirmed | Reopening or overwriting the original would erase evidence, reuse stale assumptions, and risk duplicate execution while an Interrupted Provider might still be live. |
| 2026-08-02 | Make Handoff eligibility independent of Current Context thresholds and Session lifecycle. | Confirmed | Alerts may recommend but never authorize or gate an operation; Busy, Idle, and Exited Sessions can all hand off when their concrete source, working-directory, concurrency, and authorization preconditions hold. |
| 2026-08-02 | Support same-Provider and cross-Provider successors, defaulting to the source Provider. | Confirmed | The fixed Artifact is Provider-neutral, so an explicit target choice adds cross-Provider value without pretending to migrate native Session identity or Transcript history. |
| 2026-08-02 | Represent successor startup as a platform-neutral Launch Plan consumed by terminal adapters. | Confirmed | No universal terminal-launch protocol exists; structured executable arguments keep Handoff portable and safely testable while v1 ships only a macOS adapter and Copy fallback. |
| 2026-08-02 | Link every Successor Session through launched-process evidence rather than a preassigned or guessed ID. | Confirmed | One external contract keeps Provider differences inside adapters, records only Sessions that actually start, and avoids false association when several Sessions share a working directory. |
| 2026-08-02 | Retain Tasks and Artifacts until the user explicitly deletes a terminal Task. | Confirmed | Automatic expiry would violate `Close for Later`; explicit irreversible deletion provides privacy control without touching source or Successor Sessions. |
| 2026-08-02 | Keep Handoff execution, terminal launch, and Successor linkage as independent outcomes. | Confirmed | Worker interruption must not fabricate an Artifact, and launch or linkage failure after `Ready` must not invalidate an already verified Handoff Result. |
| 2026-08-09 | Keep compact selected-session `DETAILS` below `PROJECT`, size it from content, and explain the Tree's `!` marker with the highest-priority Attention cause plus an additional-cause count. | Confirmed | The inline facts remain compact while making warning and critical markers understandable without opening the complete Detail surface. |
| 2026-08-09 | Split the lower workspace with one vertical divider, placing `PROJECT` and inline `DETAILS` in the left inventory and the Session Workspace on the right. | Confirmed | One structural divider and one internal Details rule preserve functional grouping without presenting the workspace as two unrelated framed cards. |
| 2026-08-09 | Reuse one Current Context progress component in compact inline `DETAILS` and the wider Session Detail modal while keeping the Project Tree's aligned percentage. | Confirmed | The two evidence surfaces share thresholds and unknown/overflow behavior without widening the dense hierarchy. |
| 2026-08-09 | Open Session Detail as a centered, dimmed-backdrop modal whose value-bearing fields are selected with vertical keys and copied directly with `c`. | Confirmed | The modal preserves page context; direct value copy removes an unnecessary nested menu while retaining complete untruncated metadata access. |
| 2026-08-09 | Apply one explicit focused-region style to `PROJECT`, `RECENT SIGNALS`, and `TASKS`. | Confirmed | A highlighted title and accent top rule make `Tab` movement visible while the shared row-selection background remains stable. |

## 18. Adding the next requirement

Use the following shape when incorporating a new requirement point:

```text
ID:
Status: Proposed | Confirmed | Deferred | Rejected
Requirement:
User value:
Acceptance criteria:
Affected screens/data:
Open questions:
```

Do not append ideas as an unstructured backlog. Integrate each confirmed point into the
relevant workflow above and record the resulting decision in the log.
