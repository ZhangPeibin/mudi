# Work Continuity - Requirements Discussion

**Status:** deferred; Checkpoint is outside the current v2.0.1 delivery boundary
**Release:** v2.0.1
**Feature:** work-continuity
**Last updated:** 2026-08-28

The accepted boundary is [`feature/v2.0.1/README.md`](../../../../feature/v2.0.1/README.md).
`CONTEXT.md` and accepted ADRs remain authoritative for domain and architecture decisions.

## How to read this document

This is a synthesis of the accepted `v2.0.0` Managed Session Core and `v2.0.1` Work
Continuity baselines, plus the detailed design questions still under discussion. It
deliberately distinguishes three states:

- **Accepted baseline**: scope already recorded in `feature/v2.0.0` or `feature/v2.0.1`.
- **Candidate requirement**: a useful product hypothesis, not yet committed to a version.
- **Open decision**: a choice that needs product discussion before it can become a
  requirement, update `CONTEXT.md`, or lead to an ADR.

This document does not expand accepted scope, authorize implementation, or publish an Issue
PRD.

## Current scope decision

Checkpoint is deferred from the current v2.0.1 delivery boundary. The repository has no
active delivery task that demonstrates a continuity gap requiring a new durable Checkpoint
object: GitHub Issues own intended work, Git owns code state, `state.db` owns Mudi workflow
state, precise Resume continues one native Session, and Handoff transfers context to a
distinct Successor Session.

No Checkpoint Schema, Repository, evidence collector, or UI should be implemented from this
discussion. Reopening the capability requires a concrete interrupted-work case that cannot
be recovered adequately through the existing Issue, Git, Resume, and Handoff surfaces.

## Product thesis

Coding-agent work is currently trapped in provider conversations. When a developer
returns the next day, reaches a context limit, opens a parallel Session, or changes from
Codex CLI to Claude Code, they must reconstruct what the work was, what changed, what was
verified, and what must happen next.

The candidate product direction is a local **continuity layer** for coding work:

> The work should remain understandable and recoverable while provider-native Sessions
> remain distinct, replaceable workers.

Mudi would not become a model, a transcript archive, an issue tracker, or the
authority for product requirements. Claude Code and Codex CLI continue to own reasoning,
tool execution, and their native Sessions. Mudi owns only local execution evidence,
explicitly authorized control, and the developer-facing continuity surface.

## The developer outcome

A developer should be able to leave one coding Session and return through another without
re-explaining the work or trusting an opaque chat summary. Before the next provider starts,
the developer can inspect a small, evidence-backed account of:

- the intended outcome and constraints;
- the exact working directory and Project;
- files and repository state changed during the work;
- commands and checks actually run, with their observed result;
- decisions, unresolved risks, and the next useful action; and
- whether continuation is the same native Session or a distinct Successor Session.

The important promise is not that Mudi produces a cleverer summary. It is that a
developer can quickly determine what is known, what is an Agent claim, and what has
verifiable local evidence.

## Accepted v2.0.0 foundation

`v2.0.0` remains the first prerequisite, not the continuity product itself.

| Capability | v2.0.0 position |
|---|---|
| Start a Codex CLI or Claude Code Managed Session in a selected Project directory | Accepted |
| Persist local Title (Session Alias) and Description | Accepted |
| Bind native Session ID, provider process, Current Run, and private tmux evidence conclusively | Accepted |
| Inspect and attach to / detach from a Managed Session through an external terminal | Accepted |
| Keep managed tmux alive after normal TUI exit and reconcile on restart | Accepted |
| Precisely resume the same provider-native Session in a new Managed Current Run | Accepted |
| Continue to observe Externally Started Current Runs without assuming control | Accepted |
| Automatic delegation, Action dispatch, Result contracts, Flow scheduling, daemon, embedded terminal | Deferred |

The following invariants stay in force:

- A native Session ID is Session identity. tmux is execution transport and evidence, never
  identity.
- Starting, attaching, detaching, and resuming are explicit control operations; observation
  does not grant authority.
- Ambiguous identity or process evidence stays unlinked rather than being guessed.
- Mudi stores no prompt bodies, generated summaries, tool arguments, tool output, or
  repository file contents in its local stores.

## Deferred Checkpoint direction and open design

The working name **Relay** means a developer-directed transition between coding Sessions.
It is not yet a domain term and must not be added to `CONTEXT.md` until its durable model is
accepted.

Relay must distinguish two fundamentally different paths:

| Developer intent | Correct continuity mechanism | Identity rule |
|---|---|---|
| Return to the same provider-native conversation | Precise resume | Same native Session; new Current Run |
| Continue with another Provider or a new conversation | Authorized Handoff to a Successor | Distinct native Session; visible lineage only |

For example, moving from a Codex Session to a Claude Session can preserve the work's
continuity, but it must not claim that the two provider-native Sessions are one Session.
The existing Handoff Artifact is the candidate bridge for this second path. This draft does
not change its content contract, storage boundary, or authorization rules.

## Deferred scope notes and open design questions

### R1. Developer-directed checkpoint

Before leaving a Session, a developer should be able to explicitly create a continuity
checkpoint. The checkpoint should help answer "what can the next Session safely do now?"
without asking the developer to reconstruct the full history manually.

The candidate checkpoint contains:

- an objective and explicit constraints supplied or confirmed by the developer;
- repository evidence available locally at checkpoint time, such as Project, exact working
  directory, revision, diff status, changed-path list, and commands/checks run;
- clearly labelled Agent-proposed decisions, unknowns, and next action; and
- links to the source Session and any successor relationship.

**Open:** whether this is a new durable artifact, an extension of Handoff Artifact, or a
rendered view over existing evidence. That choice affects both the privacy model and the
domain model.

### R2. Evidence before prose

Every continuity view must separate verified facts from human or Agent statements. A claim
such as "tests pass" is useful only when it identifies the command, observed outcome, and
applicable repository state. Missing evidence must be shown as missing, not inferred from
the last Agent response.

The initial implementation should favor a small number of high-confidence facts over a
comprehensive synthetic narrative. It must never silently scrape or store provider
conversation content to fill a gap.

### R3. Immediate useful re-entry

After a restart, provider switch, or return to an existing Project, the developer should
be able to choose a known continuation and get to a useful next action rapidly. The UI
should make the following visible before it asks the developer to act:

- whether the prior Current Run is Open, Exited, or unlinked;
- whether same-Session precise resume is available;
- whether an authorized Handoff is needed for a distinct Successor Session;
- the most recent verified checks and repository state; and
- unresolved risks that require developer judgment.

This is a control and recovery surface, not a global chat-history browser.

### R4. Provider-neutral work continuity without false symmetry

The developer-facing continuity workflow should be useful for Codex CLI and Claude Code.
The underlying controls may differ by Provider, and the UI must expose unavailable or
unverified capability rather than pretending both providers have identical protocols.

Codex app-server may enrich Codex-specific behavior later, but it cannot be a requirement
for this core path. The common path remains private tmux, Provider adapters, provider source
evidence, and explicit authorization.

### R5. Safety around concurrent work

The continuity surface should make it difficult to accidentally continue the same work in
two conflicting places. At minimum it must surface:

- whether the source Session has a validated Open Current Run;
- Shared Project Risk already observed by Mudi; and
- whether the requested continuation is a same-Session resume or a separate Successor.

It must not silently resume a Session alongside a validated Open Current Run, overwrite a
working directory, or take over an Externally Started Current Run.

### R6. Frequent-use interaction, not a periodic report

The intended repeat loop is deliberately small:

```text
begin or inspect work
  -> work in Codex CLI or Claude Code
  -> checkpoint before leaving, handing off, or changing approach
  -> resume the same Session or hand off to a distinct Successor
  -> inspect evidence and take the next action
```

The product must make this faster than manually rereading a transcript, checking `git diff`,
and explaining the situation to a new Agent. It should not require the developer to manage
a second project-management hierarchy.

## Deferred candidate user journey

1. The developer chooses a Project directory, then explicitly starts a Managed Session
   through the v2.0.0 flow.
2. Mudi records only the metadata and conclusive launch evidence allowed by the
   current privacy and control rules. The developer works in the Provider terminal.
3. Before a context limit, a model change, or a break, the developer explicitly requests a
   checkpoint. Mudi gathers allowed local evidence and presents any Agent-proposed
   statements separately for confirmation or correction.
4. To continue with the same provider-native Session, the developer authorizes precise
   resume once its prior Current Run has exited.
5. To continue through a different Provider or a new conversation, the developer uses the
   existing explicit Handoff path. Mudi creates and later links a distinct Successor
   Session only through conclusive launched-process evidence.
6. The next Session opens with the approved, evidence-backed continuity material, while
   the developer can still inspect the exact source Session, repository evidence, and
   lineage.

## Explicit non-goals for this discussion slice

- Replacing GitHub Issues, PRDs, or ADRs with an internal work-item tracker.
- Treating Mudi as an Agent runtime or interpreting transcripts as product truth.
- Automatically deciding that a Session is complete from terminal text or a final response.
- Unattended switching, delegation, or cross-provider Session control.
- Automatically advancing an Execution Plan or Flow.
- Uploading coding context, source code, or provider conversation content to a remote
  Mudi service.
- An embedded terminal emulator.

## Evidence that would justify reopening the direction

These are validation hypotheses, not current launch criteria:

- A developer returning to known work can identify the next useful action in under one
  minute without reading the full transcript.
- A developer can change provider or start a Successor without manually rebuilding a
  handoff prompt from `git diff`, terminal history, and memory.
- Every displayed completion or verification claim is either linked to evidence or clearly
  labelled as an unverified statement.
- Repeat users checkpoint and resume work several times per week because the workflow saves
  time at natural interruption points.
- No externally started live process becomes controllable without a new, explicit control
  channel and authorization.

## Decisions needed before a future version is scoped

1. What is the minimum durable continuity object, and does it extend the existing Handoff
   Artifact or remain a rendered view over other evidence?
2. Which fields must be developer-authored, which may be Agent-proposed, and which are
   mechanically observed facts?
3. May Mudi generate the Agent-proposed portion from a provider conversation without
   retaining that conversation? If so, what exact authorization and privacy boundary applies?
4. What is the smallest checkpoint interaction that is fast enough for repeat use while
   keeping Handoff, resume, and provider launch explicit?
5. What evidence is sufficient for a check: command invocation alone, exit code, captured
   output reference, repository revision, or a combination?
6. Where should a successor receive continuity material, given that providers have different
   mechanisms for loading context?
7. Does the product's first frequent-use promise center on individual continuation, or on a
   later Execution Plan / Flow that coordinates multiple Sessions?
8. Which of these decisions changes durable domain vocabulary and therefore requires a
   `CONTEXT.md` update and an ADR before implementation?

## Recommended next discussion order

1. Capture a concrete interrupted-work case that existing Issue, Git, Resume, and Handoff
   behavior cannot recover adequately.
2. Confirm that the gap is a durable continuity problem rather than failed-Session recovery
   or a Handoff usability problem.
3. Only then define the smallest evidence-backed object that closes the validated gap
   without storing prohibited content.
4. Amend a future version boundary explicitly before producing a PRD or implementation
   ticket.

## Related records

- [`feature/v2.0.0/README.md`](../../../../feature/v2.0.0/README.md) - accepted Managed
  Session Core scope baseline.
- [`feature/v2.0.1/README.md`](../../../../feature/v2.0.1/README.md) - accepted Work
  Continuity scope baseline.
- [`CONTEXT.md`](../../../../CONTEXT.md) - authoritative domain vocabulary and privacy rules.
- [`ADR 0009`](../../../adr/0009-private-tmux-for-managed-current-runs.md) - private tmux
  runtime boundary.
- [`ADR 0002`](../../../adr/0002-explicitly-authorized-control.md) - explicit control
  authorization.
- [`ADR 0007`](../../../adr/0007-link-successors-through-launched-process-evidence.md) - no
  guessed Successor linkage.

## Session Log

### 2026-08-28

- Deferred Checkpoint after reviewing the completed v2.0.1 system and finding no active or
  demonstrated task that needs a new durable continuity object.
- Kept the earlier design exploration as historical evidence rather than converting it into
  a PRD or implementation ticket.
- Required a concrete gap beyond Issue, Git, Resume, and Handoff before the capability can
  re-enter a future version boundary.
