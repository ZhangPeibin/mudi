//! Claude Code adapter.
//!
//! Transcripts: `~/.claude/projects/<encoded-project>/<session-id>.jsonl`.
//!
//! Key semantics:
//! - Current context = latest assistant message `input_tokens + cache_read_input_tokens`
//!   (never `cache_creation_input_tokens`).
//! - Streaming writes multiple records per `message.id`; usage must be de-duplicated by
//!   id (last occurrence wins — it carries the final counts).
//! - Compaction = `type:system` / `subtype:compact_boundary`.

use super::{iso_to_ms, ParsedSession, ParsedUsage, PerModel, ProviderAdapter};
use crate::config::Config;
use crate::domain::{Compaction, ContextState, Provider, SessionState, Usage};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The Claude Code provider adapter.
pub struct ClaudeAdapter;

impl ProviderAdapter for ClaudeAdapter {
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

    fn context_window(&self, model: &str) -> Option<u64> {
        let m = model.to_ascii_lowercase();
        if m.starts_with("gpt-5.6-sol")
            || m.starts_with("gpt-5.6-terra")
            || m.starts_with("gpt-5.6-luna")
        {
            return Some(1_050_000);
        }
        if !m.starts_with("claude") {
            return None;
        }
        // Haiku-tier models are 200k; current Opus/Sonnet run a 1M window in
        // Claude Code. The transcript never records the window, so it is derived
        // from the model name here.
        if m.contains("haiku") {
            Some(200_000)
        } else {
            Some(1_000_000)
        }
    }

    fn live_state(
        &self,
        probe: &super::state::StateDetector,
        source_path: &Path,
    ) -> super::state::LiveState {
        // A Claude session's id is its transcript filename stem.
        match source_path.file_stem().and_then(|s| s.to_str()) {
            Some(id) => probe.claude_state(id),
            None => super::state::LiveState {
                state: SessionState::Exited,
                current_run_started_at_ms: None,
            },
        }
    }

    fn current_run_usage(&self, source_path: &Path, started_at_ms: i64) -> Option<ParsedUsage> {
        let text = std::fs::read_to_string(source_path).ok()?;
        Some(parse_current_run_str(&text, started_at_ms))
    }
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
                // Current context excludes cache creation (write-side).
                let ctx_tokens = input + cache_read;

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
            Some("system") => {
                if v.get("subtype").and_then(Value::as_str) == Some("compact_boundary") {
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
                window: ClaudeAdapter.context_window(&m.model),
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
        assert_eq!(s.context.window, Some(1_000_000));
        assert_eq!(s.latest_model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn proxied_gpt_5_6_models_use_their_documented_context_window() {
        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            assert_eq!(ClaudeAdapter.context_window(model), Some(1_050_000));
        }
    }

    #[test]
    fn context_window_is_1m_except_haiku() {
        // Current Claude Code Opus/Sonnet models run a 1M window; Haiku-tier is 200k.
        assert_eq!(
            ClaudeAdapter.context_window("claude-opus-4-8"),
            Some(1_000_000)
        );
        assert_eq!(
            ClaudeAdapter.context_window("claude-sonnet-4-6"),
            Some(1_000_000)
        );
        assert_eq!(
            ClaudeAdapter.context_window("claude-haiku-4-5"),
            Some(200_000)
        );
        // Case-insensitive on the model string.
        assert_eq!(
            ClaudeAdapter.context_window("Claude-Haiku-4-5"),
            Some(200_000)
        );
        // Non-Claude strings remain unknown.
        assert_eq!(ClaudeAdapter.context_window("gpt-5.5"), None);
    }

    #[test]
    fn large_context_no_longer_exceeds_100_percent() {
        // Observed 212.7k tokens on opus-4-8 previously rendered as 106% against a
        // hardcoded 200k window; against the real 1M window it is well under 100%.
        let ctx = ContextState {
            tokens: 212_700,
            window: ClaudeAdapter.context_window("claude-opus-4-8"),
        };
        let pct = ctx.percent().expect("window known");
        assert!(
            pct < 100.0,
            "212.7k should be under 100% of the real window, got {pct}"
        );
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
