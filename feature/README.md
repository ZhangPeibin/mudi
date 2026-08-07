# Versioned feature records

Agreed product increments are recorded under `feature/vX.Y.Z/`. Each version directory
contains a `README.md` that states the capability boundary, included behavior, deferred
work, and known architectural prerequisites for that increment.

These records are scope baselines, not PRDs or architectural decisions:

- GitHub Issues in `ZhangPeibin/SessionMesh` remain authoritative for PRDs and delivery
  tickets.
- `CONTEXT.md` remains authoritative for shared domain vocabulary.
- Accepted architectural decisions and superseding decisions live under `docs/adr/`.
- Exploration records may preserve discussion but do not override any of the above.

## Versions

- [`v2.0.1`](./v2.0.1/README.md) - Managed Session Core
