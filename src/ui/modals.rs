//! 全局模态覆盖层、编辑器和终端尺寸保护。

use super::{
    detail, guide, layout, text, theme::Theme, NewSessionField, SessionEditorField, Ui,
    MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH,
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

const CODEX_FULL_ACCESS_NOTICE: &str = "Full access · No sandbox · No approvals";

/// Whether a modal interaction layer currently owns input over page content.
pub(super) fn blocks_background_interaction(ui: &Ui) -> bool {
    ui.hidden_tree_open
        || ui.detail.is_some()
        || ui.guide_open
        || ui.new_session.is_some()
        || ui.directory_browser.is_some()
        || ui.edit_session.is_some()
        || ui.managed_resume_confirmation.is_some()
        || ui.handoff_confirmation.is_some()
        || ui.task_cancellation_confirmation.is_some()
        || ui.task_deletion_confirmation.is_some()
        || ui.task_retry_confirmation.is_some()
        || ui.hide_confirmation.is_some()
        || ui.inventory_delete_confirmation.is_some()
}

/// 按层级绘制当前打开的 Detail、Guide、编辑器和授权确认框。
pub(super) fn draw_overlays(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    // 后绘制的确认框覆盖 Detail，保证授权流程始终处于最高优先级。
    if !ui.hidden_tree_open && ui.detail_session(snap).is_some() {
        detail::draw(f, snap, ui);
    }
    if ui.guide_open {
        guide::draw(f, ui);
    }
    if ui.new_session.is_some() {
        draw_new_session(f, ui);
    }
    if ui.directory_browser.is_some() {
        draw_directory_browser(f, ui);
    }
    if ui.edit_session.is_some() {
        draw_session_editor(f, ui);
    }
    if ui.managed_resume_confirmation.is_some() {
        draw_managed_resume_confirmation(f, ui);
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
    if ui.hide_confirmation.is_some() {
        draw_hide_confirmation(f, ui);
    }
    if ui.inventory_delete_confirmation.is_some() {
        draw_inventory_delete_confirmation(f, ui);
    }
}

fn draw_directory_browser(f: &mut Frame, ui: &Ui) {
    let Some(browser) = ui.directory_browser.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect_with_max_size(92, 82, 18, f.area());
    f.render_widget(
        Block::default().style(Style::default().add_modifier(Modifier::DIM)),
        f.area(),
    );
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        area,
    );
    f.render_widget_ref(browser.explorer.widget(), area);
}

fn draw_inventory_delete_confirmation(f: &mut Frame, ui: &Ui) {
    let Some(target) = ui.inventory_delete_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let count = target.sessions.len();
    let plural = if count == 1 { "" } else { "s" };
    let area = layout::centered_rect_with_max_width(72, 50, 72, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from(Span::styled(
            if count == 1 {
                "Permanently Exclude Session?"
            } else {
                "Permanently Exclude Project Sessions?"
            },
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "{count} Session{plural} will be removed from Mudi inventory."
        )),
        Line::from("Alias and Description will be deleted."),
        Line::from("Provider JSONL remains unchanged."),
        Line::from("Managed, Task, Handoff, and Successor records remain unchanged."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter Delete   Esc Keep Hidden",
            Style::default().fg(theme.accent),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.critical))
                    .title(" Permanent Mudi Exclusion "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

fn draw_hide_confirmation(f: &mut Frame, ui: &Ui) {
    let Some(target) = ui.hide_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect_with_max_width(64, 42, 64, f.area());
    f.render_widget(Clear, area);
    let (title, lines, footer) = match target {
        super::HideConfirmation::Session { session, title } => (
            " Hide Session ",
            vec![
                Line::from(Span::styled(
                    "Hide Session?",
                    Style::default()
                        .fg(theme.foreground)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(format!("Title       {title}")),
                Line::from(format!("Provider    {}", session.provider.label())),
                Line::from(format!("Session ID  {}", session.native_id)),
                Line::from(""),
                Line::from("Provider history remains unchanged."),
                Line::from("The Session can be restored from Hidden Tree."),
                Line::from(""),
            ],
            "Enter Hide   Esc Keep Visible".to_string(),
        ),
        super::HideConfirmation::Project {
            project_label,
            sessions,
            open_count,
        } => (
            " Hide Project Sessions ",
            vec![
                Line::from(Span::styled(
                    "Hide Project Sessions?",
                    Style::default()
                        .fg(theme.foreground)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(format!("Project  {project_label}")),
                Line::from(format!(
                    "{} Exited Session{} will be hidden",
                    sessions.len(),
                    if sessions.len() == 1 { "" } else { "s" }
                )),
                Line::from(format!(
                    "{open_count} Open Session{} will remain visible",
                    if *open_count == 1 { "" } else { "s" }
                )),
                Line::from("Future Sessions remain visible."),
                Line::from("Provider history remains unchanged."),
                Line::from(""),
            ],
            format!("Enter Hide {}   Esc Keep Visible", sessions.len()),
        ),
    };
    let mut lines = lines;
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(theme.accent),
    )));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(title),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

fn draw_managed_resume_confirmation(f: &mut Frame, ui: &Ui) {
    let Some(target) = ui.managed_resume_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect_with_max_width(82, 70, 72, f.area());
    f.render_widget(Clear, area);
    let mut lines = vec![
        Line::from(Span::styled(
            "Authorize Resume?",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Provider    {}", target.provider.label())),
        Line::from(format!("Session ID  {}", target.native_session_id)),
        Line::from(""),
    ];
    if target.provider == Provider::CodexCli {
        lines.push(Line::from(Span::styled(
            CODEX_FULL_ACCESS_NOTICE,
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }
    lines.extend([
        Line::from("Starts the Provider's exact native Resume path."),
        Line::from("Creates a new private tmux and distinct Current Run."),
        Line::from("Managed authority requires a new complete Binding Receipt."),
        Line::from("The prior Current Run is revalidated before launch."),
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
                    .title(" Authorize Resume "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

fn draw_new_session(f: &mut Frame, ui: &Ui) {
    const FIELD_INDICATOR_WIDTH: usize = 2;
    const FIELD_LABEL_WIDTH: usize = 14;

    let Some(form) = &ui.new_session else {
        return;
    };
    let theme = ui.theme.palette();
    let area = layout::centered_rect_with_max_size(88, 78, 11, f.area());
    let content_width = usize::from(area.width).saturating_sub(2);
    let value_width = content_width.saturating_sub(FIELD_INDICATOR_WIDTH + FIELD_LABEL_WIDTH);
    let target = form.targets.get(form.target_index);
    let editing = form.editor.editing();
    let field_line = |label: &str, value: &str, field: NewSessionField, keep_end: bool| {
        let focused = form.editor.focused() == field.index();
        let field_draft = editing.filter(|draft| draft.field() == field.index());
        let label_style = if focused {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        let empty = value.is_empty() && field_draft.is_none();
        let visible_value = if let Some(draft) = field_draft {
            draft.visible(value_width.saturating_sub(1)).0
        } else if empty {
            text::fit("Not set", value_width, false)
        } else {
            text::fit(value, value_width, keep_end)
        };
        let value_style = Style::default()
            .fg(if empty {
                theme.muted
            } else if focused {
                theme.selected_foreground
            } else {
                theme.foreground
            })
            .bg(if focused {
                theme.selected_background
            } else {
                theme.background
            });
        let padding =
            " ".repeat(value_width.saturating_sub(UnicodeWidthStr::width(visible_value.as_str())));
        Line::from(vec![
            Span::styled(if focused { "› " } else { "  " }, label_style),
            Span::styled(format!("{label:<FIELD_LABEL_WIDTH$}"), label_style),
            Span::styled(visible_value, value_style),
            Span::styled(padding, value_style),
        ])
    };
    let status_line = if let Some(error) = &form.error {
        Line::from(Span::styled(
            text::fit(&format!("! {error}"), content_width, false),
            Style::default().fg(theme.critical),
        ))
    } else {
        Line::from("")
    };
    let provider = form.provider.map(Provider::label).unwrap_or_default();
    let location = target
        .map(|target| target.working_directory.to_string_lossy())
        .unwrap_or_default();
    let lines = vec![
        field_line("Provider *", provider, NewSessionField::Provider, false),
        Line::from(""),
        field_line(
            "Location",
            location.as_ref(),
            NewSessionField::Location,
            true,
        ),
        Line::from(""),
        field_line("Title *", &form.title, NewSessionField::Title, false),
        Line::from(""),
        field_line(
            "Description",
            &form.description,
            NewSessionField::Description,
            false,
        ),
        Line::from(""),
        status_line,
    ];
    let footer = if editing.is_some() {
        " Enter Save · Esc Cancel "
    } else {
        " ↑/↓ Move · Enter Edit · y Start · Esc/q Close "
    };
    f.render_widget(
        Block::default().style(Style::default().add_modifier(Modifier::DIM)),
        f.area(),
    );
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border))
                    .title(Span::styled(
                        " New ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(
                        Line::from(Span::styled(footer, Style::default().fg(theme.muted)))
                            .centered(),
                    ),
            ),
        area,
    );
    if let Some(draft) = editing {
        let (_, cursor_column) = draft.visible(value_width.saturating_sub(1));
        f.set_cursor_position(Position::new(
            area.x
                + 1
                + FIELD_INDICATOR_WIDTH as u16
                + FIELD_LABEL_WIDTH as u16
                + cursor_column as u16,
            area.y + 1 + draft.field() as u16 * 2,
        ));
    }

    if let Some(choice) = form.editor.choice() {
        f.render_widget(
            Block::default().style(Style::default().add_modifier(Modifier::DIM)),
            f.area(),
        );
        match NewSessionField::from_index(choice.field) {
            NewSessionField::Provider => draw_provider_choice(f, form, choice.cursor, theme),
            NewSessionField::Location => draw_location_choice(f, form, choice.cursor, theme),
            _ => {}
        }
    }
}

fn draw_provider_choice(f: &mut Frame, form: &super::NewSessionForm, cursor: usize, theme: Theme) {
    let area = layout::centered_rect_with_max_size(70, 58, 7, f.area());
    let content_width = usize::from(area.width).saturating_sub(2);
    let choices = [
        ("Codex CLI", Provider::CodexCli, theme.provider_codex),
        ("Claude Code", Provider::ClaudeCode, theme.provider_claude),
    ];
    let mut lines = vec![Line::from("")];
    lines.extend(
        choices
            .iter()
            .enumerate()
            .map(|(index, (label, provider, color))| {
                property_choice_line(
                    label,
                    index == cursor,
                    form.provider == Some(*provider),
                    content_width,
                    *color,
                    theme,
                )
            }),
    );
    lines.push(
        if cursor == 0 || form.provider == Some(Provider::CodexCli) {
            Line::from(Span::styled(
                text::fit(CODEX_FULL_ACCESS_NOTICE, content_width, false),
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from("")
        },
    );
    lines.push(Line::from(""));
    draw_property_choice_dialog(f, area, " Select Provider ", lines, theme);
}

fn draw_location_choice(f: &mut Frame, form: &super::NewSessionForm, cursor: usize, theme: Theme) {
    const MAX_VISIBLE_OPTIONS: usize = 8;

    let option_count = form.targets.len() + 1;
    let visible_count = option_count.clamp(1, MAX_VISIBLE_OPTIONS);
    let area = layout::centered_rect_with_max_size(
        88,
        78,
        u16::try_from(visible_count + 4).unwrap_or(12),
        f.area(),
    );
    let content_width = usize::from(area.width).saturating_sub(2);
    let start = cursor
        .saturating_sub(visible_count - 1)
        .min(option_count.saturating_sub(visible_count));
    let mut lines = vec![Line::from("")];
    lines.extend((start..start + visible_count).map(|index| {
        if let Some(target) = form.targets.get(index) {
            property_choice_line(
                &target.working_directory.to_string_lossy(),
                index == cursor,
                index == form.target_index,
                content_width,
                theme.foreground,
                theme,
            )
        } else {
            property_choice_line(
                "Choose Directory...",
                index == cursor,
                false,
                content_width,
                theme.accent,
                theme,
            )
        }
    }));
    lines.push(Line::from(""));
    draw_property_choice_dialog(f, area, " Select Location ", lines, theme);
}

fn property_choice_line(
    label: &str,
    focused: bool,
    selected: bool,
    width: usize,
    accent: ratatui::style::Color,
    theme: Theme,
) -> Line<'static> {
    let prefix = format!(
        "{}{} ",
        if focused { "› " } else { "  " },
        if selected { "•" } else { "◦" }
    );
    let label = text::fit(
        label,
        width.saturating_sub(UnicodeWidthStr::width(prefix.as_str())),
        true,
    );
    let style = Style::default()
        .fg(if selected { accent } else { theme.foreground })
        .bg(if focused {
            theme.selected_background
        } else {
            theme.background
        })
        .add_modifier(if focused || selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    Line::from(Span::styled(format!("{prefix}{label}"), style))
}

fn draw_property_choice_dialog(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    title: &'static str,
    lines: Vec<Line<'static>>,
    theme: Theme,
) {
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(Span::styled(
                        title,
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(
                        Line::from(Span::styled(
                            " ↑/↓ Move · Enter Select · Esc Back ",
                            Style::default().fg(theme.muted),
                        ))
                        .centered(),
                    ),
            ),
        area,
    );
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
            Line::from("Mudi revalidates it before requesting graceful exit."),
            Line::from("The users Terminal and shell remain open."),
        ]);
    }
    lines.extend([
        Line::from(""),
        Line::from("Generation keeps the source Project and Worktree read-only."),
        Line::from("The detached Task continues after Mudi closes."),
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
