//! Collector: discover source files, drive incremental indexing, and build the
//! normalized snapshot the UI renders.

use crate::config::Config;
use crate::domain::{ModelUsage, ProjectIdentity, Provider, Session};
use crate::index::{fingerprint_of, Index, StoredSession};
use crate::pricing::RateCard;
use crate::project;
use crate::providers::state::StateDetector;
use crate::providers::{self, ProviderAdapter};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Progress reported while (re)indexing.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanStats {
    pub discovered: usize,
    pub parsed: usize,
    pub skipped: usize,
    pub pruned: usize,
}

/// Incrementally refresh the index from source: parse changed/new files, skip
/// unchanged ones, and prune sessions whose source vanished. Each provider's
/// adapter owns discovery and parsing; nothing here enumerates providers.
pub fn refresh(cfg: &Config, index: &mut Index) -> ScanStats {
    let mut stats = ScanStats::default();
    let mut present: HashSet<PathBuf> = HashSet::new();

    for adapter in providers::ADAPTERS {
        let files = adapter.discover(cfg);
        stats.discovered += files.len();
        for path in files {
            present.insert(path.clone());
            index_one(index, &path, *adapter, &mut stats);
        }
    }

    stats.pruned = index.prune_missing(&present).unwrap_or(0);
    stats
}

fn index_one(index: &mut Index, path: &Path, adapter: &dyn ProviderAdapter, stats: &mut ScanStats) {
    let Some(fp) = fingerprint_of(path) else {
        return;
    };
    // Skip unchanged files — this is what keeps incremental updates cheap.
    if index.fingerprint(path) == Some(fp) {
        stats.skipped += 1;
        return;
    }
    if let Some(parsed) = adapter.parse_file(path) {
        if index.upsert(&parsed, path, fp).is_ok() {
            stats.parsed += 1;
        }
    }
}

/// Build the normalized session list from the index, attaching live state and cost.
pub fn build_sessions(cfg: &Config, index: &Index, rates: &RateCard) -> Vec<Session> {
    let stored = index.load_all().unwrap_or_default();
    let detector = StateDetector::gather(&cfg.claude_sessions_dir());
    let mut resolutions: HashMap<String, project::ProjectResolution> = HashMap::new();

    let mut sessions: Vec<Session> = stored
        .into_iter()
        .map(|stored| {
            let resolved = stored
                .parsed
                .cwd
                .as_deref()
                .map(|cwd| {
                    resolutions
                        .entry(cwd.to_string())
                        .or_insert_with(|| project::resolve(Some(Path::new(cwd))))
                        .clone()
                })
                .unwrap_or_default();
            to_session(stored, &detector, rates, resolved)
        })
        .collect();

    disambiguate_projects(&mut sessions);
    sessions
}

fn to_session(
    stored: StoredSession,
    detector: &StateDetector,
    rates: &RateCard,
    resolved: project::ProjectResolution,
) -> Session {
    let StoredSession {
        parsed,
        source_path,
    } = stored;

    let state = providers::adapter_for(parsed.provider).live_state(detector, &source_path);

    // Per-model cost; total is None if any contributing model price is unknown.
    let mut any_unknown = false;
    let mut total = 0.0;
    let per_model: Vec<ModelUsage> = parsed
        .per_model
        .iter()
        .map(|(model, usage)| {
            let cost = rates.estimate(model, usage);
            match cost {
                Some(c) => total += c,
                None => any_unknown = true,
            }
            ModelUsage {
                model: model.clone(),
                usage: *usage,
                cost,
            }
        })
        .collect();
    let cost = if per_model.is_empty() || any_unknown {
        None
    } else {
        Some(total)
    };

    Session {
        provider: parsed.provider,
        id: parsed.id,
        project: resolved.project,
        worktree: resolved.worktree,
        cwd: parsed.cwd,
        latest_model: parsed.latest_model,
        state,
        context: parsed.context,
        usage: parsed.usage,
        per_model,
        compactions: parsed.compactions,
        last_activity_ms: parsed.last_activity_ms,
        cost,
    }
}

/// When distinct Project basenames collide, prepend enough canonical parent path
/// to disambiguate them (e.g. `dex/backend`). Sessions in the same Project keep
/// the same label.
fn disambiguate_projects(sessions: &mut [Session]) {
    let mut groups: HashMap<String, HashSet<ProjectIdentity>> = HashMap::new();
    for project in sessions
        .iter()
        .filter_map(|session| session.project.as_ref())
    {
        groups
            .entry(project.label.clone())
            .or_default()
            .insert(project.identity.clone());
    }
    for (_, identities) in groups {
        if identities.len() < 2 {
            continue;
        }

        let mut depth = 2;
        loop {
            let mut labels: HashMap<String, usize> = HashMap::new();
            for identity in &identities {
                let label = tail_path(identity.display_path(), depth);
                *labels.entry(label).or_default() += 1;
            }
            let all_unique = labels.values().all(|&c| c == 1);
            let exhausted = identities
                .iter()
                .all(|identity| path_depth(identity.display_path()) <= depth);
            if all_unique || exhausted {
                let resolved: HashMap<ProjectIdentity, String> = identities
                    .iter()
                    .cloned()
                    .map(|identity| {
                        let label = tail_path(identity.display_path(), depth);
                        (identity, label)
                    })
                    .collect();
                for project in sessions
                    .iter_mut()
                    .filter_map(|session| session.project.as_mut())
                {
                    if let Some(label) = resolved.get(&project.identity) {
                        project.label.clone_from(label);
                    }
                }
                break;
            }
            depth += 1;
        }
    }
}

/// Last `n` path components joined with `/`.
fn tail_path(path: &Path, n: usize) -> String {
    let comps: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let start = comps.len().saturating_sub(n);
    comps[start..].join("/")
}

fn path_depth(path: &Path) -> usize {
    path.components()
        .filter(|component| matches!(component, std::path::Component::Normal(_)))
        .count()
}

/// Sort: Busy first, Idle second, then remaining by latest update (newest first).
pub fn sort_for_display(sessions: &mut [Session]) {
    sessions.sort_by(|a, b| {
        a.state
            .sort_rank()
            .cmp(&b.state.sort_rank())
            .then(b.last_activity_ms.cmp(&a.last_activity_ms))
    });
}

/// Filter to sessions of a provider active within `cutoff_ms` (0 = all time).
pub fn provider_sessions(all: &[Session], provider: Provider, cutoff_ms: i64) -> Vec<Session> {
    let mut v: Vec<Session> = all
        .iter()
        .filter(|s| s.provider == provider && s.last_activity_ms >= cutoff_ms)
        .cloned()
        .collect();
    sort_for_display(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Project, SessionState};

    fn sess(cwd: &str) -> Session {
        let path = PathBuf::from(cwd);
        Session {
            provider: Provider::ClaudeCode,
            id: cwd.into(),
            project: Some(Project {
                label: tail_path(&path, 1),
                identity: ProjectIdentity::CanonicalCwd(path),
            }),
            worktree: None,
            cwd: Some(cwd.into()),
            latest_model: None,
            state: SessionState::Idle,
            context: Default::default(),
            usage: Default::default(),
            per_model: vec![],
            compactions: vec![],
            last_activity_ms: 0,
            cost: None,
        }
    }

    #[test]
    fn disambiguates_colliding_basenames() {
        let mut s = vec![
            sess("/Users/x/dex/backend"),
            sess("/Users/x/pay/backend"),
            sess("/Users/x/solo"),
        ];
        disambiguate_projects(&mut s);
        assert_eq!(s[0].project_label(), "dex/backend");
        assert_eq!(s[1].project_label(), "pay/backend");
        assert_eq!(s[2].project_label(), "solo"); // unique, untouched
    }
}
