# Verify TokenPlay

TokenPlay is a Rust terminal UI with two non-interactive examples that exercise the real local logs without requiring a TTY.

## Setup

Homebrew Rust may need:

```sh
export PATH="/opt/homebrew/bin:$PATH"
```

Use a fresh disposable index for each run:

```sh
TOKENPLAY_DATA_DIR=$(mktemp -d) cargo run --quiet --example scan
TOKENPLAY_DATA_DIR=$(mktemp -d) cargo run --quiet --example render
```

## What to inspect

- `scan`: first and second scan statistics prove the incremental path; provider sections show Busy/Idle/Exited and context percentages against real logs.
- `render`: off-screen Ratatui frames for All, Claude, and Codex tabs. Capture stdout and grep the relevant rows.
- For an old-schema migration, create `index.db` with the prior `codex_open_turn` column, point `TOKENPLAY_DATA_DIR` to its directory, run `--example scan`, then inspect `pragma_table_info('sessions')` with `sqlite3`.

The interactive surface is `cargo run --release` in a real terminal when keyboard navigation itself changed.
