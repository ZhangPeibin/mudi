//! Normalized domain model shared across the whole app.
//!
//! Provider-specific field names never appear outside the adapters in
//! `providers/`. Everything here speaks the vocabulary from `CONTEXT.md`.

use serde::{Deserialize, Serialize};

/// A provider-native session source. Sessions are never merged across providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    ClaudeCode,
    CodexCli,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "claude",
            Provider::CodexCli => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "Claude Code",
            Provider::CodexCli => "Codex CLI",
        }
    }

    pub fn from_str(s: &str) -> Option<Provider> {
        match s {
            "claude" => Some(Provider::ClaudeCode),
            "codex" => Some(Provider::CodexCli),
            _ => None,
        }
    }
}

/// Live lifecycle state. `Exited` renders with no marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Busy,
    Idle,
    Exited,
}

impl SessionState {
    /// Sort key: Busy first, Idle second, exited last.
    pub fn sort_rank(self) -> u8 {
        match self {
            SessionState::Busy => 0,
            SessionState::Idle => 1,
            SessionState::Exited => 2,
        }
    }

    pub fn is_open(self) -> bool {
        !matches!(self, SessionState::Exited)
    }
}

/// Cumulative, provider-reported token processing. Kept strictly separate from
/// [`ContextState`]. Categories are only populated when the provider exposes them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// The latest known occupancy of the model context window. Not cumulative.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextState {
    /// Latest token count occupying the window.
    pub tokens: u64,
    /// Model context window size, if known.
    pub window: Option<u64>,
}

impl ContextState {
    /// Percentage full, or `None` when the window is unknown.
    pub fn percent(&self) -> Option<f64> {
        match self.window {
            Some(w) if w > 0 => Some((self.tokens as f64 / w as f64) * 100.0),
            _ => None,
        }
    }
}

/// Per-model rollup within a session (mixed-model sessions split usage here).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub usage: Usage,
    /// Estimated API-rate dollars, `None` when the model price is unknown.
    pub cost: Option<f64>,
}

/// Trigger for a compaction, kept as an opaque provider label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compaction {
    /// Epoch milliseconds when observed.
    pub at_ms: i64,
    /// Provider trigger string (e.g. "auto", "manual"), if any.
    pub trigger: Option<String>,
    /// Tokens present before compaction, if reported.
    pub pre_tokens: Option<u64>,
}

/// One normalized session. This is what the UI renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub provider: Provider,
    /// Native provider session id.
    pub id: String,
    /// Display project label (basename, disambiguated across visible sessions).
    pub project: String,
    /// Absolute working directory of the session, if known.
    pub cwd: Option<String>,
    /// Latest model used by the session.
    pub latest_model: Option<String>,
    pub state: SessionState,
    pub context: ContextState,
    /// Cumulative processed usage across the session lifetime.
    pub usage: Usage,
    pub per_model: Vec<ModelUsage>,
    pub compactions: Vec<Compaction>,
    /// Epoch ms of the last activity in the source log.
    pub last_activity_ms: i64,
    /// Total estimated dollars; `None` if any contributing model price is unknown.
    pub cost: Option<f64>,
}

impl Session {
    /// Project label: basename of cwd (disambiguation happens at snapshot build time).
    pub fn project_basename(&self) -> String {
        match &self.cwd {
            Some(p) => std::path::Path::new(p)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.clone()),
            None => "—".to_string(),
        }
    }

    pub fn compaction_count(&self) -> usize {
        self.compactions.len()
    }
}
