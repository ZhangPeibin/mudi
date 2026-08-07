use super::{layout, Areas};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

#[derive(Debug, Clone, Default)]
pub(in crate::ui) struct State {
    preferred_left_width: Option<u16>,
    dragging: bool,
}

impl State {
    pub(in crate::ui) fn new(preferred_left_width: Option<u16>) -> Self {
        Self {
            preferred_left_width,
            dragging: false,
        }
    }

    pub(in crate::ui) fn layout(&self, area: Rect) -> Areas {
        layout::split(area, self.preferred_left_width)
    }

    pub(in crate::ui) fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub(in crate::ui) fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> bool {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if self.hits_divider(event, area) => {
                self.dragging = true;
                false
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.move_divider(event.column, area);
                false
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.move_divider(event.column, area);
                self.dragging = false;
                true
            }
            _ => false,
        }
    }

    pub(in crate::ui) fn cancel_drag(&mut self) {
        self.dragging = false;
    }

    pub(in crate::ui) fn adjust(&mut self, area: Rect, increase: bool) {
        let current = self.layout(area).left.width;
        let desired = if increase {
            current.saturating_add(1)
        } else {
            current.saturating_sub(1)
        };
        self.preferred_left_width = Some(layout::split(area, Some(desired)).left.width);
    }

    pub(in crate::ui) fn preferred_left_width(&self) -> Option<u16> {
        self.preferred_left_width
    }

    fn hits_divider(&self, event: MouseEvent, area: Rect) -> bool {
        let divider = self.layout(area).divider;
        event.column >= divider.x
            && event.column < divider.x.saturating_add(divider.width)
            && event.row >= divider.y
            && event.row < divider.y.saturating_add(divider.height)
    }

    fn move_divider(&mut self, column: u16, area: Rect) {
        let desired = column.saturating_sub(area.x);
        self.preferred_left_width = Some(layout::split(area, Some(desired)).left.width);
    }
}
