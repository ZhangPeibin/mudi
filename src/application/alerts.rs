//! Alert engine: context-threshold crossings + macOS notifications.
//!
//! Only open (Busy/Idle) sessions contribute. Notify once when crossing Warning (70%)
//! and once when crossing Critical (85%). Reset when context drops below a threshold —
//! including after Compaction, which lowers the raw context.

#[cfg(test)]
use crate::application::session_labels::SessionMetadataQuery;
use crate::application::session_labels::{short_session_id, SessionMetadataRepository};
use crate::application::{CRIT_PCT, WARN_PCT};
use crate::domain::{Provider, Session};
use std::collections::HashMap;

pub trait NotificationPort: Send + Sync {
    fn notify(&self, message: &str);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    None = 0,
    Warn = 1,
    Crit = 2,
}

fn level_for(pct: f64) -> Level {
    if pct >= CRIT_PCT {
        Level::Crit
    } else if pct >= WARN_PCT {
        Level::Warn
    } else {
        Level::None
    }
}

/// Count of open sessions currently at each alert level (for the All dashboard).
#[derive(Debug, Clone, Copy, Default)]
pub struct AlertCounts {
    pub warning: usize,
    pub critical: usize,
}

/// Compute how many open sessions sit at Warning (70–85%) and Critical (85%+).
pub fn counts(sessions: &[Session]) -> AlertCounts {
    let mut c = AlertCounts::default();
    for s in sessions {
        if !s.state.is_open() {
            continue;
        }
        if let Some(pct) = s.context.percent() {
            match level_for(pct) {
                Level::Crit => c.critical += 1,
                Level::Warn => c.warning += 1,
                Level::None => {}
            }
        }
    }
    c
}

/// Tracks per-session alert level so each threshold notifies only once per crossing.
pub struct AlertEngine {
    seen: HashMap<(Provider, String), Level>,
    labels: Box<dyn SessionMetadataRepository>,
    notifications: Box<dyn NotificationPort>,
}

impl AlertEngine {
    pub fn new(
        labels: Box<dyn SessionMetadataRepository>,
        notifications: Box<dyn NotificationPort>,
    ) -> Self {
        AlertEngine {
            seen: HashMap::new(),
            labels,
            notifications,
        }
    }

    #[cfg(test)]
    fn silent_with_labels(labels: impl SessionMetadataRepository + 'static) -> Self {
        Self::new(Box::new(labels), Box::new(SilentNotifications))
    }

    #[cfg(test)]
    fn silent() -> Self {
        Self::silent_with_labels(TestSessionMetadata::default())
    }

    /// Update state from the latest snapshot, emitting notifications for upward
    /// threshold crossings. Returns the messages that were (or would be) delivered.
    pub fn update(&mut self, sessions: &[Session]) -> Vec<String> {
        self.labels.reload();
        let mut messages = Vec::new();

        for s in sessions {
            let key = (s.provider, s.id.clone());

            if !s.state.is_open() {
                // Exited sessions never alert; forget prior state so a future reopen
                // can alert again.
                self.seen.remove(&key);
                continue;
            }

            let Some(pct) = s.context.percent() else {
                self.seen.insert(key, Level::None);
                continue;
            };
            let level = level_for(pct);
            let prev = self.seen.get(&key).copied().unwrap_or(Level::None);

            // Notify only on upward crossings into a higher band.
            if level > prev {
                let title = self.labels.title(s.provider, &s.id);
                let short_id = short_session_id(&s.id);
                let msg = match level {
                    Level::Crit => format!(
                        "CRITICAL: {title} · {} · {} · {short_id} · Current Context {:.0}%",
                        s.provider.label(),
                        s.project_label(),
                        pct
                    ),
                    Level::Warn => format!(
                        "Warning: {title} · {} · {} · {short_id} · Current Context {:.0}%",
                        s.provider.label(),
                        s.project_label(),
                        pct
                    ),
                    Level::None => unreachable!(),
                };
                self.notifications.notify(&msg);
                messages.push(msg);
            }

            self.seen.insert(key, level);
        }

        messages
    }
}

#[cfg(test)]
struct SilentNotifications;

#[cfg(test)]
impl NotificationPort for SilentNotifications {
    fn notify(&self, _message: &str) {}
}

#[cfg(test)]
#[derive(Default)]
struct TestSessionMetadata {
    aliases: HashMap<(Provider, String), String>,
    descriptions: HashMap<(Provider, String), String>,
    curation: HashMap<(Provider, String), crate::application::session_labels::CurationDisposition>,
    external_aliases: Option<SharedAliasMap>,
}

#[cfg(test)]
type AliasMap = HashMap<(Provider, String), String>;

#[cfg(test)]
type SharedAliasMap = std::sync::Arc<std::sync::Mutex<AliasMap>>;

#[cfg(test)]
impl TestSessionMetadata {
    fn reloadable() -> (Self, SharedAliasMap) {
        let aliases = std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()));
        (
            Self {
                external_aliases: Some(aliases.clone()),
                ..Self::default()
            },
            aliases,
        )
    }
}

#[cfg(test)]
impl SessionMetadataQuery for TestSessionMetadata {
    fn is_editable(&self) -> bool {
        true
    }

    fn error(&self) -> Option<&str> {
        None
    }

    fn reload(&mut self) {
        if let Some(external) = &self.external_aliases {
            self.aliases = external
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
        }
    }

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

    fn disposition(
        &self,
        provider: Provider,
        native_id: &str,
    ) -> Option<crate::application::session_labels::CurationDisposition> {
        self.curation
            .get(&(provider, native_id.to_string()))
            .copied()
    }
}

#[cfg(test)]
impl SessionMetadataRepository for TestSessionMetadata {
    fn hide_sessions(
        &mut self,
        sessions: &[crate::domain::SessionIdentity],
    ) -> std::io::Result<()> {
        for session in sessions {
            self.curation.insert(
                (session.provider, session.native_id.clone()),
                crate::application::session_labels::CurationDisposition::Hidden,
            );
        }
        Ok(())
    }

    fn remove_from_hide(
        &mut self,
        sessions: &[crate::domain::SessionIdentity],
    ) -> std::io::Result<()> {
        for session in sessions {
            self.curation
                .remove(&(session.provider, session.native_id.clone()));
        }
        Ok(())
    }

    fn permanently_exclude_sessions(
        &mut self,
        sessions: &[crate::domain::SessionIdentity],
    ) -> std::io::Result<()> {
        for session in sessions {
            self.curation.insert(
                (session.provider, session.native_id.clone()),
                crate::application::session_labels::CurationDisposition::PermanentlyExcluded,
            );
        }
        Ok(())
    }

    fn set_alias(
        &mut self,
        provider: Provider,
        native_id: &str,
        alias: &str,
    ) -> std::io::Result<()> {
        self.aliases
            .insert((provider, native_id.to_string()), alias.to_string());
        Ok(())
    }

    fn associate_alias(
        &mut self,
        provider: Provider,
        native_id: &str,
        alias: &str,
    ) -> std::io::Result<bool> {
        if self.alias(provider, native_id).is_some() {
            return Ok(false);
        }
        self.set_alias(provider, native_id, alias)?;
        Ok(true)
    }

    fn clear_title(&mut self, provider: Provider, native_id: &str) -> std::io::Result<()> {
        self.aliases.remove(&(provider, native_id.to_string()));
        Ok(())
    }

    fn set_description(
        &mut self,
        provider: Provider,
        native_id: &str,
        description: &str,
    ) -> std::io::Result<()> {
        self.descriptions
            .insert((provider, native_id.to_string()), description.to_string());
        Ok(())
    }

    fn set_metadata(
        &mut self,
        provider: Provider,
        native_id: &str,
        title: &str,
        description: &str,
    ) -> std::io::Result<()> {
        self.set_alias(provider, native_id, title)?;
        self.set_description(provider, native_id, description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ContextState, Project, ProjectIdentity, SessionState};
    use std::path::PathBuf;

    fn open_session(id: &str, tokens: u64, window: u64, state: SessionState) -> Session {
        Session {
            provider: Provider::ClaudeCode,
            id: id.into(),
            provenance: crate::domain::SessionProvenance::Observed,
            source: Default::default(),
            project: Some(Project {
                identity: ProjectIdentity::new(PathBuf::from("/p")),
                label: "p".into(),
            }),
            cwd: None,
            latest_model: None,
            state,
            context: ContextState {
                tokens,
                window: Some(window),
            },
            usage: Default::default(),
            per_model: vec![],
            current_run_usage: None,
            compactions: vec![],
            last_activity_ms: 0,
            cost: None,
        }
    }

    #[test]
    fn context_notification_identifies_the_session_with_its_current_title() {
        let mut labels = TestSessionMetadata::default();
        labels
            .set_alias(Provider::ClaudeCode, "claude-session-12345678", "Auth fix")
            .unwrap();
        let mut engine = AlertEngine::silent_with_labels(labels);

        let messages = engine.update(&[open_session(
            "claude-session-12345678",
            180_000,
            200_000,
            SessionState::Busy,
        )]);

        assert_eq!(
            messages,
            vec![
                "CRITICAL: Auth fix · Claude Code · p · 12345678 · Current Context 90%".to_string()
            ]
        );
    }

    #[test]
    fn renaming_a_session_does_not_emit_another_context_notification() {
        let (mut labels, external_aliases) = TestSessionMetadata::reloadable();
        labels
            .set_alias(Provider::ClaudeCode, "claude-session", "Before")
            .unwrap();
        let mut engine = AlertEngine::silent_with_labels(labels);
        let session = open_session("claude-session", 180_000, 200_000, SessionState::Busy);
        assert_eq!(engine.update(std::slice::from_ref(&session)).len(), 1);

        external_aliases.lock().unwrap().insert(
            (Provider::ClaudeCode, "claude-session".into()),
            "After".into(),
        );

        assert!(engine.update(&[session]).is_empty());
    }

    #[test]
    fn notifies_once_per_upward_crossing() {
        let mut e = AlertEngine::silent();
        // 75% → warn once.
        let m1 = e.update(&[open_session("a", 150_000, 200_000, SessionState::Busy)]);
        assert_eq!(m1.len(), 1);
        // Still 75% → no repeat.
        let m2 = e.update(&[open_session("a", 150_000, 200_000, SessionState::Busy)]);
        assert!(m2.is_empty());
        // 90% → crit once.
        let m3 = e.update(&[open_session("a", 180_000, 200_000, SessionState::Busy)]);
        assert_eq!(m3.len(), 1);
        assert!(m3[0].starts_with("CRITICAL"));
    }

    #[test]
    fn resets_after_dropping_below_then_realerts() {
        let mut e = AlertEngine::silent();
        e.update(&[open_session("a", 150_000, 200_000, SessionState::Busy)]); // warn
                                                                              // Drop below (e.g. after compaction).
        let drop = e.update(&[open_session("a", 20_000, 200_000, SessionState::Busy)]);
        assert!(drop.is_empty());
        // Rise again → alerts again.
        let again = e.update(&[open_session("a", 150_000, 200_000, SessionState::Busy)]);
        assert_eq!(again.len(), 1);
    }

    #[test]
    fn exited_sessions_do_not_alert_or_count() {
        let mut e = AlertEngine::silent();
        let m = e.update(&[open_session("a", 190_000, 200_000, SessionState::Exited)]);
        assert!(m.is_empty());
        let c = counts(&[open_session("a", 190_000, 200_000, SessionState::Exited)]);
        assert_eq!(c.critical, 0);
    }

    #[test]
    fn counts_split_warning_and_critical() {
        let sessions = vec![
            open_session("a", 150_000, 200_000, SessionState::Busy), // 75% warn
            open_session("b", 180_000, 200_000, SessionState::Idle), // 90% crit
            open_session("c", 10_000, 200_000, SessionState::Busy),  // 5% none
        ];
        let c = counts(&sessions);
        assert_eq!(c.warning, 1);
        assert_eq!(c.critical, 1);
    }
}
