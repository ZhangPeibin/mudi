//! Conversation workspace for the All-page editor.

use super::{field::FieldLine, Ui};
use crate::app::Snapshot;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Draw the deliberately unavailable conversation surface for the selected identity.
pub(super) fn draw_workspace(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let pane = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Session Workspace ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))),
        pane[0],
    );
    let content_width = pane[1].width as usize;
    let lines = match ui.editor_session(snap) {
        Some(session) => vec![
            FieldLine::new(
                "Session",
                &format!(
                    "{} / {}",
                    session.provider.label(),
                    ui.labels.title(session.provider, &session.id)
                ),
            )
            .keep_end()
            .render(content_width, theme),
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
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Left), pane[1]);
}

#[cfg(test)]
#[path = "editor_test.rs"]
mod tests;
