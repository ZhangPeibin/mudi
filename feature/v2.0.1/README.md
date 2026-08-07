# v2.0.1 - Managed Session Core

| Field | Value |
|---|---|
| Status | Accepted scope baseline |
| Date | 2026-08-07 |
| Role | First base module for SessionMesh 2.0 |

This record fixes the product boundary for `v2.0.1`. It is not a PRD; implementation PRDs
and delivery tickets remain in GitHub Issues.

The complete design is split across four editable
[Excalidraw architecture diagrams](./diagrams/README.md): system context, logical
components, Managed Session lifecycle, and capability boundary.

## Goal

Give a developer one reliable control surface for starting, recognizing, revisiting, and
directly accessing many provider-native Sessions across Projects. This version establishes
the managed execution foundation before SessionMesh attempts Action or Flow orchestration.

## Included

- Select a Project and, for a Git Project, a Worktree in which to start work.
- Start either Codex CLI or Claude Code as a new Managed Session.
- Capture a Title and Description so the developer can distinguish the Session from other
  Sessions in the same Project. The Title is stored as the Session Alias; neither value is
  technical identity.
- Create a private tmux execution environment for the Managed Session.
- Conclusively bind the Project, Worktree, Session Provider, native Session ID, Current Run,
  provider process, and tmux session/pane using launch and process evidence.
- List and inspect Managed Sessions without hiding externally started Sessions already
  observable by SessionMesh.
- Attach to and detach from a Managed Session through an external terminal.
- Precisely resume the same provider-native Session in a new Managed Current Run.
- Keep managed tmux sessions alive when the SessionMesh TUI exits normally.
- Reconcile Session identity, Current Run identity, and live process state when SessionMesh
  restarts. Ambiguous evidence must remain unlinked rather than being guessed.
- Continue to observe Externally Started Current Runs without silently assuming control.

## Product rules

- The native Session ID remains Session identity. tmux is an execution environment, not a
  second Session identity.
- A Managed Session may have multiple Current Runs. Management provenance belongs to each
  Current Run, so an externally started Run does not become controllable merely because a
  later Run of the same Session is managed.
- Title and Description persist when the same Session is precisely resumed. A Handoff
  creates a distinct Successor Session with independent metadata.
- Passive observation remains the default. Starting or resuming a Managed Current Run is
  an explicitly authorized operation.
- Codex app-server may provide optional capabilities later, but it is not a required path.
  The common base must support both Codex CLI and Claude Code.
- Project remains the repository-scale scope for Worktrees and Sessions; this feature does
  not replace the Project concept.

## Deferred

- Execution Plans, Workflow Runs, and dependency scheduling.
- Automatic Session-to-Session delegation or communication.
- Skill-backed Action dispatch.
- Structured Action Result contracts and `needs_input` handling.
- Automatic successor Action scheduling.
- MCP or another Agent callback channel.
- A long-lived SessionMesh daemon.
- An embedded terminal emulator in the SessionMesh TUI.

The intended layering is:

```text
Managed Session Core
  -> Action Runtime
  -> Structured Result
  -> Flow Scheduler
```

## Architecture decision

ADR 0009 establishes private tmux as the required execution substrate for Managed Current
Runs and partially supersedes ADR 0003's rejection of that dependency. Existing Operation
Tasks retain ADR 0003's detached-worker and recovery rules; `v2.0.1` does not migrate those
Tasks or decide the deferred Action Runtime.
