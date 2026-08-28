//! Single owner for TUI overlay input priority and drawing order.

use super::{
    hidden_tree, DetailView, DirectoryBrowser, GuideCategory, GuidePane, HandoffConfirmation,
    HideConfirmation, InventoryDeleteConfirmation, ManagedResumeConfirmation, NewSessionField,
    NewSessionForm, SessionEditor, SessionEditorField, TaskCancellationConfirmation,
    TaskDeletionConfirmation, TaskRetryConfirmation, Ui, UiAction, UNMEASURED_GUIDE_MAX_SCROLL,
};
use crate::application::managed_session::ManagedResumeRequest;
use crate::application::observation::read_model::SessionMetadataAction;
use crate::domain::Provider;
use crate::presentation::app::Snapshot;
use crossterm::event::{Event, KeyCode, KeyEvent};
use std::cell::Cell;

pub(super) enum NewOverlay {
    Form(NewSessionForm),
    Directory {
        form: NewSessionForm,
        browser: Box<DirectoryBrowser>,
    },
}

impl NewOverlay {
    pub(super) fn form(&self) -> &NewSessionForm {
        match self {
            Self::Form(form) | Self::Directory { form, .. } => form,
        }
    }
}

pub(super) struct GuideOverlay {
    pub(super) category: GuideCategory,
    pub(super) pane: GuidePane,
    pub(super) scroll: usize,
    pub(super) max_scroll: Cell<usize>,
    pub(super) underlay: Box<OverlayState>,
}

impl GuideOverlay {
    pub(super) fn new(category: GuideCategory, underlay: OverlayState) -> Self {
        Self {
            category,
            pane: GuidePane::Categories,
            scroll: 0,
            max_scroll: Cell::new(UNMEASURED_GUIDE_MAX_SCROLL),
            underlay: Box::new(underlay),
        }
    }

    pub(super) fn reset_scroll(&mut self) {
        self.scroll = 0;
        self.max_scroll.set(UNMEASURED_GUIDE_MAX_SCROLL);
    }
}

pub(super) struct HiddenInventoryOverlay {
    pub(super) state: hidden_tree::State,
    pub(super) underlay: Box<OverlayState>,
}

pub(super) struct SessionEditOverlay {
    pub(super) form: SessionEditor,
    pub(super) underlay: Box<OverlayState>,
}

#[derive(Default)]
pub(super) enum OverlayState {
    #[default]
    None,
    HiddenInventory(HiddenInventoryOverlay),
    Detail(DetailView),
    Guide(GuideOverlay),
    New(NewOverlay),
    SessionEdit(SessionEditOverlay),
    Confirmation {
        underlay: Box<OverlayState>,
        confirmation: ConfirmationState,
    },
}

pub(super) enum ConfirmationState {
    ManagedResume(ManagedResumeConfirmation),
    Handoff(HandoffConfirmation),
    TaskCancellation(TaskCancellationConfirmation),
    TaskDeletion(TaskDeletionConfirmation),
    TaskRetry(TaskRetryConfirmation),
    Hide(HideConfirmation),
    InventoryDelete(InventoryDeleteConfirmation),
}

impl OverlayState {
    pub(super) fn blocks_background_interaction(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub(super) fn detail(&self) -> Option<&DetailView> {
        match self {
            Self::Detail(detail) => Some(detail),
            Self::Guide(guide) => guide.underlay.detail(),
            Self::SessionEdit(editor) => editor.underlay.detail(),
            Self::Confirmation { underlay, .. } => underlay.detail(),
            _ => None,
        }
    }

    pub(super) fn detail_mut(&mut self) -> Option<&mut DetailView> {
        match self {
            Self::Detail(detail) => Some(detail),
            Self::Guide(guide) => guide.underlay.detail_mut(),
            Self::SessionEdit(editor) => editor.underlay.detail_mut(),
            Self::Confirmation { underlay, .. } => underlay.detail_mut(),
            _ => None,
        }
    }

    pub(super) fn hidden_inventory(&self) -> Option<&hidden_tree::State> {
        match self {
            Self::HiddenInventory(inventory) => Some(&inventory.state),
            Self::Confirmation { underlay, .. } => underlay.hidden_inventory(),
            _ => None,
        }
    }

    pub(super) fn hidden_inventory_mut(&mut self) -> Option<&mut hidden_tree::State> {
        match self {
            Self::HiddenInventory(inventory) => Some(&mut inventory.state),
            Self::Confirmation { underlay, .. } => underlay.hidden_inventory_mut(),
            _ => None,
        }
    }

    pub(super) fn guide(&self) -> Option<&GuideOverlay> {
        match self {
            Self::Guide(guide) => Some(guide),
            Self::Confirmation { underlay, .. } => underlay.guide(),
            _ => None,
        }
    }

    pub(super) fn guide_mut(&mut self) -> Option<&mut GuideOverlay> {
        match self {
            Self::Guide(guide) => Some(guide),
            Self::Confirmation { underlay, .. } => underlay.guide_mut(),
            _ => None,
        }
    }

    pub(super) fn new_session(&self) -> Option<&NewOverlay> {
        match self {
            Self::New(new) => Some(new),
            Self::Confirmation { underlay, .. } => underlay.new_session(),
            _ => None,
        }
    }

    pub(super) fn session_editor(&self) -> Option<&SessionEditor> {
        match self {
            Self::SessionEdit(editor) => Some(&editor.form),
            Self::Confirmation { underlay, .. } => underlay.session_editor(),
            _ => None,
        }
    }

    pub(super) fn session_editor_mut(&mut self) -> Option<&mut SessionEditor> {
        match self {
            Self::SessionEdit(editor) => Some(&mut editor.form),
            Self::Confirmation { underlay, .. } => underlay.session_editor_mut(),
            _ => None,
        }
    }

    pub(super) fn confirmation(&self) -> Option<&ConfirmationState> {
        match self {
            Self::Confirmation { confirmation, .. } => Some(confirmation),
            _ => None,
        }
    }

    pub(super) fn reconcile(&mut self, snapshot: &Snapshot) {
        *self = std::mem::take(self).into_reconciled(snapshot);
    }

    fn into_reconciled(self, snapshot: &Snapshot) -> Self {
        match self {
            Self::HiddenInventory(mut inventory) => {
                inventory.underlay = Box::new(inventory.underlay.into_reconciled(snapshot));
                inventory.state.reconcile(snapshot);
                if hidden_tree::sessions(snapshot).is_empty() {
                    *inventory.underlay
                } else {
                    Self::HiddenInventory(inventory)
                }
            }
            Self::Detail(detail) if detail_target_exists(&detail, snapshot) => Self::Detail(detail),
            Self::Detail(_) => Self::None,
            Self::Guide(mut guide) => {
                guide.underlay = Box::new(guide.underlay.into_reconciled(snapshot));
                Self::Guide(guide)
            }
            Self::SessionEdit(mut editor)
                if session_exists(
                    snapshot,
                    editor.form.session.provider,
                    &editor.form.session.native_id,
                ) =>
            {
                editor.underlay = Box::new(editor.underlay.into_reconciled(snapshot));
                Self::SessionEdit(editor)
            }
            Self::SessionEdit(editor) => editor.underlay.into_reconciled(snapshot),
            Self::Confirmation {
                underlay,
                confirmation,
            } => {
                let underlay = Box::new(underlay.into_reconciled(snapshot));
                if confirmation_target_exists(&confirmation, snapshot) {
                    Self::Confirmation {
                        underlay,
                        confirmation,
                    }
                } else {
                    *underlay
                }
            }
            other => other,
        }
    }
}

fn detail_target_exists(detail: &DetailView, snapshot: &Snapshot) -> bool {
    detail.task_id.as_ref().map_or_else(
        || session_exists(snapshot, detail.session.provider, &detail.session.native_id),
        |task_id| snapshot.tasks.iter().any(|task| &task.id == task_id),
    )
}

fn confirmation_target_exists(confirmation: &ConfirmationState, snapshot: &Snapshot) -> bool {
    match confirmation {
        ConfirmationState::ManagedResume(target) => {
            session_exists(snapshot, target.provider, &target.native_session_id)
        }
        ConfirmationState::Handoff(target) => {
            session_exists(snapshot, target.session.provider, &target.session.native_id)
        }
        ConfirmationState::TaskCancellation(target) => {
            snapshot.tasks.iter().any(|task| task.id == target.task_id)
        }
        ConfirmationState::TaskDeletion(target) => {
            snapshot.tasks.iter().any(|task| task.id == target.task_id)
        }
        ConfirmationState::TaskRetry(target) => snapshot
            .tasks
            .iter()
            .any(|task| task.id == target.predecessor_task_id),
        ConfirmationState::Hide(target) => match target {
            HideConfirmation::Session { session, .. } => {
                session_exists(snapshot, session.provider, &session.native_id)
            }
            HideConfirmation::Project { sessions, .. } => sessions
                .iter()
                .any(|session| session_exists(snapshot, session.provider, &session.native_id)),
        },
        ConfirmationState::InventoryDelete(target) => target.sessions.iter().any(|session| {
            snapshot
                .session_metadata
                .in_hidden_inventory(session.provider, &session.native_id)
                && session_exists(snapshot, session.provider, &session.native_id)
        }),
    }
}

fn session_exists(snapshot: &Snapshot, provider: Provider, native_id: &str) -> bool {
    snapshot
        .sessions
        .iter()
        .any(|session| session.provider == provider && session.id == native_id)
}

/// Convert one event for a modal owner. Non-modal overlays return `None` to their own router.
pub(super) fn handle_modal_key(
    ui: &mut Ui,
    key: KeyEvent,
    snapshot: &Snapshot,
) -> std::io::Result<Option<UiAction>> {
    let overlay = std::mem::take(&mut ui.overlay);
    let (next, action) = match overlay {
        OverlayState::New(NewOverlay::Form(form)) => handle_new_form(form, key, ui),
        OverlayState::New(NewOverlay::Directory { form, browser }) => {
            handle_directory(form, browser, key)
        }
        OverlayState::SessionEdit(editor) => handle_session_edit(editor, key),
        OverlayState::Confirmation {
            underlay,
            confirmation,
        } => match confirmation {
            ConfirmationState::ManagedResume(target) => match key.code {
                KeyCode::Esc => (*underlay, UiAction::None),
                KeyCode::Enter => (
                    *underlay,
                    UiAction::StartManagedResume {
                        request: ManagedResumeRequest {
                            provider: target.provider,
                            native_session_id: target.native_session_id,
                        },
                    },
                ),
                _ => (
                    OverlayState::Confirmation {
                        underlay,
                        confirmation: ConfirmationState::ManagedResume(target),
                    },
                    UiAction::None,
                ),
            },
            ConfirmationState::Handoff(mut target) => match key.code {
                KeyCode::Esc => (*underlay, UiAction::None),
                KeyCode::Left | KeyCode::Right => {
                    target.successor_provider = other_provider(target.successor_provider);
                    (
                        OverlayState::Confirmation {
                            underlay,
                            confirmation: ConfirmationState::Handoff(target),
                        },
                        UiAction::None,
                    )
                }
                KeyCode::Enter => (
                    *underlay,
                    UiAction::AuthorizeHandoff {
                        source_provider: target.session.provider,
                        source_session_id: target.session.native_id,
                        successor_provider: target.successor_provider,
                        end_current_run: target.end_current_run,
                    },
                ),
                _ => (
                    OverlayState::Confirmation {
                        underlay,
                        confirmation: ConfirmationState::Handoff(target),
                    },
                    UiAction::None,
                ),
            },
            ConfirmationState::TaskCancellation(target) => match key.code {
                KeyCode::Esc => (*underlay, UiAction::None),
                KeyCode::Enter => (
                    *underlay,
                    UiAction::CancelTask {
                        task_id: target.task_id,
                    },
                ),
                _ => (
                    OverlayState::Confirmation {
                        underlay,
                        confirmation: ConfirmationState::TaskCancellation(target),
                    },
                    UiAction::None,
                ),
            },
            ConfirmationState::TaskDeletion(target) => match key.code {
                KeyCode::Esc => (*underlay, UiAction::None),
                KeyCode::Enter => (
                    *underlay,
                    UiAction::DeleteTask {
                        task_id: target.task_id,
                    },
                ),
                _ => (
                    OverlayState::Confirmation {
                        underlay,
                        confirmation: ConfirmationState::TaskDeletion(target),
                    },
                    UiAction::None,
                ),
            },
            ConfirmationState::TaskRetry(mut target) => match key.code {
                KeyCode::Esc => (*underlay, UiAction::None),
                KeyCode::Left | KeyCode::Right => {
                    target.successor_provider = other_provider(target.successor_provider);
                    (
                        OverlayState::Confirmation {
                            underlay,
                            confirmation: ConfirmationState::TaskRetry(target),
                        },
                        UiAction::None,
                    )
                }
                KeyCode::Enter => (
                    *underlay,
                    UiAction::RetryTask {
                        predecessor_task_id: target.predecessor_task_id,
                        source_provider: target.session.provider,
                        source_session_id: target.session.native_id,
                        successor_provider: target.successor_provider,
                        end_current_run: target.end_current_run,
                    },
                ),
                _ => (
                    OverlayState::Confirmation {
                        underlay,
                        confirmation: ConfirmationState::TaskRetry(target),
                    },
                    UiAction::None,
                ),
            },
            ConfirmationState::Hide(target) => match key.code {
                KeyCode::Esc => (*underlay, UiAction::None),
                KeyCode::Enter => {
                    let requested = match &target {
                        HideConfirmation::Session { session, .. } => std::slice::from_ref(session),
                        HideConfirmation::Project { sessions, .. } => sessions.as_slice(),
                    };
                    let sessions = requested
                        .iter()
                        .filter(|identity| {
                            snapshot.sessions.iter().any(|session| {
                                session.provider == identity.provider
                                    && session.id == identity.native_id
                                    && snapshot
                                        .session_metadata
                                        .allows_for(session, SessionMetadataAction::Hide)
                            })
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    if sessions.is_empty() {
                        let became_open = requested.iter().any(|identity| {
                            snapshot.sessions.iter().any(|session| {
                                session.provider == identity.provider
                                    && session.id == identity.native_id
                                    && session.state.is_open()
                            })
                        });
                        ui.show_notice(if became_open {
                            "Open Session cannot be hidden"
                        } else {
                            "No Exited Sessions to hide"
                        });
                        (*underlay, UiAction::None)
                    } else {
                        (*underlay, UiAction::HideSessions { sessions })
                    }
                }
                _ => (
                    OverlayState::Confirmation {
                        underlay,
                        confirmation: ConfirmationState::Hide(target),
                    },
                    UiAction::None,
                ),
            },
            ConfirmationState::InventoryDelete(target) => match key.code {
                KeyCode::Esc => (*underlay, UiAction::None),
                KeyCode::Enter => (
                    *underlay,
                    UiAction::PermanentlyExcludeSessions {
                        sessions: target.sessions,
                    },
                ),
                _ => (
                    OverlayState::Confirmation {
                        underlay,
                        confirmation: ConfirmationState::InventoryDelete(target),
                    },
                    UiAction::None,
                ),
            },
        },
        other => {
            ui.overlay = other;
            return Ok(None);
        }
    };
    ui.overlay = next;
    Ok(Some(action))
}

fn handle_new_form(mut form: NewSessionForm, key: KeyEvent, ui: &Ui) -> (OverlayState, UiAction) {
    form.error = None;
    let activation = form.activation();
    let event = form.editor.handle_key(key, activation);
    match event {
        super::PropertyEditorEvent::Close => (OverlayState::None, UiAction::None),
        super::PropertyEditorEvent::Submit => match form.request() {
            Ok(request) => (OverlayState::None, UiAction::StartManagedNew { request }),
            Err(error) => {
                form.editor.focus(error.field.index());
                form.error = Some(error.message);
                (OverlayState::New(NewOverlay::Form(form)), UiAction::None)
            }
        },
        super::PropertyEditorEvent::TextCommitted { field, value } => {
            match NewSessionField::from_index(field) {
                NewSessionField::Title => form.title = value,
                NewSessionField::Description => form.description = value,
                _ => {}
            }
            (OverlayState::New(NewOverlay::Form(form)), UiAction::None)
        }
        super::PropertyEditorEvent::ChoiceConfirmed { field, choice } => {
            match NewSessionField::from_index(field) {
                NewSessionField::Provider => {
                    form.provider = Some(if choice == 0 {
                        Provider::CodexCli
                    } else {
                        Provider::ClaudeCode
                    });
                }
                NewSessionField::Location if choice < form.targets.len() => {
                    form.target_index = choice;
                }
                NewSessionField::Location => {
                    let start_directory = form
                        .targets
                        .get(form.target_index)
                        .map(|target| target.working_directory.clone())
                        .or_else(|| std::env::current_dir().ok());
                    match start_directory {
                        Some(start_directory) => {
                            match DirectoryBrowser::new(start_directory, ui.theme.palette()) {
                                Ok(browser) => {
                                    return (
                                        OverlayState::New(NewOverlay::Directory {
                                            form,
                                            browser: Box::new(browser),
                                        }),
                                        UiAction::None,
                                    );
                                }
                                Err(error) => {
                                    form.error = Some(format!("Directory browser failed: {error}"));
                                }
                            }
                        }
                        None => {
                            form.error = Some("Directory browser has no start directory".into());
                        }
                    }
                }
                _ => {}
            }
            (OverlayState::New(NewOverlay::Form(form)), UiAction::None)
        }
        _ => (OverlayState::New(NewOverlay::Form(form)), UiAction::None),
    }
}

fn handle_directory(
    mut form: NewSessionForm,
    mut browser: Box<DirectoryBrowser>,
    key: KeyEvent,
) -> (OverlayState, UiAction) {
    if key.code == KeyCode::Esc {
        return (OverlayState::New(NewOverlay::Form(form)), UiAction::None);
    }
    if key.code == KeyCode::Char('y') {
        let directory = browser.explorer.cwd().clone();
        return (
            OverlayState::New(NewOverlay::Form(form)),
            UiAction::ResolveManagedNewDirectory { directory },
        );
    }
    if let Err(error) = browser.explorer.handle(&Event::Key(key)) {
        form.error = Some(format!("Directory browser failed: {error}"));
        return (OverlayState::New(NewOverlay::Form(form)), UiAction::None);
    }
    (
        OverlayState::New(NewOverlay::Directory { form, browser }),
        UiAction::None,
    )
}

fn handle_session_edit(mut editor: SessionEditOverlay, key: KeyEvent) -> (OverlayState, UiAction) {
    let form = &mut editor.form;
    form.error = None;
    let activation = form.activation();
    let event = form.editor.handle_key(key, activation);
    match event {
        super::PropertyEditorEvent::Close => (*editor.underlay, UiAction::None),
        super::PropertyEditorEvent::Submit => {
            let action = UiAction::SaveSessionMetadata {
                session: form.session.clone(),
                title: form.title.clone(),
                description: form.description.clone(),
            };
            (OverlayState::SessionEdit(editor), action)
        }
        super::PropertyEditorEvent::TextCommitted { field, value } => {
            match SessionEditorField::from_index(field) {
                SessionEditorField::Title => form.title = value,
                SessionEditorField::Description => form.description = value,
            }
            (OverlayState::SessionEdit(editor), UiAction::None)
        }
        _ => (OverlayState::SessionEdit(editor), UiAction::None),
    }
}

fn other_provider(provider: Provider) -> Provider {
    match provider {
        Provider::ClaudeCode => Provider::CodexCli,
        Provider::CodexCli => Provider::ClaudeCode,
    }
}
