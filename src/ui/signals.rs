//! Recent Signal 的格式化与首页面板。

use super::{selected_style, text, AllRegion, Ui};
use crate::app::{fmt_relative, fmt_tokens};
use crate::domain::{Provider, SessionState};
use crate::session_labels::SessionLabels;
use crate::signals::{ContextAlertLevel, RecentSignal, RecentSignalKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::collections::{HashMap, HashSet};

/// 在首页状态带中绘制最近发生的状态变化证据。
pub(super) fn draw_panel(
    f: &mut Frame,
    area: Rect,
    signals: &[RecentSignal],
    now_ms: i64,
    ui: &Ui,
) {
    let theme = ui.theme.palette();
    let width = area.width.saturating_sub(2) as usize;
    let height = area.height.saturating_sub(2) as usize;
    let titles = visible_titles(signals, &ui.labels);
    // 每个 Signal 可能需要多行证据；达到面板高度后立即停止，不提供历史滚动。
    let lines = if signals.is_empty() {
        vec![Line::from(Span::styled(
            "No recent Signals.",
            Style::default().fg(theme.muted),
        ))]
    } else {
        let mut lines = Vec::new();
        let compact = signals
            .iter()
            .map(|signal| lines_for(signal, width, &titles).len())
            .sum::<usize>()
            > height;
        for (index, signal) in signals.iter().enumerate() {
            let style = if ui.all_region == AllRegion::RecentSignals && index == ui.signal_selected
            {
                selected_style(theme)
            } else {
                Style::default().fg(theme.foreground)
            };
            let max_signal_lines = if compact { 1 } else { usize::MAX };
            for text in lines_for(signal, width, &titles)
                .into_iter()
                .take(max_signal_lines)
            {
                if lines.len() == height {
                    break;
                }
                lines.push(Line::from(Span::styled(text, style)));
            }
            if lines.len() == height {
                break;
            }
        }
        lines
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(
            Style::default().fg(if ui.all_region == AllRegion::RecentSignals {
                theme.accent
            } else {
                theme.border
            }),
        )
        .title(Line::from(vec![
            Span::styled(
                " RECENT SIGNALS ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} ", signals.len()),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    // 只有标题空间足够时才显示 latest，避免和左侧标题重叠。
    if area.width >= 40 {
        if let Some(latest_at_ms) = signals.iter().map(RecentSignal::observed_at_ms).max() {
            block = block.title(
                Line::from(vec![
                    Span::styled(" latest ", Style::default().fg(theme.muted)),
                    Span::styled(
                        format!("{} ", fmt_relative(latest_at_ms, now_ms)),
                        Style::default().fg(theme.foreground),
                    ),
                ])
                .right_aligned(),
            );
        }
    }
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// 按 Provider 和原生 Session ID 索引本次渲染解析出的可见标题。
type VisibleTitles = HashMap<Provider, HashMap<String, String>>;

/// 一次性解析 Signals 涉及的所有 Session 标题，并处理重复标题消歧。
fn visible_titles(signals: &[RecentSignal], labels: &SessionLabels) -> VisibleTitles {
    let mut identities = Vec::<(Provider, String)>::new();
    let mut seen = HashSet::new();
    let mut add = |provider: Provider, native_id: &str| {
        let identity = (provider, native_id.to_string());
        if seen.insert(identity.clone()) {
            identities.push(identity);
        }
    };
    for signal in signals {
        match signal {
            RecentSignal::Session {
                provider,
                session_id,
                ..
            } => add(*provider, session_id),
            RecentSignal::Worktree { before, after, .. } => {
                for identity in before.iter().chain(after) {
                    add(identity.provider, &identity.native_id);
                }
            }
        }
    }
    let identity_refs = identities
        .iter()
        .map(|(provider, native_id)| (*provider, native_id.as_str()))
        .collect::<Vec<_>>();
    let resolved_titles = labels.titles_for(&identity_refs);
    let mut visible = VisibleTitles::new();
    for ((provider, native_id), title) in identities.into_iter().zip(resolved_titles) {
        visible
            .entry(provider)
            .or_default()
            .insert(native_id, title);
    }
    visible
}

/// 读取已解析标题。
fn visible_title<'a>(titles: &'a VisibleTitles, provider: Provider, native_id: &str) -> &'a str {
    titles
        .get(&provider)
        .and_then(|provider_titles| provider_titles.get(native_id))
        .map(String::as_str)
        .expect("every visible Signal Session has a Title")
}

/// 把一个 Recent Signal 压缩成适合当前宽度的一组纯文本证据行。
fn lines_for(signal: &RecentSignal, width: usize, titles: &VisibleTitles) -> Vec<String> {
    let RecentSignal::Session {
        provider,
        session_id,
        kind,
        ..
    } = signal
    else {
        let RecentSignal::Worktree {
            worktree_root,
            change,
            before,
            after,
            ..
        } = signal
        else {
            unreachable!();
        };
        let change = match change {
            crate::signals::SharedWorktreeRiskChange::Detected => "detected",
            crate::signals::SharedWorktreeRiskChange::Cleared => "cleared",
        };
        let suffix = format!("  Risk {change}");
        let root_width = width.saturating_sub(3 + suffix.chars().count());
        return vec![
            format!(
                "WT {}{suffix}",
                text::shorten_start(&worktree_root.display().to_string(), root_width)
            ),
            text::shorten_end(
                &format!("Before {}: {}", before.len(), members(before, titles)),
                width,
            ),
            text::shorten_end(
                &format!("After {}: {}", after.len(), members(after, titles)),
                width,
            ),
        ];
    };
    let evidence = match kind {
        RecentSignalKind::SessionStateChanged { from, to } => {
            format!("{} -> {}", state_label(*from), state_label(*to))
        }
        RecentSignalKind::ContextAlertChanged {
            from,
            to,
            before,
            after,
        } => {
            let (change, level) = match (from, to) {
                (None, Some(to)) => ("entered", *to),
                (Some(ContextAlertLevel::Warning), Some(ContextAlertLevel::Critical)) => {
                    ("entered", ContextAlertLevel::Critical)
                }
                (Some(from), _) => ("cleared", *from),
                (None, None) => unreachable!("unchanged Context bands are not Signals"),
            };
            format!(
                "Ctx {}{} {}>{}",
                if change == "entered" { "+" } else { "-" },
                alert_label(level),
                fmt_tokens(before.tokens),
                fmt_tokens(after.tokens)
            )
        }
        RecentSignalKind::CompactionObserved {
            compaction,
            context_before,
            context_after,
            cleared_alert,
        } => {
            let trigger =
                text::shorten_start(compaction.trigger.as_deref().unwrap_or("unknown"), 6);
            let cleared =
                cleared_alert.map_or_else(String::new, |level| format!(" -{}", alert_label(level)));
            format!(
                "Cmp {trigger} {}>{}{cleared}",
                fmt_tokens(context_before.tokens),
                fmt_tokens(context_after.tokens)
            )
        }
    };
    let title = visible_title(titles, *provider, session_id);
    let provider = match provider {
        Provider::ClaudeCode => "CL",
        Provider::CodexCli => "CX",
    };
    let fixed_width = provider.chars().count() + evidence.chars().count() + 2;
    let session_width = width.saturating_sub(fixed_width);
    let session = text::shorten_end(title, session_width);
    let text = if session.is_empty() {
        format!("{provider} {evidence}")
    } else {
        format!("{provider} {session} {evidence}")
    };
    vec![text::shorten_start(&text, width)]
}

/// 格式化 Worktree Signal 中带 Provider 前缀的成员列表。
fn members(members: &[crate::attention::SessionIdentity], titles: &VisibleTitles) -> String {
    members
        .iter()
        .map(|identity| {
            let provider = match identity.provider {
                Provider::ClaudeCode => "CL",
                Provider::CodexCli => "CX",
            };
            let title = visible_title(titles, identity.provider, &identity.native_id);
            format!("{provider}:{title}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn state_label(state: SessionState) -> &'static str {
    match state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => "Exited",
    }
}

fn alert_label(level: ContextAlertLevel) -> &'static str {
    match level {
        ContextAlertLevel::Warning => "Warning",
        ContextAlertLevel::Critical => "Critical",
    }
}

#[cfg(test)]
#[path = "signals_test.rs"]
mod tests;
