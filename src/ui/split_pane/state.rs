use super::{layout, Areas};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum MouseOutcome {
    Ignored,
    Dragging,
    Committed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum ResizeDirection {
    Left,
    Right,
}

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

    pub(in crate::ui) fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> MouseOutcome {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if self.hits_divider(event, area) => {
                self.dragging = true;
                MouseOutcome::Dragging
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.move_divider(event.column, area);
                MouseOutcome::Dragging
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.move_divider(event.column, area);
                self.dragging = false;
                MouseOutcome::Committed
            }
            _ => MouseOutcome::Ignored,
        }
    }

    pub(in crate::ui) fn cancel_drag(&mut self) {
        self.dragging = false;
    }

    pub(in crate::ui) fn adjust(&mut self, area: Rect, direction: ResizeDirection) {
        let current = self
            .preferred_left_width
            .unwrap_or_else(|| self.layout(area).left.width)
            .max(layout::MIN_LEFT_WIDTH);
        let desired = match direction {
            ResizeDirection::Left => current.saturating_sub(1),
            ResizeDirection::Right => current.saturating_add(1),
        };
        self.preferred_left_width = Some(desired.max(layout::MIN_LEFT_WIDTH));
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
