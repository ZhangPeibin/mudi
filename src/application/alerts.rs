//! Alert engine: context-threshold crossings + macOS notifications.
//!
//! Only open (Busy/Idle) sessions contribute. Notify once when crossing Warning (70%)
//! and once when crossing Critical (85%). Reset when context drops below a threshold —
//! including after Compaction, which lowers the raw context.

use crate::adapters::config::{CRIT_PCT, WARN_PCT};
use crate::application::session_labels::SessionLabels;
use crate::domain::{Provider, Session};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

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
#[derive(Default)]
pub struct AlertEngine {
    seen: HashMap<(Provider, String), Level>,
    labels: SessionLabels,
    /// When false, notifications are computed but not delivered (tests).
    deliver: bool,
}

impl AlertEngine {
    pub fn new(label_store_path: impl AsRef<Path>) -> Self {
        AlertEngine {
            seen: HashMap::new(),
            labels: SessionLabels::load(label_store_path),
            deliver: true,
        }
    }

    #[cfg(test)]
    fn silent() -> Self {
        AlertEngine {
            seen: HashMap::new(),
            labels: SessionLabels::default(),
            deliver: false,
        }
    }

    #[cfg(test)]
    fn silent_with_labels(labels: SessionLabels) -> Self {
        AlertEngine {
            seen: HashMap::new(),
            labels,
            deliver: false,
        }
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
                let short_id = SessionLabels::short_id(&s.id);
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
                if self.deliver {
                    notify(&msg);
                }
                messages.push(msg);
            }

            self.seen.insert(key, level);
        }

        messages
    }
}

/// Post a macOS notification via `osascript`. Best-effort; failures are ignored.
fn notify(message: &str) {
    let script = format!("display notification {:?} with title \"Mudi\"", message);
    let _ = Command::new("osascript").arg("-e").arg(script).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::session_labels::SessionLabels;
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
        let mut labels = SessionLabels::default();
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
        let directory =
            std::env::temp_dir().join(format!("mudi-alert-title-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("session-labels.json");
        let mut labels = SessionLabels::load(&path);
        labels
            .set_alias(Provider::ClaudeCode, "claude-session", "Before")
            .unwrap();
        let mut engine = AlertEngine::silent_with_labels(labels);
        let session = open_session("claude-session", 180_000, 200_000, SessionState::Busy);
        assert_eq!(engine.update(std::slice::from_ref(&session)).len(), 1);

        SessionLabels::load(&path)
            .set_alias(Provider::ClaudeCode, "claude-session", "After")
            .unwrap();

        assert!(engine.update(&[session]).is_empty());
        std::fs::remove_dir_all(directory).unwrap();
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
