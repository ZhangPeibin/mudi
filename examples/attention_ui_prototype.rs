//! PROTOTYPE - throw this file away after Q-007 is decided.
//!
//! Question: does the confirmed attention-first UI work at 120x30 and 80x24 without
//! overlap, incoherent truncation, or loss of required actions?
//!
//! Run interactively:
//!   cargo run --example attention_ui_prototype
//!
//! Generate review fixtures:
//!   cargo run --example attention_ui_prototype -- --render-fixtures

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::{Frame, Terminal};

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;
const FULL_WIDTH: u16 = 120;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    All,
    Claude,
    Codex,
}

impl Page {
    fn number(self) -> usize {
        match self {
            Self::All => 1,
            Self::Claude => 2,
            Self::Codex => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Focus {
    Now,
    Signals,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
        }
    }

    fn short(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionState {
    Busy,
    Idle,
    Exited,
}

impl SessionState {
    fn label(self) -> &'static str {
        match self {
            Self::Busy => "Busy",
            Self::Idle => "Idle",
            Self::Exited => "Exited",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Attention {
    Critical,
    SharedRisk,
    Warning,
    Idle,
    Healthy,
    Historical,
}

impl Attention {
    fn label(self) -> &'static str {
        match self {
            Self::Critical => "CRITICAL",
            Self::SharedRisk => "SHARED",
            Self::Warning => "WARNING",
            Self::Idle => "IDLE",
            Self::Healthy => "BUSY",
            Self::Historical => "EXITED",
        }
    }
}

#[derive(Clone, Copy)]
struct Usage {
    input: &'static str,
    output: &'static str,
    cache_read: &'static str,
    cache_create: &'static str,
    cost: &'static str,
}

#[derive(Clone)]
struct Session {
    id: &'static str,
    provider: Provider,
    project: &'static str,
    worktree: Option<&'static str>,
    cwd: &'static str,
    source_log: &'static str,
    branch: Option<&'static str>,
    state: SessionState,
    model: &'static str,
    context: Option<u8>,
    last_activity: &'static str,
    attention: Attention,
    compactions: u8,
    recent: bool,
    current_run: Option<Usage>,
    lifetime: Usage,
}

struct Signal {
    age: &'static str,
    severity: Attention,
    event: &'static str,
    scope: &'static str,
    action: &'static str,
    session: Option<usize>,
    worktree: Option<&'static str>,
}

struct Fixtures {
    sessions: Vec<Session>,
    signals: Vec<Signal>,
}

#[derive(Clone, Copy)]
struct Theme {
    name: &'static str,
    background: Color,
    foreground: Color,
    muted: Color,
    border: Color,
    accent: Color,
    selected_background: Color,
    selected_foreground: Color,
    critical: Color,
    warning: Color,
    idle: Color,
    busy: Color,
    exited: Color,
}

const THEMES: [Theme; 3] = [
    Theme {
        name: "tokenplay",
        background: Color::Rgb(20, 22, 24),
        foreground: Color::Rgb(224, 220, 210),
        muted: Color::Rgb(132, 136, 141),
        border: Color::Rgb(92, 99, 106),
        accent: Color::Rgb(230, 180, 80),
        selected_background: Color::Rgb(68, 61, 43),
        selected_foreground: Color::Rgb(255, 245, 218),
        critical: Color::Rgb(245, 96, 96),
        warning: Color::Rgb(238, 190, 70),
        idle: Color::Rgb(105, 184, 220),
        busy: Color::Rgb(120, 205, 120),
        exited: Color::Rgb(112, 116, 120),
    },
    Theme {
        name: "light",
        background: Color::Rgb(250, 249, 246),
        foreground: Color::Rgb(28, 31, 34),
        muted: Color::Rgb(96, 101, 106),
        border: Color::Rgb(126, 130, 133),
        accent: Color::Rgb(25, 91, 130),
        selected_background: Color::Rgb(20, 84, 116),
        selected_foreground: Color::Rgb(255, 255, 255),
        critical: Color::Rgb(176, 24, 32),
        warning: Color::Rgb(145, 92, 0),
        idle: Color::Rgb(24, 91, 142),
        busy: Color::Rgb(28, 112, 55),
        exited: Color::Rgb(105, 105, 105),
    },
    Theme {
        name: "high-contrast",
        background: Color::Rgb(0, 0, 0),
        foreground: Color::Rgb(255, 255, 255),
        muted: Color::Rgb(190, 190, 190),
        border: Color::Rgb(255, 255, 255),
        accent: Color::Rgb(0, 255, 255),
        selected_background: Color::Rgb(255, 255, 255),
        selected_foreground: Color::Rgb(0, 0, 0),
        critical: Color::Rgb(255, 90, 90),
        warning: Color::Rgb(255, 255, 0),
        idle: Color::Rgb(0, 255, 255),
        busy: Color::Rgb(80, 255, 80),
        exited: Color::Rgb(190, 190, 190),
    },
];

struct App {
    page: Page,
    focus: Focus,
    now_selected: usize,
    signal_selected: usize,
    claude_selected: usize,
    codex_selected: usize,
    expanded_older: HashSet<&'static str>,
    detail_session: Option<usize>,
    detail_scroll: usize,
    copy_menu: Option<usize>,
    theme: usize,
    status: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            page: Page::All,
            focus: Focus::Now,
            now_selected: 0,
            signal_selected: 0,
            claude_selected: 0,
            codex_selected: 0,
            expanded_older: HashSet::new(),
            detail_session: None,
            detail_scroll: 0,
            copy_menu: None,
            theme: 0,
            status: "PROTOTYPE - Q-007 layout validation".into(),
        }
    }
}

#[derive(Clone, Copy)]
enum ProviderTarget {
    Session(usize),
    Disclosure(&'static str),
}

struct ProviderLine {
    text: String,
    style: Style,
    target: Option<ProviderTarget>,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--render-fixtures") {
        let output = args
            .get(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("docs/prototype-attention/renders"));
        render_fixtures(&output)?;
        println!(
            "wrote attention UI prototype fixtures to {}",
            output.display()
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("--dump") {
        let scenario = args.get(1).map(String::as_str).unwrap_or("page1");
        let size = args.get(2).map(String::as_str).unwrap_or("80x24");
        let theme = args.get(3).map(String::as_str).unwrap_or("tokenplay");
        let (width, height) = parse_size(size)?;
        let app = scenario_app(scenario, theme)?;
        print!("{}", render_to_buffer(&fixtures(), &app, width, height)?.0);
        return Ok(());
    }
    if !args.is_empty() {
        bail!(
            "usage: attention_ui_prototype [--render-fixtures [DIR] | --dump SCENARIO SIZE THEME]"
        );
    }
    run_interactive()
}

fn run_interactive() -> Result<()> {
    let data = fixtures();
    let mut app = App::default();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|frame| draw(frame, &data, &app))?;
            if event::poll(Duration::from_millis(250))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press
                        && handle_key(&mut app, &data, key.code, key.modifiers)
                    {
                        break;
                    }
                }
            }
        }
        Ok(())
    })();

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn handle_key(app: &mut App, data: &Fixtures, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if app.copy_menu.is_some() {
        match code {
            KeyCode::Esc => app.copy_menu = None,
            KeyCode::Up | KeyCode::Char('k') => {
                app.copy_menu = Some(app.copy_menu.unwrap_or(0).saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.copy_menu = Some((app.copy_menu.unwrap_or(0) + 1).min(3));
            }
            KeyCode::Enter => {
                let label =
                    ["Source log", "Worktree", "cwd", "Session ID"][app.copy_menu.unwrap_or(0)];
                app.status = format!("Prototype: would copy complete {label}");
                app.copy_menu = None;
            }
            _ => {}
        }
        return false;
    }

    if app.detail_session.is_some() {
        match code {
            KeyCode::Esc => {
                app.detail_session = None;
                app.detail_scroll = 0;
                app.status = "Origin selection and disclosure state restored".into();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.detail_scroll = app.detail_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => app.detail_scroll += 1,
            KeyCode::Char('c') => app.copy_menu = Some(0),
            KeyCode::Char('1') => go_page(app, Page::All),
            KeyCode::Char('2') => go_page(app, Page::Claude),
            KeyCode::Char('3') => go_page(app, Page::Codex),
            KeyCode::Char('t') => cycle_theme(app),
            KeyCode::Char('q') => return true,
            _ => {}
        }
        return false;
    }

    match code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('1') => go_page(app, Page::All),
        KeyCode::Char('2') => go_page(app, Page::Claude),
        KeyCode::Char('3') => go_page(app, Page::Codex),
        KeyCode::Char('t') => cycle_theme(app),
        KeyCode::Tab => {
            if app.page == Page::All {
                app.focus = if app.focus == Focus::Now {
                    Focus::Signals
                } else {
                    Focus::Now
                };
            }
        }
        KeyCode::BackTab => {
            if app.page == Page::All {
                app.focus = if app.focus == Focus::Now {
                    Focus::Signals
                } else {
                    Focus::Now
                };
            }
        }
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, data, false),
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, data, true),
        KeyCode::Enter => activate_selection(app, data),
        KeyCode::Char('?') if !modifiers.contains(KeyModifiers::CONTROL) => {
            app.status = "1/2/3 pages | Tab focus | j/k move | Enter primary action".into();
        }
        _ => {}
    }
    false
}

fn go_page(app: &mut App, page: Page) {
    app.page = page;
    app.detail_session = None;
    app.copy_menu = None;
    app.detail_scroll = 0;
    app.status = format!("Page {}", page.number());
}

fn cycle_theme(app: &mut App) {
    app.theme = (app.theme + 1) % THEMES.len();
    app.status = format!("Theme: {}", THEMES[app.theme].name);
}

fn move_selection(app: &mut App, data: &Fixtures, down: bool) {
    let move_index = |current: &mut usize, len: usize| {
        if len == 0 {
            *current = 0;
        } else if down {
            *current = (*current + 1).min(len - 1);
        } else {
            *current = current.saturating_sub(1);
        }
    };

    match app.page {
        Page::All if app.focus == Focus::Now => {
            move_index(&mut app.now_selected, now_sessions().len())
        }
        Page::All => move_index(&mut app.signal_selected, data.signals.len()),
        Page::Claude => {
            let count = provider_lines(data, app, Provider::Claude, 120, &THEMES[app.theme])
                .iter()
                .filter(|line| line.target.is_some())
                .count();
            move_index(&mut app.claude_selected, count);
        }
        Page::Codex => {
            let count = provider_lines(data, app, Provider::Codex, 120, &THEMES[app.theme])
                .iter()
                .filter(|line| line.target.is_some())
                .count();
            move_index(&mut app.codex_selected, count);
        }
    }
}

fn activate_selection(app: &mut App, data: &Fixtures) {
    match app.page {
        Page::All if app.focus == Focus::Now => {
            if let Some(index) = now_sessions().get(app.now_selected) {
                app.detail_session = Some(*index);
                app.detail_scroll = 0;
            }
        }
        Page::All => {
            if let Some(signal) = data.signals.get(app.signal_selected) {
                if let Some(index) = signal.session {
                    app.detail_session = Some(index);
                    app.detail_scroll = 0;
                } else if let Some(worktree) = signal.worktree {
                    app.focus = Focus::Now;
                    app.now_selected = now_sessions()
                        .iter()
                        .position(|index| data.sessions[*index].worktree == Some(worktree))
                        .unwrap_or(app.now_selected);
                    app.status = "Located Shared Worktree Risk in NOW".into();
                }
            }
        }
        Page::Claude | Page::Codex => {
            let provider = if app.page == Page::Claude {
                Provider::Claude
            } else {
                Provider::Codex
            };
            let selected = if app.page == Page::Claude {
                app.claude_selected
            } else {
                app.codex_selected
            };
            let target = provider_lines(data, app, provider, 120, &THEMES[app.theme])
                .into_iter()
                .filter_map(|line| line.target)
                .nth(selected);
            match target {
                Some(ProviderTarget::Session(index)) => {
                    app.detail_session = Some(index);
                    app.detail_scroll = 0;
                }
                Some(ProviderTarget::Disclosure(project)) => {
                    if !app.expanded_older.remove(project) {
                        app.expanded_older.insert(project);
                    }
                }
                None => {}
            }
        }
    }
}

fn draw(frame: &mut Frame, data: &Fixtures, app: &App) {
    let area = frame.area();
    let theme = &THEMES[app.theme];
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background).fg(theme.foreground)),
        area,
    );

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(frame, area, theme);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(frame, rows[0], app, theme);

    if let Some(index) = app.detail_session {
        draw_detail(frame, rows[1], data, app, index, theme);
    } else {
        match app.page {
            Page::All => draw_all(frame, rows[1], data, app, theme),
            Page::Claude => draw_provider(frame, rows[1], data, app, Provider::Claude, theme),
            Page::Codex => draw_provider(frame, rows[1], data, app, Provider::Codex, theme),
        }
    }
    draw_footer(frame, rows[2], app, theme);

    if let (Some(index), Some(selected)) = (app.detail_session, app.copy_menu) {
        draw_copy_menu(frame, area, &data.sessions[index], selected, theme);
    }
}

fn draw_too_small(frame: &mut Frame, area: Rect, theme: &Theme) {
    let message = format!(
        "Terminal too small\n\nRequired: {MIN_WIDTH}x{MIN_HEIGHT}\nCurrent:  {}x{}\n\nq  Quit",
        area.width, area.height
    );
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.critical).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.critical))
                    .title(" TokenPlay | PROTOTYPE "),
            ),
        area,
    );
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let mut spans = vec![Span::styled(" ", Style::default())];
    for page in [Page::All, Page::Claude, Page::Codex] {
        let style = if app.page == page {
            Style::default()
                .fg(theme.selected_foreground)
                .bg(theme.selected_background)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        spans.push(Span::styled(format!(" [{}] ", page.number()), style));
        spans.push(Span::raw(" "));
    }
    let right = format!("Theme: {} ", theme.name);
    let available = area.width.saturating_sub(2) as usize;
    let left_width = 20usize;
    spans.push(Span::styled(
        format!(
            "{:>width$}",
            right,
            width = available.saturating_sub(left_width)
        ),
        Style::default().fg(theme.muted),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    " TokenPlay | PROTOTYPE ",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn draw_all(frame: &mut Frame, area: Rect, data: &Fixtures, app: &App, theme: &Theme) {
    if area.width >= FULL_WIDTH {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
            .split(area);
        draw_now(frame, cols[0], data, app, false, theme);
        draw_signals(frame, cols[1], data, app, false, theme);
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(14), Constraint::Min(6)])
            .split(area);
        draw_now(frame, rows[0], data, app, true, theme);
        draw_signals(frame, rows[1], data, app, true, theme);
    }
}

fn draw_now(
    frame: &mut Frame,
    area: Rect,
    data: &Fixtures,
    app: &App,
    compact: bool,
    theme: &Theme,
) {
    let focused = app.focus == Focus::Now;
    let border = if focused { theme.accent } else { theme.border };
    let title = if focused { " NOW | FOCUSED " } else { " NOW " };
    let width = area.width.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    let mut selectable = 0usize;

    for group in now_groups() {
        lines.push(Line::from(Span::styled(
            fit(&format!("PROJECT  {}", group.project), width),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        for worktree in group.worktrees {
            let members = worktree.sessions;
            let risk = members.len() > 1;
            let label = if compact {
                format!(
                    "  W {} @ {}{}",
                    short_path(worktree.path),
                    worktree.branch,
                    if risk { " [WARN Shared x2]" } else { "" }
                )
            } else {
                let suffix = if risk {
                    " [WARNING Shared Worktree Risk x2]"
                } else {
                    ""
                };
                let prefix_width = width.saturating_sub(suffix.len());
                format!(
                    "{}{}",
                    fit(
                        &format!("  W {} @ {}", short_path(worktree.path), worktree.branch),
                        prefix_width
                    ),
                    suffix
                )
            };
            lines.push(Line::from(Span::styled(
                fit(&label, width),
                Style::default().fg(if risk { theme.warning } else { theme.muted }),
            )));
            for index in members {
                let selected = selectable == app.now_selected;
                lines.push(session_line(
                    &data.sessions[*index],
                    width,
                    compact,
                    selected,
                    theme,
                ));
                selectable += 1;
            }
        }
        for index in group.direct_sessions {
            let selected = selectable == app.now_selected;
            lines.push(session_line(
                &data.sessions[*index],
                width,
                compact,
                selected,
                theme,
            ));
            selectable += 1;
        }
    }

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border))
                .title(Span::styled(
                    title,
                    Style::default().fg(border).add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn session_line(
    session: &Session,
    width: usize,
    compact: bool,
    selected: bool,
    theme: &Theme,
) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let context = context_text(session.context, !compact);
    let text = if compact {
        format!(
            "{} {:<8} {:<5} {:<6} {:<4}  Inspect",
            marker,
            session.attention.label(),
            session.state.label(),
            session.provider.short(),
            context_percent(session.context)
        )
    } else {
        let fixed = 2 + 10 + 6 + 8 + 18 + 6 + 9;
        let model_width = width.saturating_sub(fixed).max(8);
        format!(
            "{} {:<8} {:<5} {:<7} {:<17} {:<model_width$} {:<5} Inspect",
            marker,
            session.attention.label(),
            session.state.label(),
            session.provider.short(),
            context,
            fit(session.model, model_width),
            session.last_activity,
        )
    };
    let mut style = Style::default().fg(attention_color(session.attention, theme));
    if selected {
        style = style
            .fg(theme.selected_foreground)
            .bg(theme.selected_background)
            .add_modifier(Modifier::BOLD);
    }
    Line::from(Span::styled(pad_fit(&text, width), style))
}

fn draw_signals(
    frame: &mut Frame,
    area: Rect,
    data: &Fixtures,
    app: &App,
    compact: bool,
    theme: &Theme,
) {
    let focused = app.focus == Focus::Signals;
    let border = if focused { theme.accent } else { theme.border };
    let title = if focused {
        " RECENT SIGNALS | FOCUSED "
    } else {
        " RECENT SIGNALS "
    };
    let width = area.width.saturating_sub(2) as usize;
    let visible = if compact { 4 } else { 5 };
    let mut lines = Vec::new();

    for (index, signal) in data.signals.iter().take(visible).enumerate() {
        let selected = index == app.signal_selected;
        let marker = if selected { ">" } else { " " };
        let style = if selected {
            Style::default()
                .fg(theme.selected_foreground)
                .bg(theme.selected_background)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(attention_color(signal.severity, theme))
        };
        if compact {
            let text = format!(
                "{} {} {:<8} {} [{}] - {}",
                marker,
                signal.age,
                signal.severity.label(),
                signal.event,
                signal.action,
                signal.scope
            );
            lines.push(Line::from(Span::styled(pad_fit(&text, width), style)));
        } else {
            lines.push(Line::from(Span::styled(
                pad_fit(
                    &format!("{} {}  {}", marker, signal.age, signal.severity.label()),
                    width,
                ),
                style,
            )));
            lines.push(Line::from(Span::styled(
                fit(&format!("  {}", signal.event), width),
                Style::default().fg(theme.foreground),
            )));
            lines.push(Line::from(Span::styled(
                fit(&format!("  {}  {}", signal.scope, signal.action), width),
                Style::default().fg(theme.muted),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border))
                .title(Span::styled(
                    title,
                    Style::default().fg(border).add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn draw_provider(
    frame: &mut Frame,
    area: Rect,
    data: &Fixtures,
    app: &App,
    provider: Provider,
    theme: &Theme,
) {
    let compact = area.width < FULL_WIDTH;
    let width = area.width.saturating_sub(2) as usize;
    let mut rendered = Vec::new();
    let selected = if provider == Provider::Claude {
        app.claude_selected
    } else {
        app.codex_selected
    };
    let mut selectable = 0usize;

    let header = if compact {
        "  State  Session             Ctx    Cost  Cmp  Action"
    } else {
        "  State   Session                    Model                    Context            Lifetime $  Updated  Cmp  Action"
    };
    rendered.push(Line::from(Span::styled(
        fit(header, width),
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::BOLD),
    )));

    for line in provider_lines(data, app, provider, width, theme) {
        if line.target.is_some() {
            let is_selected = selectable == selected;
            let marker = if is_selected { ">" } else { " " };
            let style = if is_selected {
                Style::default()
                    .fg(theme.selected_foreground)
                    .bg(theme.selected_background)
                    .add_modifier(Modifier::BOLD)
            } else {
                line.style
            };
            rendered.push(Line::from(Span::styled(
                pad_fit(&format!("{}{}", marker, line.text), width),
                style,
            )));
            selectable += 1;
        } else {
            rendered.push(Line::from(Span::styled(fit(&line.text, width), line.style)));
        }
    }

    let title = format!(
        " {} SESSIONS | grouped by Project ",
        provider.label().to_uppercase()
    );
    frame.render_widget(
        Paragraph::new(rendered).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn provider_lines(
    data: &Fixtures,
    app: &App,
    provider: Provider,
    width: usize,
    theme: &Theme,
) -> Vec<ProviderLine> {
    let mut lines = Vec::new();
    for project in provider_projects(provider) {
        let sessions: Vec<usize> = data
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| session.provider == provider && session.project == *project)
            .map(|(index, _)| index)
            .collect();
        if sessions.is_empty() {
            continue;
        }
        lines.push(ProviderLine {
            text: format!("PROJECT  {project}"),
            style: Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            target: None,
        });
        for index in sessions
            .iter()
            .filter(|index| data.sessions[**index].recent)
        {
            lines.push(provider_session_line(
                &data.sessions[*index],
                *index,
                width,
                theme,
            ));
        }
        let older: Vec<usize> = sessions
            .iter()
            .copied()
            .filter(|index| !data.sessions[*index].recent)
            .collect();
        if !older.is_empty() {
            let expanded = app.expanded_older.contains(project);
            lines.push(ProviderLine {
                text: format!(
                    "  {} OLDER SESSIONS ({})  {}",
                    if expanded { "-" } else { "+" },
                    older.len(),
                    if expanded { "Collapse" } else { "Expand" }
                ),
                style: Style::default().fg(theme.muted),
                target: Some(ProviderTarget::Disclosure(project)),
            });
            if expanded {
                for index in older {
                    lines.push(provider_session_line(
                        &data.sessions[index],
                        index,
                        width,
                        theme,
                    ));
                }
            }
        }
    }
    lines
}

fn provider_session_line(
    session: &Session,
    index: usize,
    width: usize,
    theme: &Theme,
) -> ProviderLine {
    let compact = width < FULL_WIDTH as usize - 2;
    let id = short_id(session.id);
    let text = if compact {
        format!(
            " {:<6} {:<19} {:<5} {:<6} C{:<2} Inspect",
            session.state.label(),
            tail_fit(id, 19),
            context_percent(session.context),
            session.lifetime.cost,
            session.compactions
        )
    } else {
        format!(
            " {:<7} {:<26} {:<24} {:<18} {:<11} {:<8} C{:<2} Inspect",
            session.state.label(),
            fit(id, 26),
            fit(session.model, 24),
            context_text(session.context, true),
            session.lifetime.cost,
            session.last_activity,
            session.compactions
        )
    };
    ProviderLine {
        text,
        style: Style::default().fg(if session.state == SessionState::Exited {
            theme.exited
        } else {
            attention_color(session.attention, theme)
        }),
        target: Some(ProviderTarget::Session(index)),
    }
}

fn draw_detail(
    frame: &mut Frame,
    area: Rect,
    data: &Fixtures,
    app: &App,
    index: usize,
    theme: &Theme,
) {
    let session = &data.sessions[index];
    let width = area.width.saturating_sub(2) as usize;
    let compact = width < 90;
    let mut lines = Vec::new();
    let label_style = Style::default().fg(theme.muted);
    let value_style = Style::default().fg(theme.foreground);

    if compact {
        lines.push(detail_pair(
            "Provider",
            &format!("{} / {}", session.provider.label(), session.state.label()),
            width,
            label_style,
            value_style,
        ));
    } else {
        lines.push(detail_pair(
            "Provider",
            session.provider.label(),
            width,
            label_style,
            value_style,
        ));
        lines.push(detail_pair(
            "State",
            session.state.label(),
            width,
            label_style,
            value_style,
        ));
    }
    lines.push(detail_pair(
        "Project",
        session.project,
        width,
        label_style,
        value_style,
    ));
    lines.push(detail_pair(
        "Session",
        session.id,
        width,
        label_style,
        value_style,
    ));
    lines.push(detail_pair(
        "Model",
        session.model,
        width,
        label_style,
        value_style,
    ));
    if !compact {
        lines.push(detail_pair(
            "Branch",
            session.branch.unwrap_or("N/A"),
            width,
            label_style,
            value_style,
        ));
    }
    let context = format!(
        "{} | {}",
        context_text(session.context, true),
        match session.attention {
            Attention::Critical => "CRITICAL Context >=85%",
            Attention::Warning => "WARNING Context >=70%",
            Attention::SharedRisk => "WARNING Shared Worktree Risk",
            Attention::Idle => "neutral Idle Attention",
            Attention::Healthy => "healthy Busy",
            Attention::Historical => "historical; no current alert",
        }
    );
    lines.push(detail_pair(
        "Context",
        &context,
        width,
        label_style,
        attention_style(session.attention, theme),
    ));
    if !compact {
        lines.push(Line::raw(""));
    }

    if let Some(run) = session.current_run {
        lines.push(section_line("CURRENT RUN USAGE", theme));
        lines.push(usage_line(run, width, theme));
    }
    lines.push(section_line("TRANSCRIPT LIFETIME USAGE", theme));
    lines.push(usage_line(session.lifetime, width, theme));
    if !compact {
        lines.push(Line::raw(""));
    }

    lines.push(section_line("SOURCE METADATA", theme));
    if let Some(worktree) = session.worktree {
        push_wrapped_field(&mut lines, "Worktree", worktree, width, theme);
    } else {
        lines.push(detail_pair(
            "Worktree",
            "N/A (non-Git Project)",
            width,
            label_style,
            value_style,
        ));
    }
    push_wrapped_field(&mut lines, "cwd", session.cwd, width, theme);
    push_wrapped_field(&mut lines, "Source log", session.source_log, width, theme);
    lines.push(detail_pair(
        "Format",
        "JSONL",
        width,
        label_style,
        value_style,
    ));
    lines.push(Line::raw(""));

    lines.push(section_line(
        &format!("COMPACTIONS ({})", session.compactions),
        theme,
    ));
    if session.compactions == 0 {
        lines.push(Line::from(Span::styled(
            "none observed",
            Style::default().fg(theme.muted),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "2026-07-30 09:42  auto  pre=181.4k | Context Critical cleared",
            Style::default().fg(theme.foreground),
        )));
        lines.push(Line::from(Span::styled(
            "2026-07-29 18:17  manual  pre=146.0k",
            Style::default().fg(theme.foreground),
        )));
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let total_lines = lines.len();
    let max_scroll = total_lines.saturating_sub(inner_height);
    let scroll = app.detail_scroll.min(max_scroll);
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(inner_height).collect();
    let title = format!(
        " SESSION DETAIL | {} | {} ",
        session.provider.label(),
        if session.state == SessionState::Exited {
            "EXITED"
        } else {
            "OPEN"
        }
    );
    frame.render_widget(
        Paragraph::new(visible).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Span::styled(
                    format!(
                        " lines {}-{} of {} ",
                        scroll + 1,
                        (scroll + inner_height).min(total_lines),
                        total_lines
                    ),
                    Style::default().fg(theme.muted),
                )),
        ),
        area,
    );
}

fn detail_pair(
    label: &str,
    value: &str,
    width: usize,
    label_style: Style,
    value_style: Style,
) -> Line<'static> {
    let label_width = if width < 90 { 11 } else { 18 };
    let value_width = width.saturating_sub(label_width);
    Line::from(vec![
        Span::styled(format!("{label:<label_width$}"), label_style),
        Span::styled(fit(value, value_width), value_style),
    ])
}

fn section_line(title: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    ))
}

fn usage_line(usage: Usage, width: usize, theme: &Theme) -> Line<'static> {
    let text = format!(
        "in {} | out {} | cache read {} | cache create {} | API est. {}",
        usage.input, usage.output, usage.cache_read, usage.cache_create, usage.cost
    );
    Line::from(Span::styled(
        fit(&text, width),
        Style::default().fg(theme.foreground),
    ))
}

fn push_wrapped_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    width: usize,
    theme: &Theme,
) {
    let label_width = if width < 90 { 11 } else { 18 };
    let first_width = width.saturating_sub(label_width).max(1);
    let chunks = wrap_chunks(value, first_width);
    for (index, chunk) in chunks.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                if index == 0 {
                    format!("{label:<label_width$}")
                } else {
                    " ".repeat(label_width)
                },
                Style::default().fg(theme.muted),
            ),
            Span::styled(chunk.clone(), Style::default().fg(theme.foreground)),
        ]));
    }
}

fn draw_copy_menu(
    frame: &mut Frame,
    area: Rect,
    session: &Session,
    selected: usize,
    theme: &Theme,
) {
    let popup = centered_rect(86, 11, area);
    frame.render_widget(Clear, popup);
    let fields = [
        ("Source log", session.source_log),
        ("Worktree", session.worktree.unwrap_or("N/A")),
        ("cwd", session.cwd),
        ("Session ID", session.id),
    ];
    let width = popup.width.saturating_sub(2) as usize;
    let mut lines = vec![Line::from(Span::styled(
        "Choose one complete metadata value",
        Style::default().fg(theme.muted),
    ))];
    for (index, (label, value)) in fields.iter().enumerate() {
        let text = format!(
            "{} {:<12} {}",
            if index == selected { ">" } else { " " },
            label,
            value
        );
        let style = if index == selected {
            Style::default()
                .fg(theme.selected_foreground)
                .bg(theme.selected_background)
                .add_modifier(Modifier::BOLD)
        } else if *value == "N/A" {
            Style::default().fg(theme.exited)
        } else {
            Style::default().fg(theme.foreground)
        };
        lines.push(Line::from(Span::styled(pad_fit(&text, width), style)));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Enter Copy | Esc Cancel | clipboard integration intentionally stubbed",
        Style::default().fg(theme.muted),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(Span::styled(
                        " COPY SOURCE METADATA ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    )),
            ),
        popup,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let keys = if app.detail_session.is_some() {
        " Esc Back | j/k Scroll | c Copy | 1/2/3 Page | t Theme | q Quit"
    } else if app.page == Page::All {
        " Enter Inspect | Tab Focus | j/k Move | 1/2/3 Page | t Theme | q Quit"
    } else {
        " Enter Inspect/Expand | j/k Move | 1/2/3 Page | t Theme | q Quit"
    };
    let status_room = area.width.saturating_sub(keys.len() as u16 + 3) as usize;
    let status = if area.width >= 100 && status_room >= 12 {
        format!(" | {}", fit(&app.status, status_room.saturating_sub(3)))
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(keys, Style::default().fg(theme.muted)),
            Span::styled(
                format!("{:>width$}", status, width = status_room),
                Style::default().fg(theme.accent),
            ),
        ])),
        area,
    );
}

fn render_fixtures(output: &Path) -> Result<()> {
    fs::create_dir_all(output).with_context(|| format!("create {}", output.display()))?;
    let data = fixtures();
    let specs = [
        ("page1_120x30_tokenplay", "page1", 120, 30, "tokenplay"),
        ("page1_80x24_tokenplay", "page1", 80, 24, "tokenplay"),
        ("claude_120x30", "claude", 120, 30, "tokenplay"),
        ("claude_80x24", "claude", 80, 24, "tokenplay"),
        (
            "claude_older_expanded_80x24",
            "claude-expanded",
            80,
            24,
            "tokenplay",
        ),
        ("codex_120x30", "codex", 120, 30, "tokenplay"),
        ("codex_80x24", "codex", 80, 24, "tokenplay"),
        ("detail_open_120x30", "detail-open", 120, 30, "tokenplay"),
        ("detail_open_80x24", "detail-open", 80, 24, "tokenplay"),
        (
            "detail_exited_120x30",
            "detail-exited",
            120,
            30,
            "tokenplay",
        ),
        ("detail_exited_80x24", "detail-exited", 80, 24, "tokenplay"),
        ("copy_menu_80x24", "copy-menu", 80, 24, "tokenplay"),
        ("page1_120x30_light", "page1", 120, 30, "light"),
        (
            "page1_120x30_high_contrast",
            "page1",
            120,
            30,
            "high-contrast",
        ),
        ("too_small_79x23", "page1", 79, 23, "tokenplay"),
    ];

    let mut index = String::from(
        "# Attention UI prototype render fixtures\n\nPROTOTYPE output for Q-007. Plain `.txt` files preserve terminal geometry; `.ansi` files preserve semantic Theme colors and can be viewed with `less -R`.\n\n",
    );
    for (name, scenario, width, height, theme_name) in specs {
        let app = scenario_app(scenario, theme_name)?;
        let (plain, ansi) = render_to_buffer(&data, &app, width, height)?;
        validate_render(name, &plain, width, height)?;
        fs::write(output.join(format!("{name}.txt")), plain)?;
        fs::write(output.join(format!("{name}.ansi")), ansi)?;
        index.push_str(&format!("- `{name}.txt` / `{name}.ansi`\n"));
    }
    validate_themes()?;
    validate_interactions(&data)?;
    fs::write(output.join("README.md"), index)?;
    Ok(())
}

fn scenario_app(scenario: &str, theme_name: &str) -> Result<App> {
    let mut app = App::default();
    app.theme = THEMES
        .iter()
        .position(|theme| theme.name == theme_name)
        .with_context(|| format!("unknown theme {theme_name}"))?;
    app.status = format!("Theme: {theme_name}");
    match scenario {
        "page1" => {}
        "claude" => app.page = Page::Claude,
        "claude-expanded" => {
            app.page = Page::Claude;
            app.expanded_older.insert("tokenplay");
        }
        "codex" => app.page = Page::Codex,
        "detail-open" => {
            app.page = Page::All;
            app.detail_session = Some(0);
        }
        "detail-exited" => {
            app.page = Page::Claude;
            app.detail_session = Some(5);
        }
        "copy-menu" => {
            app.page = Page::All;
            app.detail_session = Some(0);
            app.copy_menu = Some(0);
        }
        _ => bail!("unknown scenario {scenario}"),
    }
    Ok(app)
}

fn render_to_buffer(
    data: &Fixtures,
    app: &App,
    width: u16,
    height: u16,
) -> Result<(String, String)> {
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|frame| draw(frame, data, app))?;
    let buffer = terminal.backend().buffer();
    Ok((plain_buffer(buffer), ansi_buffer(buffer)))
}

fn plain_buffer(buffer: &Buffer) -> String {
    let mut output = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            output.push_str(buffer[(x, y)].symbol());
        }
        output.push('\n');
    }
    output
}

fn ansi_buffer(buffer: &Buffer) -> String {
    let mut output = String::new();
    for y in 0..buffer.area.height {
        let mut previous: Option<(Color, Color, Modifier)> = None;
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            let style = (cell.fg, cell.bg, cell.modifier);
            if previous != Some(style) {
                output.push_str("\x1b[0m");
                output.push_str(&ansi_style(cell));
                previous = Some(style);
            }
            output.push_str(cell.symbol());
        }
        output.push_str("\x1b[0m\n");
    }
    output
}

fn ansi_style(cell: &Cell) -> String {
    let mut codes = vec![ansi_color(cell.fg, true), ansi_color(cell.bg, false)];
    if cell.modifier.contains(Modifier::BOLD) {
        codes.push("1".into());
    }
    format!("\x1b[{}m", codes.join(";"))
}

fn ansi_color(color: Color, foreground: bool) -> String {
    let base = if foreground { 38 } else { 48 };
    match color {
        Color::Rgb(r, g, b) => format!("{base};2;{r};{g};{b}"),
        Color::Indexed(index) => format!("{base};5;{index}"),
        Color::Black => if foreground { "30" } else { "40" }.into(),
        Color::White => if foreground { "37" } else { "47" }.into(),
        Color::Reset => if foreground { "39" } else { "49" }.into(),
        _ => if foreground { "39" } else { "49" }.into(),
    }
}

fn validate_render(name: &str, render: &str, width: u16, height: u16) -> Result<()> {
    let lines: Vec<&str> = render.lines().collect();
    if lines.len() != height as usize
        || lines
            .iter()
            .any(|line| line.chars().count() != width as usize)
    {
        bail!("{name}: render dimensions do not match {width}x{height}");
    }
    let required: &[&str] = if name.starts_with("too_small") {
        &[
            "Terminal too small",
            "Required: 80x24",
            "Current:  79x23",
            "q  Quit",
        ]
    } else if name.starts_with("page1") {
        &[
            "NOW",
            "RECENT SIGNALS",
            "PROJECT",
            "Claude",
            "Codex",
            "%",
            "Inspect",
        ]
    } else if name.starts_with("claude") || name.starts_with("codex") {
        &["SESSIONS", "PROJECT", "OLDER SESSIONS", "%", "Inspect"]
    } else if name.starts_with("detail_open") {
        &[
            "SESSION DETAIL",
            "CURRENT RUN USAGE",
            "TRANSCRIPT LIFETIME USAGE",
            "SOURCE METADATA",
            "JSONL",
        ]
    } else if name.starts_with("detail_exited") {
        &[
            "SESSION DETAIL",
            "EXITED",
            "TRANSCRIPT LIFETIME USAGE",
            "SOURCE METADATA",
            "JSONL",
        ]
    } else {
        &[
            "COPY SOURCE METADATA",
            "Source log",
            "Worktree",
            "Session ID",
            "Enter Copy",
        ]
    };
    for needle in required {
        if !render.contains(needle) {
            bail!("{name}: required text `{needle}` is not visible");
        }
    }
    if name.starts_with("detail_exited") && render.contains("CURRENT RUN USAGE") {
        bail!("{name}: Exited Detail must not show Current Run Usage");
    }
    Ok(())
}

fn validate_themes() -> Result<()> {
    for theme in THEMES {
        let colors = [theme.critical, theme.warning, theme.idle, theme.busy];
        if theme.foreground == theme.background
            || theme.muted == theme.background
            || theme.selected_background == theme.background
            || theme.selected_foreground == theme.selected_background
            || colors.iter().any(|color| *color == theme.background)
        {
            bail!(
                "theme {} does not preserve required semantic contrast",
                theme.name
            );
        }
        for (role, foreground, background, minimum) in [
            ("foreground", theme.foreground, theme.background, 4.5),
            ("muted", theme.muted, theme.background, 3.0),
            (
                "selected",
                theme.selected_foreground,
                theme.selected_background,
                4.5,
            ),
            ("critical", theme.critical, theme.background, 4.5),
            ("warning", theme.warning, theme.background, 4.5),
            ("idle", theme.idle, theme.background, 4.5),
            ("busy", theme.busy, theme.background, 4.5),
        ] {
            let ratio = contrast_ratio(foreground, background);
            if ratio < minimum {
                bail!(
                    "theme {} role {role} contrast {ratio:.2} is below {minimum:.1}",
                    theme.name
                );
            }
        }
    }
    Ok(())
}

fn validate_interactions(data: &Fixtures) -> Result<()> {
    let mut app = App::default();
    handle_key(&mut app, data, KeyCode::Tab, KeyModifiers::NONE);
    if app.focus != Focus::Signals {
        bail!("Tab did not move focus from NOW to RECENT SIGNALS");
    }
    handle_key(&mut app, data, KeyCode::Down, KeyModifiers::NONE);
    handle_key(&mut app, data, KeyCode::Enter, KeyModifiers::NONE);
    if app.focus != Focus::Now || app.now_selected != 0 || app.detail_session.is_some() {
        bail!("Shared Worktree Risk Signal did not locate its Worktree in NOW");
    }

    handle_key(&mut app, data, KeyCode::Tab, KeyModifiers::NONE);
    app.signal_selected = 0;
    handle_key(&mut app, data, KeyCode::Enter, KeyModifiers::NONE);
    if app.detail_session != Some(0) {
        bail!("Session-scoped Signal did not open Session Detail");
    }
    let origin_now_selection = app.now_selected;
    handle_key(&mut app, data, KeyCode::Esc, KeyModifiers::NONE);
    if app.page != Page::All || app.now_selected != origin_now_selection {
        bail!("Esc did not restore the origin page selection");
    }

    handle_key(&mut app, data, KeyCode::Char('2'), KeyModifiers::NONE);
    if app.page != Page::Claude || app.detail_session.is_some() {
        bail!("2 did not change only the top-level page");
    }
    app.expanded_older.insert("tokenplay");
    app.claude_selected = 1;
    app.detail_session = Some(2);
    handle_key(&mut app, data, KeyCode::Esc, KeyModifiers::NONE);
    if app.claude_selected != 1 || !app.expanded_older.contains("tokenplay") {
        bail!("Esc did not preserve Provider selection and disclosure state");
    }

    let before = (
        app.page,
        app.focus,
        app.claude_selected,
        app.expanded_older.clone(),
    );
    handle_key(&mut app, data, KeyCode::Char('t'), KeyModifiers::NONE);
    let after = (
        app.page,
        app.focus,
        app.claude_selected,
        app.expanded_older.clone(),
    );
    if before != after || app.theme != 1 {
        bail!("Theme cycling changed behavior state");
    }
    Ok(())
}

fn contrast_ratio(foreground: Color, background: Color) -> f64 {
    let luminance = |color| match color {
        Color::Rgb(r, g, b) => {
            let channel = |value: u8| {
                let value = f64::from(value) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
        }
        _ => 0.0,
    };
    let a = luminance(foreground);
    let b = luminance(background);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn parse_size(value: &str) -> Result<(u16, u16)> {
    let (width, height) = value
        .split_once('x')
        .with_context(|| format!("invalid size {value}"))?;
    Ok((width.parse()?, height.parse()?))
}

fn attention_style(attention: Attention, theme: &Theme) -> Style {
    Style::default().fg(attention_color(attention, theme))
}

fn attention_color(attention: Attention, theme: &Theme) -> Color {
    match attention {
        Attention::Critical => theme.critical,
        Attention::SharedRisk | Attention::Warning => theme.warning,
        Attention::Idle => theme.idle,
        Attention::Healthy => theme.busy,
        Attention::Historical => theme.exited,
    }
}

fn context_percent(context: Option<u8>) -> String {
    context
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "N/A".into())
}

fn context_text(context: Option<u8>, bar: bool) -> String {
    match (context, bar) {
        (Some(value), true) => {
            let filled = ((value as usize + 9) / 10).min(10);
            format!(
                "[{}{}] {value}%",
                "#".repeat(filled),
                "-".repeat(10 - filled)
            )
        }
        (Some(value), false) => format!("{value}%"),
        (None, true) => "[----------] N/A".into(),
        (None, false) => "N/A".into(),
    }
}

fn fit(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let count = value.chars().count();
    if count <= width {
        return value.into();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let head: String = value.chars().take(width - 3).collect();
    format!("{head}...")
}

fn tail_fit(value: &str, width: usize) -> String {
    if width <= 3 || value.chars().count() <= width {
        return fit(value, width);
    }
    let tail: String = value
        .chars()
        .rev()
        .take(width - 3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{tail}")
}

fn pad_fit(value: &str, width: usize) -> String {
    let fitted = fit(value, width);
    format!("{fitted:<width$}")
}

fn wrap_chunks(value: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = value.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(width.max(1))
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn short_id(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn short_path(value: &str) -> String {
    let parts: Vec<&str> = value.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() <= 2 {
        value.into()
    } else {
        format!(".../{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    }
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical_margin = area.height.saturating_sub(height) / 2;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(vertical_margin),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal_margin = 100u16.saturating_sub(percent_x) / 2;
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(horizontal_margin),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(horizontal_margin),
        ])
        .split(rows[1])[1]
}

struct NowWorktree {
    path: &'static str,
    branch: &'static str,
    sessions: &'static [usize],
}

struct NowGroup {
    project: &'static str,
    worktrees: &'static [NowWorktree],
    direct_sessions: &'static [usize],
}

fn now_sessions() -> &'static [usize] {
    &[0, 1, 2, 4, 3]
}

fn now_groups() -> &'static [NowGroup] {
    static TOKENPLAY_WORKTREES: [NowWorktree; 2] = [
        NowWorktree {
            path: "/Users/peibin/code/personal/tokenplay",
            branch: "feat/attention-console",
            sessions: &[0, 1],
        },
        NowWorktree {
            path: "/Users/peibin/code/personal/tokenplay-worktrees/attention-layout-prototype",
            branch: "prototype/q-007-responsive-terminal-layout",
            sessions: &[2],
        },
    ];
    static MERCHANT_WORKTREES: [NowWorktree; 1] = [NowWorktree {
        path: "/Users/peibin/code/client/merchant-observability-console-with-an-intentionally-long-name",
        branch: "main",
        sessions: &[3],
    }];
    static GROUPS: [NowGroup; 3] = [
        NowGroup {
            project: "tokenplay",
            worktrees: &TOKENPLAY_WORKTREES,
            direct_sessions: &[],
        },
        NowGroup {
            project: "research-notes-with-a-very-long-standalone-project-name",
            worktrees: &[],
            direct_sessions: &[4],
        },
        NowGroup {
            project: "merchant-observability-console-with-an-intentionally-long-name",
            worktrees: &MERCHANT_WORKTREES,
            direct_sessions: &[],
        },
    ];
    &GROUPS
}

fn provider_projects(provider: Provider) -> &'static [&'static str] {
    match provider {
        Provider::Claude => &[
            "tokenplay",
            "research-notes-with-a-very-long-standalone-project-name",
            "merchant-observability-console-with-an-intentionally-long-name",
            "tokenplay (archive clone)",
        ],
        Provider::Codex => &[
            "tokenplay",
            "merchant-observability-console-with-an-intentionally-long-name",
        ],
    }
}

fn fixtures() -> Fixtures {
    let open_run = Usage {
        input: "81.2k",
        output: "9.8k",
        cache_read: "642.1k",
        cache_create: "18.4k",
        cost: "$6.42",
    };
    let lifetime = Usage {
        input: "344.8k",
        output: "38.2k",
        cache_read: "2.8m",
        cache_create: "77.1k",
        cost: "$28.64",
    };
    let sessions = vec![
        Session {
            id: "claude-native-session-7f31b652-811d-4bb7-a675-b8af91ac77d2",
            provider: Provider::Claude,
            project: "tokenplay",
            worktree: Some("/Users/peibin/code/personal/tokenplay"),
            cwd: "/Users/peibin/code/personal/tokenplay/src/ui/experiments/attention-console",
            source_log: "/Users/peibin/.claude/projects/-Users-peibin-code-personal-tokenplay/7f31b652-811d-4bb7-a675-b8af91ac77d2.jsonl",
            branch: Some("feat/attention-console"),
            state: SessionState::Busy,
            model: "claude-opus-4-20260514-thinking-long-context",
            context: Some(92),
            last_activity: "2m",
            attention: Attention::Critical,
            compactions: 2,
            recent: true,
            current_run: Some(open_run),
            lifetime,
        },
        Session {
            id: "codex-native-session-019c6e38-77c1-7d63-a4d2-a0f6e06fe551",
            provider: Provider::Codex,
            project: "tokenplay",
            worktree: Some("/Users/peibin/code/personal/tokenplay"),
            cwd: "/Users/peibin/code/personal/tokenplay",
            source_log: "/Users/peibin/.codex/sessions/2026/07/30/rollout-2026-07-30T10-14-52-019c6e38-77c1-7d63-a4d2-a0f6e06fe551.jsonl",
            branch: Some("feat/attention-console"),
            state: SessionState::Idle,
            model: "gpt-5.6-codex-ultra-long-model-identifier",
            context: Some(67),
            last_activity: "4m",
            attention: Attention::SharedRisk,
            compactions: 1,
            recent: true,
            current_run: Some(open_run),
            lifetime,
        },
        Session {
            id: "claude-native-session-prototype-linked-worktree-4c16",
            provider: Provider::Claude,
            project: "tokenplay",
            worktree: Some("/Users/peibin/code/personal/tokenplay-worktrees/attention-layout-prototype"),
            cwd: "/Users/peibin/code/personal/tokenplay-worktrees/attention-layout-prototype",
            source_log: "/Users/peibin/.claude/projects/-Users-peibin-code-personal-tokenplay-worktrees-attention-layout-prototype/4c16.jsonl",
            branch: Some("prototype/q-007-responsive-terminal-layout"),
            state: SessionState::Busy,
            model: "claude-sonnet-4-20260514",
            context: Some(76),
            last_activity: "8m",
            attention: Attention::Warning,
            compactions: 0,
            recent: true,
            current_run: Some(open_run),
            lifetime,
        },
        Session {
            id: "codex-native-session-merchant-console-healthy-busy-a951",
            provider: Provider::Codex,
            project: "merchant-observability-console-with-an-intentionally-long-name",
            worktree: Some("/Users/peibin/code/client/merchant-observability-console-with-an-intentionally-long-name"),
            cwd: "/Users/peibin/code/client/merchant-observability-console-with-an-intentionally-long-name",
            source_log: "/Users/peibin/.codex/sessions/2026/07/30/rollout-merchant-console-healthy-busy-a951.jsonl",
            branch: Some("main"),
            state: SessionState::Busy,
            model: "gpt-5.5-codex",
            context: Some(41),
            last_activity: "1m",
            attention: Attention::Healthy,
            compactions: 0,
            recent: true,
            current_run: Some(open_run),
            lifetime,
        },
        Session {
            id: "claude-native-session-standalone-non-git-unknown-context-d004",
            provider: Provider::Claude,
            project: "research-notes-with-a-very-long-standalone-project-name",
            worktree: None,
            cwd: "/Users/peibin/Documents/research-notes-with-a-very-long-standalone-project-name",
            source_log: "/Users/peibin/.claude/projects/-Users-peibin-Documents-research-notes/standalone-d004.jsonl",
            branch: None,
            state: SessionState::Idle,
            model: "claude-sonnet-4-20260514",
            context: None,
            last_activity: "11m",
            attention: Attention::Idle,
            compactions: 0,
            recent: true,
            current_run: Some(open_run),
            lifetime,
        },
        Session {
            id: "claude-native-session-exited-older-2a17",
            provider: Provider::Claude,
            project: "tokenplay",
            worktree: Some("/Users/peibin/code/personal/tokenplay"),
            cwd: "/Users/peibin/code/personal/tokenplay",
            source_log: "/Users/peibin/.claude/projects/-Users-peibin-code-personal-tokenplay/older/exited-2a17.jsonl",
            branch: Some("main"),
            state: SessionState::Exited,
            model: "claude-sonnet-4-20250514",
            context: Some(88),
            last_activity: "12d",
            attention: Attention::Historical,
            compactions: 3,
            recent: false,
            current_run: None,
            lifetime,
        },
        Session {
            id: "claude-native-session-archive-clone-older-013a",
            provider: Provider::Claude,
            project: "tokenplay (archive clone)",
            worktree: Some("/Users/peibin/archive/tokenplay"),
            cwd: "/Users/peibin/archive/tokenplay",
            source_log: "/Users/peibin/.claude/projects/-Users-peibin-archive-tokenplay/archive-013a.jsonl",
            branch: Some("main"),
            state: SessionState::Exited,
            model: "claude-haiku-3-5",
            context: Some(23),
            last_activity: "31d",
            attention: Attention::Historical,
            compactions: 0,
            recent: false,
            current_run: None,
            lifetime,
        },
        Session {
            id: "claude-native-session-merchant-recent-exited-fca1",
            provider: Provider::Claude,
            project: "merchant-observability-console-with-an-intentionally-long-name",
            worktree: Some("/Users/peibin/code/client/merchant-observability-console-with-an-intentionally-long-name"),
            cwd: "/Users/peibin/code/client/merchant-observability-console-with-an-intentionally-long-name",
            source_log: "/Users/peibin/.claude/projects/-Users-peibin-code-client-merchant-console/fca1.jsonl",
            branch: Some("main"),
            state: SessionState::Exited,
            model: "claude-sonnet-4-20260514",
            context: Some(63),
            last_activity: "2d",
            attention: Attention::Historical,
            compactions: 1,
            recent: true,
            current_run: None,
            lifetime,
        },
        Session {
            id: "codex-native-session-tokenplay-older-9be2",
            provider: Provider::Codex,
            project: "tokenplay",
            worktree: Some("/Users/peibin/code/personal/tokenplay"),
            cwd: "/Users/peibin/code/personal/tokenplay",
            source_log: "/Users/peibin/.codex/sessions/2026/07/05/rollout-tokenplay-older-9be2.jsonl",
            branch: Some("main"),
            state: SessionState::Exited,
            model: "gpt-5.4-codex",
            context: Some(84),
            last_activity: "25d",
            attention: Attention::Historical,
            compactions: 4,
            recent: false,
            current_run: None,
            lifetime,
        },
        Session {
            id: "codex-native-session-merchant-older-f9c2",
            provider: Provider::Codex,
            project: "merchant-observability-console-with-an-intentionally-long-name",
            worktree: Some("/Users/peibin/code/client/merchant-observability-console-with-an-intentionally-long-name"),
            cwd: "/Users/peibin/code/client/merchant-observability-console-with-an-intentionally-long-name",
            source_log: "/Users/peibin/.codex/sessions/2026/06/10/rollout-merchant-older-f9c2.jsonl",
            branch: Some("release/2026-06"),
            state: SessionState::Exited,
            model: "gpt-5.3-codex",
            context: None,
            last_activity: "50d",
            attention: Attention::Historical,
            compactions: 0,
            recent: false,
            current_run: None,
            lifetime,
        },
    ];
    let signals = vec![
        Signal {
            age: "01m",
            severity: Attention::Critical,
            event: "Context entered Critical at 92%",
            scope: "tokenplay / main / Claude",
            action: "Inspect",
            session: Some(0),
            worktree: None,
        },
        Signal {
            age: "04m",
            severity: Attention::SharedRisk,
            event: "Shared risk: 2 Open Sessions",
            scope: "tokenplay / main",
            action: "Locate",
            session: None,
            worktree: Some("/Users/peibin/code/personal/tokenplay"),
        },
        Signal {
            age: "07m",
            severity: Attention::Warning,
            event: "Compaction 91%>38%; Critical cleared",
            scope: "tokenplay / linked / Claude",
            action: "Inspect",
            session: Some(2),
            worktree: None,
        },
        Signal {
            age: "11m",
            severity: Attention::Idle,
            event: "Busy changed to Idle",
            scope: "research-notes / Claude",
            action: "Inspect",
            session: Some(4),
            worktree: None,
        },
        Signal {
            age: "18m",
            severity: Attention::Historical,
            event: "Open Session changed to Exited",
            scope: "merchant-console / Claude",
            action: "Inspect",
            session: Some(7),
            worktree: None,
        },
    ];
    Fixtures { sessions, signals }
}
