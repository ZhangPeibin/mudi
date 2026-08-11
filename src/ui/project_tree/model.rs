//! Project Tree stable identities, row model, grouping, and ordering.

use super::super::{current_attention, is_system_temporary_cwd, snapshot_now_ms};
use crate::app::Snapshot;
use crate::attention::AttentionSnapshot;
use crate::domain::{Project, ProjectIdentity, Session, SessionIdentity, Worktree};
use std::collections::HashSet;
use std::path::PathBuf;

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
    Worktree {
        project: ProjectIdentity,
        root: PathBuf,
    },
    Disclosure {
        project: Option<ProjectIdentity>,
        worktree_root: Option<PathBuf>,
        band: SessionAgeBand,
    },
    Session(SessionIdentity),
}

impl ProjectTreeKey {
    /// Historical groups default closed; Projects and Worktrees default open.
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
    Worktree {
        key: ProjectTreeKey,
        parent: ProjectTreeKey,
        worktree: &'a Worktree,
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
            | Self::Worktree { key, .. }
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
            Self::Worktree { parent, .. }
            | Self::Disclosure { parent, .. }
            | Self::Session { parent, .. } => Some(parent),
        }
    }

    pub(super) fn depth(&self) -> usize {
        match self {
            Self::Project { depth, .. } | Self::Worktree { depth, .. } => *depth,
            Self::Disclosure { depth, .. } | Self::Session { depth, .. } => *depth,
        }
    }

    pub(super) fn is_group(&self) -> bool {
        !matches!(self, Self::Session { .. })
    }

    pub(super) fn is_expanded(&self) -> bool {
        match self {
            Self::Project { expanded, .. }
            | Self::Worktree { expanded, .. }
            | Self::Disclosure { expanded, .. } => *expanded,
            Self::Session { .. } => false,
        }
    }
}

struct HistoricalDisclosure<'bucket, 'session> {
    project: Option<ProjectIdentity>,
    worktree_root: Option<PathBuf>,
    band: SessionAgeBand,
    parent: ProjectTreeKey,
    sessions: &'bucket [&'session Session],
    depth: usize,
}

/// 按 Project 最近活动时间和 Session Attention 建立应用折叠状态后的 Tree 行。
///
/// 只有一个 Worktree 且没有 Project 直属 Session 时省略 Worktree 层；多个
/// Worktree 仍保留分组，避免不同工作副本的 Session 混在一起。
pub(super) fn rows<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
    now_ms: i64,
    collapsed: &HashSet<ProjectTreeKey>,
    expanded_disclosures: &HashSet<ProjectTreeKey>,
) -> Vec<ProjectTreeRow<'a>> {
    let mut rows = Vec::new();
    let inventory = grouped_sessions(sessions, attention, now_ms);
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
        project,
        worktrees,
        direct_sessions,
        ..
    } = project;
    let project_identity = project.identity.clone();
    let project_key = ProjectTreeKey::Project(project_identity.clone());
    let project_expanded = !collapsed.contains(&project_key);
    let session_count = direct_sessions.len()
        + worktrees
            .iter()
            .map(|worktree| worktree.sessions.len())
            .sum::<usize>();
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

    let flatten_single_worktree = worktrees.len() == 1 && direct_sessions.is_empty();
    if flatten_single_worktree {
        let worktree = worktrees
            .into_iter()
            .next()
            .expect("one Worktree was counted");
        append_session_buckets(
            rows,
            worktree.sessions,
            Some(project_identity.clone()),
            Some(worktree.worktree.root.clone()),
            project_key.clone(),
            depth + 1,
            expanded_disclosures,
        );
    } else {
        for worktree in worktrees {
            let worktree_key = ProjectTreeKey::Worktree {
                project: project_identity.clone(),
                root: worktree.worktree.root.clone(),
            };
            let worktree_expanded = !collapsed.contains(&worktree_key);
            rows.push(ProjectTreeRow::Worktree {
                key: worktree_key.clone(),
                parent: project_key.clone(),
                worktree: worktree.worktree,
                expanded: worktree_expanded,
                depth: depth + 1,
            });
            if worktree_expanded {
                append_session_buckets(
                    rows,
                    worktree.sessions,
                    Some(project_identity.clone()),
                    Some(worktree.worktree.root.clone()),
                    worktree_key,
                    depth + 2,
                    expanded_disclosures,
                );
            }
        }
    }

    append_session_buckets(
        rows,
        direct_sessions,
        Some(project_identity),
        None,
        project_key,
        depth + 1,
        expanded_disclosures,
    );
}

fn append_session_buckets<'session>(
    rows: &mut Vec<ProjectTreeRow<'session>>,
    buckets: SessionBuckets<'session>,
    project: Option<ProjectIdentity>,
    worktree_root: Option<PathBuf>,
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
            worktree_root: worktree_root.clone(),
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
            worktree_root,
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
        worktree_root,
        band,
        parent,
        sessions,
        depth,
    } = disclosure;
    if sessions.is_empty() {
        return;
    }
    let key = ProjectTreeKey::Disclosure {
        project,
        worktree_root,
        band,
    };
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

    fn is_empty(&self) -> bool {
        self.open.is_empty() && self.recent.is_empty() && self.older.is_empty()
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
    worktrees: Vec<WorktreeGroup<'a>>,
    direct_sessions: SessionBuckets<'a>,
    latest_activity_ms: i64,
    has_open_session: bool,
}

struct WorktreeGroup<'a> {
    worktree: &'a Worktree,
    sessions: SessionBuckets<'a>,
}

struct ProjectInventory<'a> {
    current_projects: Vec<ProjectGroup<'a>>,
}

/// 将已解析 Project 的 Session 分组，并按 Project 最新 Session 活动时间倒序排列。
fn grouped_sessions<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
    now_ms: i64,
) -> ProjectInventory<'a> {
    const SEVEN_DAYS_MS: i64 = 7 * 86_400_000;
    let cutoff_ms = now_ms - SEVEN_DAYS_MS;
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
                    worktrees: Vec::new(),
                    direct_sessions: SessionBuckets::default(),
                    latest_activity_ms: session.last_activity_ms,
                    has_open_session: false,
                });
                projects.len() - 1
            });
        let project = &mut projects[project_index];
        project.latest_activity_ms = project.latest_activity_ms.max(session.last_activity_ms);
        project.has_open_session |= session.state.is_open();

        if let Some(worktree) = session.worktree.as_ref() {
            let worktree_index = project
                .worktrees
                .iter()
                .position(|group| group.worktree.root == worktree.root)
                .unwrap_or_else(|| {
                    project.worktrees.push(WorktreeGroup {
                        worktree,
                        sessions: SessionBuckets::default(),
                    });
                    project.worktrees.len() - 1
                });
            project.worktrees[worktree_index]
                .sessions
                .push(session, session_bucket(session, cutoff_ms));
        } else {
            project
                .direct_sessions
                .push(session, session_bucket(session, cutoff_ms));
        }
    }

    for project in &mut projects {
        for worktree in &mut project.worktrees {
            worktree.sessions.sort(attention);
        }
        project.worktrees.sort_by(|left, right| {
            compare_groups(attention, &left.sessions.open, &right.sessions.open)
                .then_with(|| left.worktree.root.cmp(&right.worktree.root))
        });
        project.direct_sessions.sort(attention);
    }
    projects.sort_by(|left, right| {
        right
            .latest_activity_ms
            .cmp(&left.latest_activity_ms)
            .then_with(|| left.project.label.cmp(&right.project.label))
    });
    projects.retain(|project| project.has_open_session || project.latest_activity_ms >= cutoff_ms);
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

fn compare_groups(
    attention: &AttentionSnapshot,
    left: &[&Session],
    right: &[&Session],
) -> std::cmp::Ordering {
    compare_optional(attention, left.first().copied(), right.first().copied())
}

fn compare_optional(
    attention: &AttentionSnapshot,
    left: Option<&Session>,
    right: Option<&Session>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => attention.compare_sessions(left, right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}
