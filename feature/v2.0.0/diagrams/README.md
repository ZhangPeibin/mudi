# v2.0.0 architecture diagrams

The `v2.0.0` design is split into five editable Excalidraw diagrams. Each diagram has one
job so the product boundary, internal architecture, runtime behavior, and deferred layers
remain readable independently.

1. [`01-system-context.excalidraw`](./01-system-context.excalidraw) - developer surfaces,
   Mudi control plane, private tmux execution, Providers, and evidence boundaries.
2. [`02-logical-components.excalidraw`](./02-logical-components.excalidraw) - interaction,
   application, adapter, and local-system responsibilities and dependencies.
3. [`03-managed-session-lifecycle.excalidraw`](./03-managed-session-lifecycle.excalidraw) -
   create, bind, attach, TUI exit, restart reconciliation, and precise resume sequence.
4. [`04-capability-boundary.excalidraw`](./04-capability-boundary.excalidraw) - included
   capabilities, invariants, deferred work, and the later orchestration layers.
5. [`05-managed-start-ui.excalidraw`](./05-managed-start-ui.excalidraw) - the TUI's
   Managed Start status strip, verified Project Tree, Terminal.app launch path, and the
   Binding Receipt boundary between a pending Registration and a Managed Session. The global
   `n` shortcut starts creation from the selected Tree context, or opens directory selection
   when no Project/Worktree is selected.

In diagram 05, the Project Tree uses a solid green square for a Mudi launch channel
and a gray diamond for an external channel. The marker shape, as well as its color,
communicates management provenance.

Open the `.excalidraw` files directly in Excalidraw. Domain terms follow `CONTEXT.md`; the
scope boundary follows this version's feature record and ADR 0009.
