//! Claude Code 与 Codex CLI 的 Provider 会话清单。

use super::{
    context, is_system_temporary_cwd, selected_style, snapshot_now_ms, text, theme::Theme,
    ProviderDisclosureKey, ProviderDisclosureKind, ProviderPageRow, ProviderProjectKey, Ui,
};
use crate::app::{fmt_context, fmt_cost, fmt_relative, fmt_tokens, Snapshot};
use crate::domain::{Project, Provider, Session, SessionState};
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;
use std::cmp::Reverse;
use std::collections::HashSet;

const RECENT_SESSION_LIMIT: usize = 5;

struct ProjectGroup<'a> {
    project: Option<&'a Project>,
    recent_sessions: Vec<&'a Session>,
    older_sessions: Vec<&'a Session>,
}

struct Inventory<'a> {
    projects: Vec<ProjectGroup<'a>>,
    unavailable_cwd: Vec<&'a Session>,
}

fn inventory(sessions: &[Session], provider: Provider, now_ms: i64) -> Inventory<'_> {
    const SEVEN_DAYS_MS: i64 = 7 * 86_400_000;
    let cutoff_ms = now_ms - SEVEN_DAYS_MS;
    let mut projects: Vec<ProjectGroup<'_>> = Vec::new();
    let mut unavailable_cwd = Vec::new();

    for session in sessions
        .iter()
        .filter(|session| session.provider == provider)
    {
        if is_system_temporary_cwd(session.cwd.as_deref()) {
            continue;
        }
        if !session.state.is_open() && session.project.is_none() {
            unavailable_cwd.push(session);
            continue;
        }
        let project = projects.iter_mut().find(|group| {
            group.project.map(|project| &project.identity)
                == session.project.as_ref().map(|project| &project.identity)
        });
        if let Some(project) = project {
            if session.last_activity_ms >= cutoff_ms {
                project.recent_sessions.push(session);
            } else {
                project.older_sessions.push(session);
            }
        } else {
            projects.push(ProjectGroup {
                project: session.project.as_ref(),
                recent_sessions: (session.last_activity_ms >= cutoff_ms)
                    .then_some(session)
                    .into_iter()
                    .collect(),
                older_sessions: (session.last_activity_ms < cutoff_ms)
                    .then_some(session)
                    .into_iter()
                    .collect(),
            });
        }
    }

    for project in &mut projects {
        project
            .recent_sessions
            .sort_by_key(|session| Reverse(session.last_activity_ms));
        project
            .older_sessions
            .sort_by_key(|session| Reverse(session.last_activity_ms));
    }
    projects.sort_by(|left, right| {
        let newest = |project: &ProjectGroup<'_>| {
            project
                .recent_sessions
                .first()
                .or_else(|| project.older_sessions.first())
                .map_or(i64::MIN, |session| session.last_activity_ms)
        };
        newest(right).cmp(&newest(left)).then_with(|| {
            left.project
                .map_or("—", |project| project.label.as_str())
                .cmp(right.project.map_or("—", |project| project.label.as_str()))
        })
    });
    unavailable_cwd.sort_by_key(|session| Reverse(session.last_activity_ms));
    Inventory {
        projects,
        unavailable_cwd,
    }
}

/// 根据展开状态把 Provider 清单展平成实际渲染行。
pub(super) fn rows<'a>(
    sessions: &'a [Session],
    provider: Provider,
    now_ms: i64,
    expanded_disclosures: &HashSet<ProviderDisclosureKey>,
) -> Vec<ProviderPageRow<'a>> {
    let mut rows = Vec::new();
    let inventory = inventory(sessions, provider, now_ms);
    for project in inventory.projects {
        rows.push(ProviderPageRow::Project(project.project));
        let project_key = ProviderProjectKey {
            provider,
            project: project.project.map(|project| project.identity.clone()),
        };
        rows.extend(
            project
                .recent_sessions
                .iter()
                .take(RECENT_SESSION_LIMIT)
                .copied()
                .map(ProviderPageRow::Session),
        );
        if project.recent_sessions.len() > RECENT_SESSION_LIMIT {
            let key = ProviderDisclosureKey::MoreRecent(project_key.clone());
            let expanded = expanded_disclosures.contains(&key);
            rows.push(ProviderPageRow::Disclosure {
                key,
                kind: ProviderDisclosureKind::MoreRecent,
                count: project.recent_sessions.len() - RECENT_SESSION_LIMIT,
                expanded,
            });
            if expanded {
                rows.extend(
                    project
                        .recent_sessions
                        .iter()
                        .skip(RECENT_SESSION_LIMIT)
                        .copied()
                        .map(ProviderPageRow::Session),
                );
            }
        }
        if !project.older_sessions.is_empty() {
            let key = ProviderDisclosureKey::Older(project_key);
            let expanded = expanded_disclosures.contains(&key);
            rows.push(ProviderPageRow::Disclosure {
                key,
                kind: ProviderDisclosureKind::Older,
                count: project.older_sessions.len(),
                expanded,
            });
            if expanded {
                rows.extend(
                    project
                        .older_sessions
                        .into_iter()
                        .map(ProviderPageRow::Session),
                );
            }
        }
    }
    if !inventory.unavailable_cwd.is_empty() {
        let key = ProviderDisclosureKey::UnavailableCwd(provider);
        let expanded = expanded_disclosures.contains(&key);
        rows.push(ProviderPageRow::Disclosure {
            key,
            kind: ProviderDisclosureKind::UnavailableCwd,
            count: inventory.unavailable_cwd.len(),
            expanded,
        });
        if expanded {
            rows.extend(
                inventory
                    .unavailable_cwd
                    .into_iter()
                    .map(ProviderPageRow::Session),
            );
        }
    }
    rows
}

/// 绘制当前 Provider 页，并在宽表格与紧凑行布局之间响应式切换。
pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let provider_rows = ui.provider_page_rows(snap);
    let visible_sessions = provider_rows
        .iter()
        .filter_map(|row| match row {
            ProviderPageRow::Session(session) => Some(*session),
            _ => None,
        })
        .collect::<Vec<_>>();
    let visible_identities = visible_sessions
        .iter()
        .map(|session| (session.provider, session.id.as_str()))
        .collect::<Vec<_>>();
    let titles = ui.labels.titles_for(&visible_identities);
    let now = snapshot_now_ms(snap);
    let theme = ui.theme.palette();

    if provider_rows.is_empty() {
        let msg = if snap.ready {
            "No sessions for this Provider."
        } else {
            "Indexing… sessions will appear shortly."
        };
        let paragraph = Paragraph::new(msg)
            .style(Style::default().fg(theme.muted))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border)),
            );
        f.render_widget(paragraph, area);
        return;
    }

    if area.width < 120 {
        draw_compact(f, area, provider_rows, titles, now, ui, theme);
        return;
    }

    let header = Row::new(vec![
        Cell::from("State"),
        Cell::from("Title"),
        Cell::from("Model"),
        Cell::from("Context"),
        Cell::from("Lifetime Usage"),
        Cell::from("Est. $"),
        Cell::from("Updated"),
        Cell::from("Cmp"),
    ])
    .style(
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    );

    let mut rows = Vec::new();
    let mut selected_row = 0;
    let mut selectable = 0;
    let mut titles = titles.into_iter();
    for provider_row in provider_rows {
        match provider_row {
            ProviderPageRow::Project(project) => {
                rows.push(
                    Row::new(vec![
                        Cell::from("PROJECT"),
                        Cell::from(format!(
                            " {}",
                            project.map_or("—", |project| project.label.as_str())
                        )),
                    ])
                    .style(
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                );
            }
            ProviderPageRow::Session(session) => {
                if selectable == ui.selected {
                    selected_row = rows.len();
                }
                let title = titles.next().expect("every Session row has a Title");
                rows.push(session_row(session, &title, now, theme));
                selectable += 1;
            }
            ProviderPageRow::Disclosure {
                kind,
                count,
                expanded,
                ..
            } => {
                if selectable == ui.selected {
                    selected_row = rows.len();
                }
                rows.push(
                    Row::new(vec![
                        Cell::from(""),
                        Cell::from(format!(
                            "{} {} ({count})  {}",
                            if expanded { "-" } else { "+" },
                            kind.label(),
                            if expanded { "Collapse" } else { "Expand" }
                        )),
                    ])
                    .style(Style::default().fg(theme.muted)),
                );
                selectable += 1;
            }
        }
    }

    let widths = [
        Constraint::Length(7),
        Constraint::Length(20),
        Constraint::Length(18),
        Constraint::Length(14),
        Constraint::Length(34),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(3),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    format!(
                        " {} SESSIONS | grouped by Project ",
                        ui.tab.title().to_uppercase()
                    ),
                    Style::default().fg(theme.foreground),
                ))
                .title_bottom(
                    Line::from(Span::styled(
                        " showing full lifetime usage of matching sessions ",
                        Style::default().fg(theme.muted),
                    ))
                    .right_aligned(),
                ),
        )
        .row_highlight_style(selected_style(theme))
        .highlight_symbol("▌");
    let mut state = TableState::default();
    if selectable > 0 {
        state.select(Some(selected_row));
    }
    f.render_stateful_widget(table, area, &mut state);
}

fn state_cell(state: SessionState, theme: Theme) -> Cell<'static> {
    match state {
        SessionState::Busy => Cell::from(Span::styled(
            "● Busy",
            Style::default().fg(theme.busy).add_modifier(Modifier::BOLD),
        )),
        SessionState::Idle => Cell::from(Span::styled("○ Idle", Style::default().fg(theme.idle))),
        SessionState::Exited => Cell::from(Span::styled("· —", Style::default().fg(theme.exited))),
    }
}

fn context_style(session: &Session, theme: Theme) -> Style {
    match session.context.percent() {
        Some(percent) => context::style(context::role(percent), theme),
        _ => Style::default().fg(theme.foreground),
    }
}

fn lifetime_usage_text(session: &Session) -> String {
    format!(
        "I {}  O {}  CR {}  CW {}",
        fmt_tokens(session.usage.input),
        fmt_tokens(session.usage.output),
        fmt_tokens(session.usage.cache_read),
        fmt_tokens(session.usage.cache_write),
    )
}

fn session_row(session: &Session, title: &str, now: i64, theme: Theme) -> Row<'static> {
    let model = session.latest_model.clone().unwrap_or_else(|| "—".into());
    let compactions = if session.compaction_count() > 0 {
        format!("⤳{}", session.compaction_count())
    } else {
        "·".to_string()
    };
    let compaction_style = if session.compaction_count() > 0 {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.muted)
    };

    Row::new(vec![
        state_cell(session.state, theme),
        Cell::from(Span::styled(
            title.to_string(),
            Style::default().fg(theme.foreground),
        )),
        Cell::from(Span::styled(model, Style::default().fg(theme.muted))),
        Cell::from(Span::styled(
            fmt_context(session),
            context_style(session, theme),
        )),
        Cell::from(Span::styled(
            lifetime_usage_text(session),
            Style::default().fg(theme.foreground),
        )),
        Cell::from(Span::styled(
            fmt_cost(session.cost),
            Style::default().fg(theme.foreground),
        )),
        Cell::from(Span::styled(
            fmt_relative(session.last_activity_ms, now),
            Style::default().fg(theme.muted),
        )),
        Cell::from(Span::styled(compactions, compaction_style)),
    ])
}

fn draw_compact(
    f: &mut Frame,
    area: Rect,
    provider_rows: Vec<ProviderPageRow<'_>>,
    titles: Vec<String>,
    now: i64,
    ui: &Ui,
    theme: Theme,
) {
    let mut lines = vec![
        Line::from(Span::styled(
            "  State  Title               Model               Context",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "         Transcript Lifetime Usage                 Est.$  Updated  Cmp",
            Style::default().fg(theme.muted),
        )),
    ];
    let mut selectable = 0;
    let mut selected_line = 0;
    let mut titles = titles.into_iter();

    for provider_row in provider_rows {
        match provider_row {
            ProviderPageRow::Project(project) => {
                lines.push(Line::from(Span::styled(
                    format!(
                        "PROJECT  {}",
                        project.map_or("—", |project| project.label.as_str())
                    ),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            ProviderPageRow::Session(session) => {
                if selectable == ui.selected {
                    selected_line = lines.len();
                }
                let title = titles.next().expect("every Session row has a Title");
                lines.extend(compact_session_lines(
                    session,
                    &title,
                    now,
                    selectable == ui.selected,
                    theme,
                ));
                selectable += 1;
            }
            ProviderPageRow::Disclosure {
                kind,
                count,
                expanded,
                ..
            } => {
                let selected = selectable == ui.selected;
                if selected {
                    selected_line = lines.len();
                }
                lines.push(Line::from(Span::styled(
                    format!(
                        "{} {} {} ({count})  {}",
                        if selected { ">" } else { " " },
                        if expanded { "-" } else { "+" },
                        kind.label(),
                        if expanded { "Collapse" } else { "Expand" }
                    ),
                    if selected {
                        selected_style(theme)
                    } else {
                        Style::default().fg(theme.muted)
                    },
                )));
                selectable += 1;
            }
        }
    }

    let visible_height = area.height.saturating_sub(2) as usize;
    let scroll = selected_line.saturating_sub(visible_height.saturating_sub(2)) as u16;
    let title = format!(
        " {} SESSIONS | grouped by Project ",
        ui.tab.title().to_uppercase()
    );
    f.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn compact_session_lines(
    session: &Session,
    title: &str,
    now: i64,
    selected: bool,
    theme: Theme,
) -> [Line<'static>; 2] {
    let selected_or = |style| {
        if selected {
            selected_style(theme)
        } else {
            style
        }
    };
    let state = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => "· —",
    };
    let state_style = match session.state {
        SessionState::Busy => Style::default().fg(theme.busy).add_modifier(Modifier::BOLD),
        SessionState::Idle => Style::default().fg(theme.idle),
        SessionState::Exited => Style::default().fg(theme.exited),
    };
    let model = session.latest_model.as_deref().unwrap_or("—");
    let compactions = if session.compaction_count() > 0 {
        format!("⤳{}", session.compaction_count())
    } else {
        "·".into()
    };

    [
        Line::from(vec![
            Span::styled(
                if selected { "> " } else { "  " },
                selected_or(Style::default().fg(theme.foreground)),
            ),
            Span::styled(format!("{state:<7}"), selected_or(state_style)),
            Span::styled(
                text::fit(title, 18, false),
                selected_or(Style::default().fg(theme.foreground)),
            ),
            Span::styled(
                format!("{:<18}", text::shorten_end(model, 18)),
                selected_or(Style::default().fg(theme.muted)),
            ),
            Span::styled(
                fmt_context(session),
                selected_or(context_style(session, theme)),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("    {}  ", lifetime_usage_text(session)),
                selected_or(Style::default().fg(theme.foreground)),
            ),
            Span::styled(
                format!("{}  ", fmt_cost(session.cost)),
                selected_or(Style::default().fg(theme.foreground)),
            ),
            Span::styled(
                format!("{}  ", fmt_relative(session.last_activity_ms, now)),
                selected_or(Style::default().fg(theme.muted)),
            ),
            Span::styled(compactions, selected_or(Style::default().fg(theme.accent))),
        ]),
    ]
}

#[cfg(test)]
#[path = "provider_test.rs"]
mod tests;
