//! Reusable two-pane workspace layout with one stable vertical divider.

mod layout;
mod state;
mod view;

pub(in crate::ui) use layout::Areas;
pub(in crate::ui) use state::State;

use super::theme::Theme;
use ratatui::{layout::Rect, Frame};

pub(super) fn draw_divider(f: &mut Frame, area: Rect, dragging: bool, theme: Theme) {
    view::draw_divider(f, area, dragging, theme);
}
