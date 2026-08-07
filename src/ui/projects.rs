//! 首页 Project、Worktree 和 Open Session 清单。

use super::{
    context, current_attention, is_system_temporary_cwd, snapshot_now_ms, text, theme::Theme,
    AllRegion, ProjectTreeKey, ProjectTreeSection, SessionAgeBand, SessionIdentity, Ui,
};
use crate::app::{fmt_relative, Snapshot};
use crate::attention::{AttentionRank, AttentionSnapshot};
use crate::domain::{Project, ProjectIdentity, Provider, Session, SessionState, Worktree};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

/// All 页面实际可见的一行；纯展示信息之外还保留稳定身份和父子关系。
pub(super) enum ProjectTreeRow<'a> {
    Section {
        key: ProjectTreeKey,
        section: ProjectTreeSection,
        item_count: usize,
        expanded: bool,
    },
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
            Self::Section { key, .. }
            | Self::Project { key, .. }
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
            Self::Section { .. } => None,
            Self::Project { parent, .. } => parent.as_ref(),
            Self::Worktree { parent, .. }
            | Self::Disclosure { parent, .. }
            | Self::Session { parent, .. } => Some(parent),
        }
    }

    pub(super) fn depth(&self) -> usize {
        match self {
            Self::Section { .. } => 0,
            Self::Project { depth, .. } | Self::Worktree { depth, .. } => *depth,
            Self::Disclosure { depth, .. } | Self::Session { depth, .. } => *depth,
        }
    }

    pub(super) fn is_group(&self) -> bool {
        !matches!(self, Self::Session { .. })
    }

    pub(super) fn is_expanded(&self) -> bool {
        match self {
            Self::Section { expanded, .. }
            | Self::Project { expanded, .. }
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

    if !inventory.older_projects.is_empty() {
        let key = ProjectTreeKey::Section(ProjectTreeSection::OlderProjects);
        let expanded = expanded_disclosures.contains(&key);
        rows.push(ProjectTreeRow::Section {
            key: key.clone(),
            section: ProjectTreeSection::OlderProjects,
            item_count: inventory.older_projects.len(),
            expanded,
        });
        if expanded {
            for project in inventory.older_projects {
                append_project(
                    &mut rows,
                    project,
                    Some(key.clone()),
                    1,
                    collapsed,
                    expanded_disclosures,
                );
            }
        }
    }

    if !inventory.unavailable.is_empty() {
        let key = ProjectTreeKey::Section(ProjectTreeSection::Unavailable);
        let expanded = !collapsed.contains(&key);
        rows.push(ProjectTreeRow::Section {
            key: key.clone(),
            section: ProjectTreeSection::Unavailable,
            item_count: inventory.unavailable.len(),
            expanded,
        });
        if expanded {
            append_session_buckets(
                &mut rows,
                inventory.unavailable,
                None,
                None,
                key,
                1,
                expanded_disclosures,
            );
        }
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
    ProjectTreeKey::Session(SessionIdentity {
        provider: session.provider,
        native_id: session.id.clone(),
    })
}

/// Draw the Session Tree portion of the All-page editor.
pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let now_ms = snapshot_now_ms(snap);
    let attention = current_attention(snap);
    let tree_rows = rows(
        &snap.sessions,
        &attention,
        now_ms,
        &ui.collapsed_project_tree_nodes,
        &ui.expanded_project_tree_disclosures,
    );
    let visible_sessions = tree_rows
        .iter()
        .filter_map(|row| match row {
            ProjectTreeRow::Session { session, .. } => Some(*session),
            _ => None,
        })
        .collect::<Vec<_>>();
    let visible_identities = visible_sessions
        .iter()
        .map(|session| (session.provider, session.id.as_str()))
        .collect::<Vec<_>>();
    let mut titles = ui.labels.titles_for(&visible_identities).into_iter();
    let (_, open_session_count) = tree_counts(snap);
    let content_width = area.width as usize;
    let mut lines = Vec::new();
    let selected = (ui.all_region == AllRegion::Now)
        .then(|| ui.effective_project_tree_selection(snap))
        .flatten();

    if open_session_count == 0 && tree_rows.is_empty() {
        lines.push(Line::from(Span::styled(
            if snap.ready {
                "No Open Sessions."
            } else {
                "Indexing... Open Sessions will appear shortly."
            },
            Style::default().fg(theme.muted),
        )));
    }

    let visible_rows = area.height as usize;
    let selected_index = selected
        .as_ref()
        .and_then(|selected| tree_rows.iter().position(|row| row.key() == selected));
    let scroll = selected_index
        .unwrap_or_default()
        .saturating_sub(visible_rows.saturating_sub(1));

    for row in tree_rows {
        let is_selected = selected.as_ref().is_some_and(|key| key == row.key());
        match row {
            ProjectTreeRow::Section {
                section,
                item_count,
                expanded,
                ..
            } => lines.push(section_line(
                section,
                item_count,
                expanded,
                is_selected,
                content_width,
                theme,
            )),
            ProjectTreeRow::Project {
                project,
                session_count,
                expanded,
                depth,
                ..
            } => lines.push(project_line(
                project,
                session_count,
                depth,
                expanded,
                is_selected,
                content_width,
                theme,
            )),
            ProjectTreeRow::Worktree {
                worktree,
                expanded,
                depth,
                ..
            } => lines.push(worktree_line(
                worktree,
                &attention,
                depth,
                expanded,
                is_selected,
                content_width,
                theme,
            )),
            ProjectTreeRow::Disclosure {
                band,
                session_count,
                expanded,
                depth,
                ..
            } => lines.push(disclosure_line(
                band,
                session_count,
                depth,
                expanded,
                is_selected,
                content_width,
                theme,
            )),
            ProjectTreeRow::Session { session, depth, .. } => {
                let title = titles.next().expect("every NOW Session has a Title");
                let position = SessionTreePosition {
                    depth,
                    selected: is_selected,
                };
                lines.push(if session.state == SessionState::Exited {
                    exited_session_line(session, &title, now_ms, content_width, position, theme)
                } else {
                    open_session_line(
                        session,
                        &title,
                        &attention,
                        now_ms,
                        content_width,
                        position,
                        theme,
                    )
                });
            }
        }
    }

    f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), area);
}

/// 返回可解析 Project 总数和非临时目录中的 Open Session 总数。
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
        .filter(|session| session.state.is_open())
        .count();
    (project_count, open_session_count)
}

/// 按首页 Attention 顺序返回所有 Open Session。
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
    older_projects: Vec<ProjectGroup<'a>>,
    unavailable: SessionBuckets<'a>,
}

/// 将全部可浏览 Session 分组，并按 Project 最新 Session 活动时间倒序排列。
fn grouped_sessions<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
    now_ms: i64,
) -> ProjectInventory<'a> {
    const SEVEN_DAYS_MS: i64 = 7 * 86_400_000;
    let cutoff_ms = now_ms - SEVEN_DAYS_MS;
    let mut projects: Vec<ProjectGroup<'_>> = Vec::new();
    let mut unavailable = SessionBuckets::default();

    for session in sessions {
        if is_system_temporary_cwd(session.cwd.as_deref()) {
            continue;
        }
        let Some(resolved_project) = session.project.as_ref() else {
            unavailable.push(session, session_bucket(session, cutoff_ms));
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
    unavailable.sort(attention);

    let (current_projects, older_projects) = projects
        .into_iter()
        .partition(|project| project.has_open_session || project.latest_activity_ms >= cutoff_ms);
    ProjectInventory {
        current_projects,
        older_projects,
        unavailable,
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

fn open_session_line(
    session: &Session,
    title: &str,
    attention: &AttentionSnapshot,
    now_ms: i64,
    content_width: usize,
    position: SessionTreePosition,
    theme: Theme,
) -> Line<'static> {
    let prefix = format!(
        "{}{} ",
        "  ".repeat(position.depth),
        if position.selected { ">" } else { " " }
    );
    let attention_rank = attention.rank_for(session);
    let attention_marker = match attention_rank {
        AttentionRank::CriticalContext
        | AttentionRank::SharedWorktreeRisk
        | AttentionRank::WarningContext => "!",
        AttentionRank::Idle | AttentionRank::HealthyBusy => " ",
    };
    let attention_style = match attention_rank {
        AttentionRank::CriticalContext => Style::default()
            .fg(theme.critical)
            .add_modifier(Modifier::BOLD),
        AttentionRank::SharedWorktreeRisk | AttentionRank::WarningContext => {
            Style::default().fg(theme.warning)
        }
        AttentionRank::Idle | AttentionRank::HealthyBusy => Style::default().fg(theme.muted),
    };
    let lifecycle_marker = match session.state {
        SessionState::Busy => "●",
        SessionState::Idle => "○",
        SessionState::Exited => unreachable!("NOW contains only Open Sessions"),
    };
    let provider = provider_icon(session.provider);
    let context_value = session
        .context
        .percent()
        .map(|percent| format!("{percent:.0}%"))
        .unwrap_or_else(|| "N/A".to_string());
    let context_value = text::fit_right(&context_value, CONTEXT_VALUE_WIDTH, true);
    let elapsed = if content_width >= SHOW_ELAPSED_WIDTH {
        format!(
            " {}",
            text::fit_right(
                &fmt_relative(session.last_activity_ms, now_ms),
                ELAPSED_VALUE_WIDTH,
                true,
            )
        )
    } else {
        String::new()
    };
    let fixed_width = [
        prefix.as_str(),
        provider,
        SESSION_COLUMN_GAP,
        SESSION_COLUMN_GAP,
        attention_marker,
        SESSION_COLUMN_GAP,
        lifecycle_marker,
        SESSION_COLUMN_GAP,
        context_value.as_str(),
        elapsed.as_str(),
    ]
    .into_iter()
    .map(UnicodeWidthStr::width)
    .sum::<usize>();
    let title = text::fit(title, content_width.saturating_sub(fixed_width), true);
    let cursor_style = if position.selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let lifecycle_style = match session.state {
        SessionState::Busy => Style::default().fg(theme.busy).add_modifier(Modifier::BOLD),
        SessionState::Idle => Style::default().fg(theme.idle),
        SessionState::Exited => unreachable!("NOW contains only Open Sessions"),
    };
    let context_style = session
        .context
        .percent()
        .map(|percent| context::style(context::role(percent), theme))
        .unwrap_or_else(|| Style::default().fg(theme.muted));
    Line::from(vec![
        Span::styled(prefix, cursor_style),
        Span::styled(provider, Style::default().fg(theme.accent)),
        Span::raw(SESSION_COLUMN_GAP),
        Span::styled(title, Style::default().fg(theme.foreground)),
        Span::raw(SESSION_COLUMN_GAP),
        Span::styled(attention_marker, attention_style),
        Span::raw(SESSION_COLUMN_GAP),
        Span::styled(lifecycle_marker, lifecycle_style),
        Span::raw(SESSION_COLUMN_GAP),
        Span::styled(context_value, context_style),
        Span::styled(elapsed, Style::default().fg(theme.muted)),
    ])
}

fn exited_session_line(
    session: &Session,
    title: &str,
    now_ms: i64,
    content_width: usize,
    position: SessionTreePosition,
    theme: Theme,
) -> Line<'static> {
    let prefix = format!(
        "{}{} ",
        "  ".repeat(position.depth),
        if position.selected { ">" } else { " " }
    );
    let provider = provider_icon(session.provider);
    let elapsed = fmt_relative(session.last_activity_ms, now_ms);
    let fixed_width = UnicodeWidthStr::width(prefix.as_str())
        + UnicodeWidthStr::width(provider)
        + 2
        + UnicodeWidthStr::width(elapsed.as_str());
    let title = text::fit(title, content_width.saturating_sub(fixed_width), true);
    let cursor_style = if position.selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(prefix, cursor_style),
        Span::styled(provider, Style::default().fg(theme.accent)),
        Span::raw(" "),
        Span::styled(title, Style::default().fg(theme.exited)),
        Span::raw(" "),
        Span::styled(elapsed, Style::default().fg(theme.muted)),
    ])
}

fn provider_icon(provider: Provider) -> &'static str {
    match provider {
        Provider::ClaudeCode => "✦",
        Provider::CodexCli => "◎",
    }
}

fn section_line(
    section: ProjectTreeSection,
    item_count: usize,
    expanded: bool,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let label = match section {
        ProjectTreeSection::OlderProjects => "Older Projects",
        ProjectTreeSection::Unavailable => "Unavailable",
    };
    group_line(label, item_count, 0, expanded, selected, width, theme)
}

fn project_line(
    project: &Project,
    session_count: usize,
    depth: usize,
    expanded: bool,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let prefix = group_prefix(depth, selected, expanded);
    let count = format!(" ({session_count})");
    let label_width = width.saturating_sub(
        UnicodeWidthStr::width(prefix.as_str()) + UnicodeWidthStr::width(count.as_str()),
    );
    Line::from(vec![
        Span::styled(
            prefix,
            if selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted)
            },
        ),
        Span::styled(
            text::shorten_end(&project.label, label_width),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(count, Style::default().fg(theme.muted)),
    ])
}

fn worktree_line(
    worktree: &Worktree,
    attention: &AttentionSnapshot,
    depth: usize,
    expanded: bool,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let branch = worktree.branch.as_deref().unwrap_or("detached HEAD");
    let risk = attention
        .shared_worktree_risk(&worktree.root)
        .map(|risk| format!(" !{}", risk.contributing_sessions.len()))
        .unwrap_or_default();
    let prefix = group_prefix(depth, selected, expanded);
    let label_width = width.saturating_sub(
        UnicodeWidthStr::width(prefix.as_str()) + UnicodeWidthStr::width(risk.as_str()),
    );
    Line::from(vec![
        Span::styled(
            prefix,
            if selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted)
            },
        ),
        Span::styled(
            text::shorten_end(branch, label_width),
            Style::default().fg(theme.foreground),
        ),
        Span::styled(risk, Style::default().fg(theme.warning)),
    ])
}

fn disclosure_line(
    band: SessionAgeBand,
    session_count: usize,
    depth: usize,
    expanded: bool,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let label = match band {
        SessionAgeBand::Recent => "Recent",
        SessionAgeBand::Older => "Older",
    };
    group_line(
        label,
        session_count,
        depth,
        expanded,
        selected,
        width,
        theme,
    )
}

fn group_line(
    label: &str,
    item_count: usize,
    depth: usize,
    expanded: bool,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let prefix = group_prefix(depth, selected, expanded);
    let count = format!(" ({item_count})");
    let label_width = width.saturating_sub(
        UnicodeWidthStr::width(prefix.as_str()) + UnicodeWidthStr::width(count.as_str()),
    );
    Line::from(vec![
        Span::styled(
            prefix,
            if selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted)
            },
        ),
        Span::styled(
            text::shorten_end(label, label_width),
            Style::default().fg(theme.muted),
        ),
        Span::styled(count, Style::default().fg(theme.muted)),
    ])
}

fn group_prefix(depth: usize, selected: bool, expanded: bool) -> String {
    format!(
        "{}{}{} ",
        "  ".repeat(depth),
        if selected { ">" } else { " " },
        if expanded { "▾" } else { "▸" }
    )
}

pub(super) fn home_relative_path(path: &Path) -> Cow<'_, str> {
    let Some(home) = dirs::home_dir() else {
        return path.to_string_lossy();
    };
    let Ok(relative) = path.strip_prefix(home) else {
        return path.to_string_lossy();
    };
    if relative.as_os_str().is_empty() {
        Cow::Borrowed("~")
    } else {
        Cow::Owned(format!("~/{}", relative.to_string_lossy()))
    }
}

#[derive(Debug, Clone, Copy)]
struct SessionTreePosition {
    depth: usize,
    selected: bool,
}

const SHOW_ELAPSED_WIDTH: usize = 42;
const CONTEXT_VALUE_WIDTH: usize = 4;
const ELAPSED_VALUE_WIDTH: usize = 4;
const SESSION_COLUMN_GAP: &str = " ";

#[cfg(test)]
#[path = "projects_test.rs"]
mod tests;
