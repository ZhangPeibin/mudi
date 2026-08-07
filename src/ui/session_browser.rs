//! All-page left pane composition for navigation and selected-session facts.

use super::{project_tree, selected_session, Ui};
use crate::app::Snapshot;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

const MIN_TREE_HEIGHT: u16 = 4;

/// Draw the Project Tree above Selected Session Details in one unframed pane.
pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let (project_count, open_session_count) = project_tree::counts(snap);
    let pane = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let project_count = project_count.to_string();
    let open_session_count = open_session_count.to_string();
    let decorated_width = " PROJECT TREE ".chars().count()
        + project_count.chars().count()
        + 1
        + open_session_count.chars().count()
        + " OPEN ".chars().count()
        + 1;
    let compact_header = decorated_width > usize::from(area.width);
    let project_heading = if compact_header {
        "PROJECT TREE "
    } else {
        " PROJECT TREE "
    };
    let project_count_suffix = if compact_header { "" } else { " " };
    let open_suffix = if compact_header { " OPEN" } else { " OPEN " };
    let open_label_width = open_session_count.chars().count()
        + open_suffix.chars().count()
        + usize::from(!compact_header);
    let header = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(open_label_width as u16),
        ])
        .split(pane[0]);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                project_heading,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{project_count}{project_count_suffix}"),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        header[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                open_session_count,
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(open_suffix, Style::default().fg(theme.muted)),
        ])),
        header[1],
    );
    let inner = pane[1];

    let details_height = selected_session::preferred_height(snap, ui, inner.width)
        .min(inner.height.saturating_sub(MIN_TREE_HEIGHT));
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(MIN_TREE_HEIGHT),
            Constraint::Length(details_height),
        ])
        .split(inner);

    project_tree::draw(
        f,
        sections[0],
        snap,
        &ui.project_tree,
        &ui.labels,
        ui.all_region == super::AllRegion::Now,
        theme,
    );
    selected_session::draw(f, sections[1], snap, ui);
}

#[cfg(test)]
#[path = "session_browser_test.rs"]
mod tests;
