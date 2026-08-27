//! Persistent, Mudi-owned metadata for provider-native Sessions.

use crate::adapters::persistence::state::StateDatabase;
use crate::domain::{Provider, SessionIdentity};
use anyhow::{anyhow, Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionKey {
    provider: Provider,
    native_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurationDisposition {
    Hidden,
    PermanentlyExcluded,
}

impl CurationDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::PermanentlyExcluded => "permanently_excluded",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "hidden" => Ok(Self::Hidden),
            "permanently_excluded" => Ok(Self::PermanentlyExcluded),
            _ => Err(anyhow!("unknown Session curation disposition {value:?}")),
        }
    }
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
    /// Load Session metadata from the authoritative State Database.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let (metadata, load_error) = match load_metadata(&path) {
            Ok(metadata) => (metadata, None),
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

    /// Persist a launch-associated Alias only when the Session has no Alias yet.
    /// Returns whether this call established the Alias.
    pub fn associate_alias(
        &mut self,
        provider: Provider,
        native_id: &str,
        alias: &str,
    ) -> std::io::Result<bool> {
        if let Some(error) = &self.load_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let mut next = self.latest_metadata()?;
        let key = SessionKey {
            provider,
            native_id: native_id.to_string(),
        };
        reject_permanent_exclusion(&next, &key)?;
        if next.aliases.contains_key(&key) {
            self.publish(next);
            return Ok(false);
        }
        next.aliases.insert(key, validate_alias(alias)?);
        self.save(&next)?;
        self.publish(next);
        Ok(true)
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

    /// Replace one Session Description without changing its Alias.
    pub(crate) fn set_description(
        &mut self,
        provider: Provider,
        native_id: &str,
        description: &str,
    ) -> std::io::Result<()> {
        if let Some(error) = &self.load_error {
            return Err(std::io::Error::other(error.clone()));
        }
        let description = validate_description(description)?;
        let key = SessionKey {
            provider,
            native_id: native_id.to_string(),
        };
        let mut next = self.latest_metadata()?;
        reject_permanent_exclusion(&next, &key)?;
        replace_optional(&mut next.descriptions, key, description);
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
        load_metadata(path).map_err(std::io::Error::other)
    }

    fn save(&self, metadata: &SessionMetadata) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        save_metadata(path, metadata).map_err(std::io::Error::other)
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

fn load_metadata(path: &Path) -> Result<SessionMetadata> {
    let database = StateDatabase::open(path)?;
    let connection = database.connect()?;
    let mut statement = connection.prepare(
        "SELECT provider, native_session_id, alias, description, disposition \
         FROM session_metadata",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut metadata = SessionMetadata::default();
    for (provider, native_id, alias, description, disposition) in rows {
        let provider = Provider::parse(&provider)
            .ok_or_else(|| anyhow!("unknown Session Metadata Provider {provider:?}"))?;
        let key = SessionKey {
            provider,
            native_id,
        };
        if let Some(alias) = alias {
            metadata.aliases.insert(
                key.clone(),
                validate_alias(&alias).context("invalid stored Session Alias")?,
            );
        }
        if let Some(description) = description {
            let description = validate_description(&description)
                .context("invalid stored Session Description")?
                .ok_or_else(|| anyhow!("stored Session Description cannot be empty"))?;
            metadata.descriptions.insert(key.clone(), description);
        }
        if let Some(disposition) = disposition {
            metadata
                .curation
                .insert(key, CurationDisposition::parse(&disposition)?);
        }
    }
    Ok(metadata)
}

fn save_metadata(path: &Path, metadata: &SessionMetadata) -> Result<()> {
    let database = StateDatabase::open(path)?;
    let mut connection = database.connect()?;
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM session_metadata", [])?;

    let keys = metadata
        .aliases
        .keys()
        .chain(metadata.descriptions.keys())
        .chain(metadata.curation.keys())
        .cloned()
        .collect::<HashSet<_>>();
    for key in keys {
        let alias = metadata.aliases.get(&key).map(String::as_str);
        let description = metadata.descriptions.get(&key).map(String::as_str);
        let disposition = metadata
            .curation
            .get(&key)
            .map(|disposition| disposition.as_str());
        transaction.execute(
            "INSERT INTO session_metadata (provider, native_session_id, alias, description, disposition) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                key.provider.as_str(),
                key.native_id,
                alias,
                description,
                disposition,
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}
