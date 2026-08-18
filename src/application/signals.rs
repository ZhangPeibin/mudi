//! Process-lifetime records of meaningful Session transitions.

use crate::adapters::config::{CRIT_PCT, WARN_PCT};
use crate::application::attention::SessionIdentity;
use crate::domain::{Compaction, ContextState, ProjectIdentity, Provider, Session, SessionState};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const MAX_RECENT_SIGNALS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecentSignal {
    Session {
        observed_at_ms: i64,
        provider: Provider,
        session_id: String,
        kind: RecentSignalKind,
    },
    Project {
        observed_at_ms: i64,
        project_path: PathBuf,
        change: SharedProjectRiskChange,
        before: Vec<SessionIdentity>,
        after: Vec<SessionIdentity>,
    },
}

impl RecentSignal {
    pub fn observed_at_ms(&self) -> i64 {
        match self {
            RecentSignal::Session { observed_at_ms, .. }
            | RecentSignal::Project { observed_at_ms, .. } => *observed_at_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedProjectRiskChange {
    Detected,
    Cleared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAlertLevel {
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecentSignalKind {
    SessionStateChanged {
        from: SessionState,
        to: SessionState,
    },
    ContextAlertChanged {
        from: Option<ContextAlertLevel>,
        to: Option<ContextAlertLevel>,
        before: ContextState,
        after: ContextState,
    },
    CompactionObserved {
        compaction: Compaction,
        context_before: ContextState,
        context_after: ContextState,
        cleared_alert: Option<ContextAlertLevel>,
    },
}

#[derive(Debug, Clone, Copy)]
struct Observation {
    state: SessionState,
    context: ContextState,
}

type SessionKey = (Provider, String);

#[derive(Debug, Default)]
pub struct RecentSignalsEngine {
    initialized: bool,
    observations: HashMap<SessionKey, Observation>,
    project_members: HashMap<ProjectIdentity, Vec<SessionIdentity>>,
    seen_compactions: HashMap<SessionKey, HashSet<Compaction>>,
    signals: Vec<RecentSignal>,
}

impl RecentSignalsEngine {
    pub fn update(&mut self, sessions: &[Session], observed_at_ms: i64) -> &[RecentSignal] {
        let current = sessions
            .iter()
            .map(|session| {
                (
                    (session.provider, session.id.clone()),
                    Observation {
                        state: session.state,
                        context: session.context,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let project_members = open_project_members(sessions);
        let new_compactions = self.collect_new_compactions(sessions);

        if self.initialized {
            for session in sessions {
                let key = (session.provider, session.id.clone());
                let previous = self.observations.get(&key).copied();
                let compactions = new_compactions.get(&key).map_or(&[][..], Vec::as_slice);
                self.record_changes(session, previous, compactions, observed_at_ms);
            }
            self.record_project_risk_changes(&project_members, observed_at_ms);
        }

        self.observations = current;
        self.project_members = project_members;
        self.initialized = true;
        self.signals.truncate(MAX_RECENT_SIGNALS);
        &self.signals
    }

    fn collect_new_compactions(
        &mut self,
        sessions: &[Session],
    ) -> HashMap<SessionKey, Vec<Compaction>> {
        sessions
            .iter()
            .map(|session| {
                let key = (session.provider, session.id.clone());
                let seen = self.seen_compactions.entry(key.clone()).or_default();
                let mut compactions = session
                    .compactions
                    .iter()
                    .filter(|compaction| seen.insert((*compaction).clone()))
                    .cloned()
                    .collect::<Vec<_>>();
                compactions.sort_by(|left, right| {
                    left.at_ms
                        .cmp(&right.at_ms)
                        .then_with(|| left.trigger.cmp(&right.trigger))
                        .then_with(|| left.pre_tokens.cmp(&right.pre_tokens))
                });
                (key, compactions)
            })
            .collect()
    }

    fn record_changes(
        &mut self,
        session: &Session,
        previous: Option<Observation>,
        compactions: &[Compaction],
        observed_at_ms: i64,
    ) {
        let Some(previous) = previous else {
            for compaction in compactions {
                self.push(
                    session,
                    observed_at_ms,
                    RecentSignalKind::CompactionObserved {
                        compaction: compaction.clone(),
                        context_before: session.context,
                        context_after: session.context,
                        cleared_alert: None,
                    },
                );
            }
            return;
        };

        if meaningful_lifecycle_transition(previous.state, session.state) {
            self.push(
                session,
                observed_at_ms,
                RecentSignalKind::SessionStateChanged {
                    from: previous.state,
                    to: session.state,
                },
            );
        }

        let from = context_alert_level(previous.state, previous.context);
        let to = context_alert_level(session.state, session.context);
        let compound_clear = !compactions.is_empty() && alert_was_cleared(from, to);
        if from != to && !compound_clear {
            self.push(
                session,
                observed_at_ms,
                RecentSignalKind::ContextAlertChanged {
                    from,
                    to,
                    before: previous.context,
                    after: session.context,
                },
            );
        }

        for (index, compaction) in compactions.iter().enumerate() {
            let cleared_alert = (compound_clear && index + 1 == compactions.len())
                .then_some(from)
                .flatten();
            self.push(
                session,
                observed_at_ms,
                RecentSignalKind::CompactionObserved {
                    compaction: compaction.clone(),
                    context_before: previous.context,
                    context_after: session.context,
                    cleared_alert,
                },
            );
        }
    }

    fn push(&mut self, session: &Session, observed_at_ms: i64, kind: RecentSignalKind) {
        self.signals.insert(
            0,
            RecentSignal::Session {
                observed_at_ms,
                provider: session.provider,
                session_id: session.id.clone(),
                kind,
            },
        );
    }

    fn record_project_risk_changes(
        &mut self,
        current: &HashMap<ProjectIdentity, Vec<SessionIdentity>>,
        observed_at_ms: i64,
    ) {
        let mut projects = self
            .project_members
            .keys()
            .chain(current.keys())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| right.display_path().cmp(left.display_path()));

        for project_identity in projects {
            let before = self
                .project_members
                .get(&project_identity)
                .cloned()
                .unwrap_or_default();
            let after = current.get(&project_identity).cloned().unwrap_or_default();
            let change = match (before.len() >= 2, after.len() >= 2) {
                (false, true) => SharedProjectRiskChange::Detected,
                (true, false) => SharedProjectRiskChange::Cleared,
                _ => continue,
            };
            self.signals.insert(
                0,
                RecentSignal::Project {
                    observed_at_ms,
                    project_path: project_identity.display_path().to_path_buf(),
                    change,
                    before,
                    after,
                },
            );
        }
    }
}

fn open_project_members(sessions: &[Session]) -> HashMap<ProjectIdentity, Vec<SessionIdentity>> {
    let mut members = HashMap::<ProjectIdentity, HashSet<SessionIdentity>>::new();
    for session in sessions.iter().filter(|session| session.state.is_open()) {
        let Some(project) = session.project.as_ref() else {
            continue;
        };
        members
            .entry(project.identity.clone())
            .or_default()
            .insert(SessionIdentity::from(session));
    }
    members
        .into_iter()
        .map(|(project_identity, identities)| {
            let mut identities = identities.into_iter().collect::<Vec<_>>();
            identities.sort_by(|left, right| {
                provider_rank(left.provider)
                    .cmp(&provider_rank(right.provider))
                    .then_with(|| left.native_id.cmp(&right.native_id))
            });
            (project_identity, identities)
        })
        .collect()
}

fn provider_rank(provider: Provider) -> u8 {
    match provider {
        Provider::ClaudeCode => 0,
        Provider::CodexCli => 1,
    }
}

fn meaningful_lifecycle_transition(from: SessionState, to: SessionState) -> bool {
    matches!(
        (from, to),
        (SessionState::Busy, SessionState::Idle)
            | (
                SessionState::Busy | SessionState::Idle,
                SessionState::Exited
            )
            | (
                SessionState::Exited,
                SessionState::Busy | SessionState::Idle
            )
    )
}

fn context_alert_level(state: SessionState, context: ContextState) -> Option<ContextAlertLevel> {
    if !state.is_open() {
        return None;
    }
    match context.percent() {
        Some(percent) if percent >= CRIT_PCT => Some(ContextAlertLevel::Critical),
        Some(percent) if percent >= WARN_PCT => Some(ContextAlertLevel::Warning),
        _ => None,
    }
}

fn alert_was_cleared(from: Option<ContextAlertLevel>, to: Option<ContextAlertLevel>) -> bool {
    matches!(
        (from, to),
        (Some(ContextAlertLevel::Warning), None)
            | (
                Some(ContextAlertLevel::Critical),
                None | Some(ContextAlertLevel::Warning)
            )
    )
}
