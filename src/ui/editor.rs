//! Selected Session inspector and conversation workspace for the All-page editor.

use super::{projects, text, Ui};
use crate::app::{fmt_context, Snapshot};
use crate::domain::{Session, SessionState};
use crate::tasks::OperationTask;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// Draw the compact, always-visible facts required while navigating the Session Tree.
pub(super) fn draw_selected_session(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let content_width = area.width.saturating_sub(2) as usize;
    let lines = match ui.editor_session(snap) {
        Some(session) => selected_session_lines(session, snap, ui, content_width),
        None => vec![Line::from(Span::styled(
            "No Open Session selected.",
            Style::default().fg(theme.muted),
        ))],
    };
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    " Selected Session ",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

/// Draw the deliberately unavailable conversation surface for the selected identity.
pub(super) fn draw_workspace(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let content_width = area.width.saturating_sub(2) as usize;
    let lines = match ui.editor_session(snap) {
        Some(session) => vec![
            field_line(
                "Session",
                &format!(
                    "{} / {}",
                    session.provider.label(),
                    ui.labels.title(session.provider, &session.id)
                ),
                content_width,
                theme.muted,
                theme.foreground,
                true,
            ),
            Line::from(Span::styled(
                "Conversation unavailable.",
                Style::default().fg(theme.muted),
            )),
        ],
        None => vec![Line::from(Span::styled(
            "Conversation unavailable. No Open Session selected.",
            Style::default().fg(theme.muted),
        ))],
    };
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border))
                    .title(Span::styled(
                        " Session Workspace ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    )),
            )
            .alignment(Alignment::Left),
        area,
    );
}

fn selected_session_lines(
    session: &Session,
    snap: &Snapshot,
    ui: &Ui,
    content_width: usize,
) -> Vec<Line<'static>> {
    let theme = ui.theme.palette();
    let lifecycle = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => "Exited",
    };
    let project = session
        .project
        .as_ref()
        .map(|project| project.label.as_str())
        .unwrap_or("N/A");
    let worktree = session.worktree.as_ref().map_or_else(
        || "N/A".to_string(),
        |worktree| {
            format!(
                "{} @ {}",
                projects::home_relative_path(&worktree.root),
                worktree.branch.as_deref().unwrap_or("detached HEAD")
            )
        },
    );
    let current_run = if !session.state.is_open() {
        "N/A (Exited Session)".to_string()
    } else if session.current_run_usage.is_some() {
        format!("{lifecycle} (usage available)")
    } else {
        format!("{lifecycle} (usage N/A)")
    };
    let task = task_for_session(session, &snap.tasks)
        .map(|task| format!("Handoff {}", task.state.label()))
        .unwrap_or_else(|| "N/A".to_string());

    vec![
        field_line(
            "Session Label",
            &ui.labels.title(session.provider, &session.id),
            content_width,
            theme.muted,
            theme.foreground,
            false,
        ),
        field_line(
            "Provider / Lifecycle",
            &format!("{} / {lifecycle}", session.provider.label()),
            content_width,
            theme.muted,
            theme.foreground,
            false,
        ),
        field_line(
            "Project",
            project,
            content_width,
            theme.muted,
            theme.foreground,
            false,
        ),
        field_line(
            "Worktree",
            &worktree,
            content_width,
            theme.muted,
            theme.foreground,
            true,
        ),
        field_line(
            "Latest model",
            session.latest_model.as_deref().unwrap_or("N/A"),
            content_width,
            theme.muted,
            theme.foreground,
            false,
        ),
        field_line(
            "Current Context",
            &fmt_context(session),
            content_width,
            theme.muted,
            theme.foreground,
            false,
        ),
        field_line(
            "Current Run",
            &current_run,
            content_width,
            theme.muted,
            theme.foreground,
            false,
        ),
        field_line(
            "Operation Task",
            &task,
            content_width,
            theme.muted,
            theme.foreground,
            false,
        ),
    ]
}

fn task_for_session<'a>(
    session: &Session,
    tasks: &'a [OperationTask],
) -> Option<&'a OperationTask> {
    let belongs_to_session = |task: &&OperationTask| {
        task.source_provider == session.provider && task.source_session_id == session.id
    };
    tasks
        .iter()
        .filter(belongs_to_session)
        .filter(|task| !task.state.is_terminal())
        .max_by_key(|task| (task.state_changed_at_ms, task.created_at_ms, &task.id))
        .or_else(|| {
            tasks
                .iter()
                .filter(belongs_to_session)
                .max_by_key(|task| (task.state_changed_at_ms, task.created_at_ms, &task.id))
        })
}

fn field_line(
    label: &str,
    value: &str,
    content_width: usize,
    label_color: ratatui::style::Color,
    value_color: ratatui::style::Color,
    keep_end: bool,
) -> Line<'static> {
    let label = format!("{label} ");
    let label_width = UnicodeWidthStr::width(label.as_str());
    let value = text::fit(value, content_width.saturating_sub(label_width), keep_end);
    Line::from(vec![
        Span::styled(label, Style::default().fg(label_color)),
        Span::styled(value, Style::default().fg(value_color)),
    ])
}

#[cfg(test)]
#[path = "editor_test.rs"]
mod tests;
