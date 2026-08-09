//! All-page left pane composition for navigation and selected-session facts.

use super::{
    project_tree, region_border_style, region_title_style, selected_session, AllRegion, Ui,
};
use crate::app::Snapshot;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

const MIN_TREE_HEIGHT: u16 = 3;

#[derive(Clone, Copy)]
struct Areas {
    header: Rect,
    project_rows: Rect,
    details: Rect,
}

fn areas(area: Rect, snap: &Snapshot, ui: &Ui) -> Areas {
    let pane = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
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

    Areas {
        header: pane[0],
        project_rows: sections[0],
        details: sections[1],
    }
}

pub(super) fn project_rows_area(area: Rect, snap: &Snapshot, ui: &Ui) -> Rect {
    areas(area, snap, ui).project_rows
}

#[derive(Clone, Copy)]
struct HeaderFormat {
    project_heading: &'static str,
    project_count_prefix: &'static str,
    project_count_suffix: &'static str,
    open_count_prefix: &'static str,
    open_suffix: &'static str,
}

impl HeaderFormat {
    fn total_width(self, project_count: &str, open_session_count: &str) -> usize {
        self.project_heading.chars().count()
            + self.project_count_prefix.chars().count()
            + project_count.chars().count()
            + self.project_count_suffix.chars().count()
            + self.open_count_prefix.chars().count()
            + open_session_count.chars().count()
            + self.open_suffix.chars().count()
    }
}

const DECORATED_HEADER: HeaderFormat = HeaderFormat {
    project_heading: " PROJECT ",
    project_count_prefix: "",
    project_count_suffix: " ",
    open_count_prefix: " ",
    open_suffix: " OPEN  ",
};

const COMPACT_HEADER: HeaderFormat = HeaderFormat {
    project_heading: "PROJECT ",
    project_count_prefix: "P",
    project_count_suffix: " ",
    open_count_prefix: "O",
    open_suffix: "",
};

fn compact_count(count: usize) -> String {
    if count < 10_000 {
        return count.to_string();
    }

    let count = count as u128;
    for (threshold, suffix) in [
        (1_000_000_000_000_000_000_u128, "E"),
        (1_000_000_000_000_000_u128, "Q"),
        (1_000_000_000_000_u128, "T"),
        (1_000_000_000_u128, "B"),
        (1_000_000_u128, "M"),
        (1_000_u128, "K"),
    ] {
        if count >= threshold {
            return format!("{}{suffix}", count / threshold);
        }
    }

    unreachable!("counts below 10,000 return before unit selection")
}

/// Draw the Project Tree above Selected Session Details in one unframed pane.
pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let focused = ui.all_region == AllRegion::Now;
    let (project_count, open_session_count) = project_tree::counts(snap);
    let areas = areas(area, snap, ui);
    let full_project_count = project_count.to_string();
    let full_open_session_count = open_session_count.to_string();
    let (header_format, project_count, open_session_count) = if DECORATED_HEADER
        .total_width(&full_project_count, &full_open_session_count)
        > usize::from(area.width)
    {
        (
            COMPACT_HEADER,
            compact_count(project_count),
            compact_count(open_session_count),
        )
    } else {
        (
            DECORATED_HEADER,
            full_project_count,
            full_open_session_count,
        )
    };
    f.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(region_border_style(theme, focused))
            .title(Line::from(vec![
                Span::styled(
                    header_format.project_heading,
                    region_title_style(theme, focused),
                ),
                Span::styled(
                    format!(
                        "{}{project_count}{}",
                        header_format.project_count_prefix, header_format.project_count_suffix
                    ),
                    region_title_style(theme, focused),
                ),
            ]))
            .title(
                Line::from(vec![
                    Span::styled(
                        format!("{}{open_session_count}", header_format.open_count_prefix),
                        Style::default()
                            .fg(theme.foreground)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(header_format.open_suffix, Style::default().fg(theme.muted)),
                ])
                .right_aligned(),
            ),
        areas.header,
    );

    project_tree::draw(
        f,
        areas.project_rows,
        snap,
        &ui.project_tree,
        &ui.labels,
        theme,
    );
    selected_session::draw(f, areas.details, snap, ui);
}

#[cfg(test)]
#[path = "session_browser_test.rs"]
mod tests;
