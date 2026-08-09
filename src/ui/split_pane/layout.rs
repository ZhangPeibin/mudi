use ratatui::layout::Rect;

pub(super) const MIN_LEFT_WIDTH: u16 = 24;
pub(super) const MIN_RIGHT_WIDTH: u16 = 48;
const DIVIDER_WIDTH: u16 = 1;
const DEFAULT_LEFT_PERCENT: u32 = 46;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) struct Areas {
    pub(in crate::ui) left: Rect,
    pub(in crate::ui) divider: Rect,
    pub(in crate::ui) right: Rect,
    pub(in crate::ui) bottom: Rect,
}

pub(super) fn split(area: Rect, preferred_left_width: Option<u16>) -> Areas {
    let desired = preferred_left_width
        .unwrap_or_else(|| ((u32::from(area.width) * DEFAULT_LEFT_PERCENT) / 100) as u16);
    let maximum_left = area.width.saturating_sub(DIVIDER_WIDTH + MIN_RIGHT_WIDTH);
    let minimum_left = MIN_LEFT_WIDTH.min(maximum_left);
    let left_width = desired.clamp(minimum_left, maximum_left);
    let divider_width = DIVIDER_WIDTH.min(area.width.saturating_sub(left_width));
    let right_width = area.width.saturating_sub(left_width + divider_width);
    let content_height = area.height.saturating_sub(1);
    let bottom_height = area.height.min(1);

    Areas {
        left: Rect::new(area.x, area.y, left_width, content_height),
        divider: Rect::new(
            area.x.saturating_add(left_width),
            area.y,
            divider_width,
            content_height,
        ),
        right: Rect::new(
            area.x.saturating_add(left_width + divider_width),
            area.y,
            right_width,
            content_height,
        ),
        bottom: Rect::new(
            area.x,
            area.y.saturating_add(content_height),
            area.width,
            bottom_height,
        ),
    }
}
