//! Current Context 的语义角色和可视证据。

use super::theme::Theme;
use crate::config::{CRIT_PCT, WARN_PCT};
use crate::domain::Session;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Normal,
    Warning,
    Critical,
}

/// 按 Warning/Critical 阈值选择 Current Context 的语义角色。
pub(super) fn role(percent: f64) -> Role {
    if percent >= CRIT_PCT {
        Role::Critical
    } else if percent >= WARN_PCT {
        Role::Warning
    } else {
        Role::Normal
    }
}

/// 把 Context 语义角色映射为当前主题样式。
pub(super) fn style(role: Role, theme: Theme) -> Style {
    let color = match role {
        Role::Normal => theme.context_normal,
        Role::Warning => theme.context_warning,
        Role::Critical => theme.context_critical,
    };
    let style = Style::default().fg(color);
    if role == Role::Critical {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

/// 生成 Context 进度条、百分比及未知窗口的可视证据。
pub(super) fn evidence(session: &Session, bar_width: usize, theme: Theme) -> Vec<Span<'static>> {
    let muted = Style::default().fg(theme.muted);
    let evidence_width = bar_width + 8;
    let Some(percent) = session.context.percent() else {
        let text = format!("[{}] N/A", "-".repeat(bar_width));
        let padding = evidence_width.saturating_sub(text.chars().count());
        return vec![Span::styled(
            format!("{text}{}", " ".repeat(padding)),
            muted,
        )];
    };

    let filled =
        ((percent.clamp(0.0, 100.0) / 100.0 * bar_width as f64).round() as usize).min(bar_width);
    let current_role = role(percent);
    let mut spans = Vec::with_capacity(bar_width + 4);
    spans.push(Span::styled("[", muted));
    for index in 0..bar_width {
        if index < filled {
            spans.push(Span::styled("#", style(current_role, theme)));
        } else {
            spans.push(Span::styled("-", muted));
        }
    }
    spans.push(Span::styled("]", muted));

    let percent_text = format!(" {:.0}%", percent);
    let padding = evidence_width.saturating_sub(bar_width + 2 + percent_text.chars().count());
    spans.push(Span::styled(percent_text, style(role(percent), theme)));
    spans.push(Span::styled(" ".repeat(padding), muted));
    spans
}

#[cfg(test)]
#[path = "context_test.rs"]
mod tests;
