//! All-page left pane composition for navigation and selected-session facts.

use super::{projects, selected_session, Ui};
use crate::app::Snapshot;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

const MIN_TREE_HEIGHT: u16 = 4;

/// Draw one framed browser with the Project Tree above a selected-session divider.
pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let (project_count, open_session_count) = projects::tree_counts(snap);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(
            Style::default().fg(if ui.all_region == super::AllRegion::Now {
                theme.accent
            } else {
                theme.border
            }),
        )
        .title(Line::from(vec![
            Span::styled(
                " PROJECT TREE ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{project_count} "),
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
                Span::styled(" OPEN ", Style::default().fg(theme.muted)),
            ])
            .right_aligned(),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let details_height = selected_session::preferred_height(snap, ui, inner.width)
        .min(inner.height.saturating_sub(MIN_TREE_HEIGHT));
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(MIN_TREE_HEIGHT),
            Constraint::Length(details_height),
        ])
        .split(inner);

    projects::draw(f, sections[0], snap, ui);
    selected_session::draw(f, sections[1], snap, ui);
}

#[cfg(test)]
#[path = "session_browser_test.rs"]
mod tests;
