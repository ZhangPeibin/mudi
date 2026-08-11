# v2.0.1 - Work Continuity

| Field | Value |
|---|---|
| Status | Accepted scope baseline |
| Accepted | 2026-08-10 |
| Role | First repeat-use layer after Managed Session Core |

`v2.0.1` lets a developer leave and reliably re-enter a body of coding work without
reconstructing it from terminal history, Git state, and memory. It builds on `v2.0.0`
Managed Session Core; it is not an Action Runtime or Flow Scheduler.

## Goal

Give a developer an explicitly created, evidence-backed checkpoint for a selected Session
and a clear continuation choice: precisely resume the same provider-native Session, or
explicitly Handoff to a distinct Successor Session.

The primary question answered is: **what can the next Session safely do now?**

## Included

- Create a local checkpoint only after the developer explicitly requests it for a selected
  Session with a resolved Project and, when applicable, Worktree.
- Preserve the checkpoint's source Session identity, Project, Worktree, Provider, observed
  Current Run state, repository revision, and allowed working-tree evidence needed to orient
  the developer later.
- Let the developer add bounded, local objective, constraint, and next-action metadata.
  This is separate from the Session Description, which continues to explain why the Session
  exists rather than its transient work state.
- Display observed evidence separately from user-authored or Agent-proposed statements.
  Missing evidence must remain missing; SessionMesh never infers a passed check or completed
  task from terminal prose.
- Reopen a checkpoint to show whether its source Session can be precisely resumed, is still
  Open, is Exited, or has unlinked evidence.
- Offer the correct continuation route without conflating identities:
  - precise resume keeps the same native Session ID and creates a new Managed Current Run;
  - Handoff creates a distinct Successor Session and preserves visible lineage.
- Reuse the existing explicit Handoff Operation and private Handoff Artifact for a
  cross-Provider or new-Session continuation. A checkpoint does not itself create a Handoff
  Artifact, launch a Successor, or grant control over an Externally Started Current Run.
- Keep the workflow useful for Codex CLI and Claude Code while exposing Provider-specific
  limitations rather than claiming identical control capabilities.

## Product rules

- A checkpoint is developer-directed. SessionMesh never creates one from elapsed time,
  context pressure, or a provider observation alone.
- The native Session ID remains Session identity. Work may continue across a Handoff, but a
  Successor is never merged with its source Session.
- Checkpoint data remains local and private. It must not persist prompt bodies, generated
  summaries, provider tool arguments, provider tool output, arbitrary terminal text, or
  repository file content.
- Repository evidence is metadata only. The exact evidence schema must be auditable and
  avoid treating raw shell output as an authoritative completion signal.
- Starting, resuming, and Handoff remain separate explicitly authorized operations. A
  checkpoint authorizes none of them.
- Existing Handoff Artifact storage and Successor-linkage rules remain unchanged: the
  Artifact stays outside the Project, and a Successor link requires conclusive
  launched-process evidence.

## Deferred

- Automatic checkpoint creation or automatic continuation.
- A generic Agent-generated work summary, transcript analysis, or background prompt
  extraction.
- Automatic provider or model switching.
- Action dispatch, structured Action Results, `needs_input`, Execution Plans, Workflow Runs,
  dependency scheduling, and automatic Flow advancement.
- Embedded terminal emulation, a SessionMesh daemon, and a second project or issue tracker.

## Open design decisions within the scope

- The exact durable representation and retention behavior of checkpoint metadata.
- The minimum repository evidence that is useful without collecting sensitive command or
  terminal content.
- Which checkpoint fields may be user-authored, Agent-proposed with explicit approval, or
  mechanically observed.
- The interaction that connects a checkpoint to the existing Handoff Operation without
  making a Handoff appear automatic.

## Intended layering

```text
v2.0.0  Managed Session Core
  -> v2.0.1  Work Continuity
       -> v2.1.0  Bounded Action Runtime       (scope pending)
       -> v2.2.0  Verified Action Results      (scope pending)
       -> v2.3.0  Approved Flow Scheduler      (scope pending)
```

## References

- [`v2.0.0 Managed Session Core`](../v2.0.0/README.md)
- [`Work Continuity requirements discussion`](../../docs/feature/v2.0.1/work-continuity/discussion.md)
- [`legacy Assisted Handoff record`](../../docs/legacy/v1/assisted-handoff-prd.md)
- [`docs/adr/0005-keep-handoff-artifacts-outside-projects.md`](../../docs/adr/0005-keep-handoff-artifacts-outside-projects.md)
- [`docs/adr/0007-link-successors-through-launched-process-evidence.md`](../../docs/adr/0007-link-successors-through-launched-process-evidence.md)
