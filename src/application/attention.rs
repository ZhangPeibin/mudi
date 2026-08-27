//! Derive and order current Attention Conditions from a Session snapshot.

use crate::application::{CRIT_PCT, WARN_PCT};
pub use crate::domain::SessionIdentity;
use crate::domain::{ProjectIdentity, Provider, Session, SessionState};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttentionRank {
    CriticalContext,
    SharedProjectRisk,
    WarningContext,
    Idle,
    HealthyBusy,
}

impl AttentionRank {
    /// Whether the Project Session row presents this rank with an attention marker.
    pub fn shows_marker(self) -> bool {
        matches!(
            self,
            Self::CriticalContext | Self::SharedProjectRisk | Self::WarningContext
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionSeverity {
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttentionCondition {
    ContextAlert {
        severity: AttentionSeverity,
        percentage: f64,
        threshold_percent: u8,
        since_ms: i64,
    },
    SharedProjectRisk {
        project_path: PathBuf,
        severity: AttentionSeverity,
        contributing_sessions: usize,
        since_ms: i64,
    },
    Idle {
        since_ms: i64,
        transition_observed: bool,
    },
}

impl AttentionCondition {
    /// Warning/Critical severity is what makes a condition marker-producing.
    pub fn marker_severity(&self) -> Option<AttentionSeverity> {
        match self {
            Self::ContextAlert { severity, .. } | Self::SharedProjectRisk { severity, .. } => {
                Some(*severity)
            }
            Self::Idle { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedProjectRisk {
    pub project_path: PathBuf,
    pub severity: AttentionSeverity,
    pub contributing_sessions: Vec<SessionIdentity>,
    pub detected_at_ms: i64,
}

#[derive(Debug, Clone)]
struct SessionAttention {
    rank: AttentionRank,
    evidence_strength: EvidenceStrength,
    relevant_ms: i64,
    conditions: Vec<AttentionCondition>,
}

#[derive(Debug, Clone, Copy)]
enum EvidenceStrength {
    ContextPercentage(f64),
    ContributorCount(usize),
    Timestamp(i64),
}

#[derive(Debug, Clone, Copy)]
struct SessionOrder {
    rank: AttentionRank,
    evidence_strength: EvidenceStrength,
    relevant_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct AttentionSnapshot {
    sessions: HashMap<SessionIdentity, SessionAttention>,
    shared_project_risks: Vec<SharedProjectRisk>,
}

impl AttentionSnapshot {
    pub fn shared_project_risks(&self) -> &[SharedProjectRisk] {
        &self.shared_project_risks
    }

    pub fn shared_project_risk(&self, project_path: &Path) -> Option<&SharedProjectRisk> {
        self.shared_project_risks
            .iter()
            .find(|risk| risk.project_path == project_path)
    }

    pub fn conditions_for(&self, session: &Session) -> &[AttentionCondition] {
        self.sessions
            .get(&SessionIdentity::from(session))
            .map_or(&[], |attention| attention.conditions.as_slice())
    }

    pub fn rank_for(&self, session: &Session) -> AttentionRank {
        self.order_for(session).rank
    }

    pub fn ordered_open_sessions<'a>(&self, sessions: &'a [Session]) -> Vec<&'a Session> {
        let mut ordered = sessions
            .iter()
            .filter(|session| session.state.is_open())
            .collect::<Vec<_>>();
        ordered.sort_by(|left, right| self.compare_sessions(left, right));
        ordered
    }

    pub fn compare_sessions(&self, left: &Session, right: &Session) -> Ordering {
        let left_attention = self.order_for(left);
        let right_attention = self.order_for(right);

        left_attention
            .rank
            .cmp(&right_attention.rank)
            .then_with(|| {
                compare_evidence_strength(
                    left_attention.evidence_strength,
                    right_attention.evidence_strength,
                )
            })
            .then_with(|| right_attention.relevant_ms.cmp(&left_attention.relevant_ms))
            .then_with(|| project_path(left).cmp(&project_path(right)))
            .then_with(|| provider_rank(left.provider).cmp(&provider_rank(right.provider)))
            .then_with(|| left.id.cmp(&right.id))
    }

    fn order_for(&self, session: &Session) -> SessionOrder {
        let fallback;
        let attention = match self.sessions.get(&SessionIdentity::from(session)) {
            Some(attention) => attention,
            None => {
                fallback = baseline_attention(session);
                &fallback
            }
        };
        SessionOrder {
            rank: attention.rank,
            evidence_strength: attention.evidence_strength,
            relevant_ms: attention.relevant_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextBand {
    None,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy)]
struct Observation {
    state: SessionState,
    context_band: ContextBand,
    idle_since_ms: Option<i64>,
    context_since_ms: Option<i64>,
}

#[derive(Debug, Default)]
pub struct AttentionEngine {
    observations: HashMap<SessionIdentity, Observation>,
    risk_since_ms: HashMap<ProjectIdentity, i64>,
}

impl AttentionEngine {
    pub fn update(&mut self, sessions: &[Session], observed_at_ms: i64) -> AttentionSnapshot {
        let mut current_observations = HashMap::new();
        for session in sessions {
            let identity = SessionIdentity::from(session);
            let previous = self.observations.get(&identity);
            let context_band = context_band(session);
            let idle_since_ms = match (session.state, previous) {
                (SessionState::Idle, Some(previous)) if previous.state == SessionState::Idle => {
                    previous.idle_since_ms
                }
                (SessionState::Idle, Some(previous)) if previous.state == SessionState::Busy => {
                    Some(observed_at_ms)
                }
                _ => None,
            };
            let context_since_ms = if context_band == ContextBand::None {
                None
            } else {
                match previous {
                    Some(previous) if previous.context_band == context_band => {
                        previous.context_since_ms
                    }
                    Some(_) => Some(observed_at_ms),
                    None => None,
                }
            };
            current_observations.insert(
                identity,
                Observation {
                    state: session.state,
                    context_band,
                    idle_since_ms,
                    context_since_ms,
                },
            );
        }
        self.observations = current_observations;

        let mut project_members: HashMap<ProjectIdentity, HashSet<SessionIdentity>> =
            HashMap::new();
        for session in sessions.iter().filter(|session| session.state.is_open()) {
            let Some(project) = session.project.as_ref() else {
                continue;
            };
            project_members
                .entry(project.identity.clone())
                .or_default()
                .insert(SessionIdentity::from(session));
        }

        let mut current_risk_projects = HashSet::new();
        let mut shared_project_risks = project_members
            .into_iter()
            .filter_map(|(project_identity, members)| {
                if members.len() < 2 {
                    return None;
                }
                current_risk_projects.insert(project_identity.clone());
                let detected_at_ms = *self
                    .risk_since_ms
                    .entry(project_identity.clone())
                    .or_insert(observed_at_ms);
                let mut contributing_sessions = members.into_iter().collect::<Vec<_>>();
                contributing_sessions.sort_by(|left, right| {
                    provider_rank(left.provider)
                        .cmp(&provider_rank(right.provider))
                        .then_with(|| left.native_id.cmp(&right.native_id))
                });
                Some(SharedProjectRisk {
                    project_path: project_identity.display_path().to_path_buf(),
                    severity: AttentionSeverity::Warning,
                    contributing_sessions,
                    detected_at_ms,
                })
            })
            .collect::<Vec<_>>();
        self.risk_since_ms
            .retain(|identity, _| current_risk_projects.contains(identity));
        shared_project_risks.sort_by(|left, right| left.project_path.cmp(&right.project_path));
        let mut snapshot = AttentionSnapshot {
            shared_project_risks,
            ..AttentionSnapshot::default()
        };

        for session in sessions.iter().filter(|session| session.state.is_open()) {
            let identity = SessionIdentity::from(session);
            let risk = session
                .project
                .as_ref()
                .and_then(|project| snapshot.shared_project_risk(project.identity.display_path()))
                .filter(|risk| risk.contributing_sessions.contains(&identity));
            let observation = self
                .observations
                .get(&identity)
                .copied()
                .unwrap_or_else(|| initial_observation(session));
            let attention = session_attention(session, observation, risk);
            snapshot.sessions.insert(identity, attention);
        }

        snapshot
    }
}

fn baseline_attention(session: &Session) -> SessionAttention {
    session_attention(session, initial_observation(session), None)
}

fn initial_observation(session: &Session) -> Observation {
    Observation {
        state: session.state,
        context_band: context_band(session),
        idle_since_ms: None,
        context_since_ms: None,
    }
}

fn context_band(session: &Session) -> ContextBand {
    match session.context.percent() {
        Some(percent) if percent >= CRIT_PCT => ContextBand::Critical,
        Some(percent) if percent >= WARN_PCT => ContextBand::Warning,
        _ => ContextBand::None,
    }
}

fn session_attention(
    session: &Session,
    observation: Observation,
    risk: Option<&SharedProjectRisk>,
) -> SessionAttention {
    let mut conditions = Vec::new();
    if let Some(percentage) = session.context.percent() {
        let context_condition = match observation.context_band {
            ContextBand::Critical => Some(AttentionCondition::ContextAlert {
                severity: AttentionSeverity::Critical,
                percentage,
                threshold_percent: CRIT_PCT as u8,
                since_ms: observation
                    .context_since_ms
                    .unwrap_or(session.last_activity_ms),
            }),
            ContextBand::Warning => Some(AttentionCondition::ContextAlert {
                severity: AttentionSeverity::Warning,
                percentage,
                threshold_percent: WARN_PCT as u8,
                since_ms: observation
                    .context_since_ms
                    .unwrap_or(session.last_activity_ms),
            }),
            ContextBand::None => None,
        };
        if let Some(condition) = context_condition {
            conditions.push(condition);
        }
    }
    if let Some(risk) = risk {
        conditions.push(AttentionCondition::SharedProjectRisk {
            project_path: risk.project_path.clone(),
            severity: risk.severity,
            contributing_sessions: risk.contributing_sessions.len(),
            since_ms: risk.detected_at_ms,
        });
    }
    if session.state == SessionState::Idle {
        conditions.push(AttentionCondition::Idle {
            since_ms: observation
                .idle_since_ms
                .unwrap_or(session.last_activity_ms),
            transition_observed: observation.idle_since_ms.is_some(),
        });
    }
    conditions.sort_by_key(condition_rank);

    let rank = conditions
        .first()
        .map(condition_rank)
        .unwrap_or(AttentionRank::HealthyBusy);
    let (evidence_strength, relevant_ms) = match conditions.first() {
        Some(AttentionCondition::ContextAlert {
            percentage,
            since_ms,
            ..
        }) => (EvidenceStrength::ContextPercentage(*percentage), *since_ms),
        Some(AttentionCondition::SharedProjectRisk {
            contributing_sessions,
            since_ms,
            ..
        }) => (
            EvidenceStrength::ContributorCount(*contributing_sessions),
            *since_ms,
        ),
        Some(AttentionCondition::Idle { since_ms, .. }) => {
            (EvidenceStrength::Timestamp(*since_ms), *since_ms)
        }
        None => (
            EvidenceStrength::Timestamp(session.last_activity_ms),
            session.last_activity_ms,
        ),
    };
    SessionAttention {
        rank,
        evidence_strength,
        relevant_ms,
        conditions,
    }
}

fn condition_rank(condition: &AttentionCondition) -> AttentionRank {
    match condition {
        AttentionCondition::ContextAlert {
            severity: AttentionSeverity::Critical,
            ..
        } => AttentionRank::CriticalContext,
        AttentionCondition::SharedProjectRisk { .. } => AttentionRank::SharedProjectRisk,
        AttentionCondition::ContextAlert {
            severity: AttentionSeverity::Warning,
            ..
        } => AttentionRank::WarningContext,
        AttentionCondition::Idle { .. } => AttentionRank::Idle,
    }
}

fn compare_evidence_strength(left: EvidenceStrength, right: EvidenceStrength) -> Ordering {
    match (left, right) {
        (EvidenceStrength::ContextPercentage(left), EvidenceStrength::ContextPercentage(right)) => {
            right.total_cmp(&left)
        }
        (EvidenceStrength::ContributorCount(left), EvidenceStrength::ContributorCount(right)) => {
            right.cmp(&left)
        }
        (EvidenceStrength::Timestamp(left), EvidenceStrength::Timestamp(right)) => right.cmp(&left),
        _ => Ordering::Equal,
    }
}

fn project_path(session: &Session) -> Option<&std::path::Path> {
    session
        .project
        .as_ref()
        .map(|project| project.identity.display_path())
}

fn provider_rank(provider: Provider) -> u8 {
    match provider {
        Provider::ClaudeCode => 0,
        Provider::CodexCli => 1,
    }
}
