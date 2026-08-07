//! Current Context 的语义角色和可视证据。

use super::theme::Theme;
use crate::config::{CRIT_PCT, WARN_PCT};
use ratatui::style::{Modifier, Style};

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

#[cfg(test)]
#[path = "context_test.rs"]
mod tests;
