## Agent skills

### Issue tracker

Issues and PRDs are tracked in GitHub Issues for `ZhangPeibin/mudi`.
External PRs are not a triage surface. See `docs/agents/issue-tracker.md`.

### Triage labels

Use the five canonical triage labels without aliases.
See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository. Read root `CONTEXT.md` and relevant
ADRs under `docs/adr/`. See `docs/agents/domain.md`.

### Feature docs

Versioned feature scope records live under `feature/vX.Y.Z/README.md`. Add each
future feature in a new version directory and update the version index in
`feature/README.md`. Feature records define an agreed scope baseline; they do not
replace GitHub Issue PRDs or architectural decisions under `docs/adr/`.

Pre-PRD requirements discussions live under
`docs/feature/<release>/<feature-slug>/discussion.md`.
See `docs/agents/feature-docs.md`.

### Requirement routing

Classify a requested change before editing requirement documents:

- Put a new version capability or a change to an agreed version boundary in the relevant
  `feature/vX.Y.Z/README.md`, and update `feature/README.md` when adding a version.
- Put routine UI/UX refinement, visual polish, wording, keybinding adjustment, and other
  incremental delivery work in a GitHub Issue with concrete acceptance criteria. These
  changes do not belong in a version feature record merely because they affect the UI.
- Update a PRD only when the change materially alters the product workflow, cross-feature
  behavior, safety/privacy boundary, or another durable product contract owned by that
  PRD. A PRD is not a changelog for day-to-day optimization.
- When routine work directly contradicts an explicit requirement in an active PRD, make
  only the smallest amendment needed to remove the contradiction and reference the GitHub
  Issue. Do not append a detailed optimization history or duplicate the Issue specification.
- Continue to use `CONTEXT.md` for shared domain vocabulary and `docs/adr/` for accepted
  architectural decisions; do not move those decisions into feature records or UI issues.

## Rust engineering

### Repository facts

- Mudi is one Rust 2021 package, not a Cargo workspace. It currently declares no Cargo
  feature matrix, pinned Rust toolchain, or MSRV. CI uses stable Rust on macOS; do not invent
  package names, supported configurations, or portability requirements.
- The requested outcome and accepted product contracts define intended behavior. Source,
  tests, Cargo manifests, schemas, and CI are evidence of current behavior and constraints;
  investigate and report conflicts instead of silently choosing one.
- Mudi is macOS-only. Process discovery, Terminal.app, tmux, Unix permissions, and filesystem
  behavior are supported platform contracts. Do not add unrequested speculative cross-platform
  code.

### Change workflow

1. Inspect the target code, callers, tests, `CONTEXT.md`, and relevant ADRs before editing.
2. Find the owning module and existing traits, errors, helpers, and fixtures.
3. Define the smallest coherent behavior change and an observable success criterion.
4. Edit the authoritative source without unrelated API, dependency, lockfile, or formatting
   churn.
5. Run the smallest relevant test first, then widen verification according to risk.
6. Inspect the complete task-owned diff and report checks run and relevant checks omitted.

- Do not expand public API, add a dependency or feature, or introduce an abstraction without
  a current requirement and consumer.
- Follow established error and dependency conventions. Preserve error sources and add context
  where a failure becomes actionable.
- Do not introduce panic-style handling for recoverable, persisted, provider-supplied, OS, or
  user-controlled conditions. A bug fix should add a regression test when the behavior can be
  exercised reliably.

### Module ownership

- `domain` owns provider-neutral vocabulary and state. It must not depend on `application`,
  `adapters`, or `presentation`.
- `adapters` owns provider, filesystem, process, macOS, and disposable-index integration.
  Provider-specific discovery, parsing, lifecycle, and launched-process evidence belong behind
  the provider registry; presentation types and policy do not.
- `application` owns workflows, authorization, reconciliation, workflow-owned durable stores,
  Operation Task behavior, and read-model construction. `presentation` owns TUI rendering and
  interaction, not domain or provider policy.
- Keep Session Provider separate from Model Provider. Exact model metadata belongs in
  `models/catalog.json`; unknown or ambiguous identifiers remain unknown rather than using a
  nearby model.

### Observation, persistence, and privacy

- Provider JSONL is authoritative for observed Session state; the Session SQLite index is
  disposable and rebuildable. Changes to discovery or parsing must handle appended, truncated,
  deleted, and malformed sources without crashing or inventing valid-looking data.
- The Task Store is authoritative only for authorized Operation Task metadata and result
  references. Handoff Artifact contents stay in owner-only files under Mudi's private data
  directory and never enter a metadata store.
- When changing schemas, stores, files, or result contracts, inspect existing readers, writers,
  migrations, permissions, and cleanup paths. Preserve committed compatibility, atomic publish
  behavior, `0700` directories, and `0600` private files; test existing-data and partial-failure
  paths that the change affects.
- Never persist prompt bodies, generated summaries, tool arguments, tool outputs, provider
  conversation content, or Project file contents in Mudi metadata stores.

### Control and process safety

- Passive observation, Alerts, thresholds, elapsed time, and discovered processes never grant
  control authority. Every state-changing Control Operation requires the explicit, bounded
  authorization defined by `CONTEXT.md` and ADR 0002.
- Managed control authority and launched-process-to-Session linkage require conclusive provider
  and live-process evidence. Validate process start identity against PID reuse; never guess from
  cwd, timestamps, newest files, tmux names, panes, or launch intent. Missing, stale, conflicting,
  or ambiguous evidence must remain unlinked or enter the defined review/interrupted path.
- Preserve `Path`/`OsStr` values and pass process arguments directly. Do not construct a shell
  command string unless the platform adapter explicitly requires shell semantics; then keep
  quoting and expansion behavior inside that adapter and test spaces, quotes, metacharacters,
  and Unicode paths.
- Cancellation, timeout, restart reconciliation, and partial failure must leave authoritative
  state valid and reconcilable. Never silently retry an Interrupted Operation Task, reopen a
  terminal Task, or infer success from a process exit or channel close alone.

### Async and worker lifecycles

- The TUI owns its Tokio runtime and in-process collector task. A changed or new Tokio task must
  have an explicit owner, shutdown behavior, and failure-observation strategy.
- Operation Tasks deliberately run in detached Mudi worker processes and must survive TUI exit
  under ADR 0003. Managed Current Runs deliberately live in private tmux environments under ADR
  0009. Do not pull either lifecycle into the TUI runtime or treat detachment as Session identity
  or control authority.
- For changed async paths, move potentially unbounded blocking work off runtime worker threads;
  do not hold a lock guard across `.await` unless the bounded critical section is deliberate and
  documented. Do not broaden the change into an unrelated rewrite of existing synchronous probes.
- Preserve required backpressure. A new or changed intentionally unbounded channel must explain
  why bounded capacity is unsuitable. Cancellation and receiver/task drop must leave shared,
  persisted, and externally visible state valid and reconcilable.

### Dependencies, unsafe code, and generated data

- Before adding or upgrading a dependency, inspect existing alternatives and macOS, build-time,
  binary-size, maintenance, and transitive-feature impact. Change `Cargo.toml` and `Cargo.lock`
  precisely; do not refresh unrelated lockfile entries.
- Prefer an established safe abstraction over new `unsafe`. Every changed unsafe block requires
  a `SAFETY:` comment that states the invariants, plus focused evidence for the affected aliasing,
  initialization, lifetime, thread-safety, panic, and FFI assumptions.
- Do not hand-edit generated artifacts when a canonical source and generator exist. Update the
  source, run the generator, and verify stable regeneration.

### Verification

- For a Rust behavior change, run the smallest focused test first. Before completion, run the CI
  equivalents from the repository root:

  ```bash
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo test
  ```

- For persistence, process, concurrency, macOS integration, or rendered TUI changes, add and run
  the focused integration or render test that exercises the affected contract; host-independent
  unit tests alone are insufficient evidence for a macOS boundary.
- For documentation-only changes, run only applicable documentation or formatting checks; Rust
  tests are not required unless the documentation changes an executable contract.
- Always run `git diff --check`, inspect status and the full task-owned diff, and report exact
  commands and results plus any relevant test, target, or integration path not exercised.
