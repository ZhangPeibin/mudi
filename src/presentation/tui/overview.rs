//! 总览页布局组合。

use super::{
    editor, managed_start, session_browser, signals, snapshot_now_ms, split_pane, tasks, Ui,
};
use crate::presentation::app::Snapshot;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

/// 顶部 Signals/Tasks 状态带的固定高度。
pub(super) const STATUS_HEIGHT: u16 = 8;

fn row_areas(area: Rect) -> (Rect, Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(STATUS_HEIGHT), Constraint::Min(3)])
        .split(area);
    (rows[0], rows[1])
}

pub(super) fn workspace_area(area: Rect) -> Rect {
    row_areas(area).1
}

/// 绘制总览页，并把各区域委托给对应功能模块。
pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let now_ms = snapshot_now_ms(snap);
    // First split the fixed status band from the editor workspace.
    let (status_area, workspace_area) = row_areas(area);
    // 只有可恢复的启动才占用状态带；历史失败记录不挤占总览空间。
    let status_panels_area = if ui.selected_managed_launch(snap).is_some() {
        let status_columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(area.width / 3), Constraint::Min(0)])
            .split(status_area);
        managed_start::draw(f, status_columns[0], snap, ui);
        status_columns[1]
    } else {
        status_area
    };
    let status_panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(status_panels_area);

    signals::draw_panel(
        f,
        status_panels[0],
        &snap.recent_signals,
        &snap.session_metadata,
        now_ms,
        ui,
    );
    tasks::draw_panel(
        f,
        status_panels[1],
        &snap.tasks,
        &snap.session_metadata,
        now_ms,
        ui,
    );
    let workspace = ui.workspace_split.layout(workspace_area);
    session_browser::draw(f, workspace.left, snap, ui);
    editor::draw_workspace(f, workspace.right, snap, ui);
    split_pane::draw_frame(
        f,
        workspace,
        ui.workspace_split.is_dragging(),
        ui.theme.palette(),
    );
}

#[cfg(test)]
#[path = "overview_test.rs"]
mod tests;
