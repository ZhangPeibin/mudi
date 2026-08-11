//! Ratatui rendering for Project Tree rows.

use super::super::{
    context, current_attention, provenance_mark, provider_mark, retained_selection,
    snapshot_now_ms, text, theme::Theme,
};
use super::model::{ProjectTreeRow, SessionAgeBand};
use super::State;
use crate::app::{fmt_relative, Snapshot};
use crate::attention::{AttentionRank, AttentionSnapshot};
use crate::domain::{Project, Session, SessionState, Worktree};
use crate::session_labels::SessionLabels;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// Draw the Project Tree using only component state and presentation inputs.
pub(in crate::ui) fn draw(
    f: &mut Frame,
    area: Rect,
    snap: &Snapshot,
    state: &State,
    labels: &SessionLabels,
    theme: Theme,
) {
    let now_ms = snapshot_now_ms(snap);
    let attention = current_attention(snap);
    let tree_rows = state.rows(snap);
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
    let mut titles = labels.titles_for(&visible_identities).into_iter();
    let (_, open_session_count) = super::counts(snap);
    let content_width = area.width as usize;
    let mut lines = Vec::new();
    let selected = state.effective_selection(snap);

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

pub(super) fn open_session_line(
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
    let attention_marker = if attention_rank.shows_marker() {
        "!"
    } else {
        " "
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
    let provider = provider_mark::icon(session.provider);
    let provenance = provenance_mark::icon(session.provenance);
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
        provenance,
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
    let line = Line::from(vec![
        Span::styled(prefix, cursor_style),
        Span::styled(
            provider,
            provider_mark::style(session.provider, theme, Style::default()),
        ),
        Span::raw(SESSION_COLUMN_GAP),
        Span::styled(title, Style::default().fg(theme.foreground)),
        Span::raw(SESSION_COLUMN_GAP),
        Span::styled(
            provenance,
            provenance_mark::style(session.provenance, theme, Style::default()),
        ),
        Span::raw(SESSION_COLUMN_GAP),
        Span::styled(attention_marker, attention_style),
        Span::raw(SESSION_COLUMN_GAP),
        Span::styled(lifecycle_marker, lifecycle_style),
        Span::raw(SESSION_COLUMN_GAP),
        Span::styled(context_value, context_style),
        Span::styled(elapsed, Style::default().fg(theme.muted)),
    ]);
    retained_selection(line, position.selected, content_width, theme)
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
    let provider = provider_mark::icon(session.provider);
    let provenance = provenance_mark::icon(session.provenance);
    let elapsed = fmt_relative(session.last_activity_ms, now_ms);
    let fixed_width = UnicodeWidthStr::width(prefix.as_str())
        + UnicodeWidthStr::width(provider)
        + UnicodeWidthStr::width(provenance)
        + 3
        + UnicodeWidthStr::width(elapsed.as_str());
    let title = text::fit(title, content_width.saturating_sub(fixed_width), true);
    let cursor_style = if position.selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let line = Line::from(vec![
        Span::styled(prefix, cursor_style),
        Span::styled(
            provider,
            provider_mark::style(session.provider, theme, Style::default()),
        ),
        Span::raw(" "),
        Span::styled(title, Style::default().fg(theme.exited)),
        Span::raw(" "),
        Span::styled(
            provenance,
            provenance_mark::style(session.provenance, theme, Style::default()),
        ),
        Span::raw(" "),
        Span::styled(elapsed, Style::default().fg(theme.muted)),
    ]);
    retained_selection(line, position.selected, content_width, theme)
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
    let line = Line::from(vec![
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
    ]);
    retained_selection(line, selected, width, theme)
}

pub(super) fn worktree_line(
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
    let line = Line::from(vec![
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
    ]);
    retained_selection(line, selected, width, theme)
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
    let line = Line::from(vec![
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
    ]);
    retained_selection(line, selected, width, theme)
}

pub(super) fn group_prefix(depth: usize, selected: bool, expanded: bool) -> String {
    format!(
        "{}{}{} ",
        "  ".repeat(depth),
        if selected { ">" } else { " " },
        if expanded { "▾" } else { "▸" }
    )
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SessionTreePosition {
    pub(super) depth: usize,
    pub(super) selected: bool,
}

const SHOW_ELAPSED_WIDTH: usize = 42;
const CONTEXT_VALUE_WIDTH: usize = 4;
const ELAPSED_VALUE_WIDTH: usize = 4;
const SESSION_COLUMN_GAP: &str = " ";
