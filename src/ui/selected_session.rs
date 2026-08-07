//! Compact facts that complement, rather than repeat, the selected Tree row.

use super::{field::FieldLine, path_display, Ui};
use crate::app::{fmt_tokens, Snapshot};
use crate::domain::Session;
use crate::tasks::OperationTask;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

const COMPACT_USAGE_WIDTH: usize = 45;
const DETAILS_PADDING: Padding = Padding::new(1, 0, 1, 0);
const DETAILS_BORDER_HEIGHT: usize = 1;

fn content_width(width: u16) -> usize {
    usize::from(width.saturating_sub(DETAILS_PADDING.left + DETAILS_PADDING.right))
}

pub(super) fn preferred_height(snap: &Snapshot, ui: &Ui, width: u16) -> u16 {
    detail_lines(snap, ui, content_width(width))
        .len()
        .saturating_add(
            DETAILS_BORDER_HEIGHT + usize::from(DETAILS_PADDING.top + DETAILS_PADDING.bottom),
        ) as u16
}

pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    if area.height == 0 {
        return;
    }
    let theme = ui.theme.palette();
    let lines = detail_lines(snap, ui, content_width(area.width));
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme.border))
                    .padding(DETAILS_PADDING)
                    .title(Span::styled(
                        " DETAILS ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    )),
            ),
        area,
    );
}

fn detail_lines(snap: &Snapshot, ui: &Ui, content_width: usize) -> Vec<Line<'static>> {
    let theme = ui.theme.palette();
    let Some(session) = ui.editor_session(snap) else {
        return vec![Line::from(Span::styled(
            "No Open Session selected.",
            Style::default().fg(theme.muted),
        ))];
    };

    let mut lines = vec![FieldLine::new("Provider", session.provider.label())
        .label_width(10)
        .render(content_width, theme)];
    if let Some(worktree) = session.worktree.as_ref() {
        lines.push(
            FieldLine::new(
                "Worktree",
                &format!(
                    "{} @ {}",
                    path_display::home_relative(&worktree.root),
                    worktree.branch.as_deref().unwrap_or("detached HEAD")
                ),
            )
            .label_width(10)
            .keep_end()
            .render(content_width, theme),
        );
    } else if let Some(cwd) = session.cwd.as_deref() {
        lines.push(
            FieldLine::new(
                "Directory",
                &path_display::home_relative(std::path::Path::new(cwd)),
            )
            .label_width(10)
            .keep_end()
            .render(content_width, theme),
        );
    }
    if let Some(model) = session.latest_model.as_deref() {
        lines.push(
            FieldLine::new("Model", model)
                .label_width(10)
                .keep_end()
                .render(content_width, theme),
        );
    }
    if let Some(run) = session
        .current_run_usage
        .as_ref()
        .filter(|_| content_width >= COMPACT_USAGE_WIDTH)
    {
        lines.push(
            FieldLine::new(
                "Run usage",
                &format!(
                    "in {} | out {} | read {} | create {}",
                    fmt_tokens(run.usage.input),
                    fmt_tokens(run.usage.output),
                    fmt_tokens(run.usage.cache_read),
                    fmt_tokens(run.usage.cache_write),
                ),
            )
            .label_width(10)
            .render(content_width, theme),
        );
    }
    if let Some(task) = task_for_session(session, &snap.tasks) {
        lines.push(
            FieldLine::new("Task", &format!("Handoff {}", task.state.label()))
                .label_width(10)
                .render(content_width, theme),
        );
    }
    lines
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

#[cfg(test)]
#[path = "selected_session_test.rs"]
mod tests;
