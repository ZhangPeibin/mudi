//! SessionMesh entry point.
//!
//! The TUI opens immediately. A background task performs the initial (potentially long)
//! historical index with visible progress, then keeps the index fresh with cheap
//! incremental scans ~every 2s. The render loop never blocks on IO — it reads the latest
//! snapshot from shared state each frame.

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

use sessionmesh::alerts::{self, AlertEngine};
use sessionmesh::app::Snapshot;
use sessionmesh::attention::AttentionEngine;
use sessionmesh::clipboard::{Clipboard, MacosClipboard};
use sessionmesh::collector;
use sessionmesh::config::{theme_override_from_args, Config};
use sessionmesh::handoff::{
    CompletionNotice, CompletionNotifier, DetachedWorkerLauncher, HandoffPaths, HandoffService,
    MacosCompletionNotifier, MacosCurrentRunControl, ProviderExecutables, WorkerLauncher,
};
use sessionmesh::handoff_skill::ManagedHandoffSkills;
use sessionmesh::index::Index;
use sessionmesh::models::ModelCatalog;
use sessionmesh::signals::RecentSignalsEngine;
use sessionmesh::successor::{MacosTerminalLauncher, TerminalLauncher};
use sessionmesh::tasks::TaskStore;
use sessionmesh::ui::{draw, Ui, UiAction};

/// Shared snapshot updated by the collector, read by the UI.
type Shared = Arc<Mutex<Snapshot>>;

#[tokio::main]
async fn main() -> Result<()> {
    if sessionmesh::successor::run_successor_from_args(std::env::args_os())? {
        return Ok(());
    }
    if sessionmesh::handoff::run_worker_from_args(std::env::args_os())? {
        return Ok(());
    }
    let cfg = Config::load();
    let theme_override = theme_override_from_args(std::env::args());
    let ui = Ui::load(cfg.profile_path(), theme_override.as_deref())?;
    let handoff_paths = HandoffPaths {
        task_store: cfg.task_store_path(),
        artifact_root: cfg.artifact_root(),
    };
    let sessionmesh_executable = std::env::current_exe()?;
    let mut terminal_launcher = MacosTerminalLauncher::new(
        sessionmesh_executable.clone(),
        handoff_paths.task_store.clone(),
    );
    let provider_executables = ProviderExecutables::discover();
    let claude_sessions_dir = cfg.claude_sessions_dir();
    let user_home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let managed_handoff_skills = ManagedHandoffSkills::for_user_home(&user_home, &cfg.claude_dir);
    let worker_launcher = DetachedWorkerLauncher::new(
        sessionmesh_executable,
        handoff_paths.clone(),
        provider_executables.clone(),
        claude_sessions_dir.clone(),
    )
    .with_managed_handoff_skills(managed_handoff_skills.clone())
    .with_notifications();
    let mut handoff = HandoffService::open_with_current_run_control(
        handoff_paths,
        provider_executables,
        worker_launcher,
        Arc::new(MacosCurrentRunControl::new(claude_sessions_dir)),
    )?
    .with_managed_handoff_skills(managed_handoff_skills);
    handoff.reconcile_tasks(chrono::Utc::now().timestamp_millis())?;
    let shared: Shared = Arc::new(Mutex::new(Snapshot {
        status: "Starting…".to_string(),
        ..Default::default()
    }));

    // Channel to request an out-of-band rescan (the `r` key).
    let (rescan_tx, rescan_rx) = mpsc::unbounded_channel::<()>();

    // Background collector task.
    let bg = tokio::spawn(collector_loop(cfg.clone(), shared.clone(), rescan_rx));

    // Run the TUI on the current thread.
    let result = run_tui(
        shared.clone(),
        rescan_tx,
        ui,
        &mut handoff,
        &mut terminal_launcher,
    );

    bg.abort();
    result
}

/// Background loop: scan → build → alert → publish, repeating ~every 2s.
async fn collector_loop(cfg: Config, shared: Shared, mut rescan_rx: mpsc::UnboundedReceiver<()>) {
    // Index and alert engine live only here (single-owner, off the UI thread).
    let mut index = match Index::open(&cfg.index_path()) {
        Ok(i) => i,
        Err(e) => {
            if let Ok(mut s) = shared.lock() {
                s.status = format!("index error: {e}");
            }
            return;
        }
    };
    let task_store = match TaskStore::open(&cfg.task_store_path()) {
        Ok(store) => store,
        Err(error) => {
            if let Ok(mut snapshot) = shared.lock() {
                snapshot.status = format!("Task Store error: {error}");
            }
            return;
        }
    };
    let models = ModelCatalog::load(Some(&cfg.model_catalog_override_path()));
    let mut alert_engine = AlertEngine::new(cfg.label_store_path());
    let mut attention_engine = AttentionEngine::default();
    let mut recent_signals_engine = RecentSignalsEngine::default();
    let mut first = true;

    loop {
        {
            if let Ok(mut s) = shared.lock() {
                s.status = if first {
                    "Indexing…".into()
                } else {
                    "Refreshing…".into()
                };
            }
        }

        let stats = collector::refresh(&cfg, &mut index);
        let mut sessions = collector::build_sessions(&cfg, &index, &models);
        collector::sort_for_display(&mut sessions);

        // Alerts + notifications run off the raw context of open sessions.
        alert_engine.update(&sessions);
        let alert_counts = alerts::counts(&sessions);

        let now = chrono::Local::now().timestamp_millis();
        let tasks = match task_store.list() {
            Ok(tasks) => tasks,
            Err(error) => {
                if let Ok(mut snapshot) = shared.lock() {
                    snapshot.status = format!("Task Store error: {error}");
                }
                return;
            }
        };
        let attention = attention_engine.update(&sessions, now);
        let recent_signals = recent_signals_engine.update(&sessions, now).to_vec();
        let snap = Snapshot {
            sessions,
            tasks,
            recent_signals,
            attention: Some(attention),
            stats,
            alerts: alert_counts,
            status: format!(
                "watching · {} sessions · {} parsed / {} skipped{}",
                stats.discovered,
                stats.parsed,
                stats.skipped,
                if stats.pruned > 0 {
                    format!(" · {} pruned", stats.pruned)
                } else {
                    String::new()
                },
            ),
            generated_ms: now,
            ready: true,
        };
        if let Ok(mut s) = shared.lock() {
            *s = snap;
        }
        first = false;

        // Wait ~2s, but wake early on an explicit rescan request.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = rescan_rx.recv() => {}
        }
    }
}

fn run_tui<L: WorkerLauncher>(
    shared: Shared,
    rescan_tx: mpsc::UnboundedSender<()>,
    mut ui: Ui,
    handoff: &mut HandoffService<L>,
    terminal_launcher: &mut dyn TerminalLauncher,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    enter_tui_screen(&mut stdout)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut clipboard = MacosClipboard;
    let res = event_loop(
        &mut terminal,
        &mut ui,
        &shared,
        &rescan_tx,
        &mut clipboard,
        handoff,
        terminal_launcher,
    );

    disable_raw_mode()?;
    leave_tui_screen(terminal.backend_mut())?;
    terminal.show_cursor()?;
    res
}

fn enter_tui_screen(writer: &mut impl io::Write) -> io::Result<()> {
    execute!(writer, EnterAlternateScreen)
}

fn leave_tui_screen(writer: &mut impl io::Write) -> io::Result<()> {
    execute!(writer, LeaveAlternateScreen)
}

fn event_loop<B: ratatui::backend::Backend, L: WorkerLauncher>(
    terminal: &mut Terminal<B>,
    ui: &mut Ui,
    shared: &Shared,
    rescan_tx: &mpsc::UnboundedSender<()>,
    clipboard: &mut dyn Clipboard,
    handoff: &mut HandoffService<L>,
    terminal_launcher: &mut dyn TerminalLauncher,
) -> Result<()> {
    loop {
        // Snapshot the shared state (cheap clone) so rendering holds no lock.
        let snap = shared.lock().map(|s| s.clone()).unwrap_or_default();
        ui.clamp_selections(&snap);

        terminal.draw(|f| draw(f, &snap, ui))?;

        // Poll for input with a timeout so the screen refreshes even without keys.
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match ui.handle_key(key, &snap)? {
            UiAction::None => {}
            UiAction::Rescan => {
                let _ = rescan_tx.send(());
            }
            UiAction::Copy { field, value } => {
                ui.copy_to_clipboard(clipboard, field, &value);
            }
            UiAction::AuthorizeHandoff {
                source_provider,
                source_session_id,
                successor_provider,
                end_current_run,
            } => {
                let message = match snap.sessions.iter().find(|session| {
                    session.provider == source_provider && session.id == source_session_id
                }) {
                    Some(session) => match handoff.authorize_session_to(
                        session,
                        successor_provider,
                        end_current_run,
                        chrono::Local::now().timestamp_millis(),
                    ) {
                        Ok(task) => {
                            if task.state.is_terminal() {
                                let _ = MacosCompletionNotifier.notify(&CompletionNotice {
                                    task_id: task.id.clone(),
                                    state: task.state,
                                    source_provider: task.source_provider,
                                    source_session_id: task.source_session_id.clone(),
                                });
                            }
                            format!(
                                "Handoff {}: {}",
                                short_task_id(&task.id),
                                task.state.label()
                            )
                        }
                        Err(error) => error.to_string(),
                    },
                    None => "Handoff is ineligible: source Session is unavailable".to_string(),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
            }
            UiAction::AcknowledgeTask { task_id } => {
                if let Err(error) =
                    handoff.acknowledge_task(&task_id, chrono::Local::now().timestamp_millis())
                {
                    ui.show_notice(format!("Task acknowledgement failed: {error}"));
                }
                let _ = rescan_tx.send(());
            }
            UiAction::CancelTask { task_id } => {
                let message =
                    match handoff.cancel_task(&task_id, chrono::Local::now().timestamp_millis()) {
                        Ok(task) => format!(
                            "Handoff {}: {}",
                            short_task_id(&task.id),
                            task.state.label()
                        ),
                        Err(error) => format!("Task cancellation failed: {error}"),
                    };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
            }
            UiAction::DeleteTask { task_id } => {
                let message =
                    match handoff.delete_task(&task_id, chrono::Local::now().timestamp_millis()) {
                        Ok(()) => format!("Deleted Task and Artifact {}", short_task_id(&task_id)),
                        Err(error) => format!("Task deletion failed: {error}"),
                    };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
            }
            UiAction::RetryTask {
                predecessor_task_id,
                source_provider,
                source_session_id,
                successor_provider,
                end_current_run,
            } => {
                let message = match snap.sessions.iter().find(|session| {
                    session.provider == source_provider && session.id == source_session_id
                }) {
                    Some(session) => match handoff.retry_task_to(
                        &predecessor_task_id,
                        session,
                        successor_provider,
                        end_current_run,
                        chrono::Local::now().timestamp_millis(),
                    ) {
                        Ok(task) => format!(
                            "Handoff Retry {}: {}",
                            short_task_id(&task.id),
                            task.state.label()
                        ),
                        Err(error) => error.to_string(),
                    },
                    None => "Handoff Retry is ineligible: source Session is unavailable".into(),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
            }
            UiAction::CopySuccessorCommand { task_id } => match handoff.task(&task_id) {
                Ok(Some(task)) => match sessionmesh::successor::render_successor_command(&task) {
                    Ok(command) => {
                        ui.copy_to_clipboard(clipboard, "Successor Launch Command", &command)
                    }
                    Err(error) => ui.show_notice(format!("Copy Command unavailable: {error}")),
                },
                Ok(None) => ui.show_notice("Copy Command unavailable: Task is missing"),
                Err(error) => ui.show_notice(format!("Copy Command unavailable: {error}")),
            },
            UiAction::OpenSuccessorTerminal { task_id } => {
                let message = match terminal_launcher
                    .open(&task_id, chrono::Local::now().timestamp_millis())
                {
                    Ok(()) => "Opened Successor in Terminal.app".to_string(),
                    Err(error) => format!("Open Terminal failed: {error}"),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
            }
            UiAction::Quit => return Ok(()),
        }
    }
}

fn short_task_id(task_id: &str) -> String {
    task_id
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_terminal_commands_preserve_native_mouse_text_selection() {
        let mut enter = Vec::new();
        enter_tui_screen(&mut enter).unwrap();
        let enter = String::from_utf8(enter).unwrap();
        assert!(enter.contains("\x1b[?1049h"));
        assert!(!enter.contains("\x1b[?1000h"));
        assert!(!enter.contains("\x1b[?1006h"));

        let mut leave = Vec::new();
        leave_tui_screen(&mut leave).unwrap();
        let leave = String::from_utf8(leave).unwrap();
        assert!(!leave.contains("\x1b[?1006l"));
        assert!(!leave.contains("\x1b[?1000l"));
        assert!(leave.contains("\x1b[?1049l"));
    }
}
