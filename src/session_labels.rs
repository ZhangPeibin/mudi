//! Persistent, SessionMesh-owned metadata for provider-native Sessions.

use crate::domain::{Provider, SessionIdentity};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

const SCHEMA_VERSION: u32 = 3;
const LEGACY_SCHEMA_VERSIONS: [u32; 2] = [1, 2];
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
    #[serde(default)]
    descriptions: Vec<DescriptionRecord>,
    #[serde(default)]
    curation: Vec<CurationRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AliasRecord {
    provider: String,
    session_id: String,
    alias: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DescriptionRecord {
    provider: String,
    session_id: String,
    description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurationDisposition {
    Hidden,
    PermanentlyExcluded,
}

#[derive(Debug, Serialize, Deserialize)]
struct CurationRecord {
    provider: String,
    session_id: String,
    disposition: CurationDisposition,
}

#[derive(Debug, Default)]
struct SessionMetadata {
    aliases: HashMap<SessionKey, String>,
    descriptions: HashMap<SessionKey, String>,
    curation: HashMap<SessionKey, CurationDisposition>,
}

/// Authoritative local Session Alias and Description storage.
#[derive(Debug, Default)]
pub struct SessionLabels {
    path: Option<PathBuf>,
    aliases: HashMap<SessionKey, String>,
    descriptions: HashMap<SessionKey, String>,
    curation: HashMap<SessionKey, CurationDisposition>,
    load_error: Option<String>,
}

impl SessionLabels {
    /// Load a Label Store. A missing file is an empty store.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let (metadata, load_error) = match fs::read(&path) {
            Ok(contents) => match decode(&contents) {
                Ok(metadata) => (metadata, None),
                Err(error) => (
                    SessionMetadata::default(),
                    Some(format!("Label Store error at {}: {error}", path.display())),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (SessionMetadata::default(), None)
            }
            Err(error) => (
                SessionMetadata::default(),
                Some(format!("Label Store error at {}: {error}", path.display())),
            ),
        };
        Self {
            path: Some(path),
            aliases: metadata.aliases,
            descriptions: metadata.descriptions,
            curation: metadata.curation,
            load_error,
        }
    }

    /// Whether persistent Session metadata edits are currently safe.
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

    /// Return the user-authored Description for one Session.
    pub fn description(&self, provider: Provider, native_id: &str) -> Option<&str> {
        self.descriptions
            .get(&SessionKey {
                provider,
                native_id: native_id.to_string(),
            })
            .map(String::as_str)
    }

    pub fn disposition(&self, provider: Provider, native_id: &str) -> Option<CurationDisposition> {
        self.curation
            .get(&SessionKey {
                provider,
                native_id: native_id.to_string(),
            })
            .copied()
    }

    pub fn hide_sessions(&mut self, sessions: &[SessionIdentity]) -> std::io::Result<()> {
        if let Some(error) = &self.load_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let mut next = self.latest_metadata()?;
        for session in sessions {
            let key = SessionKey {
                provider: session.provider,
                native_id: session.native_id.clone(),
            };
            if next.curation.get(&key) != Some(&CurationDisposition::PermanentlyExcluded) {
                next.curation.insert(key, CurationDisposition::Hidden);
            }
        }
        self.save(&next)?;
        self.publish(next);
        Ok(())
    }

    pub fn remove_from_hide(&mut self, sessions: &[SessionIdentity]) -> std::io::Result<()> {
        if let Some(error) = &self.load_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let mut next = self.latest_metadata()?;
        for session in sessions {
            let key = SessionKey {
                provider: session.provider,
                native_id: session.native_id.clone(),
            };
            if next.curation.get(&key) == Some(&CurationDisposition::Hidden) {
                next.curation.remove(&key);
            }
        }
        self.save(&next)?;
        self.publish(next);
        Ok(())
    }

    pub fn permanently_exclude_sessions(
        &mut self,
        sessions: &[SessionIdentity],
    ) -> std::io::Result<()> {
        if let Some(error) = &self.load_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let mut next = self.latest_metadata()?;
        for session in sessions {
            let key = SessionKey {
                provider: session.provider,
                native_id: session.native_id.clone(),
            };
            next.aliases.remove(&key);
            next.descriptions.remove(&key);
            next.curation
                .insert(key, CurationDisposition::PermanentlyExcluded);
        }
        self.save(&next)?;
        self.publish(next);
        Ok(())
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
        let mut next = self.latest_metadata()?;
        let key = SessionKey {
            provider,
            native_id: native_id.to_string(),
        };
        reject_permanent_exclusion(&next, &key)?;
        next.aliases.insert(key, alias);
        self.save(&next)?;
        self.publish(next);
        Ok(())
    }

    /// Remove the user-authored Alias for one Session without changing its Description.
    pub fn clear_title(&mut self, provider: Provider, native_id: &str) -> std::io::Result<()> {
        if let Some(error) = &self.load_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let mut next = self.latest_metadata()?;
        next.aliases.remove(&SessionKey {
            provider,
            native_id: native_id.to_string(),
        });
        self.save(&next)?;
        self.publish(next);
        Ok(())
    }

    /// Atomically replace the user-authored Title and Description for one Session.
    /// Empty values remove the corresponding local metadata.
    pub fn set_metadata(
        &mut self,
        provider: Provider,
        native_id: &str,
        title: &str,
        description: &str,
    ) -> std::io::Result<()> {
        if let Some(error) = &self.load_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let title = validate_optional_alias(title)?;
        let description = validate_description(description)?;
        let key = SessionKey {
            provider,
            native_id: native_id.to_string(),
        };
        let mut next = self.latest_metadata()?;
        reject_permanent_exclusion(&next, &key)?;
        replace_optional(&mut next.aliases, key.clone(), title);
        replace_optional(&mut next.descriptions, key, description);
        self.save(&next)?;
        self.publish(next);
        Ok(())
    }

    fn latest_metadata(&self) -> std::io::Result<SessionMetadata> {
        let Some(path) = &self.path else {
            return Ok(SessionMetadata {
                aliases: self.aliases.clone(),
                descriptions: self.descriptions.clone(),
                curation: self.curation.clone(),
            });
        };
        match fs::read(path) {
            Ok(contents) => decode(&contents).map_err(std::io::Error::other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(SessionMetadata::default())
            }
            Err(error) => Err(error),
        }
    }

    fn save(&self, metadata: &SessionMetadata) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            create_private_dir(parent)?;
        }
        let mut aliases = metadata
            .aliases
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
        let mut descriptions = metadata
            .descriptions
            .iter()
            .map(|(key, description)| DescriptionRecord {
                provider: key.provider.as_str().to_string(),
                session_id: key.native_id.clone(),
                description: description.clone(),
            })
            .collect::<Vec<_>>();
        descriptions.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        let mut curation = metadata
            .curation
            .iter()
            .map(|(key, disposition)| CurationRecord {
                provider: key.provider.as_str().to_string(),
                session_id: key.native_id.clone(),
                disposition: *disposition,
            })
            .collect::<Vec<_>>();
        curation.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        let contents = serde_json::to_vec_pretty(&LabelFile {
            version: SCHEMA_VERSION,
            aliases,
            descriptions,
            curation,
        })
        .map_err(std::io::Error::other)?;
        atomic_write_private_file(path, &contents)
    }

    fn publish(&mut self, metadata: SessionMetadata) {
        self.aliases = metadata.aliases;
        self.descriptions = metadata.descriptions;
        self.curation = metadata.curation;
    }
}

fn reject_permanent_exclusion(metadata: &SessionMetadata, key: &SessionKey) -> std::io::Result<()> {
    if metadata.curation.get(key) == Some(&CurationDisposition::PermanentlyExcluded) {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Permanently excluded Session metadata cannot be edited",
        ))
    } else {
        Ok(())
    }
}

fn short_session_id(native_id: &str, length: usize) -> String {
    let mut suffix = native_id.chars().rev().take(length).collect::<Vec<_>>();
    suffix.reverse();
    suffix.into_iter().collect()
}

pub(crate) fn validate_alias(alias: &str) -> std::io::Result<String> {
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

fn validate_optional_alias(alias: &str) -> std::io::Result<Option<String>> {
    if alias.trim().is_empty() {
        Ok(None)
    } else {
        validate_alias(alias).map(Some)
    }
}

pub(crate) fn validate_description(description: &str) -> std::io::Result<Option<String>> {
    let description = description.trim();
    if description.is_empty() {
        return Ok(None);
    }
    if description.chars().any(char::is_control) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Description must be one line without control characters",
        ));
    }
    if description.chars().count() > 240 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Description must be 240 characters or fewer",
        ));
    }
    Ok(Some(description.to_string()))
}

fn replace_optional(
    values: &mut HashMap<SessionKey, String>,
    key: SessionKey,
    value: Option<String>,
) {
    if let Some(value) = value {
        values.insert(key, value);
    } else {
        values.remove(&key);
    }
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

fn decode(contents: &[u8]) -> Result<SessionMetadata, String> {
    let file = serde_json::from_slice::<LabelFile>(contents).map_err(|error| error.to_string())?;
    if file.version != SCHEMA_VERSION && !LEGACY_SCHEMA_VERSIONS.contains(&file.version) {
        return Err(format!(
            "unsupported schema version {} (expected 1, 2, or {SCHEMA_VERSION})",
            file.version
        ));
    }
    let aliases = file
        .aliases
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
        .collect::<Result<HashMap<_, _>, String>>()?;
    let descriptions = file
        .descriptions
        .into_iter()
        .map(|record| {
            let provider = Provider::parse(&record.provider)
                .ok_or_else(|| format!("unknown Provider {:?}", record.provider))?;
            let description = validate_description(&record.description)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Description cannot be empty in the Label Store".to_string())?;
            Ok((
                SessionKey {
                    provider,
                    native_id: record.session_id,
                },
                description,
            ))
        })
        .collect::<Result<HashMap<_, _>, String>>()?;
    let curation = file
        .curation
        .into_iter()
        .map(|record| {
            let provider = Provider::parse(&record.provider)
                .ok_or_else(|| format!("unknown Provider {:?}", record.provider))?;
            Ok((
                SessionKey {
                    provider,
                    native_id: record.session_id,
                },
                record.disposition,
            ))
        })
        .collect::<Result<HashMap<_, _>, String>>()?;
    Ok(SessionMetadata {
        aliases,
        descriptions,
        curation,
    })
}
