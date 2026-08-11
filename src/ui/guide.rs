//! Guide 弹窗、主题内容和滚动测量。

use super::{
    layout::centered_rect, provider_mark, selected_style, theme::Theme, GuideCategory, GuidePane,
    Ui,
};
use crate::config::{CRIT_PCT, WARN_PCT};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState, Wrap};
use ratatui::Frame;

pub(super) fn draw(f: &mut Frame, ui: &Ui) {
    let theme = ui.theme.palette();
    let area = centered_rect(80, 76, f.area());
    let focus_hint = match ui.guide_pane {
        GuidePane::Categories => " j/k Categories | Tab/Right Content | Esc Close ",
        GuidePane::Content => " j/k Scroll | Shift+Tab/Left Categories | Esc Close ",
    };
    let base_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            " Guide ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = base_block.inner(area);
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(24), Constraint::Percentage(76)])
        .split(inner);
    let content = guide_content(ui.guide_category, theme, ui.active_theme_name());
    let wrap = Wrap { trim: false };
    // 先判断是否需要滚动条，再用扣除滚动条后的宽度重新测量真实行数。
    let initial_total_lines = Paragraph::new(content.clone())
        .wrap(wrap)
        .line_count(panes[1].width);
    let overflow = initial_total_lines > panes[1].height as usize;
    let content_area = Rect {
        width: panes[1].width.saturating_sub(u16::from(overflow)),
        ..panes[1]
    };
    let total_lines = Paragraph::new(content.clone())
        .wrap(wrap)
        .line_count(content_area.width);
    let max_scroll = total_lines.saturating_sub(content_area.height as usize);
    ui.guide_max_scroll.set(max_scroll);
    let scroll = ui.guide_scroll.min(max_scroll);
    let mut block = base_block.title_bottom(
        Line::from(Span::styled(focus_hint, Style::default().fg(theme.muted))).left_aligned(),
    );
    if overflow {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {}/{} ", scroll + 1, total_lines),
                Style::default().fg(theme.muted),
            ))
            .right_aligned(),
        );
    }
    let categories = GuideCategory::ALL
        .into_iter()
        .map(|category| {
            let selected = category == ui.guide_category;
            Line::from(Span::styled(
                format!("{} {}", if selected { '>' } else { ' ' }, category.label()),
                if selected {
                    selected_style(theme)
                } else {
                    Style::default().fg(theme.foreground)
                },
            ))
        })
        .collect::<Vec<_>>();

    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(categories)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(
                        Style::default().fg(if ui.guide_pane == GuidePane::Categories {
                            theme.accent
                        } else {
                            theme.border
                        }),
                    ),
            )
            .wrap(Wrap { trim: false }),
        panes[0],
    );
    f.render_widget(
        Paragraph::new(content)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .scroll((scroll as u16, 0))
            .wrap(wrap),
        content_area,
    );
    if overflow {
        let scrollbar = Scrollbar::default()
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(theme.accent))
            .track_style(Style::default().fg(theme.border));
        let mut scrollbar_state = ScrollbarState::new(total_lines)
            .position(scroll)
            .viewport_content_length(content_area.height as usize);
        f.render_stateful_widget(scrollbar, panes[1], &mut scrollbar_state);
    }
}

/// 为指定 Guide 主题生成带语义样式的正文行。
fn guide_content(
    category: GuideCategory,
    theme: Theme,
    active_theme_name: &str,
) -> Vec<Line<'static>> {
    let claude_mark = provider_mark::CLAUDE;
    let codex_mark = provider_mark::CODEX;
    let heading = |text: &'static str| {
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let lines = match category {
        GuideCategory::Overview => vec![
            heading(category.label()),
            Line::from("SessionMesh is local to this Mac."),
            Line::from("Passive observation is the default."),
            Line::from("Control Operations require explicit authorization."),
            heading("Pages"),
            Line::from("1  All"),
            Line::from("2  Claude Code"),
            Line::from("3  Codex CLI"),
            heading("Current and recent"),
            Line::from("NOW shows current conditions."),
            Line::from("Recent Signals show transitions."),
        ],
        GuideCategory::Navigation => vec![
            heading(category.label()),
            Line::from("1 / 2 / 3  Open a page   n  New Session"),
            Line::from("Tab / Shift+Tab  Change region or Guide pane"),
            Line::from("Up/Down or j/k  Move or scroll   Enter  Inspect or expand"),
            Line::from("Left/Right  Collapse, expand, or move through the Session Tree"),
            Line::from("Enter  View an indexed linked Successor from Task Detail"),
            Line::from("Esc  Back or close   ?  Guide   q  Quit"),
            Line::from("c  Copy the selected Session Detail field"),
            Line::from("h  Authorize a Handoff from Session Detail"),
            Line::from("h  Open Hidden Tree from a top-level page"),
            Line::from("H  Hide the selected Exited Session or Project snapshot"),
            Line::from("u / d  Restore or permanently exclude from Hidden Tree"),
            Line::from("Left/Right  Choose a Successor Provider"),
            Line::from("o  Open a Ready Successor in Terminal.app"),
            Line::from("c  Copy a Ready Successor command"),
            Line::from("Exact launched-process evidence confirms Successor linkage."),
            Line::from("x  Cancel a non-terminal Task after confirmation"),
            Line::from("r  Refresh, or Retry an eligible Task"),
            Line::from("d  Delete a terminal Task and owned Artifact after confirmation"),
            Line::from("Tasks and Artifacts never expire automatically."),
        ],
        GuideCategory::Sessions => vec![
            heading(category.label()),
            heading("Session Titles"),
            Line::from("Title  The name SessionMesh displays for a Session."),
            Line::from("Alias  A local user-authored Title."),
            Line::from("It changes only SessionMesh; Provider data is unchanged."),
            Line::from("Short Session ID  The last eight characters used when no Alias exists."),
            Line::from("Duplicate Titles include their Short Session ID."),
            Line::from("e Edit session  Set or clear its local Title and Description."),
            Line::from("Inventory: H hides Exited Sessions; h opens Hidden Tree."),
            Line::from("u restores; d excludes permanently. Provider history stays unchanged."),
            heading("Providers"),
            Line::from(format!(
                "{claude_mark} Claude Code   {codex_mark} Codex CLI"
            )),
            heading("Open Session"),
            Line::from("● Busy   ○ Idle   no marker Exited"),
            Line::from("!  The Session currently needs attention."),
            heading("Current Context"),
            Line::from("Current Context is the latest known content occupying the model window."),
            Line::from("It is not cumulative Processed Usage."),
            Line::from(format!(
                "Warning  {WARN_PCT:.0}%   Critical {CRIT_PCT:.0}%"
            )),
        ],
        GuideCategory::Theme => vec![
            heading(category.label()),
            Line::from(format!("Active Theme  {active_theme_name}")),
            heading("Built-in Themes"),
            Line::from("sessionmesh  catppuccin  dracula"),
            Line::from("gruvbox  light  high-contrast"),
            heading("Command-line override"),
            Line::from("--theme <name>"),
            Line::from("Applies to one process and does not change the saved Theme."),
            heading("Semantics"),
            Line::from("Current Context uses Normal, Warning, and Critical colors."),
            Line::from("Idle and Busy use separate lifecycle colors."),
            Line::from("Theme changes presentation, never Session state or thresholds."),
        ],
        GuideCategory::Signals => vec![
            heading(category.label()),
            Line::from("Recent Signals answer: what just changed? They are not current state."),
            heading("Session lifecycle"),
            Line::from(format!(
                "{codex_mark} session1 Busy -> Idle  Request ended; the Open Session is waiting for input."
            )),
            Line::from(format!(
                "{codex_mark} session1 Busy -> Exited  A running Provider process exited."
            )),
            Line::from(format!(
                "{codex_mark} session1 Idle -> Exited  A waiting Provider process exited."
            )),
            Line::from(format!(
                "{codex_mark} session1 Exited -> Busy  A Session resumed and is running."
            )),
            Line::from(format!(
                "{codex_mark} session1 Exited -> Idle  A Session reopened and is waiting for input."
            )),
            heading("Current Context"),
            Line::from(format!(
                "{claude_mark} session1 Ctx +Warning 120.0k>150.0k  Current Context entered Warning."
            )),
            Line::from(format!(
                "{claude_mark} session1 Ctx +Critical 160.0k>180.0k  Current Context entered Critical."
            )),
            Line::from(format!(
                "{claude_mark} session1 Ctx -Warning 150.0k>120.0k  The Warning condition cleared."
            )),
            Line::from(format!(
                "{claude_mark} session1 Ctx -Critical 180.0k>160.0k  Critical cleared to Warning or normal."
            )),
            Line::from("Signals show before -> after Current Context values."),
            Line::from(
                "Only Open Sessions with a known Current Context percentage cross thresholds.",
            ),
            Line::from(format!(
                "Warning starts at {WARN_PCT:.0}%; Critical starts at {CRIT_PCT:.0}%."
            )),
            heading("Compaction"),
            Line::from(format!(
                "{claude_mark} session1 Cmp auto 188.0k>100.0k  Compaction observed."
            )),
            Line::from(format!(
                "{claude_mark} session1 Cmp auto 188.0k>100.0k -Critical  Also cleared Critical."
            )),
            heading("Worktree"),
            Line::from(
                "WT /project  Risk detected  Two or more Open Sessions share a writable Worktree.",
            ),
            Line::from("WT /project  Risk cleared  Fewer than two Open Sessions remain there."),
            Line::from(format!("Before 1: {claude_mark}:session1")),
            Line::from(format!(
                "After 2: {claude_mark}:session1, {codex_mark}:session2"
            )),
            Line::from("Two Open Sessions trigger Shared Worktree Risk."),
            Line::from("Evidence shows Open Session membership before -> after."),
            Line::from("This is not proof of a file-level or Git conflict."),
            heading("Marks"),
            Line::from(format!("{claude_mark}  Claude Code")),
            Line::from(format!("{codex_mark}  Codex CLI")),
            Line::from("WT  Worktree"),
            Line::from("Ctx Current Context"),
            Line::from("Cmp Compaction"),
            heading("Not recorded"),
            Line::from("Idle -> Busy is not recorded."),
            Line::from("Busy heartbeats and ordinary Current Context percentage changes."),
            Line::from("Usage, Cost, source timestamps, or duplicate polls."),
            Line::from("Possible Stall is not recorded."),
            heading("Retention and privacy"),
            Line::from("The first scan is a silent baseline and emits no Signals."),
            Line::from("Recent Signals are metadata-only."),
            Line::from(
                "Only the five newest Signals are retained, newest first, for this process.",
            ),
            Line::from("Current state belongs in NOW."),
            Line::from("Recent Signals never include:"),
            Line::from("prompt, response, tool, or source-code content"),
        ],
    };
    lines
        .into_iter()
        .map(|line| provider_mark::colorize(line, theme))
        .collect()
}

#[cfg(test)]
#[path = "guide_test.rs"]
mod tests;
