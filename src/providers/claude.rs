//! Claude Code adapter.
//!
//! Transcripts: `~/.claude/projects/<encoded-project>/<session-id>.jsonl`.
//!
//! Key semantics:
//! - Current context = latest assistant message `input_tokens + cache_read_input_tokens`.
//!   When cache read is zero, cache creation is the prompt-cache contribution instead.
//!   Cache read and cache creation are never added together.
//! - Streaming writes multiple records per `message.id`; usage must be de-duplicated by
//!   id (last occurrence wins — it carries the final counts).
//! - Compaction = `type:system` / `subtype:compact_boundary`.

use super::{
    iso_to_ms, ObservationPort, ParsedSession, ParsedUsage, PerModel, ProviderExecutables,
    RuntimeControlPort, SkillTransportPort,
};
use crate::config::Config;
use crate::domain::{Compaction, ContextState, Provider, Usage};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The Claude Code provider adapter.
pub struct ClaudeAdapter;

impl ObservationPort for ClaudeAdapter {
    fn provider(&self) -> Provider {
        Provider::ClaudeCode
    }

    fn source_format(&self) -> crate::domain::SourceFormat {
        crate::domain::SourceFormat::JsonLines
    }

    fn discover(&self, cfg: &Config) -> Vec<PathBuf> {
        super::discover_files(&cfg.claude_project_roots(), |name| name.ends_with(".jsonl"))
    }

    fn parse_file(&self, path: &Path) -> Option<ParsedSession> {
        parse_file(path)
    }

    fn live_state(
        &self,
        probe: &super::state::StateDetector,
        source_session_id: &str,
        source_path: &Path,
    ) -> super::state::LiveState {
        self.observe_current_run(probe, source_session_id, source_path)
    }

    fn current_run_usage(&self, source_path: &Path, started_at_ms: i64) -> Option<ParsedUsage> {
        let text = std::fs::read_to_string(source_path).ok()?;
        Some(parse_current_run_str(&text, started_at_ms))
    }
}

impl RuntimeControlPort for ClaudeAdapter {
    fn observe_current_run(
        &self,
        probe: &super::state::StateDetector,
        source_session_id: &str,
        _source_path: &Path,
    ) -> super::state::LiveState {
        probe.claude_state(source_session_id)
    }

    fn handoff_resume_args(&self, native_session_id: &str) -> Vec<String> {
        [
            "--print",
            "--resume",
            native_session_id,
            "--bare",
            "--permission-mode",
            "plan",
            "--tools",
            "",
            "--strict-mcp-config",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn resolve_successor_session(
        &self,
        cfg: &Config,
        process: super::state::ProcessIdentity,
    ) -> Option<String> {
        resolve_successor_session(cfg, process)
    }

    fn successor_launch_arguments(&self, artifact_path: &Path) -> Vec<String> {
        successor_arguments("Claude Code", artifact_path)
    }

    fn executable<'a>(&self, executables: &'a ProviderExecutables) -> Option<&'a Path> {
        let path = executables.claude_path();
        super::is_executable(path).then_some(path)
    }
}

impl SkillTransportPort for ClaudeAdapter {
    fn handoff_skill_invocation(&self) -> &'static str {
        "/sessionmesh-handoff"
    }
}

fn successor_arguments(provider_name: &str, artifact_path: &Path) -> Vec<String> {
    let artifact_directory = artifact_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_string_lossy()
        .into_owned();
    vec![
        "--add-dir".into(),
        artifact_directory,
        format!(
            "Read the Handoff Artifact at {} and continue the work in this new {provider_name} Session.",
            artifact_path.to_string_lossy()
        ),
    ]
}

fn resolve_successor_session(
    cfg: &Config,
    process: super::state::ProcessIdentity,
) -> Option<String> {
    if !super::state::process_identity_is_live(process) {
        return None;
    }
    let mut session_id: Option<String> = None;
    for entry in std::fs::read_dir(cfg.claude_sessions_dir()).ok()?.flatten() {
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("json")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if value.get("pid").and_then(Value::as_u64) != Some(u64::from(process.pid)) {
            continue;
        }
        let recorded_start = value.get("startedAt").and_then(Value::as_i64)?;
        if (recorded_start - process.started_at_ms).abs() > 2_000 {
            continue;
        }
        let observed = value
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|session_id| !session_id.is_empty())?;
        if session_id
            .as_deref()
            .is_some_and(|current| current != observed)
        {
            return None;
        }
        session_id = Some(observed.to_string());
    }
    let session_id = session_id?;
    super::state::process_identity_is_live(process).then_some(session_id)
}

/// Parse a Claude transcript file. Returns `None` if the file is unreadable or yields
/// nothing usable. Malformed lines are skipped, never fatal.
pub fn parse_file(path: &Path) -> Option<ParsedSession> {
    let text = std::fs::read_to_string(path).ok()?;
    let id = path.file_stem()?.to_string_lossy().into_owned();
    parse_str(&id, &text)
}

/// Parse transcript content with a known session id. Split out for testing.
pub fn parse_str(id: &str, text: &str) -> Option<ParsedSession> {
    // De-duplicated assistant usage, keyed by message id (last write wins).
    struct Msg {
        usage: Usage,
        model: String,
        ts: i64,
        ctx_tokens: u64,
    }
    let mut msgs: HashMap<String, Msg> = HashMap::new();
    let mut cwd: Option<String> = None;
    let mut compactions: Vec<Compaction> = Vec::new();
    let mut last_activity_ms: i64 = 0;
    let mut anon_counter: u64 = 0;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // malformed line — skip
        };

        let ts = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(iso_to_ms);
        if let Some(t) = ts {
            last_activity_ms = last_activity_ms.max(t);
        }
        if cwd.is_none() {
            if let Some(c) = v.get("cwd").and_then(Value::as_str) {
                cwd = Some(c.to_string());
            }
        }

        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let Some(msg) = v.get("message") else {
                    continue;
                };
                let Some(usage_v) = msg.get("usage") else {
                    continue;
                };
                let model = msg
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();

                let input = u(usage_v, "input_tokens");
                let output = u(usage_v, "output_tokens");
                let cache_read = u(usage_v, "cache_read_input_tokens");
                let cache_write = u(usage_v, "cache_creation_input_tokens");

                let usage = Usage {
                    input,
                    output,
                    cache_read,
                    cache_write,
                };
                // A cache miss reports the prompt in cache creation instead of cache read.
                // Prefer cache read so records containing both categories never double-count.
                let cached_context = if cache_read == 0 {
                    cache_write
                } else {
                    cache_read
                };
                let ctx_tokens = input + cached_context;

                let key = msg
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        anon_counter += 1;
                        format!("__anon_{anon_counter}")
                    });

                // Last occurrence wins: streaming's final chunk has complete counts.
                msgs.insert(
                    key,
                    Msg {
                        usage,
                        model,
                        ts: ts.unwrap_or(0),
                        ctx_tokens,
                    },
                );
            }
            Some("system")
                if v.get("subtype").and_then(Value::as_str) == Some("compact_boundary") =>
            {
                let meta = v.get("compactMetadata");
                let trigger = meta
                    .and_then(|m| m.get("trigger"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let pre_tokens = meta
                    .and_then(|m| m.get("preTokens"))
                    .and_then(Value::as_u64);
                compactions.push(Compaction {
                    at_ms: ts.unwrap_or(last_activity_ms),
                    trigger,
                    pre_tokens,
                });
            }
            _ => {}
        }
    }

    if msgs.is_empty() && compactions.is_empty() {
        return None;
    }

    // Roll up totals and per-model split from de-duplicated messages.
    let mut total = Usage::default();
    let mut per_model = PerModel::default();
    let mut latest: Option<&Msg> = None;
    for m in msgs.values() {
        total.add(&m.usage);
        per_model.add(&m.model, &m.usage);
        latest = match latest {
            Some(cur) if cur.ts >= m.ts => Some(cur),
            _ => Some(m),
        };
    }

    let (context, latest_model) = match latest {
        Some(m) => (
            ContextState {
                tokens: m.ctx_tokens,
                // Claude JSONL does not report a window. The Model Catalog fills
                // it after indexing, using the exact Provider + model entry.
                window: None,
            },
            Some(m.model.clone()),
        ),
        None => (ContextState::default(), None),
    };

    Some(ParsedSession {
        provider: Provider::ClaudeCode,
        id: id.to_string(),
        cwd,
        latest_model,
        context,
        usage: total,
        per_model: per_model.into_vec(),
        compactions,
        last_activity_ms,
    })
}

/// Calculate Claude Usage from assistant messages at or after a validated
/// process-start boundary. Records without timestamps cannot be assigned to a Run.
pub fn parse_current_run_str(text: &str, started_at_ms: i64) -> ParsedUsage {
    struct Msg {
        usage: Usage,
        model: String,
    }

    let mut order = Vec::new();
    let mut msgs: HashMap<String, Msg> = HashMap::new();
    let mut anon_counter = 0_u64;

    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let Some(timestamp_ms) = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(iso_to_ms)
        else {
            continue;
        };
        if timestamp_ms < started_at_ms
            || v.get("type").and_then(Value::as_str) != Some("assistant")
        {
            continue;
        }
        let Some(message) = v.get("message") else {
            continue;
        };
        let Some(raw_usage) = message.get("usage") else {
            continue;
        };
        let key = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                anon_counter += 1;
                format!("__run_anon_{anon_counter}")
            });
        if !msgs.contains_key(&key) {
            order.push(key.clone());
        }
        msgs.insert(
            key,
            Msg {
                usage: Usage {
                    input: u(raw_usage, "input_tokens"),
                    output: u(raw_usage, "output_tokens"),
                    cache_read: u(raw_usage, "cache_read_input_tokens"),
                    cache_write: u(raw_usage, "cache_creation_input_tokens"),
                },
                model: message
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
            },
        );
    }

    let mut usage = Usage::default();
    let mut per_model = PerModel::default();
    for key in order {
        if let Some(message) = msgs.remove(&key) {
            usage.add(&message.usage);
            per_model.add(&message.model, &message.usage);
        }
    }

    ParsedUsage {
        usage,
        per_model: per_model.into_vec(),
    }
}

fn u(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicting_pid_state_records_do_not_identify_a_successor_session() {
        let process = super::super::state::process_identity(std::process::id()).unwrap();
        let root = std::env::temp_dir().join(format!(
            "sessionmesh-claude-link-conflict-{}-{}",
            process.pid,
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let state = |session_id: &str| {
            format!(
                "{{\"pid\":{},\"sessionId\":\"{session_id}\",\"status\":\"idle\",\"startedAt\":{}}}",
                process.pid, process.started_at_ms
            )
        };
        std::fs::write(
            sessions.join(format!("{}.json", process.pid)),
            state("successor-a"),
        )
        .unwrap();
        std::fs::write(sessions.join("conflict.json"), state("successor-b")).unwrap();
        let config = Config {
            claude_dir: root.clone(),
            codex_dir: root.join("codex"),
            data_dir: root.join("data"),
            extra_dirs: Vec::new(),
        };

        let resolved = ClaudeAdapter.resolve_successor_session(&config, process);

        assert_eq!(resolved, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_pid_state_record_does_not_identify_a_successor_session() {
        let process = super::super::state::process_identity(std::process::id()).unwrap();
        let root = std::env::temp_dir().join(format!(
            "sessionmesh-claude-link-stale-{}-{}",
            process.pid,
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("{}.json", process.pid)),
            format!(
                "{{\"pid\":{},\"sessionId\":\"stale-successor\",\"status\":\"idle\",\"startedAt\":{}}}",
                process.pid,
                process.started_at_ms - 60_000
            ),
        )
        .unwrap();
        let config = Config {
            claude_dir: root.clone(),
            codex_dir: root.join("codex"),
            data_dir: root.join("data"),
            extra_dirs: Vec::new(),
        };

        let resolved = ClaudeAdapter.resolve_successor_session(&config, process);

        assert_eq!(resolved, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dedups_streamed_records_by_message_id() {
        // Same message.id appears 3x with growing output; only final counts once.
        let text = r#"
{"type":"assistant","timestamp":"2026-07-29T08:00:00.000Z","cwd":"/tmp/proj","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"cache_read_input_tokens":50,"cache_creation_input_tokens":10,"output_tokens":5}}}
{"type":"assistant","timestamp":"2026-07-29T08:00:01.000Z","cwd":"/tmp/proj","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"cache_read_input_tokens":50,"cache_creation_input_tokens":10,"output_tokens":20}}}
{"type":"assistant","timestamp":"2026-07-29T08:00:02.000Z","cwd":"/tmp/proj","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"cache_read_input_tokens":50,"cache_creation_input_tokens":10,"output_tokens":40}}}
"#;
        let s = parse_str("sess", text).unwrap();
        // Counted once, final output 40 (not 5+20+40).
        assert_eq!(s.usage.output, 40);
        assert_eq!(s.usage.input, 100);
        assert_eq!(s.usage.cache_read, 50);
        assert_eq!(s.usage.cache_write, 10);
    }

    #[test]
    fn context_excludes_cache_creation_and_uses_latest() {
        let text = r#"
{"type":"assistant","timestamp":"2026-07-29T08:00:00.000Z","message":{"id":"a","model":"claude-opus-4-8","usage":{"input_tokens":1000,"cache_read_input_tokens":2000,"cache_creation_input_tokens":9999,"output_tokens":10}}}
{"type":"assistant","timestamp":"2026-07-29T09:00:00.000Z","message":{"id":"b","model":"claude-opus-4-8","usage":{"input_tokens":1500,"cache_read_input_tokens":3000,"cache_creation_input_tokens":8888,"output_tokens":20}}}
"#;
        let s = parse_str("sess", text).unwrap();
        // Latest = "b": 1500 + 3000, excluding cache creation.
        assert_eq!(s.context.tokens, 4500);
        assert_eq!(s.context.window, None);
        assert_eq!(s.latest_model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn context_uses_cache_creation_when_cache_read_is_zero() {
        let text = r#"
{"type":"assistant","timestamp":"2026-07-29T09:00:00.000Z","message":{"id":"cache-miss","model":"claude-opus-4-8","usage":{"input_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":129330,"output_tokens":201}}}
"#;

        let s = parse_str("sess", text).unwrap();

        assert_eq!(s.context.tokens, 129_332);
    }

    #[test]
    fn extracts_compaction_boundary() {
        let text = r#"
{"type":"assistant","timestamp":"2026-07-29T08:00:00.000Z","message":{"id":"a","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1}}}
{"type":"system","subtype":"compact_boundary","timestamp":"2026-07-29T08:30:00.000Z","compactMetadata":{"trigger":"auto","preTokens":150000}}
"#;
        let s = parse_str("sess", text).unwrap();
        assert_eq!(s.compactions.len(), 1);
        assert_eq!(s.compactions[0].trigger.as_deref(), Some("auto"));
        assert_eq!(s.compactions[0].pre_tokens, Some(150000));
    }

    #[test]
    fn malformed_lines_are_skipped_not_fatal() {
        let text = "not json\n{broken\n{\"type\":\"assistant\",\"message\":{\"id\":\"a\",\"model\":\"claude-opus-4-8\",\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n";
        let s = parse_str("sess", text).unwrap();
        assert_eq!(s.usage.input, 5);
    }

    #[test]
    fn current_run_matches_controlled_usage_fixture_after_process_boundary() {
        let text = r#"
{"type":"assistant","timestamp":"2026-07-29T08:59:59.000Z","message":{"id":"old","model":"claude-opus-4-8","usage":{"input_tokens":9000,"output_tokens":900,"cache_read_input_tokens":8000,"cache_creation_input_tokens":700}}}
{"type":"assistant","timestamp":"2026-07-29T09:00:01.000Z","message":{"id":"run-a","model":"claude-opus-4-8","usage":{"input_tokens":1200,"output_tokens":25,"cache_read_input_tokens":2300,"cache_creation_input_tokens":45}}}
{"type":"assistant","timestamp":"2026-07-29T09:00:02.000Z","message":{"id":"run-a","model":"claude-opus-4-8","usage":{"input_tokens":1200,"output_tokens":125,"cache_read_input_tokens":2300,"cache_creation_input_tokens":45}}}
{"type":"assistant","timestamp":"2026-07-29T09:01:00.000Z","message":{"id":"run-b","model":"claude-sonnet-4-6","usage":{"input_tokens":400,"output_tokens":75,"cache_read_input_tokens":600,"cache_creation_input_tokens":25}}}
"#;
        let boundary = iso_to_ms("2026-07-29T09:00:00.000Z").unwrap();

        let run = parse_current_run_str(text, boundary);

        assert_eq!(
            run.usage,
            Usage {
                input: 1_600,
                output: 200,
                cache_read: 2_900,
                cache_write: 70,
            }
        );
        assert_eq!(run.per_model.len(), 2);
        assert_eq!(
            run.per_model
                .iter()
                .find(|(model, _)| model == "claude-opus-4-8")
                .unwrap()
                .1,
            Usage {
                input: 1_200,
                output: 125,
                cache_read: 2_300,
                cache_write: 45,
            }
        );
        assert_eq!(
            run.per_model
                .iter()
                .find(|(model, _)| model == "claude-sonnet-4-6")
                .unwrap()
                .1,
            Usage {
                input: 400,
                output: 75,
                cache_read: 600,
                cache_write: 25,
            }
        );
    }
}
