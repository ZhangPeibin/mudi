//! Provider adapters. Each adapter turns a source JSONL file into a
//! [`ParsedSession`]. Provider-specific JSONL field names never escape this module.

pub mod claude;
pub mod codex;
pub mod state;

use crate::domain::{Compaction, ContextState, Provider, Usage};

/// The normalized result of parsing one source session file.
///
/// Live lifecycle state and dollar cost are attached later (state detector +
/// pricing) — this struct is purely what the log file itself reveals.
#[derive(Debug, Clone)]
pub struct ParsedSession {
    pub provider: Provider,
    pub id: String,
    pub cwd: Option<String>,
    pub latest_model: Option<String>,
    pub context: ContextState,
    pub usage: Usage,
    /// Usage split by model, in first-seen order.
    pub per_model: Vec<(String, Usage)>,
    pub compactions: Vec<Compaction>,
    pub last_activity_ms: i64,
    /// Codex only: `Some(true)` = latest task boundary is an unmatched `task_started`,
    /// `Some(false)` = latest boundary is a completion/abort. `None` for Claude.
    pub codex_open_turn: Option<bool>,
}

/// Parse ISO-8601 (RFC3339) timestamps to epoch milliseconds.
pub fn iso_to_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Best-known context window for a model, in tokens. `None` when unknown — the
/// UI then shows the raw token count with limit/percent as `N/A`.
pub fn known_context_window(model: &str) -> Option<u64> {
    let m = model.to_ascii_lowercase();
    if m.starts_with("claude") {
        // Claude Code models are 200k-window by default in this era.
        Some(200_000)
    } else {
        // Codex records its own window in the log; don't guess here.
        None
    }
}

/// A small helper to accumulate per-model usage while preserving first-seen order.
#[derive(Default)]
pub(crate) struct PerModel {
    order: Vec<String>,
    map: std::collections::HashMap<String, Usage>,
}

impl PerModel {
    pub fn add(&mut self, model: &str, u: &Usage) {
        let entry = self.map.entry(model.to_string()).or_insert_with(|| {
            self.order.push(model.to_string());
            Usage::default()
        });
        entry.add(u);
    }

    pub fn into_vec(self) -> Vec<(String, Usage)> {
        let PerModel { order, mut map } = self;
        order
            .into_iter()
            .filter_map(|k| map.remove(&k).map(|u| (k, u)))
            .collect()
    }
}
