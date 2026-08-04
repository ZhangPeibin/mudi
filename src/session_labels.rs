//! Persistent, SessionMesh-owned names for provider-native Sessions.

use crate::domain::Provider;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

const SCHEMA_VERSION: u32 = 1;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionKey {
    provider: Provider,
    native_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct LabelFile {
    version: u32,
    aliases: Vec<AliasRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AliasRecord {
    provider: String,
    session_id: String,
    alias: String,
}

/// Authoritative local Session Alias storage.
#[derive(Debug, Default)]
pub struct SessionLabels {
    path: Option<PathBuf>,
    aliases: HashMap<SessionKey, String>,
    load_error: Option<String>,
}

impl SessionLabels {
    /// Load a Label Store. A missing file is an empty store.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let (aliases, load_error) = match fs::read(&path) {
            Ok(contents) => match decode(&contents) {
                Ok(aliases) => (aliases, None),
                Err(error) => (
                    HashMap::new(),
                    Some(format!("Label Store error at {}: {error}", path.display())),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (HashMap::new(), None),
            Err(error) => (
                HashMap::new(),
                Some(format!("Label Store error at {}: {error}", path.display())),
            ),
        };
        Self {
            path: Some(path),
            aliases,
            load_error,
        }
    }

    /// Whether persistent Alias edits are currently safe.
    pub fn is_editable(&self) -> bool {
        self.load_error.is_none()
    }

    /// Persistent Label Store failure, including its path.
    pub fn error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// Reload the current Label Store, updating either data or degraded state.
    pub fn reload(&mut self) {
        if let Some(path) = self.path.clone() {
            *self = Self::load(path);
        }
    }

    /// Return the user-authored Alias for one Session.
    pub fn alias(&self, provider: Provider, native_id: &str) -> Option<&str> {
        self.aliases
            .get(&SessionKey {
                provider,
                native_id: native_id.to_string(),
            })
            .map(String::as_str)
    }

    /// Resolve one Session's current Title without surrounding Provider context.
    pub fn title(&self, provider: Provider, native_id: &str) -> String {
        self.alias(provider, native_id)
            .map(str::to_string)
            .unwrap_or_else(|| short_session_id(native_id, 8))
    }

    /// Deterministic compact reference for a native Session ID.
    pub fn short_id(native_id: &str) -> String {
        short_session_id(native_id, 8)
    }

    /// Resolve Titles for one visible set, disambiguating duplicate Aliases.
    pub fn titles_for(&self, sessions: &[(Provider, &str)]) -> Vec<String> {
        let short_ids = unique_short_session_ids(sessions);
        let titles = sessions
            .iter()
            .zip(&short_ids)
            .map(|((provider, native_id), short_id)| {
                self.alias(*provider, native_id)
                    .map(str::to_string)
                    .unwrap_or_else(|| short_id.clone())
            })
            .collect::<Vec<_>>();
        let mut counts = HashMap::<String, usize>::new();
        for title in &titles {
            *counts.entry(title.clone()).or_default() += 1;
        }
        sessions
            .iter()
            .zip(titles)
            .zip(short_ids)
            .map(|(((provider, native_id), title), short_id)| {
                if counts.get(title.as_str()).copied().unwrap_or(0) > 1
                    && self.alias(*provider, native_id).is_some()
                {
                    format!("{title} · {short_id}")
                } else {
                    title
                }
            })
            .collect()
    }

    /// Persist one user-authored Alias.
    pub fn set_alias(
        &mut self,
        provider: Provider,
        native_id: &str,
        alias: &str,
    ) -> std::io::Result<()> {
        if let Some(error) = &self.load_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let alias = validate_alias(alias)?;
        let mut next = self.latest_aliases()?;
        next.insert(
            SessionKey {
                provider,
                native_id: native_id.to_string(),
            },
            alias,
        );
        self.save(&next)?;
        self.aliases = next;
        Ok(())
    }

    /// Remove all SessionMesh-owned Labels for one Session.
    pub fn clear_title(&mut self, provider: Provider, native_id: &str) -> std::io::Result<()> {
        if let Some(error) = &self.load_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let mut next = self.latest_aliases()?;
        next.remove(&SessionKey {
            provider,
            native_id: native_id.to_string(),
        });
        self.save(&next)?;
        self.aliases = next;
        Ok(())
    }

    fn latest_aliases(&self) -> std::io::Result<HashMap<SessionKey, String>> {
        let Some(path) = &self.path else {
            return Ok(self.aliases.clone());
        };
        match fs::read(path) {
            Ok(contents) => decode(&contents).map_err(std::io::Error::other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(error) => Err(error),
        }
    }

    fn save(&self, aliases_by_session: &HashMap<SessionKey, String>) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            create_private_dir(parent)?;
        }
        let mut aliases = aliases_by_session
            .iter()
            .map(|(key, alias)| AliasRecord {
                provider: key.provider.as_str().to_string(),
                session_id: key.native_id.clone(),
                alias: alias.clone(),
            })
            .collect::<Vec<_>>();
        aliases.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        let contents = serde_json::to_vec_pretty(&LabelFile {
            version: SCHEMA_VERSION,
            aliases,
        })
        .map_err(std::io::Error::other)?;
        atomic_write_private_file(path, &contents)
    }
}

fn short_session_id(native_id: &str, length: usize) -> String {
    let mut suffix = native_id.chars().rev().take(length).collect::<Vec<_>>();
    suffix.reverse();
    suffix.into_iter().collect()
}

fn validate_alias(alias: &str) -> std::io::Result<String> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Title cannot be empty",
        ));
    }
    if alias.chars().any(char::is_control) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Title must be one line without control characters",
        ));
    }
    if alias.chars().count() > 80 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Title must be 80 characters or fewer",
        ));
    }
    Ok(alias.to_string())
}

fn unique_short_session_ids(sessions: &[(Provider, &str)]) -> Vec<String> {
    let total_lengths = sessions
        .iter()
        .map(|(_, native_id)| native_id.chars().count())
        .collect::<Vec<_>>();
    let mut lengths = total_lengths
        .iter()
        .map(|length| (*length).min(8))
        .collect::<Vec<_>>();

    loop {
        let mut groups = HashMap::<(Provider, String), Vec<usize>>::new();
        for (index, (provider, native_id)) in sessions.iter().enumerate() {
            groups
                .entry((*provider, short_session_id(native_id, lengths[index])))
                .or_default()
                .push(index);
        }
        let mut changed = false;
        for indexes in groups.values().filter(|indexes| indexes.len() > 1) {
            let identities = indexes
                .iter()
                .map(|index| sessions[*index].1)
                .collect::<HashSet<_>>();
            if identities.len() < 2 {
                continue;
            }
            for index in indexes {
                if lengths[*index] < total_lengths[*index] {
                    lengths[*index] += 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    sessions
        .iter()
        .enumerate()
        .map(|(index, (_, native_id))| short_session_id(native_id, lengths[index]))
        .collect()
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)
}

fn atomic_write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("session-labels.json");
    let temporary = parent.join(format!(".{filename}.tmp-{}-{sequence}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        #[cfg(unix)]
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
        fs::rename(&temporary, path)?;
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn decode(contents: &[u8]) -> Result<HashMap<SessionKey, String>, String> {
    let file = serde_json::from_slice::<LabelFile>(contents).map_err(|error| error.to_string())?;
    if file.version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported schema version {} (expected {SCHEMA_VERSION})",
            file.version
        ));
    }
    file.aliases
        .into_iter()
        .map(|record| {
            let provider = Provider::parse(&record.provider)
                .ok_or_else(|| format!("unknown Provider {:?}", record.provider))?;
            let alias = validate_alias(&record.alias).map_err(|error| error.to_string())?;
            Ok((
                SessionKey {
                    provider,
                    native_id: record.session_id,
                },
                alias,
            ))
        })
        .collect()
}
