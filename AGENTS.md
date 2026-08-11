## Agent skills

### Issue tracker

Issues and PRDs are tracked in GitHub Issues for `ZhangPeibin/SessionMesh`.
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
