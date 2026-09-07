# Documentation Map

## Sources of Truth

- [`CONTEXT.md`](../CONTEXT.md) - shared domain vocabulary, privacy rules, and durable
  product invariants.
- [`adr/`](./adr/) - accepted architectural decisions.
- [`../feature/`](../feature/) - accepted version capability boundaries.
- [GitHub Issues](https://github.com/ZhangPeibin/mudi/issues) - formal PRDs and
  delivery tickets.

## Active Product Work

Pre-PRD requirements discussions live at
`docs/feature/<release>/<feature-slug>/discussion.md`. They explore a capability but do
not change an accepted version scope on their own.

- [`v2.0.0 / Managed Session Core`](./feature/v2.0.0/managed-session-core/discussion.md)
- [`v2.0.1 / Work Continuity`](./feature/v2.0.1/work-continuity/discussion.md)
- [`v2.0.1 / Failed Session Recovery`](./feature/v2.0.1/failed-session-recovery/discussion.md)
  - candidate small requirement; not yet part of the accepted v2.0.1 scope.
- [`v2.0.1 / Managed Session Lifecycle Tasks`](./feature/v2.0.1/managed-session-lifecycle-tasks/discussion.md)
  - candidate change that unifies New, Resume, and Handoff under Operation Tasks.

## Engineering Conventions

- [`source-map.md`](./source-map.md) - module-by-module map of `src/`, for onboarding
  readers.
- [`refactoring/v2.0.1-architecture-refactoring-plan.md`](./refactoring/v2.0.1-architecture-refactoring-plan.md)
  - completed architecture stabilization plan; its Checkpoint implementation phase is
    deferred from the current v2.0.1 delivery boundary.
- [`agents/`](./agents/) - issue-tracker, domain-document, triage-label, and feature-doc
  conventions used by engineering skills.
- Product identity and storage ownership are defined by `README.md`, `CONTEXT.md`, and the
  accepted architecture ADRs; Mudi does not maintain a legacy product-identity compatibility
  layer.

## Research and History

- [`research/`](./research/) - fixed-version technical research; it informs design but is
  not product or architecture authority.
