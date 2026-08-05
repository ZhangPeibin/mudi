//! 全局模态覆盖层、编辑器和终端尺寸保护。

use super::{
    guide, layout, selected_style, text, theme::Theme, CopyField, Ui, MIN_TERMINAL_HEIGHT,
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

/// 按层级绘制当前打开的菜单、Guide、编辑器和授权确认框。
pub(super) fn draw_overlays(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    // 后绘制的确认框覆盖前面的菜单，保证最高优先级操作始终可见。
    if ui
        .detail
        .as_ref()
        .is_some_and(|detail| detail.copy_menu_selected.is_some())
    {
        draw_copy_menu(f, snap, ui);
    }
    if ui.guide_open {
        guide::draw(f, ui);
    }
    if ui.edit_title.is_some() {
        draw_title_editor(f, ui);
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

/// 绘制详情页的本地元数据复制菜单。
fn draw_copy_menu(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let Some(session) = ui.detail_session(snap) else {
        return;
    };
    let Some(selected) = ui
        .detail
        .as_ref()
        .and_then(|detail| detail.copy_menu_selected)
    else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect(84, 38, f.area());
    let mut lines = vec![Line::from(Span::styled(
        "Choose one complete metadata value",
        Style::default().fg(theme.muted),
    ))];
    for (index, field) in CopyField::ALL.iter().copied().enumerate() {
        let value = field.value(session);
        let text = format!(
            "{} {:<12} {}",
            if index == selected { ">" } else { " " },
            field.label(),
            value.as_deref().unwrap_or("Unavailable")
        );
        let style = if value.is_none() {
            Style::default().fg(theme.muted)
        } else if index == selected {
            selected_style(theme)
        } else {
            Style::default().fg(theme.foreground)
        };
        lines.push(Line::from(Span::styled(text, style)));
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
                        " Copy source metadata ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(
                        Line::from(Span::styled(
                            " Enter Copy | Esc Cancel ",
                            Style::default().fg(theme.muted),
                        ))
                        .right_aligned(),
                    ),
            ),
        area,
    );
}

/// 绘制 Session Alias 编辑器，并把终端光标放到可见输入末尾。
fn draw_title_editor(f: &mut Frame, ui: &Ui) {
    const TITLE_LABEL: &str = "Title    ";

    let Some(editor) = &ui.edit_title else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect(76, 32, f.area());
    // 输入过长时只显示尾部，同时按 Unicode 显示宽度计算真实光标列。
    let label_width = UnicodeWidthStr::width(TITLE_LABEL);
    let input_width = area.width.saturating_sub(3 + label_width as u16) as usize;
    let visible_input = text::visible_input_tail(&editor.input, input_width);
    let visible_input_width = UnicodeWidthStr::width(visible_input.as_str()) as u16;
    let short_id = SessionLabels::short_id(&editor.session.native_id);
    let clears_existing_alias = editor.input.trim().is_empty()
        && ui
            .labels
            .alias(editor.session.provider, &editor.session.native_id)
            .is_some();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Session  ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{} · {short_id}", editor.session.provider.label()),
                Style::default().fg(theme.foreground),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(TITLE_LABEL, Style::default().fg(theme.muted)),
            Span::styled(visible_input, Style::default().fg(theme.foreground)),
        ]),
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
                        " Edit title ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(
                        Line::from(Span::styled(
                            if clears_existing_alias {
                                " Enter Clear title | Esc Cancel "
                            } else {
                                " Enter Save | Esc Cancel "
                            },
                            Style::default().fg(theme.muted),
                        ))
                        .right_aligned(),
                    ),
            ),
        area,
    );
    f.set_cursor_position(Position::new(
        area.x + 1 + label_width as u16 + visible_input_width,
        area.y + 3,
    ));
}

#[cfg(test)]
#[path = "modals_test.rs"]
mod tests;
