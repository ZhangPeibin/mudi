//! Routes declarative UI actions to typed application workflows.

use super::{Ui, UiAction};
use crate::application::clipboard::ClipboardPort;
use crate::application::handlers::{
    CurationRequest, HandoffAuthorizationRequest, HandoffRetryRequest, ManagedCurrentRunRequest,
    SessionMetadataUpdateRequest, WorkflowError, WorkflowHandlers,
};
use crate::application::handoff::WorkerLauncher;
use crate::application::observation::read_model::RefreshSnapshot;
use crate::application::successor::TerminalLauncher;
use crate::application::tasks::OperationTask;
use crate::domain::SessionIdentity;

/// Returns `true` only when the caller should leave the TUI.
pub fn route_action<L: WorkerLauncher, T: TerminalLauncher>(
    action: UiAction,
    snapshot: &RefreshSnapshot,
    ui: &mut Ui,
    clipboard: &mut dyn ClipboardPort,
    request_rescan: &mut dyn FnMut(),
    handlers: &mut WorkflowHandlers<L, T>,
) -> bool {
    match action {
        UiAction::None => false,
        UiAction::Rescan => {
            request_rescan();
            false
        }
        UiAction::Copy { field, value } => {
            ui.copy_to_clipboard(clipboard, field, &value);
            false
        }
        UiAction::AuthorizeHandoff {
            source_provider,
            source_session_id,
            successor_provider,
            end_current_run,
        } => {
            let message = match handlers.authorize_handoff(
                HandoffAuthorizationRequest {
                    source: SessionIdentity {
                        provider: source_provider,
                        native_id: source_session_id,
                    },
                    successor_provider,
                    end_current_run,
                },
                snapshot,
            ) {
                Ok(task) => handoff_status("Handoff", &task),
                Err(WorkflowError::TargetUnavailable(_)) => {
                    "Handoff is ineligible: source Session is unavailable".to_string()
                }
                Err(error) => error.to_string(),
            };
            ui.show_notice(message);
            request_rescan();
            false
        }
        UiAction::AcknowledgeTask { task_id } => {
            if let Err(error) = handlers.acknowledge_task(&task_id) {
                ui.show_notice(format!("Task acknowledgement failed: {error}"));
            }
            request_rescan();
            false
        }
        UiAction::CancelTask { task_id } => {
            let message = match handlers.cancel_task(&task_id) {
                Ok(task) => handoff_status("Handoff", &task),
                Err(error) => format!("Task cancellation failed: {error}"),
            };
            ui.show_notice(message);
            request_rescan();
            false
        }
        UiAction::DeleteTask { task_id } => {
            let message = match handlers.delete_task(&task_id) {
                Ok(()) => format!("Deleted Task and Artifact {}", short_id(&task_id)),
                Err(error) => format!("Task deletion failed: {error}"),
            };
            ui.show_notice(message);
            request_rescan();
            false
        }
        UiAction::RetryTask {
            predecessor_task_id,
            source_provider,
            source_session_id,
            successor_provider,
            end_current_run,
        } => {
            let message = match handlers.retry_handoff(
                HandoffRetryRequest {
                    predecessor_task_id,
                    source: SessionIdentity {
                        provider: source_provider,
                        native_id: source_session_id,
                    },
                    successor_provider,
                    end_current_run,
                },
                snapshot,
            ) {
                Ok(task) => handoff_status("Handoff Retry", &task),
                Err(WorkflowError::TargetUnavailable(_)) => {
                    "Handoff Retry is ineligible: source Session is unavailable".to_string()
                }
                Err(error) => error.to_string(),
            };
            ui.show_notice(message);
            request_rescan();
            false
        }
        UiAction::CopySuccessorCommand { task_id } => {
            match handlers.successor_command(&task_id) {
                Ok(command) => {
                    ui.copy_to_clipboard(clipboard, "Successor Launch Command", &command)
                }
                Err(error) => ui.show_notice(format!("Copy Command unavailable: {error}")),
            }
            false
        }
        UiAction::OpenSuccessorTerminal { task_id } => {
            let message = match handlers.open_successor_terminal(&task_id) {
                Ok(()) => "Opened Successor in Terminal.app".to_string(),
                Err(error) => format!("Open Terminal failed: {error}"),
            };
            ui.show_notice(message);
            request_rescan();
            false
        }
        UiAction::ResolveManagedNewDirectory { directory } => {
            let selection = handlers
                .resolve_managed_target(directory)
                .map(Some)
                .map_err(|error| error.to_string());
            ui.finish_managed_new_directory_selection(selection);
            false
        }
        UiAction::StartManagedNew { request } => {
            let message = match handlers.start_managed_new(request) {
                Ok(outcome) if outcome.terminal_opened => format!(
                    "Started {} New in Terminal.app; Binding {}",
                    outcome.registration.provider.label(),
                    short_id(&outcome.registration.id)
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
            request_rescan();
            false
        }
        UiAction::StartManagedResume { request } => {
            let message = match handlers.start_managed_resume(request) {
                Ok(outcome) if outcome.terminal_opened => format!(
                    "Started {} Resume in Terminal.app; Binding {}",
                    outcome.registration.provider.label(),
                    short_id(&outcome.registration.id)
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
            request_rescan();
            false
        }
        UiAction::CancelManagedStart { registration_id } => {
            let message = match handlers.cancel_managed_start(&registration_id) {
                Ok(()) => format!("Cancelled Managed Start {}", short_id(&registration_id)),
                Err(error) => format!("Cancel Start failed: {error}"),
            };
            ui.show_notice(message);
            request_rescan();
            false
        }
        UiAction::OpenManagedTerminal { registration_id } => {
            let message = match handlers.open_managed_terminal(&registration_id) {
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
            request_rescan();
            false
        }
        UiAction::RetryManagedTerminal { registration_id } => {
            let message = match handlers.retry_managed_terminal(&registration_id) {
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
            request_rescan();
            false
        }
        UiAction::CopyManagedLaunchDiagnostics { registration_id } => {
            match handlers.managed_launch_diagnostics(&registration_id) {
                Ok(diagnostics) => {
                    let text = format_managed_launch_diagnostics(&diagnostics);
                    ui.copy_to_clipboard(clipboard, "Managed Launch Diagnostics", &text)
                }
                Err(error) => ui.show_notice(format!("Copy Diagnostics failed: {error}")),
            }
            request_rescan();
            false
        }
        UiAction::OpenManagedCurrentRunTerminal {
            provider,
            native_session_id,
            opaque_ref,
        } => {
            let request = ManagedCurrentRunRequest {
                provider,
                native_session_id,
                opaque_ref,
            };
            let message = match handlers.open_managed_current_run(&request) {
                Ok(()) => "Opened Managed Current Run in Terminal.app".to_string(),
                Err(error) => format!("Open Terminal failed: {error}"),
            };
            ui.show_notice(message);
            request_rescan();
            false
        }
        UiAction::CopyManagedCurrentRunAttachCommand {
            provider,
            native_session_id,
            opaque_ref,
        } => {
            let request = ManagedCurrentRunRequest {
                provider,
                native_session_id,
                opaque_ref,
            };
            match handlers.managed_current_run_attach_command(&request) {
                Ok(command) => {
                    ui.copy_to_clipboard(clipboard, "Managed Current Run Attach Command", &command)
                }
                Err(error) => ui.show_notice(format!("Copy Attach Command failed: {error}")),
            }
            request_rescan();
            false
        }
        UiAction::SaveSessionMetadata {
            session,
            title,
            description,
        } => {
            let result = handlers
                .save_session_metadata(SessionMetadataUpdateRequest {
                    session,
                    title,
                    description,
                })
                .map(|_| ())
                .map_err(|error| error.to_string());
            ui.finish_session_metadata_save(result);
            request_rescan();
            false
        }
        UiAction::HideSessions { sessions } => {
            let result = handlers
                .hide_sessions(CurationRequest { sessions }, snapshot)
                .map(|outcome| outcome.changed)
                .map_err(|error| error.to_string());
            ui.finish_hide_sessions(result);
            request_rescan();
            false
        }
        UiAction::RestoreSessions { sessions } => {
            let result = handlers
                .restore_sessions(CurationRequest { sessions })
                .map(|outcome| outcome.changed)
                .map_err(|error| error.to_string());
            ui.finish_restore_sessions(result);
            request_rescan();
            false
        }
        UiAction::PermanentlyExcludeSessions { sessions } => {
            let result = handlers
                .permanently_exclude_sessions(CurationRequest { sessions })
                .map(|outcome| outcome.changed)
                .map_err(|error| error.to_string());
            ui.finish_permanent_exclusion(result);
            request_rescan();
            false
        }
        UiAction::Quit => true,
    }
}

fn format_managed_launch_diagnostics(
    diagnostics: &crate::application::managed_session::ManagedLaunchDiagnostics,
) -> String {
    let mut lines = vec![
        format!(
            "Managed Launch Registration: {}",
            diagnostics.registration_id
        ),
        "Status: Needs Review".to_string(),
        format!("Provider: {}", diagnostics.provider.label()),
        format!(
            "Working directory: {}",
            diagnostics.working_directory.display()
        ),
        format!(
            "Launched pane process: pid {}, started {}",
            diagnostics.launch_process.pid, diagnostics.launch_process.started_at_ms
        ),
        format!(
            "Evidence: private tmux pane {}; Binding Receipt not issued",
            diagnostics.channel_id
        ),
        format!(
            "Binding error: {}",
            diagnostics.binding_error.as_deref().unwrap_or("unknown")
        ),
    ];
    if let Some(error) = diagnostics.terminal_error.as_deref() {
        lines.push(format!("Terminal error: {error}"));
    }
    lines.join("\n")
}

fn handoff_status(prefix: &str, task: &OperationTask) -> String {
    format!("{prefix} {}: {}", short_id(&task.id), task.state.label())
}

fn short_id(value: &str) -> String {
    value
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}
