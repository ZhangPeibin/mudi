//! TUI rendering (Ratatui). Cozy, dense, three-tab layout.
//!
//! The UI is pure: it reads a [`Snapshot`] plus interactive [`Ui`] state and draws.
//! It never touches provider field names or performs IO.

use crate::alerts::AlertCounts;
use crate::app::{
    fmt_context, fmt_cost, fmt_datetime, fmt_relative, fmt_tokens, ProviderTotals, Snapshot, Tab,
    TimeFilter,
};
use crate::collector;
use crate::domain::{Provider, Session, SessionState};
use chrono::Local;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use ratatui::Frame;

// ---- theme ----
const ACCENT: Color = Color::Rgb(0xE6, 0xB4, 0x50); // warm gold
const DIM: Color = Color::Rgb(0x6c, 0x6c, 0x70);
const BUSY: Color = Color::Rgb(0x8f, 0xd6, 0x7a); // green
const IDLE: Color = Color::Rgb(0x7a, 0xb8, 0xd6); // blue
const WARN: Color = Color::Rgb(0xE6, 0xC0, 0x50);
const CRIT: Color = Color::Rgb(0xE0, 0x6c, 0x6c);
const FG: Color = Color::Rgb(0xd8, 0xd4, 0xcc);

/// Interactive UI state (owned by the event loop).
pub struct Ui {
    pub tab: Tab,
    pub filter: TimeFilter,
    pub selected: usize,
    pub detail: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Ui { tab: Tab::All, filter: TimeFilter::Today, selected: 0, detail: false }
    }
}

impl Ui {
    /// Sessions visible on the current provider tab (filtered + sorted).
    pub fn visible<'a>(&self, snap: &'a Snapshot) -> Vec<Session> {
        match self.tab.provider() {
            Some(p) => collector::provider_sessions(&snap.sessions, p, self.filter.cutoff_ms()),
            None => Vec::new(),
        }
    }

    pub fn clamp_selection(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }
}

/// Draw the whole screen.
pub fn draw(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // tabs
            Constraint::Length(1), // time-filter bar
            Constraint::Min(3),    // content
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_tabs(f, chunks[0], ui);
    draw_filter_bar(f, chunks[1], ui);

    match ui.tab {
        Tab::All => draw_all(f, chunks[2], snap, ui),
        _ => draw_provider(f, chunks[2], snap, ui),
    }

    draw_footer(f, chunks[3], snap, ui);

    if ui.detail && ui.tab != Tab::All {
        draw_detail(f, snap, ui);
    }
}

fn draw_tabs(f: &mut Frame, area: Rect, ui: &Ui) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|t| Line::from(format!(" {} ", t.title())))
        .collect();
    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DIM))
                .title(Span::styled(
                    " TokenPlay ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
        )
        .select(ui.tab.index())
        .style(Style::default().fg(FG))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

/// A segmented time-range selector: `Time: [Active Today] [Last 7 Days] …` with the
/// active range highlighted. Makes it obvious that `f` cycles the time window.
fn draw_filter_bar(f: &mut Frame, area: Rect, ui: &Ui) {
    let mut spans = vec![Span::styled(" Time  ", Style::default().fg(DIM))];
    for (i, tf) in TimeFilter::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default().fg(DIM)));
        }
        let style = if *tf == ui.filter {
            Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG)
        };
        spans.push(Span::styled(format!(" {} ", tf.title()), style));
    }
    spans.push(Span::styled("   (f to change)", Style::default().fg(DIM)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---- All dashboard ----

fn draw_all(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let cutoff = ui.filter.cutoff_ms();
    let claude = snap.totals(Provider::ClaudeCode, cutoff);
    let codex = snap.totals(Provider::CodexCli, cutoff);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // provider cards
            Constraint::Length(6), // combined + alerts
            Constraint::Min(0),    // note
        ])
        .split(area);

    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    draw_provider_card(f, cards[0], "Claude Code", claude, Provider::ClaudeCode);
    draw_provider_card(f, cards[1], "Codex CLI", codex, Provider::CodexCli);

    let lower = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    draw_combined_card(f, lower[0], claude, codex);
    draw_alerts_card(f, lower[1], snap.alerts);

    let note = Paragraph::new(Line::from(vec![
        Span::styled("Note: ", Style::default().fg(DIM)),
        Span::styled(
            "totals are the full lifetime usage of sessions active in the selected window; usage is never split across days.",
            Style::default().fg(DIM),
        ),
    ]))
    .wrap(Wrap { trim: true });
    f.render_widget(note, rows[2]);
}

fn draw_provider_card(f: &mut Frame, area: Rect, title: &str, t: ProviderTotals, provider: Provider) {
    let color = match provider {
        Provider::ClaudeCode => ACCENT,
        Provider::CodexCli => IDLE,
    };
    let cost = if t.cost_complete { fmt_cost(Some(t.cost)) } else { format!("{}+", fmt_cost(Some(t.cost))) };
    let body = vec![
        Line::from(vec![
            Span::styled("Sessions  ", Style::default().fg(DIM)),
            Span::styled(t.sessions.to_string(), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
            Span::styled(format!("   ({} open)", t.open), Style::default().fg(DIM)),
        ]),
        Line::from(vec![
            Span::styled("Est. cost ", Style::default().fg(DIM)),
            Span::styled(cost, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![Span::styled(
            if t.cost_complete { "" } else { "some models unpriced (N/A)" },
            Style::default().fg(DIM),
        )]),
    ];
    let p = Paragraph::new(body).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM))
            .title(Span::styled(format!(" {title} "), Style::default().fg(color).add_modifier(Modifier::BOLD))),
    );
    f.render_widget(p, area);
}

fn draw_combined_card(f: &mut Frame, area: Rect, c: ProviderTotals, x: ProviderTotals) {
    let complete = c.cost_complete && x.cost_complete;
    let total = c.cost + x.cost;
    let cost = if complete { fmt_cost(Some(total)) } else { format!("{}+", fmt_cost(Some(total))) };
    let body = vec![
        Line::from(vec![
            Span::styled("Total sessions  ", Style::default().fg(DIM)),
            Span::styled((c.sessions + x.sessions).to_string(), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("Combined cost   ", Style::default().fg(DIM)),
            Span::styled(cost, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ]),
    ];
    let p = Paragraph::new(body).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM))
            .title(Span::styled(" Combined ", Style::default().fg(FG).add_modifier(Modifier::BOLD))),
    );
    f.render_widget(p, area);
}

fn draw_alerts_card(f: &mut Frame, area: Rect, a: AlertCounts) {
    let warn_style = if a.warning > 0 { Style::default().fg(WARN).add_modifier(Modifier::BOLD) } else { Style::default().fg(DIM) };
    let crit_style = if a.critical > 0 { Style::default().fg(CRIT).add_modifier(Modifier::BOLD) } else { Style::default().fg(DIM) };
    let body = vec![
        Line::from(vec![
            Span::styled("Warning  (≥70%)  ", Style::default().fg(DIM)),
            Span::styled(a.warning.to_string(), warn_style),
        ]),
        Line::from(vec![
            Span::styled("Critical (≥85%)  ", Style::default().fg(DIM)),
            Span::styled(a.critical.to_string(), crit_style),
        ]),
        Line::from(Span::styled("open sessions only", Style::default().fg(DIM))),
    ];
    let p = Paragraph::new(body).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM))
            .title(Span::styled(" Context Alerts ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))),
    );
    f.render_widget(p, area);
}

// ---- provider tab ----

fn draw_provider(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let sessions = ui.visible(snap);
    let now = Local::now().timestamp_millis();

    if sessions.is_empty() {
        let msg = if snap.ready {
            "No sessions match this filter."
        } else {
            "Indexing… sessions will appear shortly."
        };
        let p = Paragraph::new(msg)
            .style(Style::default().fg(DIM))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(DIM)));
        f.render_widget(p, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("State"),
        Cell::from("Project"),
        Cell::from("Model"),
        Cell::from("Context"),
        Cell::from("Est. $"),
        Cell::from("Updated"),
        Cell::from("Cmp"),
    ])
    .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = sessions.iter().map(|s| session_row(s, now)).collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Percentage(30),
        Constraint::Length(18),
        Constraint::Length(16),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(4),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DIM))
                .title(Span::styled(
                    format!(" {} ", ui.tab.title()),
                    Style::default().fg(FG),
                )),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(0x33, 0x30, 0x28)).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌");

    let mut ts = TableState::default();
    ts.select(Some(ui.selected.min(sessions.len().saturating_sub(1))));
    f.render_stateful_widget(table, area, &mut ts);
}

fn state_cell(state: SessionState) -> Cell<'static> {
    match state {
        SessionState::Busy => Cell::from(Span::styled("● Busy", Style::default().fg(BUSY).add_modifier(Modifier::BOLD))),
        SessionState::Idle => Cell::from(Span::styled("○ Idle", Style::default().fg(IDLE))),
        SessionState::Exited => Cell::from(Span::styled("· —", Style::default().fg(DIM))),
    }
}

fn context_style(s: &Session) -> Style {
    match s.context.percent() {
        Some(p) if p >= 85.0 => Style::default().fg(CRIT).add_modifier(Modifier::BOLD),
        Some(p) if p >= 70.0 => Style::default().fg(WARN),
        _ => Style::default().fg(FG),
    }
}

fn session_row(s: &Session, now: i64) -> Row<'static> {
    let model = s.latest_model.clone().unwrap_or_else(|| "—".into());
    let cmp = if s.compaction_count() > 0 { format!("⤳{}", s.compaction_count()) } else { "·".to_string() };
    let cmp_style = if s.compaction_count() > 0 { Style::default().fg(ACCENT) } else { Style::default().fg(DIM) };

    Row::new(vec![
        state_cell(s.state),
        Cell::from(Span::styled(s.project.clone(), Style::default().fg(FG))),
        Cell::from(Span::styled(model, Style::default().fg(DIM))),
        Cell::from(Span::styled(fmt_context(s), context_style(s))),
        Cell::from(Span::styled(fmt_cost(s.cost), Style::default().fg(FG))),
        Cell::from(Span::styled(fmt_relative(s.last_activity_ms, now), Style::default().fg(DIM))),
        Cell::from(Span::styled(cmp, cmp_style)),
    ])
}

// ---- detail overlay ----

fn draw_detail(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let sessions = ui.visible(snap);
    let Some(s) = sessions.get(ui.selected) else { return };

    let area = centered_rect(72, 80, f.area());
    f.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::new();
    let hdr = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<16}"), Style::default().fg(DIM)),
            Span::styled(v, Style::default().fg(FG)),
        ])
    };

    lines.push(hdr("Provider", s.provider.label().to_string()));
    lines.push(hdr("Project", s.project.clone()));
    lines.push(hdr("Session", s.id.clone()));
    lines.push(hdr("Latest model", s.latest_model.clone().unwrap_or_else(|| "—".into())));
    lines.push(hdr("State", match s.state {
        SessionState::Busy => "Busy".into(),
        SessionState::Idle => "Idle".into(),
        SessionState::Exited => "exited (no marker)".into(),
    }));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("Current Context", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))));
    let ctx = match s.context.percent() {
        Some(pct) => format!("{} / {} ({:.1}%)", fmt_tokens(s.context.tokens), s.context.window.map(fmt_tokens).unwrap_or_else(|| "?".into()), pct),
        None => format!("{} / N/A (N/A)", fmt_tokens(s.context.tokens)),
    };
    lines.push(hdr("Occupancy", ctx));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("Processed Usage (lifetime)", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))));
    lines.push(hdr("Input", fmt_tokens(s.usage.input)));
    lines.push(hdr("Output", fmt_tokens(s.usage.output)));
    lines.push(hdr("Cache read", fmt_tokens(s.usage.cache_read)));
    lines.push(hdr("Cache write", fmt_tokens(s.usage.cache_write)));
    lines.push(hdr("Est. cost", fmt_cost(s.cost)));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("Per-model breakdown", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))));
    for m in &s.per_model {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<20}", m.model), Style::default().fg(FG)),
            Span::styled(
                format!("in {} out {} cr {} cw {}  ", fmt_tokens(m.usage.input), fmt_tokens(m.usage.output), fmt_tokens(m.usage.cache_read), fmt_tokens(m.usage.cache_write)),
                Style::default().fg(DIM),
            ),
            Span::styled(fmt_cost(m.cost), Style::default().fg(FG)),
        ]));
    }
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        format!("Compaction timeline ({})", s.compaction_count()),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )));
    if s.compactions.is_empty() {
        lines.push(Line::from(Span::styled("  none observed", Style::default().fg(DIM))));
    } else {
        for c in &s.compactions {
            let trigger = c.trigger.clone().unwrap_or_else(|| "—".into());
            let pre = c.pre_tokens.map(fmt_tokens).unwrap_or_else(|| "?".into());
            lines.push(Line::from(Span::styled(
                format!("  {}  trigger={}  pre={}", fmt_datetime(c.at_ms), trigger, pre),
                Style::default().fg(FG),
            )));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(" Session detail — Esc to close ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
    let p = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

// ---- footer ----

fn draw_footer(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let left = if ui.tab == Tab::All {
        "Tab/1-3 switch · f time range · r rescan · q quit".to_string()
    } else {
        "↑/↓ move · Enter detail · f time range · r rescan · Tab switch · q quit".to_string()
    };
    let right = format!("{}   ·   Estimated using API rates", snap.status);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);
    f.render_widget(Paragraph::new(Span::styled(left, Style::default().fg(DIM))), cols[0]);
    f.render_widget(
        Paragraph::new(Span::styled(right, Style::default().fg(DIM))).alignment(Alignment::Right),
        cols[1],
    );
}

fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ContextState, ModelUsage, Provider, Usage};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn demo_snapshot() -> Snapshot {
        let session = Session {
            provider: Provider::ClaudeCode,
            id: "demo-session".into(),
            project: "tokenplay".into(),
            cwd: Some("/Users/x/code/tokenplay".into()),
            latest_model: Some("claude-opus-4-8".into()),
            state: SessionState::Busy,
            context: ContextState { tokens: 150_000, window: Some(200_000) },
            usage: Usage { input: 1000, output: 500, cache_read: 200, cache_write: 100 },
            per_model: vec![ModelUsage {
                model: "claude-opus-4-8".into(),
                usage: Usage { input: 1000, output: 500, cache_read: 200, cache_write: 100 },
                cost: Some(0.42),
            }],
            compactions: vec![],
            last_activity_ms: 0,
            cost: Some(0.42),
        };
        Snapshot {
            sessions: vec![session],
            alerts: AlertCounts { warning: 1, critical: 0 },
            status: "watching".into(),
            ready: true,
            ..Default::default()
        }
    }

    fn render_tab(tab: Tab, detail: bool) {
        let snap = demo_snapshot();
        let ui = Ui { tab, filter: TimeFilter::All, selected: 0, detail };
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        // Must not panic while laying out or drawing.
        terminal.draw(|f| draw(f, &snap, &ui)).unwrap();
    }

    #[test]
    fn renders_all_tab() {
        render_tab(Tab::All, false);
    }

    #[test]
    fn renders_provider_tab_and_detail() {
        render_tab(Tab::Claude, false);
        render_tab(Tab::Claude, true);
    }
}

