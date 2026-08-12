//! 多个 UI 功能共用的几何布局辅助函数。

use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub(super) struct ScreenAreas {
    pub(super) header: Rect,
    pub(super) content: Rect,
    pub(super) footer: Rect,
}

pub(super) fn screen(area: Rect) -> ScreenAreas {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);
    ScreenAreas {
        header: chunks[0],
        content: chunks[1],
        footer: chunks[2],
    }
}

/// 在给定区域内按宽高百分比计算一个居中的矩形。
pub(super) fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vertical[1])[1]
}

/// Center a percentage-sized region while keeping its width within a readable limit.
pub(super) fn centered_rect_with_max_width(
    pct_x: u16,
    pct_y: u16,
    max_width: u16,
    area: Rect,
) -> Rect {
    let percentage_area = centered_rect(pct_x, pct_y, area);
    let width = percentage_area.width.min(max_width);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        percentage_area.y,
        width,
        percentage_area.height,
    )
}

/// Center a percentage-width region while capping both dimensions to its content needs.
pub(super) fn centered_rect_with_max_size(
    pct_x: u16,
    max_width: u16,
    max_height: u16,
    area: Rect,
) -> Rect {
    let percentage_area = centered_rect(pct_x, 100, area);
    let width = percentage_area.width.min(max_width);
    let height = area.height.min(max_height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
#[path = "layout_test.rs"]
mod tests;
