//! Conversation workspace for the All-page editor.

use super::{text, Ui};
use crate::app::Snapshot;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

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
