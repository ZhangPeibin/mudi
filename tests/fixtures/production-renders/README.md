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
| `all_populated_120x30.txt` | Full All page with side-by-side PROJECT and populated RECENT SIGNALS |
| `all_populated_80x24.txt` | Stacked compact All page with Critical, Shared Worktree Risk, Warning, Idle, and healthy Busy states |
| `all_empty_signals_80x24.txt` | Compact All page with the empty Signals state |
| `claude_120x30.txt` | Full Claude provider inventory and complete lifetime Usage columns |
| `claude_older_expanded_80x24.txt` | Compact expanded per-Project older-session disclosure |
| `codex_80x24.txt` | Compact Codex provider inventory, including an Exited Session |
| `detail_open_120x30.txt` | Full-target open Session Detail with Attention, scoped Usage, source metadata, and Compactions |
| `detail_open_80x24.txt` | Compact open Session Detail before scrolling |
| `detail_open_scrolled_80x24.txt` | Compact open Session Detail remains stable when all Compaction rows already fit |
| `detail_exited_80x24.txt` | Compact exited Session Detail without Current Run or Last Run Usage |
| `help_80x24.txt` | Compact Help overlay and essential keyboard mapping |
| `copy_menu_80x24.txt` | Compact source-metadata Copy menu |
| `too_small_79x23.txt` | Terminal-too-small fallback with required/current dimensions and Quit only |

`production_renders.rs` also renders all six built-in Themes through the production
buffer, verifies semantic-role contrast, and checks that changing Theme does not change
the rendered product semantics.

When intentional UI behavior changes, review the complete affected buffer before editing
its fixture. Keep dimensions exact and do not replace these literals with auto-accepted
snapshots.
