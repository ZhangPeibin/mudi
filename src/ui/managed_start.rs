//! Metadata-only status and recovery controls for incomplete Managed launches.

use super::Ui;
use crate::app::Snapshot;
use crate::managed_sessions::{ManagedLaunchAction, ManagedLaunchStatus};
use crate::read_model::{ManagedLaunchSummary, ManagedTerminalState};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let Some(launch) = ui.selected_managed_launch(snap) else {
        return;
    };
    let theme = ui.theme.palette();
    let selected_index = snap
        .managed_launches
        .iter()
        .position(|candidate| candidate.id == launch.id)
        .unwrap_or(0);
    let status_color = match launch.status {
        ManagedLaunchStatus::Binding => theme.warning,
        ManagedLaunchStatus::NeedsReview => theme.accent,
        ManagedLaunchStatus::Failed => theme.critical,
    };
    let title = if snap.managed_launches.len() > 1 {
        " MANAGED START [/] "
    } else {
        " MANAGED START "
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(status_color))
        .title(Span::styled(
            title,
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ));
    let lines = vec![
        Line::from(vec![
            Span::styled(
                launch_status(launch.status),
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {}  {}/{}",
                    launch.id,
                    selected_index + 1,
                    snap.managed_launches.len()
                ),
                Style::default().fg(theme.muted),
            ),
        ]),
        Line::from(Span::styled(
            launch_context(launch),
            Style::default().fg(theme.foreground),
        )),
        Line::from(Span::styled(
            launch.title.as_str(),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            terminal_status(launch),
            Style::default().fg(match launch.terminal_state {
                ManagedTerminalState::Error => theme.critical,
                ManagedTerminalState::Pending => theme.muted,
                ManagedTerminalState::Opened => theme.context_normal,
            }),
        )),
    ];
    let lines = lines
        .into_iter()
        .chain(
            launch
                .binding_error
                .as_deref()
                .map(|error| Line::from(Span::styled(error, Style::default().fg(theme.warning)))),
        )
        .chain(action_lines(launch, theme.muted))
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn launch_status(status: ManagedLaunchStatus) -> &'static str {
    match status {
        ManagedLaunchStatus::Binding => "BINDING",
        ManagedLaunchStatus::NeedsReview => "NEEDS REVIEW",
        ManagedLaunchStatus::Failed => "FAILED",
    }
}

fn launch_context(launch: &ManagedLaunchSummary) -> String {
    let worktree = launch
        .worktree_root
        .as_deref()
        .and_then(|path| path.file_name())
        .map_or_else(
            || "default".into(),
            |name| name.to_string_lossy().into_owned(),
        );
    format!(
        "{} | {} / {}",
        launch.provider.label(),
        launch.project_label,
        worktree
    )
}

fn terminal_status(launch: &ManagedLaunchSummary) -> String {
    match launch.terminal_state {
        ManagedTerminalState::Pending => "Terminal pending".into(),
        ManagedTerminalState::Opened => "Terminal open".into(),
        ManagedTerminalState::Error => format!(
            "Terminal error: {}",
            launch.terminal_error.as_deref().unwrap_or("unknown")
        ),
    }
}

fn action_lines(launch: &ManagedLaunchSummary, color: ratatui::style::Color) -> Vec<Line<'static>> {
    let actions = launch
        .available_actions
        .iter()
        .filter_map(|action| match action {
            ManagedLaunchAction::CancelStart => Some("x Cancel Start"),
            ManagedLaunchAction::OpenTerminal => Some("o Open Terminal"),
            ManagedLaunchAction::RetryOpenTerminal => Some("o Retry Terminal"),
            ManagedLaunchAction::CopyDiagnostics => Some("c Copy Diagnostics"),
            ManagedLaunchAction::Resume
            | ManagedLaunchAction::RetryBinding
            | ManagedLaunchAction::AdoptAsManaged => None,
        })
        .map(|label| Line::from(Span::styled(label, Style::default().fg(color))))
        .collect::<Vec<_>>();
    if actions.is_empty() {
        vec![Line::from(Span::styled(
            "No recovery action",
            Style::default().fg(color),
        ))]
    } else {
        actions
    }
}
