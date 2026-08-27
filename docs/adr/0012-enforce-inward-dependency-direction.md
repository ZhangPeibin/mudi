# ADR 0012 - Enforce inward dependencies through application-owned ports

- **Status:** Accepted (2026-08-27)
- **Scope:** v2.0.1 architecture stabilization

## Context

Mudi has `domain`, `application`, `adapters`, and `presentation` directories, but the current
dependency graph does not enforce those names. Application code accepts TUI state, writes UI
notices, opens SQLite directly, launches system commands, and selects concrete macOS
implementations. Presentation also owns authoritative Session Metadata. This makes a new
workflow expand the global controller and TUI while coupling policy tests to local system
tools.

## Decision

Dependencies will point inward within the existing single Rust package:

- `domain` contains provider-neutral data and rules and depends on no application, adapter,
  presentation, or I/O implementation.
- `application` depends on `domain` and port traits owned by the application use cases. It
  contains workflow policy but no presentation types, `rusqlite`, process commands, Ratatui,
  or concrete macOS implementations.
- `adapters` implements application ports for SQLite, Provider files and commands, process
  evidence, tmux, Terminal.app, clipboard, notifications, and other external boundaries.
- `presentation` turns interaction into application commands and renders read-only view
  models. It does not open repositories or write authoritative business state.
- `main.rs` is the composition root. It constructs implementations and connects them without
  owning SQL, Provider branches, authorization policy, or UI workflow rules.

Each application workflow accepts a typed request and returns a typed outcome or error.
Presentation owns user-facing wording, clipboard interaction, and requests to refresh the
projection. Port traits are introduced only for real external boundaries, not mechanically
for every concrete type. Lightweight dependency checks in CI will enforce the direction once
the migration makes each rule green.

## Consequences

Managed New, Binding, Current Run access, Resume, Recovery, Handoff, Session Metadata, and
Curation can be tested without tmux, Terminal.app, Provider executables, or a TUI. Concrete
integration remains local and macOS-specific where the product requires it. The migration is
incremental: one workflow moves completely before its old implementation is removed, without
permanent facades or dual implementations.

Splitting the package into multiple crates was rejected for now because source-level gates
can enforce the required direction without adding release and build structure. Retaining a
global controller that mutates `Ui` was rejected because it leaves presentation response
wording and rescan transport inside application policy.
