use super::super::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

pub(super) fn draw_divider(f: &mut Frame, area: Rect, dragging: bool, theme: Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    f.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(if dragging { theme.accent } else { theme.border })),
        area,
    );
}
