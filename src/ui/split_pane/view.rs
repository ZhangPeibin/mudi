use super::{super::theme::Theme, Areas};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub(super) fn draw_frame(f: &mut Frame, areas: Areas, dragging: bool, theme: Theme) {
    let divider_style = Style::default().fg(if dragging { theme.accent } else { theme.border });
    if areas.divider.width > 0 && areas.divider.height > 0 {
        f.render_widget(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(divider_style),
            areas.divider,
        );
    }

    if areas.bottom.width == 0 || areas.bottom.height == 0 {
        return;
    }

    f.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.border)),
        areas.bottom,
    );
    if areas.divider.width > 0 {
        f.render_widget(
            Paragraph::new(Span::styled("┴", divider_style)),
            Rect::new(areas.divider.x, areas.bottom.y, 1, 1),
        );
    }
}
