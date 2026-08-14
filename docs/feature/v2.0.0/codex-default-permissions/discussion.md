# Codex Default Permissions - Requirements Discussion

**Status:** ready-for-prd
**Release:** v2.0.0
**Feature:** codex-default-permissions

## Problem

Mudi currently starts a Managed Codex CLI Session without an explicit sandbox or
approval-policy override. The resulting permissions depend on the developer's Codex
configuration and may interrupt work with approval prompts. The requested outcome is for a
Mudi-started Codex Session to begin with the highest intended permission level by
default.

## Users and Outcomes

Developers who explicitly start or resume a Managed Codex Session should receive a predictable
permission policy instead of inheriting an unknown local default. The selected policy and its
safety implications must be unambiguous.

## Current Experience

The Codex runtime adapter currently supplies only native New or Resume arguments. It does not
set `--sandbox`, `--ask-for-approval`, or
`--dangerously-bypass-approvals-and-sandbox`, so Codex resolves those values from its own
configuration.

## Scope

### In Scope

- Select an explicit default permission policy for Mudi-started Managed Codex Sessions.
- Keep the permission behavior inside the Codex runtime adapter.
- Make the effective high-risk default visible before the developer authorizes launch if the
  chosen policy disables both sandboxing and approvals.

### Out of Scope

- Changing Claude Code permissions.
- Modifying the developer's global Codex configuration files.
- Granting Mudi authority over Externally Started Current Runs.
- Treating one launch authorization as authority for unrelated Mudi Control Operations.

## Required Behaviour

- Mudi passes a deterministic Codex-native permission policy on launch.
- Managed Codex New and Resume both pass
  `--dangerously-bypass-approvals-and-sandbox`.
- The policy applies only to the exact Managed launch the developer authorizes.
- Provider-specific flags remain behind `RuntimeControlPort`.
- When Codex is selected in New, the form prominently displays
  `Full access · No sandbox · No approvals`.
- The Managed Codex Resume authorization displays the same warning.
- The warning is informational rather than a separate toggle or second confirmation;
  `Start New` and `Authorize Resume` remain the explicit per-launch authorization actions.
- Launch, Binding Receipt, Project/Worktree identity, and restart evidence rules remain
  unchanged.

## Edge and Failure Cases

- An installed Codex version that rejects the selected arguments must produce a known launch
  failure; Mudi must not silently retry with weaker or different permissions.
- Resume must not accidentally use a different policy from New if the agreed scope includes
  both paths.
- Provider configuration must not silently override an explicit Mudi launch policy.

## Constraints and Dependencies

- ADR 0002 requires explicit, target-specific authorization for Mudi control.
- The v2.0.0 scope currently authorizes starting the Provider CLI, but does not state that this
  authorization grants an unbounded model-generated command bypass.
- `codex --help` describes `--dangerously-bypass-approvals-and-sandbox` as extremely dangerous
  and intended only for externally sandboxed environments. A private tmux is a terminal
  transport and lifecycle boundary, not an external sandbox.
- Codex also exposes the separate policies `--sandbox danger-full-access` and
  `--ask-for-approval <policy>`.

## Decisions

- The policy will be passed per invocation; Mudi will not rewrite user configuration.
- The developer explicitly selected complete bypass through
  `--dangerously-bypass-approvals-and-sandbox`, accepting that a private tmux is not an
  external sandbox and that Codex will execute without sandboxing or approval prompts.
- The bypass policy applies consistently to both Managed Codex New and Managed Codex Resume.
- The New and Resume authorization surfaces always disclose the effective bypass policy. They
  do not add another switch or confirmation step.

## Open Questions

None.

## Evidence

- `src/providers/codex.rs` - current New and Resume native arguments.
- `src/managed_sessions.rs` - Managed launch authorization and private tmux boundary.
- `feature/v2.0.0/README.md` - accepted Managed Session Core scope.
- `docs/feature/v2.0.0/managed-session-core/discussion.md` - existing authorization decisions.
- Local `codex --help`, inspected 2026-08-11 - supported sandbox and approval policies and the
  explicit warning for the bypass flag.

## Session Log

### 2026-08-11

- Captured the request to improve the Managed New dialog, fix tmux scrolling, and start Codex
  with the highest permissions by default.
- Routed the dialog and scrolling work to routine UI/UX delivery; isolated Codex permissions
  here because it changes a durable safety boundary.
- Confirmed that current launches inherit Codex permission configuration and that private tmux
  is not an external sandbox.
- The developer explicitly selected Codex's complete sandbox-and-approval bypass rather than
  full filesystem access with interactive approvals.
- The developer confirmed that Managed Codex New and Resume use the same bypass policy.
- The developer confirmed a fixed warning on both authorization surfaces and retained the
  existing per-launch authorization action without another toggle or confirmation step.
- The developer confirmed that the analysis is ready to become a formal GitHub Issue PRD.
