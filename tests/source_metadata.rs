use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::style::{Modifier, Style};
use ratatui::Terminal;
use tokenplay::app::Snapshot;
use tokenplay::clipboard::Clipboard;
use tokenplay::collector::build_sessions;
use tokenplay::config::Config;
use tokenplay::domain::{
    ContextState, Provider, Session, SessionState, SourceFormat, SourceMetadata, Usage, Worktree,
};
use tokenplay::index::{Fingerprint, Index};
use tokenplay::pricing::RateCard;
use tokenplay::providers::ParsedSession;
use tokenplay::ui::{draw, Ui, UiAction};

struct RecordingClipboard {
    writes: Vec<String>,
    error: Option<&'static str>,
}

impl Clipboard for RecordingClipboard {
    fn write_text(&mut self, value: &str) -> std::io::Result<()> {
        self.writes.push(value.to_string());
        match self.error {
            Some(message) => Err(std::io::Error::other(message)),
            None => Ok(()),
        }
    }
}

fn session_with_metadata(
    source_path: &str,
    worktree_path: Option<&str>,
    cwd: Option<&str>,
    native_id: &str,
) -> Session {
    Session {
        provider: Provider::ClaudeCode,
        id: native_id.into(),
        source: SourceMetadata {
            path: source_path.into(),
            format: SourceFormat::JsonLines,
        },
        project: None,
        worktree: worktree_path.map(|root| Worktree {
            root: root.into(),
            branch: Some("feature/source-metadata".into()),
            writable: true,
        }),
        cwd: cwd.map(str::to_string),
        latest_model: None,
        state: SessionState::Exited,
        context: ContextState::default(),
        usage: Usage::default(),
        per_model: vec![],
        current_run_usage: None,
        compactions: vec![],
        last_activity_ms: 0,
        cost: None,
    }
}

fn open_detail(snapshot: &Snapshot) -> Ui {
    let mut ui = Ui::default();
    for code in [KeyCode::Char('2'), KeyCode::Enter] {
        ui.handle_key(KeyEvent::new(code, KeyModifiers::NONE), snapshot)
            .unwrap();
    }
    ui
}

fn render(ui: &Ui, snapshot: &Snapshot, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, snapshot, ui)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn rendered_style_at(ui: &Ui, snapshot: &Snapshot, text: &str) -> Style {
    let backend = TestBackend::new(80, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, snapshot, ui)).unwrap();
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    let text_width = text.chars().count();
    for row in buffer.content().chunks(width) {
        if let Some(cells) = row
            .windows(text_width)
            .find(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == text)
        {
            return cells[0].style();
        }
    }
    panic!("rendered text not found: {text}");
}

#[test]
fn indexed_source_metadata_reaches_the_normalized_session() {
    let source_path = PathBuf::from(
        "/Users/example/.claude/projects/-Users-example-code-tokenplay/session-123.jsonl",
    );
    let parsed = ParsedSession {
        provider: Provider::ClaudeCode,
        id: "session-123".into(),
        cwd: None,
        latest_model: None,
        context: ContextState::default(),
        usage: Usage::default(),
        per_model: vec![],
        compactions: vec![],
        last_activity_ms: 0,
    };
    let mut index = Index::open_in_memory().unwrap();
    index
        .upsert(
            &parsed,
            &source_path,
            Fingerprint {
                mtime_ns: 1,
                size: 2,
            },
        )
        .unwrap();
    let config = Config {
        claude_dir: PathBuf::from("/missing/claude"),
        codex_dir: PathBuf::from("/missing/codex"),
        data_dir: PathBuf::from("/missing/data"),
        extra_dirs: vec![],
    };

    let sessions = build_sessions(&config, &index, &RateCard::load(None));

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].source.path, source_path);
    assert_eq!(sessions[0].source.format, SourceFormat::JsonLines);
}

#[test]
fn detail_displays_complete_local_source_metadata_without_shortening() {
    let source_path = "/Users/example/.claude/projects/-Users-example-code-tokenplay/a-very-long-source-directory/native-session-123.jsonl";
    let worktree_path = "/Users/example/code/tokenplay-worktrees/a-very-long-feature-worktree";
    let cwd =
        "/Users/example/code/tokenplay-worktrees/a-very-long-feature-worktree/crates/tokenplay-ui";
    let native_id = "native-session-123-with-a-complete-provider-identifier";
    let snapshot = Snapshot {
        sessions: vec![session_with_metadata(
            source_path,
            Some(worktree_path),
            Some(cwd),
            native_id,
        )],
        ready: true,
        ..Snapshot::default()
    };
    let ui = open_detail(&snapshot);
    let rendered = render(&ui, &snapshot, 80, 40);
    let without_layout_whitespace: String = rendered
        .chars()
        .filter(|character| {
            !character.is_whitespace() && !matches!(character, '│' | '┌' | '┐' | '└' | '┘' | '─')
        })
        .collect();

    for expected in [source_path, worktree_path, cwd, native_id, "JSONL"] {
        assert!(
            without_layout_whitespace.contains(expected),
            "Detail should display the complete value {expected:?}\n{rendered}"
        );
    }
}

#[test]
fn detail_copy_menu_shows_all_fields_and_marks_unavailable_values() {
    let snapshot = Snapshot {
        sessions: vec![session_with_metadata(
            "/logs/native-session-123.jsonl",
            None,
            None,
            "native-session-123",
        )],
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = open_detail(&snapshot);

    ui.handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();

    let rendered = render(&ui, &snapshot, 80, 30);
    for expected in [
        "Copy source metadata",
        "Source log",
        "Worktree",
        "cwd",
        "Session ID",
    ] {
        assert!(
            rendered.contains(expected),
            "Copy menu should show {expected:?}"
        );
    }
    assert_eq!(rendered.matches("Unavailable").count(), 2);
}

#[test]
fn copy_menu_navigation_skips_unavailable_fields_and_escape_returns_to_detail() {
    let snapshot = Snapshot {
        sessions: vec![session_with_metadata(
            "/logs/native-session-123.jsonl",
            None,
            None,
            "native-session-123",
        )],
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = open_detail(&snapshot);
    for code in [KeyCode::Char('c'), KeyCode::Down] {
        ui.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &snapshot)
            .unwrap();
    }

    let is_selected = |ui: &Ui, text: &str| {
        rendered_style_at(ui, &snapshot, text)
            .add_modifier
            .contains(Modifier::BOLD)
    };
    assert!(is_selected(&ui, "Session ID"));
    for (key, selected) in [
        (KeyCode::Up, "Source log"),
        (KeyCode::Char('j'), "Session ID"),
        (KeyCode::Char('k'), "Source log"),
        (KeyCode::Down, "Session ID"),
    ] {
        ui.handle_key(KeyEvent::new(key, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(is_selected(&ui, selected));
    }

    ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
        .unwrap();
    let rendered = render(&ui, &snapshot, 80, 30);
    assert!(!rendered.contains("Copy source metadata"));
    assert!(rendered.contains("Session detail"));
}

#[test]
fn copy_menu_requests_the_exact_selected_value_and_returns_to_detail() {
    let source_path = "/logs/a-very-long/native-session-123.jsonl";
    let worktree_path = "/worktrees/a-very-long-feature";
    let cwd = "/worktrees/a-very-long-feature/crates/tokenplay-ui";
    let native_id = "native-session-123";
    let snapshot = Snapshot {
        sessions: vec![session_with_metadata(
            source_path,
            Some(worktree_path),
            Some(cwd),
            native_id,
        )],
        ready: true,
        ..Snapshot::default()
    };

    for (down_count, field, expected_value) in [
        (0, "Source log", source_path),
        (1, "Worktree", worktree_path),
        (2, "cwd", cwd),
        (3, "Session ID", native_id),
    ] {
        let mut ui = open_detail(&snapshot);
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        for _ in 0..down_count {
            ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
                .unwrap();
        }

        let action = ui
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        assert_eq!(
            action,
            UiAction::Copy {
                field,
                value: expected_value.to_string(),
            }
        );
        let rendered = render(&ui, &snapshot, 80, 30);
        assert!(!rendered.contains("Copy source metadata"));
        assert!(rendered.contains("Session detail"));
    }
}

#[test]
fn clipboard_success_and_failure_are_reported_without_propagating_errors() {
    let source_path = "/logs/native-session-123.jsonl";
    let snapshot = Snapshot {
        sessions: vec![session_with_metadata(
            source_path,
            None,
            None,
            "native-session-123",
        )],
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = open_detail(&snapshot);
    let before = snapshot.sessions[0].clone();
    let mut clipboard = RecordingClipboard {
        writes: vec![],
        error: None,
    };

    ui.copy_to_clipboard(&mut clipboard, "Source log", source_path);

    assert_eq!(clipboard.writes, vec![source_path]);
    assert!(render(&ui, &snapshot, 80, 30).contains("Copied Source log"));

    clipboard.error = Some("clipboard unavailable");
    ui.copy_to_clipboard(&mut clipboard, "cwd", "/worktrees/tokenplay");

    assert!(render(&ui, &snapshot, 80, 30).contains("Copy failed: clipboard unavailable"));
    assert_eq!(snapshot.sessions[0].id, before.id);
    assert_eq!(snapshot.sessions[0].source, before.source);
}

#[test]
fn detail_and_copy_expose_source_metadata_without_source_log_contents() {
    let source_path = std::env::temp_dir().join(format!(
        "tokenplay-private-source-{}.jsonl",
        std::process::id()
    ));
    let private_content = "PRIVATE_PROMPT_RESPONSE_TOOL_SOURCE_SENTINEL";
    std::fs::write(
        &source_path,
        format!("{{\"prompt\":\"{private_content}\"}}\n"),
    )
    .unwrap();
    let source_path_text = source_path.to_string_lossy().into_owned();
    let snapshot = Snapshot {
        sessions: vec![session_with_metadata(
            &source_path_text,
            None,
            None,
            "native-session-123",
        )],
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = open_detail(&snapshot);
    let detail = render(&ui, &snapshot, 120, 30);
    ui.handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
        &snapshot,
    )
    .unwrap();
    let menu = render(&ui, &snapshot, 120, 30);
    let action = ui
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    assert!(!detail.contains(private_content));
    assert!(!menu.contains(private_content));
    assert_eq!(
        action,
        UiAction::Copy {
            field: "Source log",
            value: source_path_text,
        }
    );

    std::fs::remove_file(source_path).unwrap();
}

#[test]
fn a_value_that_becomes_unavailable_cannot_be_copied() {
    let mut snapshot = Snapshot {
        sessions: vec![session_with_metadata(
            "/logs/native-session-123.jsonl",
            Some("/worktrees/tokenplay"),
            Some("/worktrees/tokenplay/crates/ui"),
            "native-session-123",
        )],
        ready: true,
        ..Snapshot::default()
    };
    let mut ui = open_detail(&snapshot);
    for code in [KeyCode::Char('c'), KeyCode::Down] {
        ui.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &snapshot)
            .unwrap();
    }
    snapshot.sessions[0].worktree = None;

    let action = ui
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
        .unwrap();

    assert_eq!(action, UiAction::None);
    let rendered = render(&ui, &snapshot, 80, 30);
    assert!(rendered.contains("Copy source metadata"));
    assert!(rendered.contains("Unavailable"));
}
