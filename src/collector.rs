//! Collector: discover source files, drive incremental indexing, and build the
//! normalized snapshot the UI renders.

use crate::config::Config;
use crate::domain::{ModelUsage, ProjectIdentity, Session, SourceMetadata, Usage, UsageSummary};
use crate::index::{fingerprint_of, Index, StoredSession};
use crate::models::ModelCatalog;
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
pub fn build_sessions(cfg: &Config, index: &Index, models: &ModelCatalog) -> Vec<Session> {
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
            to_session(stored, &detector, models, resolved)
        })
        .collect();

    disambiguate_projects(&mut sessions);
    sessions
}

fn to_session(
    stored: StoredSession,
    detector: &StateDetector,
    models: &ModelCatalog,
    resolved: project::ProjectResolution,
) -> Session {
    let StoredSession {
        parsed,
        source_path,
    } = stored;

    let adapter = providers::adapter_for(parsed.provider);
    let live = adapter.live_state(detector, &source_path);
    let state = live.state;
    let mut context = parsed.context;
    context.window = parsed
        .latest_model
        .as_deref()
        .and_then(|model| models.resolve_context_window(parsed.provider, model, context.window));

    let (per_model, cost) = price_models(
        parsed
            .per_model
            .iter()
            .map(|(model, usage)| (model.clone(), *usage)),
        models,
    );
    let current_run_usage = if state.is_open() {
        live.current_run_started_at_ms
            .and_then(|started_at_ms| adapter.current_run_usage(&source_path, started_at_ms))
            .map(|run| price_usage(run, models))
    } else {
        None
    };

    Session {
        provider: parsed.provider,
        id: parsed.id,
        source: SourceMetadata {
            path: source_path,
            format: adapter.source_format(),
        },
        project: resolved.project,
        worktree: resolved.worktree,
        cwd: parsed.cwd,
        latest_model: parsed.latest_model,
        state,
        context,
        usage: parsed.usage,
        per_model,
        current_run_usage,
        compactions: parsed.compactions,
        last_activity_ms: parsed.last_activity_ms,
        cost,
    }
}

fn price_usage(parsed: providers::ParsedUsage, models: &ModelCatalog) -> UsageSummary {
    let (per_model, cost) = price_models(parsed.per_model, models);

    UsageSummary {
        usage: parsed.usage,
        per_model,
        cost,
    }
}

fn price_models(
    usage_by_model: impl IntoIterator<Item = (String, Usage)>,
    models: &ModelCatalog,
) -> (Vec<ModelUsage>, Option<f64>) {
    let mut any_unknown = false;
    let mut total = 0.0;
    let per_model: Vec<ModelUsage> = usage_by_model
        .into_iter()
        .map(|(model, usage)| {
            let cost = models.estimate(&model, &usage);
            match cost {
                Some(value) => total += value,
                None => any_unknown = true,
            }
            ModelUsage { model, usage, cost }
        })
        .collect();
    let cost = if per_model.is_empty() || any_unknown {
        None
    } else {
        Some(total)
    };
    (per_model, cost)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Project, Provider, SessionState};

    fn sess(cwd: &str) -> Session {
        let path = PathBuf::from(cwd);
        Session {
            provider: Provider::ClaudeCode,
            id: cwd.into(),
            source: SourceMetadata::default(),
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
            current_run_usage: None,
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

    #[test]
    fn build_sessions_applies_new_context_window_rules_to_cached_sessions() {
        let cfg = Config {
            claude_dir: PathBuf::from("/missing/claude"),
            codex_dir: PathBuf::from("/missing/codex"),
            data_dir: PathBuf::from("/missing/data"),
            extra_dirs: vec![],
        };
        let mut index = Index::open_in_memory().unwrap();
        let cached = crate::providers::ParsedSession {
            provider: Provider::ClaudeCode,
            id: "cached-proxied-session".into(),
            cwd: None,
            latest_model: Some("gpt-5.6-sol".into()),
            context: crate::domain::ContextState {
                tokens: 378_229,
                window: None,
            },
            usage: Default::default(),
            per_model: vec![],
            compactions: vec![],
            last_activity_ms: 0,
        };
        index
            .upsert(
                &cached,
                Path::new("/logs/cached-proxied-session.jsonl"),
                crate::index::Fingerprint {
                    mtime_ns: 1,
                    size: 2,
                },
            )
            .unwrap();

        let sessions = build_sessions(&cfg, &index, &ModelCatalog::load(None));
        let session = sessions
            .iter()
            .find(|session| session.id == "cached-proxied-session")
            .unwrap();

        assert_eq!(session.context.tokens, 378_229);
        assert_eq!(session.context.window, Some(1_050_000));
    }

    #[test]
    fn open_session_attaches_current_run_usage_from_the_validated_process_start() {
        let started = chrono::Utc::now();
        let started_at_ms = started.timestamp_millis();
        let nonce = started.timestamp_nanos_opt().unwrap();
        let root = std::env::temp_dir().join(format!(
            "sessionmesh-current-run-{}-{nonce}",
            std::process::id()
        ));
        let claude_dir = root.join("claude");
        let transcript_dir = claude_dir.join("projects/project");
        let state_dir = claude_dir.join("sessions");
        std::fs::create_dir_all(&transcript_dir).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();

        let before = (started - chrono::Duration::seconds(1)).to_rfc3339();
        let after = (started + chrono::Duration::seconds(1)).to_rfc3339();
        std::fs::write(
            transcript_dir.join("live-session.jsonl"),
            format!(
                "{{\"type\":\"assistant\",\"timestamp\":\"{before}\",\"message\":{{\"id\":\"old\",\"model\":\"claude-opus-4-8\",\"usage\":{{\"input_tokens\":900,\"output_tokens\":90,\"cache_read_input_tokens\":800,\"cache_creation_input_tokens\":70}}}}}}\n\
                 {{\"type\":\"assistant\",\"timestamp\":\"{after}\",\"message\":{{\"id\":\"run\",\"model\":\"claude-opus-4-8\",\"usage\":{{\"input_tokens\":100,\"output_tokens\":10,\"cache_read_input_tokens\":200,\"cache_creation_input_tokens\":20}}}}}}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            state_dir.join("state.json"),
            format!(
                "{{\"pid\":{},\"sessionId\":\"live-session\",\"status\":\"busy\",\"startedAt\":{started_at_ms}}}",
                std::process::id()
            ),
        )
        .unwrap();
        let cfg = Config {
            claude_dir,
            codex_dir: root.join("codex"),
            data_dir: root.join("data"),
            extra_dirs: vec![],
        };
        let mut index = Index::open_in_memory().unwrap();
        refresh(&cfg, &mut index);

        let sessions = build_sessions(&cfg, &index, &ModelCatalog::load(None));

        let session = sessions
            .iter()
            .find(|session| session.id == "live-session")
            .unwrap();
        assert_eq!(session.state, SessionState::Busy);
        assert_eq!(session.usage.input, 1_000);
        let run = session.current_run_usage.as_ref().unwrap();
        assert_eq!(
            run.usage,
            crate::domain::Usage {
                input: 100,
                output: 10,
                cache_read: 200,
                cache_write: 20,
            }
        );
        assert_eq!(run.per_model.len(), 1);
        assert_eq!(run.per_model[0].model, "claude-opus-4-8");
        assert!((run.cost.unwrap() - 0.000975).abs() < 1e-12);

        std::fs::remove_file(state_dir.join("state.json")).unwrap();
        let exited = build_sessions(&cfg, &index, &ModelCatalog::load(None));
        let exited = exited
            .iter()
            .find(|session| session.id == "live-session")
            .unwrap();
        assert_eq!(exited.state, SessionState::Exited);
        assert_eq!(exited.usage.input, 1_000);
        assert!(exited.current_run_usage.is_none());

        std::fs::remove_dir_all(root).unwrap();
    }
}
