//! Configuration: data directories, environment overrides, thresholds.
//!
//! Standard locations with env overrides and configurable extra directories.

use std::path::PathBuf;

/// Warning context threshold (percent of window).
pub const WARN_PCT: f64 = 70.0;
/// Critical context threshold (percent of window).
pub const CRIT_PCT: f64 = 85.0;

/// Return the process-local Theme name supplied as `--theme <name>`.
pub fn theme_override_from_args<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter().skip(1);
    while let Some(argument) = args.next() {
        if argument.as_ref() == "--theme" {
            return args.next().map(|name| name.as_ref().to_string());
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Root of Claude Code data (`~/.claude` by default).
    pub claude_dir: PathBuf,
    /// Root of Codex CLI data (`~/.codex` by default).
    pub codex_dir: PathBuf,
    /// Where the disposable SQLite index lives.
    pub data_dir: PathBuf,
    /// Additional directories to scan for provider logs.
    pub extra_dirs: Vec<PathBuf>,
}

impl Config {
    /// Resolve configuration from the environment, falling back to standard paths.
    ///
    /// Overrides:
    /// - `SESSIONMESH_CLAUDE_DIR`
    /// - `SESSIONMESH_CODEX_DIR`
    /// - `SESSIONMESH_DATA_DIR`
    /// - `SESSIONMESH_EXTRA_DIRS` (colon-separated)
    pub fn load() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

        let claude_dir = env_path("SESSIONMESH_CLAUDE_DIR", "TOKENPLAY_CLAUDE_DIR")
            .unwrap_or_else(|| home.join(".claude"));
        let codex_dir = env_path("SESSIONMESH_CODEX_DIR", "TOKENPLAY_CODEX_DIR")
            .unwrap_or_else(|| home.join(".codex"));

        let data_dir =
            env_path("SESSIONMESH_DATA_DIR", "TOKENPLAY_DATA_DIR").unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("sessionmesh")
            });

        let extra_dirs = env_value("SESSIONMESH_EXTRA_DIRS", "TOKENPLAY_EXTRA_DIRS")
            .map(|v| {
                v.split(':')
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();

        Config {
            claude_dir,
            codex_dir,
            data_dir,
            extra_dirs,
        }
    }

    /// Path to the SQLite index file (inside `data_dir`).
    pub fn index_path(&self) -> PathBuf {
        self.data_dir.join("index.db")
    }

    /// Authoritative Operation Task Store, separate from the disposable Session index.
    pub fn task_store_path(&self) -> PathBuf {
        self.data_dir.join("tasks.db")
    }

    /// Private root for Task-owned Handoff Artifacts.
    pub fn artifact_root(&self) -> PathBuf {
        self.data_dir.join("handoff-artifacts")
    }

    /// Local Profile preferences, stored separately from the disposable Session index.
    pub fn profile_path(&self) -> PathBuf {
        self.data_dir.join("profile.json")
    }

    /// Authoritative local Session metadata store, separate from the disposable index.
    pub fn label_store_path(&self) -> PathBuf {
        self.data_dir.join("session-labels.json")
    }

    /// Optional local Model Catalog override (`data_dir/models.json`).
    pub fn model_catalog_override_path(&self) -> PathBuf {
        self.data_dir.join("models.json")
    }

    /// Directories to search for Claude transcripts.
    pub fn claude_project_roots(&self) -> Vec<PathBuf> {
        let mut v = vec![self.claude_dir.join("projects")];
        v.extend(self.extra_dirs.iter().cloned());
        v
    }

    /// Directories to search for Codex rollouts.
    pub fn codex_session_roots(&self) -> Vec<PathBuf> {
        let mut v = vec![self.codex_dir.join("sessions")];
        v.extend(self.extra_dirs.iter().cloned());
        v
    }

    /// Path to the Claude open-session state directory.
    pub fn claude_sessions_dir(&self) -> PathBuf {
        self.claude_dir.join("sessions")
    }
}

fn env_path(primary_key: &str, legacy_key: &str) -> Option<PathBuf> {
    env_value(primary_key, legacy_key).map(PathBuf::from)
}

fn env_value(primary_key: &str, legacy_key: &str) -> Option<String> {
    std::env::var(primary_key)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var(legacy_key).ok().filter(|s| !s.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_override_is_read_from_process_arguments() {
        let args = ["sessionmesh", "--theme", "dracula"];

        assert_eq!(theme_override_from_args(args), Some("dracula".to_string()));
    }

    #[test]
    fn label_store_lives_in_the_sessionmesh_data_directory() {
        let config = Config {
            claude_dir: PathBuf::from("/claude"),
            codex_dir: PathBuf::from("/codex"),
            data_dir: PathBuf::from("/sessionmesh-data"),
            extra_dirs: Vec::new(),
        };

        assert_eq!(
            config.label_store_path(),
            PathBuf::from("/sessionmesh-data/session-labels.json")
        );
    }

    #[test]
    fn model_catalog_override_lives_in_the_sessionmesh_data_directory() {
        let config = Config {
            claude_dir: PathBuf::from("/claude"),
            codex_dir: PathBuf::from("/codex"),
            data_dir: PathBuf::from("/sessionmesh-data"),
            extra_dirs: Vec::new(),
        };

        assert_eq!(
            config.model_catalog_override_path(),
            PathBuf::from("/sessionmesh-data/models.json")
        );
    }

    #[test]
    fn task_store_and_artifacts_are_separate_from_the_disposable_index() {
        let config = Config {
            claude_dir: PathBuf::from("/claude"),
            codex_dir: PathBuf::from("/codex"),
            data_dir: PathBuf::from("/sessionmesh-data"),
            extra_dirs: Vec::new(),
        };

        assert_eq!(
            config.task_store_path(),
            PathBuf::from("/sessionmesh-data/tasks.db")
        );
        assert_eq!(
            config.artifact_root(),
            PathBuf::from("/sessionmesh-data/handoff-artifacts")
        );
        assert_ne!(config.task_store_path(), config.index_path());
    }

    #[test]
    fn renamed_environment_variables_take_priority_over_legacy_values() {
        let suffix = std::process::id();
        let primary = format!("SESSIONMESH_TEST_PRIMARY_{suffix}");
        let legacy = format!("SESSIONMESH_TEST_LEGACY_{suffix}");
        std::env::set_var(&legacy, "/legacy");

        assert_eq!(env_path(&primary, &legacy), Some(PathBuf::from("/legacy")));

        std::env::set_var(&primary, "/renamed");
        assert_eq!(env_path(&primary, &legacy), Some(PathBuf::from("/renamed")));

        std::env::remove_var(primary);
        std::env::remove_var(legacy);
    }
}
