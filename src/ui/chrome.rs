//! 全局页头、页脚和上下文状态提示。

use super::{project_tree, Ui};
use crate::app::{fmt_tokens, Snapshot};
use crate::read_model::RefreshHealth;
use crate::tasks::OperationTask;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::time::Instant;

/// 绘制产品名、版本、全局 Usage 和 Project 数量组成的页头。
pub(super) fn draw_header(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let wide = area.width >= 100;
    let compact_projects = area.width < 88;
    let left = vec![
        Span::styled(
            " SessionMesh ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("| ", Style::default().fg(theme.border)),
        Span::styled(
            format!("v{} ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme.muted),
        ),
    ];

    let (input, output) = snap
        .sessions
        .iter()
        .fold((0_u64, 0_u64), |totals, session| {
            (
                totals.0.saturating_add(session.usage.input),
                totals.1.saturating_add(session.usage.output),
            )
        });
    let project_count = project_tree::counts(snap).0;
    let mut right = vec![
        Span::styled(" In ", Style::default().fg(theme.muted)),
        Span::styled(
            fmt_tokens(input),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if wide { "  Out " } else { " Out " },
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            fmt_tokens(output),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
    ];
    if compact_projects {
        right.push(Span::styled("P ", Style::default().fg(theme.muted)));
        right.push(Span::styled(
            project_count.to_string(),
            Style::default().fg(theme.foreground),
        ));
    } else {
        right.push(Span::styled(
            format!(
                "{project_count} Project{}",
                if project_count == 1 { "" } else { "s" }
            ),
            Style::default().fg(theme.foreground),
        ));
    }
    right.push(Span::raw(" "));

    f.render_widget(
        Block::default()
            .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
            .border_style(Style::default().fg(theme.border))
            .title(Line::from(left))
            .title(Line::from(right).right_aligned()),
        area,
    );
}
/// 根据当前页面和模态状态绘制可执行快捷键与扫描状态。
pub(super) fn draw_footer(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let compact = area.width < 100;
    let can_edit_session = ui.labels.is_editable() && ui.selected_session(snap).is_some();
    let can_handoff = ui.detail_session(snap).is_some();
    let now = Instant::now();
    let status_notice = ui
        .status_notice
        .as_ref()
        .filter(|(_, until)| now < *until)
        .map(|(message, _)| message.clone());
    let label_error = ui.labels.error().map(str::to_string);
    let task_detail = ui.detail_task(snap);
    // 左侧快捷键按最具体的模态上下文优先，避免展示当前不可执行的命令。
    let left = if ui.inventory_delete_confirmation.is_some() {
        "Enter Delete | Esc Keep Hidden"
    } else if ui.hide_confirmation.is_some() {
        "Enter Hide | Esc Keep Visible"
    } else if ui.hidden_tree_open {
        "j/k Move | Enter Expand/Collapse | u Restore | d Delete | Esc Back"
    } else if ui.new_session.is_some() {
        ""
    } else if ui.task_retry_confirmation.is_some() {
        "Left/Right Provider | Enter Authorize Retry | Esc Cancel"
    } else if ui.task_cancellation_confirmation.is_some() {
        "Enter Confirm Cancel | Esc Keep Task"
    } else if ui.handoff_confirmation.is_some() {
        "Left/Right Provider | Enter Authorize | Esc Cancel"
    } else if task_detail.is_some_and(OperationTask::is_being_deleted) {
        "Esc Back | j/k Scroll | ? Guide | q Quit"
    } else if task_detail.is_some_and(OperationTask::offers_retry) {
        "d Delete | r Retry | Esc | j/k | ? | q Quit"
    } else if task_detail.is_some_and(|task| task.state == crate::tasks::OperationTaskState::Ready)
    {
        "d Delete | o Open Terminal | c Copy Command | Esc"
    } else if task_detail.is_some_and(|task| !task.state.is_terminal()) {
        "x Cancel | Esc Back | j/k Scroll | ? Guide | q Quit"
    } else if task_detail.is_some() {
        "d Delete | Esc | j/k | ? | q Quit"
    } else if compact && status_notice.is_some() {
        ""
    } else if ui.detail.is_some() && compact && can_edit_session && can_handoff {
        "Esc | j/k Fields | c Copy | e Edit | h Handoff | ? | q Quit"
    } else if ui.detail.is_some() && compact && can_edit_session {
        "Esc | j/k Fields | c Copy | e Edit | ? Guide | q Quit"
    } else if ui.detail.is_some() && compact {
        "Esc Back | j/k Fields | c Copy | ? Guide | q Quit"
    } else if ui.detail.is_some() && can_edit_session && can_handoff {
        "Esc | j/k Fields | c Copy | e Edit session | h Handoff | ? | q Quit"
    } else if ui.detail.is_some() && can_edit_session {
        "Esc | j/k Fields | c Copy | e Edit session | ? Guide | q Quit"
    } else if ui.detail.is_some() {
        "Esc | j/k Fields | c Copy | ? Guide | q Quit"
    } else if compact && can_edit_session {
        "j/k | Left/Right | Enter | e Edit | ? | q Quit"
    } else if compact {
        "j/k | Left/Right | Enter | ? Guide | q Quit"
    } else if can_edit_session {
        "j/k | Left/Right | Enter | e Edit session | r | ? | q Quit"
    } else {
        "j/k | Left/Right | Enter | r | ? Guide | q Quit"
    };
    // 临时操作结果优先于持久错误，持久错误又优先于普通扫描状态。
    let status = if let Some(status_notice) = status_notice.as_ref() {
        status_notice.clone()
    } else if let Some(label_error) = label_error.as_ref() {
        label_error.clone()
    } else {
        refresh_status(snap)
    };
    let shows_price_note = ui.detail.is_some();
    let right = if status_notice.is_some() {
        status
    } else if compact && shows_price_note {
        "Estimated using API rates".to_string()
    } else if ui.detail.is_none() {
        status
    } else {
        format!("{status}   ·   Estimated using API rates")
    };
    let cols = if label_error.is_some() && status_notice.is_none() {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(0), Constraint::Min(1)])
            .split(area)
    } else if compact {
        let available = area
            .width
            .saturating_sub(left.chars().count() as u16)
            .saturating_sub(1);
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length((right.chars().count() as u16).min(available)),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area)
    };
    f.render_widget(
        Paragraph::new(Span::styled(left, Style::default().fg(theme.muted))),
        cols[0],
    );
    let status_color = if status_notice.is_some() {
        theme.accent
    } else if label_error.is_some() {
        theme.critical
    } else {
        theme.muted
    };
    f.render_widget(
        Paragraph::new(Span::styled(right, Style::default().fg(status_color)))
            .alignment(Alignment::Right),
        cols[1],
    );
}

fn refresh_status(snap: &Snapshot) -> String {
    if !snap.ready {
        return "Starting...".to_string();
    }
    match &snap.health {
        RefreshHealth::Fresh => format!(
            "watching · {} sessions · {} parsed / {} skipped{}",
            snap.stats.discovered,
            snap.stats.parsed,
            snap.stats.skipped,
            if snap.stats.pruned > 0 {
                format!(" · {} pruned", snap.stats.pruned)
            } else {
                String::new()
            }
        ),
        RefreshHealth::Degraded { diagnostics } => {
            format!("degraded · {} refresh diagnostic(s)", diagnostics.len())
        }
    }
}

#[cfg(test)]
#[path = "chrome_test.rs"]
mod tests;
