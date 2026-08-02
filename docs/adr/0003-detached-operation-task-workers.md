# ADR 0003 - Execute Operation Tasks in detached local workers

Operation Tasks must continue after the SessionMesh TUI exits, so they cannot be owned by
the TUI's Tokio runtime. Each authorized task will be recorded in a separate authoritative
task store and executed by a detached invocation of the SessionMesh binary; that worker
uses the provider's non-interactive CLI, sends generated instructions through stdin,
verifies the declared result, records only task metadata, and delivers completion
notifications. The TUI starts work through a small task-execution interface and reads task
state without knowing process, provider-command, or recovery details. This deliberately
rejects in-process tasks, terminal keystroke injection, and a required `tmux` dependency;
a long-lived local daemon may later replace per-task workers behind the same interface when
central scheduling justifies its installation and IPC complexity.

A restarted TUI reconciles a non-terminal task against validated Worker/Provider liveness
and its declared result. A live process remains Running and a valid completed result becomes
Ready; any outcome that cannot be established becomes Interrupted. SessionMesh never
automatically resubmits an Interrupted task because the previous Prompt may already have
produced side effects.
