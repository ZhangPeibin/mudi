//! 全局模态覆盖层、编辑器和终端尺寸保护。

use super::{
    detail, guide, layout, text, theme::Theme, SessionEditorField, Ui, MIN_TERMINAL_HEIGHT,
    MIN_TERMINAL_WIDTH,
};
use crate::app::Snapshot;
use crate::domain::{Provider, SessionState};
use crate::session_labels::SessionLabels;
use ratatui::layout::{Alignment, Position};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// Whether a modal interaction layer currently owns input over page content.
pub(super) fn blocks_background_interaction(ui: &Ui) -> bool {
    ui.detail.is_some()
        || ui.guide_open
        || ui.edit_session.is_some()
        || ui.handoff_confirmation.is_some()
        || ui.task_cancellation_confirmation.is_some()
        || ui.task_deletion_confirmation.is_some()
        || ui.task_retry_confirmation.is_some()
}

/// 按层级绘制当前打开的 Detail、Guide、编辑器和授权确认框。
pub(super) fn draw_overlays(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    // 后绘制的确认框覆盖 Detail，保证授权流程始终处于最高优先级。
    if ui.detail_session(snap).is_some() {
        detail::draw(f, snap, ui);
    }
    if ui.guide_open {
        guide::draw(f, ui);
    }
    if ui.edit_session.is_some() {
        draw_session_editor(f, ui);
    }
    if ui.handoff_confirmation.is_some() {
        draw_handoff_confirmation(f, snap, ui);
    }
    if ui.task_cancellation_confirmation.is_some() {
        draw_task_cancellation_confirmation(f, ui);
    }
    if ui.task_deletion_confirmation.is_some() {
        draw_task_deletion_confirmation(f, ui);
    }
    if ui.task_retry_confirmation.is_some() {
        draw_task_retry_confirmation(f, ui);
    }
}

/// 绘制 Retry 的重新授权确认框。
fn draw_task_retry_confirmation(f: &mut Frame, ui: &Ui) {
    let Some(target) = ui.task_retry_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect(68, 52, f.area());
    f.render_widget(Clear, area);
    let mut lines = vec![
        Line::from(Span::styled(
            "Authorize Retry?",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Previous Task  {}", target.predecessor_task_id)),
        Line::from(format!(
            "Provider       {}",
            target.session.provider.label()
        )),
        Line::from(format!("Session ID     {}", target.session.native_id)),
        Line::from(match target.successor_provider {
            Provider::ClaudeCode => "Successor  [Claude Code]  Codex CLI",
            Provider::CodexCli => "Successor  Claude Code  [Codex CLI]",
        }),
        Line::from(""),
        Line::from("A new Task, deadlines, Artifact, and authorization will be used."),
    ];
    if target.end_current_run {
        lines.push(Line::from(format!(
            "The {} Current Run may be ended after revalidation.",
            match target.source_state {
                SessionState::Busy => "Busy",
                SessionState::Idle => "Idle",
                SessionState::Exited => "Exited",
            }
        )));
    }
    lines.extend([
        Line::from("The previous Task remains unchanged."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter Authorize   Esc Cancel",
            Style::default().fg(theme.accent),
        )),
    ]);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(" Retry Handoff "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

/// 绘制取消非终态 Operation Task 的确认框。
fn draw_task_cancellation_confirmation(f: &mut Frame, ui: &Ui) {
    let Some(target) = ui.task_cancellation_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect(64, 38, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from(Span::styled(
            "Cancel Task?",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Task ID  {}", target.task_id)),
        Line::from(""),
        Line::from("A valid Handoff Result can still become Ready."),
        Line::from("No Prompt will be resent and no Retry will start."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter Confirm   Esc Keep Task",
            Style::default().fg(theme.accent),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(" Cancel Task "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

/// 绘制不可逆删除终态 Task 及其 Artifact 的确认框。
fn draw_task_deletion_confirmation(f: &mut Frame, ui: &Ui) {
    let Some(target) = ui.task_deletion_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect(70, 46, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from(Span::styled(
            "Delete Task and Artifact?",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Task ID  {}", target.task_id)),
        Line::from(""),
        Line::from("This irreversible action removes the Operation Task,"),
        Line::from("its Handoff Artifact, Successor Launch Plan, and interaction metadata."),
        Line::from("Source and Successor Sessions remain unchanged."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter Delete   Esc Keep Task",
            Style::default().fg(theme.accent),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(" Delete Task and Artifact "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

/// 绘制 Handoff 授权确认框，并展示来源状态与目标 Provider。
fn draw_handoff_confirmation(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let Some(target) = ui.handoff_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let session = snap.sessions.iter().find(|session| {
        session.provider == target.session.provider && session.id == target.session.native_id
    });
    let cwd = session
        .and_then(|session| session.cwd.as_deref())
        .unwrap_or("N/A");
    let area = layout::centered_rect(72, 64, f.area());
    f.render_widget(Clear, area);
    let mut lines = vec![
        Line::from(Span::styled(
            "Authorize one Handoff Operation?",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Provider    {}", target.session.provider.label())),
        Line::from(format!("Session ID  {}", target.session.native_id)),
        Line::from(format!("cwd         {cwd}")),
        Line::from(match target.successor_provider {
            Provider::ClaudeCode => "Successor  [Claude Code]  Codex CLI",
            Provider::CodexCli => "Successor  Claude Code  [Codex CLI]",
        }),
    ];
    if target.end_current_run {
        let current_run_state = match target.source_state {
            SessionState::Busy => "Busy",
            SessionState::Idle => "Idle",
            SessionState::Exited => "Exited",
        };
        lines.extend([
            Line::from(format!("Current Run {current_run_state}")),
            Line::from(""),
            Line::from("This authorization may end this one Current Run."),
            Line::from("A Busy Current Run waits until Idle and is never interrupted."),
            Line::from("SessionMesh revalidates it before requesting graceful exit."),
            Line::from("The users Terminal and shell remain open."),
        ]);
    }
    lines.extend([
        Line::from(""),
        Line::from("Generation keeps the source Project and Worktree read-only."),
        Line::from("The detached Task continues after SessionMesh closes."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter Authorize   Esc Cancel",
            Style::default().fg(theme.accent),
        )),
    ]);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(" Authorize Handoff "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

/// 在终端小于硬性支持尺寸时，只绘制明确的尺寸提示。
pub(super) fn draw_terminal_too_small(f: &mut Frame, theme: Theme) {
    let current = f.area();
    let area = layout::centered_rect(100, 40, current);
    let lines = vec![
        Line::from(Span::styled(
            "Terminal too small",
            Style::default()
                .fg(theme.critical)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "Required: {MIN_TERMINAL_WIDTH}x{MIN_TERMINAL_HEIGHT}"
        )),
        Line::from(format!("Current: {}x{}", current.width, current.height)),
        Line::from(""),
        Line::from("q Quit"),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.critical)),
            ),
        area,
    );
}

/// 绘制 Session 元数据编辑器，并把终端光标放到当前字段的可见输入末尾。
fn draw_session_editor(f: &mut Frame, ui: &Ui) {
    const FIELD_INDICATOR_WIDTH: usize = 2;
    const FIELD_LABEL_WIDTH: usize = 13;

    let Some(editor) = &ui.edit_session else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect(76, 42, f.area());
    // Reserve both borders and one cell for the terminal cursor at the input end.
    let input_width =
        usize::from(area.width).saturating_sub(3 + FIELD_INDICATOR_WIDTH + FIELD_LABEL_WIDTH);
    let title_input = text::visible_input_tail(&editor.title, input_width);
    let description_input = text::visible_input_tail(&editor.description, input_width);
    let short_id = SessionLabels::short_id(&editor.session.native_id);
    let field_line = |label: &str, input: String, field: SessionEditorField| {
        let focused = editor.field == field;
        let label_style = if focused {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        Line::from(vec![
            Span::styled(if focused { "> " } else { "  " }, label_style),
            Span::styled(format!("{label:<FIELD_LABEL_WIDTH$}"), label_style),
            Span::styled(input, Style::default().fg(theme.foreground)),
        ])
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Session  ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{} · {short_id}", editor.session.provider.label()),
                Style::default().fg(theme.foreground),
            ),
        ]),
        Line::from(""),
        field_line("Title", title_input.clone(), SessionEditorField::Title),
        field_line(
            "Description",
            description_input.clone(),
            SessionEditorField::Description,
        ),
    ];
    if let Some(error) = &editor.error {
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(theme.critical),
        )));
    }
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(Span::styled(
                        " Edit Session ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(
                        Line::from(Span::styled(
                            " Tab/Shift+Tab Fields | Enter Save | Esc Cancel ",
                            Style::default().fg(theme.muted),
                        ))
                        .right_aligned(),
                    ),
            ),
        area,
    );
    let (visible_input, row_offset) = match editor.field {
        SessionEditorField::Title => (title_input, 3),
        SessionEditorField::Description => (description_input, 4),
    };
    f.set_cursor_position(Position::new(
        area.x
            + 1
            + FIELD_INDICATOR_WIDTH as u16
            + FIELD_LABEL_WIDTH as u16
            + UnicodeWidthStr::width(visible_input.as_str()) as u16,
        area.y + row_offset,
    ));
}

#[cfg(test)]
#[path = "modals_test.rs"]
mod tests;
