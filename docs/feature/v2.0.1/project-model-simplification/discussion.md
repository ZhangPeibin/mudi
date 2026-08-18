# Project Model Simplification - Requirements Discussion

**Status:** ready-for-prd
**Release:** v2.0.1
**Feature:** project-model-simplification
**Scope status:** This is a new v2.0.1 iteration requirement. The accepted
`feature/v2.0.1/README.md` boundary must be amended by the later specification and
delivery work; this discussion does not change the v2.0.0 baseline.

## Problem

Before this simplification, Mudi modeled both Project and Worktree even though no product
requirement needed Git worktree-aware behavior. The distinction spread Git common-directory identity,
Worktree roots, branch and writability metadata, nested Project Tree structure, Binding
validation, risk signals, persistence, tests, and documentation across the system.

That extra identity also caused a real launch failure. A directory was cached as a non-Git
Project before it became a Git repository. During Binding, Mudi derived a different Project
from the Git common directory even though the Provider cwd and Launch Location were the same
directory. The Registration reached `Needs Review` because two derived grouping identities
disagreed, not because the Session or process evidence was unsafe.

## Users and Outcomes

This affects developers launching, viewing, and resuming Sessions in Git repositories and
ordinary directories.

After this change:

- the Project Tree has one understandable grouping level: `Project -> Session`;
- a Git repository is grouped by its canonical Git top-level directory;
- an ordinary directory is grouped by the Session's canonical working directory;
- Session identity and Managed Binding do not depend on a derived Project snapshot;
- filesystem Git-state changes may regroup Sessions without moving Provider data or
  changing the Session identity.

## Pre-change Experience

For Git directories, Project identity is based on the canonical Git common directory while
Worktree identity is based on the canonical Git top-level. Non-Git directories use their
canonical cwd as Project identity. Launch Registration captures both Project and optional
Worktree information, and Binding recalculates and compares them.

The Project Tree and safety model therefore expose concepts such as Worktree, Shared
Worktree Risk, branch, and writable state despite the absence of linked-worktree workflows.
The same Session cwd can be interpreted differently when its directory changes between
observation, launch, and Binding.

## Scope

### In Scope

- Remove Worktree completely from the current domain model, application behavior, adapters,
  persistence, presentation, v2.0.1 feature records and discussions, current diagrams,
  tests, and fixtures.
- Remove Worktree roots, Git common-directory Project identity, branch metadata, and
  `writable` metadata without replacement compatibility fields.
- Define a Project solely as a directory grouping dynamically derived from a Session's
  working directory.
- Resolve any directory inside a Git repository to that repository's canonical Git
  top-level directory.
- Resolve a non-Git directory to its canonical working directory.
- Treat a linked Git worktree's own Git top-level as an independent Project if one is
  encountered, without linked-worktree-specific behavior or common-directory grouping.
- Persist `working_directory` where durable launch or Session metadata requires location;
  do not persist a derived Project ID or label.
- Remove Project validation from Managed Binding. Binding continues to validate Provider,
  exact process, Provider source, native Session ID, and canonical Provider cwd equal to
  canonical Launch Location.
- Change the Project Tree hierarchy to `Project -> Session`.
- Define `New` Location defaults from the current selection while keeping Location editable.
- Replace Shared Worktree Risk with Shared Project Risk for all Project types.
- Derive Project membership again as filesystem Git state changes, including after restart
  or refresh.
- Preserve a Session whose recorded working directory no longer exists by grouping it under
  the recorded absolute cwd path.
- Add and update coverage for Git, non-Git, changing Git state, missing directories, launch,
  Binding, tree rendering, and risk behavior.

### Out of Scope

- Product support for `git worktree`, linked-worktree relationships, Git common-directory
  identity, or coordination between related working copies.
- Branch-aware behavior or writability policy.
- Moving Provider JSONL files into a Project directory or changing their Provider-owned
  storage layout.
- Making Location a durable domain entity distinct from working directory.
- Manual Project creation, renaming, pinning, or membership assignment.
- Migration, compatibility reads, or automatic cleanup for pre-change development data.

## Required Behaviour

1. Every observed Session retains its Provider-native identity and exact working directory.
2. On refresh, Mudi derives the Session's Project from that working directory:
   - inside a Git repository: canonical Git top-level;
   - otherwise: canonical cwd.
3. Project identity is the resulting directory path. It is never `.git`, a Git common
   directory, a remote URL, or a persisted label.
4. A Session may dynamically move between Project groups when the filesystem's Git state
   changes. Its native Session ID, Provider JSONL, Title, and exact cwd remain unchanged.
5. If the recorded cwd is deleted or temporarily inaccessible, Mudi keeps the Session and
   uses the recorded absolute cwd as the fallback Project grouping path.
6. The Project Tree renders only `Project -> Session`; no Worktree row or Worktree-aware
   selection remains.
7. Pressing `n` with a Project selected defaults Location to the Project root. Pressing `n`
   with a Session selected defaults Location to that Session's exact cwd. The user may edit
   Location before confirming `New`.
8. Mudi launches the Provider through `tmux new-session -c <Location>`. The tmux `-c` option
   establishes the Provider process working directory; it is not a Mudi CLI option.
9. A Managed Launch Registration persists the Launch Location/working directory but no
   Project or Worktree snapshot.
10. Managed Binding compares the canonical Provider cwd with the canonical Launch Location
    and validates the existing Provider, source, process, and native Session ID evidence. It
    neither calculates nor validates Project membership.
11. Shared Project Risk is present when at least two distinct Open Sessions resolve to the
    same Project. It applies equally to Git and non-Git Projects and retains Warning severity.
12. All Worktree vocabulary, fields, dormant abstractions, compatibility shims, and legacy
    UI or persistence paths are deleted rather than deprecated.

## Edge and Failure Cases

- A normal directory becomes a Git repository after Sessions already exist: the Sessions
  regroup under the canonical Git top-level on the next derivation.
- A Git repository becomes a normal directory: Sessions regroup according to their exact
  recorded working directories.
- A Session runs at `/repo/packages/app` inside a Git repository rooted at `/repo`: cwd
  remains `/repo/packages/app`, while Project is `/repo`.
- A Session runs at `/repo/packages/app` when that path is not inside a Git repository:
  both cwd and Project are `/repo/packages/app`.
- A linked worktree is encountered despite being outside supported product workflows: its
  own canonical Git top-level becomes an independent Project.
- A working directory disappears but its Provider source remains: the Session remains
  visible under the recorded absolute cwd path.
- Multiple Open Sessions share a non-Git directory: Shared Project Risk applies exactly as
  it does for a Git Project.

## Constraints and Dependencies

- Provider JSONL remains the source of truth for observed Session data and stays in the
  Provider-owned storage location. It records cwd; it is not relocated into the Project.
- Session identity remains `(Session Provider, native Session ID)` and never depends on
  Project membership.
- Canonical launched-process and source evidence remain mandatory for Managed Binding.
- The Session Title lifecycle decoupling requirement should land first so Project grouping
  cannot suppress an already associated Title.
- `CONTEXT.md`, the v2.0.1 scope baseline, relevant v2.0.1 discussions, Shared Worktree Risk
  terminology, and affected current diagrams must be revised as part of delivery.
- The accepted v2.0.0 feature record, discussion, and diagrams remain historical records of
  that closed baseline. The new requirement is not backported or rewritten into v2.0.0.
- Accepted ADRs about explicit authorization, private tmux, and conclusive process evidence
  remain in force. No accepted ADR requires Git common-directory or Worktree identity.

## Decisions

- Mudi has only Project; Worktree is removed completely because no current product workflow
  requires it.
- Git Project identity is canonical Git top-level, never `.git` or Git common directory.
- Non-Git Project identity is canonical cwd.
- Linked worktrees receive no special support and are treated as independent Projects.
- Project is dynamically derived grouping data, not part of Session identity or Binding
  authority.
- Only working directory is persisted; Project ID and label are derived.
- `New` defaults Location from Project or Session selection, and Location remains editable.
- Shared Project Risk replaces Shared Worktree Risk and applies to Git and non-Git Projects.
- `branch` and `writable` are deleted without replacements.
- No old-data migration, compatibility reader, legacy field, or automatic cleanup is
  provided during the development stage.
- The change is a v2.0.1 iteration even though it removes concepts introduced in v2.0.0.

## Open Questions

None.

## Pre-change Evidence

- At discussion time, `CONTEXT.md` defined Project by Git common directory and separately
  defined Worktree by Git top-level.
- `feature/v2.0.0/README.md` requires Project/Worktree selection and conclusive
  Project/Worktree Binding; this remains historical v2.0.0 scope rather than the release
  assignment for this change.
- At discussion time, `feature/v2.0.1/README.md` built its `New` and continuity workflows on
  the prior grouping model and required amendment for the new v2.0.1 iteration.
- At discussion time, `src/application/observation/collector.rs` cached
  directory-to-Project resolution.
- At discussion time, `src/application/managed_sessions.rs` persisted derived grouping
  launch targets and rejected Binding when recalculated values differed.
- At discussion time, `src/adapters/project.rs` resolved the prior Git identities.
- At discussion time, `src/presentation/tui/project_tree` implemented the nested grouping
  and selection behavior.
- At discussion time, attention and signal code, persistence schemas, and broad integration
  coverage contained the prior grouping vocabulary and behavior.

## Session Log

### 2026-08-18

- Traced missing Titles back to a cached non-Git Project identity disagreeing with a later
  Git common-directory identity during Managed Binding, despite an exact cwd match.
- Rejected Git common-directory and Worktree identity because the product has no current
  linked-worktree requirements.
- Confirmed Git top-level and exact non-Git cwd Project resolution, dynamic regrouping,
  cwd-only persistence, and Project-independent Binding.
- Confirmed Project Tree, `New` Location defaults, editable Location, Shared Project Risk,
  deleted-directory fallback, and complete removal of `branch` and `writable`.
- Confirmed complete deletion of Worktree from current code, UI, persistence, tests, domain
  docs, and v2.0.1 records and diagrams with no dormant runtime abstraction or compatibility
  layer. The closed v2.0.0 baseline remains historical documentation.
- Confirmed that no migration or cleanup is required for old development data.
- Confirmed the complete launch, identity, Title, Binding, and Project grouping flow as the
  shared understanding.
- Assigned the requirement to v2.0.1. The user clarified the release policy: v2.0.0 is a
  closed baseline, and all later requirement changes are iterations in v2.0.1 even when they
  revise a v2.0.0 design.
- The user explicitly confirmed that the analysis is ready for a formal PRD.
