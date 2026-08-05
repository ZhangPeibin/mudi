//! 总览页布局组合。

use super::{editor, projects, signals, snapshot_now_ms, tasks, Ui};
use crate::app::Snapshot;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

/// 顶部 Signals/Tasks 状态带的固定高度。
pub(super) const STATUS_HEIGHT: u16 = 8;

/// 绘制总览页，并把各区域委托给对应功能模块。
pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let now_ms = snapshot_now_ms(snap);
    // First split the fixed status band from the editor workspace.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(STATUS_HEIGHT), Constraint::Min(3)])
        .split(area);
    // 状态带左侧三分之一是产品约定的保留空白，不绘制任何面板。
    let reserved_width = area.width / 3;
    let status_columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(reserved_width), Constraint::Min(0)])
        .split(rows[0]);
    // 剩余右侧空间等分：Signals 在左，Tasks 在右。
    let status_panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(status_columns[1]);

    signals::draw_panel(f, status_panels[0], &snap.recent_signals, now_ms, ui);
    tasks::draw_panel(f, status_panels[1], &snap.tasks, now_ms, ui);
    let editor_columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(rows[1]);
    let tree_height = if editor_columns[0].height <= 14 {
        4
    } else {
        editor_columns[0].height / 2
    };
    let editor_left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(tree_height), Constraint::Min(8)])
        .split(editor_columns[0]);

    projects::draw(f, editor_left[0], snap, ui);
    editor::draw_selected_session(f, editor_left[1], snap, ui);
    editor::draw_workspace(f, editor_columns[1], snap, ui);
}

#[cfg(test)]
#[path = "overview_test.rs"]
mod tests;
