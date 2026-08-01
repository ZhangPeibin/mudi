//! Provider adapters. Each adapter turns a source JSONL file into a
//! [`ParsedSession`]. Provider-specific JSONL field names never escape this module.

pub mod claude;
pub mod codex;
pub mod state;

use crate::config::Config;
use crate::domain::{Compaction, ContextState, Provider, SourceFormat, Usage};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// A provider adapter owns everything that differs between providers: where its
/// logs live, how to parse them, and how to read a model's context window. One
/// impl per provider means adding a provider is a pure addition — no `match
/// provider` sites elsewhere.
pub trait ProviderAdapter {
    /// Which provider this adapter speaks for.
    fn provider(&self) -> Provider;
    /// Encoding used by this provider's discovered source logs.
    fn source_format(&self) -> SourceFormat;
    /// Discover all candidate source files under the configured roots.
    fn discover(&self, cfg: &Config) -> Vec<PathBuf>;
    /// Parse one source file into the normalized [`ParsedSession`], or `None` if
    /// it is unreadable or yields nothing usable.
    fn parse_file(&self, path: &Path) -> Option<ParsedSession>;
    /// Best-known context window for `model`, in tokens. `None` when unknown —
    /// the UI then shows the raw token count with limit/percent as `N/A`.
    fn context_window(&self, model: &str) -> Option<u64>;
    /// Live lifecycle state for a session, given the shared once-per-scan process
    /// probe and the session's source path. The path is sufficient: Claude derives
    /// its session id from the transcript filename; Codex keys on the path itself.
    fn live_state(&self, probe: &state::StateDetector, source_path: &Path) -> state::LiveState;
    /// Usage observed at or after a validated Current Run start. `None` means
    /// the source cannot be read; an empty successful Run returns zero Usage.
    fn current_run_usage(&self, source_path: &Path, started_at_ms: i64) -> Option<ParsedUsage>;
}

/// Every provider adapter, in display order. Iterated instead of matching on
/// [`Provider`], so no dispatch site enumerates the variants.
pub const ADAPTERS: &[&dyn ProviderAdapter] = &[&claude::ClaudeAdapter, &codex::CodexAdapter];

/// The adapter that owns `provider`. Every [`Provider`] has exactly one.
pub fn adapter_for(provider: Provider) -> &'static dyn ProviderAdapter {
    ADAPTERS
        .iter()
        .copied()
        .find(|a| a.provider() == provider)
        .expect("every Provider has a registered adapter")
}

/// Walk `roots` recursively, collecting files whose name satisfies `keep`.
/// Shared by every adapter's `discover`; missing roots are skipped.
pub(crate) fn discover_files(roots: &[PathBuf], keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                if keep(name) {
                    out.push(entry.into_path());
                }
            }
        }
    }
    out
}

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
}

/// Provider-normalized Usage for a validated Current Run boundary.
#[derive(Debug, Clone, Default)]
pub struct ParsedUsage {
    pub usage: Usage,
    pub per_model: Vec<(String, Usage)>,
}

/// Parse ISO-8601 (RFC3339) timestamps to epoch milliseconds.
pub fn iso_to_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_dispatches_each_provider_to_its_own_adapter() {
        for p in [Provider::ClaudeCode, Provider::CodexCli] {
            assert_eq!(adapter_for(p).provider(), p);
        }
    }

    #[test]
    fn each_adapter_discovers_only_its_own_files() {
        use std::fs;
        let base = std::env::temp_dir().join("sessionmesh-adapter-discover-test");
        let _ = fs::remove_dir_all(&base);
        let claude_dir = base.join("claude");
        let codex_dir = base.join("codex");
        fs::create_dir_all(claude_dir.join("projects/p")).unwrap();
        fs::create_dir_all(codex_dir.join("sessions/2026/01/01")).unwrap();
        fs::write(claude_dir.join("projects/p/abc.jsonl"), "{}").unwrap();
        fs::write(
            codex_dir.join("sessions/2026/01/01/rollout-1-uuid.jsonl"),
            "{}",
        )
        .unwrap();
        // A stray file under the Codex root that the rollout- filter must exclude.
        fs::write(codex_dir.join("sessions/2026/01/01/other.jsonl"), "{}").unwrap();

        let cfg = Config {
            claude_dir: claude_dir.clone(),
            codex_dir: codex_dir.clone(),
            data_dir: base.join("data"),
            extra_dirs: vec![],
        };

        let claude = adapter_for(Provider::ClaudeCode).discover(&cfg);
        let codex = adapter_for(Provider::CodexCli).discover(&cfg);

        assert_eq!(claude.len(), 1, "Claude adapter sees only its .jsonl");
        assert!(claude[0].ends_with("abc.jsonl"));
        assert_eq!(
            codex.len(),
            1,
            "rollout- prefix filter excludes other.jsonl"
        );
        assert!(codex[0]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("rollout-"));

        let _ = fs::remove_dir_all(&base);
    }
}
