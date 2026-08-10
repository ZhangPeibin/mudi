//! SessionMesh entry point.
//!
//! The TUI opens immediately. A background task performs the initial (potentially long)
//! historical index with visible progress, then keeps the index fresh with cheap
//! incremental scans ~every 2s. The render loop never blocks on IO — it reads the latest
//! snapshot from shared state each frame.

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use sessionmesh::application::ApplicationController;
use sessionmesh::clipboard::{Clipboard, MacosClipboard};
use sessionmesh::config::{theme_override_from_args, Config};
use sessionmesh::handoff::{
    DetachedWorkerLauncher, HandoffPaths, HandoffService, MacosCurrentRunControl,
    ProviderExecutables, WorkerLauncher,
};
use sessionmesh::handoff_skill::ManagedHandoffSkills;
use sessionmesh::managed_sessions::{
    discover_tmux_executable, run_attach_from_args, MacosManagedTerminalAccess,
    ManagedCurrentRunAccessService, ManagedCurrentRunVerifier, ManagedLaunchStore,
    ManagedSessionService, SystemManagedCurrentRunEvidence, TmuxLaunchBroker,
};
use sessionmesh::read_model::{InMemoryReadModelCache, ReadModelCache};
use sessionmesh::refresh::RefreshService;
use sessionmesh::successor::{MacosTerminalLauncher, TerminalLauncher};
use sessionmesh::ui::{draw, Ui};

/// Shared read-only projection published by refresh and consumed by the UI.
type Shared = Arc<dyn ReadModelCache>;

#[tokio::main]
async fn main() -> Result<()> {
    if run_attach_from_args(std::env::args_os())? {
        return Ok(());
    }
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
    let terminal_launcher = MacosTerminalLauncher::new(
        sessionmesh_executable.clone(),
        handoff_paths.task_store.clone(),
    );
    let provider_executables = ProviderExecutables::discover();
    let tmux_executable = discover_tmux_executable().unwrap_or_default();
    let managed_sessions = ManagedSessionService::new(
        ManagedLaunchStore::open(cfg.managed_launch_store_path())?,
        provider_executables.clone(),
        Box::new(TmuxLaunchBroker::new(tmux_executable.clone())),
        Box::new(MacosManagedTerminalAccess::new(tmux_executable.clone())),
    );
    let managed_current_runs = ManagedCurrentRunAccessService::new(
        ManagedLaunchStore::open(cfg.managed_launch_store_path())?,
        ManagedCurrentRunVerifier::new(cfg.clone(), Box::new(SystemManagedCurrentRunEvidence)),
        Box::new(MacosManagedTerminalAccess::new(tmux_executable)),
    );
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
    let handoff = HandoffService::open_with_current_run_control(
        handoff_paths,
        provider_executables,
        worker_launcher,
        Arc::new(MacosCurrentRunControl::new(claude_sessions_dir)),
    )?
    .with_managed_handoff_skills(managed_handoff_skills);
    let mut controller = ApplicationController::new(handoff, terminal_launcher, managed_sessions)
        .with_managed_current_run_access(managed_current_runs);
    controller.reconcile_tasks(chrono::Utc::now().timestamp_millis())?;
    let shared: Shared = Arc::new(InMemoryReadModelCache::default());
    // Channel to request an out-of-band rescan (the `r` key).
    let (rescan_tx, rescan_rx) = mpsc::unbounded_channel::<()>();

    // Background collector task.
    let bg = tokio::spawn(collector_loop(cfg.clone(), shared.clone(), rescan_rx));

    // Run the TUI on the current thread.
    let result = run_tui(shared.clone(), rescan_tx, ui, &mut controller);

    bg.abort();
    result
}

/// Background loop: refresh and publish a structured projection every ~2s.
async fn collector_loop(cfg: Config, shared: Shared, mut rescan_rx: mpsc::UnboundedReceiver<()>) {
    let mut refresh = RefreshService::new(cfg);

    loop {
        refresh.refresh_and_publish(shared.as_ref());

        // Wait ~2s, but wake early on an explicit rescan request.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = rescan_rx.recv() => {}
        }
    }
}

fn run_tui<L: WorkerLauncher, T: TerminalLauncher>(
    shared: Shared,
    rescan_tx: mpsc::UnboundedSender<()>,
    mut ui: Ui,
    controller: &mut ApplicationController<L, T>,
) -> Result<()> {
    enable_raw_mode()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
    };
    if let Err(error) = enter_tui_screen(terminal.backend_mut()) {
        let _ = leave_tui_screen(terminal.backend_mut());
        let _ = disable_raw_mode();
        return Err(error.into());
    }

    let mut clipboard = MacosClipboard;
    let res = event_loop(
        &mut terminal,
        &mut ui,
        &shared,
        &rescan_tx,
        &mut clipboard,
        controller,
    );

    let leave_result = leave_tui_screen(terminal.backend_mut());
    let raw_mode_result = disable_raw_mode();
    let cursor_result = terminal.show_cursor();
    res?;
    leave_result?;
    raw_mode_result?;
    cursor_result?;
    Ok(())
}

fn enter_tui_screen(writer: &mut impl io::Write) -> io::Result<()> {
    execute!(writer, EnterAlternateScreen, EnableMouseCapture)
}

fn leave_tui_screen(writer: &mut impl io::Write) -> io::Result<()> {
    execute!(writer, DisableMouseCapture, LeaveAlternateScreen)
}

fn event_loop<B: ratatui::backend::Backend, L: WorkerLauncher, T: TerminalLauncher>(
    terminal: &mut Terminal<B>,
    ui: &mut Ui,
    shared: &Shared,
    rescan_tx: &mpsc::UnboundedSender<()>,
    clipboard: &mut dyn Clipboard,
    controller: &mut ApplicationController<L, T>,
) -> Result<()> {
    loop {
        // Rendering reads the latest published projection without owning refresh state.
        let snap = shared.current();
        ui.clamp_selections(&snap);

        terminal.draw(|f| draw(f, &snap, ui))?;

        // Poll for input with a timeout so the screen refreshes even without keys.
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let input = event::read()?;
        let terminal_size = terminal.size()?;
        let terminal_area =
            ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height);

        let intent = ui.handle_event(input, terminal_area, &snap)?;
        if controller.handle(intent, snap.as_ref(), ui, clipboard, rescan_tx) {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_terminal_commands_pair_mouse_capture_with_the_alternate_screen() {
        let mut enter = Vec::new();
        enter_tui_screen(&mut enter).unwrap();
        let enter = String::from_utf8(enter).unwrap();
        assert!(enter.contains("\x1b[?1049h"));
        assert!(enter.contains("\x1b[?1000h"));
        assert!(enter.contains("\x1b[?1006h"));

        let mut leave = Vec::new();
        leave_tui_screen(&mut leave).unwrap();
        let leave = String::from_utf8(leave).unwrap();
        assert!(leave.contains("\x1b[?1006l"));
        assert!(leave.contains("\x1b[?1000l"));
        assert!(leave.contains("\x1b[?1049l"));
    }
}
