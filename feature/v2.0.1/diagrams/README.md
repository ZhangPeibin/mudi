# v2.0.1 architecture diagrams

The `v2.0.1` design is split into four editable Excalidraw diagrams. Each diagram has one
job so the product boundary, internal architecture, runtime behavior, and deferred layers
remain readable independently.

1. [`01-system-context.excalidraw`](./01-system-context.excalidraw) - developer surfaces,
   SessionMesh control plane, private tmux execution, Providers, and evidence boundaries.
2. [`02-logical-components.excalidraw`](./02-logical-components.excalidraw) - interaction,
   application, adapter, and local-system responsibilities and dependencies.
3. [`03-managed-session-lifecycle.excalidraw`](./03-managed-session-lifecycle.excalidraw) -
   create, bind, attach, TUI exit, restart reconciliation, and precise resume sequence.
4. [`04-capability-boundary.excalidraw`](./04-capability-boundary.excalidraw) - included
   capabilities, invariants, deferred work, and the later orchestration layers.

Open the `.excalidraw` files directly in Excalidraw. Domain terms follow `CONTEXT.md`; the
scope boundary follows this version's feature record and ADR 0009.
