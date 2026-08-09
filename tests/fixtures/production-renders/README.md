# Production Render Fixtures

These files are reviewed golden buffers from the production Ratatui UI. The integration
tests exercise only the public `ui::draw` and `Ui::handle_key` interfaces with a fixed
`Snapshot`; the expected buffers are maintained as independent literals and are not
automatically regenerated.

Run the complete render verification with:

```sh
cargo test --test production_renders
```

## Scenarios

| Fixture | Production state covered |
|---|---|
| `all_populated_120x30.txt` | All-page Session editor with a selected Session driving the tree, compact inspector, and unavailable workspace |
| `all_populated_80x24.txt` | Compact Session editor below the side-by-side status strip |
| `all_empty_signals_80x24.txt` | Compact Session editor with the explicit empty Signals state |
| `claude_120x30.txt` | Full Claude provider inventory and complete lifetime Usage columns |
| `claude_older_expanded_80x24.txt` | Compact expanded per-Project older-session disclosure |
| `codex_80x24.txt` | Compact Codex provider inventory, including an Exited Session |
| `detail_open_120x30.txt` | Full-target open Session Detail with Attention, scoped Usage, source metadata, and Compactions |
| `detail_open_80x24.txt` | Compact open Session Detail before scrolling |
| `detail_open_scrolled_80x24.txt` | Compact open Session Detail remains stable when all Compaction rows already fit |
| `detail_exited_80x24.txt` | Compact exited Session Detail without Current Run or Last Run Usage |
| `guide_80x24.txt` | Compact Guide modal with category navigation and Overview content |
| `too_small_79x23.txt` | Terminal-too-small fallback with required/current dimensions and Quit only |

`production_renders.rs` also renders all six built-in Themes through the production
buffer, verifies semantic-role contrast, and checks that changing Theme does not change
the rendered product semantics.

When intentional UI behavior changes, review the complete affected buffer before editing
its fixture. Keep dimensions exact and do not replace these literals with auto-accepted
snapshots.
