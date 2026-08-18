//! Normalized Session state and identity.

use super::project::Project;
use super::provider::{Provider, SourceMetadata};
use super::usage::{Compaction, ContextState, ModelUsage, Usage, UsageSummary};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Busy,
    Idle,
    Exited,
}

impl SessionState {
    pub fn sort_rank(self) -> u8 {
        match self {
            SessionState::Busy => 0,
            SessionState::Idle => 1,
            SessionState::Exited => 2,
        }
    }

    pub fn is_open(self) -> bool {
        !matches!(self, SessionState::Exited)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionProvenance {
    #[default]
    Observed,
    Managed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub provider: Provider,
    pub id: String,
    #[serde(default)]
    pub provenance: SessionProvenance,
    pub source: SourceMetadata,
    pub project: Option<Project>,
    pub cwd: Option<String>,
    pub latest_model: Option<String>,
    pub state: SessionState,
    pub context: ContextState,
    pub usage: Usage,
    pub per_model: Vec<ModelUsage>,
    #[serde(default)]
    pub current_run_usage: Option<UsageSummary>,
    pub compactions: Vec<Compaction>,
    pub last_activity_ms: i64,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionIdentity {
    pub provider: Provider,
    pub native_id: String,
}

impl From<&Session> for SessionIdentity {
    fn from(session: &Session) -> Self {
        Self {
            provider: session.provider,
            native_id: session.id.clone(),
        }
    }
}

impl Session {
    pub fn project_label(&self) -> &str {
        self.project
            .as_ref()
            .map_or("—", |project| project.label.as_str())
    }

    pub fn compaction_count(&self) -> usize {
        self.compactions.len()
    }
}
