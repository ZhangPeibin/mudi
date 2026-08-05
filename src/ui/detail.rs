//! Session 详情页及其响应式字段布局。

use super::{current_attention, snapshot_now_ms, theme::Theme, CopyField, Ui};
use crate::app::{fmt_cost, fmt_datetime, fmt_relative, fmt_tokens, Snapshot};
use crate::attention::{AttentionCondition, AttentionSeverity};
use crate::domain::{ModelUsage, ProjectIdentity, Session, SessionState, Usage};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::borrow::Cow;

fn push_usage_summary(
    lines: &mut Vec<Line<'static>>,
    scope: &str,
    usage: &Usage,
    per_model: &[ModelUsage],
    cost: Option<f64>,
    theme: Theme,
) {
    let hdr = |key: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("{key:<16}"), Style::default().fg(theme.muted)),
            Span::styled(value, Style::default().fg(theme.foreground)),
        ])
    };
    lines.push(Line::from(Span::styled(
        format!("{scope} Usage"),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(hdr("Input", fmt_tokens(usage.input)));
    lines.push(hdr("Output", fmt_tokens(usage.output)));
    lines.push(hdr("Cache read", fmt_tokens(usage.cache_read)));
    lines.push(hdr("Cache write", fmt_tokens(usage.cache_write)));
    lines.push(hdr("Est. API cost", fmt_cost(cost)));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{scope} per-model Usage"),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    for model in per_model {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<20}", model.model),
                Style::default().fg(theme.foreground),
            ),
            Span::styled(
                format!(
                    "in {} out {} cr {} cw {}  ",
                    fmt_tokens(model.usage.input),
                    fmt_tokens(model.usage.output),
                    fmt_tokens(model.usage.cache_read),
                    fmt_tokens(model.usage.cache_write)
                ),
                Style::default().fg(theme.muted),
            ),
            Span::styled(fmt_cost(model.cost), Style::default().fg(theme.foreground)),
        ]));
    }
    lines.push(Line::from(""));
}

/// 绘制 Session 详情；Task 详情由任务 ID 分派到专用函数。
pub(super) fn draw(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let Some(s) = ui.detail_session(snap) else {
        return;
    };
    let theme = ui.theme.palette();

    if area.width <= 120 {
        draw_compact_detail(f, area, snap, ui, s, theme);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let hdr = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<16}"), Style::default().fg(theme.muted)),
            Span::styled(v, Style::default().fg(theme.foreground)),
        ])
    };

    let unavailable_git_field = match s.project.as_ref().map(|project| &project.identity) {
        Some(ProjectIdentity::CanonicalCwd(_)) => "N/A (non-Git Project)",
        _ => "N/A (identity unavailable)",
    };

    lines.push(hdr("Provider", s.provider.label().to_string()));
    lines.push(hdr(
        "Project",
        s.project
            .as_ref()
            .map(|project| project.label.clone())
            .unwrap_or_else(|| "N/A (identity unavailable)".into()),
    ));
    lines.push(hdr(
        "Worktree",
        s.worktree
            .as_ref()
            .map(|worktree| worktree.root.to_string_lossy().into_owned())
            .unwrap_or_else(|| unavailable_git_field.into()),
    ));
    lines.push(hdr(
        "Branch",
        match &s.worktree {
            Some(worktree) => worktree
                .branch
                .clone()
                .unwrap_or_else(|| "N/A (detached HEAD)".into()),
            None => unavailable_git_field.into(),
        },
    ));
    lines.push(hdr("cwd", s.cwd.clone().unwrap_or_else(|| "N/A".into())));
    lines.push(hdr(
        "Source log",
        CopyField::SourceLog
            .value(s)
            .map(Cow::into_owned)
            .unwrap_or_else(|| "N/A".into()),
    ));
    lines.push(hdr("Source format", s.source.format.label().into()));
    lines.push(hdr("Title", ui.labels.title(s.provider, &s.id)));
    lines.push(hdr("Session ID", s.id.clone()));
    lines.push(hdr(
        "Latest model",
        s.latest_model.clone().unwrap_or_else(|| "—".into()),
    ));
    lines.push(hdr(
        "State",
        match s.state {
            SessionState::Busy => "Busy".into(),
            SessionState::Idle => "Idle".into(),
            SessionState::Exited => "exited (no marker)".into(),
        },
    ));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "Current Context",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    let ctx = match s.context.percent() {
        Some(pct) => format!(
            "{} / {} ({:.1}%)",
            fmt_tokens(s.context.tokens),
            s.context
                .window
                .map(fmt_tokens)
                .unwrap_or_else(|| "?".into()),
            pct
        ),
        None => format!("{} / N/A (N/A)", fmt_tokens(s.context.tokens)),
    };
    lines.push(hdr("Occupancy", ctx));
    lines.push(Line::from(""));

    if s.state.is_open() {
        match &s.current_run_usage {
            Some(run) => push_usage_summary(
                &mut lines,
                "Current Run",
                &run.usage,
                &run.per_model,
                run.cost,
                theme,
            ),
            None => {
                lines.push(Line::from(Span::styled(
                    "Current Run Usage",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(hdr("Usage", "N/A".into()));
                lines.push(Line::from(""));
            }
        }
    }

    push_usage_summary(
        &mut lines,
        "Transcript Lifetime",
        &s.usage,
        &s.per_model,
        s.cost,
        theme,
    );

    lines.push(Line::from(Span::styled(
        format!("Compactions ({})", s.compaction_count()),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    if s.compactions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none observed",
            Style::default().fg(theme.muted),
        )));
    } else {
        for c in &s.compactions {
            let trigger = c.trigger.clone().unwrap_or_else(|| "—".into());
            let pre = c.pre_tokens.map(fmt_tokens).unwrap_or_else(|| "?".into());
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}  trigger={}  pre={}",
                    fmt_datetime(c.at_ms),
                    trigger,
                    pre
                ),
                Style::default().fg(theme.foreground),
            )));
        }
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            " Session detail — Esc to go back ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                format!(
                    " lines {}-{} of {} ",
                    scroll + usize::from(total_lines > 0),
                    (scroll + inner_height).min(total_lines),
                    total_lines
                ),
                Style::default().fg(theme.muted),
            ))
            .right_aligned(),
        );
    let p = Paragraph::new(lines)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .block(block)
        .scroll((scroll as u16, 0))
        .wrap(wrap);
    f.render_widget(p, area);
}

/// 绘制窄终端 Session 详情，并按实际换行行数提供滚动范围。
fn draw_compact_detail(
    f: &mut Frame,
    area: Rect,
    snap: &Snapshot,
    ui: &Ui,
    session: &Session,
    theme: Theme,
) {
    let field = |key: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("{key:<14}"), Style::default().fg(theme.muted)),
            Span::styled(value, Style::default().fg(theme.foreground)),
        ])
    };
    let section = |label: String| {
        Line::from(Span::styled(
            label,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let state = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => "Exited",
    };
    let context = match session.context.percent() {
        Some(percent) => format!(
            "{} / {} ({percent:.1}%)",
            fmt_tokens(session.context.tokens),
            session
                .context
                .window
                .map(fmt_tokens)
                .unwrap_or_else(|| "N/A".into())
        ),
        None => format!("{} / N/A (N/A)", fmt_tokens(session.context.tokens)),
    };
    let usage_line = |usage: &Usage, cost: Option<f64>| {
        format!(
            "in {} | out {} | cache read {} | cache create {} | API est. {}",
            fmt_tokens(usage.input),
            fmt_tokens(usage.output),
            fmt_tokens(usage.cache_read),
            fmt_tokens(usage.cache_write),
            fmt_cost(cost),
        )
    };

    let mut lines = vec![
        field(
            "Provider",
            format!("{} / {state}", session.provider.label()),
        ),
        field(
            "Project",
            session
                .project
                .as_ref()
                .map(|project| project.label.clone())
                .unwrap_or_else(|| "N/A".into()),
        ),
        field("Title", ui.labels.title(session.provider, &session.id)),
        field("Session ID", session.id.clone()),
        field(
            "Model",
            session.latest_model.clone().unwrap_or_else(|| "N/A".into()),
        ),
        field("Context", context),
        section("Attention Conditions".into()),
    ];
    let attention = current_attention(snap);
    let conditions = attention.conditions_for(session);
    if session.state == SessionState::Exited {
        lines.push(Line::from(Span::styled(
            "  None - Exited Session has no current Attention Conditions",
            Style::default().fg(theme.muted),
        )));
    } else if conditions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  None - healthy Busy Session",
            Style::default().fg(theme.busy),
        )));
    } else {
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
                        format!(
                            "  {label} Current Context {percentage:.1}% >= {threshold_percent}%"
                        ),
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
            lines.push(Line::from(Span::styled(text, style)));
        }
    }

    if session.state.is_open() {
        lines.push(section("Current Run Usage".into()));
        match &session.current_run_usage {
            Some(run) => lines.push(Line::from(usage_line(&run.usage, run.cost))),
            None => lines.push(Line::from("N/A")),
        }
    }
    lines.push(section("Transcript Lifetime Usage".into()));
    lines.push(Line::from(usage_line(&session.usage, session.cost)));
    let source_width = area.width.saturating_sub(2) as usize;
    push_wrapped_detail_field(
        &mut lines,
        "Worktree",
        &session.worktree.as_ref().map_or_else(
            || "N/A".into(),
            |worktree| {
                format!(
                    "{} @ {}",
                    worktree.root.display(),
                    worktree.branch.as_deref().unwrap_or("detached HEAD")
                )
            },
        ),
        source_width,
        theme,
    );
    push_wrapped_detail_field(
        &mut lines,
        "cwd",
        session.cwd.as_deref().unwrap_or("N/A"),
        source_width,
        theme,
    );
    let source_log = CopyField::SourceLog
        .value(session)
        .map(Cow::into_owned)
        .unwrap_or_else(|| "N/A".into());
    push_wrapped_detail_field(&mut lines, "Source log", &source_log, source_width, theme);
    push_wrapped_detail_field(
        &mut lines,
        "Source format",
        session.source.format.label(),
        source_width,
        theme,
    );
    let compaction_text = |compaction: &crate::domain::Compaction| {
        format!(
            "  {} ago  trigger={}  pre={}",
            fmt_relative(compaction.at_ms, snapshot_now_ms(snap)),
            compaction.trigger.as_deref().unwrap_or("N/A"),
            compaction
                .pre_tokens
                .map(fmt_tokens)
                .unwrap_or_else(|| "N/A".into())
        )
    };
    let mut compaction_heading = vec![Span::styled(
        format!("Compactions ({})", session.compaction_count()),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )];
    if session.compactions.is_empty() {
        compaction_heading.push(Span::styled(
            "  none observed".to_string(),
            Style::default().fg(theme.muted),
        ));
        lines.push(Line::from(compaction_heading));
    } else {
        compaction_heading.push(Span::styled(
            compaction_text(&session.compactions[0]),
            Style::default().fg(theme.foreground),
        ));
        lines.push(Line::from(compaction_heading));
        for compaction in session.compactions.iter().skip(1) {
            lines.push(Line::from(compaction_text(compaction)));
        }
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            " Session detail - Esc to go back ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                format!(
                    " lines {}-{} of {} ",
                    scroll + usize::from(total_lines > 0),
                    (scroll + inner_height).min(total_lines),
                    total_lines
                ),
                Style::default().fg(theme.muted),
            ))
            .right_aligned(),
        );
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(block)
            .scroll((scroll as u16, 0))
            .wrap(wrap),
        area,
    );
}

/// 将可能很长的详情字段按 Unicode 字符分段，并让续行与值列对齐。
fn push_wrapped_detail_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    width: usize,
    theme: Theme,
) {
    const LABEL_WIDTH: usize = 14;
    let value_width = width.saturating_sub(LABEL_WIDTH).max(1);
    let characters = value.chars().collect::<Vec<_>>();
    let chunks = characters.chunks(value_width).collect::<Vec<_>>();
    for (index, chunk) in chunks.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                if index == 0 {
                    format!("{label:<LABEL_WIDTH$}")
                } else {
                    " ".repeat(LABEL_WIDTH)
                },
                Style::default().fg(theme.muted),
            ),
            Span::styled(
                chunk.iter().collect::<String>(),
                Style::default().fg(theme.foreground),
            ),
        ]));
    }
    if chunks.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{label:<LABEL_WIDTH$}"),
            Style::default().fg(theme.muted),
        )));
    }
}

#[cfg(test)]
#[path = "detail_test.rs"]
mod tests;
