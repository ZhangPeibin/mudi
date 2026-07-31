//! Alert engine: context-threshold crossings + macOS notifications.
//!
//! Only open (Busy/Idle) sessions contribute. Notify once when crossing Warning (70%)
//! and once when crossing Critical (85%). Reset when context drops below a threshold —
//! including after Compaction, which lowers the raw context.

use crate::config::{CRIT_PCT, WARN_PCT};
use crate::domain::{Provider, Session};
use std::collections::HashMap;
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
    /// When false, notifications are computed but not delivered (tests).
    deliver: bool,
}

impl AlertEngine {
    pub fn new() -> Self {
        AlertEngine {
            seen: HashMap::new(),
            deliver: true,
        }
    }

    #[cfg(test)]
    fn silent() -> Self {
        AlertEngine {
            seen: HashMap::new(),
            deliver: false,
        }
    }

    /// Update state from the latest snapshot, emitting notifications for upward
    /// threshold crossings. Returns the messages that were (or would be) delivered.
    pub fn update(&mut self, sessions: &[Session]) -> Vec<String> {
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
                let msg = match level {
                    Level::Crit => format!(
                        "CRITICAL: {} · {} context at {:.0}%",
                        s.provider.label(),
                        s.project_label(),
                        pct
                    ),
                    Level::Warn => format!(
                        "Warning: {} · {} context at {:.0}%",
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
    let script = format!(
        "display notification {:?} with title \"TokenPlay\"",
        message
    );
    let _ = Command::new("osascript").arg("-e").arg(script).spawn();
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
            project: Some(Project {
                identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/p")),
                label: "p".into(),
            }),
            worktree: None,
            cwd: None,
            latest_model: None,
            state,
            context: ContextState {
                tokens,
                window: Some(window),
            },
            usage: Default::default(),
            per_model: vec![],
            compactions: vec![],
            last_activity_ms: 0,
            cost: None,
        }
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
