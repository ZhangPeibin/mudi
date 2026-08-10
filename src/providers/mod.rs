//! Provider adapters. Each adapter turns a source JSONL file into a
//! [`ParsedSession`]. Provider-specific JSONL field names never escape this module.

pub mod claude;
pub mod codex;
pub mod state;

use crate::config::Config;
use crate::domain::{Compaction, ContextState, Provider, SourceFormat, SourceMetadata, Usage};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Fixed, opt-in message used only to induce a Provider source record for Binding.
pub const BINDING_INITIALIZER: &str =
    "SessionMesh initialization only. Do not inspect files, run commands, or make changes. Reply with exactly: READY.";

/// Provider-normalized source and process evidence considered for a Binding Receipt.
/// Provider-private field names and file layouts do not cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBindingEvidence {
    pub provider: Provider,
    pub source_session_id: String,
    pub process_session_id: String,
    pub source: SourceMetadata,
    pub working_directory: PathBuf,
    pub provider_process: state::ProcessIdentity,
}

/// Source discovery, parsing, live observation, and Current Run usage for one Provider.
pub trait ObservationPort: Sync {
    /// Which provider this adapter speaks for.
    fn provider(&self) -> Provider;
    /// Encoding used by this provider's discovered source logs.
    fn source_format(&self) -> SourceFormat;
    /// Discover all candidate source files under the configured roots.
    fn discover(&self, cfg: &Config) -> Vec<PathBuf>;
    /// Parse one source file into the normalized [`ParsedSession`], or `None` if
    /// it is unreadable or yields nothing usable.
    fn parse_file(&self, path: &Path) -> Option<ParsedSession>;
    /// Live lifecycle state for a session, given the shared once-per-scan process
    /// probe and the session's source path. The path is sufficient: Claude derives
    /// its session id from the transcript filename; Codex keys on the path itself.
    fn live_state(
        &self,
        probe: &state::StateDetector,
        source_session_id: &str,
        source_path: &Path,
    ) -> state::LiveState;
    /// Usage observed at or after a validated Current Run start. `None` means
    /// the source cannot be read; an empty successful Run returns zero Usage.
    fn current_run_usage(&self, source_path: &Path, started_at_ms: i64) -> Option<ParsedUsage>;
}

/// Provider-native process and invocation behavior used by authorized workflows.
pub trait RuntimeControlPort: Sync {
    /// Observe the exact Current Run named by source evidence.
    fn observe_current_run(
        &self,
        probe: &state::StateDetector,
        source_session_id: &str,
        source_path: &Path,
    ) -> state::LiveState;
    /// Normalize the Provider source record tied to one exact launched process.
    /// `None` means evidence is missing, stale, ambiguous, or contradictory.
    fn binding_evidence(
        &self,
        cfg: &Config,
        process: state::ProcessIdentity,
    ) -> Option<ProviderBindingEvidence>;
    /// Provider-native arguments for starting a new interactive Session.
    fn new_session_args(&self, binding_initializer: bool) -> Vec<String>;
    /// Provider-native arguments for resuming one exact interactive Session.
    fn resume_session_args(&self, native_session_id: &str) -> Vec<String>;
    /// Provider-native arguments for read-only, noninteractive Handoff generation.
    fn handoff_resume_args(&self, native_session_id: &str) -> Vec<String>;
    /// Resolve one exact launched process to a Provider-native Successor Session.
    /// `None` means the available source evidence is not conclusive.
    fn resolve_successor_session(
        &self,
        cfg: &Config,
        process: state::ProcessIdentity,
    ) -> Option<String>;
    /// Build a provider-native launch plan for a validated Handoff Artifact.
    fn successor_launch_arguments(&self, artifact_path: &Path) -> Vec<String>;
    /// Resolve this Provider's configured executable when it is usable.
    fn executable<'a>(&self, executables: &'a ProviderExecutables) -> Option<&'a Path>;
}

/// Provider-native transport for a declared managed Skill.
pub trait SkillTransportPort: Sync {
    /// Explicit Provider-native invocation for the managed Handoff Skill.
    fn handoff_skill_invocation(&self) -> &'static str;
}

/// Backwards-compatible aggregate used only by registry compatibility helpers.
pub trait ProviderAdapter: ObservationPort + RuntimeControlPort + SkillTransportPort {}

impl<T> ProviderAdapter for T where T: ObservationPort + RuntimeControlPort + SkillTransportPort {}

/// A requested capability is not available for the selected Provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityUnavailable {
    provider: Provider,
    capability: &'static str,
}

impl CapabilityUnavailable {
    fn new(provider: Provider, capability: &'static str) -> Self {
        Self {
            provider,
            capability,
        }
    }
}

impl fmt::Display for CapabilityUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} does not support {}",
            self.provider.label(),
            self.capability
        )
    }
}

impl std::error::Error for CapabilityUnavailable {}

struct ProviderCapabilities {
    provider: Provider,
    adapter: &'static dyn ProviderAdapter,
    observation: &'static dyn ObservationPort,
    runtime_control: &'static dyn RuntimeControlPort,
    skill_transport: &'static dyn SkillTransportPort,
}

/// The sole owner of Provider capability dispatch.
pub struct ProviderRegistry {
    providers: &'static [ProviderCapabilities],
}

impl ProviderRegistry {
    fn entry(&self, provider: Provider) -> Option<&'static ProviderCapabilities> {
        self.providers
            .iter()
            .find(|entry| entry.provider == provider)
    }

    pub fn observation(
        &self,
        provider: Provider,
    ) -> Result<&'static dyn ObservationPort, CapabilityUnavailable> {
        self.entry(provider)
            .map(|entry| entry.observation)
            .ok_or_else(|| CapabilityUnavailable::new(provider, "source observation"))
    }

    pub fn observations(&self) -> impl Iterator<Item = &'static dyn ObservationPort> + '_ {
        self.providers.iter().map(|entry| entry.observation)
    }

    pub fn runtime_control(
        &self,
        provider: Provider,
    ) -> Result<&'static dyn RuntimeControlPort, CapabilityUnavailable> {
        self.entry(provider)
            .map(|entry| entry.runtime_control)
            .ok_or_else(|| CapabilityUnavailable::new(provider, "runtime control"))
    }

    pub fn skill_transport(
        &self,
        provider: Provider,
    ) -> Result<&'static dyn SkillTransportPort, CapabilityUnavailable> {
        self.entry(provider)
            .map(|entry| entry.skill_transport)
            .ok_or_else(|| CapabilityUnavailable::new(provider, "Skill transport"))
    }
}

static CLAUDE_ADAPTER: claude::ClaudeAdapter = claude::ClaudeAdapter;
static CODEX_ADAPTER: codex::CodexAdapter = codex::CodexAdapter;

static PROVIDER_CAPABILITIES: &[ProviderCapabilities] = &[
    ProviderCapabilities {
        provider: Provider::ClaudeCode,
        adapter: &CLAUDE_ADAPTER,
        observation: &CLAUDE_ADAPTER,
        runtime_control: &CLAUDE_ADAPTER,
        skill_transport: &CLAUDE_ADAPTER,
    },
    ProviderCapabilities {
        provider: Provider::CodexCli,
        adapter: &CODEX_ADAPTER,
        observation: &CODEX_ADAPTER,
        runtime_control: &CODEX_ADAPTER,
        skill_transport: &CODEX_ADAPTER,
    },
];

static REGISTRY: ProviderRegistry = ProviderRegistry {
    providers: PROVIDER_CAPABILITIES,
};

pub fn registry() -> &'static ProviderRegistry {
    &REGISTRY
}

/// Provider-native executables used by authorized runtime workflows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderExecutables {
    claude: PathBuf,
    codex: PathBuf,
}

impl ProviderExecutables {
    pub fn new(claude: PathBuf, codex: PathBuf) -> Self {
        Self { claude, codex }
    }

    pub fn discover() -> Self {
        let search = std::env::var_os("PATH")
            .as_deref()
            .map(std::env::split_paths)
            .map(Iterator::collect::<Vec<_>>)
            .unwrap_or_default();
        let mut discovered = Self::discover_in(&search);
        if let Some(path) = std::env::var_os("SESSIONMESH_CLAUDE_EXECUTABLE") {
            discovered.claude = PathBuf::from(path);
        }
        if let Some(path) = std::env::var_os("SESSIONMESH_CODEX_EXECUTABLE") {
            discovered.codex = PathBuf::from(path);
        }
        discovered
    }

    pub fn discover_in(search_directories: &[PathBuf]) -> Self {
        Self {
            claude: find_executable(search_directories, "claude").unwrap_or_default(),
            codex: find_executable(search_directories, "codex").unwrap_or_default(),
        }
    }

    pub fn executable_for(&self, provider: Provider) -> Option<&Path> {
        registry().runtime_control(provider).ok()?.executable(self)
    }

    pub(crate) fn claude_path(&self) -> &Path {
        &self.claude
    }

    pub(crate) fn codex_path(&self) -> &Path {
        &self.codex
    }
}

fn find_executable(search_directories: &[PathBuf], name: &str) -> Option<PathBuf> {
    search_directories
        .iter()
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

pub(crate) fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Every provider adapter, in display order. Iterated instead of matching on
/// [`Provider`], so no dispatch site enumerates the variants.
pub const ADAPTERS: &[&dyn ProviderAdapter] = &[&CLAUDE_ADAPTER, &CODEX_ADAPTER];

/// The adapter that owns `provider`. Every [`Provider`] has exactly one.
pub fn adapter_for(provider: Provider) -> &'static dyn ProviderAdapter {
    registry()
        .entry(provider)
        .map(|entry| entry.adapter)
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
/// Model Catalog data) — this struct is purely what the log file itself reveals.
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
    fn unavailable_capabilities_are_explicit() {
        let registry = ProviderRegistry { providers: &[] };
        let error = registry
            .runtime_control(Provider::ClaudeCode)
            .err()
            .expect("an absent capability must not be guessed");

        assert_eq!(
            error.to_string(),
            "Claude Code does not support runtime control"
        );
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
