//! Collector: discover source files, drive incremental indexing, and build the
//! normalized snapshot the UI renders.

use crate::config::Config;
use crate::domain::{ModelUsage, Provider, Session};
use crate::index::{fingerprint_of, Index, StoredSession};
use crate::pricing::RateCard;
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

    let mut sessions: Vec<Session> = stored
        .into_iter()
        .map(|s| to_session(s, &detector, rates))
        .collect();

    disambiguate_projects(&mut sessions);
    sessions
}

fn to_session(stored: StoredSession, detector: &StateDetector, rates: &RateCard) -> Session {
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

    let project = basename_of(parsed.cwd.as_deref());

    Session {
        provider: parsed.provider,
        id: parsed.id,
        project,
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

fn basename_of(cwd: Option<&str>) -> String {
    match cwd {
        Some(p) => Path::new(p)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string()),
        None => "—".to_string(),
    }
}

/// When basenames collide across visible sessions, prepend enough parent path to
/// disambiguate (e.g. `dex/backend`).
fn disambiguate_projects(sessions: &mut [Session]) {
    // Group session indices by current basename label.
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, s) in sessions.iter().enumerate() {
        groups.entry(s.project.clone()).or_default().push(i);
    }
    for (_, idxs) in groups {
        if idxs.len() < 2 {
            continue;
        }
        // Extend each colliding label with parent components until unique (or exhausted).
        let mut depth = 2;
        loop {
            let mut labels: HashMap<String, usize> = HashMap::new();
            for &i in &idxs {
                let label = tail_path(sessions[i].cwd.as_deref(), depth);
                *labels.entry(label).or_default() += 1;
            }
            let all_unique = labels.values().all(|&c| c == 1);
            let exhausted = idxs
                .iter()
                .all(|&i| path_depth(sessions[i].cwd.as_deref()) <= depth);
            if all_unique || exhausted {
                for &i in &idxs {
                    sessions[i].project = tail_path(sessions[i].cwd.as_deref(), depth);
                }
                break;
            }
            depth += 1;
        }
    }
}

/// Last `n` path components joined with `/`.
fn tail_path(cwd: Option<&str>, n: usize) -> String {
    match cwd {
        Some(p) => {
            let comps: Vec<&str> = p
                .trim_end_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
                .collect();
            let start = comps.len().saturating_sub(n);
            comps[start..].join("/")
        }
        None => "—".to_string(),
    }
}

fn path_depth(cwd: Option<&str>) -> usize {
    match cwd {
        Some(p) => p
            .trim_end_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .count(),
        None => 1,
    }
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
    use crate::domain::SessionState;

    fn sess(cwd: &str) -> Session {
        Session {
            provider: Provider::ClaudeCode,
            id: cwd.into(),
            project: basename_of(Some(cwd)),
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
        assert_eq!(s[0].project, "dex/backend");
        assert_eq!(s[1].project, "pay/backend");
        assert_eq!(s[2].project, "solo"); // unique, untouched
    }
}
