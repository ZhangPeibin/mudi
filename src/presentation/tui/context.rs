//! Current Context 的语义角色和共享进度呈现。

use super::theme::Theme;
use crate::adapters::config::{CRIT_PCT, WARN_PCT};
use crate::domain::{ContextState, SessionState};
use crate::presentation::app::fmt_tokens;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Normal,
    Warning,
    Critical,
    Neutral,
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
    let style = Style::default().fg(role_color(role, theme));
    if role == Role::Critical {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

/// The shared visual treatment used by Current Context detail surfaces.
#[derive(Debug, Clone, Copy)]
pub(super) struct Progress {
    tokens: u64,
    window: Option<u64>,
    percent: Option<f64>,
    role: Role,
    exited: bool,
}

/// The two detail presentations of the shared Current Context progress component.
#[derive(Debug, Clone, Copy)]
pub(super) enum Presentation {
    Compact,
    Expanded,
}

impl Progress {
    pub(super) fn new(context: ContextState, state: SessionState) -> Self {
        let percent = context.percent();
        let exited = state == SessionState::Exited;
        let role = if exited {
            Role::Neutral
        } else {
            percent.map_or(Role::Neutral, role)
        };

        Self {
            tokens: context.tokens,
            window: context.window.filter(|window| *window > 0),
            percent,
            role,
            exited,
        }
    }

    pub(super) fn percent_text(self) -> String {
        self.percent
            .map(|percent| format!("{percent:.1}%"))
            .unwrap_or_else(|| "N/A".into())
    }

    #[cfg(test)]
    pub(super) fn role(self) -> Role {
        self.role
    }

    /// Render a labelled Current Context line within the supplied content width.
    ///
    /// Compact details trade the token/window text for a short bar. Expanded
    /// details retain tokens and the context window, then add a wider bar when
    /// the surface has enough room. Both presentations retain the percentage or
    /// `N/A` when the bar is omitted.
    pub(super) fn line(
        self,
        presentation: Presentation,
        label: &str,
        label_width: usize,
        content_width: usize,
        theme: Theme,
    ) -> Line<'static> {
        let label_width = label_width.min(content_width);
        let separator = if label_width > 0 && content_width > label_width {
            " "
        } else {
            ""
        };
        let label = format!("{}{separator}", fit_label(label, label_width));
        let value_width = content_width.saturating_sub(UnicodeWidthStr::width(label.as_str()));
        let percent = self.percent_text();
        let (summary, max_bar_width, min_bar_width, min_content_width) = match presentation {
            Presentation::Compact => (None, 12, 6, 0),
            Presentation::Expanded => (
                Some(format!(
                    "{} / {}",
                    fmt_tokens(self.tokens),
                    self.window.map(fmt_tokens).unwrap_or_else(|| "N/A".into())
                )),
                32,
                12,
                // A Detail modal caps at roughly 100 columns. This applies after
                // its label column, so it keeps the bar at normal modal widths
                // while compact modals still degrade cleanly.
                72,
            ),
        };
        let bar_width = self.bar_width(
            summary.as_deref(),
            &percent,
            value_width,
            max_bar_width,
            min_bar_width,
            min_content_width,
        );
        let label_style = match presentation {
            Presentation::Compact => Style::default().fg(theme.muted),
            Presentation::Expanded => Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        };
        let progress_style = match presentation {
            Presentation::Compact => Style::default().fg(role_color(self.role, theme)),
            Presentation::Expanded => style(self.role, theme),
        };
        let mut spans = vec![Span::styled(label, label_style)];

        match (summary, bar_width) {
            (Some(summary), Some(width)) => {
                spans.push(Span::styled(summary, self.value_style(theme)));
                spans.push(Span::raw(" "));
                spans.extend(self.bar_spans(width, progress_style, theme));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(format!("({percent})"), progress_style));
            }
            (Some(summary), None) => {
                spans.push(Span::styled(summary, self.value_style(theme)));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(format!("({percent})"), progress_style));
            }
            (None, Some(width)) => {
                spans.extend(self.bar_spans(width, progress_style, theme));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(percent, progress_style));
            }
            (None, None) => spans.push(Span::styled(percent, progress_style)),
        }

        Line::from(spans)
    }

    fn bar_width(
        self,
        summary: Option<&str>,
        percent: &str,
        value_width: usize,
        max_bar_width: usize,
        min_bar_width: usize,
        min_content_width: usize,
    ) -> Option<usize> {
        if value_width < min_content_width {
            return None;
        }

        let fixed_width = summary.map_or(0, |summary| UnicodeWidthStr::width(summary) + 1)
            + UnicodeWidthStr::width(percent)
            + 3;
        let width = value_width.saturating_sub(fixed_width).min(max_bar_width);
        (width >= min_bar_width).then_some(width)
    }

    fn bar_spans(self, width: usize, progress_style: Style, theme: Theme) -> Vec<Span<'static>> {
        let filled = self
            .percent
            .map(|percent| ((percent / 100.0).clamp(0.0, 1.0) * width as f64).round() as usize)
            .unwrap_or_default();
        let filled = filled.min(width);
        let empty_style = Style::default().fg(theme.muted);

        vec![
            Span::styled("[", progress_style),
            Span::styled("#".repeat(filled), progress_style),
            Span::styled("-".repeat(width - filled), empty_style),
            Span::styled("]", progress_style),
        ]
    }

    fn value_style(self, theme: Theme) -> Style {
        Style::default().fg(if self.exited {
            theme.muted
        } else {
            theme.foreground
        })
    }
}

fn role_color(role: Role, theme: Theme) -> ratatui::style::Color {
    match role {
        Role::Normal => theme.context_normal,
        Role::Warning => theme.context_warning,
        Role::Critical => theme.context_critical,
        Role::Neutral => theme.muted,
    }
}

fn fit_label(label: &str, width: usize) -> String {
    let label_width = UnicodeWidthStr::width(label);
    if label_width >= width {
        return label.chars().take(width).collect();
    }

    format!("{label:<width$}")
}

#[cfg(test)]
#[path = "context_test.rs"]
mod tests;
