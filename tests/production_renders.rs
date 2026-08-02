use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;
use sessionmesh::app::Snapshot;
use sessionmesh::attention::{AttentionEngine, SessionIdentity};
use sessionmesh::domain::{
    Compaction, ContextState, ModelUsage, Project, ProjectIdentity, Provider, Session,
    SessionState, SourceFormat, SourceMetadata, Usage, UsageSummary, Worktree,
};
use sessionmesh::signals::{
    ContextAlertLevel, RecentSignal, RecentSignalKind, SharedWorktreeRiskChange,
};
use sessionmesh::ui::{draw, Ui};

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct ProductionRender {
    width: u16,
    height: u16,
    text: String,
    buffer: Buffer,
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

    fn styles_at_in_row(&self, row_text: &str, text: &str) -> Vec<Style> {
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
                return cells.iter().map(|cell| cell.style()).collect();
            }
        }
        panic!("rendered text {text:?} not found in row containing {row_text:?}");
    }
}

fn render(snapshot: &Snapshot, ui: &Ui, width: u16, height: u16) -> ProductionRender {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, snapshot, ui)).unwrap();
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
    }
}

fn fixture_session(
    provider: Provider,
    id: &str,
    model: &str,
    state: SessionState,
    context_tokens: u64,
    last_activity_ms: i64,
    worktree_root: &str,
    branch: &str,
) -> Session {
    let usage = Usage {
        input: 12_345,
        output: 2_345,
        cache_read: 34_567,
        cache_write: 456,
    };
    Session {
        provider,
        id: id.into(),
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
        shared_root,
        "main",
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
        shared_root,
        "main",
    );
    let warning = fixture_session(
        Provider::ClaudeCode,
        "claude-warning",
        "claude-sonnet-4-5",
        SessionState::Busy,
        150_000,
        NOW - 120_000,
        "/Users/fixture/code/sessionmesh-warning",
        "warning",
    );
    let idle = fixture_session(
        Provider::CodexCli,
        "codex-idle",
        "gpt-5.2-codex",
        SessionState::Idle,
        60_000,
        NOW - 180_000,
        "/Users/fixture/code/sessionmesh-idle",
        "idle",
    );
    let healthy = fixture_session(
        Provider::ClaudeCode,
        "claude-healthy",
        "claude-haiku-4-5",
        SessionState::Busy,
        40_000,
        NOW - 240_000,
        "/Users/fixture/code/sessionmesh-healthy",
        "healthy",
    );
    let older = fixture_session(
        Provider::ClaudeCode,
        "claude-older",
        "claude-sonnet-4-5",
        SessionState::Exited,
        50_000,
        NOW - 8 * DAY,
        "/Users/fixture/code/sessionmesh-older",
        "archive",
    );
    let recent_exited = fixture_session(
        Provider::CodexCli,
        "codex-exited",
        "gpt-5-codex",
        SessionState::Exited,
        70_000,
        NOW - DAY,
        "/Users/fixture/code/sessionmesh-exited",
        "done",
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
        status: "fixture ready".into(),
        generated_ms: NOW,
        ready: true,
        ..Snapshot::default()
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
    assert!(wide
        .text
        .lines()
        .nth(1)
        .is_some_and(|line| line.contains("PROJECT") && line.contains("RECENT SIGNALS")));

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
        .is_some_and(|line| line.contains("PROJECT")));
}

#[test]
fn all_page_frame_titles_summarize_open_work_and_signal_freshness() {
    let snapshot = production_snapshot();
    let rendered = render(&snapshot, &Ui::default(), 120, 30);
    let titles = rendered.text.lines().nth(1).unwrap();

    for expected in [
        "PROJECTS 1",
        "5 OPEN SESSIONS",
        "RECENT SIGNALS 5",
        "latest 5s",
    ] {
        assert!(
            titles.contains(expected),
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
fn all_page_labels_the_project_region_and_uses_theme_foreground_for_project_names() {
    let snapshot = production_snapshot();
    let ui = Ui::default();

    let dark = render(&snapshot, &ui, 120, 30);
    assert!(dark
        .text
        .lines()
        .any(|line| line.contains(" PROJECTS ") && line.contains(" RECENT SIGNALS ")));
    assert!(dark
        .text
        .lines()
        .any(|line| line.starts_with("│sessionmesh")));
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
fn all_page_keeps_multiple_worktrees_below_the_project_without_context() {
    let rendered = render(&production_snapshot(), &Ui::default(), 120, 30);
    let worktree = rendered
        .text
        .lines()
        .find(|line| line.contains("WARNING Shared Worktree Risk x2"))
        .expect("All page should render the shared Worktree below its Project");

    assert!(worktree.starts_with("│  "));
    assert!(worktree.contains("| main"));
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
        &root_text,
        "main",
    );
    let snapshot = Snapshot {
        sessions: vec![session],
        generated_ms: 1_700_000_000_000,
        ready: true,
        ..Snapshot::default()
    };

    let rendered = render(&snapshot, &Ui::default(), 120, 30);

    assert!(rendered
        .text
        .contains("sessionmesh | ~/code/sessionmesh | main"));
    assert!(!rendered
        .text
        .lines()
        .find(|line| line.starts_with("│sessionmesh |"))
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
        .find(|line| line.contains("Claude stable-c"))
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
    let regions = rendered
        .text
        .lines()
        .find(|line| line.contains("PROJECT") && line.contains("RECENT SIGNALS"));
    assert!(
        regions.is_some(),
        "full All page should render PROJECT and RECENT SIGNALS side by side"
    );
    for required in [
        "CRITICAL",
        "Shared Worktree Risk",
        "WARNING",
        "IDLE",
        "BUSY",
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
fn compact_all_page_matches_the_reviewed_production_fixture() {
    let rendered = render(&production_snapshot(), &Ui::default(), 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    assert!(
        rendered.text.contains("claude-warning"),
        "compact Signals should retain the fourth newest transition"
    );
    assert!(!rendered.text.contains("Before 1: CL:claude-critical"));
    assert!(!rendered
        .text
        .contains("After 2: CL:claude-critical, CX:codex-shared"));
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
    assert!(rendered.text.contains("claude-older"));
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
    for session_id in ["codex-shared", "codex-idle", "codex-exited"] {
        assert!(rendered.text.contains(session_id));
    }
    for claude_session in ["claude-critical", "claude-warning", "claude-healthy"] {
        assert!(!rendered.text.contains(claude_session));
    }
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/codex_80x24.txt"),
    );
}

#[test]
fn compact_open_detail_matches_the_reviewed_production_fixture() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Enter);
    let rendered = render(&snapshot, &ui, 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    for required in [
        "Session detail",
        "claude-critical",
        "Attention Conditions",
        "Current Run Usage",
        "Transcript Lifetime Usage",
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
fn compact_open_detail_stays_stable_when_complete_compaction_history_fits() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Enter);
    let initial = render(&snapshot, &ui, 80, 24);
    assert!(initial.text.contains("trigger=auto"));
    assert!(initial.text.contains("trigger=manual"));

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
    assert_eq!(rendered.visible_text(), initial.visible_text());
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
    assert!(rendered.text.contains("Transcript Lifetime Usage"));
    assert!(rendered
        .text
        .contains("Exited Session has no current Attention"));
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
        "Attention Conditions",
        "CRITICAL Current Context",
        "Shared Worktree Risk",
        "Current Run Usage",
        "Transcript Lifetime Usage",
        "Compactions (2)",
        "trigger=auto",
        "trigger=manual",
        "q Quit",
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
        "Tab / Shift+Tab  Change region or Guide pane",
        "Up/Down or j/k  Move or scroll",
        "Enter  Inspect or expand",
        "Esc  Back or close",
        "c  Copy source metadata in Session Detail",
        "r  Refresh",
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
        "Busy -> Idle",
        "Busy -> Exited",
        "Idle -> Exited",
        "Exited -> Busy",
        "Exited -> Idle",
        "Ctx +Warning",
        "Ctx +Critical",
        "Ctx -Warning",
        "Ctx -Critical",
        "Warning starts at 70%; Critical starts at 85%.",
        "Cmp <trigger>",
        "-Critical",
        "Risk detected",
        "Risk cleared",
        "CL  Claude Code",
        "CX  Codex CLI",
        "WT  Worktree",
        "Idle -> Busy is not recorded.",
        "ordinary Context percentage changes",
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
        .contains("Only Open Sessions with a known Context percentage"));
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
        "CL  Claude Code",
        "CX  Codex CLI",
        "Open Session",
        "Busy  A request is running.",
        "Idle  The process is waiting for input.",
        "Exited  No live Provider process is attached.",
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
fn compact_copy_menu_matches_the_reviewed_production_fixture() {
    let snapshot = production_snapshot();
    let mut ui = Ui::default();
    press(&mut ui, &snapshot, KeyCode::Enter);
    press(&mut ui, &snapshot, KeyCode::Char('c'));
    let rendered = render(&snapshot, &ui, 80, 24);

    assert_eq!((rendered.width, rendered.height), (80, 24));
    for required in [
        "Copy source metadata",
        "> Source log",
        "Worktree",
        "cwd",
        "Session ID",
        "Enter Copy | Esc Cancel",
    ] {
        assert!(rendered.text.contains(required));
    }
    assert_fixture(
        &rendered,
        include_str!("fixtures/production-renders/copy_menu_80x24.txt"),
    );
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
            ("critical", rendered.style_at("CRITICAL")),
            ("warning", rendered.style_at("WARNING")),
            ("idle", rendered.style_at("IDLE")),
            ("busy", rendered.style_at("BUSY")),
        ];
        for (role, style) in roles {
            let ratio = contrast_ratio(style);
            assert!(
                ratio >= 4.5,
                "Theme {expected_theme} rendered {role} at contrast {ratio:.2}"
            );
        }
        assert_ne!(rendered.style_at("WARNING"), rendered.style_at("IDLE"));
        assert_ne!(rendered.style_at("IDLE"), rendered.style_at("BUSY"));

        let warning = rendered.style_at_in_row("claude-w", "WARNING");
        let busy = rendered.style_at_in_row("claude-w", "Busy");
        let idle = rendered.style_at_in_row("codex-id", "Idle");
        let selected_cursor = rendered.style_at_in_row("claude-c", ">");
        let selected_critical = rendered.style_at_in_row("claude-c", "CRITICAL");
        let selected_busy = rendered.style_at_in_row("claude-c", "Busy");
        let critical_bar = rendered.styles_at_in_row("claude-c", "[##############-]");
        let low_context = critical_bar[1];
        let medium_context = critical_bar[8];
        let warning_context = critical_bar[11];
        let critical_context = critical_bar[14];
        let unfilled_context = critical_bar[15];
        for (role, style) in [
            ("low Context", low_context),
            ("medium Context", medium_context),
            ("warning Context", warning_context),
            ("critical Context", critical_context),
        ] {
            let ratio = contrast_ratio(style);
            assert!(
                ratio >= 4.5,
                "Theme {expected_theme} rendered {role} at contrast {ratio:.2}"
            );
        }
        assert_ne!(low_context.fg, medium_context.fg);
        assert_ne!(medium_context.fg, warning_context.fg);
        assert_ne!(warning_context.fg, critical_context.fg);
        assert_ne!(critical_context.fg, unfilled_context.fg);
        assert_eq!(
            rendered.style_at_in_row("claude-c", "90%").fg,
            critical_context.fg
        );
        assert_eq!(
            rendered.style_at_in_row("claude-w", "75%").fg,
            warning_context.fg
        );
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
        assert_eq!(
            selected_cursor.fg,
            rendered.style_at("PROJECTS").fg,
            "Theme {expected_theme} did not use its accent for the selected Session cursor"
        );
        assert!(selected_cursor.add_modifier.contains(Modifier::BOLD));
        for (role, style) in [
            ("cursor", selected_cursor),
            ("Critical attention", selected_critical),
            ("Busy state", selected_busy),
        ] {
            assert_eq!(
                style.bg, warning.bg,
                "Theme {expected_theme} added a selected background to {role}"
            );
        }
        assert_ne!(
            selected_critical, selected_busy,
            "Theme {expected_theme} let selection merge Attention and lifecycle roles"
        );
    }
}
