use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mudi::app::Snapshot;
use mudi::clipboard::Clipboard;
use mudi::collector::build_sessions;
use mudi::config::Config;
use mudi::domain::{
    ContextState, Project, ProjectIdentity, Provider, Session, SessionState, SourceFormat,
    SourceMetadata, Usage, Worktree,
};
use mudi::index::{Fingerprint, Index};
use mudi::models::ModelCatalog;
use mudi::providers::ParsedSession;
use mudi::ui::{draw, Ui, UiAction};
use ratatui::backend::TestBackend;
use ratatui::style::{Modifier, Style};
use ratatui::Terminal;

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
        provenance: mudi::domain::SessionProvenance::Observed,
        source: SourceMetadata {
            path: source_path.into(),
            format: SourceFormat::JsonLines,
        },
        project: Some(Project {
            identity: ProjectIdentity::CanonicalCwd(PathBuf::from("/workspace/mudi")),
            label: "mudi".into(),
        }),
        worktree: worktree_path.map(|root| Worktree {
            root: root.into(),
            branch: Some("feature/source-metadata".into()),
            writable: true,
        }),
        cwd: cwd.map(str::to_string),
        latest_model: None,
        state: SessionState::Idle,
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
    ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), snapshot)
        .unwrap();
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
    let mut first_match = None;
    for row in buffer.content().chunks(width) {
        for cells in row
            .windows(text_width)
            .filter(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == text)
        {
            let style = cells[0].style();
            if style.add_modifier.contains(Modifier::BOLD) {
                return style;
            }
            first_match.get_or_insert(style);
        }
    }
    first_match.unwrap_or_else(|| panic!("rendered text not found: {text}"))
}

#[test]
fn indexed_source_metadata_reaches_the_normalized_session() {
    let source_path =
        PathBuf::from("/Users/example/.claude/projects/-Users-example-code-mudi/session-123.jsonl");
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

    let sessions = build_sessions(&config, &index, &ModelCatalog::load(None));

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].source.path, source_path);
    assert_eq!(sessions[0].source.format, SourceFormat::JsonLines);
}

#[test]
fn detail_displays_complete_local_source_metadata_without_shortening() {
    let source_path = "/Users/example/.claude/projects/-Users-example-code-mudi/a-very-long-source-directory/native-session-123.jsonl";
    let worktree_path = "/Users/example/code/mudi-worktrees/a-very-long-feature-worktree";
    let cwd = "/Users/example/code/mudi-worktrees/a-very-long-feature-worktree/crates/mudi-ui";
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
fn detail_selects_real_fields_and_copies_the_complete_selected_value_directly() {
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

    for _ in 0..2 {
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
    }
    assert!(rendered_style_at(&ui, &snapshot, source_path)
        .add_modifier
        .contains(Modifier::BOLD));

    assert_eq!(
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &snapshot
        )
        .unwrap(),
        UiAction::Copy {
            field: "Source log",
            value: source_path.into(),
        }
    );
    let rendered = render(&ui, &snapshot, 120, 30);
    assert!(rendered.contains("Session Detail"));
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
    for _ in 0..2 {
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
    }
    let mut clipboard = RecordingClipboard {
        writes: vec![],
        error: None,
    };

    let action = ui
        .handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
    assert_eq!(
        action,
        UiAction::Copy {
            field: "Source log",
            value: source_path.into(),
        }
    );
    ui.copy_to_clipboard(&mut clipboard, "Source log", source_path);

    assert_eq!(clipboard.writes, vec![source_path]);
    assert!(render(&ui, &snapshot, 80, 30).contains("Copied Source log"));

    clipboard.error = Some("clipboard unavailable");
    ui.copy_to_clipboard(&mut clipboard, "Source log", source_path);

    assert!(render(&ui, &snapshot, 80, 30).contains("Copy failed: clipboard unavailable"));
    assert!(rendered_style_at(&ui, &snapshot, source_path)
        .add_modifier
        .contains(Modifier::BOLD));
}

#[test]
fn detail_and_direct_copy_expose_source_metadata_without_source_log_contents() {
    let source_path =
        std::env::temp_dir().join(format!("mudi-private-source-{}.jsonl", std::process::id()));
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
    for _ in 0..2 {
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
    }
    let action = ui
        .handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

    assert!(!detail.contains(private_content));
    assert_eq!(
        action,
        UiAction::Copy {
            field: "Source log",
            value: source_path_text,
        }
    );

    std::fs::remove_file(source_path).unwrap();
}
