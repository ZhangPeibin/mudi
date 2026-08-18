//! Project Tree stable identities, row model, grouping, and ordering.

#[cfg(test)]
use super::super::{current_attention, snapshot_now_ms};
use super::super::{is_system_temporary_cwd, RECENT_SESSION_WINDOW_MS};
use crate::application::attention::AttentionSnapshot;
use crate::domain::{Project, ProjectIdentity, Session, SessionIdentity};
use crate::presentation::app::Snapshot;
use std::collections::{HashMap, HashSet};

/// Per-application-run Project ordering. Initial Projects are ranked by activity;
/// Projects discovered later are inserted ahead of the retained ranks.
#[derive(Clone, Default)]
pub(in crate::presentation::tui) struct ProjectOrder {
    ranks: HashMap<ProjectIdentity, i64>,
    next_new_rank: i64,
    initialized: bool,
}

impl ProjectOrder {
    pub(in crate::presentation::tui) fn observe(&mut self, sessions: &[Session]) {
        let mut projects = project_ordering_candidates(sessions);
        if !self.initialized {
            projects.sort_by(compare_initial_projects);
            for (rank, project) in projects.into_iter().enumerate() {
                self.ranks
                    .insert(project.project.identity.clone(), rank as i64);
            }
            self.next_new_rank = -1;
            self.initialized = true;
            return;
        }

        projects.retain(|project| !self.ranks.contains_key(&project.project.identity));
        projects.sort_by(compare_initial_projects);
        let first_rank = self.next_new_rank - projects.len() as i64 + 1;
        for (offset, project) in projects.into_iter().enumerate() {
            self.ranks
                .insert(project.project.identity.clone(), first_rank + offset as i64);
        }
        self.next_new_rank = first_rank - 1;
    }

    pub(in crate::presentation::tui) fn compare(
        &self,
        left: &Project,
        right: &Project,
    ) -> std::cmp::Ordering {
        match (
            self.ranks.get(&left.identity),
            self.ranks.get(&right.identity),
        ) {
            (Some(left_rank), Some(right_rank)) => left_rank.cmp(right_rank),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => compare_project_identity(left, right),
        }
    }
}

struct ProjectOrderingCandidate<'a> {
    project: &'a Project,
    latest_activity_ms: i64,
}

fn project_ordering_candidates(sessions: &[Session]) -> Vec<ProjectOrderingCandidate<'_>> {
    let mut projects: Vec<ProjectOrderingCandidate<'_>> = Vec::new();
    for session in sessions {
        if is_system_temporary_cwd(session.cwd.as_deref()) {
            continue;
        }
        let Some(project) = session.project.as_ref() else {
            continue;
        };
        if let Some(existing) = projects
            .iter_mut()
            .find(|existing| existing.project.identity == project.identity)
        {
            existing.latest_activity_ms = existing.latest_activity_ms.max(session.last_activity_ms);
        } else {
            projects.push(ProjectOrderingCandidate {
                project,
                latest_activity_ms: session.last_activity_ms,
            });
        }
    }
    projects
}

fn compare_initial_projects(
    left: &ProjectOrderingCandidate<'_>,
    right: &ProjectOrderingCandidate<'_>,
) -> std::cmp::Ordering {
    right
        .latest_activity_ms
        .cmp(&left.latest_activity_ms)
        .then_with(|| compare_project_identity(left.project, right.project))
}

fn compare_project_identity(left: &Project, right: &Project) -> std::cmp::Ordering {
    left.label.cmp(&right.label).then_with(|| {
        left.identity
            .display_path()
            .cmp(right.identity.display_path())
    })
}

/// Time bucket used by historical Session disclosures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum SessionAgeBand {
    Recent,
    Older,
}

/// Stable identity for every selectable Project Tree node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum ProjectTreeKey {
    Project(ProjectIdentity),
    Disclosure {
        project: Option<ProjectIdentity>,
        band: SessionAgeBand,
    },
    Session(SessionIdentity),
}

impl ProjectTreeKey {
    /// Historical groups default closed; Projects default open.
    pub(super) fn uses_disclosure_state(&self) -> bool {
        matches!(self, Self::Disclosure { .. })
    }
}

/// All 页面实际可见的一行；纯展示信息之外还保留稳定身份和父子关系。
pub(super) enum ProjectTreeRow<'a> {
    Project {
        key: ProjectTreeKey,
        parent: Option<ProjectTreeKey>,
        project: &'a Project,
        session_count: usize,
        expanded: bool,
        depth: usize,
    },
    Disclosure {
        key: ProjectTreeKey,
        parent: ProjectTreeKey,
        band: SessionAgeBand,
        session_count: usize,
        expanded: bool,
        depth: usize,
    },
    Session {
        key: ProjectTreeKey,
        parent: ProjectTreeKey,
        session: &'a Session,
        depth: usize,
    },
}

impl ProjectTreeRow<'_> {
    pub(super) fn key(&self) -> &ProjectTreeKey {
        match self {
            Self::Project { key, .. }
            | Self::Disclosure { key, .. }
            | Self::Session { key, .. } => key,
        }
    }

    pub(super) fn session_key(&self) -> Option<ProjectTreeKey> {
        matches!(self, Self::Session { .. }).then(|| self.key().clone())
    }

    pub(super) fn parent_key(&self) -> Option<&ProjectTreeKey> {
        match self {
            Self::Project { parent, .. } => parent.as_ref(),
            Self::Disclosure { parent, .. } | Self::Session { parent, .. } => Some(parent),
        }
    }

    pub(super) fn depth(&self) -> usize {
        match self {
            Self::Project { depth, .. } => *depth,
            Self::Disclosure { depth, .. } | Self::Session { depth, .. } => *depth,
        }
    }

    pub(super) fn is_group(&self) -> bool {
        !matches!(self, Self::Session { .. })
    }

    pub(super) fn is_expanded(&self) -> bool {
        match self {
            Self::Project { expanded, .. } | Self::Disclosure { expanded, .. } => *expanded,
            Self::Session { .. } => false,
        }
    }
}

struct HistoricalDisclosure<'bucket, 'session> {
    project: Option<ProjectIdentity>,
    band: SessionAgeBand,
    parent: ProjectTreeKey,
    sessions: &'bucket [&'session Session],
    depth: usize,
}

/// 按 Project 最近活动时间和 Session Attention 建立应用折叠状态后的 Tree 行。
#[cfg(test)]
pub(super) fn rows<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
    now_ms: i64,
    collapsed: &HashSet<ProjectTreeKey>,
    expanded_disclosures: &HashSet<ProjectTreeKey>,
) -> Vec<ProjectTreeRow<'a>> {
    let mut project_order = ProjectOrder::default();
    project_order.observe(sessions);
    rows_with_order(
        sessions,
        attention,
        now_ms,
        collapsed,
        expanded_disclosures,
        &project_order,
    )
}

pub(super) fn rows_with_order<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
    now_ms: i64,
    collapsed: &HashSet<ProjectTreeKey>,
    expanded_disclosures: &HashSet<ProjectTreeKey>,
    project_order: &ProjectOrder,
) -> Vec<ProjectTreeRow<'a>> {
    let mut rows = Vec::new();
    let inventory = grouped_sessions(sessions, attention, now_ms, project_order);
    for project in inventory.current_projects {
        append_project(&mut rows, project, None, 0, collapsed, expanded_disclosures);
    }

    rows
}

fn append_project<'session>(
    rows: &mut Vec<ProjectTreeRow<'session>>,
    project: ProjectGroup<'session>,
    parent: Option<ProjectTreeKey>,
    depth: usize,
    collapsed: &HashSet<ProjectTreeKey>,
    expanded_disclosures: &HashSet<ProjectTreeKey>,
) {
    let ProjectGroup {
        project, sessions, ..
    } = project;
    let project_identity = project.identity.clone();
    let project_key = ProjectTreeKey::Project(project_identity.clone());
    let project_expanded = !collapsed.contains(&project_key);
    let session_count = sessions.len();
    rows.push(ProjectTreeRow::Project {
        key: project_key.clone(),
        parent,
        project,
        session_count,
        expanded: project_expanded,
        depth,
    });
    if !project_expanded {
        return;
    }

    append_session_buckets(
        rows,
        sessions,
        Some(project_identity),
        project_key,
        depth + 1,
        expanded_disclosures,
    );
}

fn append_session_buckets<'session>(
    rows: &mut Vec<ProjectTreeRow<'session>>,
    buckets: SessionBuckets<'session>,
    project: Option<ProjectIdentity>,
    parent: ProjectTreeKey,
    depth: usize,
    expanded_disclosures: &HashSet<ProjectTreeKey>,
) {
    rows.extend(
        buckets
            .open
            .into_iter()
            .map(|session| ProjectTreeRow::Session {
                key: session_tree_key(session),
                parent: parent.clone(),
                session,
                depth,
            }),
    );
    append_disclosure(
        rows,
        HistoricalDisclosure {
            project: project.clone(),
            band: SessionAgeBand::Recent,
            parent: parent.clone(),
            sessions: &buckets.recent,
            depth,
        },
        expanded_disclosures,
    );
    append_disclosure(
        rows,
        HistoricalDisclosure {
            project,
            band: SessionAgeBand::Older,
            parent,
            sessions: &buckets.older,
            depth,
        },
        expanded_disclosures,
    );
}

fn append_disclosure<'session>(
    rows: &mut Vec<ProjectTreeRow<'session>>,
    disclosure: HistoricalDisclosure<'_, 'session>,
    expanded_disclosures: &HashSet<ProjectTreeKey>,
) {
    let HistoricalDisclosure {
        project,
        band,
        parent,
        sessions,
        depth,
    } = disclosure;
    if sessions.is_empty() {
        return;
    }
    let key = ProjectTreeKey::Disclosure { project, band };
    let expanded = expanded_disclosures.contains(&key);
    rows.push(ProjectTreeRow::Disclosure {
        key: key.clone(),
        parent,
        band,
        session_count: sessions.len(),
        expanded,
        depth,
    });
    if expanded {
        rows.extend(
            sessions
                .iter()
                .copied()
                .map(|session| ProjectTreeRow::Session {
                    key: session_tree_key(session),
                    parent: key.clone(),
                    session,
                    depth: depth + 1,
                }),
        );
    }
}

fn session_tree_key(session: &Session) -> ProjectTreeKey {
    ProjectTreeKey::Session(SessionIdentity::from(session))
}

/// 返回可解析 Project 总数和这些 Project 下的 Open Session 总数。
pub(super) fn tree_counts(snap: &Snapshot) -> (usize, usize) {
    let project_count = snap
        .sessions
        .iter()
        .filter(|session| !is_system_temporary_cwd(session.cwd.as_deref()))
        .filter(|session| session.state.is_open())
        .filter_map(|session| session.project.as_ref())
        .map(|project| &project.identity)
        .collect::<HashSet<_>>()
        .len();
    let open_session_count = snap
        .sessions
        .iter()
        .filter(|session| !is_system_temporary_cwd(session.cwd.as_deref()))
        .filter(|session| session.project.is_some())
        .filter(|session| session.state.is_open())
        .count();
    (project_count, open_session_count)
}

/// 按首页 Attention 顺序返回已解析到 Project 的 Open Session。
#[cfg(test)]
pub(super) fn sessions(snap: &Snapshot) -> Vec<&Session> {
    let attention = current_attention(snap);
    let collapsed = HashSet::new();
    rows(
        &snap.sessions,
        &attention,
        snapshot_now_ms(snap),
        &collapsed,
        &HashSet::new(),
    )
    .into_iter()
    .filter_map(|row| match row {
        ProjectTreeRow::Session { session, .. } => Some(session),
        _ => None,
    })
    .collect()
}

#[derive(Default)]
struct SessionBuckets<'a> {
    open: Vec<&'a Session>,
    recent: Vec<&'a Session>,
    older: Vec<&'a Session>,
}

impl<'a> SessionBuckets<'a> {
    fn push(&mut self, session: &'a Session, bucket: SessionBucket) {
        match bucket {
            SessionBucket::Open => self.open.push(session),
            SessionBucket::Recent => self.recent.push(session),
            SessionBucket::Older => self.older.push(session),
        }
    }

    fn len(&self) -> usize {
        self.open.len() + self.recent.len() + self.older.len()
    }

    fn sort(&mut self, attention: &AttentionSnapshot) {
        self.open
            .sort_by(|left, right| attention.compare_sessions(left, right));
        self.recent
            .sort_by_key(|session| std::cmp::Reverse(session.last_activity_ms));
        self.older
            .sort_by_key(|session| std::cmp::Reverse(session.last_activity_ms));
    }
}

#[derive(Clone, Copy)]
enum SessionBucket {
    Open,
    Recent,
    Older,
}

struct ProjectGroup<'a> {
    project: &'a Project,
    sessions: SessionBuckets<'a>,
    has_open_session: bool,
}

struct ProjectInventory<'a> {
    current_projects: Vec<ProjectGroup<'a>>,
}

/// 将已解析 Project 的 Session 分组。Project 列表使用本次运行固定的 rank；最新
/// 活动只决定 Current/Older 分区，避免同时活跃的 Project 随每次刷新重排。
fn grouped_sessions<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
    now_ms: i64,
    project_order: &ProjectOrder,
) -> ProjectInventory<'a> {
    let cutoff_ms = now_ms - RECENT_SESSION_WINDOW_MS;
    let mut projects: Vec<ProjectGroup<'_>> = Vec::new();

    for session in sessions {
        if is_system_temporary_cwd(session.cwd.as_deref()) {
            continue;
        }
        let Some(resolved_project) = session.project.as_ref() else {
            continue;
        };
        let project_index = projects
            .iter()
            .position(|group| group.project.identity == resolved_project.identity)
            .unwrap_or_else(|| {
                projects.push(ProjectGroup {
                    project: resolved_project,
                    sessions: SessionBuckets::default(),
                    has_open_session: false,
                });
                projects.len() - 1
            });
        let project = &mut projects[project_index];
        project.has_open_session |= session.state.is_open();

        project
            .sessions
            .push(session, session_bucket(session, cutoff_ms));
    }

    for project in &mut projects {
        project.sessions.sort(attention);
    }
    projects.sort_by(|left, right| project_order.compare(left.project, right.project));
    projects.retain(|project| project.has_open_session);
    ProjectInventory {
        current_projects: projects,
    }
}

fn session_bucket(session: &Session, cutoff_ms: i64) -> SessionBucket {
    if session.state.is_open() {
        SessionBucket::Open
    } else if session.last_activity_ms >= cutoff_ms {
        SessionBucket::Recent
    } else {
        SessionBucket::Older
    }
}
