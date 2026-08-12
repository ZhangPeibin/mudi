use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::{Backend, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;
use sessionmesh::app::Snapshot;
use sessionmesh::attention::{AttentionEngine, SessionIdentity};
use sessionmesh::domain::{
    Compaction, ContextState, ModelUsage, Project, ProjectIdentity, Provider, Session,
    SessionState, SourceFormat, SourceMetadata, Usage, UsageSummary, Worktree,
};
use sessionmesh::providers::state::ProcessIdentity;
use sessionmesh::session_labels::{CurationDisposition, SessionLabels};
use sessionmesh::signals::{
    ContextAlertLevel, RecentSignal, RecentSignalKind, SharedWorktreeRiskChange,
};
use sessionmesh::tasks::{OperationTask, OperationTaskState, SuccessorLaunchPlan};
use sessionmesh::ui::{draw, Ui, UiAction};

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct CursorBackend {
    inner: TestBackend,
    cursor_visible: bool,
}

impl CursorBackend {
    fn new(width: u16, height: u16) -> Self {
        Self {
            inner: TestBackend::new(width, height),
            cursor_visible: false,
        }
    }

    fn buffer(&self) -> &Buffer {
        self.inner.buffer()
    }
}

impl Backend for CursorBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = false;
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = true;
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct ProductionRender {
    width: u16,
    height: u16,
    text: String,
    buffer: Buffer,
    cursor_position: Position,
    cursor_visible: bool,
}

impl ProductionRender {
    fn visible_text(&self) -> String {
        let text = self
            .text
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        text.trim_end_matches('\n').to_string()
    }

    fn style_at(&self, text: &str) -> Style {
        let cell_count = text.chars().count();
        for row in self.buffer.content().chunks(self.width as usize) {
            if let Some(cells) = row
                .windows(cell_count)
                .find(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == text)
            {
                return cells[0].style();
            }
        }
        panic!("rendered text not found: {text:?}");
    }

    fn style_at_in_row(&self, row_text: &str, text: &str) -> Style {
        let cell_count = text.chars().count();
        for row in self.buffer.content().chunks(self.width as usize) {
            let rendered_row = row.iter().map(|cell| cell.symbol()).collect::<String>();
            if !rendered_row.contains(row_text) {
                continue;
            }
            if let Some(cells) = row
                .windows(cell_count)
                .find(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == text)
            {
                return cells[0].style();
            }
        }
        panic!("rendered text {text:?} not found in row containing {row_text:?}");
    }

    fn position_of(&self, text: &str) -> Position {
        let cell_count = text.chars().count();
        for (y, row) in self
            .buffer
            .content()
            .chunks(self.width as usize)
            .enumerate()
        {
            if let Some(x) = row.windows(cell_count).position(|cells| {
                cells.iter().map(|cell| cell.symbol()).collect::<String>() == text
            }) {
                return Position::new(x as u16, y as u16);
            }
        }
        panic!("rendered text not found: {text:?}");
    }

    fn top_rule_style(&self, title: &str) -> Style {
        let position = self.position_of(title);
        let row_start = position.y as usize * self.width as usize;
        let row = &self.buffer.content()[row_start..row_start + self.width as usize];
        row[..position.x as usize]
            .iter()
            .rev()
            .chain(row[position.x as usize + title.chars().count()..].iter())
            .find(|cell| matches!(cell.symbol(), "─" | "┌" | "┬"))
            .unwrap_or_else(|| panic!("top rule not found beside region title {title:?}"))
            .style()
    }

    fn style_before_right_border_in_row(&self, text: &str) -> Style {
        let position = self.position_of(text);
        let row_start = position.y as usize * self.width as usize;
        let row = &self.buffer.content()[row_start..row_start + self.width as usize];
        let border = row[position.x as usize..]
            .iter()
            .position(|cell| cell.symbol() == "│")
            .map(|offset| position.x as usize + offset)
            .unwrap_or_else(|| panic!("right border not found after selected row {text:?}"));
        row[border.saturating_sub(1)].style()
    }
}

fn render(snapshot: &Snapshot, ui: &Ui, width: u16, height: u16) -> ProductionRender {
    let backend = CursorBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, snapshot, ui)).unwrap();
    let cursor_position = terminal.get_cursor_position().unwrap();
    let cursor_visible = terminal.backend().cursor_visible;
    let buffer = terminal.backend().buffer();
    let text = buffer
        .content()
        .chunks(width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    ProductionRender {
        width: buffer.area.width,
        height: buffer.area.height,
        text,
        buffer: buffer.clone(),
        cursor_position,
        cursor_visible,
    }
}

fn fixture_session(
    provider: Provider,
    id: &str,
    model: &str,
    state: SessionState,
    context_tokens: u64,
    last_activity_ms: i64,
    worktree: (&str, &str),
) -> Session {
    let (worktree_root, branch) = worktree;
    let usage = Usage {
        input: 12_345,
        output: 2_345,
        cache_read: 34_567,
        cache_write: 456,
    };
    Session {
        provider,
        id: id.into(),
        provenance: sessionmesh::domain::SessionProvenance::Observed,
        source: SourceMetadata {
            path: PathBuf::from(format!("/Users/fixture/.sessions/{id}.jsonl")),
            format: SourceFormat::JsonLines,
        },
        project: Some(Project {
            identity: ProjectIdentity::GitCommonDir(PathBuf::from(
                "/Users/fixture/code/sessionmesh/.git",
            )),
            label: "sessionmesh".into(),
        }),
        worktree: Some(Worktree {
            root: PathBuf::from(worktree_root),
            branch: Some(branch.into()),
            writable: true,
        }),
        cwd: Some(worktree_root.into()),
        latest_model: Some(model.into()),
        state,
        context: ContextState {
            tokens: context_tokens,
            window: Some(200_000),
        },
        usage,
        per_model: vec![ModelUsage {
            model: model.into(),
            usage,
            cost: Some(0.42),
        }],
        current_run_usage: state.is_open().then_some(UsageSummary {
            usage: Usage {
                input: 1_200,
                output: 230,
                cache_read: 3_400,
                cache_write: 45,
            },
            per_model: vec![ModelUsage {
                model: model.into(),
                usage: Usage {
                    input: 1_200,
                    output: 230,
                    cache_read: 3_400,
                    cache_write: 45,
                },
                cost: Some(0.04),
            }],
            cost: Some(0.04),
        }),
        compactions: vec![],
        last_activity_ms,
        cost: Some(0.42),
    }
}

fn production_snapshot() -> Snapshot {
    const NOW: i64 = 1_700_000_000_000;
    const DAY: i64 = 86_400_000;
    let shared_root = "/Users/fixture/code/sessionmesh";
    let mut critical = fixture_session(
        Provider::ClaudeCode,
        "claude-critical",
        "claude-opus-4-8",
        SessionState::Busy,
        180_000,
        NOW - 30_000,
        (shared_root, "main"),
    );
    critical.compactions = vec![
        Compaction {
            at_ms: NOW - 90_000,
            trigger: Some("auto".into()),
            pre_tokens: Some(188_000),
        },
        Compaction {
            at_ms: NOW - 60_000,
            trigger: Some("manual".into()),
            pre_tokens: Some(181_000),
        },
    ];
    critical.source.path = PathBuf::from(
        "/Users/fixture/.claude/projects/-Users-fixture-code-sessionmesh/claude-critical.jsonl",
    );
    let shared = fixture_session(
        Provider::CodexCli,
        "codex-shared",
        "gpt-5-codex",
        SessionState::Busy,
        80_000,
        NOW - 60_000,
        (shared_root, "main"),
    );
    let warning = fixture_session(
        Provider::ClaudeCode,
        "claude-warning",
        "claude-sonnet-4-5",
        SessionState::Busy,
        150_000,
        NOW - 120_000,
        ("/Users/fixture/code/sessionmesh-warning", "warning"),
    );
    let idle = fixture_session(
        Provider::CodexCli,
        "codex-idle",
        "gpt-5.2-codex",
        SessionState::Idle,
        60_000,
        NOW - 180_000,
        ("/Users/fixture/code/sessionmesh-idle", "idle"),
    );
    let healthy = fixture_session(
        Provider::ClaudeCode,
        "claude-healthy",
        "claude-haiku-4-5",
        SessionState::Busy,
        40_000,
        NOW - 240_000,
        ("/Users/fixture/code/sessionmesh-healthy", "healthy"),
    );
    let older = fixture_session(
        Provider::ClaudeCode,
        "claude-older",
        "claude-sonnet-4-5",
        SessionState::Exited,
        50_000,
        NOW - 8 * DAY,
        ("/Users/fixture/code/sessionmesh-older", "archive"),
    );
    let recent_exited = fixture_session(
        Provider::CodexCli,
        "codex-exited",
        "gpt-5-codex",
        SessionState::Exited,
        70_000,
        NOW - DAY,
        ("/Users/fixture/code/sessionmesh-exited", "done"),
    );
    let sessions = vec![
        critical,
        shared,
        warning,
        idle,
        healthy,
        older,
        recent_exited,
    ];
    let attention = AttentionEngine::default().update(&sessions, NOW);
    let session_identity = |provider, native_id: &str| SessionIdentity {
        provider,
        native_id: native_id.into(),
    };
    let recent_signals = vec![
        RecentSignal::Worktree {
            observed_at_ms: NOW - 5_000,
            worktree_root: PathBuf::from(shared_root),
            change: SharedWorktreeRiskChange::Detected,
            before: vec![session_identity(Provider::ClaudeCode, "claude-critical")],
            after: vec![
                session_identity(Provider::ClaudeCode, "claude-critical"),
                session_identity(Provider::CodexCli, "codex-shared"),
            ],
        },
        RecentSignal::Session {
            observed_at_ms: NOW - 10_000,
            provider: Provider::ClaudeCode,
            session_id: "claude-critical".into(),
            kind: RecentSignalKind::ContextAlertChanged {
                from: Some(ContextAlertLevel::Warning),
                to: Some(ContextAlertLevel::Critical),
                before: ContextState {
                    tokens: 160_000,
                    window: Some(200_000),
                },
                after: ContextState {
                    tokens: 180_000,
                    window: Some(200_000),
                },
            },
        },
        RecentSignal::Session {
            observed_at_ms: NOW - 15_000,
            provider: Provider::ClaudeCode,
            session_id: "claude-critical".into(),
            kind: RecentSignalKind::CompactionObserved {
                compaction: Compaction {
                    at_ms: NOW - 15_000,
                    trigger: Some("auto".into()),
                    pre_tokens: Some(188_000),
                },
                context_before: ContextState {
                    tokens: 188_000,
                    window: Some(200_000),
                },
                context_after: ContextState {
                    tokens: 100_000,
                    window: Some(200_000),
                },
                cleared_alert: Some(ContextAlertLevel::Critical),
            },
        },
        RecentSignal::Session {
            observed_at_ms: NOW - 20_000,
            provider: Provider::ClaudeCode,
            session_id: "claude-warning".into(),
            kind: RecentSignalKind::ContextAlertChanged {
                from: None,
                to: Some(ContextAlertLevel::Warning),
                before: ContextState {
                    tokens: 120_000,
                    window: Some(200_000),
                },
                after: ContextState {
                    tokens: 150_000,
                    window: Some(200_000),
                },
            },
        },
        RecentSignal::Session {
            observed_at_ms: NOW - 25_000,
            provider: Provider::CodexCli,
            session_id: "codex-idle".into(),
            kind: RecentSignalKind::SessionStateChanged {
                from: SessionState::Busy,
                to: SessionState::Idle,
            },
        },
    ];

    Snapshot {
        sessions,
        recent_signals,
        attention: Some(attention),
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    }
}

fn focus_fixture_task(now_ms: i64) -> OperationTask {
    OperationTask {
        id: "task-focus".into(),
        state: OperationTaskState::Queued,
        source_provider: Provider::ClaudeCode,
        source_session_id: "claude-critical".into(),
        source_record_path: PathBuf::from("/private/source.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::from("/usr/local/bin/claude"),
        created_at_ms: now_ms - 1_000,
        state_changed_at_ms: now_ms - 1_000,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: None,
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: None,
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    }
}

fn assert_overview_focus(rendered: &ProductionRender, focused_index: usize) {
    let focused_style = rendered.style_at("1 2 3");
    let titles = [
        rendered.style_at("PROJECT"),
        rendered.style_at("RECENT SIGNALS"),
        rendered.style_at("TASKS"),
    ];
    let title_counts = [
        rendered.style_at_in_row("PROJECT", "1"),
        rendered.style_at_in_row("RECENT SIGNALS", "5"),
        rendered.style_at_in_row("TASKS", "1"),
    ];
    for (title, count) in titles.iter().zip(title_counts) {
        assert_eq!(
            count, *title,
            "region title count split the focus treatment"
        );
    }
    assert_eq!(
        titles
            .iter()
            .filter(|style| {
                style.fg == focused_style.fg
                    && style.bg == focused_style.bg
                    && style.add_modifier.contains(Modifier::BOLD)
            })
            .count(),
        1,
        "exactly one Overview title should carry the focused presentation"
    );
    assert_eq!(titles[focused_index].fg, focused_style.fg);
    assert_eq!(titles[focused_index].bg, focused_style.bg);
    assert!(titles[focused_index].add_modifier.contains(Modifier::BOLD));
    assert!(contrast_ratio(titles[focused_index]) >= 4.5);
    for (index, title) in titles.iter().enumerate() {
        if index == focused_index {
            continue;
        }
        assert_ne!(title.bg, focused_style.bg);
        assert!(!title.add_modifier.contains(Modifier::BOLD));
    }

    let rules = [
        rendered.top_rule_style("PROJECT"),
        rendered.top_rule_style("RECENT SIGNALS"),
        rendered.top_rule_style("TASKS"),
    ];
    let focused_rule = rules[focused_index].fg;
    assert_eq!(
        rules
            .iter()
            .filter(|style| style.fg == focused_rule)
            .count(),
        1,
        "exactly one Overview top rule should carry the focus accent"
    );

    let selected_background = focused_style.bg;
    assert_eq!(
        rendered.style_at_in_row("> ✳ critical", "critical").bg,
        selected_background
    );
    assert_eq!(rendered.style_at("Risk detected").bg, selected_background);
    assert_eq!(rendered.style_at("Queued").bg, selected_background);
    for selected_row in ["> ✳ critical", "Risk detected", "Queued"] {
        assert_eq!(
            rendered.style_before_right_border_in_row(selected_row).bg,
            selected_background,
            "selected row background did not fill its region for {selected_row:?}"
        );
    }
}

fn assert_fixture(rendered: &ProductionRender, fixture: &str) {
    assert_eq!(rendered.text.lines().count(), rendered.height as usize);
    for (row_index, row) in rendered.text.lines().enumerate() {
        assert_eq!(
            row.chars().count(),
            rendered.width as usize,
            "rendered row {row_index} changed the requested width"
        );
    }
    assert!(rendered
        .text
        .chars()
        .any(|character| !character.is_whitespace()));

    assert_eq!(rendered.visible_text(), fixture.trim_end_matches('\n'));
}

fn press(ui: &mut Ui, snapshot: &Snapshot, code: KeyCode) {
    ui.handle_key(KeyEvent::new(code, KeyModifiers::NONE), snapshot)
        .unwrap();
}

#[test]
fn exited_session_hide_is_confirmed_persisted_and_removed_from_the_normal_tree() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hide-session-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let mut snapshot = production_snapshot();
    let mut exited = snapshot
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    exited.last_activity_ms = snapshot.generated_ms - 1_000;
    let mut open = exited.clone();
    open.id = "codex-open".into();
    open.state = SessionState::Busy;
    open.last_activity_ms = snapshot.generated_ms;
    snapshot.sessions = vec![open, exited.clone()];
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Right);
    press(&mut ui, &snapshot, KeyCode::Down);

    press(&mut ui, &snapshot, KeyCode::Char('H'));
    let confirmation = render(&snapshot, &ui, 120, 30).text;
    for required in [
        "Hide Session?",
        "Codex CLI",
        "codex-exited",
        "Enter Hide",
        "Esc Keep Visible",
    ] {
        assert!(
            confirmation.contains(required),
            "missing Hide confirmation text {required:?}\n{confirmation}"
        );
    }

    press(&mut ui, &snapshot, KeyCode::Enter);
    let hidden = render(&snapshot, &ui, 120, 30).text;
    assert!(!hidden.contains("x-exited"));
    let labels = SessionLabels::load(directory.join("session-labels.json"));
    assert_eq!(
        labels.disposition(exited.provider, &exited.id),
        Some(CurationDisposition::Hidden)
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn open_session_cannot_be_hidden() {
    let source = production_snapshot();
    let open = source
        .sessions
        .iter()
        .find(|session| session.id == "claude-critical")
        .unwrap()
        .clone();
    let snapshot = Snapshot {
        sessions: vec![open],
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Char('H'));

    let rendered = render(&snapshot, &ui, 120, 30).text;
    assert!(rendered.contains("Open Session cannot be hidden"));
    assert!(!rendered.contains("Hide Session?"));
}

#[test]
fn hide_confirmation_revalidates_that_the_session_is_still_exited() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hide-revalidation-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let source = production_snapshot();
    let mut exited = source
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    exited.last_activity_ms = source.generated_ms - 1_000;
    let mut open = exited.clone();
    open.id = "codex-open".into();
    open.state = SessionState::Busy;
    open.last_activity_ms = source.generated_ms;
    let mut snapshot = Snapshot {
        sessions: vec![open, exited.clone()],
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Right);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Char('H'));

    snapshot
        .sessions
        .iter_mut()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .state = SessionState::Busy;
    press(&mut ui, &snapshot, KeyCode::Enter);

    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("Open Session cannot be hidden"));
    assert_eq!(
        SessionLabels::load(directory.join("session-labels.json"))
            .disposition(exited.provider, &exited.id),
        None
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_hide_is_a_snapshot_that_skips_and_reports_open_sessions() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hide-project-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let source = production_snapshot();
    let open = source
        .sessions
        .iter()
        .find(|session| session.id == "claude-critical")
        .unwrap()
        .clone();
    let mut first_exited = source
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    first_exited.project = open.project.clone();
    first_exited.worktree = open.worktree.clone();
    first_exited.cwd = open.cwd.clone();
    first_exited.last_activity_ms = source.generated_ms - 1_000;
    let mut second_exited = first_exited.clone();
    second_exited.provider = Provider::ClaudeCode;
    second_exited.id = "claude-exited-too".into();
    second_exited.last_activity_ms = source.generated_ms - 2_000;
    let snapshot = Snapshot {
        sessions: vec![open.clone(), first_exited.clone(), second_exited.clone()],
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Left);

    press(&mut ui, &snapshot, KeyCode::Char('H'));
    let confirmation = render(&snapshot, &ui, 120, 30).text;
    for required in [
        "Hide Project Sessions?",
        "2 Exited Sessions will be hidden",
        "1 Open Session will remain visible",
        "Future Sessions remain visible",
        "Enter Hide 2",
    ] {
        assert!(
            confirmation.contains(required),
            "missing Project Hide text {required:?}\n{confirmation}"
        );
    }

    press(&mut ui, &snapshot, KeyCode::Enter);
    let rendered = render(&snapshot, &ui, 120, 30).text;
    assert!(rendered.contains("critical"));
    assert!(!rendered.contains("x-exited"));
    let labels = SessionLabels::load(directory.join("session-labels.json"));
    for exited in [&first_exited, &second_exited] {
        assert_eq!(
            labels.disposition(exited.provider, &exited.id),
            Some(CurationDisposition::Hidden)
        );
    }
    assert_eq!(labels.disposition(open.provider, &open.id), None);

    let mut future = first_exited.clone();
    future.id = "codex-future-session".into();
    future.state = SessionState::Busy;
    future.last_activity_ms = source.generated_ms + 1_000;
    let mut rediscovered = snapshot.clone();
    rediscovered.sessions.push(future.clone());
    let visible_again = render(&rediscovered, &ui, 120, 30).text;
    assert!(visible_again.contains("-session"));
    assert_eq!(labels.disposition(future.provider, &future.id), None);

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn global_hidden_tree_shortcut_reports_an_empty_state_without_opening_a_view() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Char('h'));

    let rendered = render(&snapshot, &ui, 120, 30).text;
    assert!(rendered.contains("No hidden sessions"));
    assert!(!rendered.contains("HIDDEN SESSIONS"));
}

#[test]
fn hidden_tree_opens_collapsed_and_orders_sessions_newest_first() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hidden-tree-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let mut snapshot = production_snapshot();
    let mut first = snapshot
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    first.project = Some(Project {
        identity: ProjectIdentity::GitCommonDir(PathBuf::from(
            "/Users/fixture/code/sessionmesh-exited/.git",
        )),
        label: "sessionmesh-exited".into(),
    });
    let mut second_project = snapshot
        .sessions
        .iter()
        .find(|session| session.id == "claude-older")
        .unwrap()
        .clone();
    second_project.project = Some(Project {
        identity: ProjectIdentity::GitCommonDir(PathBuf::from(
            "/Users/fixture/code/sessionmesh-older/.git",
        )),
        label: "sessionmesh-older".into(),
    });
    let mut newer = first.clone();
    newer.provider = Provider::ClaudeCode;
    newer.id = "claude-newest-hidden".into();
    newer.last_activity_ms = first.last_activity_ms + 1_000;
    snapshot.sessions = vec![first.clone(), newer.clone(), second_project.clone()];
    let label_path = directory.join("session-labels.json");
    let mut labels = SessionLabels::load(&label_path);
    labels
        .set_metadata(newer.provider, &newer.id, "Newest hidden", "")
        .unwrap();
    labels
        .set_metadata(first.provider, &first.id, "Older hidden", "")
        .unwrap();
    labels
        .hide_sessions(&[
            SessionIdentity::from(&first),
            SessionIdentity::from(&newer),
            SessionIdentity::from(&second_project),
        ])
        .unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();

    press(&mut ui, &snapshot, KeyCode::Char('h'));
    let collapsed = render(&snapshot, &ui, 120, 30).text;
    assert!(collapsed.contains("HIDDEN SESSIONS"));
    assert!(collapsed.contains("2 Projects"), "{collapsed}");
    assert!(collapsed.contains("3 Sessions"));
    assert!(collapsed.contains("sessionmesh-exited"));
    assert!(collapsed.contains("sessionmesh-older"));
    assert!(!collapsed.contains("Newest hidden"));
    assert!(!collapsed.contains("Older hidden"));

    press(&mut ui, &snapshot, KeyCode::Enter);
    let expanded = render(&snapshot, &ui, 120, 30).text;
    let newest = expanded.find("Newest hidden").unwrap();
    let older = expanded.find("Older hidden").unwrap();
    assert!(
        newest < older,
        "newest Session should render first\n{expanded}"
    );
    assert!(expanded.contains("done"));
    press(&mut ui, &snapshot, KeyCode::Left);
    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("Newest hidden"));
    press(&mut ui, &snapshot, KeyCode::Right);
    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("Newest hidden"));
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Char('d'));
    assert!(!render(&snapshot, &ui, 120, 30)
        .text
        .contains("Permanently Exclude"));

    let mut restarted = Ui::load(&profile_path, None).unwrap();
    press(&mut restarted, &snapshot, KeyCode::Char('h'));
    let restarted = render(&snapshot, &restarted, 120, 30).text;
    assert!(!restarted.contains("Newest hidden"));
    assert!(!restarted.contains("Older hidden"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn session_detail_hidden_list_entry_opens_the_same_tree_and_restores_its_origin() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-detail-hidden-list-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let snapshot = production_snapshot();
    let hidden = snapshot
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap();
    SessionLabels::load(directory.join("session-labels.json"))
        .hide_sessions(&[SessionIdentity::from(hidden)])
        .unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();

    press(&mut ui, &snapshot, KeyCode::Enter);
    let detail = render(&snapshot, &ui, 120, 30).text;
    assert!(detail.contains("Session Detail"));
    assert!(detail.contains("Hidden List"));
    assert!(detail.contains("1 hidden Session"));
    assert!(detail.contains("h Handoff"));

    press(&mut ui, &snapshot, KeyCode::Enter);
    let hidden_tree = render(&snapshot, &ui, 120, 30).text;
    assert!(hidden_tree.contains("HIDDEN SESSIONS"));
    assert!(!hidden_tree.contains("Session Detail"));
    press(&mut ui, &snapshot, KeyCode::Esc);
    let restored = render(&snapshot, &ui, 120, 30).text;
    assert!(restored.contains("Session Detail"));
    assert!(restored.contains("Hidden List"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn hidden_tree_u_restores_the_selected_session_to_its_origin_inventory() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hidden-restore-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let source = production_snapshot();
    let mut hidden = source
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    hidden.last_activity_ms = source.generated_ms - 1_000;
    let snapshot = Snapshot {
        sessions: vec![hidden.clone()],
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    let label_path = directory.join("session-labels.json");
    SessionLabels::load(&label_path)
        .hide_sessions(&[SessionIdentity::from(&hidden)])
        .unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Char('3'));
    press(&mut ui, &snapshot, KeyCode::Char('h'));
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);

    press(&mut ui, &snapshot, KeyCode::Char('u'));

    let restored = render(&snapshot, &ui, 120, 30).text;
    assert!(!restored.contains("HIDDEN SESSIONS"));
    assert!(restored.contains("x-exited"));
    assert!(restored.contains("1 Session restored"));
    assert_eq!(
        SessionLabels::load(&label_path).disposition(hidden.provider, &hidden.id),
        None
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn hidden_tree_d_permanently_excludes_metadata_without_touching_provider_jsonl() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hidden-delete-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    let source_path = directory.join("provider-session.jsonl");
    let source_contents = br#"{"type":"provider-owned"}\n"#;
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    std::fs::write(&source_path, source_contents).unwrap();
    let source = production_snapshot();
    let mut hidden = source
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    hidden.source.path = source_path.clone();
    hidden.last_activity_ms = source.generated_ms - 1_000;
    let snapshot = Snapshot {
        sessions: vec![hidden.clone()],
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    let label_path = directory.join("session-labels.json");
    let mut labels = SessionLabels::load(&label_path);
    labels
        .set_metadata(
            hidden.provider,
            &hidden.id,
            "Delete me",
            "No longer relevant",
        )
        .unwrap();
    labels
        .hide_sessions(&[SessionIdentity::from(&hidden)])
        .unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Char('h'));
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);

    press(&mut ui, &snapshot, KeyCode::Char('d'));
    let confirmation = render(&snapshot, &ui, 120, 30).text;
    for required in [
        "Permanently Exclude Session?",
        "1 Session",
        "Alias and Description",
        "Provider JSONL remains unchanged",
        "Enter Delete",
        "Esc Keep Hidden",
    ] {
        assert!(
            confirmation.contains(required),
            "missing Delete text {required:?}\n{confirmation}"
        );
    }

    press(&mut ui, &snapshot, KeyCode::Enter);
    let labels = SessionLabels::load(&label_path);
    assert_eq!(
        labels.disposition(hidden.provider, &hidden.id),
        Some(CurationDisposition::PermanentlyExcluded)
    );
    assert_eq!(labels.alias(hidden.provider, &hidden.id), None);
    assert_eq!(labels.description(hidden.provider, &hidden.id), None);
    assert_eq!(std::fs::read(&source_path).unwrap(), source_contents);
    assert!(!render(&snapshot, &ui, 120, 30)
        .text
        .contains("HIDDEN SESSIONS"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn hidden_tree_project_delete_applies_to_all_current_hidden_sessions() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hidden-project-delete-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let source = production_snapshot();
    let first = source
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    let mut second = first.clone();
    second.provider = Provider::ClaudeCode;
    second.id = "claude-hidden-peer".into();
    second.last_activity_ms -= 1_000;
    let snapshot = Snapshot {
        sessions: vec![first.clone(), second.clone()],
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    let label_path = directory.join("session-labels.json");
    SessionLabels::load(&label_path)
        .hide_sessions(&[
            SessionIdentity::from(&first),
            SessionIdentity::from(&second),
        ])
        .unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Char('h'));

    press(&mut ui, &snapshot, KeyCode::Char('d'));
    let confirmation = render(&snapshot, &ui, 120, 30).text;
    assert!(confirmation.contains("Permanently Exclude Project Sessions?"));
    assert!(confirmation.contains("2 Sessions"));
    press(&mut ui, &snapshot, KeyCode::Enter);

    let labels = SessionLabels::load(&label_path);
    for session in [&first, &second] {
        assert_eq!(
            labels.disposition(session.provider, &session.id),
            Some(CurationDisposition::PermanentlyExcluded)
        );
    }

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn hidden_tree_project_restore_applies_to_all_current_hidden_sessions() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hidden-project-restore-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let source = production_snapshot();
    let first = source
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    let mut second = first.clone();
    second.provider = Provider::ClaudeCode;
    second.id = "claude-hidden-peer".into();
    let snapshot = Snapshot {
        sessions: vec![first.clone(), second.clone()],
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    let label_path = directory.join("session-labels.json");
    SessionLabels::load(&label_path)
        .hide_sessions(&[
            SessionIdentity::from(&first),
            SessionIdentity::from(&second),
        ])
        .unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Char('h'));

    press(&mut ui, &snapshot, KeyCode::Char('u'));

    let restored = render(&snapshot, &ui, 120, 30).text;
    assert!(!restored.contains("HIDDEN SESSIONS"));
    assert!(restored.contains("2 Sessions restored"));
    let labels = SessionLabels::load(&label_path);
    for session in [&first, &second] {
        assert_eq!(labels.disposition(session.provider, &session.id), None);
    }

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn hidden_exited_visibility_is_consistent_and_only_validated_open_overrides_it() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hidden-projection-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let source = production_snapshot();
    let mut session = source
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    session.last_activity_ms = source.generated_ms - 1_000;
    let signal = RecentSignal::Session {
        observed_at_ms: source.generated_ms - 500,
        provider: session.provider,
        session_id: session.id.clone(),
        kind: RecentSignalKind::SessionStateChanged {
            from: SessionState::Busy,
            to: SessionState::Exited,
        },
    };
    let mut snapshot = Snapshot {
        sessions: vec![session.clone()],
        recent_signals: vec![signal],
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    SessionLabels::load(directory.join("session-labels.json"))
        .hide_sessions(&[SessionIdentity::from(&session)])
        .unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();

    let all_hidden = render(&snapshot, &ui, 120, 30).text;
    assert!(!all_hidden.contains("x-exited"));
    assert!(all_hidden.contains("In 0"));
    assert!(all_hidden.contains("Out 0"));
    assert!(all_hidden.contains("0 Projects"));
    assert!(all_hidden.contains("RECENT SIGNALS 0"));
    press(&mut ui, &snapshot, KeyCode::Char('3'));
    assert!(!render(&snapshot, &ui, 120, 30).text.contains("x-exited"));

    snapshot.sessions[0].last_activity_ms = source.generated_ms + 60_000;
    assert!(!render(&snapshot, &ui, 120, 30).text.contains("x-exited"));

    snapshot.sessions[0].state = SessionState::Busy;
    let open = render(&snapshot, &ui, 120, 30).text;
    assert!(open.contains("x-exited"));
    press(&mut ui, &snapshot, KeyCode::Char('1'));
    let open_all = render(&snapshot, &ui, 120, 30).text;
    assert!(open_all.contains("x-exited"));
    assert!(open_all.contains("Busy -> Exited"));

    snapshot.sessions[0].state = SessionState::Exited;
    assert!(!render(&snapshot, &ui, 120, 30).text.contains("x-exited"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn hidden_curation_is_dormant_while_source_evidence_is_absent_and_resumes_on_return() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-hidden-source-return-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let source = production_snapshot();
    let hidden = source
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    let label_path = directory.join("session-labels.json");
    SessionLabels::load(&label_path)
        .hide_sessions(&[SessionIdentity::from(&hidden)])
        .unwrap();
    let mut ui = Ui::load(&profile_path, None).unwrap();
    let absent = Snapshot {
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };

    press(&mut ui, &absent, KeyCode::Char('h'));
    assert!(render(&absent, &ui, 120, 30)
        .text
        .contains("No hidden sessions"));
    assert_eq!(
        SessionLabels::load(&label_path).disposition(hidden.provider, &hidden.id),
        Some(CurationDisposition::Hidden)
    );

    let returned = Snapshot {
        sessions: vec![hidden],
        revision: 1,
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    press(&mut ui, &returned, KeyCode::Char('h'));
    assert!(render(&returned, &ui, 120, 30)
        .text
        .contains("HIDDEN SESSIONS"));

    std::fs::remove_dir_all(directory).unwrap();
}

fn ui_with_theme(theme: &str) -> Ui {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let profile_path = std::env::temp_dir().join(format!(
        "sessionmesh-production-theme-{}-{}-{}.json",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed),
        theme,
    ));
    std::fs::write(
        &profile_path,
        format!(r#"{{"help_seen":true,"theme":"{theme}"}}"#),
    )
    .unwrap();
    let ui = Ui::load(&profile_path, None).unwrap();
    std::fs::remove_file(profile_path).unwrap();
    ui
}

fn contrast_ratio(style: Style) -> f64 {
    let foreground = luminance(style.fg.expect("rendered role should set a foreground"));
    let background = luminance(style.bg.expect("rendered role should set a background"));
    let (lighter, darker) = if foreground > background {
        (foreground, background)
    } else {
        (background, foreground)
    };
    (lighter + 0.05) / (darker + 0.05)
}

fn luminance(color: Color) -> f64 {
    let Color::Rgb(red, green, blue) = color else {
        panic!("built-in Themes should render RGB colors");
    };
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

fn linear(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
fn exited_session_handoff_requires_explicit_confirmation() {
    let mut snapshot = production_snapshot();
    let mut exited = snapshot
        .sessions
        .iter()
        .find(|session| session.provider == Provider::ClaudeCode && !session.state.is_open())
        .unwrap()
        .clone();
    exited.last_activity_ms = snapshot.generated_ms - 1_000;
    let native_id = exited.id.clone();
    snapshot.sessions = vec![exited];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('2'));
    press(&mut ui, &snapshot, KeyCode::Enter);

    assert!(render(&snapshot, &ui, 120, 30).text.contains("h Handoff"));
    let open_action = ui
        .handle_key(
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
    let confirmation = render(&snapshot, &ui, 120, 30).text;

    assert_eq!(open_action, UiAction::None);
    for required in [
        "Authorize Handoff",
        "Claude Code",
        native_id.as_str(),
        "read-only",
        "continues after SessionMesh closes",
        "Enter Authorize",
        "Esc Cancel",
    ] {
        assert!(
            confirmation.contains(required),
            "missing confirmation text {required:?}\n{confirmation}"
        );
    }

    let cancelled = ui
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert_eq!(cancelled, UiAction::None);
    assert!(!render(&snapshot, &ui, 120, 30)
        .text
        .contains("Authorize one Handoff Operation?"));
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    let confirmed = ui
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    assert_eq!(
        confirmed,
        UiAction::AuthorizeHandoff {
            source_provider: Provider::ClaudeCode,
            source_session_id: native_id,
            successor_provider: Provider::ClaudeCode,
            end_current_run: false,
        }
    );
}

#[test]
fn handoff_authorization_defaults_to_the_source_provider_and_offers_both_successors() {
    let mut snapshot = production_snapshot();
    let mut exited = snapshot
        .sessions
        .iter()
        .find(|session| session.provider == Provider::ClaudeCode && !session.state.is_open())
        .unwrap()
        .clone();
    exited.last_activity_ms = snapshot.generated_ms - 1_000;
    snapshot.sessions = vec![exited];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('2'));
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Char('h'));

    let defaulted = render(&snapshot, &ui, 120, 30).text;
    assert!(defaulted.contains("Successor  [Claude Code]  Codex CLI"));

    press(&mut ui, &snapshot, KeyCode::Right);
    let selected = render(&snapshot, &ui, 120, 30).text;
    assert!(selected.contains("Successor  Claude Code  [Codex CLI]"));

    press(&mut ui, &snapshot, KeyCode::Left);
    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("Successor  [Claude Code]  Codex CLI"));
}

#[test]
fn handoff_authorization_emits_the_selected_successor_provider() {
    let mut snapshot = production_snapshot();
    let mut exited = snapshot
        .sessions
        .iter()
        .find(|session| session.provider == Provider::ClaudeCode && !session.state.is_open())
        .unwrap()
        .clone();
    exited.last_activity_ms = snapshot.generated_ms - 1_000;
    let native_id = exited.id.clone();
    snapshot.sessions = vec![exited];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('2'));
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Char('h'));
    press(&mut ui, &snapshot, KeyCode::Right);

    let confirmed = ui
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    assert_eq!(
        confirmed,
        UiAction::AuthorizeHandoff {
            source_provider: Provider::ClaudeCode,
            source_session_id: native_id,
            successor_provider: Provider::CodexCli,
            end_current_run: false,
        }
    );
}

#[test]
fn open_session_handoff_explicitly_authorizes_ending_only_its_current_run() {
    let snapshot = production_snapshot();
    let source = snapshot
        .sessions
        .iter()
        .find(|session| session.provider == Provider::ClaudeCode && session.state.is_open())
        .unwrap();
    let native_id = source.id.clone();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Enter);

    let open_action = ui
        .handle_key(
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
    let confirmation = render(&snapshot, &ui, 120, 30).text;

    assert_eq!(open_action, UiAction::None);
    for required in [
        "Authorize Handoff",
        "Current Run",
        "Busy",
        "waits until Idle",
        "end this one Current Run",
        "graceful exit",
        "Terminal and shell remain open",
        "Enter Authorize",
        "Esc Cancel",
    ] {
        assert!(
            confirmation.contains(required),
            "missing confirmation text {required:?}\n{confirmation}"
        );
    }
    assert_eq!(
        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap(),
        UiAction::None
    );
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    assert_eq!(
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap(),
        UiAction::AuthorizeHandoff {
            source_provider: Provider::ClaudeCode,
            source_session_id: native_id,
            successor_provider: Provider::ClaudeCode,
            end_current_run: true,
        }
    );
}

#[test]
fn retained_handoff_task_appears_in_the_minimal_upper_right_tasks_surface() {
    let mut snapshot = production_snapshot();
    let queued = OperationTask {
        id: "task-00000001".into(),
        state: OperationTaskState::Queued,
        source_provider: Provider::ClaudeCode,
        source_session_id: "claude-critical".into(),
        source_record_path: PathBuf::from("/private/provider-output-must-not-render"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::from("/usr/local/bin/claude"),
        created_at_ms: snapshot.generated_ms - 1_000,
        state_changed_at_ms: snapshot.generated_ms - 1_000,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: None,
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: None,
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    };
    let mut ready = queued.clone();
    ready.id = "task-00000002".into();
    ready.state = OperationTaskState::Ready;
    ready.state_changed_at_ms += 1;
    ready.artifact_path = Some(PathBuf::from("/private/artifact-must-not-render.md"));
    let mut failed = queued.clone();
    failed.id = "task-00000003".into();
    failed.state = OperationTaskState::Failed;
    failed.state_changed_at_ms += 2;
    failed.failure_code = Some("private-failure-detail-must-not-render".into());
    snapshot.tasks = vec![failed, ready, queued];

    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);
    let mut rendered = String::new();
    for index in 0..snapshot.tasks.len() {
        rendered.push_str(&render(&snapshot, &ui, 120, 30).text);
        if index + 1 < snapshot.tasks.len() {
            press(&mut ui, &snapshot, KeyCode::Down);
        }
    }

    for expected in [
        "TASKS", "Handoff", "✳", "critical", "Queued", "Ready", "Failed",
    ] {
        assert!(
            rendered.contains(expected),
            "missing Task metadata {expected:?}"
        );
    }
    assert!(!rendered.contains("provider-output-must-not-render"));
    assert!(!rendered.contains("artifact-must-not-render"));
    assert!(!rendered.contains("private-failure-detail-must-not-render"));
    let tasks_row = rendered
        .lines()
        .position(|line| line.contains("TASKS"))
        .unwrap();
    let signals_row = rendered
        .lines()
        .position(|line| line.contains("RECENT SIGNALS"))
        .unwrap();
    assert_eq!(signals_row, tasks_row);
}

#[test]
fn task_deletion_confirmation_progress_failure_removal_and_history_render_in_production() {
    let mut snapshot = production_snapshot();
    let task = OperationTask {
        id: "task-delete-render".into(),
        state: OperationTaskState::Ready,
        source_provider: Provider::ClaudeCode,
        source_session_id: "claude-critical".into(),
        source_record_path: PathBuf::from("/private/source.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::CodexCli,
        successor_executable: PathBuf::from("/usr/local/bin/codex"),
        created_at_ms: snapshot.generated_ms - 10_000,
        state_changed_at_ms: snapshot.generated_ms - 2_000,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: Some(PathBuf::from(
            "/private/artifacts/task-delete-render/handoff.md",
        )),
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: Some(snapshot.generated_ms - 1_000),
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    };
    snapshot.tasks = vec![task.clone()];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Char('d'));

    let confirmation = render(&snapshot, &ui, 120, 30).text;
    for expected in [
        "Delete Task and Artifact?",
        "task-delete-render",
        "irreversible",
        "Handoff Artifact",
        "Successor Launch Plan",
        "Enter Delete",
        "Esc Keep Task",
    ] {
        assert!(
            confirmation.contains(expected),
            "missing {expected:?}\n{confirmation}"
        );
    }
    assert_eq!(
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap(),
        UiAction::DeleteTask {
            task_id: task.id.clone()
        }
    );

    let mut deleting = task.clone();
    deleting.deletion_requested_at_ms = Some(snapshot.generated_ms);
    snapshot.tasks = vec![deleting.clone()];
    let in_progress = render(&snapshot, &ui, 120, 30).text;
    assert!(in_progress.contains("In progress"));
    assert!(!in_progress.contains("d Delete"));
    assert!(!in_progress.contains("o Open Terminal"));

    deleting.deletion_failure_code = Some("artifact_remove_failed".into());
    snapshot.tasks = vec![deleting];
    let failed = render(&snapshot, &ui, 120, 30).text;
    assert!(failed.contains("Failed; will retry"));
    assert!(failed.contains("artifact_remove_failed"));

    let mut retained_retry = task.clone();
    retained_retry.id = "retained-retry".into();
    retained_retry.state = OperationTaskState::Failed;
    retained_retry.artifact_path = None;
    retained_retry.retry_of_task_id = Some("deleted-predecessor-uuid".into());
    snapshot.tasks = vec![retained_retry];
    let mut retry_ui = Ui::default();
    press(&mut retry_ui, &snapshot, KeyCode::Tab);
    press(&mut retry_ui, &snapshot, KeyCode::Tab);
    press(&mut retry_ui, &snapshot, KeyCode::Enter);
    let history = render(&snapshot, &retry_ui, 120, 30).text;
    assert!(history.contains("deleted-predecessor-uuid (unavailable)"));

    snapshot.tasks.clear();
    let removed = render(&snapshot, &Ui::default(), 120, 30).text;
    assert!(removed.contains("TASKS 0"));
    assert!(!removed.contains("task-delete-render"));
}

#[test]
fn wide_home_places_status_panels_side_by_side_in_the_upper_right() {
    let mut snapshot = production_snapshot();
    snapshot.tasks = vec![OperationTask {
        id: "task-wide-layout".into(),
        state: OperationTaskState::Queued,
        source_provider: Provider::ClaudeCode,
        source_session_id: "claude-critical".into(),
        source_record_path: PathBuf::from("/private/source.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::from("/usr/local/bin/claude"),
        created_at_ms: snapshot.generated_ms - 1_000,
        state_changed_at_ms: snapshot.generated_ms - 1_000,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: None,
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: None,
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    }];

    let rendered = render(&snapshot, &Ui::default(), 120, 30).text;
    let (tasks_row, tasks_column) = rendered
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find("TASKS")
                .map(|byte| (row, line[..byte].chars().count()))
        })
        .unwrap();
    let (editor_row, editor_column) = rendered
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find("PROJECT")
                .map(|byte| (row, line[..byte].chars().count()))
        })
        .unwrap();
    let (signals_row, signals_column) = rendered
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find("RECENT SIGNALS")
                .map(|byte| (row, line[..byte].chars().count()))
        })
        .unwrap();

    assert_eq!(signals_row, tasks_row);
    assert!(tasks_row < editor_row);
    assert!(tasks_column > editor_column);
    assert!(signals_column > editor_column);
    assert!(tasks_column > signals_column);
    for line in rendered.lines().take(editor_row).skip(signals_row) {
        assert!(
            line.chars()
                .take(signals_column - 2)
                .all(|character| character == ' '),
            "upper-left region is not empty: {line:?}"
        );
    }
}

#[test]
fn compact_tasks_can_receive_focus_and_scroll_to_every_retained_task() {
    let mut snapshot = production_snapshot();
    snapshot.tasks = (0..8)
        .map(|index| OperationTask {
            id: format!("task-{index:02}"),
            state: OperationTaskState::Queued,
            source_provider: Provider::ClaudeCode,
            source_session_id: format!("source-{index:02}"),
            source_record_path: PathBuf::from("/private/source.jsonl"),
            source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
            successor_provider: Provider::ClaudeCode,
            successor_executable: PathBuf::from("/usr/local/bin/claude"),
            created_at_ms: snapshot.generated_ms - index * 1_000,
            state_changed_at_ms: snapshot.generated_ms - index * 1_000,
            worker_process_identity: None,
            provider_process_identity: None,
            prompt_sent_at_ms: None,
            provider_exited_at_ms: None,
            provider_exit_code: None,
            running_deadline_ms: None,
            cancellation_requested_at_ms: None,
            termination_requested_at_ms: None,
            retry_of_task_id: None,
            retry_ready_at_ms: None,
            artifact_path: None,
            successor_launch_plan: None,
            successor_launch_metadata: None,
            successor_session_id: None,
            successor_linked_at_ms: None,
            failure_code: None,
            acknowledged_at_ms: None,
            end_current_run_authorized: false,
            source_process_identity: None,
            waiting_reason: None,
            waiting_deadline_ms: None,
            control_used_at_ms: None,
            deletion_requested_at_ms: None,
            deletion_failure_code: None,
        })
        .collect();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);
    for _ in 0..7 {
        press(&mut ui, &snapshot, KeyCode::Down);
    }

    let rendered = render(&snapshot, &ui, 80, 24);

    assert!(rendered.text.contains("TASKS"));
    assert!(rendered.text.contains("ource-07"));
    assert!(!rendered.text.contains("ource-00"));
    assert_eq!(
        rendered.style_at("ource-07").bg,
        Some(Color::Rgb(32, 35, 38))
    );
}

#[test]
fn task_rows_show_transition_time_and_unread_only_for_unacknowledged_terminal_outcomes() {
    let mut snapshot = production_snapshot();
    let ready = OperationTask {
        id: "task-ready".into(),
        state: OperationTaskState::Ready,
        source_provider: Provider::ClaudeCode,
        source_session_id: "ready-id".into(),
        source_record_path: PathBuf::from("/private/source.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::from("/usr/local/bin/claude"),
        created_at_ms: snapshot.generated_ms - 10_000,
        state_changed_at_ms: snapshot.generated_ms - 1_000,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: Some(PathBuf::from("/private/artifact.md")),
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: None,
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    };
    let mut failed = ready.clone();
    failed.id = "task-failed".into();
    failed.state = OperationTaskState::Failed;
    failed.source_provider = Provider::CodexCli;
    failed.source_session_id = "failedid".into();
    failed.state_changed_at_ms = snapshot.generated_ms - 2_000;
    failed.artifact_path = None;
    failed.failure_code = Some("provider_invocation_failed".into());
    failed.acknowledged_at_ms = Some(snapshot.generated_ms - 500);
    snapshot.tasks = vec![ready, failed];

    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);
    let ready_rendered = render(&snapshot, &ui, 120, 30).text;
    press(&mut ui, &snapshot, KeyCode::Down);
    let failed_rendered = render(&snapshot, &ui, 120, 30).text;
    let ready_row = ready_rendered
        .lines()
        .find(|line| line.contains("ready-id"))
        .unwrap();
    let failed_row = failed_rendered
        .lines()
        .find(|line| line.contains("failedid"))
        .unwrap();

    for expected in ["NEW", "Handoff", "✳:ready-id", "Ready", "1s"] {
        assert!(
            ready_row.contains(expected),
            "missing {expected:?}: {ready_row}"
        );
    }
    for expected in ["Handoff", "⬡:failedid", "Failed", "2s"] {
        assert!(
            failed_row.contains(expected),
            "missing {expected:?}: {failed_row}"
        );
    }
    assert!(!failed_row.contains("NEW"));
}

#[test]
fn task_rows_resolve_the_current_session_alias_without_changing_task_identity() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-task-title-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let mut snapshot = production_snapshot();
    snapshot.tasks = vec![OperationTask {
        id: "task-session-title".into(),
        state: OperationTaskState::Queued,
        source_provider: Provider::ClaudeCode,
        source_session_id: "claude-critical".into(),
        source_record_path: PathBuf::from("/private/source.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::from("/usr/local/bin/claude"),
        created_at_ms: snapshot.generated_ms - 1_000,
        state_changed_at_ms: snapshot.generated_ms - 1_000,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: None,
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: None,
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    }];
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Release coordinator".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);

    let rendered = render(&snapshot, &ui, 80, 24).text;

    assert!(rendered.contains("✳:...oordinator"));
    assert!(!rendered.contains("✳:critical"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn task_rows_disambiguate_colliding_session_labels() {
    let mut snapshot = production_snapshot();
    let task = OperationTask {
        id: "task-a".into(),
        state: OperationTaskState::Queued,
        source_provider: Provider::CodexCli,
        source_session_id: "codex-Asame0000".into(),
        source_record_path: PathBuf::from("/private/source.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::CodexCli,
        successor_executable: PathBuf::from("/usr/local/bin/codex"),
        created_at_ms: snapshot.generated_ms - 1_000,
        state_changed_at_ms: snapshot.generated_ms - 1_000,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: None,
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: None,
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    };
    let mut other = task.clone();
    other.id = "task-b".into();
    other.source_session_id = "codex-Bsame0000".into();
    snapshot.tasks = vec![task, other];

    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);
    let first = render(&snapshot, &ui, 80, 24).text;
    press(&mut ui, &snapshot, KeyCode::Down);
    let second = render(&snapshot, &ui, 80, 24).text;

    assert!(first.contains("⬡:Asame0000"));
    assert!(second.contains("⬡:Bsame0000"));
}

#[test]
fn opening_task_detail_requests_deliberate_acknowledgement_for_an_unread_terminal_task() {
    let mut snapshot = production_snapshot();
    snapshot.tasks = vec![OperationTask {
        id: "task-open-detail".into(),
        state: OperationTaskState::Ready,
        source_provider: Provider::ClaudeCode,
        source_session_id: "ready-id".into(),
        source_record_path: PathBuf::from("/private/source-must-not-render.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::from("/usr/local/bin/claude"),
        created_at_ms: snapshot.generated_ms - 10_000,
        state_changed_at_ms: snapshot.generated_ms - 1_000,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: Some(PathBuf::from("/private/artifact-must-not-render.md")),
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: None,
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    }];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);

    let action = ui
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    assert_eq!(
        action,
        UiAction::AcknowledgeTask {
            task_id: "task-open-detail".into()
        }
    );
    let detail = render(&snapshot, &ui, 120, 30).text;
    assert!(detail.contains("Task detail"));
    assert!(detail.contains("task-open-detail"));
    assert!(!detail.contains("RECENT SIGNALS"));
}

#[test]
fn failed_task_detail_explains_identity_timing_failure_result_and_available_actions() {
    let mut snapshot = production_snapshot();
    snapshot.tasks = vec![OperationTask {
        id: "task-failed-detail".into(),
        state: OperationTaskState::Failed,
        source_provider: Provider::CodexCli,
        source_session_id: "codex-failed-session".into(),
        source_record_path: PathBuf::from("/private/provider-record.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::CodexCli,
        successor_executable: PathBuf::from("/usr/local/bin/codex"),
        created_at_ms: snapshot.generated_ms - 10_000,
        state_changed_at_ms: snapshot.generated_ms - 2_000,
        worker_process_identity: Some(ProcessIdentity {
            pid: 42,
            started_at_ms: snapshot.generated_ms - 9_000,
        }),
        provider_process_identity: Some(ProcessIdentity {
            pid: 43,
            started_at_ms: snapshot.generated_ms - 5_000,
        }),
        prompt_sent_at_ms: Some(snapshot.generated_ms - 4_000),
        provider_exited_at_ms: Some(snapshot.generated_ms - 2_000),
        provider_exit_code: Some(23),
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: None,
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: Some("provider_invocation_failed".into()),
        acknowledged_at_ms: None,
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    }];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    let detail = render(&snapshot, &ui, 120, 30).text;

    for expected in [
        "Task detail",
        "task-failed-detail",
        "Handoff",
        "Source Provider",
        "Codex CLI",
        "codex-failed-session",
        "/Users/fixture/code/sessionmesh",
        "Successor Provider",
        "Failed",
        "Created",
        "Latest transition",
        "Execution trace",
        "Worker started",
        "Provider started",
        "Prompt sent",
        "Provider exited",
        "Exit code",
        "23",
        "provider_invocation_failed",
        "Provider invocation failed.",
        "Result",
        "Not ready",
        "Esc Back",
    ] {
        assert!(detail.contains(expected), "missing {expected:?}\n{detail}");
    }
    assert!(!detail.contains("provider-record.jsonl"));
}

#[test]
fn waiting_task_detail_shows_its_reason_and_fixed_authorization_deadline() {
    let mut snapshot = production_snapshot();
    let deadline_ms = snapshot.generated_ms + 7_200_000;
    snapshot.tasks = vec![OperationTask {
        id: "task-waiting-detail".into(),
        state: OperationTaskState::Waiting,
        source_provider: Provider::ClaudeCode,
        source_session_id: "claude-critical".into(),
        source_record_path: PathBuf::from("/private/provider-record-must-not-render.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::from("/usr/local/bin/claude"),
        created_at_ms: snapshot.generated_ms,
        state_changed_at_ms: snapshot.generated_ms,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: None,
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: None,
        end_current_run_authorized: true,
        source_process_identity: Some(ProcessIdentity {
            pid: 42,
            started_at_ms: snapshot.generated_ms - 1_000,
        }),
        waiting_reason: Some("source_current_run_busy".into()),
        waiting_deadline_ms: Some(deadline_ms),
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    }];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);

    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let detail = render(&snapshot, &ui, 120, 30).text;

    for expected in [
        "Waiting",
        "Waiting reason",
        "Source Current Run is Busy",
        "Authorization deadline",
        &sessionmesh::app::fmt_datetime(deadline_ms),
    ] {
        assert!(detail.contains(expected), "missing {expected:?}\n{detail}");
    }
    assert!(!detail.contains("provider-record-must-not-render"));
    assert!(!detail.contains("PID 42"));
}

#[test]
fn expired_waiting_authority_is_an_unread_cancelled_task_with_a_clear_explanation() {
    let mut snapshot = production_snapshot();
    let deadline_ms = snapshot.generated_ms - 1_000;
    snapshot.tasks = vec![OperationTask {
        id: "task-cancelled-detail".into(),
        state: OperationTaskState::Cancelled,
        source_provider: Provider::CodexCli,
        source_session_id: "codex-idle".into(),
        source_record_path: PathBuf::from("/private/rollout.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::CodexCli,
        successor_executable: PathBuf::from("/usr/local/bin/codex"),
        created_at_ms: snapshot.generated_ms - 7_201_000,
        state_changed_at_ms: deadline_ms,
        worker_process_identity: None,
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: None,
        successor_launch_plan: None,
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: Some("current_run_authority_expired".into()),
        acknowledged_at_ms: None,
        end_current_run_authorized: true,
        source_process_identity: Some(ProcessIdentity {
            pid: 42,
            started_at_ms: snapshot.generated_ms - 8_000_000,
        }),
        waiting_reason: Some("source_current_run_busy".into()),
        waiting_deadline_ms: Some(deadline_ms),
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    }];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);
    let tasks = render(&snapshot, &ui, 120, 30).text;

    let cancelled_row = tasks
        .lines()
        .find(|line| line.contains("Cancelled"))
        .unwrap();
    for expected in ["NEW", "Handoff", "⬡:", "Cancelled"] {
        assert!(
            cancelled_row.contains(expected),
            "missing {expected:?}: {cancelled_row}"
        );
    }
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let detail = render(&snapshot, &ui, 120, 30).text;

    for expected in [
        "Cancelled",
        "current_run_authority_expired",
        "Unused Current Run authority expired at its deadline",
        "Authorization deadline",
    ] {
        assert!(detail.contains(expected), "missing {expected:?}\n{detail}");
    }
}

#[test]
fn close_for_later_returns_to_tasks_without_hiding_or_invalidating_the_ready_result() {
    let mut snapshot = production_snapshot();
    let artifact_path = PathBuf::from("/private/handoff.md");
    snapshot.tasks = vec![OperationTask {
        id: "task-ready-later".into(),
        state: OperationTaskState::Ready,
        source_provider: Provider::ClaudeCode,
        source_session_id: "ready-id".into(),
        source_record_path: PathBuf::from("/private/source.jsonl"),
        source_working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
        successor_provider: Provider::ClaudeCode,
        successor_executable: PathBuf::from("/usr/local/bin/claude"),
        created_at_ms: snapshot.generated_ms - 10_000,
        state_changed_at_ms: snapshot.generated_ms - 2_000,
        worker_process_identity: Some(ProcessIdentity {
            pid: 42,
            started_at_ms: snapshot.generated_ms - 2_000,
        }),
        provider_process_identity: None,
        prompt_sent_at_ms: None,
        provider_exited_at_ms: None,
        provider_exit_code: None,
        running_deadline_ms: None,
        cancellation_requested_at_ms: None,
        termination_requested_at_ms: None,
        retry_of_task_id: None,
        retry_ready_at_ms: None,
        artifact_path: Some(artifact_path.clone()),
        successor_launch_plan: Some(SuccessorLaunchPlan {
            successor_provider: Provider::ClaudeCode,
            working_directory: PathBuf::from("/Users/fixture/code/sessionmesh"),
            executable: PathBuf::from("/usr/local/bin/claude"),
            arguments: vec!["Read the Handoff Artifact and continue.".into()],
            artifact_path: artifact_path.clone(),
        }),
        successor_launch_metadata: None,
        successor_session_id: None,
        successor_linked_at_ms: None,
        failure_code: None,
        acknowledged_at_ms: Some(snapshot.generated_ms - 1_000),
        end_current_run_authorized: false,
        source_process_identity: None,
        waiting_reason: None,
        waiting_deadline_ms: None,
        control_used_at_ms: None,
        deletion_requested_at_ms: None,
        deletion_failure_code: None,
    }];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);
    assert_eq!(
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap(),
        UiAction::None
    );
    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("Esc Close for Later"));

    let close = ui
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let home = render(&snapshot, &ui, 120, 30).text;

    assert_eq!(close, UiAction::None);
    assert!(home.contains("TASKS"));
    assert!(home.contains("ready-id"));
    assert!(home.contains("Ready"));
    assert_eq!(
        snapshot.tasks[0].artifact_path.as_ref(),
        Some(&artifact_path)
    );
    assert!(snapshot.tasks[0].successor_launch_plan.is_some());
}

#[test]
fn terminal_below_the_hard_minimum_renders_only_the_too_small_state() {
    let rendered = render(&Snapshot::default(), &Ui::default(), 79, 23);

    assert_eq!((rendered.width, rendered.height), (79, 23));
    for required in [
        "Terminal too small",
        "Required: 80x24",
        "Current: 79x23",
        "q Quit",
    ] {
        assert!(
            rendered.text.contains(required),
            "missing required fallback text: {required:?}"
        );
    }
    for normal_ui in [
        "SessionMesh",
        "1 2 3",
        " PROJECT ",
        " RECENT SIGNALS ",
        "Theme",
    ] {
        assert!(
            !rendered.text.contains(normal_ui),
            "normal UI leaked into the fallback: {normal_ui:?}"
        );
    }
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/too_small_79x23.txt"),
    );
}

#[test]
fn global_header_combines_navigation_with_indexed_usage_and_project_totals() {
    let snapshot = production_snapshot();

    let wide = render(&snapshot, &Ui::default(), 120, 30);
    let wide_header = wide.text.lines().next().unwrap();
    assert!(wide_header.contains(&format!(
        "SessionMesh | v{} | 1 2 3",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(!wide_header.contains("1 All"));
    assert!(!wide_header.contains("2 Claude"));
    assert!(!wide_header.contains("3 Codex"));
    assert!(wide_header.contains("In 86.4k  Out 16.4k | 1 Project"));
    let tasks_row = wide
        .text
        .lines()
        .position(|line| line.contains("TASKS"))
        .unwrap();
    let session_tree_row = wide
        .text
        .lines()
        .position(|line| line.contains("PROJECT"))
        .unwrap();
    let signals_row = wide
        .text
        .lines()
        .position(|line| line.contains("RECENT SIGNALS"))
        .unwrap();
    assert_eq!(signals_row, tasks_row);
    assert!(tasks_row < session_tree_row);

    let compact = render(&snapshot, &Ui::default(), 80, 24);
    let compact_header = compact.text.lines().next().unwrap();
    assert!(compact_header.contains(&format!(
        "SessionMesh | v{} | 1 2 3",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(compact_header.contains("In 86.4k Out 16.4k | P 1"));
    assert!(compact
        .text
        .lines()
        .nth(1)
        .is_some_and(|line| line.contains("RECENT SIGNALS")));
    assert!(compact
        .text
        .lines()
        .nth(1)
        .is_some_and(|line| line.contains("TASKS")));
    assert!(compact
        .text
        .lines()
        .nth(9)
        .is_some_and(|line| line.contains("PROJECT")));
}

#[test]
fn all_page_frame_titles_summarize_open_work_and_signal_freshness() {
    let snapshot = production_snapshot();
    let rendered = render(&snapshot, &Ui::default(), 120, 30);
    for expected in ["PROJECT 1", "5 OPEN", "RECENT SIGNALS 5", "latest 5s"] {
        assert!(
            rendered.text.lines().any(|line| line.contains(expected)),
            "missing frame summary: {expected}"
        );
    }

    let mut empty = snapshot;
    empty.recent_signals.clear();
    let empty = render(&empty, &Ui::default(), 80, 24);
    let signal_title = empty
        .text
        .lines()
        .find(|line| line.contains("RECENT SIGNALS"))
        .unwrap();
    assert!(signal_title.contains("RECENT SIGNALS 0"));
    assert!(!signal_title.contains("latest"));
}

#[test]
fn overview_focus_and_retained_selection_render_across_themes_and_target_sizes() {
    for theme in [
        "sessionmesh",
        "catppuccin",
        "dracula",
        "gruvbox",
        "light",
        "high-contrast",
    ] {
        for (width, height) in [(120, 30), (80, 24)] {
            let mut snapshot = production_snapshot();
            snapshot.tasks = vec![focus_fixture_task(snapshot.generated_ms)];
            let mut ui = ui_with_theme(theme);

            let project = render(&snapshot, &ui, width, height);
            assert_overview_focus(&project, 0);

            press(&mut ui, &snapshot, KeyCode::Tab);
            let signals = render(&snapshot, &ui, width, height);
            assert_overview_focus(&signals, 1);

            press(&mut ui, &snapshot, KeyCode::Tab);
            let tasks = render(&snapshot, &ui, width, height);
            assert_overview_focus(&tasks, 2);

            for rendered in [&project, &signals, &tasks] {
                assert!(rendered
                    .text
                    .lines()
                    .all(|line| line.chars().count() == width as usize));
            }

            ui.handle_key(
                KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
                &snapshot,
            )
            .unwrap();
            assert_overview_focus(&render(&snapshot, &ui, width, height), 1);
            ui.handle_key(
                KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
                &snapshot,
            )
            .unwrap();
            assert_overview_focus(&render(&snapshot, &ui, width, height), 0);
        }
    }
}

#[test]
fn all_page_labels_the_session_tree_and_uses_theme_foreground_for_project_names() {
    let snapshot = production_snapshot();
    let ui = Ui::default();

    let dark = render(&snapshot, &ui, 120, 30);
    assert!(dark.text.lines().any(|line| line.contains(" PROJECT ")));
    assert!(dark.text.lines().any(|line| line.contains("▾ sessionmesh")));
    assert!(!dark.text.contains("PROJECT  sessionmesh"));
    let dark_project = dark.style_at("sessionmesh");
    assert_eq!(dark_project.fg, Some(Color::Rgb(224, 220, 210)));
    assert!(dark_project.add_modifier.contains(Modifier::BOLD));

    let light_ui = ui_with_theme("light");
    let light = render(&snapshot, &light_ui, 120, 30);
    let light_project = light.style_at("sessionmesh");
    assert_eq!(light_project.fg, Some(Color::Rgb(28, 31, 34)));
    assert!(light_project.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn all_page_current_context_percentages_use_one_color_for_each_semantic_role() {
    let rendered = render(&production_snapshot(), &Ui::default(), 120, 30);

    for (session, percent, expected_color) in [
        ("x-shared", "40%", Color::Rgb(120, 200, 154)),
        ("-warning", "75%", Color::Rgb(238, 190, 70)),
        ("critical", "90%", Color::Rgb(255, 95, 109)),
    ] {
        assert_eq!(
            rendered.style_at_in_row(session, percent).fg,
            Some(expected_color)
        );
    }
}

#[test]
fn all_page_keeps_multiple_worktrees_below_the_project_without_context() {
    let rendered = render(&production_snapshot(), &Ui::default(), 120, 30);
    let worktree = rendered
        .text
        .lines()
        .find(|line| line.contains("▾ main !2"))
        .expect("All page should render the shared Worktree below its Project");

    assert!(worktree.starts_with("  "));
    assert!(!worktree.starts_with('│'));
    assert!(worktree.contains("main"));
    assert!(!worktree.contains("/Users"));
    assert!(!worktree.contains("Context"));
}

#[test]
fn all_page_abbreviates_the_current_users_home_in_worktree_paths() {
    let home = dirs::home_dir().expect("test environment should have a home directory");
    let root = home.join("code/sessionmesh");
    let root_text = root.to_string_lossy();
    let session = fixture_session(
        Provider::ClaudeCode,
        "home-relative",
        "claude-opus-4-8",
        SessionState::Busy,
        100_000,
        1_700_000_000_000,
        (&root_text, "main"),
    );
    let snapshot = Snapshot {
        sessions: vec![session],
        generated_ms: 1_700_000_000_000,
        ready: true,
        ..Snapshot::default()
    };

    let rendered = render(&snapshot, &Ui::default(), 120, 30);

    assert!(rendered.text.contains("▾ sessionmesh (1)"));
    assert!(rendered.text.contains("~/code/sessionmesh @ main"));
    assert!(!rendered
        .text
        .lines()
        .find(|line| line.contains("▾ sessionmesh"))
        .unwrap()
        .contains("Context"));
    assert!(!rendered.text.contains(root_text.as_ref()));
}

#[test]
fn production_render_uses_the_snapshot_time_for_relative_session_age() {
    let generated_ms = 1_700_000_000_000;
    let snapshot = Snapshot {
        sessions: vec![Session {
            provider: Provider::ClaudeCode,
            id: "stable-clock-session".into(),
            provenance: sessionmesh::domain::SessionProvenance::Observed,
            source: Default::default(),
            project: Some(Project {
                identity: ProjectIdentity::GitCommonDir(PathBuf::from(
                    "/fixtures/stable-clock/.git",
                )),
                label: "stable-clock".into(),
            }),
            worktree: Some(Worktree {
                root: PathBuf::from("/fixtures/stable-clock"),
                branch: Some("main".into()),
                writable: true,
            }),
            cwd: Some("/fixtures/stable-clock".into()),
            latest_model: Some("stable-clock-model".into()),
            state: SessionState::Busy,
            context: ContextState {
                tokens: 100_000,
                window: Some(200_000),
            },
            usage: Default::default(),
            per_model: vec![],
            current_run_usage: None,
            compactions: vec![],
            last_activity_ms: generated_ms - 120_000,
            cost: None,
        }],
        generated_ms,
        ready: true,
        ..Snapshot::default()
    };

    let rendered = render(&snapshot, &Ui::default(), 120, 30);
    let session_row = rendered
        .text
        .lines()
        .find(|line| line.contains("✳ -session"))
        .unwrap_or_else(|| {
            panic!(
                "stable clock Session should be rendered:\n{}",
                rendered.text
            )
        });

    assert!(
        session_row.contains("2m"),
        "Session age should be relative to Snapshot generation: {session_row:?}"
    );
}

#[test]
fn full_all_page_matches_the_reviewed_production_fixture() {
    let rendered = render(&production_snapshot(), &Ui::default(), 120, 30);

    assert_eq!((rendered.width, rendered.height), (120, 30));
    let lines = rendered.text.lines().collect::<Vec<_>>();
    assert!(lines[1].contains("RECENT SIGNALS"));
    assert!(lines[1].contains("TASKS"));
    assert!(lines[9].contains("PROJECT"));
    assert!(lines[9].contains("Session Workspace"));
    assert_ne!(lines[9].chars().next(), Some('┌'));
    assert_ne!(lines[9].chars().last(), Some('┐'));
    assert_eq!(lines[9].chars().nth(55), Some('│'));
    for line in &lines[1..9] {
        assert!(
            line.chars().take(40).all(|character| character == ' '),
            "upper-left status region should remain empty: {line:?}"
        );
    }
    for required in [
        "90%",
        "main !2",
        "75%",
        "○",
        "Conversation unavailable.",
        "DETAILS",
    ] {
        assert!(
            rendered.text.contains(required),
            "full All page should preserve {required:?} evidence"
        );
    }
    for speculative in ["FOCUS", "PROJECT PULSE", "SIGNAL HISTORY", "Signal History"] {
        assert!(
            !rendered.text.contains(speculative),
            "unassigned main-screen region acquired {speculative:?}"
        );
    }
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/all_populated_120x30.txt"),
    );
}

#[test]
fn overview_workspace_uses_one_split_surface_with_a_shared_bottom_rule() {
    let rendered = render(&production_snapshot(), &Ui::default(), 120, 30);
    let lines = rendered.text.lines().collect::<Vec<_>>();

    assert!(lines[9].contains("PROJECT"));
    assert!(lines[9].contains("Session Workspace"));
    assert_ne!(lines[9].chars().next(), Some('├'));
    assert_ne!(lines[9].chars().next(), Some('┌'));
    assert_ne!(lines[9].chars().last(), Some('┤'));
    assert_ne!(lines[9].chars().last(), Some('┐'));
    for line in &lines[9..28] {
        assert_eq!(line.chars().nth(55), Some('│'), "{line:?}");
    }
    let bottom = lines[28].chars().collect::<Vec<_>>();
    assert!(bottom[..55].iter().all(|character| *character == '─'));
    assert_eq!(bottom[55], '┴');
    assert!(bottom[56..].iter().all(|character| *character == '─'));
}

#[test]
fn workspace_and_details_headings_keep_horizontal_section_rules() {
    let rendered = render(&production_snapshot(), &Ui::default(), 120, 30);
    let lines = rendered.text.lines().collect::<Vec<_>>();
    let (project_header, workspace_header) = lines[9].split_once('│').unwrap();
    let details_header = lines
        .iter()
        .find(|line| line.contains("DETAILS"))
        .unwrap()
        .chars()
        .take(55)
        .collect::<String>();

    assert!(project_header.contains("PROJECT 1"), "{project_header:?}");
    assert!(
        !project_header.contains("PROJECT TREE"),
        "{project_header:?}"
    );
    assert!(project_header.contains('─'), "{project_header:?}");
    assert!(workspace_header.contains('─'), "{workspace_header:?}");
    assert!(details_header.contains('─'), "{details_header:?}");
}

#[test]
fn selected_session_details_start_immediately_and_use_content_height() {
    let rendered = render(&production_snapshot(), &Ui::default(), 120, 30);
    let lines = rendered.text.lines().collect::<Vec<_>>();
    let details_row = lines
        .iter()
        .position(|line| line.contains("DETAILS"))
        .unwrap();
    let left_pane = |row: usize| lines[row].split_once('│').unwrap().0;

    assert!(left_pane(details_row + 1).starts_with(" Title"));
    assert_eq!(rendered.height as usize - 1 - details_row, 9);
    assert!(!rendered
        .style_at_in_row("Provider    Claude Code -> claude-opus-4-8", "Provider")
        .add_modifier
        .contains(Modifier::BOLD));
    assert!(!rendered
        .style_at_in_row("Provider    Claude Code -> claude-opus-4-8", "Claude Code")
        .add_modifier
        .contains(Modifier::BOLD));
}

#[test]
fn mouse_wheel_navigates_only_when_pointer_is_over_project_rows() {
    let snapshot = production_snapshot();
    let terminal_area = ratatui::layout::Rect::new(0, 0, 120, 30);
    let mut ui = Ui::default();
    let send_wheel = |ui: &mut Ui, kind, column, row| {
        ui.handle_event(
            Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }),
            terminal_area,
            &snapshot,
        )
        .unwrap();
    };
    let assert_selected = |ui: &Ui, label: &str| {
        let rendered = render(&snapshot, ui, 120, 30);
        let row = rendered
            .text
            .lines()
            .find(|row| {
                row.split_once('│')
                    .is_some_and(|(project, _)| project.contains(label))
            })
            .unwrap();
        assert!(row.contains("> "), "{row:?}");
    };

    assert_selected(&ui, "critical");
    send_wheel(&mut ui, MouseEventKind::ScrollDown, 10, 12);
    assert_selected(&ui, "x-shared");

    send_wheel(&mut ui, MouseEventKind::ScrollDown, 70, 12);
    send_wheel(&mut ui, MouseEventKind::ScrollDown, 10, 26);
    assert_selected(&ui, "x-shared");

    send_wheel(&mut ui, MouseEventKind::ScrollUp, 10, 12);
    assert_selected(&ui, "critical");
}

#[test]
fn minimum_width_header_keeps_four_digit_project_and_open_counts_distinct() {
    let sessions = (0..1000)
        .map(|index| {
            let mut session = fixture_session(
                Provider::ClaudeCode,
                &format!("session-{index}"),
                "test-model",
                SessionState::Busy,
                10_000,
                1_700_000_000_000 - i64::from(index),
                (&format!("/fixtures/project-{index}"), "main"),
            );
            session.project = Some(Project {
                identity: ProjectIdentity::GitCommonDir(PathBuf::from(format!(
                    "/fixtures/project-{index}/.git"
                ))),
                label: format!("project-{index}"),
            });
            session
        })
        .collect();
    let snapshot = Snapshot {
        sessions,
        generated_ms: 1_700_000_000_000,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();
    let terminal_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
    ] {
        ui.handle_event(
            Event::Mouse(MouseEvent {
                kind,
                column: if matches!(kind, MouseEventKind::Down(_)) {
                    31
                } else {
                    0
                },
                row: 12,
                modifiers: KeyModifiers::NONE,
            }),
            terminal_area,
            &snapshot,
        )
        .unwrap();
    }

    let rendered = render(&snapshot, &ui, 80, 24);
    let left_header = rendered
        .text
        .lines()
        .nth(9)
        .unwrap()
        .chars()
        .take(24)
        .collect::<String>();
    assert!(left_header.contains("PROJECT"), "{left_header:?}");
    assert!(left_header.contains("P1000"), "{left_header:?}");
    assert!(left_header.contains("O1000"), "{left_header:?}");
}

#[test]
fn dragging_the_overview_divider_resizes_the_complete_workspace() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    let terminal_area = ratatui::layout::Rect::new(0, 0, 120, 30);

    ui.handle_event(
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 55,
            row: 12,
            modifiers: KeyModifiers::NONE,
        }),
        terminal_area,
        &snapshot,
    )
    .unwrap();
    ui.handle_event(
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 70,
            row: 12,
            modifiers: KeyModifiers::NONE,
        }),
        terminal_area,
        &snapshot,
    )
    .unwrap();

    let rendered = render(&snapshot, &ui, 120, 30);
    let lines = rendered.text.lines().collect::<Vec<_>>();
    for line in &lines[9..28] {
        assert_eq!(line.chars().nth(70), Some('│'), "{line:?}");
    }
    assert_eq!(lines[28].chars().nth(70), Some('┴'));
    assert_eq!(
        rendered.buffer[(70, 12)].style().fg,
        Some(Color::Rgb(230, 180, 80))
    );
}

#[test]
fn divider_clamps_to_both_pane_minima_and_stops_after_release() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    let terminal_area = ratatui::layout::Rect::new(0, 0, 120, 30);
    let send_mouse = |ui: &mut Ui, kind, column| {
        ui.handle_event(
            Event::Mouse(MouseEvent {
                kind,
                column,
                row: 12,
                modifiers: KeyModifiers::NONE,
            }),
            terminal_area,
            &snapshot,
        )
        .unwrap();
    };

    send_mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 55);
    send_mouse(&mut ui, MouseEventKind::Drag(MouseButton::Left), 0);
    send_mouse(&mut ui, MouseEventKind::Up(MouseButton::Left), 0);
    send_mouse(&mut ui, MouseEventKind::Drag(MouseButton::Left), 70);
    let left_minimum = render(&snapshot, &ui, 120, 30);
    assert!(left_minimum
        .text
        .lines()
        .skip(9)
        .take(19)
        .all(|line| line.chars().nth(24) == Some('│')));

    send_mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 24);
    send_mouse(&mut ui, MouseEventKind::Drag(MouseButton::Left), 119);
    let right_minimum = render(&snapshot, &ui, 120, 30);
    assert!(right_minimum
        .text
        .lines()
        .skip(9)
        .take(19)
        .all(|line| line.chars().nth(71) == Some('│')));
}

#[test]
fn mouse_activity_outside_the_visible_workspace_does_not_resize_it() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    let terminal_area = ratatui::layout::Rect::new(0, 0, 120, 30);
    let send_mouse = |ui: &mut Ui, kind, column| {
        ui.handle_event(
            Event::Mouse(MouseEvent {
                kind,
                column,
                row: 12,
                modifiers: KeyModifiers::NONE,
            }),
            terminal_area,
            &snapshot,
        )
        .unwrap();
    };

    send_mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 54);
    send_mouse(&mut ui, MouseEventKind::Drag(MouseButton::Left), 70);
    ui.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)),
        terminal_area,
        &snapshot,
    )
    .unwrap();
    send_mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 55);
    send_mouse(&mut ui, MouseEventKind::Drag(MouseButton::Left), 70);
    ui.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        terminal_area,
        &snapshot,
    )
    .unwrap();

    let rendered = render(&snapshot, &ui, 120, 30);
    assert!(rendered
        .text
        .lines()
        .skip(9)
        .take(19)
        .all(|line| line.chars().nth(55) == Some('│')));
}

#[test]
fn non_primary_other_tab_and_detail_mouse_events_do_not_resize_the_overview() {
    let snapshot = production_snapshot();
    let terminal_area = ratatui::layout::Rect::new(0, 0, 120, 30);
    let send_mouse = |ui: &mut Ui, kind| {
        ui.handle_event(
            Event::Mouse(MouseEvent {
                kind,
                column: 55,
                row: 12,
                modifiers: KeyModifiers::NONE,
            }),
            terminal_area,
            &snapshot,
        )
        .unwrap();
        ui.handle_event(
            Event::Mouse(MouseEvent {
                kind: match kind {
                    MouseEventKind::Down(button) => MouseEventKind::Drag(button),
                    _ => kind,
                },
                column: 70,
                row: 12,
                modifiers: KeyModifiers::NONE,
            }),
            terminal_area,
            &snapshot,
        )
        .unwrap();
    };
    let assert_default_split = |ui: &Ui| {
        let rendered = render(&snapshot, ui, 120, 30);
        assert!(rendered
            .text
            .lines()
            .skip(9)
            .take(19)
            .all(|line| line.chars().nth(55) == Some('│')));
    };

    let mut ui = Ui::default();
    send_mouse(&mut ui, MouseEventKind::Down(MouseButton::Right));
    assert_default_split(&ui);

    ui.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE)),
        terminal_area,
        &snapshot,
    )
    .unwrap();
    send_mouse(&mut ui, MouseEventKind::Down(MouseButton::Left));
    ui.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE)),
        terminal_area,
        &snapshot,
    )
    .unwrap();
    assert_default_split(&ui);

    ui.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        terminal_area,
        &snapshot,
    )
    .unwrap();
    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("Session Detail"));
    send_mouse(&mut ui, MouseEventKind::Down(MouseButton::Left));
    ui.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        terminal_area,
        &snapshot,
    )
    .unwrap();
    assert_default_split(&ui);
}

#[test]
fn alt_arrow_resizes_without_replacing_project_tree_navigation() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    let terminal_area = ratatui::layout::Rect::new(0, 0, 120, 30);

    ui.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)),
        terminal_area,
        &snapshot,
    )
    .unwrap();
    let resized = render(&snapshot, &ui, 120, 30);
    assert!(resized
        .text
        .lines()
        .skip(9)
        .take(19)
        .all(|line| line.chars().nth(56) == Some('│')));

    ui.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
        terminal_area,
        &snapshot,
    )
    .unwrap();
    let navigated = render(&snapshot, &ui, 120, 30);
    assert!(navigated
        .text
        .lines()
        .skip(9)
        .take(19)
        .all(|line| line.chars().nth(56) == Some('│')));
}

#[test]
fn keyboard_resize_adjusts_the_preference_after_the_effective_width_is_clamped() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-keyboard-split-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(
        &profile_path,
        br#"{"help_seen":true,"overview_left_width":65}"#,
    )
    .unwrap();
    let snapshot = production_snapshot();
    let mut ui = Ui::load(&profile_path, None).unwrap();

    ui.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)),
        ratatui::layout::Rect::new(0, 0, 80, 24),
        &snapshot,
    )
    .unwrap();

    let reloaded = Ui::load(&profile_path, None).unwrap();
    let restored = render(&snapshot, &reloaded, 120, 30);
    assert!(restored
        .text
        .lines()
        .skip(9)
        .take(19)
        .all(|line| line.chars().nth(66) == Some('│')));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn completed_drag_persists_the_preferred_width_across_terminal_sizes() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-workspace-split-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let snapshot = production_snapshot();
    let terminal_area = ratatui::layout::Rect::new(0, 0, 120, 30);
    let mut ui = Ui::load(&profile_path, None).unwrap();

    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        ui.handle_event(
            Event::Mouse(MouseEvent {
                kind,
                column: if matches!(kind, MouseEventKind::Down(_)) {
                    55
                } else {
                    65
                },
                row: 12,
                modifiers: KeyModifiers::NONE,
            }),
            terminal_area,
            &snapshot,
        )
        .unwrap();
    }

    let reloaded = Ui::load(&profile_path, None).unwrap();
    let compact = render(&snapshot, &reloaded, 80, 24);
    assert!(compact
        .text
        .lines()
        .skip(9)
        .take(13)
        .all(|line| line.chars().nth(31) == Some('│')));
    let wide = render(&snapshot, &reloaded, 120, 30);
    assert!(wide
        .text
        .lines()
        .skip(9)
        .take(19)
        .all(|line| line.chars().nth(65) == Some('│')));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn compact_all_page_matches_the_reviewed_production_fixture() {
    let rendered = render(&production_snapshot(), &Ui::default(), 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    assert!(rendered.text.contains("critical"));
    assert!(rendered.text.contains("DETAILS"));
    assert!(rendered.text.contains("Session Workspace"));
    assert!(rendered.text.contains("Conversation unavailable."));
    assert!(rendered.text.contains("Ctx +Warning"));
    assert!(!rendered.text.contains("Before 1: ✳:critical"));
    assert!(!rendered.text.contains("After 2: ✳:critical, ⬡:x-shared"));
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/all_populated_80x24.txt"),
    );
}

#[test]
fn compact_all_page_with_no_signals_matches_the_reviewed_production_fixture() {
    let mut snapshot = production_snapshot();
    snapshot.recent_signals.clear();
    let rendered = render(&snapshot, &Ui::default(), 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/all_empty_signals_80x24.txt"),
    );
}

#[test]
fn session_tree_keeps_open_sessions_visible_and_discloses_exited_age_bands() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let worktree = ("/Users/fixture/code/inventory", "main");
    let open = fixture_session(
        Provider::ClaudeCode,
        "open-session",
        "open-model",
        SessionState::Busy,
        20_000,
        NOW - 8 * DAY_MS,
        worktree,
    );
    let recent = fixture_session(
        Provider::CodexCli,
        "recent-session",
        "recent-model",
        SessionState::Exited,
        20_000,
        NOW - 7 * DAY_MS,
        worktree,
    );
    let older = fixture_session(
        Provider::ClaudeCode,
        "older-session",
        "older-model",
        SessionState::Exited,
        20_000,
        NOW - 7 * DAY_MS - 1,
        worktree,
    );
    let snapshot = Snapshot {
        sessions: vec![open, recent, older],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };

    let rendered = render(&snapshot, &Ui::default(), 120, 30);

    assert!(rendered.text.contains("open-model"));
    assert!(rendered.text.contains("Recent (1)"));
    assert!(rendered.text.contains("Older (1)"));
    assert!(!rendered.text.contains("recent-model"));
    assert!(!rendered.text.contains("older-model"));
}

#[test]
fn historical_session_tree_disclosures_render_at_supported_sizes() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let worktree = ("/Users/fixture/code/responsive-history", "main");
    let open = fixture_session(
        Provider::ClaudeCode,
        "open-session",
        "open-model",
        SessionState::Busy,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let recent = fixture_session(
        Provider::CodexCli,
        "recent-session",
        "recent-model",
        SessionState::Exited,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let older = fixture_session(
        Provider::ClaudeCode,
        "older-session",
        "older-model",
        SessionState::Exited,
        20_000,
        NOW - 8 * DAY_MS,
        worktree,
    );
    let snapshot = Snapshot {
        sessions: vec![open, recent, older],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };

    for (width, height) in [(80, 24), (120, 30)] {
        let mut ui = Ui::default();
        if width == 80 {
            press(&mut ui, &snapshot, KeyCode::Down);
            let recent = render(&snapshot, &ui, width, height);
            assert!(
                recent.text.contains("Recent (1)"),
                "compact render lost Recent disclosure:\n{}",
                recent.text
            );
            press(&mut ui, &snapshot, KeyCode::Down);
            let older = render(&snapshot, &ui, width, height);
            assert!(
                older.text.contains("Older (1)"),
                "compact render lost Older disclosure:\n{}",
                older.text
            );
            assert!(!older.text.contains("recent-model"));
            assert!(!older.text.contains("older-model"));
        } else {
            let rendered = render(&snapshot, &ui, width, height);
            assert!(rendered.text.contains("Recent (1)"));
            assert!(rendered.text.contains("Older (1)"));
            assert!(!rendered.text.contains("recent-model"));
            assert!(!rendered.text.contains("older-model"));
        }
    }
}

#[test]
fn session_tree_age_disclosures_expand_independently() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let worktree = ("/Users/fixture/code/disclosures", "main");
    let open = fixture_session(
        Provider::ClaudeCode,
        "open-session",
        "open-model",
        SessionState::Busy,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let recent = fixture_session(
        Provider::CodexCli,
        "recent-session",
        "recent-model",
        SessionState::Exited,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let older = fixture_session(
        Provider::ClaudeCode,
        "older-session",
        "older-model",
        SessionState::Exited,
        20_000,
        NOW - 8 * DAY_MS,
        worktree,
    );
    let snapshot = Snapshot {
        sessions: vec![open, recent, older],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let recent_expanded = render(&snapshot, &ui, 120, 30);
    assert!(recent_expanded.text.contains("▾ Recent (1)"));
    assert!(recent_expanded.text.contains("▸ Older (1)"));
    assert!(recent_expanded
        .text
        .lines()
        .any(|line| line.contains('⬡') && line.contains("1d")));
    assert!(!recent_expanded
        .text
        .lines()
        .any(|line| line.contains('✳') && line.contains("8d")));

    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let both_expanded = render(&snapshot, &ui, 120, 30);
    assert!(both_expanded.text.contains("▾ Recent (1)"));
    assert!(both_expanded.text.contains("▾ Older (1)"));
    assert!(both_expanded
        .text
        .lines()
        .any(|line| line.contains('⬡') && line.contains("1d")));
    assert!(both_expanded
        .text
        .lines()
        .any(|line| line.contains('✳') && line.contains("8d")));

    press(&mut ui, &snapshot, KeyCode::Up);
    press(&mut ui, &snapshot, KeyCode::Up);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let only_older_expanded = render(&snapshot, &ui, 120, 30);
    assert!(only_older_expanded.text.contains("▸ Recent (1)"));
    assert!(only_older_expanded.text.contains("▾ Older (1)"));
    assert!(!only_older_expanded
        .text
        .lines()
        .any(|line| line.contains('⬡') && line.contains("1d")));
    assert!(only_older_expanded
        .text
        .lines()
        .any(|line| line.contains('✳') && line.contains("8d")));
}

#[test]
fn selecting_an_exited_session_targets_the_inspector_and_workspace() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let worktree = ("/Users/fixture/code/selected-exited", "main");
    let open = fixture_session(
        Provider::ClaudeCode,
        "open",
        "open-model",
        SessionState::Busy,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let exited = fixture_session(
        Provider::CodexCli,
        "recent",
        "recent-model",
        SessionState::Exited,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let snapshot = Snapshot {
        sessions: vec![open, exited],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Down);
    let rendered = render(&snapshot, &ui, 120, 30);

    for required in [
        "⬡ recent",
        "Provider    Codex CLI -> recent-model",
        "Session Codex CLI / recent",
        "Conversation unavailable.",
    ] {
        assert!(
            rendered.text.contains(required),
            "selected Exited Session missed {required:?}"
        );
    }
    assert!(!rendered.text.contains("No Open Session selected."));
}

#[test]
fn exited_session_tree_selection_opens_transcript_lifetime_detail_and_returns() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let worktree = ("/Users/fixture/code/exited-detail", "main");
    let open = fixture_session(
        Provider::ClaudeCode,
        "open",
        "open-model",
        SessionState::Busy,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let exited = fixture_session(
        Provider::CodexCli,
        "recent",
        "recent-model",
        SessionState::Exited,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let snapshot = Snapshot {
        sessions: vec![open, exited],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let detail = render(&snapshot, &ui, 160, 30);
    assert!(detail.text.contains("State           Exited"));
    assert!(detail.text.contains("Transcript Lifetime Usage"));
    assert!(!detail.text.contains("Current Run Usage"));

    press(&mut ui, &snapshot, KeyCode::Esc);
    let restored = render(&snapshot, &ui, 120, 30);
    assert!(restored.text.contains("Session Codex CLI / recent"));
}

#[test]
fn project_tree_hides_unresolved_and_system_temporary_sessions() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let mut unavailable = fixture_session(
        Provider::ClaudeCode,
        "missing",
        "missing-model",
        SessionState::Busy,
        20_000,
        NOW - DAY_MS,
        ("/Users/fixture/code/missing", "main"),
    );
    unavailable.project = None;
    unavailable.worktree = None;
    unavailable.cwd = Some("/deleted/project".into());

    let mut temporary = unavailable.clone();
    temporary.id = "temporary".into();
    temporary.latest_model = Some("temporary-model".into());
    temporary.cwd = Some("/private/tmp/sessionmesh-test".into());
    let snapshot = Snapshot {
        sessions: vec![unavailable, temporary],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };
    let rendered = render(&snapshot, &Ui::default(), 120, 30);

    assert!(rendered.text.contains("PROJECT 0"));
    assert!(rendered.text.contains("0 OPEN"));
    assert!(!rendered.text.contains("Unavailable"));
    assert!(!rendered.text.contains("missing-model"));
    assert!(!rendered.text.contains("temporary-model"));
}

#[test]
fn historical_tree_selection_survives_refresh_by_provider_and_native_id() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let worktree = ("/Users/fixture/code/identity", "main");
    let open = fixture_session(
        Provider::ClaudeCode,
        "open",
        "open-model",
        SessionState::Busy,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let codex = fixture_session(
        Provider::CodexCli,
        "same-native-id",
        "codex-model",
        SessionState::Exited,
        20_000,
        NOW - DAY_MS,
        worktree,
    );
    let claude = fixture_session(
        Provider::ClaudeCode,
        "same-native-id",
        "claude-model",
        SessionState::Exited,
        20_000,
        NOW - 2 * DAY_MS,
        worktree,
    );
    let mut snapshot = Snapshot {
        sessions: vec![open, codex, claude],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Down);
    snapshot.sessions.reverse();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    let refreshed = render(&snapshot, &ui, 120, 30);

    assert!(refreshed
        .text
        .contains("Provider    Codex CLI -> codex-model"));
    assert!(refreshed.text.contains("Session Codex CLI /"));
    assert!(!refreshed.text.contains("Claude Code -> claude-model"));
}

#[test]
fn session_tree_keeps_non_git_history_directly_under_its_project() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let mut session = fixture_session(
        Provider::ClaudeCode,
        "non-git",
        "non-git-model",
        SessionState::Exited,
        20_000,
        NOW - DAY_MS,
        ("/Users/fixture/code/non-git", "main"),
    );
    session.project = Some(Project {
        identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/Users/fixture/code/non-git")),
        label: "non-git-project".into(),
    });
    session.worktree = None;
    let mut open = session.clone();
    open.id = "non-git-open".into();
    open.latest_model = Some("non-git-open-model".into());
    open.state = SessionState::Busy;
    open.last_activity_ms = NOW;
    let snapshot = Snapshot {
        sessions: vec![open, session],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let rendered = render(&snapshot, &ui, 120, 30);

    assert_eq!(rendered.text.matches("non-git-project").count(), 1);
    assert!(rendered.text.contains("Recent (1)"));
    assert!(rendered.text.contains("✳ non-git"));
    assert!(!rendered.text.contains("detached HEAD"));
}

#[test]
fn session_tree_worktree_disclosures_keep_independent_state() {
    const DAY_MS: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;
    let alpha = fixture_session(
        Provider::ClaudeCode,
        "alpha",
        "alpha-model",
        SessionState::Exited,
        20_000,
        NOW - DAY_MS,
        ("/Users/fixture/code/sessionmesh-alpha", "alpha"),
    );
    let beta = fixture_session(
        Provider::CodexCli,
        "beta",
        "beta-model",
        SessionState::Exited,
        20_000,
        NOW - 9 * DAY_MS,
        ("/Users/fixture/code/sessionmesh-beta", "beta"),
    );
    let mut alpha_open = alpha.clone();
    alpha_open.id = "alpha-open".into();
    alpha_open.latest_model = Some("alpha-open-model".into());
    alpha_open.state = SessionState::Busy;
    alpha_open.last_activity_ms = NOW;
    let mut beta_open = beta.clone();
    beta_open.id = "beta-open".into();
    beta_open.latest_model = Some("beta-open-model".into());
    beta_open.state = SessionState::Idle;
    beta_open.last_activity_ms = NOW;
    let snapshot = Snapshot {
        sessions: vec![alpha_open, alpha, beta_open, beta],
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let alpha_expanded = render(&snapshot, &ui, 120, 30);
    assert!(alpha_expanded.text.contains("✳ alpha"));
    assert!(!alpha_expanded.text.contains("⬡ beta"));

    press(&mut ui, &snapshot, KeyCode::Up);
    press(&mut ui, &snapshot, KeyCode::Up);
    press(&mut ui, &snapshot, KeyCode::Up);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let both_expanded = render(&snapshot, &ui, 120, 30);
    assert!(both_expanded.text.contains("✳ alpha"));
    assert!(both_expanded.text.contains("⬡ beta"));

    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let only_beta_expanded = render(&snapshot, &ui, 120, 30);
    assert!(!only_beta_expanded.text.contains("✳ alpha"));
    assert!(only_beta_expanded.text.contains("⬡ beta"));
}

#[test]
fn session_tree_selection_drives_the_inspector_and_workspace_after_refresh() {
    let mut snapshot = production_snapshot();
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Down);
    let selected = render(&snapshot, &ui, 120, 30);
    for required in [
        "⬡ x-shared",
        "Provider    Codex CLI -> gpt-5-codex",
        "40%",
        "Session Codex CLI / x-shared",
        "Conversation unavailable.",
    ] {
        assert!(
            selected.text.contains(required),
            "selected editor missed {required:?}"
        );
    }

    snapshot.sessions.reverse();
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    let refreshed = render(&snapshot, &ui, 120, 30);
    for required in [
        "Provider    Codex CLI -> gpt-5-codex",
        "Session Codex CLI / x-shared",
        "Conversation unavailable.",
    ] {
        assert!(
            refreshed.text.contains(required),
            "snapshot refresh retargeted the editor from {required:?}"
        );
    }
}

#[test]
fn session_tree_keyboard_collapse_and_expand_render_at_supported_sizes() {
    let snapshot = production_snapshot();
    for (width, height) in [(80, 24), (120, 30)] {
        let mut ui = Ui::default();
        press(&mut ui, &snapshot, KeyCode::Up);
        press(&mut ui, &snapshot, KeyCode::Left);

        let collapsed = render(&snapshot, &ui, width, height);
        assert!(collapsed.text.lines().any(|line| line.contains(">▸")));
        assert!(!collapsed
            .text
            .lines()
            .any(|line| line.contains("✳ critical") && line.contains("90%")));
        assert!(!collapsed
            .text
            .lines()
            .any(|line| line.contains("⬡ x-shared")));

        press(&mut ui, &snapshot, KeyCode::Right);
        press(&mut ui, &snapshot, KeyCode::Right);
        let expanded = render(&snapshot, &ui, width, height);
        assert!(expanded
            .text
            .lines()
            .any(|line| line.contains("> ✳ critical") && line.contains("90%")));
    }
}

#[test]
fn now_session_row_uses_the_saved_alias_as_its_title() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth fix".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 120, 30);
    let critical_row = rendered
        .text
        .lines()
        .find(|line| line.contains("✳ Auth fix"))
        .expect("critical NOW Session should render");
    assert!(
        critical_row.contains("Auth fix"),
        "NOW should render the saved Alias as its Title: {critical_row}"
    );
}

#[test]
fn compact_now_renders_a_readable_unicode_session_title() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Dao 文档整理".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    for (width, height) in [(80, 24), (120, 30)] {
        let rendered = render(&snapshot, &ui, width, height);
        let critical_row = rendered
            .text
            .lines()
            .find(|line| line.contains("✳ Dao"))
            .expect("critical NOW Session should render");
        assert!(critical_row.contains("Dao"));
        if width == 120 {
            assert!(rendered
                .text
                .replace(' ', "")
                .contains("SessionClaudeCode/Dao文档整理"));
        }
    }
}

#[test]
fn compact_now_uses_every_title_cell_before_operational_evidence() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "0123456789abcdefghijklmnop".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 80, 24);
    let critical_row = rendered
        .text
        .lines()
        .find(|line| line.contains('>') && line.contains("90%"))
        .expect("critical NOW Session should render");

    assert!(
        critical_row.contains("✳ ...hijklmnop ⌁ ! ●  90%│"),
        "Session Title should use every cell before operational evidence:\n{critical_row}"
    );
}

#[test]
fn wide_now_aligns_all_operational_evidence_for_supported_context_values() {
    const NOW: i64 = 1_700_000_000_000;
    let contexts = [
        ("critical", 200_000, Some(200_000), "100%", true, "0s"),
        ("warning", 150_000, Some(200_000), "75%", true, "1s"),
        ("nine", 18_000, Some(200_000), "9%", false, "2s"),
        ("unknown", 50_000, None, "N/A", false, "3s"),
    ];
    let sessions = contexts
        .iter()
        .enumerate()
        .map(|(index, (id, tokens, window, _, _, _))| {
            let mut session = fixture_session(
                Provider::ClaudeCode,
                id,
                "test-model",
                SessionState::Busy,
                *tokens,
                NOW - index as i64 * 1_000,
                ("/Users/fixture/code/aligned", "main"),
            );
            session.context.window = *window;
            session.worktree.as_mut().unwrap().writable = false;
            session
        })
        .collect::<Vec<_>>();
    let attention = AttentionEngine::default().update(&sessions, NOW);
    let snapshot = Snapshot {
        sessions,
        attention: Some(attention),
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
    };

    let rendered = render(&snapshot, &Ui::default(), 120, 30);
    let rows = contexts
        .iter()
        .map(|(id, _, _, value, needs_attention, elapsed)| {
            let row = rendered
                .text
                .lines()
                .find(|line| line.contains('✳') && line.contains(id))
                .unwrap_or_else(|| panic!("Session {id} should render"));
            (row, *value, *needs_attention, *elapsed)
        })
        .collect::<Vec<_>>();
    let column = |row: &str, value: &str| {
        row[..row.find(value).expect("evidence should render")]
            .chars()
            .count()
    };
    let evidence_end = |row: &str, value: &str| column(row, value) + value.chars().count();

    let lifecycle_column = column(rows[0].0, "●");
    let attention_column = lifecycle_column - 2;
    let context_end = evidence_end(rows[0].0, rows[0].1);
    let elapsed_end = evidence_end(rows[0].0, rows[0].3);
    for (row, value, needs_attention, elapsed) in rows {
        assert_eq!(
            row.chars().nth(attention_column),
            Some(if needs_attention { '!' } else { ' ' })
        );
        assert_eq!(column(row, "●"), lifecycle_column);
        assert_eq!(evidence_end(row, value), context_end);
        assert_eq!(evidence_end(row, elapsed), elapsed_end);
        assert!(
            row.contains(&format!("{elapsed}│")),
            "elapsed evidence should end at the Tree edge:\n{row}"
        );
    }
}

#[test]
fn wide_editor_keeps_the_complete_session_title_in_the_inspector() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Architecture cleanup".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 160, 30);
    let critical_row = rendered
        .text
        .lines()
        .find(|line| line.contains("✳") && line.contains("cleanup"))
        .expect("critical NOW Session should render");
    assert!(critical_row.contains("Architecture cleanup"));
    assert!(rendered
        .text
        .contains("Session Claude Code / Architecture cleanup"));
}

#[test]
fn long_unicode_now_title_does_not_move_operational_evidence() {
    let snapshot = production_snapshot();
    let baseline = render(&snapshot, &Ui::default(), 80, 24);
    let baseline_row = baseline
        .text
        .lines()
        .find(|line| line.contains("✳ critical"))
        .expect("baseline critical NOW Session should render");

    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Dao 文档整理和项目路线图".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    let aliased = render(&snapshot, &ui, 80, 24);
    let aliased_row = aliased
        .text
        .lines()
        .find(|line| line.contains('>') && line.contains("90%"))
        .expect("aliased critical NOW Session should render");

    let column = |row: &str, text: &str| {
        row[..row.find(text).expect("evidence should render")]
            .chars()
            .count()
    };
    assert_eq!(
        column(aliased_row, "90%"),
        column(baseline_row, "90%"),
        "90% should stay in its stable column"
    );
    assert!(aliased_row.contains("90%"));
    assert!(!aliased
        .text
        .contains("Session Claude Code / Dao 文档整理和项目路线图"));
    let wide = render(&snapshot, &ui, 120, 30);
    assert!(wide
        .text
        .replace(' ', "")
        .contains("SessionClaudeCode/Dao文档整理和项目路线图"));
}

#[test]
fn renaming_a_session_does_not_reorder_visible_session_identities() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    let before = ui
        .visible(&snapshot)
        .into_iter()
        .map(|session| (session.provider, session.id))
        .collect::<Vec<_>>();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "ZZZ title".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    let after = ui
        .visible(&snapshot)
        .into_iter()
        .map(|session| (session.provider, session.id))
        .collect::<Vec<_>>();
    assert_eq!(after, before);
}

#[test]
fn existing_recent_signal_resolves_the_current_session_title() {
    let snapshot = production_snapshot();
    let signal_count = snapshot.recent_signals.len();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 120, 30);
    let context_signal = rendered
        .text
        .lines()
        .find(|line| line.contains("Ctx +Critical"))
        .expect("existing Context Signal should render");
    assert!(context_signal.contains("✳ Auth"));
    assert_eq!(snapshot.recent_signals.len(), signal_count);
}

#[test]
fn worktree_signal_members_resolve_current_provider_qualified_titles() {
    let mut snapshot = production_snapshot();
    snapshot.recent_signals.truncate(1);
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 160, 30);
    let before = rendered
        .text
        .lines()
        .find(|line| line.contains("Before 1:"))
        .expect("Worktree Signal before-members should render");
    let after = rendered
        .text
        .lines()
        .find(|line| line.contains("After 2:"))
        .expect("Worktree Signal after-members should render");
    assert!(before.contains("✳:Auth"));
    assert!(after.contains("✳:Auth"));
    assert!(after.contains("⬡:x-shared"));
}

#[test]
fn worktree_signal_disambiguates_duplicate_member_aliases() {
    let mut snapshot = production_snapshot();
    snapshot.recent_signals.truncate(1);
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Pair".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Pair".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 160, 30);
    let after = rendered
        .text
        .lines()
        .find(|line| line.contains("After 2:"))
        .expect("Worktree Signal after-members should render");
    assert!(after.contains("✳:Pair · critical"));
    assert!(after.contains("⬡:Pair · x-shared"));
}

#[test]
fn session_signals_disambiguate_duplicate_visible_aliases() {
    let mut snapshot = production_snapshot();
    snapshot.recent_signals = vec![
        snapshot.recent_signals[1].clone(),
        snapshot.recent_signals[3].clone(),
    ];
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Pair".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Pair".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 160, 30);
    let critical = rendered
        .text
        .lines()
        .find(|line| line.contains("Ctx +Critical"))
        .expect("Critical Context Signal should render");
    let warning = rendered
        .text
        .lines()
        .find(|line| line.contains("Ctx +Warning"))
        .expect("Warning Context Signal should render");
    assert!(critical.contains("✳ Pair · critical"));
    assert!(warning.contains("✳ Pair · -warning"));
}

#[test]
fn wide_now_keeps_duplicate_aliases_distinguishable() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Pair".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Pair".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 120, 30);
    let critical = rendered
        .text
        .lines()
        .find(|line| line.contains("✳ Pair") && line.contains("90%"))
        .expect("critical NOW row should render");
    let warning = rendered
        .text
        .lines()
        .find(|line| line.contains("✳ Pair") && line.contains("75%"))
        .expect("warning NOW row should render");
    assert!(critical.contains("Pair · critical"));
    assert!(warning.contains("Pair · -warning"));
}

#[test]
fn full_claude_provider_page_matches_the_reviewed_production_fixture() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('2'));
    let rendered = render(&snapshot, &ui, 120, 30);

    assert_eq!((rendered.width, rendered.height), (120, 30));
    assert_eq!(
        rendered
            .text
            .matches("I 12.3k  O 2.3k  CR 34.6k  CW 456")
            .count(),
        3,
        "every visible Claude Session should keep complete lifetime Usage"
    );
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/claude_120x30.txt"),
    );
}

#[test]
fn provider_inventory_names_the_column_title_and_renders_the_saved_alias() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth fix".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Char('2'));

    let rendered = render(&snapshot, &ui, 120, 30);
    let header = rendered
        .text
        .lines()
        .find(|line| line.contains("State") && line.contains("Title"))
        .expect("Provider inventory should label its Session-name column Title");
    assert!(rendered.text.contains("Auth fix"));
    assert!(!header.contains("Session"));
}

#[test]
fn provider_inventory_disambiguates_duplicate_aliases_with_short_session_ids() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Pair".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Pair".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Char('2'));

    let rendered = render(&snapshot, &ui, 120, 30);
    assert!(rendered.text.contains("Pair · critical"));
    assert!(rendered.text.contains("Pair · -warning"));
}

#[test]
fn compact_expanded_claude_provider_page_matches_the_reviewed_production_fixture() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('2'));
    for _ in 0..3 {
        press(&mut ui, &snapshot, KeyCode::Down);
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    let rendered = render(&snapshot, &ui, 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    assert!(rendered.text.contains("- OLDER SESSIONS (1)  Collapse"));
    assert!(rendered.text.contains("de-older"));
    assert!(!rendered.text.contains("qfixture"));
    assert!(rendered.text.contains("Estimated using API rates"));
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/claude_older_expanded_80x24.txt"),
    );
}

#[test]
fn compact_codex_provider_page_matches_the_reviewed_production_fixture() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('3'));
    let rendered = render(&snapshot, &ui, 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    for title in ["x-shared", "dex-idle", "x-exited"] {
        assert!(rendered.text.contains(title));
    }
    for claude_title in ["critical", "-warning", "-healthy"] {
        assert!(!rendered.text.contains(claude_title));
    }
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/codex_80x24.txt"),
    );
}

#[test]
fn compact_provider_inventory_renders_titles_in_a_stable_column() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth fix".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Char('2'));

    let rendered = render(&snapshot, &ui, 80, 24);
    let header = rendered
        .text
        .lines()
        .find(|line| line.contains("State"))
        .expect("compact Provider header should render");
    assert!(header.contains("Title"));
    assert!(!header.contains("Session"));
    assert!(rendered.text.contains("Auth fix"));
}

#[test]
fn unicode_alias_does_not_move_adjacent_compact_provider_columns() {
    let snapshot = production_snapshot();
    let mut baseline_ui = Ui::default();
    press(&mut baseline_ui, &snapshot, KeyCode::Char('2'));
    let baseline = render(&snapshot, &baseline_ui, 80, 24);
    let baseline_row = baseline
        .text
        .lines()
        .find(|line| line.contains("180.0k"))
        .expect("critical compact Provider row should render");
    let baseline_context_column = baseline_row[..baseline_row.find("180.0k").unwrap()]
        .chars()
        .count();

    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "修复认证流程".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Char('2'));
    let rendered = render(&snapshot, &ui, 80, 24);
    let aliased_row = rendered
        .text
        .lines()
        .find(|line| line.contains("180.0k"))
        .expect("aliased compact Provider row should render");
    let aliased_context_column = aliased_row[..aliased_row.find("180.0k").unwrap()]
        .chars()
        .count();

    assert_eq!(aliased_context_column, baseline_context_column);
}

#[test]
fn compact_open_detail_matches_the_reviewed_production_fixture() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Enter);
    let rendered = render(&snapshot, &ui, 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    for required in [
        "Session Detail",
        "claude-critical",
        "Current Context",
        "Occupancy",
        "Source log",
        "Source format",
        "JSONL",
    ] {
        assert!(
            rendered.text.contains(required),
            "compact Open Detail should contain {required:?}:\n{}",
            rendered.visible_text()
        );
    }
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/detail_open_80x24.txt"),
    );
}

#[test]
fn session_detail_separates_the_title_from_the_complete_session_id() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth fix".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 80, 24);
    let title = rendered
        .text
        .lines()
        .find(|line| line.contains("Title"))
        .expect("Detail should display Title");
    let identity = rendered
        .text
        .lines()
        .find(|line| line.contains("Session ID"))
        .expect("Detail should display the complete native Session ID");
    assert!(title.contains("Auth fix"));
    assert!(identity.contains("claude-critical"));
}

#[test]
fn wide_session_detail_separates_the_title_from_the_complete_session_id() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth fix".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Enter);

    let rendered = render(&snapshot, &ui, 160, 30);
    let title = rendered
        .text
        .lines()
        .find(|line| line.contains("Title"))
        .expect("wide Detail should display Title");
    let identity = rendered
        .text
        .lines()
        .find(|line| line.contains("Session ID"))
        .expect("wide Detail should display the complete native Session ID");
    assert!(title.contains("Auth fix"));
    assert!(identity.contains("claude-critical"));
}

#[test]
fn compact_detail_field_navigation_reveals_complete_compaction_history() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Enter);

    for _ in 0..100 {
        press(&mut ui, &snapshot, KeyCode::Down);
    }
    let rendered = render(&snapshot, &ui, 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    for required in ["Compactions (2)", "trigger=auto", "trigger=manual"] {
        assert!(
            rendered.text.contains(required),
            "scrolled compact Detail should contain {required:?}:\n{}",
            rendered.visible_text()
        );
    }
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/detail_open_scrolled_80x24.txt"),
    );
}

#[test]
fn compact_exited_detail_matches_the_reviewed_production_fixture() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('2'));
    for _ in 0..3 {
        press(&mut ui, &snapshot, KeyCode::Down);
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let rendered = render(&snapshot, &ui, 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    assert!(rendered.text.contains("claude-older"));
    assert!(rendered.text.contains("State           Exited"));
    assert!(rendered.text.contains("Current Context"));
    assert!(!rendered.text.contains("Current Run Usage"));
    assert!(!rendered.text.contains("Last Run Usage"));
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/detail_exited_80x24.txt"),
    );
}

#[test]
fn full_open_detail_matches_the_reviewed_production_fixture() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Enter);
    let rendered = render(&snapshot, &ui, 120, 30);

    assert_eq!((rendered.width, rendered.height), (120, 30));
    for required in [
        "Session Detail",
        "Current Context",
        "Occupancy",
        "Attention Conditions",
        "CRITICAL Current Context",
        "Shared Worktree Risk",
        "Source log",
        "Session ID",
        "fields 1/17",
    ] {
        assert!(
            rendered.text.contains(required),
            "full Open Detail should contain {required:?}"
        );
    }
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/detail_open_120x30.txt"),
    );
}

#[test]
fn edit_session_opens_for_the_selected_session_with_identity_evidence() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();

    press(&mut ui, &snapshot, KeyCode::Char('e'));
    let rendered = render(&snapshot, &ui, 80, 24);

    for required in [
        "Edit Session",
        "Session  Claude Code · critical",
        "Title",
        "Description",
        "Tab/Shift+Tab Fields",
        "Enter Save",
        "Esc Cancel",
    ] {
        assert!(
            rendered.text.contains(required),
            "Edit Session should contain {required:?}:\n{}",
            rendered.visible_text()
        );
    }
}

#[test]
fn tab_and_shift_tab_move_session_editor_focus_between_fields() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));

    let title = render(&snapshot, &ui, 80, 24);
    assert!(title.text.contains("> Title"));
    let title_cursor = title.cursor_position;

    press(&mut ui, &snapshot, KeyCode::Tab);
    let description = render(&snapshot, &ui, 80, 24);
    assert!(description.text.contains("> Description"));
    assert_eq!(description.cursor_position.y, title_cursor.y + 1);

    ui.handle_key(
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        &snapshot,
    )
    .unwrap();
    let title_again = render(&snapshot, &ui, 80, 24);
    assert!(title_again.text.contains("> Title"));
    assert_eq!(title_again.cursor_position, title_cursor);
}

#[test]
fn description_editor_accepts_unicode_and_enforces_the_240_character_limit() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    press(&mut ui, &snapshot, KeyCode::Tab);
    for character in "界".repeat(241).chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }

    press(&mut ui, &snapshot, KeyCode::Enter);
    let invalid = render(&snapshot, &ui, 120, 30);
    assert!(invalid
        .text
        .contains("Description must be 240 characters or fewer"));
    assert!(invalid.cursor_visible);

    press(&mut ui, &snapshot, KeyCode::Backspace);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let saved = render(&snapshot, &ui, 120, 30);
    assert!(!saved.cursor_visible);
    assert!(saved.text.contains("Session metadata saved"));
}

#[test]
fn empty_session_editor_places_the_cursor_after_the_title_field_label() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));

    let rendered = render(&snapshot, &ui, 80, 24);
    let label = rendered.position_of("Title");

    assert_eq!(
        rendered.cursor_position,
        Position::new(label.x + 13, label.y)
    );
    assert!(rendered.cursor_visible);
}

#[test]
fn title_editor_keeps_the_cursor_after_ascii_input() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth fix".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }

    let rendered = render(&snapshot, &ui, 80, 24);
    let label = rendered.position_of("Title");

    assert_eq!(
        rendered.cursor_position,
        Position::new(label.x + 21, label.y)
    );
}

#[test]
fn title_editor_places_the_cursor_after_double_width_unicode_input() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "登录".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }

    let rendered = render(&snapshot, &ui, 80, 24);
    let label = rendered.position_of("Title");

    assert_eq!(
        rendered.cursor_position,
        Position::new(label.x + 17, label.y)
    );
}

#[test]
fn backspace_moves_the_title_editor_cursor_to_the_new_input_end() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "登录".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Backspace);

    let rendered = render(&snapshot, &ui, 80, 24);
    let label = rendered.position_of("Title");

    assert_eq!(
        rendered.cursor_position,
        Position::new(label.x + 15, label.y)
    );
}

#[test]
fn long_title_input_keeps_the_cursor_inside_the_editor() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in format!("{}TAIL_END", "x".repeat(72)).chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }

    let rendered = render(&snapshot, &ui, 80, 24);
    let label = rendered.position_of("Title");
    let row = &rendered.buffer.content()[label.y as usize * rendered.width as usize
        ..(label.y as usize + 1) * rendered.width as usize];
    let editor_right = row
        .iter()
        .enumerate()
        .skip(label.x as usize + 13)
        .find(|(_, cell)| cell.symbol() == "│")
        .map(|(x, _)| x as u16)
        .expect("Edit Session should retain its right border");

    assert_eq!(
        rendered.cursor_position,
        Position::new(editor_right - 1, label.y)
    );
    let tail = rendered.position_of("TAIL_END");
    assert_eq!(tail.y, label.y);
    assert_eq!(tail.x + 8, rendered.cursor_position.x);
}

#[test]
fn cancelling_session_editing_hides_the_terminal_cursor() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    let mut terminal = Terminal::new(CursorBackend::new(80, 24)).unwrap();

    terminal.draw(|frame| draw(frame, &snapshot, &ui)).unwrap();
    assert!(terminal.backend().cursor_visible);

    press(&mut ui, &snapshot, KeyCode::Esc);
    terminal.draw(|frame| draw(frame, &snapshot, &ui)).unwrap();
    assert!(!terminal.backend().cursor_visible);
}

#[test]
fn edit_session_metadata_survives_reloading_the_ui() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-edit-title-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let snapshot = production_snapshot();
    let mut ui = Ui::load(&profile_path, None).unwrap();

    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth fix".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Tab);
    for character in "Auth boundary 登录".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);
    let saved = render(&snapshot, &ui, 120, 30);
    assert!(!saved.cursor_visible);
    assert!(saved.text.contains("Auth fix"));
    assert!(saved.text.contains("Auth boundary"));

    let mut reloaded = Ui::load(&profile_path, None).unwrap();
    press(&mut reloaded, &snapshot, KeyCode::Char('e'));
    let rendered = render(&snapshot, &reloaded, 80, 24);
    assert!(rendered.text.contains("Auth fix"));
    assert!(rendered.text.contains("Auth boundary"));
    assert!(rendered.text.contains('登'));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn deleting_an_existing_alias_and_confirming_clears_the_title() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-clear-title-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let snapshot = production_snapshot();
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Auth fix".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Enter);

    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for _ in "Auth fix".chars() {
        press(&mut ui, &snapshot, KeyCode::Backspace);
    }
    let cleared = render(&snapshot, &ui, 80, 24);
    assert!(cleared.text.contains("Enter Save"));
    press(&mut ui, &snapshot, KeyCode::Enter);
    assert!(!render(&snapshot, &ui, 80, 24).cursor_visible);

    let mut reloaded = Ui::load(&profile_path, None).unwrap();
    press(&mut reloaded, &snapshot, KeyCode::Char('e'));
    let rendered = render(&snapshot, &reloaded, 80, 24);
    assert!(rendered.text.contains("Title"));
    assert!(!rendered.text.contains("Auth fix"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn malformed_label_store_disables_editing_until_rescan_reloads_it() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-label-recovery-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    let label_path = directory.join("session-labels.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    std::fs::write(&label_path, b"broken").unwrap();
    let snapshot = production_snapshot();
    let mut ui = Ui::load(&profile_path, None).unwrap();

    press(&mut ui, &snapshot, KeyCode::Char('e'));
    let degraded = render(&snapshot, &ui, 160, 30);
    assert!(!degraded.text.contains("Edit Session"));
    assert!(degraded.text.contains("Label Store error"));
    assert!(degraded.text.contains("session-labels.json"));

    std::fs::remove_file(&label_path).unwrap();
    press(&mut ui, &snapshot, KeyCode::Char('r'));
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    assert!(render(&snapshot, &ui, 80, 24).text.contains("Edit Session"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn malformed_label_store_disables_curation_until_rescan_reloads_it() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-curation-recovery-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    let label_path = directory.join("session-labels.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    std::fs::write(&label_path, b"broken").unwrap();
    let source = production_snapshot();
    let mut exited = source
        .sessions
        .iter()
        .find(|session| session.id == "codex-exited")
        .unwrap()
        .clone();
    exited.last_activity_ms = source.generated_ms - 1_000;
    let mut open = exited.clone();
    open.id = "codex-open".into();
    open.state = SessionState::Busy;
    open.last_activity_ms = source.generated_ms;
    let snapshot = Snapshot {
        sessions: vec![open, exited],
        generated_ms: source.generated_ms,
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Right);
    press(&mut ui, &snapshot, KeyCode::Down);

    press(&mut ui, &snapshot, KeyCode::Char('H'));
    let degraded = render(&snapshot, &ui, 120, 30).text;
    assert!(!degraded.contains("Hide Session?"));
    assert!(degraded.contains("Curation unavailable while Label Store is degraded"));
    press(&mut ui, &snapshot, KeyCode::Char('h'));
    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("Hidden inventory unavailable: Label Store degraded"));
    assert_eq!(std::fs::read(&label_path).unwrap(), b"broken");

    std::fs::remove_file(&label_path).unwrap();
    press(&mut ui, &snapshot, KeyCode::Char('r'));
    press(&mut ui, &snapshot, KeyCode::Char('H'));
    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("Hide Session?"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn failed_metadata_save_preserves_both_editor_inputs_for_a_successful_retry() {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "sessionmesh-title-retry-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let profile_path = directory.join("profile.json");
    let label_path = directory.join("session-labels.json");
    std::fs::write(&profile_path, br#"{"help_seen":true}"#).unwrap();
    let snapshot = production_snapshot();
    let mut ui = Ui::load(&profile_path, None).unwrap();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Retry title".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Tab);
    for character in "Retry description".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    std::fs::write(&label_path, b"broken during edit").unwrap();

    press(&mut ui, &snapshot, KeyCode::Enter);
    let failed = render(&snapshot, &ui, 120, 30);
    assert!(failed.text.contains("Edit Session"));
    assert!(failed.text.contains("Retry title"));
    assert!(failed.text.contains("Retry description"));

    std::fs::remove_file(&label_path).unwrap();
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    let retried = render(&snapshot, &ui, 120, 30);
    assert!(retried.text.contains("Retry title"));
    assert!(retried.text.contains("Retry description"));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn title_editor_rejects_81_characters_and_accepts_80() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for _ in 0..81 {
        press(&mut ui, &snapshot, KeyCode::Char('a'));
    }

    press(&mut ui, &snapshot, KeyCode::Enter);
    let invalid = render(&snapshot, &ui, 120, 30);
    assert!(invalid.text.contains("Enter Save | Esc Cancel"));
    assert!(invalid
        .text
        .contains("Title must be 80 characters or fewer"));

    press(&mut ui, &snapshot, KeyCode::Backspace);
    press(&mut ui, &snapshot, KeyCode::Enter);
    let saved = render(&snapshot, &ui, 120, 30);
    assert!(!saved.text.contains("Enter Save | Esc Cancel"));
    assert!(saved.text.contains("Session metadata saved"));
}

#[test]
fn escape_cancels_session_editing_without_persisting_or_prefilling_either_draft() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    for character in "Cancelled draft".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Tab);
    for character in "Cancelled description".chars() {
        press(&mut ui, &snapshot, KeyCode::Char(character));
    }
    press(&mut ui, &snapshot, KeyCode::Esc);

    let cancelled = render(&snapshot, &ui, 120, 30);
    assert!(!cancelled.text.contains("Cancelled draft"));
    assert!(!cancelled.text.contains("Cancelled description"));
    assert!(!cancelled.cursor_visible);
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    let reopened = render(&snapshot, &ui, 120, 30);
    assert!(reopened.text.contains("Enter Save | Esc Cancel"));
    assert!(!reopened.text.contains("Cancelled draft"));
    assert!(!reopened.text.contains("Cancelled description"));
}

#[test]
fn edit_session_shortcut_is_shown_only_for_a_concrete_session_target() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();

    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("e Edit session"));

    press(&mut ui, &snapshot, KeyCode::Tab);
    let signals = render(&snapshot, &ui, 120, 30);
    assert!(!signals.text.contains("e Edit session"));
    press(&mut ui, &snapshot, KeyCode::Char('e'));
    assert!(!render(&snapshot, &ui, 120, 30)
        .text
        .contains("Edit Session"));

    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Enter);
    assert!(render(&snapshot, &ui, 120, 30)
        .text
        .contains("e Edit session"));
}

#[test]
fn compact_guide_opens_with_all_categories() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    let rendered = render(&snapshot, &ui, 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    for required in [
        "Guide",
        "Overview",
        "Navigation",
        "Sessions",
        "Signals",
        "Theme",
    ] {
        assert!(rendered.text.contains(required));
    }
    assert!(!rendered.text.contains("Help - Esc to close"));
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/guide_80x24.txt"),
    );
}

#[test]
fn guide_uses_the_agreed_centered_golden_area_at_compact_and_wide_sizes() {
    let snapshot = production_snapshot();

    for (width, height, expected_left, expected_top, expected_width, expected_height) in
        [(80, 24, 8, 3, 64, 18), (120, 30, 12, 4, 96, 22)]
    {
        let mut ui = Ui::default();
        press(&mut ui, &snapshot, KeyCode::Char('?'));
        let rendered = render(&snapshot, &ui, width, height);
        let rows = rendered.text.lines().collect::<Vec<_>>();
        let top = rows
            .iter()
            .position(|row| row.contains(" Guide "))
            .expect("Guide should render its titled top border");
        let top_characters = rows[top].chars().collect::<Vec<_>>();
        let left = top_characters
            .iter()
            .position(|character| *character == '┌')
            .unwrap();
        let right = top_characters
            .iter()
            .rposition(|character| *character == '┐')
            .unwrap();
        let bottom = rows
            .iter()
            .enumerate()
            .skip(top + 1)
            .find(|(_, row)| row.chars().nth(left) == Some('└'))
            .map(|(index, _)| index)
            .unwrap();

        assert_eq!((left, top), (expected_left, expected_top));
        assert_eq!(right - left + 1, expected_width);
        assert_eq!(bottom - top + 1, expected_height);
    }
}

#[test]
fn overview_guide_explains_scope_pages_and_current_versus_recent_state() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    let rendered = render(&snapshot, &ui, 120, 30);

    for required in [
        "SessionMesh is local to this Mac.",
        "Passive observation is the default.",
        "Control Operations require explicit authorization.",
        "1  All",
        "2  Claude Code",
        "3  Codex CLI",
        "NOW shows current conditions.",
        "Recent Signals show transitions.",
    ] {
        assert!(
            rendered.text.contains(required),
            "Overview Guide should contain {required:?}"
        );
    }
}

#[test]
fn navigation_guide_lists_global_and_contextual_controls_without_theme_cycling() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    press(&mut ui, &snapshot, KeyCode::Down);
    let rendered = render(&snapshot, &ui, 120, 30);

    for required in [
        "1 / 2 / 3  Open a page",
        "n  New Session",
        "Tab / Shift+Tab  Change region or Guide pane",
        "Up/Down or j/k  Move or scroll",
        "Enter  Inspect or expand",
        "Esc  Back or close",
        "c  Copy the selected Session Detail field",
        "h  Authorize a Handoff from Session Detail",
        "Left/Right  Choose a Successor Provider",
        "o  Open a Ready Successor in Terminal.app",
        "c  Copy a Ready Successor command",
        "r  Refresh",
        "d  Delete a terminal Task and owned Artifact after confirmation",
        "Tasks and Artifacts never expire automatically.",
        "?  Guide",
        "q  Quit",
    ] {
        assert!(
            rendered.text.contains(required),
            "Navigation Guide should contain {required:?}"
        );
    }
    assert!(!rendered.text.contains("t Theme"));
}

#[test]
fn guide_opens_on_signals_from_the_recent_signals_region() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);

    press(&mut ui, &snapshot, KeyCode::Char('?'));
    let rendered = render(&snapshot, &ui, 80, 24);

    assert!(rendered.text.contains("> Signals"));
    assert!(!rendered.text.contains("> Overview"));
}

#[test]
fn signals_guide_explains_every_recent_signal_and_retention_rule() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Char('?'));

    let rendered = render(&snapshot, &ui, 120, 80);

    for required in [
        "Session lifecycle",
        "⬡ session1 Busy -> Idle",
        "⬡ session1 Busy -> Exited",
        "⬡ session1 Idle -> Exited",
        "⬡ session1 Exited -> Busy",
        "⬡ session1 Exited -> Idle",
        "✳ session1 Ctx +Warning 120.0k>150.0k",
        "✳ session1 Ctx +Critical 160.0k>180.0k",
        "✳ session1 Ctx -Warning 150.0k>120.0k",
        "✳ session1 Ctx -Critical 180.0k>160.0k",
        "Warning starts at 70%; Critical starts at 85%.",
        "✳ session1 Cmp auto 188.0k>100.0k",
        "✳ session1 Cmp auto 188.0k>100.0k -Critical",
        "WT /project  Risk detected",
        "WT /project  Risk cleared",
        "Before 1: ✳:session1",
        "After 2: ✳:session1, ⬡:session2",
        "✳  Claude Code",
        "⬡  Codex CLI",
        "WT  Worktree",
        "Idle -> Busy is not recorded.",
        "ordinary Current Context percentage changes",
        "source timestamps",
        "Possible Stall",
        "Only the five newest Signals are retained",
        "Current state belongs in NOW.",
        "prompt, response, tool, or source-code content",
    ] {
        assert!(
            rendered.text.contains(required),
            "Signals Guide should contain {required:?}"
        );
    }
}

#[test]
fn signals_guide_explains_context_evidence_and_scope() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Char('?'));

    let rendered = render(&snapshot, &ui, 120, 80);

    assert!(rendered
        .text
        .contains("before -> after Current Context values"));
    assert!(rendered
        .text
        .contains("Only Open Sessions with a known Current Context percentage"));
}

#[test]
fn signals_guide_explains_shared_worktree_risk_evidence_and_scope() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Char('?'));

    let rendered = render(&snapshot, &ui, 120, 80);

    for required in [
        "Two Open Sessions trigger Shared Worktree Risk.",
        "Open Session membership before -> after",
        "not proof of a file-level or Git conflict",
    ] {
        assert!(
            rendered.text.contains(required),
            "Signals Guide should contain {required:?}"
        );
    }
}

#[test]
fn signals_guide_explains_the_silent_baseline_and_metadata_only_scope() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Char('?'));

    let rendered = render(&snapshot, &ui, 120, 80);

    assert!(rendered.text.contains("first scan is a silent baseline"));
    assert!(rendered.text.contains("metadata-only"));
}

#[test]
fn overflowing_signals_guide_scrolls_and_reports_its_position() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Char('?'));

    let top = render(&snapshot, &ui, 80, 24);
    assert!(top.text.contains("Recent Signals answer"));
    assert!(!top
        .text
        .contains("prompt, response, tool, or source-code content"));
    assert!(top.text.contains(" 1/"));
    assert!(top.text.contains('█'));

    press(&mut ui, &snapshot, KeyCode::Tab);
    for _ in 0..64 {
        press(&mut ui, &snapshot, KeyCode::Char('j'));
    }
    let bottom = render(&snapshot, &ui, 80, 24);
    assert!(bottom
        .text
        .contains("prompt, response, tool, or source-code content"));
    assert!(!bottom.text.contains("Recent Signals answer"));
    assert!(!bottom.text.contains(" 1/"));
}

#[test]
fn guide_scrolls_up_immediately_after_reaching_the_bottom() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    press(&mut ui, &snapshot, KeyCode::Tab);

    for _ in 0..64 {
        press(&mut ui, &snapshot, KeyCode::Char('j'));
    }
    let bottom = render(&snapshot, &ui, 80, 24);

    press(&mut ui, &snapshot, KeyCode::Char('k'));
    let one_line_up = render(&snapshot, &ui, 80, 24);

    assert_ne!(
        one_line_up.text, bottom.text,
        "one Up key should move the Guide immediately after overscrolling the bottom"
    );
}

#[test]
fn short_overview_guide_omits_scrollbar_and_position() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('?'));

    let rendered = render(&snapshot, &ui, 80, 24);

    assert!(!rendered.text.contains(" 1/"));
    assert!(!rendered.text.contains('█'));
}

#[test]
fn reopening_guide_restores_the_contextual_category_at_the_top() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    press(&mut ui, &snapshot, KeyCode::Tab);
    for _ in 0..64 {
        press(&mut ui, &snapshot, KeyCode::Char('j'));
    }
    assert!(!render(&snapshot, &ui, 80, 24)
        .text
        .contains("Recent Signals answer"));

    press(&mut ui, &snapshot, KeyCode::Esc);
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    let reopened = render(&snapshot, &ui, 80, 24);

    assert!(reopened.text.contains("> Signals"));
    assert!(reopened.text.contains("Recent Signals answer"));
    assert!(reopened.text.contains(" 1/"));
}

#[test]
fn changing_guide_category_resets_content_scroll() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Tab);
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    press(&mut ui, &snapshot, KeyCode::Tab);
    for _ in 0..64 {
        press(&mut ui, &snapshot, KeyCode::Char('j'));
    }
    assert!(!render(&snapshot, &ui, 80, 24)
        .text
        .contains("Recent Signals answer"));

    ui.handle_key(
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        &snapshot,
    )
    .unwrap();
    press(&mut ui, &snapshot, KeyCode::Down);
    press(&mut ui, &snapshot, KeyCode::Up);
    let signals_again = render(&snapshot, &ui, 80, 24);

    assert!(signals_again.text.contains("> Signals"));
    assert!(signals_again.text.contains("Recent Signals answer"));
    assert!(signals_again.text.contains(" 1/"));
}

#[test]
fn guide_opens_on_sessions_from_provider_inventory_and_session_detail() {
    let snapshot = production_snapshot();
    let mut provider_ui = Ui::default();
    press(&mut provider_ui, &snapshot, KeyCode::Char('2'));
    press(&mut provider_ui, &snapshot, KeyCode::Char('?'));
    assert!(render(&snapshot, &provider_ui, 80, 24)
        .text
        .contains("> Sessions"));

    let mut detail_ui = Ui::default();
    press(&mut detail_ui, &snapshot, KeyCode::Enter);
    press(&mut detail_ui, &snapshot, KeyCode::Char('?'));
    assert!(render(&snapshot, &detail_ui, 80, 24)
        .text
        .contains("> Sessions"));
}

#[test]
fn sessions_guide_explains_providers_lifecycle_and_current_context() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('2'));
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    let rendered = render(&snapshot, &ui, 120, 30);

    for required in [
        "✳ Claude Code   ⬡ Codex CLI",
        "Open Session",
        "● Busy   ○ Idle   no marker Exited",
        "!  The Session currently needs attention.",
        "Current Context is the latest known content occupying the model window.",
        "It is not cumulative Processed Usage.",
        "Warning  70%",
        "Critical 85%",
    ] {
        assert!(
            rendered.text.contains(required),
            "Sessions Guide should contain {required:?}"
        );
    }
}

#[test]
fn sessions_guide_explains_titles_aliases_fallbacks_and_editing() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('2'));
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    let rendered = render(&snapshot, &ui, 120, 30);

    for required in [
        "Session Titles",
        "Title  The name SessionMesh displays for a Session.",
        "Alias  A local user-authored Title.",
        "Short Session ID  The last eight characters",
        "Duplicate Titles include their Short Session ID.",
        "e Edit session  Set or clear its local Title and Description.",
    ] {
        assert!(
            rendered.text.contains(required),
            "Sessions Guide should contain {required:?}"
        );
    }
}

#[test]
fn theme_guide_explains_the_active_theme_built_ins_and_process_override() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    for _ in 0..4 {
        press(&mut ui, &snapshot, KeyCode::Down);
    }
    let rendered = render(&snapshot, &ui, 120, 30);

    for required in [
        "Active Theme  sessionmesh",
        "sessionmesh",
        "catppuccin",
        "dracula",
        "gruvbox",
        "light",
        "high-contrast",
        "--theme <name>",
        "Applies to one process and does not change the saved Theme.",
        "Theme changes presentation, never Session state or thresholds.",
    ] {
        assert!(
            rendered.text.contains(required),
            "Theme Guide should contain {required:?}"
        );
    }
}

#[test]
fn guide_category_selection_updates_without_enter() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('?'));

    press(&mut ui, &snapshot, KeyCode::Char('j'));
    let rendered = render(&snapshot, &ui, 80, 24);

    assert!(rendered.text.contains("> Navigation"));
    assert!(!rendered.text.contains("> Overview"));
}

#[test]
fn guide_category_navigation_accepts_arrows_and_vim_keys_without_wrapping() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('?'));

    press(&mut ui, &snapshot, KeyCode::Down);
    assert!(render(&snapshot, &ui, 80, 24).text.contains("> Navigation"));
    press(&mut ui, &snapshot, KeyCode::Up);
    assert!(render(&snapshot, &ui, 80, 24).text.contains("> Overview"));
    press(&mut ui, &snapshot, KeyCode::Char('j'));
    press(&mut ui, &snapshot, KeyCode::Char('k'));
    assert!(render(&snapshot, &ui, 80, 24).text.contains("> Overview"));

    for _ in 0..5 {
        press(&mut ui, &snapshot, KeyCode::Char('j'));
    }
    press(&mut ui, &snapshot, KeyCode::Down);
    assert!(render(&snapshot, &ui, 80, 24).text.contains("> Theme"));
}

#[test]
fn guide_focus_moves_between_categories_and_content() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Char('?'));
    assert!(render(&snapshot, &ui, 80, 24)
        .text
        .contains("Tab/Right Content"));

    press(&mut ui, &snapshot, KeyCode::Right);
    let content = render(&snapshot, &ui, 80, 24);
    assert!(content.text.contains("j/k Scroll"));
    assert!(content.text.contains("Shift+Tab/Left Categories"));

    ui.handle_key(
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        &snapshot,
    )
    .unwrap();
    assert!(render(&snapshot, &ui, 80, 24)
        .text
        .contains("Tab/Right Content"));

    press(&mut ui, &snapshot, KeyCode::Tab);
    assert!(render(&snapshot, &ui, 80, 24).text.contains("j/k Scroll"));

    press(&mut ui, &snapshot, KeyCode::Left);
    assert!(render(&snapshot, &ui, 80, 24)
        .text
        .contains("Tab/Right Content"));
}

#[test]
fn all_six_themes_render_readable_distinct_semantic_roles() {
    let snapshot = production_snapshot();
    let mut baseline_body = None;

    for expected_theme in [
        "sessionmesh",
        "catppuccin",
        "dracula",
        "gruvbox",
        "light",
        "high-contrast",
    ] {
        let ui = ui_with_theme(expected_theme);
        assert_eq!(ui.active_theme_name(), expected_theme);
        let rendered = render(&snapshot, &ui, 120, 30);
        let body = rendered
            .visible_text()
            .lines()
            .take(29)
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(baseline) = baseline_body.as_ref() {
            assert_eq!(&body, baseline, "Theme changed product semantics");
        } else {
            baseline_body = Some(body);
        }

        let roles = [
            ("critical", rendered.style_at_in_row("critical", "!")),
            ("warning", rendered.style_at_in_row("-warning", "!")),
            ("idle", rendered.style_at_in_row("dex-idle", "○")),
            ("busy", rendered.style_at_in_row("critical", "●")),
        ];
        for (role, style) in roles {
            let ratio = contrast_ratio(style);
            assert!(
                ratio >= 4.5,
                "Theme {expected_theme} rendered {role} at contrast {ratio:.2}"
            );
        }
        assert_ne!(
            rendered.style_at_in_row("-warning", "!"),
            rendered.style_at_in_row("dex-idle", "○")
        );
        assert_ne!(
            rendered.style_at_in_row("dex-idle", "○"),
            rendered.style_at_in_row("critical", "●")
        );

        let warning = rendered.style_at_in_row("-warning", "!");
        let busy = rendered.style_at_in_row("-warning", "●");
        let idle = rendered.style_at_in_row("dex-idle", "○");
        let selected_row = "> ✳ critical";
        let selected_cursor = rendered.style_at_in_row(selected_row, ">");
        let selected_title = rendered.style_at_in_row(selected_row, "critical");
        let selected_critical = rendered.style_at_in_row(selected_row, "!");
        let selected_busy = rendered.style_at_in_row(selected_row, "●");
        let critical_context = rendered.style_at_in_row(selected_row, "90%");
        let ratio = contrast_ratio(critical_context);
        assert!(
            ratio >= 4.5,
            "Theme {expected_theme} rendered Critical Current Context at contrast {ratio:.2}"
        );
        let warning_context = rendered.style_at_in_row("-warning", "75%");
        assert!(contrast_ratio(warning_context) >= 4.5);
        for (role, style) in [("Busy state", busy), ("Idle state", idle)] {
            let ratio = contrast_ratio(style);
            assert!(
                ratio >= 4.5,
                "Theme {expected_theme} rendered {role} at contrast {ratio:.2}"
            );
        }
        assert_ne!(busy, idle, "Theme {expected_theme} merged lifecycle roles");
        assert_ne!(
            busy, warning,
            "Theme {expected_theme} let Warning override Busy"
        );
        assert_ne!(
            idle, warning,
            "Theme {expected_theme} let Warning override Idle"
        );
        let focused_project = rendered.style_at("PROJECT");
        let selected_page = rendered.style_at("1 2 3");
        assert_eq!(focused_project.fg, selected_page.fg);
        assert_eq!(focused_project.bg, selected_page.bg);
        assert!(focused_project.add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            selected_cursor.fg,
            rendered.top_rule_style("PROJECT").fg,
            "Theme {expected_theme} did not use its accent for the selected Session cursor"
        );
        assert!(selected_cursor.add_modifier.contains(Modifier::BOLD));
        for (role, style) in [
            ("cursor", selected_cursor),
            ("title", selected_title),
            ("Critical attention", selected_critical),
            ("Busy state", selected_busy),
        ] {
            assert_eq!(
                style.bg, selected_cursor.bg,
                "Theme {expected_theme} did not extend the selected Tree background to {role}"
            );
            assert_ne!(
                style.bg, warning.bg,
                "Theme {expected_theme} did not distinguish the selected Tree {role}"
            );
        }
        let selected_row_end = rendered.buffer[(54, 12)].style();
        assert_eq!(
            selected_row_end.bg, selected_cursor.bg,
            "Theme {expected_theme} did not extend the selected Tree background through its row"
        );
        assert_ne!(
            selected_critical, selected_busy,
            "Theme {expected_theme} let selection merge Attention and lifecycle roles"
        );
    }
}
