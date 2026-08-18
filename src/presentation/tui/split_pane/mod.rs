//! Reusable two-pane workspace layout with one stable vertical divider.

mod layout;
mod state;
mod view;

pub(in crate::presentation::tui) use layout::Areas;
pub(in crate::presentation::tui) use state::{MouseOutcome, ResizeDirection, State};

use super::theme::Theme;
use ratatui::Frame;

pub(super) fn draw_frame(f: &mut Frame, areas: Areas, dragging: bool, theme: Theme) {
    view::draw_frame(f, areas, dragging, theme);
}
