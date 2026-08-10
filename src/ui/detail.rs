//! Selectable Session Detail modal and its evidence-oriented content model.

use super::{
    context::{Presentation, Progress},
    current_attention,
    layout::centered_rect_with_max_width,
    selected_style, snapshot_now_ms,
    theme::Theme,
    Ui,
};
use crate::app::{fmt_cost, fmt_datetime, fmt_relative, fmt_tokens, Snapshot};
use crate::attention::{AttentionCondition, AttentionSeverity};
use crate::domain::{ModelUsage, ProjectIdentity, Session, SessionState, Usage};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

const DETAIL_MODAL_MAX_WIDTH: u16 = 100;
const DETAIL_LABEL_WIDTH: usize = 16;

enum DetailLine {
    Field {
        label: &'static str,
        value: String,
        selectable: bool,
        muted: bool,
    },
    Content(Line<'static>),
}

impl DetailLine {
    fn field(label: &'static str, value: impl Into<String>, selectable: bool) -> Self {
        Self::Field {
            label,
            value: value.into(),
            selectable,
            muted: false,
        }
    }

    fn unavailable(label: &'static str, value: impl Into<String>) -> Self {
        Self::Field {
            label,
            value: value.into(),
            selectable: false,
            muted: true,
        }
    }

    fn is_selectable(&self) -> bool {
        matches!(
            self,
            Self::Field {
                selectable: true,
                ..
            }
        )
    }

    fn copy_value(&self) -> Option<(&'static str, String)> {
        match self {
            Self::Field {
                label,
                value,
                selectable: true,
                ..
            } => Some((label, value.clone())),
            _ => None,
        }
    }

    fn render(&self, selected: bool, theme: Theme) -> Line<'static> {
        match self {
            Self::Field {
                label,
                value,
                muted,
                ..
            } => {
                let label_style = Style::default().fg(theme.muted);
                let value_style = if selected {
                    selected_style(theme)
                } else if *muted {
                    Style::default().fg(theme.muted)
                } else {
                    Style::default().fg(theme.foreground)
                };
                Line::from(vec![
                    Span::styled(format!("{label:<DETAIL_LABEL_WIDTH$}"), label_style),
                    Span::styled(value.clone(), value_style),
                ])
            }
            Self::Content(line) => line.clone(),
        }
    }
}

fn heading(label: impl Into<String>, theme: Theme) -> DetailLine {
    DetailLine::Content(Line::from(Span::styled(
        label.into(),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )))
}

fn content(value: impl Into<String>, style: Style) -> DetailLine {
    DetailLine::Content(Line::from(Span::styled(value.into(), style)))
}

fn add_optional_field(
    lines: &mut Vec<DetailLine>,
    label: &'static str,
    value: Option<String>,
    unavailable: impl Into<String>,
) {
    match value {
        Some(value) if !value.is_empty() => lines.push(DetailLine::field(label, value, true)),
        _ => lines.push(DetailLine::unavailable(label, unavailable)),
    }
}

fn add_usage_summary(
    lines: &mut Vec<DetailLine>,
    scope: &str,
    usage: &Usage,
    per_model: &[ModelUsage],
    cost: Option<f64>,
    theme: Theme,
) {
    lines.push(heading(format!("{scope} Usage"), theme));
    lines.push(DetailLine::field("Input", fmt_tokens(usage.input), true));
    lines.push(DetailLine::field("Output", fmt_tokens(usage.output), true));
    lines.push(DetailLine::field("Est. API cost", fmt_cost(cost), true));
    lines.push(DetailLine::Content(Line::from("")));
    lines.push(heading(format!("{scope} per-model Usage"), theme));
    for model in per_model {
        lines.push(content(
            format!(
                "  {:<20} in {} out {}  {}",
                model.model,
                fmt_tokens(model.usage.input),
                fmt_tokens(model.usage.output),
                fmt_cost(model.cost),
            ),
            Style::default().fg(theme.foreground),
        ));
    }
    lines.push(DetailLine::Content(Line::from("")));
}

fn attention_lines(snap: &Snapshot, session: &Session, theme: Theme) -> Vec<DetailLine> {
    let mut lines = vec![heading("Attention Conditions", theme)];
    let attention = current_attention(snap);
    let conditions = attention.conditions_for(session);
    if session.state == SessionState::Exited {
        lines.push(content(
            "  None - Exited Session has no current Attention Conditions",
            Style::default().fg(theme.muted),
        ));
        return lines;
    }
    if conditions.is_empty() {
        lines.push(content(
            "  None - healthy Busy Session",
            Style::default().fg(theme.busy),
        ));
        return lines;
    }

    for condition in conditions {
        let (text, style) = match condition {
            AttentionCondition::ContextAlert {
                severity,
                percentage,
                threshold_percent,
                ..
            } => {
                let label = match severity {
                    AttentionSeverity::Critical => "CRITICAL",
                    AttentionSeverity::Warning => "WARNING",
                };
                let color = match severity {
                    AttentionSeverity::Critical => theme.critical,
                    AttentionSeverity::Warning => theme.warning,
                };
                (
                    format!("  {label} Current Context {percentage:.1}% >= {threshold_percent}%"),
                    Style::default().fg(color),
                )
            }
            AttentionCondition::SharedWorktreeRisk {
                contributing_sessions,
                ..
            } => (
                format!("  WARNING Shared Worktree Risk x{contributing_sessions}"),
                Style::default().fg(theme.warning),
            ),
            AttentionCondition::Idle { since_ms, .. } => (
                format!(
                    "  IDLE awaiting input for {}",
                    fmt_relative(*since_ms, snapshot_now_ms(snap))
                ),
                Style::default().fg(theme.idle),
            ),
        };
        lines.push(content(text, style));
    }
    lines
}

fn detail_lines(
    snap: &Snapshot,
    ui: &Ui,
    session: &Session,
    content_width: usize,
) -> Vec<DetailLine> {
    let theme = ui.theme.palette();
    let unavailable_git_field = match session.project.as_ref().map(|project| &project.identity) {
        Some(ProjectIdentity::CanonicalCwd(_)) => "N/A (non-Git Project)",
        _ => "N/A (identity unavailable)",
    };
    let state = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => "Exited",
    };
    let mut lines = Vec::new();

    let provider_model = session
        .latest_model
        .as_deref()
        .map(|model| format!("{} -> {model}", session.provider.label()))
        .unwrap_or_else(|| session.provider.label().to_string());
    lines.push(DetailLine::field("Provider", provider_model, true));
    add_optional_field(
        &mut lines,
        "Project",
        session
            .project
            .as_ref()
            .map(|project| project.label.clone()),
        "N/A (identity unavailable)",
    );
    add_optional_field(
        &mut lines,
        "Worktree",
        session
            .worktree
            .as_ref()
            .map(|worktree| worktree.root.to_string_lossy().into_owned()),
        unavailable_git_field,
    );
    add_optional_field(
        &mut lines,
        "Branch",
        session
            .worktree
            .as_ref()
            .and_then(|worktree| worktree.branch.clone()),
        match session.worktree {
            Some(_) => "N/A (detached HEAD)",
            None => unavailable_git_field,
        },
    );
    add_optional_field(&mut lines, "cwd", session.cwd.clone(), "N/A");
    add_optional_field(
        &mut lines,
        "Source log",
        (!session.source.path.as_os_str().is_empty())
            .then(|| session.source.path.to_string_lossy().into_owned()),
        "N/A",
    );
    lines.push(DetailLine::field(
        "Source format",
        session.source.format.label(),
        true,
    ));
    lines.push(DetailLine::field(
        "Title",
        ui.labels.title(session.provider, &session.id),
        true,
    ));
    lines.push(DetailLine::field("Session ID", &session.id, true));
    lines.push(DetailLine::field("State", state, true));
    if snap
        .verified_managed_current_runs
        .iter()
        .any(|run| run.is_verified_open_for(session))
    {
        lines.push(DetailLine::Content(Line::from("")));
        lines.push(heading("Managed Current Run", theme));
        lines.push(content(
            "  o Open Terminal | a Copy Attach Command",
            Style::default().fg(theme.foreground),
        ));
    }
    lines.push(DetailLine::Content(Line::from("")));

    lines.push(heading("Current Context", theme));
    lines.push(DetailLine::Content(
        Progress::new(session.context, session.state).line(
            Presentation::Expanded,
            "Occupancy",
            DETAIL_LABEL_WIDTH,
            content_width,
            theme,
        ),
    ));
    lines.push(DetailLine::Content(Line::from("")));
    lines.extend(attention_lines(snap, session, theme));
    lines.push(DetailLine::Content(Line::from("")));

    if session.state.is_open() {
        match &session.current_run_usage {
            Some(run) => add_usage_summary(
                &mut lines,
                "Current Run",
                &run.usage,
                &run.per_model,
                run.cost,
                theme,
            ),
            None => {
                lines.push(heading("Current Run Usage", theme));
                lines.push(DetailLine::unavailable("Usage", "N/A"));
                lines.push(DetailLine::Content(Line::from("")));
            }
        }
    }
    add_usage_summary(
        &mut lines,
        "Transcript Lifetime",
        &session.usage,
        &session.per_model,
        session.cost,
        theme,
    );

    lines.push(heading(
        format!("Compactions ({})", session.compaction_count()),
        theme,
    ));
    if session.compactions.is_empty() {
        lines.push(content("  none observed", Style::default().fg(theme.muted)));
    } else {
        for compaction in &session.compactions {
            let trigger = compaction.trigger.as_deref().unwrap_or("N/A");
            let pre_tokens = compaction
                .pre_tokens
                .map(fmt_tokens)
                .unwrap_or_else(|| "N/A".into());
            lines.push(content(
                format!(
                    "  {}  trigger={trigger}  pre={pre_tokens}",
                    fmt_datetime(compaction.at_ms)
                ),
                Style::default().fg(theme.foreground),
            ));
        }
    }
    lines.push(DetailLine::field(
        "Compaction count",
        session.compaction_count().to_string(),
        true,
    ));
    lines
}

fn selectable_lines(snap: &Snapshot, ui: &Ui, session: &Session) -> Vec<DetailLine> {
    detail_lines(snap, ui, session, DETAIL_MODAL_MAX_WIDTH as usize)
        .into_iter()
        .filter(DetailLine::is_selectable)
        .collect()
}

/// Return the selected complete value for the direct Detail copy command.
pub(super) fn selected_copy_request(snap: &Snapshot, ui: &Ui) -> Option<(&'static str, String)> {
    let session = ui.detail_session(snap)?;
    let selected = ui.detail.as_ref()?.selected_field;
    selectable_lines(snap, ui, session)
        .get(selected)
        .and_then(DetailLine::copy_value)
}

/// Move the Detail selection without changing the origin page's Session selection.
pub(super) fn move_selected_field(ui: &mut Ui, snap: &Snapshot, forward: bool) {
    let Some(session) = ui.detail_session(snap) else {
        return;
    };
    let count = selectable_lines(snap, ui, session).len();
    let Some(detail) = ui.detail.as_mut() else {
        return;
    };
    if count == 0 {
        detail.selected_field = 0;
    } else if forward {
        detail.selected_field = detail.selected_field.saturating_add(1).min(count - 1);
    } else {
        detail.selected_field = detail.selected_field.saturating_sub(1);
    }
}

/// Draw the centered Session Detail modal over its dimmed origin page.
pub(super) fn draw(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let Some(session) = ui.detail_session(snap) else {
        return;
    };
    let Some(detail) = ui.detail.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = centered_rect_with_max_width(80, 80, DETAIL_MODAL_MAX_WIDTH, f.area());
    let inner_height = area.height.saturating_sub(2) as usize;
    let content_width = usize::from(area.width.saturating_sub(2)).max(1);
    let entries = detail_lines(snap, ui, session, content_width);
    let wrap = Wrap { trim: false };
    let mut field_index = 0;
    let mut total_lines = 0;
    let mut selected_range = None;
    let lines = entries
        .iter()
        .map(|entry| {
            let selected = entry.is_selectable() && field_index == detail.selected_field;
            if entry.is_selectable() {
                field_index += 1;
            }
            let rendered = entry.render(selected, theme);
            let rendered_count = Paragraph::new(rendered.clone())
                .wrap(wrap)
                .line_count(area.width.saturating_sub(2));
            if selected {
                selected_range = Some((total_lines, total_lines + rendered_count));
            }
            total_lines += rendered_count;
            rendered
        })
        .collect::<Vec<_>>();
    let max_scroll = total_lines.saturating_sub(inner_height);
    let mut scroll = detail.scroll.get().min(max_scroll);
    if let Some((start, end)) = selected_range {
        if start < scroll {
            scroll = start;
        } else if end > scroll + inner_height {
            scroll = end.saturating_sub(inner_height);
        }
    }
    scroll = scroll.min(max_scroll);
    detail.scroll.set(scroll);

    // Preserve the origin page while reducing its contrast before clearing the modal surface.
    f.render_widget(
        Block::default().style(Style::default().add_modifier(Modifier::DIM)),
        f.area(),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            " Session Detail ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                " Up/Down Select | c Copy | h Handoff | Esc Close ",
                Style::default().fg(theme.muted),
            ))
            .left_aligned(),
        )
        .title_bottom(
            Line::from(Span::styled(
                format!(
                    " fields {}/{} ",
                    detail.selected_field.saturating_add(1).min(field_index),
                    field_index
                ),
                Style::default().fg(theme.muted),
            ))
            .right_aligned(),
        );
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(block)
            .scroll((scroll.min(u16::MAX as usize) as u16, 0))
            .wrap(wrap),
        area,
    );
}

#[cfg(test)]
#[path = "detail_test.rs"]
mod tests;
