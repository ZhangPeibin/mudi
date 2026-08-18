# Multi-Agent Workflow - Requirements Discussion

**Status:** analyzing
**Release:** v2.0.1
**Feature:** multi-agent-workflow
**Last updated:** 2026-08-16
**Scope status:** candidate change to the accepted v2.0.1 Work Continuity boundary;
this discussion does not change that baseline or authorize implementation by itself.

## Problem

The current multi-Agent delivery workflow stops at decomposition. `to-tickets` can turn a
Coding Target into multiple GitHub Issues and identify work that can proceed in parallel,
but each Codex Session is then started with only an instruction such as
`Implement issue #N`. That instruction does not carry the surrounding Coding Target,
Acceptance Criteria, pinned base commit, dependency progress, owned business outcome,
expected change boundary, protected contracts, required tests, pause conditions, or result
reporting contract.

The developer therefore remains the scheduler and integrator. They open multiple tmux
windows, launch independent Codex Sessions, decide when each Issue is ready, interpret
free-form completion reports, and detect overlap, duplicated work, conflicting logic, and
unsafe dependency advancement. Parallel execution can reduce elapsed time, but the current
handoff from Issue decomposition to execution does not make it reliable or unattended.

The initial idea of a `dispatch-tickets` Skill has consequently expanded into a broader
orchestration workflow. A Skill alone cannot own process lifetime, durable Run state,
recovery, dependency advancement, or duplicate-launch prevention.

## Users and Outcomes

The primary user is a developer who has an approved Coding Target represented by multiple
GitHub Issues and wants Mudi to execute the ready work with multiple Codex Sessions without
manually coordinating every step.

The desired outcomes are:

- the developer explicitly starts one bounded automatic workflow rather than following a
  user-facing Plan by hand;
- each Codex Session receives enough structured context to own one declared business
  outcome without silently expanding its boundary;
- independent work runs concurrently, while dependent work starts only after its declared
  prerequisites have produced acceptable evidence;
- the controller detects overlap, prevents duplicate launches, and surfaces incompatible
  results instead of letting different implementations silently diverge;
- interruption, failure, missing work, and unresolved design choices return durable,
  inspectable states rather than relying on terminal prose or developer memory; and
- the developer can leave the workflow running and later understand what ran, what changed,
  what was verified, and what still requires a decision.

## Current Experience

- `to-tickets` creates multiple Issues and tells the developer which work may be parallel.
- The developer opens multiple tmux windows and manually starts separate Codex Sessions.
- These Sessions are peers rather than children of one persistent root Agent. The developer
  acts as the coordinator between them.
- The Sessions commonly share one working copy. This has been adequate for short, independent
  tasks that are intended to ship together, especially when their likely file ownership
  does not overlap.
- Separate Git worktrees are useful when tasks need independent branches, commits or PRs,
  have likely file overlap, or need stronger isolation, but they are not assumed to be the
  default for every parallel task.
- Completion is reported in free-form conversation. There is no durable machine-readable
  result contract that another Session or controller can safely consume.
- Mudi currently has no Workflow Run, multi-Issue dependency scheduler, or automatic Flow
  advancement. The accepted v2.0.1 scope explicitly defers those capabilities.

## Scope

### In Scope

- Define one explicitly started automatic workflow for a Coding Target represented by
  multiple GitHub Issues.
- Define a machine-consumable, versioned Workflow Definition or Execution Graph containing
  task boundaries, dependencies, expected outcomes, likely touchpoints, protected
  contracts, required checks, pause rules, and result requirements.
- Define a generation Skill that produces and validates that structured definition without
  launching tmux or Codex Sessions itself.
- Define an external controller that consumes the definition, derives ready work, starts
  Codex Sessions, collects structured Task Results, advances dependencies, and stops when
  the Run reaches a terminal or user-decision state.
- Separate immutable definition revisions, controller policy, mutable Run state, and
  append-only Task Results so execution can be inspected and recovered.
- Define shared-working-copy and separate-Git-worktree policies, including collision detection,
  integration barriers, Git ownership, and when stronger isolation is required.
- Define authorization, cancellation, retry, crash recovery, duplicate-launch prevention,
  and idempotency behavior.
- Define how discovered missing work produces a new graph revision without rewriting the
  history of the revision that was actually executed.

### Out of Scope

- Having the generation Skill launch tmux, Codex, or background workers directly.
- Requiring one Git worktree per Agent regardless of task overlap or Git lifecycle.
- Scraping JSON fragments from free-form Agent prose as an authoritative execution
  contract.
- Treating every Issue labeled as parallel as automatically safe to execute concurrently.
- Allowing a Session to expand product contracts or resolve an undecided design question
  merely because the controller is running automatically.
- Writing the formal PRD, creating delivery Issues, or implementing the workflow during
  this requirements discussion.
- Silently changing the accepted v2.0.1 feature baseline through this discussion document.

## Required Behaviour

The following direction is confirmed:

- The primary product experience is automatic execution after an explicit start, not a
  Plan that the developer must manually dispatch step by step.
- The generation Skill produces structured orchestration input. Process launching and Run
  advancement belong to an external controller.
- The controller loops over currently ready tasks, launches Codex Sessions, records their
  results, and reevaluates dependencies until no further work can safely advance.
- A task instruction must carry more than an Issue number. It must provide the Coding
  Target, Acceptance Criteria, current base and dependency state, owned outcome, expected
  modification boundary, protected contracts, required tests, pause conditions, and
  structured completion requirements.
- Execution waves are derived from dependencies and current Run state. They are not a
  second permanently encoded ordering that can drift from the graph.
- When execution discovers missing work or an invalid decomposition, the current revision
  remains immutable and the system proposes Revision 2. Revision 2 must not erase what
  Revision 1 authorized or executed.
- Shared-working-copy execution is supported for suitably independent tasks; multiple Git
  worktrees are a policy choice for stronger isolation rather than the definition of
  multi-Agent work.

The current candidate artifact model is:

- an immutable, versioned Workflow Definition or Execution Graph revision;
- Controller Policy covering concurrency, working-copy isolation, Git ownership, retries,
  integration barriers, and stop conditions;
- mutable Workflow Run state covering task state, Session identity, attempts, leases, and
  reconciliation data; and
- append-only structured Task Results containing declared outcomes and verification
  evidence.

This model remains provisional until the authorization envelope, result contract, and
recovery rules are resolved. Likely task outcomes include `succeeded`, `failed`, `blocked`,
`needs_input`, `needs_replan`, and `interrupted`, but their exact semantics are not yet
accepted.

## Edge and Failure Cases

- Two tasks judged independent both edit the same file or shared abstraction.
- Two Sessions implement equivalent helper code in different modules.
- Parallel Sessions make individually valid but mutually incompatible product decisions.
- A task completes against a stale base after another task changes a protected contract.
- A Session reports success but required checks were not run or their evidence is missing.
- A dependency succeeds in its own Git worktree but has not been integrated into the base used
  by a dependent task.
- A Session discovers that an existing Issue is incomplete, incorrectly bounded, or blocked
  by work absent from the current graph.
- The controller crashes after launching a Session but before recording that launch.
- A Session or tmux environment survives while controller state says the attempt is
  interrupted.
- The same task is launched twice after restart or lease expiry.
- Cancellation arrives while a Session may already have changed files or Git state.
- A known failure may be retryable, while an uncertain outcome may already have produced
  side effects and must not be submitted again automatically.
- One task requires user input while unrelated ready tasks could still proceed.
- The Issue, base commit, Acceptance Criteria, or dependency graph changes after a Run has
  started.
- A shared working copy contains pre-existing developer changes or changes from a Session whose
  result has not yet been accepted.

## Constraints and Dependencies

- ADR 0002 requires explicit, bounded authorization for state-changing control. Automatic
  execution must define exactly what one `Start Workflow` action authorizes and when that
  authority expires.
- ADR 0003 requires execution to outlive the TUI, reconcile durable state after restart,
  validate declared results, and avoid automatic retry after an uncertain outcome.
- ADR 0009 makes private tmux a Managed Current Run substrate, but tmux is neither control
  authority, Session identity, nor completion truth.
- `CONTEXT.md` currently defines Session, Project, Managed Current Run, Control Operation,
  and Operation Task, but it does not yet define Workflow Definition, Workflow Run, graph
  revision, or structured Task Result. Accepted terminology will require domain-model work.
- The accepted v2.0.1 Work Continuity baseline explicitly defers Action dispatch,
  structured Action Results, `needs_input`, Execution Plans, Workflow Runs, dependency
  scheduling, and automatic Flow advancement. Moving this workflow into v2.0.1 therefore
  requires an explicit scope-baseline decision, not only a PRD or implementation Issue.
- `v2.3.0 Approved Flow Scheduler` exists only as a planned direction in
  `feature/README.md`; there is no `feature/v2.3.0/README.md` accepted scope record.
- GitHub Issues remain authoritative for formal PRDs and delivery work. The generation
  contract must define how Issue content, revisions, dependencies, and likely touchpoints
  become pinned structured inputs rather than relying on mutable prose at execution time.

## Decisions

- The discussion is assigned to release `v2.0.1` with feature slug
  `multi-agent-workflow`.
- The desired experience is an automatically executed workflow, not a user-operated Plan.
- The proposed generation Skill does not launch tmux or Codex. It produces structured input
  for an external controller.
- The external controller owns scheduling, Session launch, result collection, dependency
  advancement, and Run termination.
- A revised graph is represented as a new immutable revision rather than an in-place rewrite.
- Shared-working-copy execution remains a supported default for sufficiently independent work;
  separate Git worktrees are selected when isolation or independent Git lifecycle justifies
  them.
- This is a candidate change that conflicts with the accepted v2.0.1 deferred boundary.
  Recording it here does not amend that boundary.
- No implementation, GitHub PRD, delivery Issue, or domain-model amendment is authorized by
  this discussion.

## Open Questions

- What exact work does one `Start Workflow` authorize: only the pinned dependency closure of
  the selected Issues, or also newly discovered Issues and contract changes?
- What durable domain name should replace the ambiguous user-facing word `Plan`:
  Workflow Definition, Execution Graph, or another term?
- How does `to-tickets` expose durable dependencies, Issue revisions, boundaries, protected
  contracts, and likely touchpoints to the generation step?
- What exact Task Result schema and verification evidence are required before dependants may
  advance?
- How are shared-working-copy collisions detected and contained, and what integration barrier
  separates one derived execution wave from the next?
- Which operations may retry automatically after a known failure, and how do cancellation,
  crash recovery, leases, duplicate-launch prevention, and idempotency interact?
- Which graph revisions may the controller adopt automatically, and which require a new
  explicit developer approval?
- Does one blocked or `needs_input` task stop the complete Workflow Run, only its dependants,
  or a policy-defined subset?

## Evidence

- The developer described the current workflow: `to-tickets` creates Issues and identifies
  parallel work; they then open multiple tmux windows and independently run
  `Implement issue #N` in Codex Sessions.
- The same workflow has commonly used one shared working copy without a persistent root Agent.
- `feature/v2.0.1/README.md` is the accepted Work Continuity scope and explicitly defers the
  orchestration capabilities discussed here.
- `feature/README.md` records v2.1.0 through v2.3.0 only as planned direction; only v2.0.0
  and v2.0.1 currently have version scope records.
- `CONTEXT.md` defines the current Session, Project, control, and task vocabulary and keeps
  observation separate from authorization.
- ADR 0002 establishes explicit bounded authorization for control.
- ADR 0003 establishes detached execution, durable reconciliation, declared-result
  validation, and no automatic retry after uncertain outcomes.
- ADR 0009 establishes private tmux as transport and containment for Managed Current Runs,
  not as authority, identity, or result evidence.
- `docs/feature/v2.0.1/managed-session-lifecycle-tasks/discussion.md` explores unifying New,
  Resume, and Handoff under the existing Operation Task model but does not define a
  multi-Issue Workflow Run.
- `docs/feature/v2.0.1/tmux-handoff/discussion.md` explores tmux-backed Handoff execution
  while explicitly keeping a generic Action Runtime and Flow Scheduler out of its scope.

## Session Log

### 2026-08-16

- Captured the current manual multi-Issue delivery workflow and the missing execution
  context in `Implement issue #N`.
- Distinguished multi-Agent execution from multiple-Git-worktree execution: multiple Codex
  Sessions may share one working copy, while separate Git worktrees provide stronger Git and file
  isolation when needed.
- Stress-tested the initial `dispatch-tickets` Skill idea and concluded that structured
  definition generation can belong to a Skill, while automatic process orchestration,
  durable state, recovery, and scheduling require an external controller.
- Recorded automatic execution as the desired product outcome and retained an internal
  versioned graph as controller input rather than a user-facing manual Plan.
- Recorded the candidate separation between immutable definition revisions, Controller
  Policy, mutable Run state, and append-only Task Results.
- Recorded Revision 2 as the response to newly discovered work rather than mutating the
  definition already authorized and executed.
- Confirmed `v2.0.1/multi-agent-workflow` as the discussion location and explicitly marked
  the idea as a candidate change to a baseline that currently defers these capabilities.
