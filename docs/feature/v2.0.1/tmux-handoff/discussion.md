# tmux Handoff - Requirements Discussion

**Status:** analyzing
**Release:** v2.0.1
**Feature:** tmux-handoff

## Problem

Handoff Operations currently run in a detached Mudi worker which starts the source
Provider through its non-interactive CLI. This execution path is separate from the private
tmux environment used by Managed Current Runs. The requested change is to make Handoff run
through tmux, but the intended relationship between that tmux execution and the source
Session's existing Managed Current Run is not yet decided.

The current private-tmux launch path starts the Provider CLI as the pane's root command.
When that process exits, Mudi has no durable pane host in which it can start a Successor.
The agreed direction is therefore to preserve a Mudi-owned tmux execution environment
across sequential Agent invocations. Reuse is determined by conclusive Mudi control
ownership rather than by which terminal application displays the Run: an Open Managed
Current Run reuses its existing private tmux, while an Externally Started Current Run or
Exited source Session receives a new owned environment. Within a reused environment, the
original developer-facing pane is reserved for the Successor and a separate background
execution pane performs Handoff generation and validation.

## Users and Outcomes

- A developer can authorize Handoff and have its Provider work execute through a durable,
  Mudi-owned tmux environment.
- A developer handing off from a Managed Current Run can remain in a stable terminal
  surface while the source Agent exits and the Successor starts.
- A developer handing off from an Externally Started Current Run receives a new Mudi-owned
  tmux surface without Mudi claiming ownership of the external terminal.
- The operation remains inspectable and recoverable across TUI exit or restart without
  weakening source Session identity, explicit authorization, or result validation.
- Provider-specific limitations remain visible rather than being hidden behind a claim of
  identical Claude Code and Codex CLI behavior.

## Current Experience

- The TUI records an authorized Handoff Operation as an Operation Task and launches a
  detached Mudi worker.
- The worker resumes the source native Session through a Provider-specific non-interactive
  command, invokes the managed Handoff Skill, validates the candidate Handoff Artifact, and
  publishes a valid result into the private Mudi artifact directory.
- A successful Handoff Result is currently Handoff-specific rather than a generic
  Operation Result: the validated Markdown Artifact and typed Successor Launch Plan are
  stored through fields on the Operation Task.
- For an Open source Session, the authorized workflow may first wait for the exact Current
  Run to become Idle and request its graceful exit. Handoff generation then starts as a
  separate non-interactive Provider invocation.
- Managed Current Runs already execute inside Mudi-owned private tmux environments,
  but existing Handoff Operation Tasks do not.
- Managed private tmux currently launches the Provider CLI directly as the pane's root
  command; it does not keep a persistent shell or runner after that CLI exits.
- Successor launch currently opens a new Terminal.app command through a platform-specific
  Terminal Launcher rather than targeting the source Managed Current Run's tmux pane.
- The detached worker is a separate Mudi child process with standard streams redirected to
  null, so it can outlive a normal TUI exit. It is not currently hosted by tmux and does not
  create a separate process session with `setsid`, so terminal-level lifetime isolation is
  not established by the current launcher.

## Scope

### In Scope

- Define how tmux carries Handoff Provider execution.
- Define whether and how a Managed source reuses its existing tmux environment, window,
  and pane across source exit, Handoff generation, and Successor launch.
- For a Managed source, preserve the developer-facing pane for the Successor and run
  Handoff generation in a separate background execution pane in the same private tmux.
- Define the Mudi-owned tmux environment created for an externally started or already
  exited source Session.
- Define a durable pane-host lifecycle that can run sequential Agent processes without
  treating tmux as Session identity or completion evidence.
- Route Successor launch to the selected owned tmux target while preserving the existing
  Successor identity and process-evidence linkage requirements.
- Preserve detached execution across TUI lifetime.
- Define identity, authorization, observation, cancellation, reconciliation, and result
  evidence for the tmux-backed execution.
- Support both Claude Code and Codex CLI, with explicit Provider-specific behavior where
  necessary.
- Define the migration or compatibility behavior for existing non-terminal Handoff Tasks.

### Out of Scope

- Automatically authorizing Handoff from an alert, threshold, or Session state.
- Treating terminal text as proof that a Handoff Result is valid.
- Changing Successor Session identity or Successor linkage rules.
- Moving Handoff Artifacts into the source Project.
- Creating a generic Action Runtime or Flow Scheduler.
- Generalizing the tmux host into arbitrary Action dispatch, typed Action Results, or
  multi-step Flow execution in v2.0.1.
- Implementation, PRD publication, or delivery tickets during this discussion.

## Required Behaviour

- A Handoff Operation requires one explicit authorization for one selected source Session.
- An Open Managed Current Run reuses its existing Mudi-owned private tmux; an Externally
  Started Current Run or Exited source Session uses a newly created Mudi-owned private tmux.
- Terminal placement or appearance never establishes reuse eligibility; conclusive Mudi
  control ownership does.
- For a Managed source, the existing developer-facing pane survives the source Agent exit
  and is reserved for an explicitly launched Successor. A separate background execution
  pane resumes the source Session, generates and validates the Handoff Artifact, then exits
  and is cleaned up.
- Handoff readiness does not automatically start the Successor in the preserved pane; the
  existing explicit launch-or-leave-for-later choice remains in force.
- The Handoff execution must continue independently when the Mudi TUI exits.
- tmux remains transport and containment only; it does not establish Session identity,
  control authority, or successful completion.
- A successful Task still requires a valid Handoff Artifact published under the existing
  private ownership and validation rules.
- Restart reconciliation must use durable task, process, tmux, Provider, and declared-result
  evidence and must not guess when evidence is ambiguous.
- An uncertain outcome becomes Interrupted and is never automatically retried.

## Edge and Failure Cases

- tmux is unavailable or fails before Provider startup.
- The tmux server, session, window, or pane disappears while the Task is non-terminal.
- The background execution pane fails or disappears while the preserved developer-facing
  pane remains healthy.
- The developer-facing pane disappears after a valid Artifact is ready but before the
  Successor is launched.
- The Provider process exits without producing a valid Artifact.
- A valid Artifact exists even though worker, tmux, or Provider liveness is later lost.
- Mudi restarts while Handoff is waiting, running, cancelling, or publishing.
- The source Current Run changes identity or becomes Busy during an authorized transition.
- Existing detached-worker Tasks remain non-terminal during an application upgrade.
- Terminal attachment or user input interferes with the Handoff execution.

## Constraints and Dependencies

- `CONTEXT.md` requires Handoff generation to remain read-only with respect to the source
  Project and keeps Handoff completion separate from Successor launch.
- ADR 0003 requires detached lifetime, authoritative Task state, restart reconciliation,
  declared-result verification, and no automatic retry. It also explicitly keeps existing
  Operation Tasks on detached workers until a later decision changes that model.
- ADR 0009 makes private tmux the substrate for Managed Current Runs, but explicitly does
  not migrate existing Operation Tasks. Adopting tmux for Handoff therefore requires a new
  architectural decision that partially supersedes or extends ADR 0003 and ADR 0009.
- Handoff Artifact location remains governed by ADR 0005.
- Safe handling of an Open source Current Run remains governed by ADR 0004 unless this
  discussion explicitly changes that workflow.

## Decisions

- The feature is assigned to release `v2.0.1`.
- The canonical pre-PRD discussion is
  `docs/feature/v2.0.1/tmux-handoff/discussion.md`.
- This discussion changes only the Handoff execution substrate; it does not by itself
  authorize a generic tmux-based Action Runtime.
- The v2.0.1 discussion will establish the tmux execution environment and reuse behavior
  needed by Handoff while leaving a general Bounded Action Runtime to a later version.
- tmux reuse is based on control ownership: reuse the existing private tmux only for an
  Open Managed Current Run; create a new private tmux for an Externally Started Current Run
  or Exited source Session.
- For a Managed source, use a dual-pane model within the existing private tmux: preserve
  the original developer-facing pane for the Successor, and use a separate background
  execution pane for Handoff generation and validation.

## Open Questions

- Which process remains as the durable pane host after a Provider CLI exits, and what
  evidence lets Mudi safely submit the next Agent invocation to it?
- Does the background execution pane accept only a durable structured request and publish
  only a validated structured result, with terminal text retained as non-authoritative
  diagnostics rather than parsed as operation input or completion evidence?
- Is the Handoff pane user-visible and attachable while work is running, and if so, can the
  user type into it?
- What terminal and Provider evidence establishes Running, Ready, Failed, Interrupted, and
  Cancelled after restart?
- How should cancellation behave once the Provider may have received the Handoff request?
- What happens to non-terminal Tasks created by the current detached-worker implementation
  when upgrading?

## Evidence

- `CONTEXT.md` defines Handoff Operation, Operation Task, Managed Current Run, Handoff
  Artifact, and the safety and privacy invariants.
- `docs/adr/0003-detached-operation-task-workers.md` accepts detached non-interactive
  workers for existing Operation Tasks.
- `docs/adr/0009-private-tmux-for-managed-current-runs.md` accepts private tmux for Managed
  Current Runs while preserving the existing Operation Task model.
- `src/application/handoff/worker.rs` currently runs Provider-native non-interactive Handoff commands
  and validates candidate Artifacts.
- `src/application/handoff/mod.rs` launches the detached Mudi worker independently of the TUI.
- `src/adapters/providers/claude.rs` and `src/adapters/providers/codex.rs` define separate non-interactive
  Handoff resume commands and managed Skill invocations.

## Session Log

### 2026-08-12

- Captured the request to move Handoff execution to tmux.
- Confirmed release `v2.0.1` and feature slug `tmux-handoff`.
- Recorded the current detached-worker execution path and the architectural boundary between
  Operation Tasks and private-tmux Managed Current Runs.
- Left the tmux ownership and source-Session relationship unresolved because it determines
  the rest of the execution, safety, and recovery model.

### 2026-08-16

- Narrowed the v2.0.1 discussion to tmux execution environments and pane reuse; a generic
  Action Runtime remains deferred to the later version already anticipated by the feature
  roadmap.
- Recorded the candidate routing rule: reuse Mudi-owned tmux for a Managed source and
  create a new owned tmux environment for an external source.
- Identified that the current Provider-as-pane-root launch cannot preserve a pane after
  Agent exit, so a durable pane host and explicit reuse boundary must be decided.
- Confirmed that reuse is governed by conclusive Mudi control ownership: an Open Managed
  Current Run reuses its private tmux, while an Externally Started Current Run or Exited
  source Session receives a new Mudi-owned private tmux.
- Confirmed a dual-pane model for a Managed source: preserve the original user-facing pane
  for the Successor and perform Handoff generation in a separate background execution pane
  within the same private tmux.

### 2026-08-17

- Clarified the current implementation baseline: the Handoff Artifact is validated
  structured Markdown, while the complete Handoff Result is represented by Artifact and
  Successor Launch Plan fields on the Operation Task rather than a generic Result object.
- Recorded that the existing worker survives normal TUI exit as a separate child process
  with null standard streams, but is not isolated by tmux or a separate process session.
- Kept structured execution-pane input and output as an open decision because no product
  choice has yet been confirmed.
