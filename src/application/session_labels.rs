//! Session Metadata and Curation contracts owned by application workflows.

use crate::domain::{Provider, SessionIdentity};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurationDisposition {
    Hidden,
    PermanentlyExcluded,
}

impl CurationDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::PermanentlyExcluded => "permanently_excluded",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "hidden" => Ok(Self::Hidden),
            "permanently_excluded" => Ok(Self::PermanentlyExcluded),
            _ => anyhow::bail!("unknown Session curation disposition {value:?}"),
        }
    }
}

pub trait SessionMetadataQuery: Send {
    fn is_editable(&self) -> bool;
    fn error(&self) -> Option<&str>;
    fn reload(&mut self);
    fn alias(&self, provider: Provider, native_id: &str) -> Option<&str>;
    fn description(&self, provider: Provider, native_id: &str) -> Option<&str>;
    fn disposition(&self, provider: Provider, native_id: &str) -> Option<CurationDisposition>;
    fn title(&self, provider: Provider, native_id: &str) -> String {
        self.alias(provider, native_id)
            .map(str::to_string)
            .unwrap_or_else(|| short_session_id(native_id))
    }
    fn titles_for(&self, sessions: &[(Provider, &str)]) -> Vec<String> {
        resolve_titles(self, sessions)
    }
}

pub trait SessionMetadataRepository: SessionMetadataQuery {
    fn hide_sessions(&mut self, sessions: &[SessionIdentity]) -> std::io::Result<()>;
    fn remove_from_hide(&mut self, sessions: &[SessionIdentity]) -> std::io::Result<()>;
    fn permanently_exclude_sessions(&mut self, sessions: &[SessionIdentity])
        -> std::io::Result<()>;
    fn set_alias(
        &mut self,
        provider: Provider,
        native_id: &str,
        alias: &str,
    ) -> std::io::Result<()>;
    fn associate_alias(
        &mut self,
        provider: Provider,
        native_id: &str,
        alias: &str,
    ) -> std::io::Result<bool>;
    fn clear_title(&mut self, provider: Provider, native_id: &str) -> std::io::Result<()>;
    fn set_description(
        &mut self,
        provider: Provider,
        native_id: &str,
        description: &str,
    ) -> std::io::Result<()>;
    fn set_metadata(
        &mut self,
        provider: Provider,
        native_id: &str,
        title: &str,
        description: &str,
    ) -> std::io::Result<()>;
}

#[derive(Debug, Default)]
pub struct SessionMetadataView {
    aliases: HashMap<(Provider, String), String>,
    descriptions: HashMap<(Provider, String), String>,
    curation: HashMap<(Provider, String), CurationDisposition>,
    error: Option<String>,
}

impl SessionMetadataView {
    pub fn set_metadata(
        &mut self,
        session: SessionIdentity,
        title: impl Into<String>,
        description: impl Into<String>,
    ) {
        let key = (session.provider, session.native_id);
        self.aliases.insert(key.clone(), title.into());
        self.descriptions.insert(key, description.into());
    }

    pub fn set_disposition(&mut self, session: SessionIdentity, disposition: CurationDisposition) {
        self.curation
            .insert((session.provider, session.native_id), disposition);
    }
}

impl SessionMetadataQuery for SessionMetadataView {
    fn is_editable(&self) -> bool {
        self.error.is_none()
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn reload(&mut self) {}

    fn alias(&self, provider: Provider, native_id: &str) -> Option<&str> {
        self.aliases
            .get(&(provider, native_id.to_string()))
            .map(String::as_str)
    }

    fn description(&self, provider: Provider, native_id: &str) -> Option<&str> {
        self.descriptions
            .get(&(provider, native_id.to_string()))
            .map(String::as_str)
    }

    fn disposition(&self, provider: Provider, native_id: &str) -> Option<CurationDisposition> {
        self.curation
            .get(&(provider, native_id.to_string()))
            .copied()
    }
}

pub fn resolve_titles<R: SessionMetadataQuery + ?Sized>(
    metadata: &R,
    sessions: &[(Provider, &str)],
) -> Vec<String> {
    let short_ids = unique_short_session_ids(sessions);
    let titles = sessions
        .iter()
        .zip(&short_ids)
        .map(|((provider, native_id), short_id)| {
            metadata
                .alias(*provider, native_id)
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
                && metadata.alias(*provider, native_id).is_some()
            {
                format!("{title} · {short_id}")
            } else {
                title
            }
        })
        .collect()
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
                .entry((
                    *provider,
                    short_session_id_with_length(native_id, lengths[index]),
                ))
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
        .map(|(index, (_, native_id))| short_session_id_with_length(native_id, lengths[index]))
        .collect()
}

pub fn short_session_id(native_id: &str) -> String {
    short_session_id_with_length(native_id, 8)
}

pub fn short_session_id_with_length(native_id: &str, length: usize) -> String {
    let mut suffix = native_id.chars().rev().take(length).collect::<Vec<_>>();
    suffix.reverse();
    suffix.into_iter().collect()
}

pub fn validate_alias(alias: &str) -> std::io::Result<String> {
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

pub fn validate_description(description: &str) -> std::io::Result<Option<String>> {
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
