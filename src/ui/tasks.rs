//! Operation Task 的界面实现。
//!
//! 本模块拥有首页 Task 状态面板；调用方只需要提供当前任务列表和 UI 选择状态。

use super::{selected_style, text, AllRegion, Ui};
use crate::app::{fmt_datetime, fmt_relative, Snapshot};
use crate::domain::Provider;
use crate::tasks::OperationTask;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// 在首页状态带中绘制紧凑、可聚焦并可滚动的 Operation Task 列表。
pub(super) fn draw_panel(f: &mut Frame, area: Rect, tasks: &[OperationTask], now_ms: i64, ui: &Ui) {
    let theme = ui.theme.palette();
    let visible_rows = area.height.saturating_sub(2) as usize;
    let content_width = area.width.saturating_sub(2) as usize;
    // 选中项超出可见窗口时，让窗口尾部跟随选中项。
    let offset = ui
        .task_selected
        .saturating_sub(visible_rows.saturating_sub(1));
    let task_identities = tasks
        .iter()
        .map(|task| (task.source_provider, task.source_session_id.as_str()))
        .collect::<Vec<_>>();
    let task_titles = ui.labels.titles_for(&task_identities);
    let lines = tasks
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible_rows)
        .map(|(index, task)| {
            let provider = match task.source_provider {
                Provider::ClaudeCode => "CL",
                Provider::CodexCli => "CX",
            };
            let unread = task.state.is_terminal() && task.acknowledged_at_ms.is_none();
            let unread_label = if unread { "NEW " } else { "    " };
            let compact_unread_label = if unread { "NEW " } else { "" };
            let state = task.state.label();
            let transition_age = fmt_relative(task.state_changed_at_ms, now_ms);
            // 窄面板省略固定的 “Handoff” 前缀，把空间留给身份和执行状态。
            let compact = content_width < 38;
            let fixed_width = if compact {
                UnicodeWidthStr::width(compact_unread_label)
                    + UnicodeWidthStr::width(provider)
                    + UnicodeWidthStr::width(":")
                    + UnicodeWidthStr::width(" ")
                    + UnicodeWidthStr::width(state)
                    + UnicodeWidthStr::width(" ")
                    + UnicodeWidthStr::width(transition_age.as_str())
            } else {
                UnicodeWidthStr::width(unread_label)
                    + UnicodeWidthStr::width("Handoff ")
                    + UnicodeWidthStr::width(provider)
                    + UnicodeWidthStr::width(":")
                    + UnicodeWidthStr::width(" ")
                    + UnicodeWidthStr::width(state)
                    + UnicodeWidthStr::width(" ")
                    + UnicodeWidthStr::width(transition_age.as_str())
            };
            let source_label_width = content_width.saturating_sub(fixed_width).max(1);
            let resolved_label = &task_titles[index];
            let source_label = text::fit(resolved_label, source_label_width, true);
            let base_style = if unread {
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            let style = if ui.all_region == AllRegion::Tasks && index == ui.task_selected {
                selected_style(theme)
            } else {
                base_style
            };
            let text = if compact {
                format!("{compact_unread_label}{provider}:{source_label} {state} {transition_age}")
            } else {
                format!("{unread_label}Handoff {provider}:{source_label} {state} {transition_age}")
            };
            Line::from(Span::styled(text, style))
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if ui.all_region == AllRegion::Tasks {
                    theme.accent
                } else {
                    theme.border
                }))
                .title(Line::from(vec![
                    Span::styled(
                        " TASKS ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{} ", tasks.len()),
                        Style::default().fg(theme.foreground),
                    ),
                ])),
        ),
        area,
    );
}

/// 绘制 Operation Task 的状态、授权、执行证据、结果和当前可用动作。
pub(super) fn draw_detail(
    f: &mut Frame,
    area: Rect,
    task: &OperationTask,
    snap: &Snapshot,
    ui: &Ui,
) {
    let theme = ui.theme.palette();
    let field = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("{label:<24}"), Style::default().fg(theme.muted)),
            Span::styled(value, Style::default().fg(theme.foreground)),
        ])
    };
    let successor_is_indexed = task
        .successor_session_id
        .as_ref()
        .is_some_and(|session_id| {
            snap.sessions.iter().any(|session| {
                session.provider == task.successor_provider && &session.id == session_id
            })
        });
    let result_ready = task.state == crate::tasks::OperationTaskState::Ready
        && task.artifact_path.is_some()
        && task.successor_launch_plan.is_some();
    let acknowledgement = match (task.state.is_terminal(), task.acknowledged_at_ms) {
        (true, Some(at_ms)) => format!("Acknowledged {}", fmt_datetime(at_ms)),
        (true, None) => "Unread terminal outcome".into(),
        (false, _) => "Not terminal".into(),
    };
    let execution_trace = format!(
        "Worker {} | Provider {} | Prompt {}",
        task.worker_process_identity
            .map(|identity| format!("started {}", fmt_datetime(identity.started_at_ms)))
            .unwrap_or_else(|| "not started".into()),
        task.provider_process_identity
            .map(|identity| format!("started {}", fmt_datetime(identity.started_at_ms)))
            .unwrap_or_else(|| "not started".into()),
        task.prompt_sent_at_ms
            .map(|at_ms| format!("sent {}", fmt_datetime(at_ms)))
            .unwrap_or_else(|| "not sent".into()),
    );
    let provider_exit = task
        .provider_exited_at_ms
        .map(|at_ms| {
            format!(
                "Provider exited {} | Exit code {}",
                fmt_datetime(at_ms),
                task.provider_exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "Unavailable".into())
            )
        })
        .unwrap_or_else(|| "Not observed".into());
    let retry_of_task = task
        .retry_of_task_id
        .as_ref()
        .map(|predecessor_id| {
            if snap
                .tasks
                .iter()
                .any(|candidate| &candidate.id == predecessor_id)
            {
                predecessor_id.clone()
            } else {
                format!("{predecessor_id} (unavailable)")
            }
        })
        .unwrap_or_else(|| "None".into());
    let mut action = if task.is_being_deleted() {
        "Esc Back".to_string()
    } else if !task.state.is_terminal() {
        "x Cancel Task | Esc Back".to_string()
    } else if task.offers_retry() {
        "r Retry | d Delete | Esc Back".to_string()
    } else if task.state == crate::tasks::OperationTaskState::Ready {
        "o Open Terminal | c Copy Command | d Delete | Esc Close for Later".to_string()
    } else {
        "d Delete | Esc Back".to_string()
    };
    if successor_is_indexed {
        action = format!("Enter View Successor | {action}");
    }
    let mut lines = vec![
        field("Task ID", task.id.clone()),
        field("Operation", "Handoff".into()),
        field("Retry of Task", retry_of_task),
        Line::from(""),
        field("Source Provider", task.source_provider.label().into()),
        field("Source Session", task.source_session_id.clone()),
        field(
            "Working directory",
            task.source_working_directory.to_string_lossy().into_owned(),
        ),
        field("Successor Provider", task.successor_provider.label().into()),
        field(
            "Successor Session",
            task.successor_session_id
                .clone()
                .unwrap_or_else(|| "Not linked".into()),
        ),
        field(
            "Successor linkage",
            match (task.successor_session_id.is_some(), successor_is_indexed) {
                (true, true) => "Linked and indexed",
                (true, false) => "Linked, not indexed",
                (false, _) => "Not linked",
            }
            .into(),
        ),
        field(
            "Linked at",
            task.successor_linked_at_ms
                .map(fmt_datetime)
                .unwrap_or_else(|| "None".into()),
        ),
        Line::from(""),
        field("State", task.state.label().into()),
        field("Created", fmt_datetime(task.created_at_ms)),
        field("Latest transition", fmt_datetime(task.state_changed_at_ms)),
        field("Acknowledgement", acknowledgement),
    ];

    if task.worker_process_identity.is_some()
        || task.provider_process_identity.is_some()
        || task.prompt_sent_at_ms.is_some()
        || task.provider_exited_at_ms.is_some()
    {
        lines.extend([
            field("Execution trace", execution_trace),
            field("Provider exit", provider_exit),
        ]);
    }
    if task.deletion_requested_at_ms.is_some() {
        lines.extend([
            field(
                "Deletion",
                if task.deletion_failure_code.is_some() {
                    "Failed; will retry"
                } else {
                    "In progress"
                }
                .into(),
            ),
            field(
                "Deletion failure",
                task.deletion_failure_code
                    .clone()
                    .unwrap_or_else(|| "None".into()),
            ),
        ]);
    }
    if task.end_current_run_authorized {
        let waiting_reason = match task.waiting_reason.as_deref() {
            Some("source_current_run_busy") => "Source Current Run is Busy",
            Some("source_current_run_idle") => "Source Current Run is Idle",
            Some(_) => "Source Current Run is not ready",
            None => "None",
        };
        lines.extend([
            field("Waiting reason", waiting_reason.into()),
            field(
                "Authorization deadline",
                task.waiting_deadline_ms
                    .map(fmt_datetime)
                    .unwrap_or_else(|| "None".into()),
            ),
            field(
                "Current Run control",
                task.control_used_at_ms
                    .map(|at_ms| format!("Used {}", fmt_datetime(at_ms)))
                    .unwrap_or_else(|| "Not used".into()),
            ),
        ]);
    }
    lines.extend([
        Line::from(""),
        field(
            "Failure",
            task.failure_code.as_deref().unwrap_or("None").into(),
        ),
        field(
            "Failure detail",
            failure_summary(task.failure_code.as_deref()).into(),
        ),
        field(
            "Result",
            if result_ready { "Ready" } else { "Not ready" }.into(),
        ),
        field(
            "Artifact",
            if task.artifact_path.is_some() {
                "Published"
            } else {
                "Not published"
            }
            .into(),
        ),
        field(
            "Launch Plan",
            if task.successor_launch_plan.is_some() {
                "Ready"
            } else {
                "Not ready"
            }
            .into(),
        ),
        Line::from(""),
        field("Available actions", action),
    ]);
    if task.state == crate::tasks::OperationTaskState::Ready {
        lines.push(Line::from(Span::styled(
            "The Handoff Result remains in TASKS after closing.",
            Style::default().fg(theme.muted),
        )));
    }
    let inner_height = area.height.saturating_sub(2) as usize;
    let wrap = Wrap { trim: false };
    let total_lines = Paragraph::new(lines.clone())
        .wrap(wrap)
        .line_count(area.width.saturating_sub(2));
    let max_scroll = total_lines.saturating_sub(inner_height);
    let scroll = ui
        .detail
        .as_ref()
        .map_or(0, |detail| detail.scroll.min(max_scroll));
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .scroll((scroll as u16, 0))
            .wrap(wrap)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(Span::styled(
                        " Task detail ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    )),
            ),
        area,
    );
}

fn failure_summary(code: Option<&str>) -> &'static str {
    match code {
        Some("worker_start_failed") => "Detached Worker could not start.",
        Some("handoff_skill_missing") => {
            "The managed Handoff Skill was missing before Provider startup."
        }
        Some("handoff_skill_stale") => "The managed Handoff Skill changed after authorization.",
        Some("handoff_skill_unreadable") => {
            "The managed Handoff Skill became unreadable before Provider startup."
        }
        Some("handoff_skill_conflict") => {
            "The managed Handoff Skill path contains a conflicting entry."
        }
        Some("provider_invocation_failed") => "Provider invocation failed.",
        Some("provider_outcome_uncertain") => "Provider outcome could not be established safely.",
        Some("invalid_artifact") => "Candidate Markdown did not satisfy the Artifact contract.",
        Some("artifact_publish_failed") => "Validated Artifact publication failed.",
        Some("current_run_authority_expired") => {
            "Unused Current Run authority expired at its deadline."
        }
        Some("source_current_run_identity_changed") => {
            "The exact source Current Run identity changed."
        }
        Some("source_current_run_control_uncertain") => {
            "The graceful-exit outcome could not be established safely."
        }
        Some("source_current_run_observation_uncertain") => {
            "The source Current Run could not be observed conclusively."
        }
        Some("cancelled_by_user") => "Cancellation completed with no valid Handoff Result.",
        Some("worker_start_deadline_exceeded") => {
            "The detached Worker did not establish startup within 30 seconds."
        }
        Some("running_deadline_exceeded") => {
            "Generation reached 30 minutes and graceful termination completed."
        }
        Some("execution_outcome_uncertain") => {
            "Process and Result evidence could not establish the outcome."
        }
        Some("worker_lost_before_prompt") | Some("worker_lost_before_control") => {
            "The Worker exited before any Provider Prompt or Current Run control."
        }
        Some("worker_lost_after_current_run_control") => {
            "The Worker exited after Current Run control and before generation."
        }
        Some("provider_termination_uncertain") | Some("current_run_reconciliation_uncertain") => {
            "Graceful termination evidence is uncertain."
        }
        Some(_) => "A normalized Task failure was recorded.",
        None => "No failure recorded.",
    }
}

#[cfg(test)]
#[path = "tasks_test.rs"]
mod tests;
