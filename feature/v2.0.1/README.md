# v2.0.1 - Work Continuity

| Field | Value |
|---|---|
| Status | Accepted scope baseline; Checkpoint deferred 2026-08-28 |
| Accepted | 2026-08-10 |
| Role | Repeat-use and inventory layer after Managed Session Core |

`v2.0.1` lets a developer keep the Session inventory focused and reliably re-enter a
known Session through precise Resume or explicit Handoff. It builds on `v2.0.0` Managed
Session Core; it is not an Action Runtime or Flow Scheduler.

## Goal

Give a developer a clear continuation choice for a selected Session: precisely resume the
same provider-native Session, or explicitly Handoff to a distinct Successor Session. Keep
the repeat-use inventory focused through explicit, durable curation without deleting
Provider-native history.

The primary question answered is: **which known Session can be continued safely, and by
which explicit route?**

## Included

- Keep a Project in the normal inventory while it has an Open Session or its newest visible
  Session falls within the rolling seven-day window, including after Mudi or macOS restarts.
- Let the developer explicitly hide Exited Sessions, restore them from a dedicated Hidden
  Tree, or permanently exclude them from Mudi inventory. Project Hide is a snapshot
  bulk action over currently observed Session identities and never suppresses future Sessions.
- Refine `New` into a reusable keyboard-first property editor with explicit Provider and
  Location choices plus inline Title and Description editing. Every `New` enables the
  system-owned Binding Initializer without exposing it as a property.
- Associate the Title supplied through `New` with the exact Session Provider and native
  Session ID as soon as conclusive Provider source and launched-process evidence identifies
  the Session. Persist it as a first-writer-wins Session Alias before and independently of
  the later Managed Binding outcome.
- Show whether a selected Session can be precisely resumed, is still Open, is Exited, or
  has unlinked evidence.
- Offer the correct continuation route without conflating identities:
  - precise resume keeps the same native Session ID and creates a new Managed Current Run;
  - Handoff creates a distinct Successor Session and preserves visible lineage.
- Reuse the existing explicit Handoff Operation and private Handoff Artifact for a
  cross-Provider or new-Session continuation. A ready Handoff Result does not itself launch
  a Successor or grant control over an Externally Started Current Run.
- Keep the workflow useful for Codex CLI and Claude Code while exposing Provider-specific
  limitations rather than claiming identical control capabilities.

## Product rules

- The native Session ID remains Session identity. Work may continue across a Handoff, but a
  Successor is never merged with its source Session.
- Starting, resuming, and Handoff remain separate explicitly authorized operations.
- The v2.0.1 `New` workflow supersedes v2.0.0's opt-in Binding Initializer presentation:
  the initializer is always enabled for `New`, remains system-owned rather than developer
  task content, and keeps its Provider-specific command details inside Provider adapters.
- Title association and Managed Binding consume the same conclusive launch-identity contract
  but establish different relationships. A Session Alias grants no Binding Receipt, Managed
  Session, Managed Current Run, management provenance, or control authority, and later launch
  reconciliation never replaces an existing or user-edited Alias.
- Inventory curation is Session-scoped and developer-directed. A validated Open Session
  temporarily overrides Hide, while permanent exclusion never deletes Provider-native logs
  or cascades into independently owned control, task, Handoff, or Successor records.
- Existing Handoff Artifact storage and Successor-linkage rules remain unchanged: the
  Artifact stays outside the Project, and a Successor link requires conclusive
  launched-process evidence.

## Deferred

- All Checkpoint capabilities, including creation, durable metadata, repository evidence,
  retention, reopening, and connections to Resume or Handoff. No Checkpoint schema or UI is
  part of the current v2.0.1 delivery boundary.
- A generic Agent-generated work summary, transcript analysis, or background prompt
  extraction.
- Automatic provider or model switching.
- Action dispatch, structured Action Results, `needs_input`, Execution Plans, Workflow Runs,
  dependency scheduling, and automatic Flow advancement.
- Embedded terminal emulation, a Mudi daemon, and a second project or issue tracker.

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
- [`Inventory Curation requirements discussion`](../../docs/feature/v2.0.1/inventory-curation/discussion.md)
- [`Inventory Curation PRD`](https://github.com/ZhangPeibin/mudi/issues/55)
- [`New property editor delivery issue`](https://github.com/ZhangPeibin/mudi/issues/63)
- [`Session Title lifecycle PRD`](https://github.com/ZhangPeibin/mudi/issues/65)
- [`Early Title association delivery issue`](https://github.com/ZhangPeibin/mudi/issues/67)
- [`Title reconciliation delivery issue`](https://github.com/ZhangPeibin/mudi/issues/68)
- [`docs/adr/0005-keep-handoff-artifacts-outside-projects.md`](../../docs/adr/0005-keep-handoff-artifacts-outside-projects.md)
- [`docs/adr/0007-link-successors-through-launched-process-evidence.md`](../../docs/adr/0007-link-successors-through-launched-process-evidence.md)
