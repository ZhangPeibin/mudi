# Versioned feature records

Agreed product increments are recorded under `feature/vX.Y.Z/`. Each version directory
contains a `README.md` that states the capability boundary, included behavior, deferred
work, and known architectural prerequisites for that increment.

These records are scope baselines, not PRDs or architectural decisions:

- GitHub Issues in `ZhangPeibin/mudi` remain authoritative for PRDs and delivery
  tickets.
- `CONTEXT.md` remains authoritative for shared domain vocabulary.
- Accepted architectural decisions and superseding decisions live under `docs/adr/`.
- Exploration records may preserve discussion but do not override any of the above.

## Versions

- [`v2.0.0`](./v2.0.0/README.md) - Managed Session Core
- [`v2.0.1`](./v2.0.1/README.md) - Work Continuity

## Planned Direction

The following order is an agreed product direction, not an accepted feature scope. A version
directory and GitHub Issue PRD are created only after its requirements discussion fixes the
capability boundary.

| Future version | Product capability | Status |
|---|---|---|
| `v2.1.0` | Bounded Action Runtime: explicitly authorize one Managed Session to perform one declared, bounded Action | Scope pending |
| `v2.2.0` | Verified Action Results: establish evidence that an Action reached its declared result instead of trusting completion prose | Scope pending |
| `v2.3.0` | Approved Flow Scheduler: coordinate multiple Sessions through a developer-approved Flow using verified results | Scope pending |
