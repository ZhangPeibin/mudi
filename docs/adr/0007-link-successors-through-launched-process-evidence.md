# ADR 0007 - Link successors through launched-process evidence

SessionMesh must associate a Handoff Task with the Successor Session that actually starts,
but Claude can accept a preassigned Session ID while Codex cannot, and selecting the newest
Session in a working directory is ambiguous under concurrency. Both Providers will instead
use one foreground launch helper and one linkage interface: the relevant Provider adapter
maps the validated launched process to its native source record, using Claude's PID state
record or Codex's open rollout file, and only then persists the Successor Session ID.

This deliberately rejects preassigning Claude IDs, guessing by timestamp or working
directory, and treating a launch request as proof that a Session exists. Failure to obtain
conclusive linkage leaves the successful Handoff Result Ready and unlinked rather than
inventing a relationship.
