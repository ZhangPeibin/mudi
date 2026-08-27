# ADR 0011 - Separate observation, reconciliation, projection, and publication

- **Status:** Accepted (2026-08-27)
- **Scope:** v2.0.1 architecture stabilization

## Context

The current refresh cycle both observes external facts and mutates authoritative Mudi state:
it scans Provider sources, updates the disposable index, advances Binding, associates Titles,
synchronizes Session Metadata, verifies managed lifecycles, constructs a Snapshot, and
publishes it. A reader cannot tell from the refresh boundary which work is passive and which
work may change durable state, and failures from those responsibilities share one diagnostic
surface.

## Decision

The background cycle will have four explicit stages:

1. **Collect** reads Provider sources and process facts, updates only the disposable index,
   and produces an immutable `ObservationFrame`. It never writes authoritative state.
2. **Reconcile** uses that frame to advance only existing, explicitly authorized Binding,
   Managed Current Run, Resume, and Operation Task records through authoritative repository
   transactions. It cannot create authorization, and replaying the same frame is idempotent.
3. **Project** combines the frame with read-only authoritative metadata into a Snapshot. It
   performs no authoritative writes.
4. **Publish** atomically replaces the current read-model Snapshot.

Each stage reports structured diagnostics independently. Missing, stale, conflicting, or
ambiguous evidence retains the existing safe state instead of being converted into identity,
authority, or success. This decision changes ownership and sequencing, not the two-second
polling interval or the safety contracts accepted by ADRs 0002 through 0009.

## Consequences

Passive observation can be tested with read-only authoritative repositories, reconciliation
can be tested for idempotence and authorization boundaries, and projection failures cannot
hide a durable write inside a nominally read-only operation. The cycle has more explicit
types and services, but it does not require an event bus, CQRS framework, daemon, or workflow
engine.

Keeping one `RefreshService::refresh()` orchestration boundary was rejected because naming
internal helper calls would not make write authority or failure behavior observable. Running
reconciliation from presentation commands alone was also rejected because restart recovery
still requires evidence-driven background progress for already authorized work.
