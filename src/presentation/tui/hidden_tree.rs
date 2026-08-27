//! Dedicated management view for currently observable Hidden Sessions.

use super::{
    is_system_temporary_cwd, provider_mark, retained_selection, snapshot_now_ms, text, theme::Theme,
};
use crate::application::session_labels::{CurationDisposition, SessionMetadataQuery};
use crate::domain::{Project, ProjectIdentity, Session, SessionIdentity};
use crate::presentation::app::{fmt_relative, Snapshot};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::collections::HashSet;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Project(ProjectIdentity),
    Session(SessionIdentity),
}

enum Row<'a> {
    Project {
        key: Key,
        project: &'a Project,
        session_count: usize,
        expanded: bool,
    },
    Session {
        key: Key,
        session: &'a Session,
        depth: usize,
    },
}

impl Row<'_> {
    fn key(&self) -> &Key {
        match self {
            Self::Project { key, .. } | Self::Session { key, .. } => key,
        }
    }
}

struct ProjectGroup<'a> {
    project: &'a Project,
    sessions: Vec<&'a Session>,
    latest_activity_ms: i64,
}

#[derive(Default)]
pub(super) struct State {
    selection: Option<Key>,
    expanded_projects: HashSet<ProjectIdentity>,
}

impl State {
    pub(super) fn reconcile(&mut self, snap: &Snapshot, labels: &dyn SessionMetadataQuery) {
        self.selection = effective_selection(self.selection.as_ref(), &self.rows(snap, labels));
    }

    pub(super) fn move_vertical(
        &mut self,
        snap: &Snapshot,
        labels: &dyn SessionMetadataQuery,
        down: bool,
    ) {
        let rows = self.rows(snap, labels);
        let Some(selection) = effective_selection(self.selection.as_ref(), &rows) else {
            return;
        };
        let Some(index) = rows.iter().position(|row| row.key() == &selection) else {
            return;
        };
        let next = if down {
            (index + 1).min(rows.len().saturating_sub(1))
        } else {
            index.saturating_sub(1)
        };
        self.selection = rows.get(next).map(|row| row.key().clone());
    }

    pub(super) fn activate(&mut self, snap: &Snapshot, labels: &dyn SessionMetadataQuery) {
        let rows = self.rows(snap, labels);
        let Some(Key::Project(project)) = effective_selection(self.selection.as_ref(), &rows)
        else {
            return;
        };
        if !self.expanded_projects.remove(&project) {
            self.expanded_projects.insert(project.clone());
        }
        self.selection = Some(Key::Project(project));
    }

    pub(super) fn selected_identities(
        &self,
        snap: &Snapshot,
        labels: &dyn SessionMetadataQuery,
    ) -> Option<Vec<SessionIdentity>> {
        let rows = self.rows(snap, labels);
        match effective_selection(self.selection.as_ref(), &rows)? {
            Key::Project(project) => Some(
                sessions(snap, labels)
                    .into_iter()
                    .filter(|session| {
                        session
                            .project
                            .as_ref()
                            .is_some_and(|candidate| candidate.identity == project)
                    })
                    .map(SessionIdentity::from)
                    .collect(),
            ),
            Key::Session(identity) => Some(vec![identity]),
        }
    }

    fn rows<'a>(&self, snap: &'a Snapshot, labels: &dyn SessionMetadataQuery) -> Vec<Row<'a>> {
        let groups = groups(snap, labels);
        let mut rows = Vec::new();
        for group in groups {
            let project_identity = group.project.identity.clone();
            let expanded = self.expanded_projects.contains(&project_identity);
            let session_count = group.sessions.len();
            rows.push(Row::Project {
                key: Key::Project(project_identity.clone()),
                project: group.project,
                session_count,
                expanded,
            });
            if !expanded {
                continue;
            }
            rows.extend(group.sessions.into_iter().map(|session| Row::Session {
                key: Key::Session(SessionIdentity::from(session)),
                session,
                depth: 1,
            }));
        }
        rows
    }
}

pub(super) fn sessions<'a>(
    snap: &'a Snapshot,
    labels: &dyn SessionMetadataQuery,
) -> Vec<&'a Session> {
    snap.sessions
        .iter()
        .filter(|session| !is_system_temporary_cwd(session.cwd.as_deref()))
        .filter(|session| session.project.is_some())
        .filter(|session| !session.state.is_open())
        .filter(|session| {
            labels.disposition(session.provider, &session.id) == Some(CurationDisposition::Hidden)
        })
        .collect()
}

fn groups<'a>(snap: &'a Snapshot, labels: &dyn SessionMetadataQuery) -> Vec<ProjectGroup<'a>> {
    let mut projects: Vec<ProjectGroup<'a>> = Vec::new();
    for session in sessions(snap, labels) {
        let project = session
            .project
            .as_ref()
            .expect("Hidden Tree requires a Project");
        let project_index = projects
            .iter()
            .position(|group| group.project.identity == project.identity)
            .unwrap_or_else(|| {
                projects.push(ProjectGroup {
                    project,
                    sessions: Vec::new(),
                    latest_activity_ms: session.last_activity_ms,
                });
                projects.len() - 1
            });
        let group = &mut projects[project_index];
        group.latest_activity_ms = group.latest_activity_ms.max(session.last_activity_ms);
        group.sessions.push(session);
    }
    for project in &mut projects {
        project
            .sessions
            .sort_by_key(|session| std::cmp::Reverse(session.last_activity_ms));
    }
    projects.sort_by(|left, right| {
        right
            .latest_activity_ms
            .cmp(&left.latest_activity_ms)
            .then_with(|| left.project.label.cmp(&right.project.label))
            .then_with(|| {
                left.project
                    .identity
                    .display_path()
                    .cmp(right.project.identity.display_path())
            })
    });
    projects
}

fn effective_selection(selection: Option<&Key>, rows: &[Row<'_>]) -> Option<Key> {
    selection
        .filter(|selection| rows.iter().any(|row| row.key() == *selection))
        .cloned()
        .or_else(|| rows.first().map(|row| row.key().clone()))
}

pub(super) fn counts(snap: &Snapshot, labels: &dyn SessionMetadataQuery) -> (usize, usize) {
    let sessions = sessions(snap, labels);
    let projects = sessions
        .iter()
        .filter_map(|session| session.project.as_ref())
        .map(|project| &project.identity)
        .collect::<HashSet<_>>()
        .len();
    (projects, sessions.len())
}

pub(super) fn draw(
    f: &mut Frame,
    area: Rect,
    snap: &Snapshot,
    state: &State,
    labels: &dyn SessionMetadataQuery,
    theme: Theme,
) {
    let tree_rows = state.rows(snap, labels);
    let selected = effective_selection(state.selection.as_ref(), &tree_rows);
    let visible_sessions = tree_rows
        .iter()
        .filter_map(|row| match row {
            Row::Session { session, .. } => Some(*session),
            _ => None,
        })
        .collect::<Vec<_>>();
    let identities = visible_sessions
        .iter()
        .map(|session| (session.provider, session.id.as_str()))
        .collect::<Vec<_>>();
    let mut titles = labels.titles_for(&identities).into_iter();
    let content_width = usize::from(area.width.saturating_sub(2));
    let mut lines = Vec::new();
    for row in &tree_rows {
        let is_selected = selected.as_ref().is_some_and(|key| key == row.key());
        let line = match row {
            Row::Project {
                project,
                session_count,
                expanded,
                ..
            } => {
                let prefix = if *expanded { "▾ " } else { "▸ " };
                let prefix = format!("{}{prefix}", if is_selected { ">" } else { " " });
                let count = format!(" ({session_count})");
                let label_width = content_width.saturating_sub(
                    UnicodeWidthStr::width(prefix.as_str())
                        + UnicodeWidthStr::width(count.as_str()),
                );
                retained_selection(
                    Line::from(vec![
                        Span::styled(
                            prefix,
                            Style::default().fg(if is_selected {
                                theme.accent
                            } else {
                                theme.muted
                            }),
                        ),
                        Span::styled(
                            text::shorten_end(&project.label, label_width),
                            Style::default()
                                .fg(theme.foreground)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(count, Style::default().fg(theme.muted)),
                    ]),
                    is_selected,
                    content_width,
                    theme,
                )
            }
            Row::Session { session, depth, .. } => {
                let title = titles.next().expect("every Hidden Session has a Title");
                let age = format!(
                    "  {}",
                    fmt_relative(session.last_activity_ms, snapshot_now_ms(snap))
                );
                let prefix = format!(
                    "{}{} ",
                    "  ".repeat(*depth),
                    if is_selected { ">" } else { " " }
                );
                let provider = provider_mark::icon(session.provider);
                let title_width = content_width.saturating_sub(
                    UnicodeWidthStr::width(prefix.as_str())
                        + UnicodeWidthStr::width(provider)
                        + 1
                        + UnicodeWidthStr::width(age.as_str()),
                );
                retained_selection(
                    Line::from(vec![
                        Span::styled(prefix, Style::default().fg(theme.accent)),
                        Span::styled(
                            provider,
                            provider_mark::style(session.provider, theme, Style::default()),
                        ),
                        Span::raw(" "),
                        Span::styled(
                            text::shorten_end(&title, title_width),
                            Style::default().fg(theme.foreground),
                        ),
                        Span::styled(age, Style::default().fg(theme.muted)),
                    ]),
                    is_selected,
                    content_width,
                    theme,
                )
            }
        };
        lines.push(line);
    }
    let visible_rows = usize::from(area.height.saturating_sub(2));
    let selected_index = selected
        .as_ref()
        .and_then(|selected| tree_rows.iter().position(|row| row.key() == selected));
    let scroll = selected_index
        .unwrap_or_default()
        .saturating_sub(visible_rows.saturating_sub(1));
    let (project_count, session_count) = counts(snap, labels);
    let project_label = if project_count == 1 {
        "Project"
    } else {
        "Projects"
    };
    let session_label = if session_count == 1 {
        "Session"
    } else {
        "Sessions"
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(Span::styled(
            " HIDDEN SESSIONS ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title(
            Line::from(Span::styled(
                format!(" {project_count} {project_label} · {session_count} {session_label} "),
                Style::default().fg(theme.muted),
            ))
            .right_aligned(),
        );
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
        area,
    );
}
