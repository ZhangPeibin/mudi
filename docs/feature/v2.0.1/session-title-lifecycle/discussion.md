# Session Title Lifecycle - Requirements Discussion

**Status:** ready-for-prd
**Release:** v2.0.1
**Feature:** session-title-lifecycle
**Scope status:** This is a new v2.0.1 iteration requirement. The accepted
`feature/v2.0.1/README.md` boundary must be amended by the later specification and
delivery work; this discussion does not change the v2.0.0 baseline.

## Problem

Before the Title lifecycle change, the Title supplied through `New` was held by the Managed
Launch Registration until Managed Binding succeeded. Derived grouping validation could
therefore prevent an otherwise conclusively identified native Session from receiving its
Session Alias. The Project Tree then fell back to the Short Session ID even though the user
had already supplied a Title for that exact Session.

The observed failure involved two launches whose Provider source working directory matched
the Launch Location exactly. Both reached `Needs Review` only because the Project identity
captured before launch disagreed with the Project identity derived during Binding after the
directory's Git state changed. Title synchronization considered only successfully bound
Managed Sessions, so neither observed Session displayed its requested Title.

## Users and Outcomes

This affects developers who use `New` to launch Claude Code or Codex CLI Sessions and expect
the required Title to identify the resulting Session regardless of whether Mudi can also
establish management provenance.

After this change:

- a uniquely identified native Session receives the requested Title immediately;
- a later Managed Binding failure cannot remove or suppress that Title;
- the Project Tree displays the Title for the observed Session even when its Launch
  Registration is retained as `Needs Review`;
- incomplete or contradictory identity evidence never assigns a Title to a guessed Session.

## Pre-change Experience

`New` stores the requested Title on a Managed Launch Registration. Refresh obtains Provider
binding evidence and attempts the complete Managed Binding transaction. Only successfully
bound Managed Sessions participate in managed metadata synchronization. Consequently,
Title persistence was accidentally coupled to derived grouping validation and every other
Binding Receipt requirement.

The native Session ID remains the actual Session identity. Project membership is unrelated
to whether the requested Session Alias belongs to that identity.

## Scope

### In Scope

- Separate Title association from Managed Binding.
- Use conclusive Provider, launched-process, Provider-source, and native Session ID evidence
  to associate a Launch Registration with exactly one Session.
- Persist the requested Title as that Session's Session Alias as soon as the identity
  association is conclusive.
- Preserve the Session Alias when later Binding succeeds, waits, fails, or reaches
  `Needs Review`.
- Make the operation idempotent across refresh and restart without overwriting an existing
  Session Alias from a conflicting Registration.
- Render the resolved Session Label normally for an observed Session whose associated
  Registration is still under review.
- Add focused coverage for the successful, waiting, contradictory, restart, and downstream
  Binding-failure paths.

### Out of Scope

- Relaxing the evidence required to identify a native Session.
- Treating a Launch Registration, tmux session, directory, timestamp, or Title as Session
  identity.
- Changing Generate Title, manual Session Alias editing, Session Description lifecycle, or
  Label precedence.
- Simplifying Project grouping; that is owned by the separate
  `project-model-simplification` requirement and its delivery tickets.
- Migrating or repairing pre-change development data.

## Required Behaviour

1. `New` creates a Managed Launch Registration carrying the requested Title and Launch
   Location, then starts the selected Provider in Mudi's private tmux environment.
2. Mudi observes the exact launch process and Provider source until it can associate the
   launch with one Provider-native Session ID.
3. If the necessary identity evidence has not appeared yet, the Registration remains in its
   waiting Binding flow and no Session Alias is written.
4. If the evidence is contradictory, the Registration reaches `Needs Review` and no Title is
   assigned to any Session.
5. Once the evidence uniquely identifies `(Session Provider, native Session ID)`, Mudi writes
   the requested Title as that Session's Session Alias before and independently of the
   Managed Binding outcome.
6. Managed Binding then separately decides whether it can issue a Binding Receipt and
   establish the Managed Session and Managed Current Run relationships.
7. A downstream Binding failure may retain the Registration as `Needs Review`, but it must
   not delete, roll back, hide, or replace the already associated Session Alias.
8. Repeated refreshes and restart reconciliation must preserve the same result. A second or
   conflicting Registration must not silently overwrite an Alias already owned by the
   Session identity.

## Edge and Failure Cases

- A Provider has started but its source record or native Session ID is not available yet:
  wait without assigning a Title.
- Provider source evidence contains conflicting native Session IDs or disagrees with the
  exact launch process: retain the launch for review without assigning a Title.
- Identity association succeeds and a later cwd or other Binding validation fails: retain
  the Title and expose the Binding problem independently.
- Multiple Registrations claim the same native Session: preserve the established Session
  Alias and do not use a later request to overwrite it implicitly.
- Mudi restarts after Title association but before Binding completes: recover both the
  Session Alias and the independent Registration state.

## Constraints and Dependencies

- The native Session ID scoped by Session Provider remains Session identity.
- Provider-source and exact launched-process evidence remain mandatory; working-directory
  or timestamp proximity is never enough.
- Session Alias storage remains local metadata and never modifies the Provider-native
  Session.
- This requirement should be delivered before Project model simplification so the
  `Session identity -> Title` invariant is independently established and testable.
- ADR 0007's conclusive launched-process linkage principle and ADR 0009's Managed Current
  Run evidence boundary remain intact.

## Decisions

- Title belongs to the Session identity, not to Project membership or management
  provenance.
- Title association and Managed Binding are separate lifecycle steps that consume the same
  conclusive identity evidence.
- Missing evidence waits; contradictory evidence assigns no Title and requires review.
- A later Binding failure never revokes an already associated Title.
- The change is a v2.0.1 iteration even though it revises behavior introduced in v2.0.0.
- Old development data receives no migration, compatibility reader, or automatic cleanup.

## Open Questions

None.

## Pre-change Evidence

- `src/application/observation/refresh.rs`: Binding reconciliation ran before managed
  metadata synchronization, and synchronization iterated Managed Sessions.
- `src/application/managed_sessions.rs`: `binding_receipt_candidate` combined identity,
  cwd, and derived grouping validation before management was established.
- `src/adapters/providers/codex.rs` and `claude.rs`: Provider adapters already normalize the
  exact process, source, native Session ID, and working-directory evidence.
- `src/presentation/tui/project_tree`: Session Label resolution falls back to Short Session
  ID when no Session Alias is persisted.
- The reproduced launches had matching canonical Provider cwd and Launch Location but
  failed the derived grouping comparison after filesystem Git state changed.

## Session Log

### 2026-08-18

- Diagnosed the missing Project Tree Titles as a lifecycle coupling problem rather than a
  cwd mismatch or missing native Session identity.
- Confirmed that Title must be written once the native Session ID is conclusively associated
  with the exact launch, independently of Managed Binding.
- Confirmed waiting, contradictory-evidence, downstream failure, conflict, and restart
  behavior.
- Confirmed that this Title change should be delivered before Project model simplification.
- Confirmed the complete launch, identity, Title, Binding, and Project grouping flow as the
  shared understanding.
- Assigned the requirement to v2.0.1. The user clarified the release policy: v2.0.0 is a
  closed baseline, and all later requirement changes are iterations in v2.0.1 even when they
  revise a v2.0.0 design.
- The user explicitly confirmed that the analysis is ready for a formal PRD.
