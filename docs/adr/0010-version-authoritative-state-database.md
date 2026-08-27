# ADR 0010 - Own authoritative metadata in one versioned state database

- **Status:** Accepted (2026-08-27)
- **Scope:** v2.0.1 architecture stabilization

## Context

Mudi's authoritative metadata is currently split across the Operation Task SQLite store,
the Managed Launch SQLite store, and the Session Label JSON store. Each owner creates and
alters its own schema at runtime. This prevents one transaction from preserving invariants
across Task, Binding, Managed Session, Session Metadata, and Curation records, and it lets
the detached Worker and TUI observe different schema assumptions. The Provider source files
and disposable Session index have different authority and must remain outside this boundary.

## Decision

Mudi will keep all Mudi-owned authoritative metadata in one `state.db`. One
`StateDatabase` implementation is the sole Schema Owner and opens the database with a
declared `PRAGMA user_version`, foreign keys enabled, WAL, a bounded busy timeout, and short
transactions. Business-specific repositories remain separate logical interfaces, but they
share this physical database and its transaction boundary. The Handoff Artifact body remains
an owner-only file outside SQLite, and `index.db` remains disposable and rebuildable from
Provider source files.

The first cutover is a deliberate development-stage reset rather than a migration. When no
`state.db` exists but pre-cutover stores exist, Mudi moves `tasks.db`,
`managed-launches.db`, `session-labels.json`, `index.db`, and the Handoff Artifact root into
one timestamped owner-only `legacy-reset` backup before creating fresh `state.db` version 1
and `index.db`. It does not import, probe, or dual-write the backed-up formats. Once
`state.db` exists, an unknown `user_version` fails fast and never causes an automatic reset.

## Consequences

- Cross-record invariants can be committed atomically and tested against one schema contract.
- The TUI and detached Worker open the same schema through the same owner.
- The cutover preserves old private data for manual recovery but does not preserve it as
  active application state.
- A future schema change requires an explicit migration or explicit reset decision; a
  repository may not add columns opportunistically during `open()`.

Maintaining independent authoritative stores, permanently supporting the current ad hoc
migrations, and dual-writing old and new stores were rejected because they retain split
transactions or create divergent sources of truth. A full import migration was rejected
because these formats are development-stage, their historical constraints are not reliably
covered, and the owner-only backup provides a safer recovery boundary.
