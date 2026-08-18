# ADR 0005 - Keep Handoff Artifacts outside Projects

Handoff Artifacts may contain sensitive working context, and writing them into a Project
would dirty its working directory and risk accidental Git commits. Mudi will instead create
each Artifact under its private data directory, using a Task ID rather than user content in
the filename, with owner-only directory and file permissions. The authoritative Task Store
records only the Artifact path and result metadata; it never stores the document body.
Successor Launch Commands explicitly grant the Provider CLI access to the Artifact directory.
The Provider returns candidate Markdown without Artifact-directory write access; the Worker
validates it and atomically publishes the owner-only file. Exporting an Artifact into a
Project remains a separate, user-authorized operation.
