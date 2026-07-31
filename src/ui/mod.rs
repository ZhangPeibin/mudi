//! TUI rendering and interaction state (Ratatui). Cozy, dense, three-page layout.
//!
//! Rendering reads a [`Snapshot`] plus interactive [`Ui`] state. Provider field names
//! stay behind adapters; Profile-backed preferences are the only UI-owned persistence.

mod theme;

use crate::app::{
    fmt_context, fmt_cost, fmt_datetime, fmt_relative, fmt_tokens, Snapshot, Tab, TimeFilter,
};
use crate::attention::{AttentionCondition, AttentionEngine, AttentionRank, AttentionSnapshot};
use crate::collector;
use crate::domain::{Project, ProjectIdentity, Provider, Session, SessionState, Worktree};
use chrono::Local;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use theme::{Theme, ThemeName};

/// Shared style for the currently-selected item in a segmented control (tabs, filter
/// bar).
fn selected_style(theme: Theme) -> Style {
    Style::default()
        .fg(theme.selected_foreground)
        .bg(theme.selected_background)
        .add_modifier(Modifier::BOLD)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionIdentity {
    provider: Provider,
    native_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailView {
    session: SessionIdentity,
    scroll: usize,
}

/// Interactive UI state (owned by the event loop).
pub struct Ui {
    pub tab: Tab,
    pub filter: TimeFilter,
    pub selected: usize,
    detail: Option<DetailView>,
    help_open: bool,
    profile_path: Option<PathBuf>,
    profile: ProfilePreferences,
    theme: ThemeName,
    theme_override_active: bool,
    theme_notice_until: Option<Instant>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
struct ProfilePreferences {
    #[serde(default)]
    help_seen: bool,
    #[serde(default)]
    theme: ThemeName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiAction {
    None,
    Rescan,
    Quit,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            tab: Tab::All,
            filter: TimeFilter::Today,
            selected: 0,
            detail: None,
            help_open: false,
            profile_path: None,
            profile: ProfilePreferences::default(),
            theme: ThemeName::default(),
            theme_override_active: false,
            theme_notice_until: None,
        }
    }
}

impl Ui {
    /// Load UI state for a local Profile.
    pub fn load(
        profile_path: impl AsRef<Path>,
        theme_override: Option<&str>,
    ) -> std::io::Result<Self> {
        let profile_path = profile_path.as_ref().to_path_buf();
        let profile = match fs::read(&profile_path) {
            Ok(contents) => serde_json::from_slice(&contents).map_err(std::io::Error::other)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ProfilePreferences::default()
            }
            Err(error) => return Err(error),
        };
        let theme = theme_override
            .and_then(ThemeName::from_name)
            .unwrap_or_else(|| {
                if theme_override.is_some() {
                    ThemeName::default()
                } else {
                    profile.theme
                }
            });
        Ok(Self {
            help_open: !profile.help_seen,
            profile_path: Some(profile_path),
            profile,
            theme,
            theme_override_active: theme_override.is_some(),
            ..Self::default()
        })
    }

    pub fn active_tab(&self) -> Tab {
        self.tab
    }

    pub fn active_theme_name(&self) -> &'static str {
        self.theme.as_str()
    }

    /// Apply one user key through the UI's public interaction interface.
    pub fn handle_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        snap: &Snapshot,
    ) -> std::io::Result<UiAction> {
        match key.code {
            crossterm::event::KeyCode::Char('q') => return Ok(UiAction::Quit),
            crossterm::event::KeyCode::Char('c')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                return Ok(UiAction::Quit);
            }
            crossterm::event::KeyCode::Char('r') => return Ok(UiAction::Rescan),
            crossterm::event::KeyCode::Char('1') => self.set_tab(Tab::All),
            crossterm::event::KeyCode::Char('2') => self.set_tab(Tab::Claude),
            crossterm::event::KeyCode::Char('3') => self.set_tab(Tab::Codex),
            crossterm::event::KeyCode::Char('?') => self.help_open = true,
            crossterm::event::KeyCode::Char('t') => {
                self.theme = self.theme.next();
                if !self.theme_override_active {
                    self.profile.theme = self.theme;
                    self.save_profile()?;
                }
                self.theme_notice_until = Some(Instant::now() + Duration::from_secs(2));
            }
            crossterm::event::KeyCode::Esc if self.help_open => {
                self.profile.help_seen = true;
                self.save_profile()?;
                self.help_open = false;
            }
            _ if self.help_open => {}
            crossterm::event::KeyCode::Char('f')
                if self.detail.is_none() && self.tab != Tab::All =>
            {
                self.filter = self.filter.next();
                self.selected = 0;
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_none() =>
            {
                let visible_len = self.visible(snap).len();
                if self.selected + 1 < visible_len {
                    self.selected += 1;
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_none() =>
            {
                self.selected = self.selected.saturating_sub(1);
            }
            crossterm::event::KeyCode::Enter
                if self.detail.is_none() && !self.visible(snap).is_empty() =>
            {
                let sessions = self.visible(snap);
                if let Some(session) = sessions.get(self.selected) {
                    self.detail = Some(DetailView {
                        session: SessionIdentity {
                            provider: session.provider,
                            native_id: session.id.clone(),
                        },
                        scroll: 0,
                    });
                }
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_some() =>
            {
                if let Some(detail) = self.detail.as_mut() {
                    detail.scroll = detail.scroll.saturating_add(1);
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_some() =>
            {
                if let Some(detail) = self.detail.as_mut() {
                    detail.scroll = detail.scroll.saturating_sub(1);
                }
            }
            crossterm::event::KeyCode::Esc => {
                self.close_detail(snap);
            }
            _ => {}
        }
        Ok(UiAction::None)
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.selected = 0;
        self.detail = None;
    }

    fn close_detail(&mut self, snap: &Snapshot) {
        let Some(identity) = self.detail.as_ref().map(|detail| detail.session.clone()) else {
            return;
        };
        if let Some(index) = self.visible(snap).iter().position(|session| {
            session.provider == identity.provider && session.id == identity.native_id
        }) {
            self.selected = index;
        }
        self.detail = None;
    }

    fn save_profile(&self) -> std::io::Result<()> {
        let Some(path) = &self.profile_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_vec_pretty(&self.profile).map_err(std::io::Error::other)?;
        fs::write(path, contents)
    }

    /// Selectable Sessions visible on the current page, in rendered row order.
    pub fn visible(&self, snap: &Snapshot) -> Vec<Session> {
        match self.tab.provider() {
            Some(p) => collector::provider_sessions(&snap.sessions, p, self.filter.cutoff_ms()),
            None => {
                let attention = current_attention(snap);
                group_open_sessions(&snap.sessions, &attention)
                    .into_iter()
                    .flat_map(|project| {
                        project
                            .worktrees
                            .into_iter()
                            .flat_map(|worktree| worktree.sessions)
                            .chain(project.direct_sessions)
                    })
                    .cloned()
                    .collect()
            }
        }
    }

    pub fn clamp_selection(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    fn detail_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        let identity = &self.detail.as_ref()?.session;
        snap.sessions.iter().find(|session| {
            session.provider == identity.provider && session.id == identity.native_id
        })
    }
}

/// Draw the whole screen.
pub fn draw(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    f.render_widget(
        Block::default().style(Style::default().fg(theme.foreground).bg(theme.background)),
        f.area(),
    );
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),                             // tabs
            Constraint::Length(u16::from(ui.tab != Tab::All)), // Provider time-filter bar
            Constraint::Min(3),                                // content
            Constraint::Length(1),                             // footer
        ])
        .split(f.area());

    draw_tabs(f, chunks[0], ui);
    if ui.tab != Tab::All {
        draw_filter_bar(f, chunks[1], ui);
    }

    if ui.detail.is_some() {
        draw_detail(f, chunks[2], snap, ui);
    } else {
        match ui.tab {
            Tab::All => draw_all(f, chunks[2], snap, ui),
            _ => draw_provider(f, chunks[2], snap, ui),
        }
    }

    draw_footer(f, chunks[3], snap, ui);

    if ui.help_open {
        draw_help(f, theme);
    }
}

fn draw_help(f: &mut Frame, theme: Theme) {
    let area = centered_rect(72, 62, f.area());
    let lines = vec![
        Line::from(Span::styled(
            "Pages",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  [1] All    [2] Claude Code    [3] Codex CLI"),
        Line::from(""),
        Line::from(Span::styled(
            "Controls",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  Tab / Shift+Tab  Focus where available"),
        Line::from("  Up/Down or j/k   Move selection"),
        Line::from("  Enter            Inspect"),
        Line::from("  f  Time range      t  Theme      r  Refresh"),
        Line::from("  ?  Help      Esc  Close      q  Quit"),
    ];
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(Span::styled(
                        " Help - Esc to close ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    )),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_tabs(f: &mut Frame, area: Rect, ui: &Ui) {
    let theme = ui.theme.palette();
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|t| Line::from(format!("[{}]", t.index() + 1)))
        .collect();
    let tabs = Tabs::new(titles)
        .divider(" ")
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    " TokenPlay ",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .select(ui.tab.index())
        .style(Style::default().fg(theme.foreground))
        .highlight_style(selected_style(theme));
    f.render_widget(tabs, area);
}

/// Segmented time-range selector, rendered as `Time  <ranges…>  (f to change)` with the
/// active range highlighted — so it's obvious `f` cycles the time range, not some
/// generic filter. Matches the segmented control shown in the product spec.
fn draw_filter_bar(f: &mut Frame, area: Rect, ui: &Ui) {
    let theme = ui.theme.palette();
    let mut spans = vec![Span::styled(" Time  ", Style::default().fg(theme.muted))];
    for (i, tf) in TimeFilter::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default().fg(theme.muted)));
        }
        let style = if *tf == ui.filter {
            selected_style(theme)
        } else {
            Style::default().fg(theme.foreground)
        };
        spans.push(Span::styled(format!(" {} ", tf.title()), style));
    }
    spans.push(Span::styled(
        "   (f to change)",
        Style::default().fg(theme.muted),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---- All page ----

fn draw_all(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let now_area = if area.width >= 120 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
            .split(area)[0]
    } else if area.height >= 14 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(14), Constraint::Min(0)])
            .split(area)[0]
    } else {
        area
    };
    let attention = current_attention(snap);
    let now = group_open_sessions(&snap.sessions, &attention);
    let now_ms = Local::now().timestamp_millis();
    let content_width = now_area.width.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    let mut selectable = 0usize;

    if now.is_empty() {
        lines.push(Line::from(Span::styled(
            if snap.ready {
                "No Open Sessions."
            } else {
                "Indexing... Open Sessions will appear shortly."
            },
            Style::default().fg(theme.muted),
        )));
    }

    for project in now {
        lines.push(Line::from(Span::styled(
            format!("PROJECT  {}", project.label()),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        for worktree in project.worktrees {
            lines.push(Line::from(Span::styled(
                now_worktree_line(&worktree, &attention, content_width),
                Style::default().fg(theme.muted),
            )));
            for session in worktree.sessions {
                lines.push(now_session_line(
                    session,
                    &attention,
                    now_ms,
                    content_width,
                    selectable == ui.selected,
                    theme,
                ));
                selectable += 1;
            }
        }
        for session in project.direct_sessions {
            lines.push(now_session_line(
                session,
                &attention,
                now_ms,
                content_width,
                selectable == ui.selected,
                theme,
            ));
            selectable += 1;
        }
    }

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent))
                .title(Span::styled(
                    " NOW ",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        now_area,
    );
}

struct NowProject<'a> {
    project: Option<&'a Project>,
    worktrees: Vec<NowWorktree<'a>>,
    direct_sessions: Vec<&'a Session>,
}

impl NowProject<'_> {
    fn label(&self) -> &str {
        self.project.map_or("N/A", |project| project.label.as_str())
    }
}

struct NowWorktree<'a> {
    worktree: &'a Worktree,
    sessions: Vec<&'a Session>,
}

fn current_attention(snap: &Snapshot) -> Cow<'_, AttentionSnapshot> {
    match snap.attention.as_ref() {
        Some(attention) => Cow::Borrowed(attention),
        None => {
            let observed_at_ms = if snap.generated_ms > 0 {
                snap.generated_ms
            } else {
                Local::now().timestamp_millis()
            };
            Cow::Owned(AttentionEngine::default().update(&snap.sessions, observed_at_ms))
        }
    }
}

fn group_open_sessions<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
) -> Vec<NowProject<'a>> {
    let mut projects: Vec<NowProject<'_>> = Vec::new();

    for session in sessions.iter().filter(|session| session.state.is_open()) {
        let project_index = session.project.as_ref().and_then(|project| {
            projects.iter().position(|group| {
                group
                    .project
                    .is_some_and(|existing| existing.identity == project.identity)
            })
        });
        let project_index = project_index.unwrap_or_else(|| {
            projects.push(NowProject {
                project: session.project.as_ref(),
                worktrees: Vec::new(),
                direct_sessions: Vec::new(),
            });
            projects.len() - 1
        });
        let project = &mut projects[project_index];

        if let Some(worktree) = session.worktree.as_ref() {
            let worktree_index = project
                .worktrees
                .iter()
                .position(|group| group.worktree.root == worktree.root)
                .unwrap_or_else(|| {
                    project.worktrees.push(NowWorktree {
                        worktree,
                        sessions: Vec::new(),
                    });
                    project.worktrees.len() - 1
                });
            project.worktrees[worktree_index].sessions.push(session);
        } else {
            project.direct_sessions.push(session);
        }
    }

    for project in &mut projects {
        for worktree in &mut project.worktrees {
            worktree
                .sessions
                .sort_by(|left, right| attention.compare_sessions(left, right));
        }
        project.worktrees.sort_by(|left, right| {
            compare_session_groups(attention, &left.sessions, &right.sessions)
                .then_with(|| left.worktree.root.cmp(&right.worktree.root))
        });
        project
            .direct_sessions
            .sort_by(|left, right| attention.compare_sessions(left, right));
    }
    projects.sort_by(|left, right| {
        compare_optional_sessions(
            attention,
            best_project_session(left, attention),
            best_project_session(right, attention),
        )
        .then_with(|| left.label().cmp(right.label()))
    });

    projects
}

fn best_project_session<'a>(
    project: &NowProject<'a>,
    attention: &AttentionSnapshot,
) -> Option<&'a Session> {
    project
        .worktrees
        .iter()
        .flat_map(|worktree| worktree.sessions.iter().copied())
        .chain(project.direct_sessions.iter().copied())
        .min_by(|left, right| attention.compare_sessions(left, right))
}

fn compare_session_groups(
    attention: &AttentionSnapshot,
    left: &[&Session],
    right: &[&Session],
) -> std::cmp::Ordering {
    compare_optional_sessions(attention, left.first().copied(), right.first().copied())
}

fn compare_optional_sessions(
    attention: &AttentionSnapshot,
    left: Option<&Session>,
    right: Option<&Session>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => attention.compare_sessions(left, right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn now_session_line(
    session: &Session,
    attention: &AttentionSnapshot,
    now_ms: i64,
    width: usize,
    selected: bool,
    theme: Theme,
) -> Line<'static> {
    let state = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => unreachable!("NOW contains only Open Sessions"),
    };
    let provider = match session.provider {
        Provider::ClaudeCode => "Claude",
        Provider::CodexCli => "Codex",
    };
    let model_width = width.saturating_sub(60).max(8);
    let model = shorten_start(
        session.latest_model.as_deref().unwrap_or("N/A"),
        model_width,
    );
    let attention_label = match attention.rank_for(session) {
        AttentionRank::CriticalContext => "CRITICAL",
        AttentionRank::SharedWorktreeRisk => "SHARED",
        AttentionRank::WarningContext => "WARNING",
        AttentionRank::Idle => "IDLE",
        AttentionRank::HealthyBusy => "BUSY",
    };
    let elapsed = attention
        .conditions_for(session)
        .iter()
        .find_map(|condition| match condition {
            AttentionCondition::Idle { since_ms, .. } => {
                Some(format!("Idle {}", fmt_relative(*since_ms, now_ms)))
            }
            _ => None,
        })
        .unwrap_or_else(|| fmt_relative(session.last_activity_ms, now_ms));
    let elapsed = shorten_start(&elapsed, 9);
    let row_style = if selected {
        selected_style(theme)
    } else {
        match attention.rank_for(session) {
            AttentionRank::CriticalContext => Style::default()
                .fg(theme.critical)
                .add_modifier(Modifier::BOLD),
            AttentionRank::SharedWorktreeRisk | AttentionRank::WarningContext => {
                Style::default().fg(theme.warning)
            }
            AttentionRank::Idle => Style::default().fg(theme.idle),
            AttentionRank::HealthyBusy if session.context.percent().is_none() => {
                Style::default().fg(theme.muted)
            }
            AttentionRank::HealthyBusy => Style::default().fg(theme.busy),
        }
    };
    let context_evidence = now_context_evidence(session);
    let context_style = if selected {
        row_style
    } else if session.context.percent().is_none() {
        Style::default().fg(theme.muted)
    } else {
        context_style(session, theme)
    };
    Line::from(vec![
        Span::styled(
            format!(
                "{} {:<8} {:<5} {:<6} ",
                if selected { ">" } else { " " },
                attention_label,
                state,
                provider,
            ),
            row_style,
        ),
        Span::styled(format!("{context_evidence:<17} "), context_style),
        Span::styled(
            format!("{model:<model_width$} {elapsed:<9} Inspect"),
            row_style,
        ),
    ])
}

fn now_worktree_line(
    group: &NowWorktree<'_>,
    attention: &AttentionSnapshot,
    width: usize,
) -> String {
    let branch = group.worktree.branch.as_deref().unwrap_or("detached HEAD");
    let context_pressure = highest_context_pressure(&group.sessions);
    let mut evidence = format!("Context {context_pressure}");
    if let Some(risk) = attention.shared_worktree_risk(&group.worktree.root) {
        evidence.push_str(&format!(
            " | WARNING Shared Worktree Risk x{}",
            risk.contributing_sessions.len()
        ));
    }
    let identity_width = width.saturating_sub(10 + evidence.chars().count());
    let path_width = identity_width.saturating_mul(3) / 5;
    let branch_width = identity_width.saturating_sub(path_width);
    format!(
        "  W {} @ {} | {}",
        shorten_end(&group.worktree.root.to_string_lossy(), path_width),
        shorten_start(branch, branch_width),
        evidence,
    )
}

fn shorten_start(value: &str, width: usize) -> String {
    shorten(value, width, false)
}

fn shorten_end(value: &str, width: usize) -> String {
    shorten(value, width, true)
}

fn shorten(value: &str, width: usize, keep_end: bool) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.len() <= width {
        return value.into();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let kept = width - 3;
    if keep_end {
        format!(
            "...{}",
            characters[characters.len() - kept..]
                .iter()
                .collect::<String>()
        )
    } else {
        format!("{}...", characters[..kept].iter().collect::<String>())
    }
}

fn now_context_evidence(session: &Session) -> String {
    match session.context.percent() {
        Some(percent) => {
            let filled = ((percent.clamp(0.0, 100.0) / 10.0).round() as usize).min(10);
            format!(
                "[{}{}] {:.0}%",
                "#".repeat(filled),
                "-".repeat(10 - filled),
                percent,
            )
        }
        None => "[----------] N/A".into(),
    }
}

fn highest_context_pressure(sessions: &[&Session]) -> String {
    sessions
        .iter()
        .filter_map(|session| session.context.percent())
        .max_by(f64::total_cmp)
        .map_or_else(|| "N/A".into(), |percent| format!("{percent:.0}%"))
}

// ---- Provider page ----

fn draw_provider(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let sessions = ui.visible(snap);
    let now = Local::now().timestamp_millis();
    let theme = ui.theme.palette();

    if sessions.is_empty() {
        let msg = if snap.ready {
            "No sessions match this filter."
        } else {
            "Indexing… sessions will appear shortly."
        };
        let p = Paragraph::new(msg)
            .style(Style::default().fg(theme.muted))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border)),
            );
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
    .style(
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = sessions
        .iter()
        .map(|s| session_row(s, now, theme))
        .collect();

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
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    format!(" {} ", ui.tab.title()),
                    Style::default().fg(theme.foreground),
                ))
                .title_bottom(
                    Line::from(Span::styled(
                        " showing full lifetime usage of matching sessions ",
                        Style::default().fg(theme.muted),
                    ))
                    .right_aligned(),
                ),
        )
        .row_highlight_style(selected_style(theme))
        .highlight_symbol("▌");

    let mut ts = TableState::default();
    ts.select(Some(ui.selected.min(sessions.len().saturating_sub(1))));
    f.render_stateful_widget(table, area, &mut ts);
}

fn state_cell(state: SessionState, theme: Theme) -> Cell<'static> {
    match state {
        SessionState::Busy => Cell::from(Span::styled(
            "● Busy",
            Style::default().fg(theme.busy).add_modifier(Modifier::BOLD),
        )),
        SessionState::Idle => Cell::from(Span::styled("○ Idle", Style::default().fg(theme.idle))),
        SessionState::Exited => Cell::from(Span::styled("· —", Style::default().fg(theme.exited))),
    }
}

fn context_style(s: &Session, theme: Theme) -> Style {
    match s.context.percent() {
        Some(p) if p >= 85.0 => Style::default()
            .fg(theme.critical)
            .add_modifier(Modifier::BOLD),
        Some(p) if p >= 70.0 => Style::default().fg(theme.warning),
        _ => Style::default().fg(theme.foreground),
    }
}

fn session_row(s: &Session, now: i64, theme: Theme) -> Row<'static> {
    let model = s.latest_model.clone().unwrap_or_else(|| "—".into());
    let cmp = if s.compaction_count() > 0 {
        format!("⤳{}", s.compaction_count())
    } else {
        "·".to_string()
    };
    let cmp_style = if s.compaction_count() > 0 {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.muted)
    };

    Row::new(vec![
        state_cell(s.state, theme),
        Cell::from(Span::styled(
            s.project_label().to_string(),
            Style::default().fg(theme.foreground),
        )),
        Cell::from(Span::styled(model, Style::default().fg(theme.muted))),
        Cell::from(Span::styled(fmt_context(s), context_style(s, theme))),
        Cell::from(Span::styled(
            fmt_cost(s.cost),
            Style::default().fg(theme.foreground),
        )),
        Cell::from(Span::styled(
            fmt_relative(s.last_activity_ms, now),
            Style::default().fg(theme.muted),
        )),
        Cell::from(Span::styled(cmp, cmp_style)),
    ])
}

// ---- detail child view ----

fn draw_detail(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let Some(s) = ui.detail_session(snap) else {
        return;
    };
    let theme = ui.theme.palette();

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
    lines.push(hdr("Session", s.id.clone()));
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

    lines.push(Line::from(Span::styled(
        "Transcript Lifetime Usage",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(hdr("Input", fmt_tokens(s.usage.input)));
    lines.push(hdr("Output", fmt_tokens(s.usage.output)));
    lines.push(hdr("Cache read", fmt_tokens(s.usage.cache_read)));
    lines.push(hdr("Cache write", fmt_tokens(s.usage.cache_write)));
    lines.push(hdr("Est. cost", fmt_cost(s.cost)));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "Per-model Usage",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    for m in &s.per_model {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<20}", m.model),
                Style::default().fg(theme.foreground),
            ),
            Span::styled(
                format!(
                    "in {} out {} cr {} cw {}  ",
                    fmt_tokens(m.usage.input),
                    fmt_tokens(m.usage.output),
                    fmt_tokens(m.usage.cache_read),
                    fmt_tokens(m.usage.cache_write)
                ),
                Style::default().fg(theme.muted),
            ),
            Span::styled(fmt_cost(m.cost), Style::default().fg(theme.foreground)),
        ]));
    }
    lines.push(Line::from(""));

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

// ---- footer ----

fn draw_footer(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let left = if ui.detail.is_some() && area.width < 100 {
        "Esc Back | j/k Scroll | 1/2/3 Page"
    } else if ui.detail.is_some() {
        "Esc Back | Up/Down or j/k Scroll | 1/2/3 Page | t Theme | ? Help | q Quit"
    } else if area.width < 100 && ui.tab == Tab::All {
        "j/k | Enter Inspect | 1/2/3 Page | t Theme | ? | q Quit"
    } else if area.width < 100 {
        "j/k | Enter | t Theme | ? | q Quit"
    } else if ui.tab == Tab::All {
        "j/k Move | Enter Inspect | 1/2/3 Page | r Scan | t Theme | ? Help | q Quit"
    } else {
        "j/k Move | Enter Detail | 1/2/3 Page | t Theme | ? Help | q Quit"
    };
    let theme_notice = ui
        .theme_notice_until
        .is_some_and(|until| Instant::now() < until);
    let status = if theme_notice {
        format!("Theme: {}", ui.active_theme_name())
    } else {
        snap.status.clone()
    };
    let right = if ui.tab == Tab::All && ui.detail.is_none() {
        status
    } else {
        format!("{status}   ·   Estimated using API rates")
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);
    f.render_widget(
        Paragraph::new(Span::styled(left, Style::default().fg(theme.muted))),
        cols[0],
    );
    let status_color = if theme_notice {
        theme.accent
    } else {
        theme.muted
    };
    f.render_widget(
        Paragraph::new(Span::styled(right, Style::default().fg(status_color)))
            .alignment(Alignment::Right),
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
    use crate::alerts::AlertCounts;
    use crate::attention::AttentionEngine;
    use crate::domain::{
        Compaction, ContextState, ModelUsage, Project, ProjectIdentity, Provider, Usage, Worktree,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_project(label: &str) -> Project {
        Project {
            identity: ProjectIdentity::CanonicalCwd(PathBuf::from(format!("/test/{label}"))),
            label: label.into(),
        }
    }

    fn unused_profile_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "tokenplay-{name}-{}-{nonce}-profile.json",
            std::process::id(),
        ))
    }

    fn render_terminal_with_snapshot(ui: &Ui, snapshot: &Snapshot) -> Terminal<TestBackend> {
        render_terminal_at(ui, snapshot, 80, 24)
    }

    fn render_terminal_at(
        ui: &Ui,
        snapshot: &Snapshot,
        width: u16,
        height: u16,
    ) -> Terminal<TestBackend> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, snapshot, ui)).unwrap();
        terminal
    }

    fn render_terminal(ui: &Ui) -> Terminal<TestBackend> {
        render_terminal_with_snapshot(ui, &Snapshot::default())
    }

    fn rendered(ui: &Ui) -> String {
        render_terminal(ui)
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn rendered_with_snapshot(ui: &Ui, snapshot: &Snapshot) -> String {
        render_terminal_with_snapshot(ui, snapshot)
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn rendered_rows_at(ui: &Ui, snapshot: &Snapshot, width: u16, height: u16) -> String {
        let terminal = render_terminal_at(ui, snapshot, width, height);
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row_text_start(buffer: &ratatui::buffer::Buffer, row: usize, text: &str) -> usize {
        let start = row * buffer.area.width as usize;
        let end = start + buffer.area.width as usize;
        let rendered_row: String = buffer.content()[start..end]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        rendered_row.find(text).unwrap()
    }

    fn rendered_style_at(terminal: &Terminal<TestBackend>, text: &str) -> Style {
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        let cell_count = text.chars().count();
        for row in buffer.content().chunks(width) {
            if let Some(cells) = row
                .windows(cell_count)
                .find(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == text)
            {
                return cells[0].style();
            }
        }
        panic!("rendered text not found: {text}");
    }

    fn demo_snapshot() -> Snapshot {
        let session = Session {
            provider: Provider::ClaudeCode,
            id: "demo-session".into(),
            project: Some(Project {
                identity: ProjectIdentity::GitCommonDir(PathBuf::from(
                    "/Users/x/code/tokenplay/.git",
                )),
                label: "tokenplay".into(),
            }),
            worktree: Some(Worktree {
                root: PathBuf::from("/Users/x/code/tokenplay"),
                branch: Some("main".into()),
                writable: true,
            }),
            cwd: Some("/Users/x/code/tokenplay".into()),
            latest_model: Some("claude-opus-4-8".into()),
            state: SessionState::Busy,
            context: ContextState {
                tokens: 150_000,
                window: Some(200_000),
            },
            usage: Usage {
                input: 1000,
                output: 500,
                cache_read: 200,
                cache_write: 100,
            },
            per_model: vec![ModelUsage {
                model: "claude-opus-4-8".into(),
                usage: Usage {
                    input: 1000,
                    output: 500,
                    cache_read: 200,
                    cache_write: 100,
                },
                cost: Some(0.42),
            }],
            compactions: vec![],
            last_activity_ms: 0,
            cost: Some(0.42),
        };
        Snapshot {
            sessions: vec![session],
            alerts: AlertCounts {
                warning: 1,
                critical: 0,
            },
            status: "watching".into(),
            ready: true,
            ..Default::default()
        }
    }

    fn render_tab(tab: Tab, detail: bool) {
        let snap = demo_snapshot();
        let mut ui = Ui {
            tab,
            filter: TimeFilter::All,
            selected: 0,
            ..Ui::default()
        };
        if detail {
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snap)
                .unwrap();
        }
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        // Must not panic while laying out or drawing.
        terminal.draw(|f| draw(f, &snap, &ui)).unwrap();
    }

    #[test]
    fn defaults_to_tokenplay_and_cycles_themes_in_the_confirmed_order() {
        let mut ui = Ui::default();
        let snapshot = Snapshot::default();

        assert_eq!(ui.active_theme_name(), "tokenplay");
        for expected in [
            "catppuccin",
            "dracula",
            "gruvbox",
            "light",
            "high-contrast",
            "tokenplay",
        ] {
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
            assert_eq!(ui.active_theme_name(), expected);
        }
    }

    #[test]
    fn cycling_theme_briefly_reports_the_new_name() {
        let mut ui = Ui::default();
        let snapshot = Snapshot {
            status: "watching".into(),
            ..Snapshot::default()
        };

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        assert!(rendered_with_snapshot(&ui, &snapshot).contains("Theme: catppuccin"));
    }

    #[test]
    fn each_theme_supplies_the_selected_page_structural_role() {
        let mut ui = Ui::default();
        let snapshot = Snapshot::default();
        let mut selected_backgrounds = Vec::new();

        for index in 0..6 {
            let terminal = render_terminal(&ui);
            let selected_page = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .find(|cell| cell.symbol() == "1")
                .unwrap();
            selected_backgrounds.push(selected_page.style().bg);

            if index < 5 {
                ui.handle_key(
                    KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                    &snapshot,
                )
                .unwrap();
            }
        }

        assert_eq!(
            selected_backgrounds,
            vec![
                Some(Color::Rgb(68, 61, 43)),
                Some(Color::Rgb(69, 71, 90)),
                Some(Color::Rgb(68, 71, 90)),
                Some(Color::Rgb(80, 73, 69)),
                Some(Color::Rgb(20, 84, 116)),
                Some(Color::Rgb(255, 255, 255)),
            ]
        );
    }

    #[test]
    fn tokenplay_uses_semantic_busy_and_critical_context_roles() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].context.tokens = 180_000;
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut selected = snapshot.sessions[0].clone();
        selected.id = "selected-session".into();
        selected.project = Some(test_project("selected-project"));
        selected.context.tokens = 50_000;
        selected.last_activity_ms = now - 1_000;
        snapshot.sessions.push(selected);
        let ui = Ui {
            tab: Tab::Claude,
            filter: TimeFilter::All,
            selected: 1,
            ..Ui::default()
        };

        let terminal = render_terminal_with_snapshot(&ui, &snapshot);

        assert_eq!(
            rendered_style_at(&terminal, "Busy").fg,
            Some(Color::Rgb(120, 205, 120))
        );
        assert_eq!(
            rendered_style_at(&terminal, "180.0k (90%)").fg,
            Some(Color::Rgb(245, 96, 96))
        );
    }

    #[test]
    fn tokenplay_uses_semantic_idle_exited_and_warning_roles() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        snapshot.sessions[0].context.tokens = 50_000;

        let mut idle = snapshot.sessions[0].clone();
        idle.id = "idle-session".into();
        idle.project = Some(test_project("idle-project"));
        idle.state = SessionState::Idle;
        idle.context.tokens = 150_000;
        idle.last_activity_ms = now - 1_000;

        let mut exited = snapshot.sessions[0].clone();
        exited.id = "exited-session".into();
        exited.project = Some(test_project("exited-project"));
        exited.state = SessionState::Exited;
        exited.context.tokens = 50_000;
        exited.last_activity_ms = now - 2_000;
        snapshot.sessions.extend([idle, exited]);

        let ui = Ui {
            tab: Tab::Claude,
            filter: TimeFilter::All,
            ..Ui::default()
        };
        let terminal = render_terminal_with_snapshot(&ui, &snapshot);

        assert_eq!(
            rendered_style_at(&terminal, "Idle").fg,
            Some(Color::Rgb(105, 184, 220))
        );
        assert_eq!(
            rendered_style_at(&terminal, "150.0k (75%)").fg,
            Some(Color::Rgb(238, 190, 70))
        );
        assert_eq!(
            rendered_style_at(&terminal, "· —").fg,
            Some(Color::Rgb(112, 116, 120))
        );
    }

    #[test]
    fn light_theme_supplies_background_border_accent_and_muted_roles() {
        let mut ui = Ui::default();
        let snapshot = Snapshot::default();
        for _ in 0..4 {
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
        }
        ui.tab = Tab::Claude;

        let terminal = render_terminal_with_snapshot(&ui, &snapshot);
        let top_left = &terminal.backend().buffer()[(0, 0)];

        assert_eq!(top_left.style().bg, Some(Color::Rgb(250, 249, 246)));
        assert_eq!(top_left.style().fg, Some(Color::Rgb(126, 130, 133)));
        assert_eq!(
            rendered_style_at(&terminal, "TokenPlay").fg,
            Some(Color::Rgb(25, 91, 130))
        );
        assert_eq!(
            rendered_style_at(&terminal, "Indexing… sessions will appear shortly.").fg,
            Some(Color::Rgb(96, 101, 106))
        );
    }

    #[test]
    fn now_groups_open_sessions_across_providers_and_omits_exited_sessions() {
        let ui = Ui::default();
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("claude-open-model".into());

        let mut codex = snapshot.sessions[0].clone();
        codex.provider = Provider::CodexCli;
        codex.id = "codex-open".into();
        codex.latest_model = Some("codex-open-model".into());
        codex.state = SessionState::Idle;

        let mut exited = snapshot.sessions[0].clone();
        exited.id = "exited-session".into();
        exited.latest_model = Some("exited-model".into());
        exited.state = SessionState::Exited;
        snapshot.sessions.extend([codex, exited]);

        let page = rendered_rows_at(&ui, &snapshot, 120, 30);
        let project = page.find("PROJECT  tokenplay").unwrap();
        let worktree = page.find("WARNING Shared Worktree Risk x2").unwrap();
        let claude = page.find("claude-open-model").unwrap();
        let codex = page.find("codex-open-model").unwrap();

        assert!(project < worktree && worktree < claude && claude < codex);
        assert_eq!(page.matches("PROJECT  tokenplay").count(), 1);
        let session_rows = page
            .lines()
            .filter(|line| line.contains("open-model"))
            .collect::<Vec<_>>();
        assert_eq!(session_rows.len(), 2);
        assert!(session_rows.iter().all(|line| line.contains("Inspect")));
        assert!(!page.contains("exited-model"));
    }

    #[test]
    fn now_applies_attention_order_to_projects_worktrees_and_sessions() {
        let mut healthy = demo_snapshot().sessions.remove(0);
        healthy.id = "healthy".into();
        healthy.project = Some(test_project("healthy"));
        healthy.worktree.as_mut().unwrap().root = PathBuf::from("/worktrees/healthy");
        healthy.latest_model = Some("healthy-model".into());
        healthy.context.tokens = 80_000;

        let mut warning = healthy.clone();
        warning.id = "warning".into();
        warning.project = Some(test_project("priority"));
        warning.worktree.as_mut().unwrap().root = PathBuf::from("/worktrees/a-warning");
        warning.latest_model = Some("warning-model".into());
        warning.context.tokens = 150_000;

        let mut shared = warning.clone();
        shared.provider = Provider::CodexCli;
        shared.id = "shared".into();
        shared.worktree.as_mut().unwrap().root = PathBuf::from("/worktrees/b-critical");
        shared.latest_model = Some("shared-model".into());
        shared.context.tokens = 80_000;

        let mut critical = shared.clone();
        critical.provider = Provider::ClaudeCode;
        critical.id = "critical".into();
        critical.latest_model = Some("critical-model".into());
        critical.state = SessionState::Idle;
        critical.context.tokens = 180_000;

        let mut snapshot = Snapshot {
            sessions: vec![healthy, warning, shared, critical],
            status: "watching".into(),
            ready: true,
            ..Snapshot::default()
        };
        snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let priority_project = page.find("PROJECT  priority").unwrap();
        let critical_worktree = page.find("WARNING Shared Worktree Risk x2").unwrap();
        let critical_session = page.find("critical-model").unwrap();
        let shared_session = page.find("shared-model").unwrap();
        let warning_worktree = page.find("/worktrees/a-warning").unwrap();
        let warning_session = page.find("warning-model").unwrap();
        let healthy_project = page.find("PROJECT  healthy").unwrap();
        let healthy_session = page.find("healthy-model").unwrap();

        assert!(priority_project < critical_worktree);
        assert!(critical_worktree < critical_session && critical_session < shared_session);
        assert!(shared_session < warning_worktree && warning_worktree < warning_session);
        assert!(warning_session < healthy_project && healthy_project < healthy_session);
        let priority_evidence = &page[priority_project..healthy_project];
        assert!(priority_evidence.contains("CRITICAL"));
        assert!(priority_evidence.contains("SHARED"));
        assert!(priority_evidence.contains("WARNING"));
        assert!(priority_evidence
            .lines()
            .find(|line| line.contains("critical-model"))
            .unwrap()
            .contains("Idle"));
    }

    #[test]
    fn now_shows_shared_worktree_risk_and_every_contributing_session() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].context.tokens = 80_000;
        snapshot.sessions[0].latest_model = Some("claude-member".into());
        let mut codex = snapshot.sessions[0].clone();
        codex.provider = Provider::CodexCli;
        codex.id = "codex-contributor".into();
        codex.latest_model = Some("codex-member".into());
        codex.state = SessionState::Idle;
        snapshot.sessions.push(codex);
        snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let risk_line = page
            .lines()
            .find(|line| line.contains("WARNING Shared Worktree Risk x2"))
            .unwrap();
        let risk = page.find(risk_line).unwrap();
        let claude = page.find("claude-member").unwrap();
        let codex = page.find("codex-member").unwrap();

        assert!(risk_line.contains("tokenplay"));
        assert!(risk_line.contains("@ main"));
        assert!(risk < claude && risk < codex);
        assert_eq!(
            page.lines()
                .filter(|line| line.contains("member") && line.contains("Inspect"))
                .count(),
            2
        );
    }

    #[test]
    fn now_shows_critical_context_label_bar_and_exact_percentage() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("critical-model".into());
        snapshot.sessions[0].context.tokens = 180_000;
        snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let critical = page
            .lines()
            .find(|line| line.contains("critical-model"))
            .unwrap();

        assert!(critical.contains("CRITICAL"));
        assert!(critical.contains("[#########-] 90%"));
        assert!(critical.contains("Busy"));
        assert!(critical.contains("Claude"));
        assert!(critical.contains("Inspect"));
    }

    #[test]
    fn now_shows_warning_context_label_bar_and_exact_percentage() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("warning-model".into());
        snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let warning = page
            .lines()
            .find(|line| line.contains("warning-model"))
            .unwrap();

        assert!(warning.contains("WARNING"));
        assert!(warning.contains("[########--] 75%"));
    }

    #[test]
    fn now_shows_idle_elapsed_from_transition_and_last_activity_fallback() {
        let now_ms = Local::now().timestamp_millis();
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].id = "observed-idle".into();
        snapshot.sessions[0].latest_model = Some("observed-idle".into());
        snapshot.sessions[0].context.tokens = 80_000;
        snapshot.sessions[0].last_activity_ms = now_ms - 20 * 60_000;
        let mut engine = AttentionEngine::default();
        engine.update(&snapshot.sessions, now_ms - 5 * 60_000);

        snapshot.sessions[0].state = SessionState::Idle;
        let mut fallback = snapshot.sessions[0].clone();
        fallback.provider = Provider::CodexCli;
        fallback.id = "fallback-idle".into();
        fallback.latest_model = Some("fallback-idle".into());
        fallback.project = Some(test_project("fallback"));
        fallback.worktree.as_mut().unwrap().root = PathBuf::from("/test/fallback");
        fallback.last_activity_ms = now_ms - 11 * 60_000;
        snapshot.sessions.push(fallback);
        snapshot.attention = Some(engine.update(&snapshot.sessions, now_ms - 4 * 60_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let observed = page
            .lines()
            .find(|line| line.contains("observed-idle"))
            .unwrap();
        let fallback = page
            .lines()
            .find(|line| line.contains("fallback-idle"))
            .unwrap();

        assert!(observed.contains("IDLE"));
        assert!(observed.contains("Idle 4m"));
        assert!(fallback.contains("IDLE"));
        assert!(fallback.contains("Idle 11m"));
    }

    #[test]
    fn all_page_selection_round_trips_through_session_detail() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("first-open-model".into());
        let mut second = snapshot.sessions[0].clone();
        second.provider = Provider::CodexCli;
        second.id = "second-open".into();
        second.latest_model = Some("second-open-model".into());
        snapshot.sessions.push(second);
        let mut ui = Ui::default();

        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(detail.contains("second-open"));
        assert!(!detail.contains("first-open-model"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let all = render_terminal_with_snapshot(&ui, &snapshot);
        assert_eq!(
            rendered_style_at(&all, "second-open-model").bg,
            Some(Color::Rgb(68, 61, 43))
        );
    }

    #[test]
    fn now_aligns_known_and_unknown_current_context_evidence() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("known-model".into());
        snapshot.sessions[0].context = ContextState {
            tokens: 140_000,
            window: Some(200_000),
        };
        let mut unknown = snapshot.sessions[0].clone();
        unknown.provider = Provider::CodexCli;
        unknown.id = "unknown-context".into();
        unknown.latest_model = Some("unknown-model".into());
        unknown.context = ContextState {
            tokens: 42_000,
            window: None,
        };
        snapshot.sessions.push(unknown);
        let ui = Ui::default();

        let terminal = render_terminal_at(&ui, &snapshot, 120, 30);
        let page = terminal
            .backend()
            .buffer()
            .content()
            .chunks(120)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>();
        let known = page
            .iter()
            .find(|line| line.contains("known-model"))
            .unwrap();
        let unknown = page
            .iter()
            .find(|line| line.contains("unknown-model"))
            .unwrap();

        assert!(known.contains("[#######---] 70%"));
        assert!(unknown.contains("[----------] N/A"));
        assert_eq!(known.find("known-model"), unknown.find("unknown-model"));
        assert_eq!(
            rendered_style_at(&terminal, "[----------] N/A").fg,
            Some(Color::Rgb(132, 136, 141))
        );
    }

    #[test]
    fn now_shows_the_worktrees_highest_current_context_pressure() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].worktree.as_mut().unwrap().branch = Some("feature/shared".into());
        snapshot.sessions[0].context.tokens = 80_000;
        let mut higher_pressure = snapshot.sessions[0].clone();
        higher_pressure.provider = Provider::CodexCli;
        higher_pressure.id = "higher-pressure".into();
        higher_pressure.context.tokens = 170_000;
        snapshot.sessions.push(higher_pressure);
        let ui = Ui::default();

        let page = rendered_rows_at(&ui, &snapshot, 120, 30);
        let worktree = page
            .lines()
            .find(|line| line.contains("WARNING Shared Worktree Risk x2"))
            .unwrap();

        assert!(worktree.contains("@ featur..."));
        assert!(worktree.contains("Context 85%"));
    }

    #[test]
    fn all_page_has_no_time_filter_or_usage_cost_surface() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].cost = Some(123.45);
        snapshot.sessions[0].usage = Usage {
            input: 987_654,
            output: 123_456,
            cache_read: 456_789,
            cache_write: 12_345,
        };
        let mut ui = Ui::default();
        let original_filter = ui.filter;

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        let page = rendered_rows_at(&ui, &snapshot, 120, 30);

        assert_eq!(ui.filter, original_filter);
        for excluded in [
            "Active Today",
            "Last 7 Days",
            "All Sessions",
            "Estimated using API rates",
            "$123",
            "987.7k",
        ] {
            assert!(
                !page.contains(excluded),
                "unexpected All-page text: {excluded}"
            );
        }
    }

    #[test]
    fn now_reserves_unlabelled_space_for_later_main_page_regions() {
        let snapshot = demo_snapshot();
        let ui = Ui::default();

        let wide = rendered_rows_at(&ui, &snapshot, 120, 30);
        let wide_top = wide.lines().find(|line| line.contains(" NOW ")).unwrap();
        assert_eq!(
            wide_top.chars().position(|character| character == '┐'),
            Some(78)
        );

        let compact = rendered_rows_at(&ui, &snapshot, 80, 24);
        assert_eq!(
            compact
                .lines()
                .enumerate()
                .find_map(|(index, line)| (index > 2 && line.starts_with('└')).then_some(index)),
            Some(16)
        );

        for speculative_label in ["RECENT SIGNALS", "FOCUS", "PROJECT PULSE"] {
            assert!(!wide.contains(speculative_label));
            assert!(!compact.contains(speculative_label));
        }
    }

    #[test]
    fn now_empty_state_explains_that_there_are_no_open_sessions() {
        let snapshot = Snapshot {
            ready: true,
            status: "watching".into(),
            ..Snapshot::default()
        };
        let ui = Ui::default();

        let page = rendered_rows_at(&ui, &snapshot, 80, 24);

        assert!(page.contains("No Open Sessions."));
        for manufactured_surface in ["Alert", "ATTENTION", "Warning", "Critical"] {
            assert!(!page.contains(manufactured_surface));
        }
    }

    #[test]
    fn now_keeps_healthy_busy_sessions_visible_without_manufacturing_attention() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("healthy-busy-model".into());
        snapshot.sessions[0].context.tokens = 80_000;
        let ui = Ui::default();

        let page = rendered_rows_at(&ui, &snapshot, 120, 30);
        let session = page
            .lines()
            .find(|line| line.contains("healthy-busy-m..."))
            .unwrap();

        assert!(session.contains("Busy"));
        assert!(session.contains("Claude"));
        assert!(session.contains("[####------] 40%"));
        assert!(session.contains("Inspect"));
        assert_eq!(page.matches("healthy-busy-m...").count(), 1);
        for manufactured_condition in ["Alert", "ATTENTION", "Warning", "Critical"] {
            assert!(!page.contains(manufactured_condition));
        }
    }

    #[test]
    fn compact_now_shortens_long_identity_before_operational_evidence() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].worktree = Some(Worktree {
            root: PathBuf::from("/Users/x/code/tokenplay-worktrees/attention-layout-prototype"),
            branch: Some("prototype/q-007-responsive-terminal-layout".into()),
            writable: true,
        });
        snapshot.sessions[0].latest_model =
            Some("claude-sonnet-with-an-intentionally-long-model-name".into());
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        let ui = Ui::default();

        let page = rendered_rows_at(&ui, &snapshot, 80, 24);
        let worktree = page
            .lines()
            .find(|line| line.contains("attention-layout-prototype"))
            .unwrap();
        let session = page
            .lines()
            .find(|line| line.contains("claude-sonnet"))
            .unwrap();

        assert!(worktree.contains("prototype/q-007"));
        assert!(worktree.contains("Context 75%"));
        for evidence in [
            "WARNING",
            "Busy",
            "Claude",
            "[########--] 75%",
            "0s",
            "Inspect",
        ] {
            assert!(
                session.contains(evidence),
                "missing compact evidence: {evidence}"
            );
        }
    }

    #[test]
    fn changing_theme_does_not_change_rendered_product_semantics() {
        let tokenplay_path = unused_profile_path("tokenplay-semantics");
        let light_path = unused_profile_path("light-semantics");
        std::fs::write(
            &tokenplay_path,
            br#"{"help_seen":true,"theme":"tokenplay"}"#,
        )
        .unwrap();
        std::fs::write(&light_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
        let mut tokenplay = Ui::load(&tokenplay_path, None).unwrap();
        let mut light = Ui::load(&light_path, None).unwrap();
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.alerts = AlertCounts {
            warning: 3,
            critical: 2,
        };

        assert_eq!(
            rendered_with_snapshot(&tokenplay, &snapshot),
            rendered_with_snapshot(&light, &snapshot)
        );

        tokenplay.tab = Tab::Claude;
        tokenplay.filter = TimeFilter::All;
        light.tab = Tab::Claude;
        light.filter = TimeFilter::All;
        assert_eq!(
            rendered_with_snapshot(&tokenplay, &snapshot),
            rendered_with_snapshot(&light, &snapshot)
        );

        let _ = std::fs::remove_file(tokenplay_path);
        let _ = std::fs::remove_file(light_path);
    }

    #[test]
    fn provider_inventory_and_detail_use_theme_structural_roles() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut secondary = snapshot.sessions[0].clone();
        secondary.id = "secondary-session".into();
        secondary.project = Some(test_project("second"));
        secondary.latest_model = Some("model-two".into());
        secondary.state = SessionState::Exited;
        secondary.last_activity_ms = now - 1_000;
        snapshot.sessions.push(secondary);
        let mut ui = Ui {
            tab: Tab::Claude,
            filter: TimeFilter::All,
            ..Ui::default()
        };
        for _ in 0..4 {
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
        }

        let inventory = render_terminal_with_snapshot(&ui, &snapshot);
        assert_eq!(
            rendered_style_at(&inventory, "State").fg,
            Some(Color::Rgb(25, 91, 130))
        );
        assert_eq!(
            rendered_style_at(&inventory, "second").fg,
            Some(Color::Rgb(28, 31, 34))
        );
        assert_eq!(
            rendered_style_at(&inventory, "model-two").fg,
            Some(Color::Rgb(96, 101, 106))
        );
        assert_eq!(
            inventory.backend().buffer()[(0, 4)].style().fg,
            Some(Color::Rgb(126, 130, 133))
        );

        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let detail = render_terminal_with_snapshot(&ui, &snapshot);
        assert_eq!(
            rendered_style_at(&detail, "Current Context").fg,
            Some(Color::Rgb(25, 91, 130))
        );
        assert_eq!(
            rendered_style_at(&detail, "Provider").fg,
            Some(Color::Rgb(96, 101, 106))
        );
        assert_eq!(
            rendered_style_at(&detail, "demo-session").fg,
            Some(Color::Rgb(28, 31, 34))
        );
    }

    #[test]
    fn help_overlay_uses_the_active_theme_roles() {
        let profile_path = unused_profile_path("themed-help");
        let ui = Ui::load(&profile_path, Some("light")).unwrap();

        let terminal = render_terminal(&ui);

        assert_eq!(
            rendered_style_at(&terminal, "Pages").fg,
            Some(Color::Rgb(25, 91, 130))
        );
        assert_eq!(
            rendered_style_at(&terminal, "[1] All").fg,
            Some(Color::Rgb(28, 31, 34))
        );
        assert_eq!(
            rendered_style_at(&terminal, "Help - Esc to close").fg,
            Some(Color::Rgb(25, 91, 130))
        );

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn filter_and_footer_use_theme_roles_and_expose_theme_cycling() {
        let mut ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
        let snapshot = Snapshot::default();
        for _ in 0..4 {
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
        }
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &snapshot, &ui)).unwrap();

        assert_eq!(
            rendered_style_at(&terminal, "Time").fg,
            Some(Color::Rgb(96, 101, 106))
        );
        assert_eq!(
            rendered_style_at(&terminal, "Active Today").bg,
            Some(Color::Rgb(20, 84, 116))
        );
        assert_eq!(
            rendered_style_at(&terminal, "t Theme").fg,
            Some(Color::Rgb(96, 101, 106))
        );
        assert_eq!(
            rendered_style_at(&terminal, "Theme: light").fg,
            Some(Color::Rgb(25, 91, 130))
        );
    }

    #[test]
    fn runtime_theme_selection_is_restored_from_the_profile() {
        let profile_path = unused_profile_path("persisted-theme");
        let mut ui = Ui::load(&profile_path, None).unwrap();

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            &Snapshot::default(),
        )
        .unwrap();

        let reloaded = Ui::load(&profile_path, None).unwrap();
        assert_eq!(reloaded.active_theme_name(), "catppuccin");

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn process_theme_override_takes_precedence_without_changing_the_profile() {
        let profile_path = unused_profile_path("theme-override");
        let mut saved = Ui::load(&profile_path, None).unwrap();
        saved
            .handle_key(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &Snapshot::default(),
            )
            .unwrap();

        let mut overridden = Ui::load(&profile_path, Some("dracula")).unwrap();
        assert_eq!(overridden.active_theme_name(), "dracula");
        overridden
            .handle_key(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &Snapshot::default(),
            )
            .unwrap();
        assert_eq!(overridden.active_theme_name(), "gruvbox");

        let reloaded = Ui::load(&profile_path, None).unwrap();
        assert_eq!(reloaded.active_theme_name(), "catppuccin");

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn unknown_process_theme_falls_back_without_changing_the_profile() {
        let profile_path = unused_profile_path("unknown-theme-override");
        let mut saved = Ui::load(&profile_path, None).unwrap();
        saved
            .handle_key(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &Snapshot::default(),
            )
            .unwrap();

        let overridden = Ui::load(&profile_path, Some("neon")).unwrap();
        assert_eq!(overridden.active_theme_name(), "tokenplay");

        let reloaded = Ui::load(&profile_path, None).unwrap();
        assert_eq!(reloaded.active_theme_name(), "catppuccin");

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn unknown_profile_theme_falls_back_to_tokenplay_without_blocking_startup() {
        let profile_path = unused_profile_path("unknown-profile-theme");
        std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"neon"}"#).unwrap();

        let ui = Ui::load(&profile_path, None).unwrap();

        assert_eq!(ui.active_theme_name(), "tokenplay");
        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn first_use_profile_opens_help() {
        let ui = Ui::load(unused_profile_path("first-use"), None).unwrap();

        assert!(rendered(&ui).contains("Help - Esc to close"));
    }

    #[test]
    fn dismissing_first_use_help_is_remembered_by_the_profile() {
        let profile_path = unused_profile_path("dismissed-help");
        let mut ui = Ui::load(&profile_path, None).unwrap();

        ui.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &Snapshot::default(),
        )
        .unwrap();

        let reloaded = Ui::load(&profile_path, None).unwrap();
        assert!(!rendered(&reloaded).contains("Help - Esc to close"));

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn deleting_help_preference_restores_first_use_help() {
        let profile_path = unused_profile_path("deleted-help-preference");
        let mut ui = Ui::load(&profile_path, None).unwrap();
        ui.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &Snapshot::default(),
        )
        .unwrap();
        std::fs::remove_file(&profile_path).unwrap();

        let reloaded = Ui::load(&profile_path, None).unwrap();

        assert!(rendered(&reloaded).contains("Help - Esc to close"));
    }

    #[test]
    fn help_can_be_reopened_and_closed_without_changing_the_active_page() {
        let profile_path = unused_profile_path("reopened-help");
        let mut ui = Ui::load(&profile_path, None).unwrap();
        let snapshot = Snapshot::default();
        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let page_before_help = rendered(&ui);

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        assert!(rendered(&ui).contains("Help - Esc to close"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert_eq!(rendered(&ui), page_before_help);

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn number_keys_always_open_their_assigned_page() {
        let mut ui = Ui::load(unused_profile_path("numeric-pages"), None).unwrap();
        let snapshot = Snapshot::default();

        for (key, expected) in [('2', Tab::Claude), ('3', Tab::Codex), ('1', Tab::All)] {
            ui.handle_key(
                KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
            assert_eq!(ui.active_tab(), expected);
        }
        assert!(rendered(&ui).contains("Help - Esc to close"));
    }

    #[test]
    fn tab_and_shift_tab_are_no_ops_on_a_single_region_page() {
        let mut ui = Ui::default();
        let snapshot = Snapshot::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        let page_before_focus_keys = rendered(&ui);

        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &snapshot,
        )
        .unwrap();

        assert_eq!(rendered(&ui), page_before_focus_keys);
    }

    #[test]
    fn persistent_navigation_is_numeric_and_highlights_the_active_page() {
        let snapshot = Snapshot::default();
        let mut ui = Ui::default();
        let terminal = render_terminal(&ui);
        let navigation_cells = &terminal.backend().buffer().content()[..80 * 3];
        let navigation: String = navigation_cells.iter().map(|cell| cell.symbol()).collect();

        for label in ["[1]", "[2]", "[3]"] {
            assert_eq!(navigation.matches(label).count(), 1);
        }
        for page_name in ["All", "Claude Code", "Codex CLI"] {
            assert!(!navigation.contains(page_name));
        }
        let first = navigation_cells
            .iter()
            .find(|cell| cell.symbol() == "1")
            .unwrap();
        let second = navigation_cells
            .iter()
            .find(|cell| cell.symbol() == "2")
            .unwrap();
        assert_eq!(first.style().bg, Some(Color::Rgb(68, 61, 43)));
        assert_ne!(second.style().bg, Some(Color::Rgb(68, 61, 43)));

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        let terminal = render_terminal(&ui);
        let navigation_cells = &terminal.backend().buffer().content()[..80 * 3];
        let second = navigation_cells
            .iter()
            .find(|cell| cell.symbol() == "2")
            .unwrap();
        assert_eq!(second.style().bg, Some(Color::Rgb(68, 61, 43)));
    }

    #[test]
    fn help_explains_page_mapping_and_essential_controls() {
        let ui = Ui::load(unused_profile_path("help-content"), None).unwrap();
        let help = rendered(&ui);

        for expected in [
            "[1] All",
            "[2] Claude Code",
            "[3] Codex CLI",
            "Tab / Shift+Tab",
            "Up/Down or j/k",
            "Enter",
            "t  Theme",
            "Refresh",
            "?",
            "Esc",
            "Quit",
        ] {
            assert!(help.contains(expected), "Help should contain {expected:?}");
        }
    }

    #[test]
    fn provider_session_detail_remains_keyboard_accessible() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session detail"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Session detail"));
    }

    #[test]
    fn global_keys_return_actions_for_the_event_loop() {
        let mut ui = Ui::default();
        let snapshot = Snapshot::default();

        assert_eq!(
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
                &snapshot
            )
            .unwrap(),
            UiAction::Rescan
        );
        assert_eq!(
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                &snapshot
            )
            .unwrap(),
            UiAction::Quit
        );
        assert_eq!(
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &snapshot,
            )
            .unwrap(),
            UiAction::Quit
        );
    }

    #[test]
    fn time_filter_key_advances_the_visible_filter() {
        let mut ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
        let snapshot = Snapshot::default();
        let terminal = render_terminal(&ui);
        let buffer = terminal.backend().buffer();
        let x = row_text_start(buffer, 3, "Last 7 Days");
        assert_ne!(
            buffer.content()[3 * 80 + x].style().bg,
            Some(Color::Rgb(68, 61, 43))
        );

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        let terminal = render_terminal(&ui);
        let buffer = terminal.backend().buffer();
        let x = row_text_start(buffer, 3, "Last 7 Days");
        assert_eq!(
            buffer.content()[3 * 80 + x].style().bg,
            Some(Color::Rgb(68, 61, 43))
        );
    }

    #[test]
    fn moving_selection_changes_which_session_opens_in_detail() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut second = snapshot.sessions[0].clone();
        second.id = "chosen-second-session".into();
        second.project = Some(test_project("second-project"));
        second.last_activity_ms = now - 1_000;
        snapshot.sessions.push(second);
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        assert!(rendered_with_snapshot(&ui, &snapshot).contains("chosen-second-session"));
    }

    #[test]
    fn detail_keeps_provider_and_native_session_identity_when_rows_reorder() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut chosen = snapshot.sessions[0].clone();
        chosen.id = "stable-detail-target".into();
        chosen.project = Some(test_project("chosen-project"));
        chosen.state = SessionState::Exited;
        chosen.last_activity_ms = now - 1_000;
        snapshot.sessions.push(chosen);
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        snapshot.sessions[0].state = SessionState::Exited;
        snapshot.sessions[1].state = SessionState::Busy;
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(detail.contains("stable-detail-target"));
        assert!(!detail.contains("demo-session"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let reopened = rendered_with_snapshot(&ui, &snapshot);
        assert!(reopened.contains("stable-detail-target"));
        assert!(!reopened.contains("demo-session"));
    }

    #[test]
    fn detail_replaces_the_content_area_and_keeps_numeric_navigation_visible() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let terminal = render_terminal_with_snapshot(&ui, &snapshot);
        let buffer = terminal.backend().buffer();
        let navigation: String = buffer.content()[..80 * 3]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(navigation.contains("[1]"));
        assert!(navigation.contains("[2]"));
        assert!(navigation.contains("[3]"));
        let content_top: String = buffer.content()[80 * 4..80 * 5]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content_top.contains("Session detail"));
        assert_eq!(buffer[(0, 4)].symbol(), "┌");
        let footer: String = buffer.content()[80 * 23..80 * 24]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(footer.contains("Esc Back"));
        assert!(footer.contains("j/k Scroll"));
    }

    #[test]
    fn escape_returns_to_the_originating_page_with_its_selection_restored() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut origin = snapshot.sessions[0].clone();
        origin.id = "origin-session".into();
        origin.project = Some(test_project("origin-project"));
        origin.last_activity_ms = now - 1_000;
        let mut third = snapshot.sessions[0].clone();
        third.id = "third-session".into();
        third.project = Some(test_project("third-project"));
        third.last_activity_ms = now - 2_000;
        snapshot.sessions.extend([origin, third]);
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(detail.contains("origin-session"));
        assert!(!detail.contains("third-session"));
    }

    #[test]
    fn number_keys_leave_detail_for_the_requested_top_level_page() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();

        for (key, expected_page) in [('1', Tab::All), ('2', Tab::Claude), ('3', Tab::Codex)] {
            let mut ui = Ui::default();
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
                .unwrap();
            assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session detail"));

            ui.handle_key(
                KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();

            assert_eq!(ui.active_tab(), expected_page);
            assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Session detail"));
        }
    }

    #[test]
    fn arrow_keys_scroll_detail_without_changing_the_origin_selection() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].id = format!("scroll-session-{}", "x".repeat(260));
        snapshot.sessions[0].project = Some(test_project(&"project".repeat(40)));
        snapshot.sessions[0].latest_model = Some("long-model".repeat(30));
        snapshot.sessions[0].compactions = (0..8)
            .map(|index| Compaction {
                at_ms: 1_700_000_000_000 + index * 1_000,
                trigger: Some(if index == 7 {
                    "scroll-target-trigger".into()
                } else {
                    format!("trigger-{index}")
                }),
                pre_tokens: Some(100_000 + index as u64),
            })
            .collect();
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        for _ in 0..100 {
            ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
                .unwrap();
        }
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        for _ in 0..100 {
            ui.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &snapshot)
                .unwrap();
        }
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        for _ in 0..100 {
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
        }
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        for _ in 0..100 {
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
        }
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("scroll-session-"));
    }

    #[test]
    fn detail_keeps_required_session_evidence_without_recent_signals() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].compactions = vec![Compaction {
            at_ms: 1_700_000_000_000,
            trigger: Some("manual".into()),
            pre_tokens: Some(146_000),
        }];
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &snapshot, &ui)).unwrap();
        let detail: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        for expected in [
            "Provider",
            "Claude Code",
            "Project",
            "tokenplay",
            "Worktree",
            "/Users/x/code/tokenplay",
            "Branch",
            "main",
            "cwd",
            "Session",
            "demo-session",
            "Latest model",
            "claude-opus-4-8",
            "State",
            "Busy",
            "Current Context",
            "150.0k / 200.0k (75.0%)",
            "Transcript Lifetime Usage",
            "Input",
            "Output",
            "Cache read",
            "Cache write",
            "Per-model Usage",
            "Est. cost",
            "$0.42",
            "Compactions (1)",
            "manual",
            "146.0k",
        ] {
            assert!(
                detail.contains(expected),
                "Detail should contain {expected:?}"
            );
        }
        assert!(!detail.contains("RECENT SIGNALS"));
        assert!(!detail.contains("Recent Signals"));
        assert!(!detail.contains("Recent Signal"));
    }

    #[test]
    fn non_git_detail_marks_git_only_fields_unavailable() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].project = Some(Project {
            identity: ProjectIdentity::CanonicalCwd(PathBuf::from(
                "/Users/x/Documents/research-notes",
            )),
            label: "research-notes".into(),
        });
        snapshot.sessions[0].worktree = None;
        snapshot.sessions[0].cwd = Some("/Users/x/Documents/research-notes".into());
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);

        assert!(detail.contains("research-notes"));
        assert!(detail.contains("N/A (non-Git Project)"));
    }

    #[test]
    fn unresolved_detail_preserves_cwd_and_marks_identity_unavailable() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].project = None;
        snapshot.sessions[0].worktree = None;
        snapshot.sessions[0].cwd = Some("/missing/session-cwd".into());
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);

        assert!(detail.contains("Project         N/A (identity unavailable)"));
        assert!(detail.contains("/missing/session-cwd"));
    }

    #[test]
    fn detached_worktree_detail_marks_only_the_branch_unavailable() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].worktree.as_mut().unwrap().branch = None;
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);

        assert!(detail.contains("Branch          N/A (detached HEAD)"));
        assert!(detail.contains("/Users/x/code/tokenplay"));
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
