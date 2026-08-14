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

## Users and Outcomes

- A developer can authorize Handoff and have its Provider work execute through a durable,
  Mudi-owned tmux environment.
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
- For an Open source Session, the authorized workflow may first wait for the exact Current
  Run to become Idle and request its graceful exit. Handoff generation then starts as a
  separate non-interactive Provider invocation.
- Managed Current Runs already execute inside Mudi-owned private tmux environments,
  but existing Handoff Operation Tasks do not.

## Scope

### In Scope

- Define how tmux carries Handoff Provider execution.
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
- Moving Handoff Artifacts into the source Project or Worktree.
- Creating a generic Action Runtime or Flow Scheduler.
- Implementation, PRD publication, or delivery tickets during this discussion.

## Required Behaviour

- A Handoff Operation requires one explicit authorization for one selected source Session.
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
- The Provider process exits without producing a valid Artifact.
- A valid Artifact exists even though worker, tmux, or Provider liveness is later lost.
- Mudi restarts while Handoff is waiting, running, cancelling, or publishing.
- The source Current Run changes identity or becomes Busy during an authorized transition.
- Existing detached-worker Tasks remain non-terminal during an application upgrade.
- Terminal attachment or user input interferes with the Handoff execution.

## Constraints and Dependencies

- `CONTEXT.md` requires Handoff generation to remain read-only with respect to the source
  Project and Worktree and keeps Handoff completion separate from Successor launch.
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

## Open Questions

- Does Handoff use the source Session's existing private tmux and interactive Managed
  Current Run, or start a dedicated tmux-hosted Provider invocation for the Operation Task?
- Is tmux-backed Handoff limited to Managed Sessions, or can an Exited or externally
  observed Session be resumed in a newly owned tmux environment for this one operation?
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
- `src/handoff/worker.rs` currently runs Provider-native non-interactive Handoff commands
  and validates candidate Artifacts.
- `src/handoff/mod.rs` launches the detached Mudi worker independently of the TUI.
- `src/providers/claude.rs` and `src/providers/codex.rs` define separate non-interactive
  Handoff resume commands and managed Skill invocations.

## Session Log

### 2026-08-12

- Captured the request to move Handoff execution to tmux.
- Confirmed release `v2.0.1` and feature slug `tmux-handoff`.
- Recorded the current detached-worker execution path and the architectural boundary between
  Operation Tasks and private-tmux Managed Current Runs.
- Left the tmux ownership and source-Session relationship unresolved because it determines
  the rest of the execution, safety, and recovery model.
