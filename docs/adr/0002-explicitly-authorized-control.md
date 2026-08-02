# ADR 0002 - Require explicit authorization for session control

SessionMesh began as a strictly read-only observer, but assisted Handoff and future
session-management features require state-changing local actions. We will preserve passive
observation as the default and permit a bounded Control Operation only after the user
selects its target and explicitly authorizes it; an alert, context threshold, or passage of
time is never sufficient authority. This deliberately expands the product from a read-only
observer into an observer with opt-in local orchestration, without granting unattended or
blanket control over provider sessions.
