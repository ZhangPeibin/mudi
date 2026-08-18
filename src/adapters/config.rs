//! Configuration: data directories, environment overrides, thresholds.
//!
//! Standard locations with env overrides and configurable extra directories.
use std::path::PathBuf;

/// Warning context threshold (percent of window).
pub const WARN_PCT: f64 = 70.0;
/// Critical context threshold (percent of window).
pub const CRIT_PCT: f64 = 85.0;

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
    /// - `MUDI_CLAUDE_DIR`
    /// - `MUDI_CODEX_DIR`
    /// - `MUDI_DATA_DIR`
    /// - `MUDI_EXTRA_DIRS` (colon-separated)
    ///
    pub fn load() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

        let claude_dir = env_path(["MUDI_CLAUDE_DIR"]).unwrap_or_else(|| home.join(".claude"));

        let codex_dir = env_path(["MUDI_CODEX_DIR"]).unwrap_or_else(|| home.join(".codex"));

        let data_dir = match env_path(["MUDI_DATA_DIR"]) {
            Some(path) => path,
            None => {
                let data_root = dirs::data_dir().unwrap_or_else(|| home.join(".local/share"));
                data_root.join("mudi")
            }
        };

        let extra_dirs = env_value(["MUDI_EXTRA_DIRS"])
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

    /// Durable Managed Launch Registrations, separate from the disposable Session index.
    pub fn managed_launch_store_path(&self) -> PathBuf {
        self.data_dir.join("managed-launches.db")
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

fn env_path<I, S>(keys: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    configured_path(keys)
}

pub(crate) fn configured_path<I, S>(keys: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let configured = keys
        .into_iter()
        .filter_map(|key| {
            let key = key.as_ref().to_string();
            std::env::var_os(&key)
                .filter(|value| !value.is_empty())
                .map(|value| (key, value))
        })
        .collect::<Vec<_>>();
    let (selected_key, selected_value) = configured.first()?;
    for (ignored_key, ignored_value) in configured.iter().skip(1) {
        if ignored_value != selected_value {
            eprintln!("warning: {selected_key} takes precedence over deprecated {ignored_key}");
        }
    }
    Some(PathBuf::from(selected_value))
}

fn env_value<I, S>(keys: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let configured = keys
        .into_iter()
        .filter_map(|key| {
            let key = key.as_ref().to_string();
            std::env::var(&key)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (key, value))
        })
        .collect::<Vec<_>>();
    let (selected_key, selected_value) = configured.first()?;
    for (ignored_key, ignored_value) in configured.iter().skip(1) {
        if ignored_value != selected_value {
            eprintln!("warning: {selected_key} takes precedence over deprecated {ignored_key}");
        }
    }
    Some(selected_value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_store_lives_in_the_mudi_data_directory() {
        let config = Config {
            claude_dir: PathBuf::from("/claude"),
            codex_dir: PathBuf::from("/codex"),
            data_dir: PathBuf::from("/mudi-data"),
            extra_dirs: Vec::new(),
        };

        assert_eq!(
            config.label_store_path(),
            PathBuf::from("/mudi-data/session-labels.json")
        );
    }

    #[test]
    fn model_catalog_override_lives_in_the_mudi_data_directory() {
        let config = Config {
            claude_dir: PathBuf::from("/claude"),
            codex_dir: PathBuf::from("/codex"),
            data_dir: PathBuf::from("/mudi-data"),
            extra_dirs: Vec::new(),
        };

        assert_eq!(
            config.model_catalog_override_path(),
            PathBuf::from("/mudi-data/models.json")
        );
    }

    #[test]
    fn task_store_and_artifacts_are_separate_from_the_disposable_index() {
        let config = Config {
            claude_dir: PathBuf::from("/claude"),
            codex_dir: PathBuf::from("/codex"),
            data_dir: PathBuf::from("/mudi-data"),
            extra_dirs: Vec::new(),
        };

        assert_eq!(
            config.task_store_path(),
            PathBuf::from("/mudi-data/tasks.db")
        );
        assert_eq!(
            config.artifact_root(),
            PathBuf::from("/mudi-data/handoff-artifacts")
        );
        assert_ne!(config.task_store_path(), config.index_path());
        assert_eq!(
            config.managed_launch_store_path(),
            PathBuf::from("/mudi-data/managed-launches.db")
        );
        assert_ne!(config.managed_launch_store_path(), config.index_path());
        assert_ne!(config.managed_launch_store_path(), config.task_store_path());
    }

    #[test]
    fn mudi_environment_variables_take_priority_over_sessionmesh_and_tokenplay_values() {
        let suffix = std::process::id();
        let canonical = format!("MUDI_TEST_CANONICAL_{suffix}");
        let legacy = format!("MUDI_TEST_SESSIONMESH_{suffix}");
        let oldest = format!("MUDI_TEST_TOKENPLAY_{suffix}");
        std::env::set_var(&oldest, "/tokenplay");

        assert_eq!(
            env_path([canonical.as_str(), legacy.as_str(), oldest.as_str()]),
            Some(PathBuf::from("/tokenplay"))
        );

        std::env::set_var(&legacy, "/sessionmesh");
        assert_eq!(
            env_path([canonical.as_str(), legacy.as_str(), oldest.as_str()]),
            Some(PathBuf::from("/sessionmesh"))
        );

        std::env::set_var(&canonical, "/mudi");
        assert_eq!(
            env_path([canonical.as_str(), legacy.as_str(), oldest.as_str()]),
            Some(PathBuf::from("/mudi"))
        );

        std::env::remove_var(canonical);
        std::env::remove_var(legacy);
        std::env::remove_var(oldest);
    }
}
