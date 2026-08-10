//! Normalized domain model shared across the whole app.
//!
//! Provider-specific field names never appear outside the adapters in
//! `providers/`. Everything here speaks the vocabulary from `CONTEXT.md`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A provider-native session source. Sessions are never merged across providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    ClaudeCode,
    CodexCli,
}

/// Encoding used by a provider's local Session source log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceFormat {
    #[default]
    JsonLines,
}

impl SourceFormat {
    pub fn label(self) -> &'static str {
        match self {
            SourceFormat::JsonLines => "JSONL",
        }
    }
}

/// Location and encoding of the local source log indexed for a Session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMetadata {
    pub path: PathBuf,
    pub format: SourceFormat,
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

    pub fn parse(s: &str) -> Option<Provider> {
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

/// Whether SessionMesh only observes a Session or has established management through a
/// Binding Receipt. Observation is always the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionProvenance {
    #[default]
    Observed,
    Managed,
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

/// Processed Usage and API price estimate for one explicitly labelled scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub usage: Usage,
    pub per_model: Vec<ModelUsage>,
    /// Estimated API-rate dollars, `None` when any contributing model is unknown.
    pub cost: Option<f64>,
}

/// Trigger for a compaction, kept as an opaque provider label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Compaction {
    /// Epoch milliseconds when observed.
    pub at_ms: i64,
    /// Provider trigger string (e.g. "auto", "manual"), if any.
    pub trigger: Option<String>,
    /// Tokens present before compaction, if reported.
    pub pre_tokens: Option<u64>,
}

/// Stable identity for a Project. Git Projects use their canonical common
/// directory; standalone non-Git Projects use their canonical working directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProjectIdentity {
    GitCommonDir(PathBuf),
    CanonicalCwd(PathBuf),
}

impl ProjectIdentity {
    /// Stable path used when deriving a human-readable Project label.
    pub fn display_path(&self) -> &std::path::Path {
        match self {
            ProjectIdentity::GitCommonDir(path) => path
                .file_name()
                .filter(|name| *name == ".git")
                .and_then(|_| path.parent())
                .unwrap_or(path),
            ProjectIdentity::CanonicalCwd(path) => path,
        }
    }
}

/// A resolved Project and its display label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub identity: ProjectIdentity,
    pub label: String,
}

/// A resolved Git Worktree. Its canonical root is its stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub writable: bool,
}

/// One normalized session. This is what the UI renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub provider: Provider,
    /// Native provider session id.
    pub id: String,
    /// Control provenance established independently from Provider source observation.
    #[serde(default)]
    pub provenance: SessionProvenance,
    /// Metadata about the indexed source log. Its contents are never exposed here.
    pub source: SourceMetadata,
    /// Canonical Project identity and display label, if resolution succeeded.
    pub project: Option<Project>,
    /// Canonical Git Worktree metadata. `None` for non-Git or unavailable identity.
    pub worktree: Option<Worktree>,
    /// Absolute working directory of the session, if known.
    pub cwd: Option<String>,
    /// Latest model used by the session.
    pub latest_model: Option<String>,
    pub state: SessionState,
    pub context: ContextState,
    /// Cumulative processed usage across the session lifetime.
    pub usage: Usage,
    pub per_model: Vec<ModelUsage>,
    /// Usage since the validated start of the Current Run. `None` means the
    /// Run boundary or source Usage is unavailable.
    #[serde(default)]
    pub current_run_usage: Option<UsageSummary>,
    pub compactions: Vec<Compaction>,
    /// Epoch ms of the last activity in the source log.
    pub last_activity_ms: i64,
    /// Total estimated dollars; `None` if any contributing model price is unknown.
    pub cost: Option<f64>,
}

/// Stable Provider-scoped identity for one Session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionIdentity {
    pub provider: Provider,
    pub native_id: String,
}

impl From<&Session> for SessionIdentity {
    fn from(session: &Session) -> Self {
        Self {
            provider: session.provider,
            native_id: session.id.clone(),
        }
    }
}

impl Session {
    pub fn project_label(&self) -> &str {
        self.project
            .as_ref()
            .map_or("—", |project| project.label.as_str())
    }

    pub fn compaction_count(&self) -> usize {
        self.compactions.len()
    }
}
