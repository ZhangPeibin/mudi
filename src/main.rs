// Mudi entry point.
//
// The TUI opens immediately. A background task performs the initial (potentially long)
// historical index with visible progress, then keeps the index fresh with cheap
// incremental scans ~every 2s. The render loop never blocks on IO - it reads the latest
// snapshot from shared state each frame.

use anyhow::Result;
use crossterm::event;
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use mudi::adapters::config::Config;
use mudi::adapters::handoff::{
    open_handoff_service_with_current_run_control, DetachedWorkerLauncher, MacosCompletionNotifier,
    MacosCurrentRunControl,
};
use mudi::adapters::handoff_worker::run_worker_from_args;
use mudi::adapters::macos::clipboard::MacosClipboard;
use mudi::adapters::managed_sessions::{
    discover_tmux_executable, run_attach_from_args, MacosManagedTerminalAccess,
    SystemManagedCurrentRunEvidence, SystemManagedResumeProviderEvidence, SystemProcessEvidence,
    TmuxLaunchBroker,
};
use mudi::adapters::persistence::managed_sessions::ManagedLaunchStore;
use mudi::adapters::persistence::session_metadata::SessionLabels;
use mudi::adapters::persistence::state::StateDatabase;
use mudi::adapters::project::CanonicalManagedTargetResolver;
use mudi::adapters::refresh::{open_refresh_service, SystemClock};
use mudi::adapters::skills::handoff::ManagedHandoffSkills;
use mudi::adapters::skills::ManagedSkillCatalog;
use mudi::adapters::successor::{
    run_successor_from_args, MacosTerminalLauncher, RegistrySuccessorRuntime,
};

use mudi::application::clipboard::ClipboardPort;
use mudi::application::handlers::{WorkflowHandlers, WorkflowPorts};
use mudi::application::handoff::{
    HandoffPaths, HandoffService, ProviderExecutables, WorkerLauncher,
};
use mudi::application::managed_sessions::{
    ManagedCurrentRunAccessService, ManagedCurrentRunVerifier, ManagedResumeVerifier,
    ManagedSessionService, SystemManagedResumeEvidence,
};
use mudi::application::observation::read_model::{InMemoryReadModelCache, ReadModelCache};
use mudi::application::successor::TerminalLauncher;
use mudi::presentation::tui::{draw, route_action, Ui};

/// Shared read-only projection published by refresh and consumed by the UI.
type SharedReadModelCache = Arc<dyn ReadModelCache>;
type TuiHandlers = WorkflowHandlers<DetachedWorkerLauncher, MacosTerminalLauncher>;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load();
    StateDatabase::open_data_dir(&config.data_dir)?;
    if run_non_tui_mode()? {
        return Ok(());
    }

    let runtime = TuiRuntime::build(config)?;
    runtime.run()
}

/// Dispatch process modes that do not start the TUI.
fn run_non_tui_mode() -> Result<bool> {
    if run_attach_from_args(std::env::args_os())? {
        return Ok(true);
    }
    if run_successor_from_args(std::env::args_os())? {
        return Ok(true);
    }
    if run_worker_from_args(std::env::args_os())? {
        return Ok(true);
    }
    Ok(false)
}

struct TuiRuntime {
    config: Config,
    ui: Ui,
    handlers: TuiHandlers,
    read_model: SharedReadModelCache,
}

impl TuiRuntime {
    fn build(config: Config) -> Result<Self> {
        let ui = Ui::load_with_session_metadata(
            config.profile_path(),
            Box::new(SessionLabels::load(config.label_store_path())),
        )?;
        let handlers = build_tui_handlers(&config)?;
        let read_model: SharedReadModelCache = Arc::new(InMemoryReadModelCache::default());

        Ok(Self {
            config,
            ui,
            handlers,
            read_model,
        })
    }

    fn run(self) -> Result<()> {
        let Self {
            config,
            ui,
            mut handlers,
            read_model,
        } = self;

        let (rescan_tx, rescan_rx) = mpsc::unbounded_channel::<()>();
        let collector = tokio::spawn(collector_loop(config, read_model.clone(), rescan_rx));

        let result = run_tui(read_model, rescan_tx, ui, &mut handlers);

        collector.abort();
        result
    }
}

fn build_tui_handlers(config: &Config) -> Result<TuiHandlers> {
    let handoff_paths = HandoffPaths {
        task_store: config.task_store_path(),
        artifact_root: config.artifact_root(),
    };
    let mudi_executable = std::env::current_exe()?;
    let terminal_launcher =
        MacosTerminalLauncher::new(mudi_executable.clone(), handoff_paths.task_store.clone());
    let provider_executables = ProviderExecutables::discover();

    let tmux_executable = discover_tmux_executable().unwrap_or_default();
    let (managed_sessions, managed_current_runs) =
        build_managed_session_services(config, &provider_executables, tmux_executable)?;
    let handoff =
        build_handoff_service(config, mudi_executable, handoff_paths, provider_executables)?;

    let ports = WorkflowPorts {
        clock: Box::new(SystemClock),
        completion_notifier: Box::new(MacosCompletionNotifier),
        successor_runtime: Arc::new(RegistrySuccessorRuntime),
        target_resolver: Box::new(CanonicalManagedTargetResolver),
        session_metadata: Box::new(SessionLabels::load(config.label_store_path())),
    };
    let handlers = WorkflowHandlers::new(handoff, terminal_launcher, managed_sessions, ports)
        .with_managed_current_run_access(managed_current_runs);
    handlers.reconcile_tasks()?;
    Ok(handlers)
}

fn build_managed_session_services(
    config: &Config,
    provider_executables: &ProviderExecutables,
    tmux_executable: PathBuf,
) -> Result<(ManagedSessionService, ManagedCurrentRunAccessService)> {
    let managed_sessions = ManagedSessionService::new(
        ManagedLaunchStore::open(config.managed_launch_store_path())?,
        provider_executables.clone(),
        Box::new(TmuxLaunchBroker::new(tmux_executable.clone())),
        Box::new(MacosManagedTerminalAccess::new(tmux_executable.clone())),
    )
    .with_resume_verifier(ManagedResumeVerifier::new(Box::new(
        SystemManagedResumeEvidence::new(
            Box::new(SystemProcessEvidence),
            Box::new(SystemManagedResumeProviderEvidence::new(config.clone())),
        ),
    )));
    let managed_current_runs = ManagedCurrentRunAccessService::new(
        ManagedLaunchStore::open(config.managed_launch_store_path())?,
        ManagedCurrentRunVerifier::new(Box::new(SystemManagedCurrentRunEvidence::new(
            config.clone(),
        ))),
        Box::new(MacosManagedTerminalAccess::new(tmux_executable)),
    );

    Ok((managed_sessions, managed_current_runs))
}

fn build_handoff_service(
    config: &Config,
    mudi_executable: PathBuf,
    handoff_paths: HandoffPaths,
    provider_executables: ProviderExecutables,
) -> Result<HandoffService<DetachedWorkerLauncher>> {
    let claude_sessions_dir = config.claude_sessions_dir();
    let user_home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let managed_handoff_skills =
        ManagedHandoffSkills::for_user_home(&user_home, &config.claude_dir);
    let worker_launcher = DetachedWorkerLauncher::new(
        mudi_executable,
        handoff_paths.clone(),
        provider_executables.clone(),
        claude_sessions_dir.clone(),
    )
    .with_managed_handoff_skills(managed_handoff_skills.clone())
    .with_notifications();
    let handoff = open_handoff_service_with_current_run_control(
        handoff_paths,
        provider_executables,
        worker_launcher,
        Arc::new(MacosCurrentRunControl::new(claude_sessions_dir)),
    )?
    .with_skill_installer(Arc::new(ManagedSkillCatalog::handoff(
        managed_handoff_skills,
    )));
    Ok(handoff)
}

/// Background loop: refresh and publish a structured projection every ~2s.
async fn collector_loop(
    cfg: Config,
    shared: SharedReadModelCache,
    mut rescan_rx: mpsc::UnboundedReceiver<()>,
) {
    let mut refresh = open_refresh_service(cfg);

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
    shared: SharedReadModelCache,
    rescan_tx: mpsc::UnboundedSender<()>,
    mut ui: Ui,
    handlers: &mut WorkflowHandlers<L, T>,
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
        handlers,
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
    execute!(
        writer,
        EnterAlternateScreen,
        // Crossterm's EnableMouseCapture also enables 1003, which reports every
        // unpressed pointer movement. Mudi only needs clicks, wheel events,
        // and button-motion events for divider dragging.
        Print(concat!(
            "\x1b[?1003l",
            "\x1b[?1000h",
            "\x1b[?1002h",
            "\x1b[?1015h",
            "\x1b[?1006h",
        ))
    )
}

fn leave_tui_screen(writer: &mut impl io::Write) -> io::Result<()> {
    execute!(
        writer,
        Print(concat!(
            "\x1b[?1006l",
            "\x1b[?1015l",
            "\x1b[?1002l",
            "\x1b[?1000l",
            "\x1b[?1003l",
        )),
        LeaveAlternateScreen
    )
}

fn event_loop<B: ratatui::backend::Backend, L: WorkerLauncher, T: TerminalLauncher>(
    terminal: &mut Terminal<B>,
    ui: &mut Ui,
    shared: &SharedReadModelCache,
    rescan_tx: &mpsc::UnboundedSender<()>,
    clipboard: &mut dyn ClipboardPort,
    handlers: &mut WorkflowHandlers<L, T>,
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

        let action = ui.handle_event(input, terminal_area, &snap)?;
        let mut request_rescan = || {
            let _ = rescan_tx.send(());
        };
        if route_action(
            action,
            snap.as_ref(),
            ui,
            clipboard,
            &mut request_rescan,
            handlers,
        ) {
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
        assert!(enter.contains("\x1b[?1002h"));
        assert!(!enter.contains("\x1b[?1003h"));
        assert!(enter.contains("\x1b[?1015h"));
        assert!(enter.contains("\x1b[?1006h"));

        let mut leave = Vec::new();
        leave_tui_screen(&mut leave).unwrap();
        let leave = String::from_utf8(leave).unwrap();
        assert!(leave.contains("\x1b[?1006l"));
        assert!(leave.contains("\x1b[?1015l"));
        assert!(leave.contains("\x1b[?1003l"));
        assert!(leave.contains("\x1b[?1002l"));
        assert!(leave.contains("\x1b[?1000l"));
        assert!(leave.contains("\x1b[?1049l"));
    }
}
