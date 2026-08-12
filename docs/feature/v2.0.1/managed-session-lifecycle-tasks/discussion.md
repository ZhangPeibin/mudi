# Managed Session Lifecycle Tasks - Requirements Discussion

**Status:** analyzing
**Release:** v2.0.1
**Feature:** managed-session-lifecycle-tasks
**Last updated:** 2026-08-11
**Scope status:** candidate change to the v2.0.0 Managed Session Core workflow and domain
model; it does not change the accepted v2.0.1 boundary or authorize implementation by
itself.

## Problem

`New`, `Resume`, and Handoff are all explicitly authorized, asynchronous workflows with a
target, durable progress, terminal outcomes, and follow-up actions. The current product
models Handoff as an Operation Task but exposes `New` and `Resume` through a separate
Managed Launch Registration status region.

That separate `MANAGED START` region has no coherent long-term role. It can remain occupied
by an old terminal `Failed` Registration, while later successful launches disappear after
binding. The developer cannot acknowledge or remove the old record, and new work may not
become visible because the UI retains the earlier Registration selection.

The resulting split makes lifecycle work harder to scan: Handoff appears under `TASKS`,
Managed Session creation and resume appear elsewhere, and Recent Signals cannot by
themselves preserve actionable state.

## Users and Outcomes

A developer should be able to inspect all explicitly authorized SessionMesh work through
one Task model and one primary Task surface, regardless of whether the operation is New,
Resume, or Handoff.

Each Task should retain its durable outcome and legal follow-up actions. Recent Signals may
announce meaningful transitions, but must not become the only place where actionable or
recoverable state exists.

## Current Experience

- Handoff already creates an Operation Task and appears in the durable `TASKS` timeline.
- `New` and `Resume` currently create Managed Launch Registrations rather than Operation
  Tasks and use the separate `MANAGED START` status region.
- `Binding` offers `Cancel Start` and may offer terminal-access retry.
- `Needs Review` preserves a potentially live scene and offers only terminal access and
  metadata-only diagnostics.
- A successful Binding Receipt atomically establishes the Managed Session and Managed
  Current Run, then removes the Binding Registration from the Managed Start surface.
- A confirmed absent Provider process changes the Registration to `Failed` and cleans the
  empty private tmux environment.
- `Failed` exposes `No recovery action`. There is no acknowledgement, dismissal, archival,
  or deletion interaction for it.
- The store lists recoverable states before `Failed` and orders records by recency within
  those groups.
- The UI stores one selected Registration ID. It changes to the first listed item only when
  the selected ID disappears, so the arrival of a newer or more actionable Registration
  does not replace a still-present `Failed` selection.
- The local state inspected on 2026-08-11 contained one retained `Failed` Registration and
  eleven Binding Receipts. Later launches had succeeded, while the All-page region still
  showed the old failure.

## Scope

### In Scope

- Model every explicitly authorized `New` and `Resume` execution as an Operation Task, as
  Handoff already is.
- Present New, Resume, and Handoff through one Task surface with operation-specific status,
  result, and legal actions.
- Remove the dedicated `MANAGED START` region after its actionable lifecycle moves into the
  Task surface.
- Emit metadata-only Recent Signals for meaningful New and Resume transitions without
  making Signals authoritative for operation state.
- Define the relationship between an Operation Task and the launch/evidence record currently
  called Managed Launch Registration.
- Define how launch-specific states map to the shared Operation Task lifecycle without
  weakening Binding Receipt or process-evidence requirements.
- Define terminal retention, acknowledgement, deletion, and restart behavior for New and
  Resume Tasks.
- Define how Task ordering and navigation work when New, Resume, and Handoff coexist.

### Out of Scope

- Changing the evidence required to issue a Binding Receipt.
- Retrying Provider launch or binding automatically.
- Converting `Needs Review` into a Managed Session without conclusive evidence.
- Treating a pre-binding New as a Session or showing it in normal Session inventory.
- Deleting or modifying Provider-native source records.
- Redesigning the complete All-page layout beyond removing the dedicated Managed Start
  region and integrating its lifecycle into the Task surface.

## Required Behaviour

The following existing safety behavior remains the baseline while the unified Task model
is discussed:

- A `Binding` Registration may be cancelled only through explicit developer action.
- `Needs Review` remains available for inspection without automatic termination, retry, or
  promotion.
- Terminal-access retry must not restart the Provider, recreate tmux, or replay binding.
- A successful Binding Receipt remains the only path that establishes a Managed Session and
  Managed Current Run.
- Task acknowledgement, viewing, or deletion must not rewrite Provider evidence or claim a
  different launch outcome.
- Arrival of a new launch must not silently authorize any control operation.

The following product direction is confirmed:

- Each explicit `New` or `Resume` execution is represented by an Operation Task, in the same
  product model used by Handoff.
- New, Resume, and Handoff share the primary Task surface while retaining operation-specific
  actions and evidence requirements.
- Meaningful state changes may also produce Recent Signals. A Recent Signal records a
  transition; it never owns Task state or recovery actions.
- The separate `MANAGED START` region is removed once its behavior is covered by Tasks and
  Signals.

The exact Task lifecycle, result shape, Registration relationship, retention, and
acknowledgement behavior remain open.

## Edge and Failure Cases

- A New Task is created before a native Session ID exists, so its target cannot initially be
  a Session identity.
- A Resume Task targets an existing Session while a New Task targets Provider, Project, and
  Worktree.
- A Binding Receipt is issued while the corresponding New or Resume Task remains durable.
- Multiple `Binding` and `Needs Review` records coexist.
- New, Resume, and Handoff Tasks run concurrently against different targets.
- A Session has an existing non-terminal Handoff Task when Resume is requested, or vice
  versa.
- A New or Resume Task enters `Needs Review` with a potentially live Provider process.
- SessionMesh restarts with non-terminal launch Tasks and incomplete binding evidence.
- Task deletion is requested while launch or Provider evidence still requires retention.
- A Task changes state between rendering and the developer invoking an action.
- Compact terminals cannot display every Task kind, status, target, and action at once.

## Constraints and Dependencies

- `CONTEXT.md` currently defines Managed Launch Registration and Operation Task as distinct
  concepts. Treating New and Resume as Operation Tasks requires an explicit domain-model
  amendment rather than a UI-only change.
- ADR 0002 requires explicit authorization for state-changing Session control. Merely
  selecting or surfacing a Registration grants no authority.
- ADR 0009 requires conclusive launch, process, and Provider-source evidence for Managed
  Current Runs. UI prioritization cannot weaken that boundary.
- The current Managed Launch store and Task Store have different authority, lifecycle, and
  recovery rules. A unified product model must define whether those stores remain separate
  implementation details or one Task authority owns the complete workflow.
- The original v2.0.0 PRD and Issue #46 define safe launch outcomes but do not define
  terminal-record retention, acknowledgement, multiple-record priority, or success
  feedback.
- This direction materially changes the durable Managed Launch and Operation Task contract;
  it requires a formal PRD amendment and a `CONTEXT.md` update after the model is resolved.

## Decisions

- The discussion belongs to release `v2.0.1` under feature slug
  `managed-session-lifecycle-tasks`.
- `New` and `Resume` should create Operation Tasks just as Handoff does.
- New, Resume, and Handoff should appear in one Task surface.
- New and Resume lifecycle transitions may also produce Recent Signals, but Signals do not
  replace durable Task state or actions.
- The dedicated `MANAGED START` region should be removed after migration to the unified Task
  surface.
- Existing evidence and explicit-authorization rules for establishing Managed authority
  remain unchanged.
- No implementation, GitHub PRD, or delivery ticket is authorized by this discussion.

## Open Questions

- Does each New or Resume Operation Task own a one-to-one Managed Launch Registration as its
  evidence record, or does the Task replace Managed Launch Registration entirely?
- How do `Binding`, `Needs Review`, and receipt issuance map onto the shared Operation Task
  states `Queued`, `Waiting`, `Running`, `Ready`, `Failed`, `Interrupted`, and `Cancelled`?
- What is the terminal result of a successful New or Resume Task: a Binding Receipt, a link
  to the established Managed Session and Current Run, or both?
- Does the current one-non-terminal-Task-per-Session rule block Resume when a Handoff Task is
  active, and how is New constrained before a Session identity exists?
- Which New and Resume transitions create Recent Signals, and how are repeated refreshes
  deduplicated?
- Should terminal New and Resume Tasks use the existing Task acknowledgement and explicit
  deletion behavior without exceptions?
- What replaces the reserved top-left area after `MANAGED START` is removed?

## Evidence

- `feature/v2.0.0/README.md` establishes the Managed Session Core capability boundary.
- `docs/feature/v2.0.0/managed-session-core/discussion.md` defines `Binding`, `Failed`,
  `Needs Review`, and their safety behavior.
- GitHub Issue #42 is the formal v2.0.0 Managed Session Core PRD; user stories 16-22 cover
  visible Binding, confirmed failure, Needs Review, and isolated terminal retry.
- GitHub Issue #46 specifies safe handling for Binding failures and Needs Review.
- `feature/v2.0.0/diagrams/05-managed-start-ui.excalidraw` places a single pending
  Registration in the top status strip and illustrates `Needs Review`, but does not define
  `Failed`, multiple-record, empty-state, or retention behavior.
- `src/ui/mod.rs` retains selection by Registration identity across snapshot reorderings.
- `src/managed_sessions.rs` gives `Failed` no available recovery action and removes a
  successful Binding Registration after Receipt issuance.
- `tests/managed_binding.rs` currently locks stable identity selection and the
  `No recovery action` rendering for `Failed`.
- `CONTEXT.md` currently says a Managed Launch Registration is not an Operation Task, while
  Operation Task is the durable execution model used by Handoff.

## Session Log

### 2026-08-11

- Recorded the reported stale Managed Start region and traced it through local persisted
  state, read-model refresh, UI selection, and rendering behavior.
- Confirmed that later launches succeeded; the apparent lack of change results from
  successful Registrations leaving the surface while one terminal `Failed` Registration
  remains selected and non-actionable.
- Traced the existing requirements to the v2.0.0 Managed Session Core discussion, PRD #42,
  delivery Issue #46, and Managed Start UI diagram. None defines terminal-record retention,
  acknowledgement, selection priority, or successful-launch feedback.
- The developer assigned this discussion to `v2.0.1`; established
  `managed-start-status-lifecycle` as the initial feature slug.
- Confirmed that the root problem is the separate product model and placement, not only
  terminal `Failed` retention. New and Resume should create Operation Tasks like Handoff,
  share the Task surface, and emit Recent Signals for meaningful transitions.
- Renamed the discussion slug to `managed-session-lifecycle-tasks` and recorded removal of
  the dedicated `MANAGED START` region as the intended UI outcome.
