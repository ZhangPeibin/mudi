//! 首页 Project、Worktree 和 Open Session 清单。

use super::{
    context, current_attention, is_system_temporary_cwd, snapshot_now_ms, text, theme::Theme,
    AllRegion, ProjectTreeKey, SessionAgeBand, SessionIdentity, Ui,
};
use crate::app::{fmt_relative, Snapshot};
use crate::attention::{AttentionCondition, AttentionRank, AttentionSnapshot};
use crate::domain::{Project, ProjectIdentity, Provider, Session, SessionState, Worktree};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

/// All 页面实际可见的一行；纯展示信息之外还保留稳定身份和父子关系。
pub(super) enum ProjectTreeRow<'a> {
    Project {
        key: ProjectTreeKey,
        project: Option<&'a Project>,
        session_count: usize,
        expanded: bool,
    },
    Worktree {
        key: ProjectTreeKey,
        parent: ProjectTreeKey,
        worktree: &'a Worktree,
        expanded: bool,
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
            Self::Project { .. } => None,
            Self::Worktree { parent, .. }
            | Self::Disclosure { parent, .. }
            | Self::Session { parent, .. } => Some(parent),
        }
    }

    pub(super) fn depth(&self) -> usize {
        match self {
            Self::Project { .. } => 0,
            Self::Worktree { .. } => 1,
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

/// 按 Attention 顺序建立应用折叠状态后的 Project -> Worktree -> Session 行。
pub(super) fn rows<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
    now_ms: i64,
    collapsed: &HashSet<ProjectTreeKey>,
    expanded_disclosures: &HashSet<ProjectTreeKey>,
) -> Vec<ProjectTreeRow<'a>> {
    let mut rows = Vec::new();
    for project in grouped_sessions(sessions, attention, now_ms) {
        let project_identity = project.project.map(|project| project.identity.clone());
        let project_key = ProjectTreeKey::Project(project_identity.clone());
        let project_expanded = !collapsed.contains(&project_key);
        let session_count = project.direct_sessions.len()
            + project
                .worktrees
                .iter()
                .map(|worktree| worktree.sessions.len())
                .sum::<usize>();
        rows.push(ProjectTreeRow::Project {
            key: project_key.clone(),
            project: project.project,
            session_count,
            expanded: project_expanded,
        });
        if !project_expanded {
            continue;
        }

        for worktree in project.worktrees {
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
            });
            if worktree_expanded {
                rows.extend(worktree.sessions.open.into_iter().map(|session| {
                    ProjectTreeRow::Session {
                        key: session_tree_key(session),
                        parent: worktree_key.clone(),
                        session,
                        depth: 2,
                    }
                }));
                append_disclosure(
                    &mut rows,
                    HistoricalDisclosure {
                        project: project_identity.clone(),
                        worktree_root: Some(worktree.worktree.root.clone()),
                        band: SessionAgeBand::Recent,
                        parent: worktree_key.clone(),
                        sessions: &worktree.sessions.recent,
                        depth: 2,
                    },
                    expanded_disclosures,
                );
                append_disclosure(
                    &mut rows,
                    HistoricalDisclosure {
                        project: project_identity.clone(),
                        worktree_root: Some(worktree.worktree.root.clone()),
                        band: SessionAgeBand::Older,
                        parent: worktree_key.clone(),
                        sessions: &worktree.sessions.older,
                        depth: 2,
                    },
                    expanded_disclosures,
                );
            }
        }
        rows.extend(project.direct_sessions.open.into_iter().map(|session| {
            ProjectTreeRow::Session {
                key: session_tree_key(session),
                parent: project_key.clone(),
                session,
                depth: 1,
            }
        }));
        append_disclosure(
            &mut rows,
            HistoricalDisclosure {
                project: project_identity.clone(),
                worktree_root: None,
                band: SessionAgeBand::Recent,
                parent: project_key.clone(),
                sessions: &project.direct_sessions.recent,
                depth: 1,
            },
            expanded_disclosures,
        );
        append_disclosure(
            &mut rows,
            HistoricalDisclosure {
                project: project_identity,
                worktree_root: None,
                band: SessionAgeBand::Older,
                parent: project_key.clone(),
                sessions: &project.direct_sessions.older,
                depth: 1,
            },
            expanded_disclosures,
        );
    }
    rows
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
    let open_project_count = snap
        .sessions
        .iter()
        .filter(|session| session.state.is_open())
        .filter_map(|session| session.project.as_ref())
        .map(|project| &project.identity)
        .collect::<HashSet<_>>()
        .len();
    let open_session_count = snap
        .sessions
        .iter()
        .filter(|session| session.state.is_open())
        .count();
    let content_width = area.width.saturating_sub(2) as usize;
    let compact = content_width < COMPACT_TREE_MIN_WIDTH;
    let column_widths = column_widths(area.width, content_width);
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

    let visible_rows = area.height.saturating_sub(2) as usize;
    let selected_index = selected
        .as_ref()
        .and_then(|selected| tree_rows.iter().position(|row| row.key() == selected));
    let scroll = selected_index
        .unwrap_or_default()
        .saturating_sub(visible_rows.saturating_sub(1));

    for row in tree_rows {
        let is_selected = selected.as_ref().is_some_and(|key| key == row.key());
        match row {
            ProjectTreeRow::Project {
                project,
                session_count,
                expanded,
                ..
            } => lines.push(project_line(
                project,
                session_count,
                expanded,
                is_selected,
                content_width,
                theme,
            )),
            ProjectTreeRow::Worktree {
                worktree, expanded, ..
            } => lines.push(worktree_line(
                worktree,
                &attention,
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
                } else if compact {
                    compact_session_line(
                        session,
                        &title,
                        &attention,
                        now_ms,
                        content_width,
                        position,
                        theme,
                    )
                } else {
                    session_line(
                        session,
                        &title,
                        &attention,
                        now_ms,
                        column_widths,
                        position,
                        theme,
                    )
                });
            }
        }
    }

    f.render_widget(
        Paragraph::new(lines).scroll((scroll as u16, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if ui.all_region == AllRegion::Now {
                    theme.accent
                } else {
                    theme.border
                }))
                .title(Line::from(vec![
                    Span::styled(
                        " SESSION TREE ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{open_project_count} "),
                        Style::default()
                            .fg(theme.foreground)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]))
                .title(
                    Line::from(vec![
                        Span::raw(" "),
                        Span::styled(
                            open_session_count.to_string(),
                            Style::default()
                                .fg(theme.foreground)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            if open_session_count == 1 {
                                " OPEN SESSION "
                            } else {
                                " OPEN SESSIONS "
                            },
                            Style::default().fg(theme.muted),
                        ),
                    ])
                    .right_aligned(),
                ),
        ),
        area,
    );
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
    project: Option<&'a Project>,
    worktrees: Vec<WorktreeGroup<'a>>,
    direct_sessions: SessionBuckets<'a>,
}

impl ProjectGroup<'_> {
    fn label(&self) -> &str {
        self.project.map_or("N/A", |project| project.label.as_str())
    }
}

struct WorktreeGroup<'a> {
    worktree: &'a Worktree,
    sessions: SessionBuckets<'a>,
}

/// 将全部可浏览 Session 分组为 Project -> Worktree -> Session Tree 层级。
fn grouped_sessions<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
    now_ms: i64,
) -> Vec<ProjectGroup<'a>> {
    const SEVEN_DAYS_MS: i64 = 7 * 86_400_000;
    let cutoff_ms = now_ms - SEVEN_DAYS_MS;
    let mut projects: Vec<ProjectGroup<'_>> = Vec::new();

    for session in sessions {
        if is_system_temporary_cwd(session.cwd.as_deref()) {
            continue;
        }
        let project_index = match session.project.as_ref() {
            Some(project) => projects.iter().position(|group| {
                group
                    .project
                    .is_some_and(|existing| existing.identity == project.identity)
            }),
            None => projects.iter().position(|group| group.project.is_none()),
        };
        let project_index = project_index.unwrap_or_else(|| {
            projects.push(ProjectGroup {
                project: session.project.as_ref(),
                worktrees: Vec::new(),
                direct_sessions: SessionBuckets::default(),
            });
            projects.len() - 1
        });
        let project = &mut projects[project_index];

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
        compare_optional(
            attention,
            best_session(left, attention),
            best_session(right, attention),
        )
        .then_with(|| left.label().cmp(right.label()))
    });

    projects
}

fn best_session<'a>(
    project: &ProjectGroup<'a>,
    attention: &AttentionSnapshot,
) -> Option<&'a Session> {
    project
        .worktrees
        .iter()
        .flat_map(|worktree| worktree.sessions.open.iter().copied())
        .chain(project.direct_sessions.open.iter().copied())
        .min_by(|left, right| attention.compare_sessions(left, right))
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

fn session_line(
    session: &Session,
    title: &str,
    attention: &AttentionSnapshot,
    now_ms: i64,
    widths: ColumnWidths,
    position: SessionTreePosition,
    theme: Theme,
) -> Line<'static> {
    let state = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => unreachable!("NOW contains only Open Sessions"),
    };
    let provider = match session.provider {
        Provider::ClaudeCode => "Claude",
        Provider::CodexCli => "Codex",
    };
    let attention_rank = attention.rank_for(session);
    let model = text::shorten_start(
        session.latest_model.as_deref().unwrap_or("N/A"),
        widths.model,
    );
    let title = text::fit(title, widths.title, true);
    let attention_label = match attention_rank {
        AttentionRank::CriticalContext => "CRITICAL",
        AttentionRank::SharedWorktreeRisk => "SHARED",
        AttentionRank::WarningContext => "WARNING",
        AttentionRank::Idle => "IDLE",
        AttentionRank::HealthyBusy => "BUSY",
    };
    let idle_since =
        attention
            .conditions_for(session)
            .iter()
            .find_map(|condition| match condition {
                AttentionCondition::Idle { since_ms, .. } => Some(*since_ms),
                _ => None,
            });
    let relative = fmt_relative(idle_since.unwrap_or(session.last_activity_ms), now_ms);
    let elapsed = if idle_since.is_some() && widths.elapsed >= 7 {
        format!("Idle {relative}")
    } else {
        relative
    };
    let elapsed = text::shorten_start(&elapsed, widths.elapsed);
    let cursor_style = if position.selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let base_style = Style::default().fg(theme.foreground);
    let attention_style = match attention_rank {
        AttentionRank::CriticalContext => Style::default()
            .fg(theme.critical)
            .add_modifier(Modifier::BOLD),
        AttentionRank::SharedWorktreeRisk | AttentionRank::WarningContext => {
            Style::default().fg(theme.warning)
        }
        AttentionRank::Idle => Style::default().fg(theme.idle),
        AttentionRank::HealthyBusy => Style::default().fg(theme.busy),
    };
    let lifecycle_style = match session.state {
        SessionState::Busy => Style::default().fg(theme.busy).add_modifier(Modifier::BOLD),
        SessionState::Idle => Style::default().fg(theme.idle),
        SessionState::Exited => unreachable!("NOW contains only Open Sessions"),
    };
    let mut spans = vec![
        Span::styled(
            format!(
                "{}{} ",
                "  ".repeat(position.depth),
                if position.selected { ">" } else { " " }
            ),
            cursor_style,
        ),
        Span::styled(format!("{attention_label:<8} "), attention_style),
        Span::styled(format!("{state:<4} "), lifecycle_style),
        Span::styled(format!("{provider:<6} "), base_style),
        Span::styled(format!("{title} "), base_style),
    ];
    spans.extend(context::evidence(session, widths.context_bar, theme));
    spans.push(Span::styled(
        format!(
            "{model:<model_width$} {elapsed:<elapsed_width$}",
            model_width = widths.model,
            elapsed_width = widths.elapsed,
        ),
        base_style,
    ));
    Line::from(spans)
}

fn compact_session_line(
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
    let attention_label = match (attention_rank, content_width >= WIDE_COMPACT_TREE_WIDTH) {
        (AttentionRank::CriticalContext, true) => "CRITICAL",
        (AttentionRank::SharedWorktreeRisk, true) => "SHARED",
        (AttentionRank::WarningContext, true) => "WARNING",
        (AttentionRank::Idle, true) => "IDLE",
        (AttentionRank::HealthyBusy, true) => "BUSY",
        (AttentionRank::CriticalContext, false) => "C",
        (AttentionRank::SharedWorktreeRisk, false) => "S",
        (AttentionRank::WarningContext, false) => "W",
        (AttentionRank::Idle, false) => "I",
        (AttentionRank::HealthyBusy, false) => "B",
    };
    let attention_style = match attention_rank {
        AttentionRank::CriticalContext => Style::default()
            .fg(theme.critical)
            .add_modifier(Modifier::BOLD),
        AttentionRank::SharedWorktreeRisk | AttentionRank::WarningContext => {
            Style::default().fg(theme.warning)
        }
        AttentionRank::Idle => Style::default().fg(theme.idle),
        AttentionRank::HealthyBusy => Style::default().fg(theme.busy),
    };
    let state = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => unreachable!("NOW contains only Open Sessions"),
    };
    let provider = match session.provider {
        Provider::ClaudeCode => "Claude",
        Provider::CodexCli => "Codex",
    };
    let context = session
        .context
        .percent()
        .map(|percent| format!("{percent:.0}%"))
        .unwrap_or_else(|| "N/A".to_string());
    let elapsed = if content_width >= WIDE_COMPACT_TREE_WIDTH {
        format!(" {}", fmt_relative(session.last_activity_ms, now_ms))
    } else {
        String::new()
    };
    let fixed_width = UnicodeWidthStr::width(prefix.as_str())
        + UnicodeWidthStr::width(attention_label)
        + UnicodeWidthStr::width(" ")
        + UnicodeWidthStr::width(state)
        + UnicodeWidthStr::width(" ")
        + UnicodeWidthStr::width(provider)
        + UnicodeWidthStr::width(" ")
        + UnicodeWidthStr::width(context.as_str())
        + UnicodeWidthStr::width(elapsed.as_str());
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
        Span::styled(attention_label, attention_style),
        Span::raw(" "),
        Span::styled(state, lifecycle_style),
        Span::raw(" "),
        Span::styled(provider, Style::default().fg(theme.foreground)),
        Span::raw(" "),
        Span::styled(title, Style::default().fg(theme.foreground)),
        Span::styled(context, context_style),
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
    let provider = match session.provider {
        Provider::ClaudeCode => "Claude",
        Provider::CodexCli => "Codex",
    };
    let elapsed = fmt_relative(session.last_activity_ms, now_ms);
    let fixed_width = UnicodeWidthStr::width(prefix.as_str())
        + UnicodeWidthStr::width("EXITED ")
        + UnicodeWidthStr::width(provider)
        + 3
        + UnicodeWidthStr::width(elapsed.as_str());
    let available_width = content_width.saturating_sub(fixed_width);
    let model_width = available_width / 2;
    let title = text::fit(title, available_width.saturating_sub(model_width), true);
    let model = text::shorten_start(
        session.latest_model.as_deref().unwrap_or("N/A"),
        model_width,
    );
    let cursor_style = if position.selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(prefix, cursor_style),
        Span::styled(
            "EXITED ",
            Style::default()
                .fg(theme.exited)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{provider} "),
            Style::default().fg(theme.foreground),
        ),
        Span::styled(title, Style::default().fg(theme.foreground)),
        Span::raw(" "),
        Span::styled(model, Style::default().fg(theme.foreground)),
        Span::raw(" "),
        Span::styled(elapsed, Style::default().fg(theme.muted)),
    ])
}

fn project_line(
    project: Option<&Project>,
    session_count: usize,
    expanded: bool,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let prefix = group_prefix(0, selected, expanded);
    let count = format!(" ({session_count})");
    let label_width = width.saturating_sub(prefix.chars().count() + count.chars().count());
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
            text::shorten_end(
                project.map_or("N/A", |project| project.label.as_str()),
                label_width,
            ),
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
    expanded: bool,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    if width < COMPACT_TREE_MIN_WIDTH {
        return compact_worktree_line(worktree, attention, expanded, selected, width, theme);
    }
    let branch = worktree.branch.as_deref().unwrap_or("detached HEAD");
    let evidence = attention
        .shared_worktree_risk(&worktree.root)
        .map(|risk| {
            format!(
                " | WARNING Shared Worktree Risk x{}",
                risk.contributing_sessions.len()
            )
        })
        .unwrap_or_default();
    let prefix = group_prefix(1, selected, expanded);
    let identity_width = width.saturating_sub(prefix.len() + 3 + evidence.chars().count());
    let path_width = identity_width.saturating_mul(3) / 5;
    let branch_width = identity_width.saturating_sub(path_width);
    let path = home_relative_path(&worktree.root);
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
            format!(
                "{} | {}{evidence}",
                text::shorten_end(&path, path_width),
                text::shorten_start(branch, branch_width),
            ),
            Style::default().fg(theme.muted),
        ),
    ])
}

fn compact_worktree_line(
    worktree: &Worktree,
    attention: &AttentionSnapshot,
    expanded: bool,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let prefix = group_prefix(1, selected, expanded);
    let branch = worktree.branch.as_deref().unwrap_or("detached HEAD");
    let risk = attention
        .shared_worktree_risk(&worktree.root)
        .map(|risk| format!(" | Risk x{}", risk.contributing_sessions.len()))
        .unwrap_or_default();
    let branch = format!(" | {branch}");
    let label_width = width.saturating_sub(
        UnicodeWidthStr::width(prefix.as_str())
            + UnicodeWidthStr::width(branch.as_str())
            + UnicodeWidthStr::width(risk.as_str()),
    );
    let label = worktree
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("N/A");
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
            Style::default().fg(theme.foreground),
        ),
        Span::styled(branch, Style::default().fg(theme.muted)),
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
        SessionAgeBand::Recent => "RECENT SESSIONS",
        SessionAgeBand::Older => "OLDER SESSIONS",
    };
    let prefix = group_prefix(depth, selected, expanded);
    let count = format!(" ({session_count})");
    let label_width = width.saturating_sub(prefix.len() + count.len());
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
        "{}{} {} ",
        "  ".repeat(depth),
        if selected { ">" } else { " " },
        if expanded { "-" } else { "+" }
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

const MIN_CONTEXT_BAR_WIDTH: usize = 12;
const COMPACT_TREE_MIN_WIDTH: usize = 70;
const WIDE_COMPACT_TREE_WIDTH: usize = 45;
const MAX_CONTEXT_BAR_WIDTH: usize = 20;
const COMPACT_TERMINAL_WIDTH: u16 = 80;
const COLUMNS_PER_CONTEXT_CELL: u16 = 5;
const MIN_TITLE_WIDTH: usize = 14;
const MAX_TITLE_WIDTH: usize = 20;
const MIN_MODEL_WIDTH: usize = 11;
const PREFERRED_MODEL_WIDTH: usize = 14;
const FIXED_WIDTH_WITHOUT_FLEXIBLE_COLUMNS_AND_ELAPSED: usize = 33;
const WORKTREE_SESSION_PREFIX_EXTRA: usize = 4;
const COMPACT_ELAPSED_WIDTH: usize = 4;
const WIDE_ELAPSED_WIDTH: usize = 8;
const COMPACT_CONTENT_WIDTH: usize = FIXED_WIDTH_WITHOUT_FLEXIBLE_COLUMNS_AND_ELAPSED
    + COMPACT_ELAPSED_WIDTH
    + MIN_TITLE_WIDTH
    + MIN_CONTEXT_BAR_WIDTH
    + MIN_MODEL_WIDTH;

#[derive(Debug, Clone, Copy)]
struct ColumnWidths {
    title: usize,
    context_bar: usize,
    model: usize,
    elapsed: usize,
}

#[derive(Debug, Clone, Copy)]
struct SessionTreePosition {
    depth: usize,
    selected: bool,
}

fn column_widths(terminal_width: u16, content_width: usize) -> ColumnWidths {
    let content_width = content_width.saturating_sub(WORKTREE_SESSION_PREFIX_EXTRA);
    let elapsed = if terminal_width <= COMPACT_TERMINAL_WIDTH {
        COMPACT_ELAPSED_WIDTH
    } else {
        WIDE_ELAPSED_WIDTH
    };
    let title = title_width(content_width);
    let context_bar = context_bar_width(terminal_width, content_width, title);
    let model = content_width
        .saturating_sub(
            FIXED_WIDTH_WITHOUT_FLEXIBLE_COLUMNS_AND_ELAPSED + elapsed + title + context_bar,
        )
        .max(MIN_MODEL_WIDTH);
    ColumnWidths {
        title,
        context_bar,
        model,
        elapsed,
    }
}

fn title_width(content_width: usize) -> usize {
    MIN_TITLE_WIDTH
        + content_width
            .saturating_sub(COMPACT_CONTENT_WIDTH)
            .min(MAX_TITLE_WIDTH - MIN_TITLE_WIDTH)
}

fn preferred_context_bar_width(terminal_width: u16) -> usize {
    let extra_cells =
        terminal_width.saturating_sub(COMPACT_TERMINAL_WIDTH) / COLUMNS_PER_CONTEXT_CELL;
    (MIN_CONTEXT_BAR_WIDTH + extra_cells as usize).min(MAX_CONTEXT_BAR_WIDTH)
}

fn context_bar_width(terminal_width: u16, content_width: usize, title_width: usize) -> usize {
    let available_cells = content_width
        .saturating_sub(
            FIXED_WIDTH_WITHOUT_FLEXIBLE_COLUMNS_AND_ELAPSED
                + WIDE_ELAPSED_WIDTH
                + title_width
                + PREFERRED_MODEL_WIDTH,
        )
        .max(MIN_CONTEXT_BAR_WIDTH);
    preferred_context_bar_width(terminal_width).min(available_cells)
}

#[cfg(test)]
#[path = "projects_test.rs"]
mod tests;
