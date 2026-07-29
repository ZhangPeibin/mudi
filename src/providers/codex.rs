//! Codex CLI adapter.
//!
//! Transcripts: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//!
//! Key semantics:
//! - `token_count.info.total_token_usage` is cumulative — take the latest, never sum.
//! - `cached_input_tokens` is a subset of `input_tokens`; split it out for pricing so
//!   input is not double-counted.
//! - Current context = latest `last_token_usage` (`input + output`) vs
//!   `model_context_window`.
//! - Per-model split via cumulative deltas attributed to the active `turn_context.model`.
//! - Compaction = top-level `type:compacted` and/or `event_msg`/`context_compacted`
//!   (collapsed when they describe the same event).

use super::{iso_to_ms, ParsedSession, PerModel};
use crate::domain::{Compaction, ContextState, Provider, Usage};
use serde_json::Value;
use std::path::Path;

/// Compaction events emitted within this window (ms) are treated as one event.
const COMPACT_DEDUP_MS: i64 = 2000;

pub fn parse_file(path: &Path) -> Option<ParsedSession> {
    let text = std::fs::read_to_string(path).ok()?;
    let fallback_id = derive_id_from_filename(path);
    parse_str(&fallback_id, &text)
}

/// Extract the session UUID from a `rollout-<ts>-<uuid>.jsonl` filename.
fn derive_id_from_filename(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // The UUID is the last 5 dash-separated groups.
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() >= 5 {
        parts[parts.len() - 5..].join("-")
    } else {
        stem
    }
}

pub fn parse_str(fallback_id: &str, text: &str) -> Option<ParsedSession> {
    let mut id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut latest_model: Option<String> = None;
    let mut last_activity_ms: i64 = 0;

    // Cumulative snapshot mapped to Usage (authoritative total).
    let mut total = Usage::default();
    // Raw cumulative previous values for delta attribution.
    let mut prev_input: u64 = 0;
    let mut prev_cached: u64 = 0;
    let mut prev_output: u64 = 0;
    let mut per_model = PerModel::default();
    let mut have_token_count = false;

    // Current context from latest last_token_usage.
    let mut context = ContextState::default();

    // Latest task boundary: Some(true)=started/open, Some(false)=completed/aborted.
    let mut open_turn: Option<bool> = None;

    let mut compaction_ts: Vec<i64> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let top_ts = v.get("timestamp").and_then(Value::as_str).and_then(iso_to_ms);
        if let Some(t) = top_ts {
            last_activity_ms = last_activity_ms.max(t);
        }

        let top_type = v.get("type").and_then(Value::as_str);
        if top_type == Some("compacted") {
            compaction_ts.push(top_ts.unwrap_or(last_activity_ms));
            continue;
        }

        let payload = v.get("payload");

        match top_type {
            Some("session_meta") => {
                if let Some(p) = payload {
                    if id.is_none() {
                        id = p.get("id").and_then(Value::as_str).map(str::to_string);
                    }
                    if cwd.is_none() {
                        cwd = p.get("cwd").and_then(Value::as_str).map(str::to_string);
                    }
                }
            }
            Some("turn_context") => {
                if let Some(p) = payload {
                    if let Some(m) = p.get("model").and_then(Value::as_str) {
                        current_model = Some(m.to_string());
                        latest_model = Some(m.to_string());
                    }
                    if cwd.is_none() {
                        cwd = p.get("cwd").and_then(Value::as_str).map(str::to_string);
                    }
                }
            }
            Some("event_msg") => {
                let Some(p) = payload else { continue };
                match p.get("type").and_then(Value::as_str) {
                    Some("task_started") => open_turn = Some(true),
                    Some("task_complete") | Some("turn_aborted") => open_turn = Some(false),
                    Some("context_compacted") => {
                        compaction_ts.push(top_ts.unwrap_or(last_activity_ms));
                    }
                    Some("token_count") => {
                        let Some(info) = p.get("info") else { continue };
                        have_token_count = true;

                        // Current context from last_token_usage.
                        if let Some(last) = info.get("last_token_usage") {
                            let li = u(last, "input_tokens");
                            let lo = u(last, "output_tokens");
                            context.tokens = li + lo;
                        }
                        if let Some(w) = info.get("model_context_window").and_then(Value::as_u64) {
                            if w > 0 {
                                context.window = Some(w);
                            }
                        }

                        // Cumulative total → authoritative Usage.
                        if let Some(tot) = info.get("total_token_usage") {
                            let ci = u(tot, "input_tokens");
                            let cc = u(tot, "cached_input_tokens");
                            let co = u(tot, "output_tokens");

                            total = Usage {
                                input: ci.saturating_sub(cc),
                                output: co,
                                cache_read: cc,
                                cache_write: 0,
                            };

                            // Delta attribution to the active model.
                            let d_input = ci.saturating_sub(prev_input);
                            let d_cached = cc.saturating_sub(prev_cached);
                            let d_output = co.saturating_sub(prev_output);
                            let model = current_model.clone().unwrap_or_else(|| "unknown".to_string());
                            per_model.add(
                                &model,
                                &Usage {
                                    input: d_input.saturating_sub(d_cached),
                                    output: d_output,
                                    cache_read: d_cached,
                                    cache_write: 0,
                                },
                            );
                            prev_input = ci;
                            prev_cached = cc;
                            prev_output = co;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    if !have_token_count && compaction_ts.is_empty() {
        return None;
    }

    // Collapse compaction events that describe the same moment.
    compaction_ts.sort_unstable();
    let mut compactions: Vec<Compaction> = Vec::new();
    let mut last_kept = i64::MIN;
    for t in compaction_ts {
        if t.saturating_sub(last_kept) > COMPACT_DEDUP_MS {
            compactions.push(Compaction { at_ms: t, trigger: None, pre_tokens: None });
            last_kept = t;
        }
    }

    Some(ParsedSession {
        provider: Provider::CodexCli,
        id: id.unwrap_or_else(|| fallback_id.to_string()),
        cwd,
        latest_model,
        context,
        usage: total,
        per_model: per_model.into_vec(),
        compactions,
        last_activity_ms,
        codex_open_turn: open_turn,
    })
}

fn u(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_latest_cumulative_never_sums() {
        let text = r#"
{"type":"session_meta","timestamp":"2026-05-29T08:30:53.148Z","payload":{"id":"abc","cwd":"/tmp/x"}}
{"type":"turn_context","timestamp":"2026-05-29T08:30:53.151Z","payload":{"model":"gpt-5.5"}}
{"type":"event_msg","timestamp":"2026-05-29T08:30:57.899Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10},"last_token_usage":{"input_tokens":100,"output_tokens":10},"model_context_window":258400}}}
{"type":"event_msg","timestamp":"2026-05-29T08:31:57.899Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"cached_input_tokens":120,"output_tokens":25},"last_token_usage":{"input_tokens":250,"output_tokens":15},"model_context_window":258400}}}
"#;
        let s = parse_str("fb", text).unwrap();
        // Latest cumulative only: input 300-120=180, cache_read 120, output 25.
        assert_eq!(s.usage.input, 180);
        assert_eq!(s.usage.cache_read, 120);
        assert_eq!(s.usage.output, 25);
        // Context from latest last_token_usage: 250 + 15.
        assert_eq!(s.context.tokens, 265);
        assert_eq!(s.context.window, Some(258400));
        assert_eq!(s.id, "abc");
        assert_eq!(s.latest_model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn task_boundary_tracks_open_turn() {
        let started = r#"
{"type":"event_msg","timestamp":"2026-05-29T08:30:53.150Z","payload":{"type":"task_started","model_context_window":258400}}
{"type":"event_msg","timestamp":"2026-05-29T08:30:57.899Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":1},"last_token_usage":{"input_tokens":10,"output_tokens":1},"model_context_window":258400}}}
"#;
        assert_eq!(parse_str("fb", started).unwrap().codex_open_turn, Some(true));

        let completed = format!(
            "{started}{}",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-05-29T08:31:00.000Z\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"t\"}}\n"
        );
        assert_eq!(parse_str("fb", &completed).unwrap().codex_open_turn, Some(false));
    }

    #[test]
    fn per_model_split_by_delta() {
        let text = r#"
{"type":"turn_context","timestamp":"2026-05-29T08:30:53.151Z","payload":{"model":"gpt-5.5"}}
{"type":"event_msg","timestamp":"2026-05-29T08:30:57.899Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10},"last_token_usage":{"input_tokens":100,"output_tokens":10},"model_context_window":258400}}}
{"type":"turn_context","timestamp":"2026-05-29T08:31:00.000Z","payload":{"model":"gpt-5"}}
{"type":"event_msg","timestamp":"2026-05-29T08:31:57.899Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":250,"cached_input_tokens":0,"output_tokens":30},"last_token_usage":{"input_tokens":150,"output_tokens":20},"model_context_window":258400}}}
"#;
        let s = parse_str("fb", text).unwrap();
        assert_eq!(s.per_model.len(), 2);
        let g55 = s.per_model.iter().find(|(m, _)| m == "gpt-5.5").unwrap();
        let g5 = s.per_model.iter().find(|(m, _)| m == "gpt-5").unwrap();
        assert_eq!(g55.1.input, 100);
        assert_eq!(g55.1.output, 10);
        assert_eq!(g5.1.input, 150); // 250-100
        assert_eq!(g5.1.output, 20); // 30-10
    }

    #[test]
    fn collapses_duplicate_compaction_events() {
        let text = r#"
{"type":"compacted","timestamp":"2026-05-20T11:33:34.024Z","payload":{"message":"..."}}
{"type":"event_msg","timestamp":"2026-05-20T11:33:34.028Z","payload":{"type":"context_compacted"}}
{"type":"event_msg","timestamp":"2026-05-20T11:33:57.899Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":1},"last_token_usage":{"input_tokens":10,"output_tokens":1},"model_context_window":258400}}}
"#;
        let s = parse_str("fb", text).unwrap();
        assert_eq!(s.compactions.len(), 1, "4ms-apart events are one compaction");
    }
}
