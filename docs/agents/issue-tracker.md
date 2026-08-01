# Issue tracker: GitHub

Issues and PRDs live in GitHub Issues for `ZhangPeibin/SessionMesh`.
Use the `gh` CLI for all operations.

## Conventions

- Create: `gh issue create --title "..." --body "..."`
- Read: `gh issue view <number> --comments`
- List: `gh issue list --state open`
- Comment: `gh issue comment <number> --body "..."`
- Label: `gh issue edit <number> --add-label "..."`
- Close: `gh issue close <number> --comment "..."`

Infer the repository from `git remote -v`.

## Pull requests as a triage surface

PRs as a request surface: no.

## Skill terminology

- "Publish to the issue tracker" means create a GitHub issue.
- "Fetch the relevant ticket" means read it with `gh issue view`.
