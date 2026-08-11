# v2.0.0 - Managed Session Core

| Field | Value |
|---|---|
| Status | Accepted scope baseline |
| Accepted | 2026-08-07 |
| Renumbered | 2026-08-09 from `v2.0.1` |
| Role | First deliverable increment for SessionMesh 2.0 |

This record fixes the product boundary for `v2.0.0`. It is not a PRD; implementation PRDs
and delivery tickets remain in GitHub Issues.

The scope was accepted on 2026-08-07 and renumbered from `v2.0.1` on 2026-08-09 to make
Managed Session Core the first SessionMesh 2.0 increment. The renumbering changes no
product or architecture decision.

The complete design is split across five editable
[Excalidraw architecture diagrams](./diagrams/README.md): system context, logical
components, Managed Session lifecycle, and capability boundary.

## Goal

Give a developer one reliable control surface for starting, recognizing, revisiting, and
directly accessing many provider-native Sessions across Projects. This version establishes
the managed execution foundation before SessionMesh attempts Action or Flow orchestration.

## Included

- Select a Project and, for a Git Project, a Worktree in which to start work.
- Start either Codex CLI or Claude Code through its provider-native `New` command; conclusive
  binding evidence then establishes the Managed Session.
- Offer an explicitly enabled, launch-time Binding Initializer for a new Session when the
  developer wants source evidence before entering a first task. Without it, the developer's
  first Provider prompt creates that source evidence.
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
- The user-facing `New` and `Resume` commands invoke the selected Provider's native new-session
  and resume commands respectively. `Managed` is verified management provenance, not a second
  set of user-facing lifecycle commands; terminal-window allocation is an implementation detail.
- A Binding Initializer is disabled by default and is available only during `New`. It is a
  system-owned, explicitly enabled request used to produce source evidence, not the developer's
  first task or a general Prompt-control feature.
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
Tasks retain ADR 0003's detached-worker and recovery rules; `v2.0.0` does not migrate those
Tasks or decide the deferred Action Runtime.

## Related discussion

- [`Managed Session Core requirements discussion`](../../docs/feature/v2.0.0/managed-session-core/discussion.md)
