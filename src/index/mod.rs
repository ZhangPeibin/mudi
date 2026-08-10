//! Disposable, incremental SQLite index.
//!
//! The index is a cache over the source JSONL logs, not an authoritative store. It can
//! be deleted and rebuilt from source at any time. Only usage *metadata* is stored —
//! never prompts, summaries, tool arguments/outputs, or file content.
//!
//! Incrementality: each source file's `(mtime, size)` fingerprint is stored. Unchanged
//! files are skipped (no re-parse). Files that vanish are reconciled away on prune.

use crate::domain::{Compaction, ContextState, Provider, Usage};
use crate::providers::ParsedSession;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One indexed session plus the source path it came from.
#[derive(Debug, Clone)]
pub struct StoredSession {
    pub parsed: ParsedSession,
    pub source_path: PathBuf,
}

/// Source-file fingerprint used to detect changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub mtime_ns: i64,
    pub size: i64,
}

pub struct Index {
    conn: Connection,
}

impl Index {
    /// Open (creating if needed) the index at `path`, ensuring the schema exists.
    pub fn open(path: &Path) -> Result<Index> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Index { conn })
    }

    /// Open an in-memory index (used by tests).
    pub fn open_in_memory() -> Result<Index> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Index { conn })
    }

    fn init(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS sessions (
                provider        TEXT NOT NULL,
                id              TEXT NOT NULL,
                source_path     TEXT NOT NULL UNIQUE,
                source_mtime_ns INTEGER NOT NULL,
                source_size     INTEGER NOT NULL,
                cwd             TEXT,
                latest_model    TEXT,
                context_tokens  INTEGER NOT NULL,
                context_window  INTEGER,
                in_tokens       INTEGER NOT NULL,
                out_tokens      INTEGER NOT NULL,
                cache_read      INTEGER NOT NULL,
                cache_write     INTEGER NOT NULL,
                last_activity_ms INTEGER NOT NULL,
                PRIMARY KEY (provider, id)
            );

            CREATE TABLE IF NOT EXISTS per_model (
                provider   TEXT NOT NULL,
                session_id TEXT NOT NULL,
                model      TEXT NOT NULL,
                in_tokens  INTEGER NOT NULL,
                out_tokens INTEGER NOT NULL,
                cache_read INTEGER NOT NULL,
                cache_write INTEGER NOT NULL,
                seq        INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS compactions (
                provider   TEXT NOT NULL,
                session_id TEXT NOT NULL,
                at_ms      INTEGER NOT NULL,
                trigger    TEXT,
                pre_tokens INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_per_model ON per_model(provider, session_id);
            CREATE INDEX IF NOT EXISTS idx_compactions ON compactions(provider, session_id);
            "#,
        )?;

        // The index is disposable and the column is harmless on existing files,
        // but drop it when present so the persisted schema matches the pure usage
        // cache model. Fresh schemas never create it.
        let has_codex_open_turn = conn
            .prepare("PRAGMA table_info(sessions)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|name| name == "codex_open_turn");
        if has_codex_open_turn {
            conn.execute("ALTER TABLE sessions DROP COLUMN codex_open_turn", [])?;
        }
        Ok(())
    }

    /// Fingerprint of a previously-indexed source file, if present.
    pub fn fingerprint(&self, source_path: &Path) -> Option<Fingerprint> {
        self.fingerprint_checked(source_path).ok().flatten()
    }

    /// Fallible form of [`Self::fingerprint`] for refresh callers that must not
    /// hide an unavailable disposable index behind an empty projection.
    pub fn fingerprint_checked(&self, source_path: &Path) -> Result<Option<Fingerprint>> {
        self.conn
            .query_row(
                "SELECT source_mtime_ns, source_size FROM sessions WHERE source_path = ?1",
                params![source_path.to_string_lossy()],
                |row| {
                    Ok(Fingerprint {
                        mtime_ns: row.get(0)?,
                        size: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Insert or replace a parsed session and its child rows.
    pub fn upsert(
        &mut self,
        parsed: &ParsedSession,
        source_path: &Path,
        fp: Fingerprint,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let provider = parsed.provider.as_str();
        let sp = source_path.to_string_lossy().to_string();

        // Clear any prior child rows for this session, then the session row.
        tx.execute(
            "DELETE FROM per_model WHERE provider = ?1 AND session_id = ?2",
            params![provider, parsed.id],
        )?;
        tx.execute(
            "DELETE FROM compactions WHERE provider = ?1 AND session_id = ?2",
            params![provider, parsed.id],
        )?;

        tx.execute(
            r#"INSERT INTO sessions
                (provider, id, source_path, source_mtime_ns, source_size, cwd, latest_model,
                 context_tokens, context_window, in_tokens, out_tokens, cache_read, cache_write,
                 last_activity_ms)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
               ON CONFLICT(provider, id) DO UPDATE SET
                 source_path=excluded.source_path,
                 source_mtime_ns=excluded.source_mtime_ns,
                 source_size=excluded.source_size,
                 cwd=excluded.cwd,
                 latest_model=excluded.latest_model,
                 context_tokens=excluded.context_tokens,
                 context_window=excluded.context_window,
                 in_tokens=excluded.in_tokens,
                 out_tokens=excluded.out_tokens,
                 cache_read=excluded.cache_read,
                 cache_write=excluded.cache_write,
                 last_activity_ms=excluded.last_activity_ms"#,
            params![
                provider,
                parsed.id,
                sp,
                fp.mtime_ns,
                fp.size,
                parsed.cwd,
                parsed.latest_model,
                parsed.context.tokens as i64,
                parsed.context.window.map(|w| w as i64),
                parsed.usage.input as i64,
                parsed.usage.output as i64,
                parsed.usage.cache_read as i64,
                parsed.usage.cache_write as i64,
                parsed.last_activity_ms,
            ],
        )?;

        for (seq, (model, u)) in parsed.per_model.iter().enumerate() {
            tx.execute(
                "INSERT INTO per_model (provider, session_id, model, in_tokens, out_tokens, cache_read, cache_write, seq)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![provider, parsed.id, model, u.input as i64, u.output as i64, u.cache_read as i64, u.cache_write as i64, seq as i64],
            )?;
        }
        for c in &parsed.compactions {
            tx.execute(
                "INSERT INTO compactions (provider, session_id, at_ms, trigger, pre_tokens) VALUES (?1,?2,?3,?4,?5)",
                params![provider, parsed.id, c.at_ms, c.trigger, c.pre_tokens.map(|t| t as i64)],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Delete indexed sessions whose source file is no longer present.
    /// Returns the number of sessions pruned.
    pub fn prune_missing(&mut self, present: &HashSet<PathBuf>) -> Result<usize> {
        let indexed: Vec<(String, String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT provider, id, source_path FROM sessions")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut pruned = 0;
        let tx = self.conn.transaction()?;
        for (provider, id, sp) in indexed {
            if !present.contains(Path::new(&sp)) {
                tx.execute(
                    "DELETE FROM sessions WHERE provider=?1 AND id=?2",
                    params![provider, id],
                )?;
                tx.execute(
                    "DELETE FROM per_model WHERE provider=?1 AND session_id=?2",
                    params![provider, id],
                )?;
                tx.execute(
                    "DELETE FROM compactions WHERE provider=?1 AND session_id=?2",
                    params![provider, id],
                )?;
                pruned += 1;
            }
        }
        tx.commit()?;
        Ok(pruned)
    }

    /// Load every stored session with its child rows.
    pub fn load_all(&self) -> Result<Vec<StoredSession>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, id, source_path, cwd, latest_model, context_tokens, context_window,
                    in_tokens, out_tokens, cache_read, cache_write, last_activity_ms
             FROM sessions",
        )?;
        let base: Vec<StoredSession> = stmt
            .query_map([], |row| {
                let provider =
                    Provider::parse(&row.get::<_, String>(0)?).unwrap_or(Provider::ClaudeCode);
                let id: String = row.get(1)?;
                let source_path = PathBuf::from(row.get::<_, String>(2)?);
                let cwd: Option<String> = row.get(3)?;
                let latest_model: Option<String> = row.get(4)?;
                let context = ContextState {
                    tokens: row.get::<_, i64>(5)? as u64,
                    window: row.get::<_, Option<i64>>(6)?.map(|w| w as u64),
                };
                let usage = Usage {
                    input: row.get::<_, i64>(7)? as u64,
                    output: row.get::<_, i64>(8)? as u64,
                    cache_read: row.get::<_, i64>(9)? as u64,
                    cache_write: row.get::<_, i64>(10)? as u64,
                };
                let last_activity_ms: i64 = row.get(11)?;
                Ok(StoredSession {
                    source_path,
                    parsed: ParsedSession {
                        provider,
                        id,
                        cwd,
                        latest_model,
                        context,
                        usage,
                        per_model: Vec::new(),
                        compactions: Vec::new(),
                        last_activity_ms,
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Attach child rows.
        let mut out = Vec::with_capacity(base.len());
        for mut s in base {
            let provider = s.parsed.provider.as_str();
            let id = s.parsed.id.clone();

            let mut pm = self.conn.prepare(
                "SELECT model, in_tokens, out_tokens, cache_read, cache_write FROM per_model
                 WHERE provider=?1 AND session_id=?2 ORDER BY seq",
            )?;
            s.parsed.per_model = pm
                .query_map(params![provider, id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        Usage {
                            input: r.get::<_, i64>(1)? as u64,
                            output: r.get::<_, i64>(2)? as u64,
                            cache_read: r.get::<_, i64>(3)? as u64,
                            cache_write: r.get::<_, i64>(4)? as u64,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut cp = self.conn.prepare(
                "SELECT at_ms, trigger, pre_tokens FROM compactions
                 WHERE provider=?1 AND session_id=?2 ORDER BY at_ms",
            )?;
            s.parsed.compactions = cp
                .query_map(params![provider, id], |r| {
                    Ok(Compaction {
                        at_ms: r.get(0)?,
                        trigger: r.get(1)?,
                        pre_tokens: r.get::<_, Option<i64>>(2)?.map(|t| t as u64),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            out.push(s);
        }
        Ok(out)
    }
}

/// Fingerprint a source file from its filesystem metadata.
pub fn fingerprint_of(path: &Path) -> Option<Fingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Some(Fingerprint {
        mtime_ns,
        size: meta.len() as i64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Provider;

    fn sample(id: &str) -> ParsedSession {
        ParsedSession {
            provider: Provider::ClaudeCode,
            id: id.to_string(),
            cwd: Some("/tmp/proj".into()),
            latest_model: Some("claude-opus-4-8".into()),
            context: ContextState {
                tokens: 4500,
                window: Some(200_000),
            },
            usage: Usage {
                input: 100,
                output: 40,
                cache_read: 50,
                cache_write: 10,
            },
            per_model: vec![(
                "claude-opus-4-8".into(),
                Usage {
                    input: 100,
                    output: 40,
                    cache_read: 50,
                    cache_write: 10,
                },
            )],
            compactions: vec![Compaction {
                at_ms: 123,
                trigger: Some("auto".into()),
                pre_tokens: Some(150000),
            }],
            last_activity_ms: 999,
        }
    }

    #[test]
    fn upsert_then_load_roundtrips() {
        let mut idx = Index::open_in_memory().unwrap();
        let path = PathBuf::from("/logs/a.jsonl");
        let fp = Fingerprint {
            mtime_ns: 1,
            size: 2,
        };
        idx.upsert(&sample("s1"), &path, fp).unwrap();

        let all = idx.load_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].parsed.id, "s1");
        assert_eq!(all[0].parsed.usage.output, 40);
        assert_eq!(all[0].parsed.per_model.len(), 1);
        assert_eq!(all[0].parsed.compactions.len(), 1);
        assert_eq!(idx.fingerprint(&path), Some(fp));
    }

    #[test]
    fn opening_an_old_index_drops_the_provider_specific_state_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                provider TEXT NOT NULL,
                id TEXT NOT NULL,
                codex_open_turn INTEGER,
                PRIMARY KEY (provider, id)
            );",
        )
        .unwrap();

        Index::init(&conn).unwrap();

        let names: Vec<String> = conn
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(!names.iter().any(|name| name == "codex_open_turn"));
    }

    #[test]
    fn reupsert_replaces_child_rows_no_duplication() {
        let mut idx = Index::open_in_memory().unwrap();
        let path = PathBuf::from("/logs/a.jsonl");
        idx.upsert(
            &sample("s1"),
            &path,
            Fingerprint {
                mtime_ns: 1,
                size: 2,
            },
        )
        .unwrap();
        idx.upsert(
            &sample("s1"),
            &path,
            Fingerprint {
                mtime_ns: 9,
                size: 9,
            },
        )
        .unwrap();
        let all = idx.load_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(
            all[0].parsed.per_model.len(),
            1,
            "child rows replaced, not appended"
        );
        assert_eq!(all[0].parsed.compactions.len(), 1);
        assert_eq!(idx.fingerprint(&path).unwrap().mtime_ns, 9);
    }

    #[test]
    fn prune_removes_deleted_sources() {
        let mut idx = Index::open_in_memory().unwrap();
        idx.upsert(
            &sample("s1"),
            Path::new("/logs/a.jsonl"),
            Fingerprint {
                mtime_ns: 1,
                size: 2,
            },
        )
        .unwrap();
        idx.upsert(
            &sample("s2"),
            Path::new("/logs/b.jsonl"),
            Fingerprint {
                mtime_ns: 1,
                size: 2,
            },
        )
        .unwrap();

        let mut present = HashSet::new();
        present.insert(PathBuf::from("/logs/a.jsonl"));
        let pruned = idx.prune_missing(&present).unwrap();
        assert_eq!(pruned, 1);
        let all = idx.load_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].parsed.id, "s1");
    }
}
