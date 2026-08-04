# Handoff Artifact contract

A Handoff Artifact is UTF-8 Markdown with the following required headings in order:

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

Each section contains only the working context needed by a Successor Session. The Artifact
must not contain a complete Transcript, secrets, or unrelated tool output.

Before an Operation Task becomes Ready, its Worker verifies that the Artifact is valid
UTF-8, non-empty, and contains every required heading. `Objective`, `Current State`,
`Remaining`, and `Next Action` must each contain non-whitespace content. Validation happens
in memory; the Task Store records only result metadata and the Artifact path.

The Provider CLI returns candidate Markdown as its final response and never writes the
Artifact directly. The Worker is the sole writer: it validates the candidate, writes an
owner-only temporary file, and atomically renames that file to the declared Artifact path.
Invalid or interrupted output never appears as a completed Artifact.

Artifact generation is read-only with respect to the source Project and Worktree. The managed
Skill uses the resumed Session context and does not invoke tools. It must not edit project files,
install dependencies, commit changes, launch tools or agents, or run new tests.
