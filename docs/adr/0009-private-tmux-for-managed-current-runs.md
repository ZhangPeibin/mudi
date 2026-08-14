# ADR 0009 - Use private tmux for Managed Current Runs

- **Status:** Accepted (2026-08-07)
- **Scope:** v2.0.0 Managed Session Core
- **Version note:** Renumbered from v2.0.1 on 2026-08-09; no decision changed.
- **Partially supersedes:** ADR 0003

Every Managed Current Run will execute inside a private tmux environment created by
Mudi. The environment belongs to the Managed Session, while each provider process
belongs to one Current Run and is linked to the native Session ID using conclusive launch,
process, and provider-source evidence; a tmux name or pane is never Session identity.
Normal TUI exit detaches without terminating that environment, and restart reconciliation
must leave ambiguous evidence unlinked rather than guessing. tmux is required for Managed
Session capabilities, but passive observation of Externally Started Current Runs remains
available without it.

tmux is the common substrate because it can host both Codex CLI and Claude Code, preserves
an addressable process and terminal surface across TUI lifetimes, and permits attachment
from an external terminal. Codex app-server cannot provide the provider-neutral base,
macOS terminal launchers do not provide owned lifecycle control, and a daemon is not needed
to establish this first runtime boundary. tmux provides containment and terminal transport
only: authority still comes from explicit authorization, identity from provider and process
evidence, and completion from declared result evidence rather than terminal text.

This decision supersedes ADR 0003 only where that ADR rejects a required tmux dependency.
It preserves ADR 0003's detached lifetime, authoritative state, restart reconciliation,
and no-automatic-retry guarantees for existing Operation Tasks; `v2.0.0` does not migrate
those Tasks or decide the later Action Runtime.
