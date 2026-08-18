//! Application controller for declarative UI intents.

pub mod alerts;
pub mod attention;
pub mod handoff;
pub mod managed_sessions;
pub mod observation;
pub mod session_labels;
pub mod signals;
pub mod successor;
pub mod tasks;

use crate::adapters::macos::clipboard::Clipboard;
use crate::adapters::project::resolve_selected_directory;
use crate::application::handoff::{
    CompletionNotice, CompletionNotifier, HandoffService, MacosCompletionNotifier, WorkerLauncher,
};
use crate::application::managed_sessions::ManagedCurrentRunAccessService;
use crate::application::managed_sessions::ManagedLaunchTarget;
use crate::application::managed_sessions::ManagedSessionService;
use crate::application::observation::read_model::RefreshSnapshot;
use crate::application::successor::TerminalLauncher;
use crate::presentation::tui::{Ui, UiIntent};
use anyhow::Result;
use tokio::sync::mpsc;

/// Performs authorized effects requested by the UI and schedules a subsequent refresh.
pub struct ApplicationController<L, T> {
    handoff: HandoffService<L>,
    terminal_launcher: T,
    managed_sessions: ManagedSessionService,
    managed_current_runs: Option<ManagedCurrentRunAccessService>,
}

impl<L: WorkerLauncher, T: TerminalLauncher> ApplicationController<L, T> {
    pub fn new(
        handoff: HandoffService<L>,
        terminal_launcher: T,
        managed_sessions: ManagedSessionService,
    ) -> Self {
        Self {
            handoff,
            terminal_launcher,
            managed_sessions,
            managed_current_runs: None,
        }
    }

    pub fn with_managed_current_run_access(
        mut self,
        access: ManagedCurrentRunAccessService,
    ) -> Self {
        self.managed_current_runs = Some(access);
        self
    }

    pub fn reconcile_tasks(&mut self, now_ms: i64) -> Result<()> {
        self.handoff.reconcile_tasks(now_ms)?;
        Ok(())
    }

    /// Returns true only when the caller should leave the TUI.
    pub fn handle(
        &mut self,
        intent: UiIntent,
        snapshot: &RefreshSnapshot,
        ui: &mut Ui,
        clipboard: &mut dyn Clipboard,
        rescan_tx: &mpsc::UnboundedSender<()>,
    ) -> bool {
        match intent {
            UiIntent::None => false,
            UiIntent::Rescan => {
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::Copy { field, value } => {
                ui.copy_to_clipboard(clipboard, field, &value);
                false
            }
            UiIntent::AuthorizeHandoff {
                source_provider,
                source_session_id,
                successor_provider,
                end_current_run,
            } => {
                let message = match snapshot.sessions.iter().find(|session| {
                    session.provider == source_provider && session.id == source_session_id
                }) {
                    Some(session) => match self.handoff.authorize_session_to(
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
                false
            }
            UiIntent::AcknowledgeTask { task_id } => {
                if let Err(error) = self
                    .handoff
                    .acknowledge_task(&task_id, chrono::Local::now().timestamp_millis())
                {
                    ui.show_notice(format!("Task acknowledgement failed: {error}"));
                }
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::CancelTask { task_id } => {
                let message = match self
                    .handoff
                    .cancel_task(&task_id, chrono::Local::now().timestamp_millis())
                {
                    Ok(task) => format!(
                        "Handoff {}: {}",
                        short_task_id(&task.id),
                        task.state.label()
                    ),
                    Err(error) => format!("Task cancellation failed: {error}"),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::DeleteTask { task_id } => {
                let message = match self
                    .handoff
                    .delete_task(&task_id, chrono::Local::now().timestamp_millis())
                {
                    Ok(()) => format!("Deleted Task and Artifact {}", short_task_id(&task_id)),
                    Err(error) => format!("Task deletion failed: {error}"),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::RetryTask {
                predecessor_task_id,
                source_provider,
                source_session_id,
                successor_provider,
                end_current_run,
            } => {
                let message = match snapshot.sessions.iter().find(|session| {
                    session.provider == source_provider && session.id == source_session_id
                }) {
                    Some(session) => match self.handoff.retry_task_to(
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
                false
            }
            UiIntent::CopySuccessorCommand { task_id } => {
                match self.handoff.task(&task_id) {
                    Ok(Some(task)) => {
                        match crate::application::successor::render_successor_command(&task) {
                            Ok(command) => ui.copy_to_clipboard(
                                clipboard,
                                "Successor Launch Command",
                                &command,
                            ),
                            Err(error) => {
                                ui.show_notice(format!("Copy Command unavailable: {error}"))
                            }
                        }
                    }
                    Ok(None) => ui.show_notice("Copy Command unavailable: Task is missing"),
                    Err(error) => ui.show_notice(format!("Copy Command unavailable: {error}")),
                }
                false
            }
            UiIntent::OpenSuccessorTerminal { task_id } => {
                let message = match self
                    .terminal_launcher
                    .open(&task_id, chrono::Local::now().timestamp_millis())
                {
                    Ok(()) => "Opened Successor in Terminal.app".to_string(),
                    Err(error) => format!("Open Terminal failed: {error}"),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::ResolveManagedNewDirectory { directory } => {
                let selection = resolve_selected_directory(&directory)
                    .and_then(canonical_context_target)
                    .map(Some)
                    .map_err(|error| error.to_string());
                ui.finish_managed_new_directory_selection(selection);
                false
            }
            UiIntent::StartManagedNew { request } => {
                let message = match self
                    .managed_sessions
                    .start_new(request, chrono::Local::now().timestamp_millis())
                {
                    Ok(outcome) if outcome.terminal_opened => format!(
                        "Started {} New in Terminal.app; Binding {}",
                        outcome.registration.provider.label(),
                        short_registration_id(&outcome.registration.id)
                    ),
                    Ok(outcome) => format!(
                        "Started {} New; Open Terminal failed: {}",
                        outcome.registration.provider.label(),
                        outcome
                            .registration
                            .terminal_error
                            .as_deref()
                            .unwrap_or("unknown error")
                    ),
                    Err(error) => format!("New failed: {error}"),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::StartManagedResume { request } => {
                let message = match self
                    .managed_sessions
                    .start_resume(request, chrono::Local::now().timestamp_millis())
                {
                    Ok(outcome) if outcome.terminal_opened => format!(
                        "Started {} Resume in Terminal.app; Binding {}",
                        outcome.registration.provider.label(),
                        short_registration_id(&outcome.registration.id)
                    ),
                    Ok(outcome) => format!(
                        "Started {} Resume; Open Terminal failed: {}",
                        outcome.registration.provider.label(),
                        outcome
                            .registration
                            .terminal_error
                            .as_deref()
                            .unwrap_or("unknown error")
                    ),
                    Err(error) => format!("Resume failed: {error}"),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::CancelManagedStart { registration_id } => {
                let message = match self.managed_sessions.cancel_start(&registration_id) {
                    Ok(()) => format!(
                        "Cancelled Managed Start {}",
                        short_registration_id(&registration_id)
                    ),
                    Err(error) => format!("Cancel Start failed: {error}"),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::OpenManagedTerminal { registration_id } => {
                let message = match self
                    .managed_sessions
                    .open_terminal(&registration_id, chrono::Local::now().timestamp_millis())
                {
                    Ok(outcome) if outcome.opened => {
                        "Opened Managed launch in Terminal.app".to_string()
                    }
                    Ok(outcome) => format!(
                        "Open Terminal failed: {}",
                        outcome
                            .registration
                            .terminal_error
                            .as_deref()
                            .unwrap_or("unknown error")
                    ),
                    Err(error) => format!("Open Terminal failed: {error}"),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::RetryManagedTerminal { registration_id } => {
                let message = match self
                    .managed_sessions
                    .retry_open_terminal(&registration_id, chrono::Local::now().timestamp_millis())
                {
                    Ok(outcome) if outcome.opened => {
                        "Reopened Managed launch in Terminal.app".to_string()
                    }
                    Ok(outcome) => format!(
                        "Retry Open Terminal failed: {}",
                        outcome
                            .registration
                            .terminal_error
                            .as_deref()
                            .unwrap_or("unknown error")
                    ),
                    Err(error) => format!("Retry Open Terminal failed: {error}"),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::CopyManagedLaunchDiagnostics { registration_id } => {
                match self.managed_sessions.copy_diagnostics(&registration_id) {
                    Ok(diagnostics) => {
                        ui.copy_to_clipboard(clipboard, "Managed Launch Diagnostics", &diagnostics)
                    }
                    Err(error) => ui.show_notice(format!("Copy Diagnostics failed: {error}")),
                }
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::OpenManagedCurrentRunTerminal {
                provider,
                native_session_id,
                opaque_ref,
            } => {
                let message = match self.managed_current_runs.as_mut() {
                    Some(access) => {
                        match access.open_terminal(provider, &native_session_id, &opaque_ref) {
                            Ok(()) => "Opened Managed Current Run in Terminal.app".to_string(),
                            Err(error) => format!("Open Terminal failed: {error}"),
                        }
                    }
                    None => "Open Terminal failed: Managed Current Run access is unavailable"
                        .to_string(),
                };
                ui.show_notice(message);
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::CopyManagedCurrentRunAttachCommand {
                provider,
                native_session_id,
                opaque_ref,
            } => {
                match self.managed_current_runs.as_ref() {
                    Some(access) => {
                        match access.attach_command(provider, &native_session_id, &opaque_ref) {
                            Ok(command) => ui.copy_to_clipboard(
                                clipboard,
                                "Managed Current Run Attach Command",
                                &command,
                            ),
                            Err(error) => {
                                ui.show_notice(format!("Copy Attach Command failed: {error}"))
                            }
                        }
                    }
                    None => ui.show_notice(
                        "Copy Attach Command failed: Managed Current Run access is unavailable",
                    ),
                }
                let _ = rescan_tx.send(());
                false
            }
            UiIntent::Quit => true,
        }
    }
}

fn canonical_context_target(
    context: crate::adapters::project::CanonicalProjectContext,
) -> Result<ManagedLaunchTarget> {
    Ok(ManagedLaunchTarget {
        working_directory: context.working_directory,
    })
}

fn short_registration_id(registration_id: &str) -> String {
    registration_id
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
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
