---
name: sessionmesh-handoff
description: Deprecated compatibility entry point for generating a Mudi Handoff Artifact candidate.
---
<!-- sessionmesh-managed-skill: sessionmesh-handoff -->
<!-- sessionmesh-managed-version: 1 -->

Generate a Handoff Artifact for the resumed source Session.

Return only UTF-8 Markdown through standard output. Do not add a preamble or a
code fence. Use these headings exactly once and in this exact order:

```markdown
# Handoff
## Objective
## Current State
## Decisions
## Completed
## Remaining
## Files Changed
## Verification
## Risks And Blockers
## Next Action
```

Include only working context needed by a Successor Session. `Objective`,
`Current State`, `Remaining`, and `Next Action` must contain non-whitespace
content. Cover decisions, completed work, changed files, existing verification
evidence, risks and blockers when that context exists.

Do not call tools, edit or create files, install dependencies, commit, launch
tools or agents, or run tests. Do not include a complete Transcript, secrets,
or unrelated tool output. Mudi is the only Handoff Artifact writer.
