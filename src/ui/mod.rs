//! TUI rendering and interaction state (Ratatui). Cozy, dense, three-page layout.
//!
//! Rendering reads a [`Snapshot`] plus interactive [`Ui`] state. Provider field names
//! stay behind adapters; Profile-backed preferences are the only UI-owned persistence.

mod theme;

use crate::app::{fmt_context, fmt_cost, fmt_datetime, fmt_relative, fmt_tokens, Snapshot, Tab};
use crate::attention::{
    AttentionCondition, AttentionEngine, AttentionRank, AttentionSeverity, AttentionSnapshot,
};
use crate::clipboard::Clipboard;
use crate::config::{CRIT_PCT, WARN_PCT};
use crate::domain::{
    ModelUsage, Project, ProjectIdentity, Provider, Session, SessionState, Usage, Worktree,
};
use crate::session_labels::SessionLabels;
use crate::signals::{ContextAlertLevel, RecentSignal, RecentSignalKind};
use crate::tasks::OperationTask;
use chrono::Local;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarState, Table, TableState, Wrap,
};
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::Cell as StateCell;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use theme::{Theme, ThemeName};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MIN_TERMINAL_WIDTH: u16 = 80;
const MIN_TERMINAL_HEIGHT: u16 = 24;
const PROVIDER_RECENT_SESSION_LIMIT: usize = 5;
const UNMEASURED_GUIDE_MAX_SCROLL: usize = usize::MAX;

/// Shared style for the currently-selected item.
fn selected_style(theme: Theme) -> Style {
    Style::default()
        .fg(theme.selected_foreground)
        .bg(theme.selected_background)
        .add_modifier(Modifier::BOLD)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionIdentity {
    provider: Provider,
    native_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HandoffConfirmation {
    session: SessionIdentity,
    source_state: SessionState,
    successor_provider: Provider,
    end_current_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskCancellationConfirmation {
    task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskDeletionConfirmation {
    task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskRetryConfirmation {
    predecessor_task_id: String,
    session: SessionIdentity,
    source_state: SessionState,
    successor_provider: Provider,
    end_current_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailView {
    session: SessionIdentity,
    task_id: Option<String>,
    scroll: usize,
    origin_signal: Option<RecentSignal>,
    copy_menu_selected: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TitleEditor {
    session: SessionIdentity,
    input: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyField {
    SourceLog,
    Worktree,
    Cwd,
    SessionId,
}

impl CopyField {
    const ALL: [Self; 4] = [Self::SourceLog, Self::Worktree, Self::Cwd, Self::SessionId];

    fn label(self) -> &'static str {
        match self {
            Self::SourceLog => "Source log",
            Self::Worktree => "Worktree",
            Self::Cwd => "cwd",
            Self::SessionId => "Session ID",
        }
    }

    fn value(self, session: &Session) -> Option<Cow<'_, str>> {
        match self {
            Self::SourceLog => (!session.source.path.as_os_str().is_empty())
                .then(|| session.source.path.to_string_lossy()),
            Self::Worktree => session
                .worktree
                .as_ref()
                .map(|worktree| worktree.root.to_string_lossy()),
            Self::Cwd => session
                .cwd
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(Cow::Borrowed),
            Self::SessionId => {
                (!session.id.is_empty()).then_some(Cow::Borrowed(session.id.as_str()))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllRegion {
    Now,
    RecentSignals,
    Tasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuideCategory {
    Overview,
    Navigation,
    Sessions,
    Signals,
    Theme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuidePane {
    Categories,
    Content,
}

impl GuideCategory {
    const ALL: [Self; 5] = [
        Self::Overview,
        Self::Navigation,
        Self::Sessions,
        Self::Signals,
        Self::Theme,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Navigation => "Navigation",
            Self::Sessions => "Sessions",
            Self::Signals => "Signals",
            Self::Theme => "Theme",
        }
    }

    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|category| *category == self)
            .unwrap_or(0);
        Self::ALL[(index + 1).min(Self::ALL.len() - 1)]
    }

    fn previous(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|category| *category == self)
            .unwrap_or(0);
        Self::ALL[index.saturating_sub(1)]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderProjectKey {
    provider: Provider,
    project: Option<ProjectIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ProviderDisclosureKey {
    MoreRecent(ProviderProjectKey),
    Older(ProviderProjectKey),
    UnavailableCwd(Provider),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderDisclosureKind {
    MoreRecent,
    Older,
    UnavailableCwd,
}

impl ProviderDisclosureKind {
    fn label(self) -> &'static str {
        match self {
            Self::MoreRecent => "MORE SESSIONS",
            Self::Older => "OLDER SESSIONS",
            Self::UnavailableCwd => "MISSING CWD",
        }
    }
}

#[derive(Clone)]
enum ProviderTarget<'a> {
    Session(&'a Session),
    Disclosure(ProviderDisclosureKey),
}

enum ProviderPageRow<'a> {
    Project(Option<&'a Project>),
    Session(&'a Session),
    Disclosure {
        key: ProviderDisclosureKey,
        kind: ProviderDisclosureKind,
        count: usize,
        expanded: bool,
    },
}

impl<'a> ProviderPageRow<'a> {
    fn into_target(self) -> Option<ProviderTarget<'a>> {
        match self {
            ProviderPageRow::Project(_) => None,
            ProviderPageRow::Session(session) => Some(ProviderTarget::Session(session)),
            ProviderPageRow::Disclosure { key, .. } => Some(ProviderTarget::Disclosure(key)),
        }
    }
}

/// Interactive UI state (owned by the event loop).
pub struct Ui {
    pub tab: Tab,
    pub selected: usize,
    all_region: AllRegion,
    signal_selected: usize,
    task_selected: usize,
    detail: Option<DetailView>,
    expanded_provider_disclosures: HashSet<ProviderDisclosureKey>,
    guide_open: bool,
    guide_category: GuideCategory,
    guide_pane: GuidePane,
    guide_scroll: usize,
    // Wrapping depends on the rendered viewport, so draw records the latest usable limit.
    guide_max_scroll: StateCell<usize>,
    edit_title: Option<TitleEditor>,
    handoff_confirmation: Option<HandoffConfirmation>,
    task_cancellation_confirmation: Option<TaskCancellationConfirmation>,
    task_deletion_confirmation: Option<TaskDeletionConfirmation>,
    task_retry_confirmation: Option<TaskRetryConfirmation>,
    labels: SessionLabels,
    profile_path: Option<PathBuf>,
    profile: ProfilePreferences,
    theme: ThemeName,
    status_notice: Option<(String, Instant)>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
struct ProfilePreferences {
    #[serde(default)]
    help_seen: bool,
    #[serde(default)]
    theme: ThemeName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    None,
    Rescan,
    Copy {
        field: &'static str,
        value: String,
    },
    AuthorizeHandoff {
        source_provider: Provider,
        source_session_id: String,
        successor_provider: Provider,
        end_current_run: bool,
    },
    AcknowledgeTask {
        task_id: String,
    },
    CancelTask {
        task_id: String,
    },
    DeleteTask {
        task_id: String,
    },
    RetryTask {
        predecessor_task_id: String,
        source_provider: Provider,
        source_session_id: String,
        successor_provider: Provider,
        end_current_run: bool,
    },
    CopySuccessorCommand {
        task_id: String,
    },
    OpenSuccessorTerminal {
        task_id: String,
    },
    Quit,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            tab: Tab::All,
            selected: 0,
            all_region: AllRegion::Now,
            signal_selected: 0,
            task_selected: 0,
            detail: None,
            expanded_provider_disclosures: HashSet::new(),
            guide_open: false,
            guide_category: GuideCategory::Overview,
            guide_pane: GuidePane::Categories,
            guide_scroll: 0,
            guide_max_scroll: StateCell::new(UNMEASURED_GUIDE_MAX_SCROLL),
            edit_title: None,
            handoff_confirmation: None,
            task_cancellation_confirmation: None,
            task_deletion_confirmation: None,
            task_retry_confirmation: None,
            labels: SessionLabels::default(),
            profile_path: None,
            profile: ProfilePreferences::default(),
            theme: ThemeName::default(),
            status_notice: None,
        }
    }
}

impl Ui {
    /// Load UI state for a local Profile.
    pub fn load(
        profile_path: impl AsRef<Path>,
        theme_override: Option<&str>,
    ) -> std::io::Result<Self> {
        let profile_path = profile_path.as_ref().to_path_buf();
        let profile = match fs::read(&profile_path) {
            Ok(contents) => serde_json::from_slice(&contents).map_err(std::io::Error::other)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ProfilePreferences::default()
            }
            Err(error) => return Err(error),
        };
        let theme = theme_override
            .and_then(ThemeName::from_name)
            .unwrap_or_else(|| {
                if theme_override.is_some() {
                    ThemeName::default()
                } else {
                    profile.theme
                }
            });
        let label_path = profile_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("session-labels.json");
        Ok(Self {
            guide_open: !profile.help_seen,
            labels: SessionLabels::load(label_path),
            profile_path: Some(profile_path),
            profile,
            theme,
            ..Self::default()
        })
    }

    pub fn active_tab(&self) -> Tab {
        self.tab
    }

    pub fn active_theme_name(&self) -> &'static str {
        self.theme.as_str()
    }

    /// Perform one explicit clipboard request and report its result briefly.
    pub fn copy_to_clipboard(&mut self, clipboard: &mut dyn Clipboard, field: &str, value: &str) {
        let message = match clipboard.write_text(value) {
            Ok(()) => format!("Copied {field}"),
            Err(error) => format!("Copy failed: {error}"),
        };
        self.status_notice = Some((message, Instant::now() + Duration::from_secs(2)));
    }

    pub fn show_notice(&mut self, message: impl Into<String>) {
        self.status_notice = Some((message.into(), Instant::now() + Duration::from_secs(4)));
    }

    /// Apply one user key through the UI's public interaction interface.
    pub fn handle_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        snap: &Snapshot,
    ) -> std::io::Result<UiAction> {
        self.clamp_selections(snap);
        if self.edit_title.is_some() {
            self.handle_title_editor_key(key);
            return Ok(UiAction::None);
        }
        if let Some(target) = self.handoff_confirmation.clone() {
            return match key.code {
                crossterm::event::KeyCode::Esc => {
                    self.handoff_confirmation = None;
                    Ok(UiAction::None)
                }
                crossterm::event::KeyCode::Left | crossterm::event::KeyCode::Right => {
                    if let Some(confirmation) = self.handoff_confirmation.as_mut() {
                        confirmation.successor_provider = match confirmation.successor_provider {
                            Provider::ClaudeCode => Provider::CodexCli,
                            Provider::CodexCli => Provider::ClaudeCode,
                        };
                    }
                    Ok(UiAction::None)
                }
                crossterm::event::KeyCode::Enter => {
                    self.handoff_confirmation = None;
                    Ok(UiAction::AuthorizeHandoff {
                        source_provider: target.session.provider,
                        source_session_id: target.session.native_id,
                        successor_provider: target.successor_provider,
                        end_current_run: target.end_current_run,
                    })
                }
                _ => Ok(UiAction::None),
            };
        }
        if let Some(target) = self.task_cancellation_confirmation.clone() {
            return match key.code {
                crossterm::event::KeyCode::Esc => {
                    self.task_cancellation_confirmation = None;
                    Ok(UiAction::None)
                }
                crossterm::event::KeyCode::Enter => {
                    self.task_cancellation_confirmation = None;
                    Ok(UiAction::CancelTask {
                        task_id: target.task_id,
                    })
                }
                _ => Ok(UiAction::None),
            };
        }
        if let Some(target) = self.task_deletion_confirmation.clone() {
            return match key.code {
                crossterm::event::KeyCode::Esc => {
                    self.task_deletion_confirmation = None;
                    Ok(UiAction::None)
                }
                crossterm::event::KeyCode::Enter => {
                    self.task_deletion_confirmation = None;
                    Ok(UiAction::DeleteTask {
                        task_id: target.task_id,
                    })
                }
                _ => Ok(UiAction::None),
            };
        }
        if let Some(target) = self.task_retry_confirmation.clone() {
            return match key.code {
                crossterm::event::KeyCode::Esc => {
                    self.task_retry_confirmation = None;
                    Ok(UiAction::None)
                }
                crossterm::event::KeyCode::Left | crossterm::event::KeyCode::Right => {
                    if let Some(confirmation) = self.task_retry_confirmation.as_mut() {
                        confirmation.successor_provider = match confirmation.successor_provider {
                            Provider::ClaudeCode => Provider::CodexCli,
                            Provider::CodexCli => Provider::ClaudeCode,
                        };
                    }
                    Ok(UiAction::None)
                }
                crossterm::event::KeyCode::Enter => {
                    self.task_retry_confirmation = None;
                    Ok(UiAction::RetryTask {
                        predecessor_task_id: target.predecessor_task_id,
                        source_provider: target.session.provider,
                        source_session_id: target.session.native_id,
                        successor_provider: target.successor_provider,
                        end_current_run: target.end_current_run,
                    })
                }
                _ => Ok(UiAction::None),
            };
        }
        if key.code == crossterm::event::KeyCode::Char('r')
            && self
                .detail_task(snap)
                .is_some_and(OperationTask::offers_retry)
        {
            let task = self.detail_task(snap).expect("checked above");
            let source_state = snap
                .sessions
                .iter()
                .find(|session| {
                    session.provider == task.source_provider && session.id == task.source_session_id
                })
                .map_or(SessionState::Exited, |session| session.state);
            self.task_retry_confirmation = Some(TaskRetryConfirmation {
                predecessor_task_id: task.id.clone(),
                session: SessionIdentity {
                    provider: task.source_provider,
                    native_id: task.source_session_id.clone(),
                },
                source_state,
                successor_provider: task.source_provider,
                end_current_run: source_state.is_open(),
            });
            return Ok(UiAction::None);
        }
        if key.code == crossterm::event::KeyCode::Char('q')
            || (key.code == crossterm::event::KeyCode::Char('c')
                && key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL))
        {
            return Ok(UiAction::Quit);
        }
        if self.guide_open {
            self.handle_guide_key(key)?;
            return Ok(UiAction::None);
        }
        match key.code {
            crossterm::event::KeyCode::Char('r') => {
                self.labels.reload();
                return Ok(UiAction::Rescan);
            }
            crossterm::event::KeyCode::Char('1') => self.set_tab(Tab::All),
            crossterm::event::KeyCode::Char('2') => self.set_tab(Tab::Claude),
            crossterm::event::KeyCode::Char('3') => self.set_tab(Tab::Codex),
            crossterm::event::KeyCode::Char('?') => {
                self.guide_category = self.guide_category_for_context();
                self.guide_pane = GuidePane::Categories;
                self.reset_guide_scroll();
                self.guide_open = true;
            }
            crossterm::event::KeyCode::Char('e') if self.labels.is_editable() => {
                self.open_title_editor(snap);
            }
            crossterm::event::KeyCode::Char('h') if self.detail_session(snap).is_some() => {
                let session = self.detail_session(snap).expect("checked above");
                self.handoff_confirmation = Some(HandoffConfirmation {
                    session: SessionIdentity {
                        provider: session.provider,
                        native_id: session.id.clone(),
                    },
                    source_state: session.state,
                    successor_provider: session.provider,
                    end_current_run: session.state.is_open(),
                });
            }
            crossterm::event::KeyCode::Char('x')
                if self
                    .detail_task(snap)
                    .is_some_and(|task| !task.state.is_terminal()) =>
            {
                let task_id = self.detail_task(snap).expect("checked above").id.clone();
                self.task_cancellation_confirmation =
                    Some(TaskCancellationConfirmation { task_id });
            }
            crossterm::event::KeyCode::Char('d')
                if self
                    .detail_task(snap)
                    .is_some_and(OperationTask::offers_deletion) =>
            {
                let task_id = self.detail_task(snap).expect("checked above").id.clone();
                self.task_deletion_confirmation = Some(TaskDeletionConfirmation { task_id });
            }
            crossterm::event::KeyCode::Char('c')
                if self.detail_task(snap).is_some_and(|task| {
                    task.state == crate::tasks::OperationTaskState::Ready
                        && !task.is_being_deleted()
                        && task.successor_launch_plan.is_some()
                }) =>
            {
                return Ok(UiAction::CopySuccessorCommand {
                    task_id: self.detail_task(snap).expect("checked above").id.clone(),
                });
            }
            crossterm::event::KeyCode::Char('o')
                if self.detail_task(snap).is_some_and(|task| {
                    task.state == crate::tasks::OperationTaskState::Ready
                        && !task.is_being_deleted()
                        && task.successor_launch_plan.is_some()
                }) =>
            {
                return Ok(UiAction::OpenSuccessorTerminal {
                    task_id: self.detail_task(snap).expect("checked above").id.clone(),
                });
            }
            crossterm::event::KeyCode::Tab if self.detail.is_none() && self.tab == Tab::All => {
                self.all_region = match self.all_region {
                    AllRegion::Now => AllRegion::RecentSignals,
                    AllRegion::RecentSignals => AllRegion::Tasks,
                    AllRegion::Tasks => AllRegion::Now,
                };
            }
            crossterm::event::KeyCode::BackTab if self.detail.is_none() && self.tab == Tab::All => {
                self.all_region = match self.all_region {
                    AllRegion::Now => AllRegion::Tasks,
                    AllRegion::RecentSignals => AllRegion::Now,
                    AllRegion::Tasks => AllRegion::RecentSignals,
                };
            }
            crossterm::event::KeyCode::Char('c') if self.detail.is_some() => {
                let selected = self.detail_session(snap).and_then(|session| {
                    CopyField::ALL
                        .iter()
                        .position(|field| field.value(session).is_some())
                });
                if let Some(detail) = self.detail.as_mut() {
                    detail.copy_menu_selected = selected;
                }
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.copy_menu_is_open() =>
            {
                self.move_copy_menu_selection(snap, true);
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.copy_menu_is_open() =>
            {
                self.move_copy_menu_selection(snap, false);
            }
            crossterm::event::KeyCode::Esc if self.copy_menu_is_open() => {
                if let Some(detail) = self.detail.as_mut() {
                    detail.copy_menu_selected = None;
                }
            }
            crossterm::event::KeyCode::Enter if self.copy_menu_is_open() => {
                let request = self
                    .detail
                    .as_ref()
                    .and_then(|detail| detail.copy_menu_selected)
                    .and_then(|selected| CopyField::ALL.get(selected).copied())
                    .and_then(|field| {
                        self.detail_session(snap).and_then(|session| {
                            field
                                .value(session)
                                .map(|value| (field.label(), value.into_owned()))
                        })
                    });
                if let Some((field, value)) = request {
                    if let Some(detail) = self.detail.as_mut() {
                        detail.copy_menu_selected = None;
                    }
                    return Ok(UiAction::Copy { field, value });
                }
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_none()
                    && self.tab == Tab::All
                    && self.all_region == AllRegion::RecentSignals =>
            {
                if self.signal_selected + 1 < snap.recent_signals.len() {
                    self.signal_selected += 1;
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_none()
                    && self.tab == Tab::All
                    && self.all_region == AllRegion::RecentSignals =>
            {
                self.signal_selected = self.signal_selected.saturating_sub(1);
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_none()
                    && self.tab == Tab::All
                    && self.all_region == AllRegion::Tasks =>
            {
                if self.task_selected + 1 < snap.tasks.len() {
                    self.task_selected += 1;
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_none()
                    && self.tab == Tab::All
                    && self.all_region == AllRegion::Tasks =>
            {
                self.task_selected = self.task_selected.saturating_sub(1);
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_none() =>
            {
                let visible_len = self.selectable_len(snap);
                if self.selected + 1 < visible_len {
                    self.selected += 1;
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_none() =>
            {
                self.selected = self.selected.saturating_sub(1);
            }
            crossterm::event::KeyCode::Enter
                if self.detail.is_none()
                    && self.tab == Tab::All
                    && self.all_region == AllRegion::Tasks =>
            {
                if let Some(action) = self.open_selected_task(snap) {
                    return Ok(action);
                }
            }
            crossterm::event::KeyCode::Enter if self.detail.is_none() => {
                self.activate_selection(snap);
            }
            crossterm::event::KeyCode::Enter if self.detail_task(snap).is_some() => {
                let successor = self.detail_task(snap).and_then(|task| {
                    let session_id = task.successor_session_id.as_ref()?;
                    snap.sessions.iter().find(|session| {
                        session.provider == task.successor_provider && &session.id == session_id
                    })
                });
                if let Some(successor) = successor {
                    self.detail = Some(DetailView {
                        session: SessionIdentity {
                            provider: successor.provider,
                            native_id: successor.id.clone(),
                        },
                        task_id: None,
                        scroll: 0,
                        origin_signal: None,
                        copy_menu_selected: None,
                    });
                }
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_some() =>
            {
                if let Some(detail) = self.detail.as_mut() {
                    detail.scroll = detail.scroll.saturating_add(1);
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_some() =>
            {
                if let Some(detail) = self.detail.as_mut() {
                    detail.scroll = detail.scroll.saturating_sub(1);
                }
            }
            crossterm::event::KeyCode::Esc => {
                self.close_detail(snap);
            }
            _ => {}
        }
        Ok(UiAction::None)
    }

    fn handle_guide_key(&mut self, key: crossterm::event::KeyEvent) -> std::io::Result<()> {
        match key.code {
            crossterm::event::KeyCode::Esc => {
                self.profile.help_seen = true;
                self.save_profile()?;
                self.guide_open = false;
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.guide_pane == GuidePane::Categories =>
            {
                self.guide_category = self.guide_category.next();
                self.reset_guide_scroll();
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.guide_pane == GuidePane::Categories =>
            {
                self.guide_category = self.guide_category.previous();
                self.reset_guide_scroll();
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.guide_pane == GuidePane::Content =>
            {
                let max_scroll = self.guide_max_scroll.get();
                self.guide_scroll = self
                    .guide_scroll
                    .min(max_scroll)
                    .saturating_add(1)
                    .min(max_scroll);
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.guide_pane == GuidePane::Content =>
            {
                self.guide_scroll = self
                    .guide_scroll
                    .min(self.guide_max_scroll.get())
                    .saturating_sub(1);
            }
            crossterm::event::KeyCode::Tab | crossterm::event::KeyCode::Right
                if self.guide_pane == GuidePane::Categories =>
            {
                self.guide_pane = GuidePane::Content;
            }
            crossterm::event::KeyCode::BackTab | crossterm::event::KeyCode::Left
                if self.guide_pane == GuidePane::Content =>
            {
                self.guide_pane = GuidePane::Categories;
            }
            _ => {}
        }
        Ok(())
    }

    fn reset_guide_scroll(&mut self) {
        self.guide_scroll = 0;
        self.guide_max_scroll.set(UNMEASURED_GUIDE_MAX_SCROLL);
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.selected = 0;
        self.all_region = AllRegion::Now;
        self.signal_selected = 0;
        self.task_selected = 0;
        self.detail = None;
        self.handoff_confirmation = None;
        self.task_cancellation_confirmation = None;
        self.task_deletion_confirmation = None;
        self.task_retry_confirmation = None;
    }

    fn guide_category_for_context(&self) -> GuideCategory {
        if self.detail.is_some() || self.tab != Tab::All {
            GuideCategory::Sessions
        } else if self.all_region == AllRegion::RecentSignals {
            GuideCategory::Signals
        } else {
            GuideCategory::Overview
        }
    }

    fn copy_menu_is_open(&self) -> bool {
        self.detail
            .as_ref()
            .is_some_and(|detail| detail.copy_menu_selected.is_some())
    }

    fn move_copy_menu_selection(&mut self, snap: &Snapshot, forward: bool) {
        let Some(current) = self
            .detail
            .as_ref()
            .and_then(|detail| detail.copy_menu_selected)
        else {
            return;
        };
        let Some(session) = self.detail_session(snap) else {
            return;
        };
        let next = if forward {
            CopyField::ALL
                .iter()
                .enumerate()
                .skip(current + 1)
                .find(|(_, field)| field.value(session).is_some())
                .map(|(index, _)| index)
        } else {
            CopyField::ALL
                .iter()
                .enumerate()
                .take(current)
                .rev()
                .find(|(_, field)| field.value(session).is_some())
                .map(|(index, _)| index)
        };
        if let (Some(next), Some(detail)) = (next, self.detail.as_mut()) {
            detail.copy_menu_selected = Some(next);
        }
    }

    fn close_detail(&mut self, snap: &Snapshot) {
        let Some(detail) = self.detail.as_ref().cloned() else {
            return;
        };
        if let Some(origin_signal) = detail.origin_signal {
            self.all_region = AllRegion::RecentSignals;
            if let Some(index) = snap
                .recent_signals
                .iter()
                .position(|signal| signal == &origin_signal)
            {
                self.signal_selected = index;
            }
            self.clamp_selections(snap);
            self.detail = None;
            return;
        }
        if let Some(task_id) = detail.task_id {
            self.all_region = AllRegion::Tasks;
            if let Some(index) = snap.tasks.iter().position(|task| task.id == task_id) {
                self.task_selected = index;
            }
            self.detail = None;
            return;
        }
        let identity = detail.session;
        let index = if self.tab.provider().is_some() {
            self.provider_targets(snap).iter().position(|target| {
                matches!(target, ProviderTarget::Session(session)
                    if session.provider == identity.provider && session.id == identity.native_id)
            })
        } else {
            self.visible(snap).iter().position(|session| {
                session.provider == identity.provider && session.id == identity.native_id
            })
        };
        if let Some(index) = index {
            self.selected = index;
        }
        self.detail = None;
    }

    fn save_profile(&self) -> std::io::Result<()> {
        let Some(path) = &self.profile_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_vec_pretty(&self.profile).map_err(std::io::Error::other)?;
        fs::write(path, contents)
    }

    /// Selectable Sessions visible on the current page, in rendered row order.
    pub fn visible(&self, snap: &Snapshot) -> Vec<Session> {
        match self.tab.provider() {
            Some(_) => self
                .provider_targets(snap)
                .into_iter()
                .filter_map(|target| match target {
                    ProviderTarget::Session(session) => Some(session.clone()),
                    ProviderTarget::Disclosure(_) => None,
                })
                .collect(),
            None => now_sessions(snap).into_iter().cloned().collect(),
        }
    }

    pub fn selectable_len(&self, snap: &Snapshot) -> usize {
        if self.tab.provider().is_some() {
            self.provider_targets(snap).len()
        } else {
            self.visible(snap).len()
        }
    }

    pub fn clamp_selection(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    pub fn clamp_selections(&mut self, snap: &Snapshot) {
        self.clamp_selection(self.selectable_len(snap));
        if snap.recent_signals.is_empty() {
            self.signal_selected = 0;
        } else if self.signal_selected >= snap.recent_signals.len() {
            self.signal_selected = snap.recent_signals.len() - 1;
        }
        if snap.tasks.is_empty() {
            self.task_selected = 0;
        } else if self.task_selected >= snap.tasks.len() {
            self.task_selected = snap.tasks.len() - 1;
        }
    }

    fn detail_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        let detail = self.detail.as_ref()?;
        if detail.task_id.is_some() {
            return None;
        }
        let identity = &detail.session;
        snap.sessions.iter().find(|session| {
            session.provider == identity.provider && session.id == identity.native_id
        })
    }

    fn detail_task<'a>(&self, snap: &'a Snapshot) -> Option<&'a OperationTask> {
        let task_id = self.detail.as_ref()?.task_id.as_deref()?;
        snap.tasks.iter().find(|task| task.id == task_id)
    }

    fn selected_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        if self.detail.is_some() {
            return self.detail_session(snap);
        }
        if self.tab == Tab::All {
            return (self.all_region == AllRegion::Now)
                .then(|| now_sessions(snap).get(self.selected).copied())
                .flatten();
        }
        match self.provider_targets(snap).into_iter().nth(self.selected) {
            Some(ProviderTarget::Session(session)) => Some(session),
            _ => None,
        }
    }

    fn open_title_editor(&mut self, snap: &Snapshot) {
        let Some(session) = self.selected_session(snap) else {
            return;
        };
        let identity = SessionIdentity {
            provider: session.provider,
            native_id: session.id.clone(),
        };
        let input = self
            .labels
            .alias(session.provider, &session.id)
            .unwrap_or("")
            .to_string();
        self.edit_title = Some(TitleEditor {
            session: identity,
            input,
            error: None,
        });
    }

    fn handle_title_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        match key.code {
            crossterm::event::KeyCode::Esc => self.edit_title = None,
            crossterm::event::KeyCode::Backspace => {
                if let Some(editor) = self.edit_title.as_mut() {
                    editor.input.pop();
                    editor.error = None;
                }
            }
            crossterm::event::KeyCode::Char(character)
                if !character.is_control()
                    && !key.modifiers.intersects(
                        crossterm::event::KeyModifiers::CONTROL
                            | crossterm::event::KeyModifiers::ALT,
                    ) =>
            {
                if let Some(editor) = self.edit_title.as_mut() {
                    editor.input.push(character);
                    editor.error = None;
                }
            }
            crossterm::event::KeyCode::Enter => {
                let Some(editor) = self.edit_title.as_ref() else {
                    return;
                };
                let provider = editor.session.provider;
                let native_id = editor.session.native_id.clone();
                let alias = editor.input.trim().to_string();
                let clearing =
                    alias.is_empty() && self.labels.alias(provider, &native_id).is_some();
                let result = if clearing {
                    self.labels.clear_title(provider, &native_id)
                } else if alias.is_empty() {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Title cannot be empty",
                    ))
                } else {
                    self.labels.set_alias(provider, &native_id, &alias)
                };
                match result {
                    Ok(()) => {
                        self.edit_title = None;
                        self.status_notice = Some((
                            if clearing {
                                "Title cleared".into()
                            } else {
                                "Title saved".into()
                            },
                            Instant::now() + Duration::from_secs(2),
                        ));
                    }
                    Err(error) => {
                        if let Some(editor) = self.edit_title.as_mut() {
                            editor.error = Some(error.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn provider_targets<'a>(&self, snap: &'a Snapshot) -> Vec<ProviderTarget<'a>> {
        self.provider_page_rows(snap)
            .into_iter()
            .filter_map(ProviderPageRow::into_target)
            .collect()
    }

    fn provider_page_rows<'a>(&self, snap: &'a Snapshot) -> Vec<ProviderPageRow<'a>> {
        let Some(provider) = self.tab.provider() else {
            return Vec::new();
        };
        provider_page_rows(
            &snap.sessions,
            provider,
            snap.generated_ms,
            &self.expanded_provider_disclosures,
        )
    }

    fn open_selected_task(&mut self, snap: &Snapshot) -> Option<UiAction> {
        let task = snap.tasks.get(self.task_selected)?;
        self.detail = Some(DetailView {
            session: SessionIdentity {
                provider: task.source_provider,
                native_id: task.source_session_id.clone(),
            },
            task_id: Some(task.id.clone()),
            scroll: 0,
            origin_signal: None,
            copy_menu_selected: None,
        });
        (task.state.is_terminal() && task.acknowledged_at_ms.is_none() && !task.is_being_deleted())
            .then(|| UiAction::AcknowledgeTask {
                task_id: task.id.clone(),
            })
    }

    fn activate_selection(&mut self, snap: &Snapshot) {
        if self.tab.provider().is_some() {
            match self.provider_targets(snap).into_iter().nth(self.selected) {
                Some(ProviderTarget::Session(session)) => {
                    self.detail = Some(DetailView {
                        session: SessionIdentity {
                            provider: session.provider,
                            native_id: session.id.clone(),
                        },
                        task_id: None,
                        scroll: 0,
                        origin_signal: None,
                        copy_menu_selected: None,
                    });
                }
                Some(ProviderTarget::Disclosure(disclosure)) => {
                    self.toggle_provider_disclosure(disclosure);
                }
                None => {}
            }
            return;
        }

        if self.all_region == AllRegion::RecentSignals {
            let Some(signal) = snap.recent_signals.get(self.signal_selected) else {
                return;
            };
            match signal {
                RecentSignal::Session {
                    provider,
                    session_id,
                    ..
                } => {
                    if snap
                        .sessions
                        .iter()
                        .any(|session| session.provider == *provider && session.id == *session_id)
                    {
                        self.detail = Some(DetailView {
                            session: SessionIdentity {
                                provider: *provider,
                                native_id: session_id.clone(),
                            },
                            task_id: None,
                            scroll: 0,
                            origin_signal: Some(signal.clone()),
                            copy_menu_selected: None,
                        });
                    }
                }
                RecentSignal::Worktree { worktree_root, .. } => {
                    if let Some(index) = now_sessions(snap).iter().position(|session| {
                        session
                            .worktree
                            .as_ref()
                            .is_some_and(|worktree| worktree.root == *worktree_root)
                    }) {
                        self.tab = Tab::All;
                        self.all_region = AllRegion::Now;
                        self.selected = index;
                        self.detail = None;
                        self.status_notice = None;
                    } else {
                        self.status_notice = Some((
                            "Worktree is no longer open".into(),
                            Instant::now() + Duration::from_secs(2),
                        ));
                    }
                }
            }
            return;
        }

        if let Some(session) = self.visible(snap).get(self.selected) {
            self.detail = Some(DetailView {
                session: SessionIdentity {
                    provider: session.provider,
                    native_id: session.id.clone(),
                },
                task_id: None,
                scroll: 0,
                origin_signal: None,
                copy_menu_selected: None,
            });
        }
    }

    fn toggle_provider_disclosure(&mut self, disclosure: ProviderDisclosureKey) {
        if !self.expanded_provider_disclosures.remove(&disclosure) {
            self.expanded_provider_disclosures.insert(disclosure);
        }
    }
}

/// Draw the whole screen.
pub fn draw(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    f.render_widget(
        Block::default().style(Style::default().fg(theme.foreground).bg(theme.background)),
        f.area(),
    );
    if f.area().width < MIN_TERMINAL_WIDTH || f.area().height < MIN_TERMINAL_HEIGHT {
        draw_terminal_too_small(f, theme);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // global header
            Constraint::Min(3),    // content
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_header(f, chunks[0], snap, ui);

    if ui.detail.is_some() {
        draw_detail(f, chunks[1], snap, ui);
    } else {
        match ui.tab {
            Tab::All => draw_all(f, chunks[1], snap, ui),
            _ => draw_provider(f, chunks[1], snap, ui),
        }
    }

    draw_footer(f, chunks[2], snap, ui);

    if ui
        .detail
        .as_ref()
        .is_some_and(|detail| detail.copy_menu_selected.is_some())
    {
        draw_copy_menu(f, snap, ui);
    }

    if ui.guide_open {
        draw_guide(f, ui);
    }

    if ui.edit_title.is_some() {
        draw_title_editor(f, ui);
    }

    if ui.handoff_confirmation.is_some() {
        draw_handoff_confirmation(f, snap, ui);
    }
    if ui.task_cancellation_confirmation.is_some() {
        draw_task_cancellation_confirmation(f, ui);
    }
    if ui.task_deletion_confirmation.is_some() {
        draw_task_deletion_confirmation(f, ui);
    }
    if ui.task_retry_confirmation.is_some() {
        draw_task_retry_confirmation(f, ui);
    }
}

fn draw_task_retry_confirmation(f: &mut Frame, ui: &Ui) {
    let Some(target) = ui.task_retry_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = centered_rect(68, 52, f.area());
    f.render_widget(Clear, area);
    let mut lines = vec![
        Line::from(Span::styled(
            "Authorize Retry?",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Previous Task  {}", target.predecessor_task_id)),
        Line::from(format!(
            "Provider       {}",
            target.session.provider.label()
        )),
        Line::from(format!("Session ID     {}", target.session.native_id)),
        Line::from(match target.successor_provider {
            Provider::ClaudeCode => "Successor  [Claude Code]  Codex CLI",
            Provider::CodexCli => "Successor  Claude Code  [Codex CLI]",
        }),
        Line::from(""),
        Line::from("A new Task, deadlines, Artifact, and authorization will be used."),
    ];
    if target.end_current_run {
        lines.push(Line::from(format!(
            "The {} Current Run may be ended after revalidation.",
            match target.source_state {
                SessionState::Busy => "Busy",
                SessionState::Idle => "Idle",
                SessionState::Exited => "Exited",
            }
        )));
    }
    lines.extend([
        Line::from("The previous Task remains unchanged."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter Authorize   Esc Cancel",
            Style::default().fg(theme.accent),
        )),
    ]);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(" Retry Handoff "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

fn draw_task_cancellation_confirmation(f: &mut Frame, ui: &Ui) {
    let Some(target) = ui.task_cancellation_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = centered_rect(64, 38, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from(Span::styled(
            "Cancel Task?",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Task ID  {}", target.task_id)),
        Line::from(""),
        Line::from("A valid Handoff Result can still become Ready."),
        Line::from("No Prompt will be resent and no Retry will start."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter Confirm   Esc Keep Task",
            Style::default().fg(theme.accent),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(" Cancel Task "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

fn draw_task_deletion_confirmation(f: &mut Frame, ui: &Ui) {
    let Some(target) = ui.task_deletion_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let area = centered_rect(70, 46, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from(Span::styled(
            "Delete Task and Artifact?",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Task ID  {}", target.task_id)),
        Line::from(""),
        Line::from("This irreversible action removes the Operation Task,"),
        Line::from("its Handoff Artifact, Successor Launch Plan, and interaction metadata."),
        Line::from("Source and Successor Sessions remain unchanged."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter Delete   Esc Keep Task",
            Style::default().fg(theme.accent),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(" Delete Task and Artifact "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

fn draw_handoff_confirmation(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let Some(target) = ui.handoff_confirmation.as_ref() else {
        return;
    };
    let theme = ui.theme.palette();
    let session = snap.sessions.iter().find(|session| {
        session.provider == target.session.provider && session.id == target.session.native_id
    });
    let cwd = session
        .and_then(|session| session.cwd.as_deref())
        .unwrap_or("N/A");
    let area = centered_rect(72, 64, f.area());
    f.render_widget(Clear, area);
    let mut lines = vec![
        Line::from(Span::styled(
            "Authorize one Handoff Operation?",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Provider    {}", target.session.provider.label())),
        Line::from(format!("Session ID  {}", target.session.native_id)),
        Line::from(format!("cwd         {cwd}")),
        Line::from(match target.successor_provider {
            Provider::ClaudeCode => "Successor  [Claude Code]  Codex CLI",
            Provider::CodexCli => "Successor  Claude Code  [Codex CLI]",
        }),
    ];
    if target.end_current_run {
        let current_run_state = match target.source_state {
            SessionState::Busy => "Busy",
            SessionState::Idle => "Idle",
            SessionState::Exited => "Exited",
        };
        lines.extend([
            Line::from(format!("Current Run {current_run_state}")),
            Line::from(""),
            Line::from("This authorization may end this one Current Run."),
            Line::from("A Busy Current Run waits until Idle and is never interrupted."),
            Line::from("SessionMesh revalidates it before requesting graceful exit."),
            Line::from("The users Terminal and shell remain open."),
        ]);
    }
    lines.extend([
        Line::from(""),
        Line::from("Generation keeps the source Project and Worktree read-only."),
        Line::from("The detached Task continues after SessionMesh closes."),
        Line::from(""),
        Line::from(Span::styled(
            "Enter Authorize   Esc Cancel",
            Style::default().fg(theme.accent),
        )),
    ]);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(" Authorize Handoff "),
            )
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        area,
    );
}

fn draw_terminal_too_small(f: &mut Frame, theme: Theme) {
    let current = f.area();
    let area = centered_rect(100, 40, current);
    let lines = vec![
        Line::from(Span::styled(
            "Terminal too small",
            Style::default()
                .fg(theme.critical)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "Required: {MIN_TERMINAL_WIDTH}x{MIN_TERMINAL_HEIGHT}"
        )),
        Line::from(format!("Current: {}x{}", current.width, current.height)),
        Line::from(""),
        Line::from("q Quit"),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.critical)),
            ),
        area,
    );
}

fn draw_copy_menu(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let Some(session) = ui.detail_session(snap) else {
        return;
    };
    let Some(selected) = ui
        .detail
        .as_ref()
        .and_then(|detail| detail.copy_menu_selected)
    else {
        return;
    };
    let theme = ui.theme.palette();
    let area = centered_rect(84, 38, f.area());
    let mut lines = vec![Line::from(Span::styled(
        "Choose one complete metadata value",
        Style::default().fg(theme.muted),
    ))];
    for (index, field) in CopyField::ALL.iter().copied().enumerate() {
        let value = field.value(session);
        let text = format!(
            "{} {:<12} {}",
            if index == selected { ">" } else { " " },
            field.label(),
            value.as_deref().unwrap_or("Unavailable")
        );
        let style = if value.is_none() {
            Style::default().fg(theme.muted)
        } else if index == selected {
            selected_style(theme)
        } else {
            Style::default().fg(theme.foreground)
        };
        lines.push(Line::from(Span::styled(text, style)));
    }

    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(Span::styled(
                        " Copy source metadata ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(
                        Line::from(Span::styled(
                            " Enter Copy | Esc Cancel ",
                            Style::default().fg(theme.muted),
                        ))
                        .right_aligned(),
                    ),
            ),
        area,
    );
}

fn draw_title_editor(f: &mut Frame, ui: &Ui) {
    const TITLE_LABEL: &str = "Title    ";

    let Some(editor) = &ui.edit_title else {
        return;
    };
    let theme = ui.theme.palette();
    let area = centered_rect(76, 32, f.area());
    let label_width = UnicodeWidthStr::width(TITLE_LABEL);
    let input_width = area.width.saturating_sub(3 + label_width as u16) as usize;
    let visible_input = visible_input_tail(&editor.input, input_width);
    let visible_input_width = UnicodeWidthStr::width(visible_input.as_str()) as u16;
    let short_id = SessionLabels::short_id(&editor.session.native_id);
    let clears_existing_alias = editor.input.trim().is_empty()
        && ui
            .labels
            .alias(editor.session.provider, &editor.session.native_id)
            .is_some();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Session  ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{} · {short_id}", editor.session.provider.label()),
                Style::default().fg(theme.foreground),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(TITLE_LABEL, Style::default().fg(theme.muted)),
            Span::styled(visible_input, Style::default().fg(theme.foreground)),
        ]),
    ];
    if let Some(error) = &editor.error {
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(theme.critical),
        )));
    }
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(Span::styled(
                        " Edit title ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(
                        Line::from(Span::styled(
                            if clears_existing_alias {
                                " Enter Clear title | Esc Cancel "
                            } else {
                                " Enter Save | Esc Cancel "
                            },
                            Style::default().fg(theme.muted),
                        ))
                        .right_aligned(),
                    ),
            ),
        area,
    );
    f.set_cursor_position(Position::new(
        area.x + 1 + label_width as u16 + visible_input_width,
        area.y + 3,
    ));
}

fn draw_guide(f: &mut Frame, ui: &Ui) {
    let theme = ui.theme.palette();
    let area = centered_rect(80, 76, f.area());
    let focus_hint = match ui.guide_pane {
        GuidePane::Categories => " j/k Categories | Tab/Right Content | Esc Close ",
        GuidePane::Content => " j/k Scroll | Shift+Tab/Left Categories | Esc Close ",
    };
    let base_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            " Guide ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = base_block.inner(area);
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(24), Constraint::Percentage(76)])
        .split(inner);
    let content = guide_content(ui.guide_category, theme, ui.active_theme_name());
    let wrap = Wrap { trim: false };
    let initial_total_lines = Paragraph::new(content.clone())
        .wrap(wrap)
        .line_count(panes[1].width);
    let overflow = initial_total_lines > panes[1].height as usize;
    let content_area = Rect {
        width: panes[1].width.saturating_sub(u16::from(overflow)),
        ..panes[1]
    };
    let total_lines = Paragraph::new(content.clone())
        .wrap(wrap)
        .line_count(content_area.width);
    let max_scroll = total_lines.saturating_sub(content_area.height as usize);
    ui.guide_max_scroll.set(max_scroll);
    let scroll = ui.guide_scroll.min(max_scroll);
    let mut block = base_block.title_bottom(
        Line::from(Span::styled(focus_hint, Style::default().fg(theme.muted))).left_aligned(),
    );
    if overflow {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {}/{} ", scroll + 1, total_lines),
                Style::default().fg(theme.muted),
            ))
            .right_aligned(),
        );
    }
    let categories = GuideCategory::ALL
        .into_iter()
        .map(|category| {
            let selected = category == ui.guide_category;
            Line::from(Span::styled(
                format!("{} {}", if selected { '>' } else { ' ' }, category.label()),
                if selected {
                    selected_style(theme)
                } else {
                    Style::default().fg(theme.foreground)
                },
            ))
        })
        .collect::<Vec<_>>();

    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(categories)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(
                        Style::default().fg(if ui.guide_pane == GuidePane::Categories {
                            theme.accent
                        } else {
                            theme.border
                        }),
                    ),
            )
            .wrap(Wrap { trim: false }),
        panes[0],
    );
    f.render_widget(
        Paragraph::new(content)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .scroll((scroll as u16, 0))
            .wrap(wrap),
        content_area,
    );
    if overflow {
        let scrollbar = Scrollbar::default()
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(theme.accent))
            .track_style(Style::default().fg(theme.border));
        let mut scrollbar_state = ScrollbarState::new(total_lines)
            .position(scroll)
            .viewport_content_length(content_area.height as usize);
        f.render_stateful_widget(scrollbar, panes[1], &mut scrollbar_state);
    }
}

fn guide_content(
    category: GuideCategory,
    theme: Theme,
    active_theme_name: &str,
) -> Vec<Line<'static>> {
    let heading = |text: &'static str| {
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
    };
    match category {
        GuideCategory::Overview => vec![
            heading(category.label()),
            Line::from("SessionMesh is local to this Mac."),
            Line::from("Passive observation is the default."),
            Line::from("Control Operations require explicit authorization."),
            heading("Pages"),
            Line::from("1  All"),
            Line::from("2  Claude Code"),
            Line::from("3  Codex CLI"),
            heading("Current and recent"),
            Line::from("NOW shows current conditions."),
            Line::from("Recent Signals show transitions."),
        ],
        GuideCategory::Navigation => vec![
            heading(category.label()),
            Line::from("1 / 2 / 3  Open a page"),
            Line::from("Tab / Shift+Tab  Change region or Guide pane"),
            Line::from("Up/Down or j/k  Move or scroll"),
            Line::from("Enter  Inspect or expand"),
            Line::from("Enter  View an indexed linked Successor from Task Detail"),
            Line::from("Esc  Back or close"),
            Line::from("c  Copy source metadata in Session Detail"),
            Line::from("h  Authorize a Handoff from Session Detail"),
            Line::from("Left/Right  Choose a Successor Provider"),
            Line::from("o  Open a Ready Successor in Terminal.app"),
            Line::from("c  Copy a Ready Successor command"),
            Line::from("Exact launched-process evidence confirms Successor linkage."),
            Line::from("x  Cancel a non-terminal Task after confirmation"),
            Line::from("r  Refresh, or Retry an eligible Task"),
            Line::from("d  Delete a terminal Task and owned Artifact after confirmation"),
            Line::from("Tasks and Artifacts never expire automatically."),
            Line::from("?  Guide"),
            Line::from("q  Quit"),
        ],
        GuideCategory::Sessions => vec![
            heading(category.label()),
            heading("Session Titles"),
            Line::from("Title  The name SessionMesh displays for a Session."),
            Line::from("Alias  A local user-authored Title."),
            Line::from("It changes only SessionMesh; Provider data is unchanged."),
            Line::from("Short Session ID  The last eight characters used when no Alias exists."),
            Line::from("Duplicate Titles include their Short Session ID."),
            Line::from("e Edit title  Set or clear the selected Session's Alias."),
            heading("Providers"),
            Line::from("CL  Claude Code"),
            Line::from("CX  Codex CLI"),
            heading("Open Session"),
            Line::from("Busy  A request is running."),
            Line::from("Idle  The process is waiting for input."),
            Line::from("Exited  No live Provider process is attached."),
            heading("Current Context"),
            Line::from("Current Context is the latest known content occupying the model window."),
            Line::from("It is not cumulative Processed Usage."),
            Line::from(format!("Warning  {WARN_PCT:.0}%")),
            Line::from(format!("Critical {CRIT_PCT:.0}%")),
        ],
        GuideCategory::Theme => vec![
            heading(category.label()),
            Line::from(format!("Active Theme  {active_theme_name}")),
            heading("Built-in Themes"),
            Line::from("sessionmesh  catppuccin  dracula"),
            Line::from("gruvbox  light  high-contrast"),
            heading("Command-line override"),
            Line::from("--theme <name>"),
            Line::from("Applies to one process and does not change the saved Theme."),
            heading("Semantics"),
            Line::from("Current Context uses Normal, Warning, and Critical colors."),
            Line::from("Idle and Busy use separate lifecycle colors."),
            Line::from("Theme changes presentation, never Session state or thresholds."),
        ],
        GuideCategory::Signals => vec![
            heading(category.label()),
            Line::from("Recent Signals answer: what just changed? They are not current state."),
            heading("Session lifecycle"),
            Line::from(
                "CX session1 Busy -> Idle  Request ended; the Open Session is waiting for input.",
            ),
            Line::from("CX session1 Busy -> Exited  A running Provider process exited."),
            Line::from("CX session1 Idle -> Exited  A waiting Provider process exited."),
            Line::from("CX session1 Exited -> Busy  A Session resumed and is running."),
            Line::from("CX session1 Exited -> Idle  A Session reopened and is waiting for input."),
            heading("Current Context"),
            Line::from("CL session1 Ctx +Warning 120.0k>150.0k  Current Context entered Warning."),
            Line::from(
                "CL session1 Ctx +Critical 160.0k>180.0k  Current Context entered Critical.",
            ),
            Line::from("CL session1 Ctx -Warning 150.0k>120.0k  The Warning condition cleared."),
            Line::from(
                "CL session1 Ctx -Critical 180.0k>160.0k  Critical cleared to Warning or normal.",
            ),
            Line::from("Signals show before -> after Current Context values."),
            Line::from(
                "Only Open Sessions with a known Current Context percentage cross thresholds.",
            ),
            Line::from(format!(
                "Warning starts at {WARN_PCT:.0}%; Critical starts at {CRIT_PCT:.0}%."
            )),
            heading("Compaction"),
            Line::from("CL session1 Cmp auto 188.0k>100.0k  Compaction observed."),
            Line::from("CL session1 Cmp auto 188.0k>100.0k -Critical  Also cleared Critical."),
            heading("Worktree"),
            Line::from(
                "WT /project  Risk detected  Two or more Open Sessions share a writable Worktree.",
            ),
            Line::from("WT /project  Risk cleared  Fewer than two Open Sessions remain there."),
            Line::from("Before 1: CL:session1"),
            Line::from("After 2: CL:session1, CX:session2"),
            Line::from("Two Open Sessions trigger Shared Worktree Risk."),
            Line::from("Evidence shows Open Session membership before -> after."),
            Line::from("This is not proof of a file-level or Git conflict."),
            heading("Prefixes"),
            Line::from("CL  Claude Code"),
            Line::from("CX  Codex CLI"),
            Line::from("WT  Worktree"),
            Line::from("Ctx Current Context"),
            Line::from("Cmp Compaction"),
            heading("Not recorded"),
            Line::from("Idle -> Busy is not recorded."),
            Line::from("Busy heartbeats and ordinary Current Context percentage changes."),
            Line::from("Usage, Cost, source timestamps, or duplicate polls."),
            Line::from("Possible Stall is not recorded."),
            heading("Retention and privacy"),
            Line::from("The first scan is a silent baseline and emits no Signals."),
            Line::from("Recent Signals are metadata-only."),
            Line::from(
                "Only the five newest Signals are retained, newest first, for this process.",
            ),
            Line::from("Current state belongs in NOW."),
            Line::from("Recent Signals never include:"),
            Line::from("prompt, response, tool, or source-code content"),
        ],
    }
}

fn draw_header(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let wide = area.width >= 100;
    let compact_projects = area.width < 88;
    let mut left = vec![
        Span::styled(
            " SessionMesh ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("| ", Style::default().fg(theme.border)),
        Span::styled(
            format!("v{} ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme.muted),
        ),
        Span::styled("| ", Style::default().fg(theme.border)),
    ];
    for (index, tab) in Tab::ALL.iter().copied().enumerate() {
        if index > 0 {
            left.push(Span::raw(" "));
        }
        left.push(Span::styled(
            (tab.index() + 1).to_string(),
            if tab == ui.tab {
                selected_style(theme)
            } else {
                Style::default().fg(theme.foreground)
            },
        ));
    }
    left.push(Span::raw(" "));

    let (input, output) = snap
        .sessions
        .iter()
        .fold((0_u64, 0_u64), |totals, session| {
            (
                totals.0.saturating_add(session.usage.input),
                totals.1.saturating_add(session.usage.output),
            )
        });
    let project_count = snap
        .sessions
        .iter()
        .filter_map(|session| session.project.as_ref())
        .map(|project| &project.identity)
        .collect::<HashSet<_>>()
        .len();
    let mut right = vec![
        Span::styled(" In ", Style::default().fg(theme.muted)),
        Span::styled(
            fmt_tokens(input),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if wide { "  Out " } else { " Out " },
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            fmt_tokens(output),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
    ];
    if compact_projects {
        right.push(Span::styled("P ", Style::default().fg(theme.muted)));
        right.push(Span::styled(
            project_count.to_string(),
            Style::default().fg(theme.foreground),
        ));
    } else {
        right.push(Span::styled(
            format!(
                "{project_count} Project{}",
                if project_count == 1 { "" } else { "s" }
            ),
            Style::default().fg(theme.foreground),
        ));
    }
    right.push(Span::raw(" "));

    f.render_widget(
        Block::default()
            .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
            .border_style(Style::default().fg(theme.border))
            .title(Line::from(left))
            .title(Line::from(right).right_aligned()),
        area,
    );
}

// ---- All page ----

fn draw_all(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let now_ms = snapshot_now_ms(snap);
    let (now_area, tasks_area, signals_area) = if area.width >= 120 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        let upper = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(67), Constraint::Percentage(33)])
            .split(rows[0]);
        let lower = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(67), Constraint::Percentage(33)])
            .split(rows[1]);
        (lower[0], Some(upper[1]), Some(lower[1]))
    } else if !snap.tasks.is_empty() && area.height >= 18 {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),
                Constraint::Length(10),
                Constraint::Min(3),
            ])
            .split(area);
        (areas[1], Some(areas[0]), Some(areas[2]))
    } else if area.height >= 14 {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(14), Constraint::Min(0)])
            .split(area);
        (areas[0], None, (areas[1].height >= 3).then_some(areas[1]))
    } else {
        (area, None, None)
    };
    if let Some(tasks_area) = tasks_area {
        draw_minimal_tasks(f, tasks_area, &snap.tasks, now_ms, ui);
    }
    let attention = current_attention(snap);
    let now = group_open_sessions(&snap.sessions, &attention);
    let visible_sessions = now
        .iter()
        .flat_map(|project| {
            project
                .worktrees
                .iter()
                .flat_map(|worktree| worktree.sessions.iter().copied())
                .chain(project.direct_sessions.iter().copied())
        })
        .collect::<Vec<_>>();
    let visible_identities = visible_sessions
        .iter()
        .map(|session| (session.provider, session.id.as_str()))
        .collect::<Vec<_>>();
    let mut titles = ui.labels.titles_for(&visible_identities).into_iter();
    let open_project_count = now
        .iter()
        .filter(|project| project.project.is_some())
        .count();
    let open_session_count = now
        .iter()
        .map(|project| {
            project.direct_sessions.len()
                + project
                    .worktrees
                    .iter()
                    .map(|worktree| worktree.sessions.len())
                    .sum::<usize>()
        })
        .sum::<usize>();
    let content_width = now_area.width.saturating_sub(2) as usize;
    let column_widths = now_column_widths(area.width, content_width);
    let mut lines = Vec::new();
    let mut selectable = 0usize;

    if now.is_empty() {
        lines.push(Line::from(Span::styled(
            if snap.ready {
                "No Open Sessions."
            } else {
                "Indexing... Open Sessions will appear shortly."
            },
            Style::default().fg(theme.muted),
        )));
    }

    for project in now {
        lines.push(now_project_line(&project, &attention, content_width, theme));
        let indent_worktrees = project.worktrees.len() > 1;
        for worktree in project.worktrees {
            if indent_worktrees {
                lines.push(Line::from(Span::styled(
                    now_worktree_line(&worktree, &attention, content_width, true),
                    Style::default().fg(theme.muted),
                )));
            }
            for session in worktree.sessions {
                let title = titles.next().expect("every NOW Session has a Title");
                lines.push(now_session_line(
                    session,
                    &title,
                    &attention,
                    now_ms,
                    column_widths,
                    ui.all_region == AllRegion::Now && selectable == ui.selected,
                    theme,
                ));
                selectable += 1;
            }
        }
        for session in project.direct_sessions {
            let title = titles.next().expect("every NOW Session has a Title");
            lines.push(now_session_line(
                session,
                &title,
                &attention,
                now_ms,
                column_widths,
                ui.all_region == AllRegion::Now && selectable == ui.selected,
                theme,
            ));
            selectable += 1;
        }
    }

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if ui.all_region == AllRegion::Now {
                    theme.accent
                } else {
                    theme.border
                }))
                .title(Line::from(vec![
                    Span::styled(
                        " PROJECTS ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{open_project_count} "),
                        Style::default()
                            .fg(theme.foreground)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]))
                .title(
                    Line::from(vec![
                        Span::raw(" "),
                        Span::styled(
                            open_session_count.to_string(),
                            Style::default()
                                .fg(theme.foreground)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            if open_session_count == 1 {
                                " OPEN SESSION "
                            } else {
                                " OPEN SESSIONS "
                            },
                            Style::default().fg(theme.muted),
                        ),
                    ])
                    .right_aligned(),
                ),
        ),
        now_area,
    );

    if let Some(signals_area) = signals_area {
        draw_recent_signals(f, signals_area, &snap.recent_signals, now_ms, ui);
    }
}

fn draw_minimal_tasks(f: &mut Frame, area: Rect, tasks: &[OperationTask], now_ms: i64, ui: &Ui) {
    let theme = ui.theme.palette();
    let visible_rows = area.height.saturating_sub(2) as usize;
    let content_width = area.width.saturating_sub(2) as usize;
    let offset = ui
        .task_selected
        .saturating_sub(visible_rows.saturating_sub(1));
    let task_identities = tasks
        .iter()
        .map(|task| (task.source_provider, task.source_session_id.as_str()))
        .collect::<Vec<_>>();
    let task_titles = ui.labels.titles_for(&task_identities);
    let lines = tasks
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible_rows)
        .map(|(index, task)| {
            let provider = match task.source_provider {
                Provider::ClaudeCode => "CL",
                Provider::CodexCli => "CX",
            };
            let unread = task.state.is_terminal() && task.acknowledged_at_ms.is_none();
            let unread_label = if unread { "NEW " } else { "    " };
            let state = task.state.label();
            let transition_age = fmt_relative(task.state_changed_at_ms, now_ms);
            let fixed_width = UnicodeWidthStr::width(unread_label)
                + UnicodeWidthStr::width("Handoff ")
                + UnicodeWidthStr::width(provider)
                + UnicodeWidthStr::width(":")
                + UnicodeWidthStr::width(" ")
                + UnicodeWidthStr::width(state)
                + UnicodeWidthStr::width(" ")
                + UnicodeWidthStr::width(transition_age.as_str());
            let source_label_width = content_width.saturating_sub(fixed_width).max(1);
            let resolved_label = &task_titles[index];
            let source_label = fit_title(resolved_label, source_label_width, true);
            let base_style = if unread {
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            let style = if ui.all_region == AllRegion::Tasks && index == ui.task_selected {
                selected_style(theme)
            } else {
                base_style
            };
            Line::from(Span::styled(
                format!("{unread_label}Handoff {provider}:{source_label} {state} {transition_age}"),
                style,
            ))
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if ui.all_region == AllRegion::Tasks {
                    theme.accent
                } else {
                    theme.border
                }))
                .title(Line::from(vec![
                    Span::styled(
                        " TASKS ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{} ", tasks.len()),
                        Style::default().fg(theme.foreground),
                    ),
                ])),
        ),
        area,
    );
}

fn draw_recent_signals(f: &mut Frame, area: Rect, signals: &[RecentSignal], now_ms: i64, ui: &Ui) {
    let theme = ui.theme.palette();
    let width = area.width.saturating_sub(2) as usize;
    let height = area.height.saturating_sub(2) as usize;
    let titles = recent_signal_titles(signals, &ui.labels);
    let lines = if signals.is_empty() {
        vec![Line::from(Span::styled(
            "No recent Signals.",
            Style::default().fg(theme.muted),
        ))]
    } else {
        let mut lines = Vec::new();
        let compact = signals
            .iter()
            .map(|signal| recent_signal_lines(signal, width, &titles).len())
            .sum::<usize>()
            > height;
        for (index, signal) in signals.iter().enumerate() {
            let style = if ui.all_region == AllRegion::RecentSignals && index == ui.signal_selected
            {
                selected_style(theme)
            } else {
                Style::default().fg(theme.foreground)
            };
            let max_signal_lines = if compact { 1 } else { usize::MAX };
            for text in recent_signal_lines(signal, width, &titles)
                .into_iter()
                .take(max_signal_lines)
            {
                if lines.len() == height {
                    break;
                }
                lines.push(Line::from(Span::styled(text, style)));
            }
            if lines.len() == height {
                break;
            }
        }
        lines
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(
            Style::default().fg(if ui.all_region == AllRegion::RecentSignals {
                theme.accent
            } else {
                theme.border
            }),
        )
        .title(Line::from(vec![
            Span::styled(
                " RECENT SIGNALS ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} ", signals.len()),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    if let Some(latest_at_ms) = signals.iter().map(RecentSignal::observed_at_ms).max() {
        block = block.title(
            Line::from(vec![
                Span::styled(" latest ", Style::default().fg(theme.muted)),
                Span::styled(
                    format!("{} ", fmt_relative(latest_at_ms, now_ms)),
                    Style::default().fg(theme.foreground),
                ),
            ])
            .right_aligned(),
        );
    }
    f.render_widget(Paragraph::new(lines).block(block), area);
}

type VisibleTitles = HashMap<Provider, HashMap<String, String>>;

fn recent_signal_titles(signals: &[RecentSignal], labels: &SessionLabels) -> VisibleTitles {
    let mut identities = Vec::<(Provider, String)>::new();
    let mut seen = HashSet::new();
    let mut add = |provider: Provider, native_id: &str| {
        let identity = (provider, native_id.to_string());
        if seen.insert(identity.clone()) {
            identities.push(identity);
        }
    };
    for signal in signals {
        match signal {
            RecentSignal::Session {
                provider,
                session_id,
                ..
            } => add(*provider, session_id),
            RecentSignal::Worktree { before, after, .. } => {
                for identity in before.iter().chain(after) {
                    add(identity.provider, &identity.native_id);
                }
            }
        }
    }
    let identity_refs = identities
        .iter()
        .map(|(provider, native_id)| (*provider, native_id.as_str()))
        .collect::<Vec<_>>();
    let resolved_titles = labels.titles_for(&identity_refs);
    let mut visible = VisibleTitles::new();
    for ((provider, native_id), title) in identities.into_iter().zip(resolved_titles) {
        visible
            .entry(provider)
            .or_default()
            .insert(native_id, title);
    }
    visible
}

fn visible_title<'a>(titles: &'a VisibleTitles, provider: Provider, native_id: &str) -> &'a str {
    titles
        .get(&provider)
        .and_then(|provider_titles| provider_titles.get(native_id))
        .map(String::as_str)
        .expect("every visible Signal Session has a Title")
}

fn recent_signal_lines(signal: &RecentSignal, width: usize, titles: &VisibleTitles) -> Vec<String> {
    let RecentSignal::Session {
        provider,
        session_id,
        kind,
        ..
    } = signal
    else {
        let RecentSignal::Worktree {
            worktree_root,
            change,
            before,
            after,
            ..
        } = signal
        else {
            unreachable!();
        };
        let change = match change {
            crate::signals::SharedWorktreeRiskChange::Detected => "detected",
            crate::signals::SharedWorktreeRiskChange::Cleared => "cleared",
        };
        let suffix = format!("  Risk {change}");
        let root_width = width.saturating_sub(3 + suffix.chars().count());
        return vec![
            format!(
                "WT {}{suffix}",
                shorten_start(&worktree_root.display().to_string(), root_width)
            ),
            shorten_end(
                &format!(
                    "Before {}: {}",
                    before.len(),
                    signal_members(before, titles)
                ),
                width,
            ),
            shorten_end(
                &format!("After {}: {}", after.len(), signal_members(after, titles)),
                width,
            ),
        ];
    };
    let evidence = match kind {
        RecentSignalKind::SessionStateChanged { from, to } => format!(
            "{} -> {}",
            signal_state_label(*from),
            signal_state_label(*to)
        ),
        RecentSignalKind::ContextAlertChanged {
            from,
            to,
            before,
            after,
        } => {
            let (change, level) = match (from, to) {
                (None, Some(to)) => ("entered", *to),
                (Some(ContextAlertLevel::Warning), Some(ContextAlertLevel::Critical)) => {
                    ("entered", ContextAlertLevel::Critical)
                }
                (Some(from), _) => ("cleared", *from),
                (None, None) => unreachable!("unchanged Context bands are not Signals"),
            };
            format!(
                "Ctx {}{} {}>{}",
                if change == "entered" { "+" } else { "-" },
                context_alert_label(level),
                fmt_tokens(before.tokens),
                fmt_tokens(after.tokens)
            )
        }
        RecentSignalKind::CompactionObserved {
            compaction,
            context_before,
            context_after,
            cleared_alert,
        } => {
            let trigger = shorten_start(compaction.trigger.as_deref().unwrap_or("unknown"), 6);
            let cleared = cleared_alert.map_or_else(String::new, |level| {
                format!(" -{}", context_alert_label(level))
            });
            format!(
                "Cmp {trigger} {}>{}{cleared}",
                fmt_tokens(context_before.tokens),
                fmt_tokens(context_after.tokens)
            )
        }
    };
    let title = visible_title(titles, *provider, session_id);
    let provider = match provider {
        Provider::ClaudeCode => "CL",
        Provider::CodexCli => "CX",
    };
    let fixed_width = provider.chars().count() + evidence.chars().count() + 2;
    let session_width = width.saturating_sub(fixed_width);
    let session = shorten_end(title, session_width);
    let text = if session.is_empty() {
        format!("{provider} {evidence}")
    } else {
        format!("{provider} {session} {evidence}")
    };
    vec![shorten_start(&text, width)]
}

fn signal_members(members: &[crate::attention::SessionIdentity], titles: &VisibleTitles) -> String {
    members
        .iter()
        .map(|identity| {
            let provider = match identity.provider {
                Provider::ClaudeCode => "CL",
                Provider::CodexCli => "CX",
            };
            let title = visible_title(titles, identity.provider, &identity.native_id);
            format!("{provider}:{title}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn signal_state_label(state: SessionState) -> &'static str {
    match state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => "Exited",
    }
}

fn context_alert_label(level: ContextAlertLevel) -> &'static str {
    match level {
        ContextAlertLevel::Warning => "Warning",
        ContextAlertLevel::Critical => "Critical",
    }
}

struct NowProject<'a> {
    project: Option<&'a Project>,
    worktrees: Vec<NowWorktree<'a>>,
    direct_sessions: Vec<&'a Session>,
}

impl NowProject<'_> {
    fn label(&self) -> &str {
        self.project.map_or("N/A", |project| project.label.as_str())
    }
}

struct NowWorktree<'a> {
    worktree: &'a Worktree,
    sessions: Vec<&'a Session>,
}

fn current_attention(snap: &Snapshot) -> Cow<'_, AttentionSnapshot> {
    match snap.attention.as_ref() {
        Some(attention) => Cow::Borrowed(attention),
        None => {
            Cow::Owned(AttentionEngine::default().update(&snap.sessions, snapshot_now_ms(snap)))
        }
    }
}

fn snapshot_now_ms(snap: &Snapshot) -> i64 {
    if snap.generated_ms > 0 {
        snap.generated_ms
    } else {
        Local::now().timestamp_millis()
    }
}

fn now_sessions(snap: &Snapshot) -> Vec<&Session> {
    let attention = current_attention(snap);
    group_open_sessions(&snap.sessions, &attention)
        .into_iter()
        .flat_map(|project| {
            project
                .worktrees
                .into_iter()
                .flat_map(|worktree| worktree.sessions)
                .chain(project.direct_sessions)
        })
        .collect()
}

fn group_open_sessions<'a>(
    sessions: &'a [Session],
    attention: &AttentionSnapshot,
) -> Vec<NowProject<'a>> {
    let mut projects: Vec<NowProject<'_>> = Vec::new();

    for session in sessions.iter().filter(|session| session.state.is_open()) {
        let project_index = session.project.as_ref().and_then(|project| {
            projects.iter().position(|group| {
                group
                    .project
                    .is_some_and(|existing| existing.identity == project.identity)
            })
        });
        let project_index = project_index.unwrap_or_else(|| {
            projects.push(NowProject {
                project: session.project.as_ref(),
                worktrees: Vec::new(),
                direct_sessions: Vec::new(),
            });
            projects.len() - 1
        });
        let project = &mut projects[project_index];

        if let Some(worktree) = session.worktree.as_ref() {
            let worktree_index = project
                .worktrees
                .iter()
                .position(|group| group.worktree.root == worktree.root)
                .unwrap_or_else(|| {
                    project.worktrees.push(NowWorktree {
                        worktree,
                        sessions: Vec::new(),
                    });
                    project.worktrees.len() - 1
                });
            project.worktrees[worktree_index].sessions.push(session);
        } else {
            project.direct_sessions.push(session);
        }
    }

    for project in &mut projects {
        for worktree in &mut project.worktrees {
            worktree
                .sessions
                .sort_by(|left, right| attention.compare_sessions(left, right));
        }
        project.worktrees.sort_by(|left, right| {
            compare_session_groups(attention, &left.sessions, &right.sessions)
                .then_with(|| left.worktree.root.cmp(&right.worktree.root))
        });
        project
            .direct_sessions
            .sort_by(|left, right| attention.compare_sessions(left, right));
    }
    projects.sort_by(|left, right| {
        compare_optional_sessions(
            attention,
            best_project_session(left, attention),
            best_project_session(right, attention),
        )
        .then_with(|| left.label().cmp(right.label()))
    });

    projects
}

fn best_project_session<'a>(
    project: &NowProject<'a>,
    attention: &AttentionSnapshot,
) -> Option<&'a Session> {
    project
        .worktrees
        .iter()
        .flat_map(|worktree| worktree.sessions.iter().copied())
        .chain(project.direct_sessions.iter().copied())
        .min_by(|left, right| attention.compare_sessions(left, right))
}

fn compare_session_groups(
    attention: &AttentionSnapshot,
    left: &[&Session],
    right: &[&Session],
) -> std::cmp::Ordering {
    compare_optional_sessions(attention, left.first().copied(), right.first().copied())
}

fn compare_optional_sessions(
    attention: &AttentionSnapshot,
    left: Option<&Session>,
    right: Option<&Session>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => attention.compare_sessions(left, right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn now_session_line(
    session: &Session,
    title: &str,
    attention: &AttentionSnapshot,
    now_ms: i64,
    column_widths: NowColumnWidths,
    selected: bool,
    theme: Theme,
) -> Line<'static> {
    let state = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => unreachable!("NOW contains only Open Sessions"),
    };
    let provider = match session.provider {
        Provider::ClaudeCode => "Claude",
        Provider::CodexCli => "Codex",
    };
    let attention_rank = attention.rank_for(session);
    let model = shorten_start(
        session.latest_model.as_deref().unwrap_or("N/A"),
        column_widths.model,
    );
    let title = fit_title(title, column_widths.title, true);
    let attention_label = match attention_rank {
        AttentionRank::CriticalContext => "CRITICAL",
        AttentionRank::SharedWorktreeRisk => "SHARED",
        AttentionRank::WarningContext => "WARNING",
        AttentionRank::Idle => "IDLE",
        AttentionRank::HealthyBusy => "BUSY",
    };
    let elapsed = attention
        .conditions_for(session)
        .iter()
        .find_map(|condition| match condition {
            AttentionCondition::Idle { since_ms, .. } => {
                Some(format!("Idle {}", fmt_relative(*since_ms, now_ms)))
            }
            _ => None,
        })
        .unwrap_or_else(|| fmt_relative(session.last_activity_ms, now_ms));
    let elapsed = shorten_start(&elapsed, 8);
    let cursor_style = if selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let base_style = Style::default().fg(theme.foreground);
    let attention_style = match attention_rank {
        AttentionRank::CriticalContext => Style::default()
            .fg(theme.critical)
            .add_modifier(Modifier::BOLD),
        AttentionRank::SharedWorktreeRisk | AttentionRank::WarningContext => {
            Style::default().fg(theme.warning)
        }
        AttentionRank::Idle => Style::default().fg(theme.idle),
        AttentionRank::HealthyBusy => Style::default().fg(theme.busy),
    };
    let lifecycle_style = match session.state {
        SessionState::Busy => Style::default().fg(theme.busy).add_modifier(Modifier::BOLD),
        SessionState::Idle => Style::default().fg(theme.idle),
        SessionState::Exited => unreachable!("NOW contains only Open Sessions"),
    };
    let mut spans = vec![
        Span::styled(
            format!("{} ", if selected { ">" } else { " " }),
            cursor_style,
        ),
        Span::styled(format!("{attention_label:<8} "), attention_style),
        Span::styled(format!("{state:<4} "), lifecycle_style),
        Span::styled(format!("{provider:<6} "), base_style),
        Span::styled(format!("{title} "), base_style),
    ];
    spans.extend(now_context_evidence(
        session,
        column_widths.context_bar,
        theme,
    ));
    spans.push(Span::styled(
        format!(
            "{model:<model_width$} {elapsed:<8}",
            model_width = column_widths.model
        ),
        base_style,
    ));
    Line::from(spans)
}

fn now_project_line(
    project: &NowProject<'_>,
    attention: &AttentionSnapshot,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let label = project.label().to_string();
    let label_width = label.chars().count();
    let mut spans = vec![Span::styled(
        label,
        Style::default()
            .fg(theme.foreground)
            .add_modifier(Modifier::BOLD),
    )];
    if project.worktrees.len() == 1 {
        spans.push(Span::styled(" | ", Style::default().fg(theme.border)));
        spans.push(Span::styled(
            now_worktree_line(
                &project.worktrees[0],
                attention,
                width.saturating_sub(label_width + 3),
                false,
            ),
            Style::default().fg(theme.muted),
        ));
    }
    Line::from(spans)
}

fn now_worktree_line(
    group: &NowWorktree<'_>,
    attention: &AttentionSnapshot,
    width: usize,
    indented: bool,
) -> String {
    let branch = group.worktree.branch.as_deref().unwrap_or("detached HEAD");
    let evidence = attention
        .shared_worktree_risk(&group.worktree.root)
        .map(|risk| {
            format!(
                " | WARNING Shared Worktree Risk x{}",
                risk.contributing_sessions.len()
            )
        })
        .unwrap_or_default();
    let prefix = if indented { "  " } else { "" };
    let identity_width = width.saturating_sub(prefix.len() + 3 + evidence.chars().count());
    let path_width = identity_width.saturating_mul(3) / 5;
    let branch_width = identity_width.saturating_sub(path_width);
    let path = home_relative_path(&group.worktree.root);
    format!(
        "{prefix}{} | {}{evidence}",
        shorten_end(&path, path_width),
        shorten_start(branch, branch_width),
    )
}

fn home_relative_path(path: &Path) -> Cow<'_, str> {
    let Some(home) = dirs::home_dir() else {
        return path.to_string_lossy();
    };
    let Ok(relative) = path.strip_prefix(home) else {
        return path.to_string_lossy();
    };
    if relative.as_os_str().is_empty() {
        Cow::Borrowed("~")
    } else {
        Cow::Owned(format!("~/{}", relative.to_string_lossy()))
    }
}

fn shorten_start(value: &str, width: usize) -> String {
    shorten(value, width, false)
}

fn shorten_end(value: &str, width: usize) -> String {
    shorten(value, width, true)
}

fn shorten(value: &str, width: usize, keep_end: bool) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.len() <= width {
        return value.into();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let kept = width - 3;
    if keep_end {
        format!(
            "...{}",
            characters[characters.len() - kept..]
                .iter()
                .collect::<String>()
        )
    } else {
        format!("{}...", characters[..kept].iter().collect::<String>())
    }
}

fn fit_title(value: &str, width: usize, keep_end: bool) -> String {
    let value_width = UnicodeWidthStr::width(value);
    if value_width <= width {
        return format!("{value}{}", " ".repeat(width - value_width));
    }
    if width <= 3 {
        return ".".repeat(width);
    }

    let available = width - 3;
    let mut kept = Vec::new();
    let mut kept_width = 0;
    let characters = if keep_end {
        value.chars().rev().collect::<Vec<_>>()
    } else {
        value.chars().collect::<Vec<_>>()
    };
    for character in characters {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if kept_width + character_width > available {
            break;
        }
        kept.push(character);
        kept_width += character_width;
    }
    if keep_end {
        kept.reverse();
    }
    let kept = kept.into_iter().collect::<String>();
    let fitted = if keep_end {
        format!("...{kept}")
    } else {
        format!("{kept}...")
    };
    let fitted_width = UnicodeWidthStr::width(fitted.as_str());
    format!("{fitted}{}", " ".repeat(width.saturating_sub(fitted_width)))
}

fn visible_input_tail(value: &str, width: usize) -> String {
    let mut visible = Vec::new();
    let mut visible_width = 0;
    for character in value.chars().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if visible_width + character_width > width {
            break;
        }
        visible.push(character);
        visible_width += character_width;
    }
    visible.reverse();
    visible.into_iter().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrentContextRole {
    Normal,
    Warning,
    Critical,
}

const MIN_CONTEXT_BAR_WIDTH: usize = 12;
const MAX_CONTEXT_BAR_WIDTH: usize = 20;
const COMPACT_TERMINAL_WIDTH: u16 = 80;
const COLUMNS_PER_CONTEXT_CELL: u16 = 5;
const MIN_NOW_TITLE_WIDTH: usize = 14;
const MAX_NOW_TITLE_WIDTH: usize = 20;
const MIN_NOW_MODEL_WIDTH: usize = 11;
const PREFERRED_MODEL_WIDTH: usize = 14;
const NOW_FIXED_WIDTH_WITHOUT_FLEXIBLE_COLUMNS: usize = 41;
const COMPACT_NOW_CONTENT_WIDTH: usize = NOW_FIXED_WIDTH_WITHOUT_FLEXIBLE_COLUMNS
    + MIN_NOW_TITLE_WIDTH
    + MIN_CONTEXT_BAR_WIDTH
    + MIN_NOW_MODEL_WIDTH;

#[derive(Debug, Clone, Copy)]
struct NowColumnWidths {
    title: usize,
    context_bar: usize,
    model: usize,
}

fn now_column_widths(terminal_width: u16, content_width: usize) -> NowColumnWidths {
    let title = now_title_width(content_width);
    let context_bar = now_context_bar_width(terminal_width, content_width, title);
    let model = content_width
        .saturating_sub(NOW_FIXED_WIDTH_WITHOUT_FLEXIBLE_COLUMNS + title + context_bar)
        .max(MIN_NOW_MODEL_WIDTH);
    NowColumnWidths {
        title,
        context_bar,
        model,
    }
}

fn now_title_width(content_width: usize) -> usize {
    MIN_NOW_TITLE_WIDTH
        + content_width
            .saturating_sub(COMPACT_NOW_CONTENT_WIDTH)
            .min(MAX_NOW_TITLE_WIDTH - MIN_NOW_TITLE_WIDTH)
}

fn preferred_context_bar_width(terminal_width: u16) -> usize {
    let extra_cells =
        terminal_width.saturating_sub(COMPACT_TERMINAL_WIDTH) / COLUMNS_PER_CONTEXT_CELL;
    (MIN_CONTEXT_BAR_WIDTH + extra_cells as usize).min(MAX_CONTEXT_BAR_WIDTH)
}

fn now_context_bar_width(terminal_width: u16, content_width: usize, title_width: usize) -> usize {
    let available_cells = content_width
        .saturating_sub(
            NOW_FIXED_WIDTH_WITHOUT_FLEXIBLE_COLUMNS + title_width + PREFERRED_MODEL_WIDTH,
        )
        .max(MIN_CONTEXT_BAR_WIDTH);
    preferred_context_bar_width(terminal_width).min(available_cells)
}

fn current_context_role(percent: f64) -> CurrentContextRole {
    if percent >= CRIT_PCT {
        CurrentContextRole::Critical
    } else if percent >= WARN_PCT {
        CurrentContextRole::Warning
    } else {
        CurrentContextRole::Normal
    }
}

fn current_context_style(role: CurrentContextRole, theme: Theme) -> Style {
    let color = match role {
        CurrentContextRole::Normal => theme.context_normal,
        CurrentContextRole::Warning => theme.context_warning,
        CurrentContextRole::Critical => theme.context_critical,
    };
    let style = Style::default().fg(color);
    if role == CurrentContextRole::Critical {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn now_context_evidence(session: &Session, bar_width: usize, theme: Theme) -> Vec<Span<'static>> {
    let muted = Style::default().fg(theme.muted);
    let evidence_width = bar_width + 8;
    let Some(percent) = session.context.percent() else {
        let text = format!("[{}] N/A", "-".repeat(bar_width));
        let padding = evidence_width.saturating_sub(text.chars().count());
        return vec![Span::styled(
            format!("{text}{}", " ".repeat(padding)),
            muted,
        )];
    };

    let filled =
        ((percent.clamp(0.0, 100.0) / 100.0 * bar_width as f64).round() as usize).min(bar_width);
    let current_role = current_context_role(percent);
    let mut spans = Vec::with_capacity(bar_width + 4);
    spans.push(Span::styled("[", muted));
    for index in 0..bar_width {
        if index < filled {
            spans.push(Span::styled(
                "#",
                current_context_style(current_role, theme),
            ));
        } else {
            spans.push(Span::styled("-", muted));
        }
    }
    spans.push(Span::styled("]", muted));

    let percent_text = format!(" {:.0}%", percent);
    let padding = evidence_width.saturating_sub(bar_width + 2 + percent_text.chars().count());
    spans.push(Span::styled(
        percent_text,
        current_context_style(current_context_role(percent), theme),
    ));
    spans.push(Span::styled(" ".repeat(padding), muted));
    spans
}

// ---- Provider page ----

struct ProviderProject<'a> {
    project: Option<&'a Project>,
    recent_sessions: Vec<&'a Session>,
    older_sessions: Vec<&'a Session>,
}

struct ProviderInventory<'a> {
    projects: Vec<ProviderProject<'a>>,
    unavailable_cwd: Vec<&'a Session>,
}

fn provider_inventory(
    sessions: &[Session],
    provider: Provider,
    now_ms: i64,
) -> ProviderInventory<'_> {
    const SEVEN_DAYS_MS: i64 = 7 * 86_400_000;
    let cutoff_ms = now_ms - SEVEN_DAYS_MS;
    let mut projects: Vec<ProviderProject<'_>> = Vec::new();
    let mut unavailable_cwd = Vec::new();

    for session in sessions
        .iter()
        .filter(|session| session.provider == provider)
    {
        if is_system_temporary_cwd(session.cwd.as_deref()) {
            continue;
        }
        if !session.state.is_open() && session.project.is_none() {
            unavailable_cwd.push(session);
            continue;
        }
        let project = projects.iter_mut().find(|group| {
            group.project.map(|project| &project.identity)
                == session.project.as_ref().map(|project| &project.identity)
        });
        if let Some(project) = project {
            if session.last_activity_ms >= cutoff_ms {
                project.recent_sessions.push(session);
            } else {
                project.older_sessions.push(session);
            }
        } else {
            projects.push(ProviderProject {
                project: session.project.as_ref(),
                recent_sessions: (session.last_activity_ms >= cutoff_ms)
                    .then_some(session)
                    .into_iter()
                    .collect(),
                older_sessions: (session.last_activity_ms < cutoff_ms)
                    .then_some(session)
                    .into_iter()
                    .collect(),
            });
        }
    }

    for project in &mut projects {
        project
            .recent_sessions
            .sort_by_key(|session| Reverse(session.last_activity_ms));
        project
            .older_sessions
            .sort_by_key(|session| Reverse(session.last_activity_ms));
    }
    projects.sort_by(|left, right| {
        let newest = |project: &ProviderProject<'_>| {
            project
                .recent_sessions
                .first()
                .or_else(|| project.older_sessions.first())
                .map_or(i64::MIN, |session| session.last_activity_ms)
        };
        newest(right).cmp(&newest(left)).then_with(|| {
            left.project
                .map_or("—", |project| project.label.as_str())
                .cmp(right.project.map_or("—", |project| project.label.as_str()))
        })
    });
    unavailable_cwd.sort_by_key(|session| Reverse(session.last_activity_ms));
    ProviderInventory {
        projects,
        unavailable_cwd,
    }
}

fn is_system_temporary_cwd(cwd: Option<&str>) -> bool {
    let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()).map(Path::new) else {
        return false;
    };
    [
        Path::new("/private/var/folders"),
        Path::new("/var/folders"),
        Path::new("/private/tmp"),
        Path::new("/tmp"),
    ]
    .iter()
    .any(|root| cwd.starts_with(root))
}

fn provider_page_rows<'a>(
    sessions: &'a [Session],
    provider: Provider,
    now_ms: i64,
    expanded_disclosures: &HashSet<ProviderDisclosureKey>,
) -> Vec<ProviderPageRow<'a>> {
    let mut rows = Vec::new();
    let inventory = provider_inventory(sessions, provider, now_ms);
    for project in inventory.projects {
        rows.push(ProviderPageRow::Project(project.project));
        let project_key = ProviderProjectKey {
            provider,
            project: project.project.map(|project| project.identity.clone()),
        };
        rows.extend(
            project
                .recent_sessions
                .iter()
                .take(PROVIDER_RECENT_SESSION_LIMIT)
                .copied()
                .map(ProviderPageRow::Session),
        );
        if project.recent_sessions.len() > PROVIDER_RECENT_SESSION_LIMIT {
            let key = ProviderDisclosureKey::MoreRecent(project_key.clone());
            let expanded = expanded_disclosures.contains(&key);
            rows.push(ProviderPageRow::Disclosure {
                key,
                kind: ProviderDisclosureKind::MoreRecent,
                count: project.recent_sessions.len() - PROVIDER_RECENT_SESSION_LIMIT,
                expanded,
            });
            if expanded {
                rows.extend(
                    project
                        .recent_sessions
                        .iter()
                        .skip(PROVIDER_RECENT_SESSION_LIMIT)
                        .copied()
                        .map(ProviderPageRow::Session),
                );
            }
        }
        if !project.older_sessions.is_empty() {
            let key = ProviderDisclosureKey::Older(project_key);
            let expanded = expanded_disclosures.contains(&key);
            rows.push(ProviderPageRow::Disclosure {
                key,
                kind: ProviderDisclosureKind::Older,
                count: project.older_sessions.len(),
                expanded,
            });
            if expanded {
                rows.extend(
                    project
                        .older_sessions
                        .into_iter()
                        .map(ProviderPageRow::Session),
                );
            }
        }
    }
    if !inventory.unavailable_cwd.is_empty() {
        let key = ProviderDisclosureKey::UnavailableCwd(provider);
        let expanded = expanded_disclosures.contains(&key);
        rows.push(ProviderPageRow::Disclosure {
            key,
            kind: ProviderDisclosureKind::UnavailableCwd,
            count: inventory.unavailable_cwd.len(),
            expanded,
        });
        if expanded {
            rows.extend(
                inventory
                    .unavailable_cwd
                    .into_iter()
                    .map(ProviderPageRow::Session),
            );
        }
    }
    rows
}

fn draw_provider(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let provider_rows = ui.provider_page_rows(snap);
    let visible_sessions = provider_rows
        .iter()
        .filter_map(|row| match row {
            ProviderPageRow::Session(session) => Some(*session),
            _ => None,
        })
        .collect::<Vec<_>>();
    let visible_identities = visible_sessions
        .iter()
        .map(|session| (session.provider, session.id.as_str()))
        .collect::<Vec<_>>();
    let titles = ui.labels.titles_for(&visible_identities);
    let now = snapshot_now_ms(snap);
    let theme = ui.theme.palette();

    if provider_rows.is_empty() {
        let msg = if snap.ready {
            "No sessions for this Provider."
        } else {
            "Indexing… sessions will appear shortly."
        };
        let p = Paragraph::new(msg)
            .style(Style::default().fg(theme.muted))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border)),
            );
        f.render_widget(p, area);
        return;
    }

    if area.width < 120 {
        draw_compact_provider(f, area, provider_rows, titles, now, ui, theme);
        return;
    }

    let header = Row::new(vec![
        Cell::from("State"),
        Cell::from("Title"),
        Cell::from("Model"),
        Cell::from("Context"),
        Cell::from("Lifetime Usage"),
        Cell::from("Est. $"),
        Cell::from("Updated"),
        Cell::from("Cmp"),
    ])
    .style(
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    );

    let mut rows = Vec::new();
    let mut selected_row = 0;
    let mut selectable = 0;
    let mut titles = titles.into_iter();
    for provider_row in provider_rows {
        match provider_row {
            ProviderPageRow::Project(project) => {
                rows.push(
                    Row::new(vec![
                        Cell::from("PROJECT"),
                        Cell::from(format!(
                            " {}",
                            project.map_or("—", |project| project.label.as_str())
                        )),
                    ])
                    .style(
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                );
            }
            ProviderPageRow::Session(session) => {
                if selectable == ui.selected {
                    selected_row = rows.len();
                }
                let title = titles.next().expect("every Session row has a Title");
                rows.push(session_row(session, &title, now, theme));
                selectable += 1;
            }
            ProviderPageRow::Disclosure {
                kind,
                count,
                expanded,
                ..
            } => {
                if selectable == ui.selected {
                    selected_row = rows.len();
                }
                rows.push(
                    Row::new(vec![
                        Cell::from(""),
                        Cell::from(format!(
                            "{} {} ({count})  {}",
                            if expanded { "-" } else { "+" },
                            kind.label(),
                            if expanded { "Collapse" } else { "Expand" }
                        )),
                    ])
                    .style(Style::default().fg(theme.muted)),
                );
                selectable += 1;
            }
        }
    }

    let widths = [
        Constraint::Length(7),
        Constraint::Length(20),
        Constraint::Length(18),
        Constraint::Length(14),
        Constraint::Length(34),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(3),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    format!(
                        " {} SESSIONS | grouped by Project ",
                        ui.tab.title().to_uppercase()
                    ),
                    Style::default().fg(theme.foreground),
                ))
                .title_bottom(
                    Line::from(Span::styled(
                        " showing full lifetime usage of matching sessions ",
                        Style::default().fg(theme.muted),
                    ))
                    .right_aligned(),
                ),
        )
        .row_highlight_style(selected_style(theme))
        .highlight_symbol("▌");

    let mut ts = TableState::default();
    if selectable > 0 {
        ts.select(Some(selected_row));
    }
    f.render_stateful_widget(table, area, &mut ts);
}

fn state_cell(state: SessionState, theme: Theme) -> Cell<'static> {
    match state {
        SessionState::Busy => Cell::from(Span::styled(
            "● Busy",
            Style::default().fg(theme.busy).add_modifier(Modifier::BOLD),
        )),
        SessionState::Idle => Cell::from(Span::styled("○ Idle", Style::default().fg(theme.idle))),
        SessionState::Exited => Cell::from(Span::styled("· —", Style::default().fg(theme.exited))),
    }
}

fn context_style(s: &Session, theme: Theme) -> Style {
    match s.context.percent() {
        Some(percent) => current_context_style(current_context_role(percent), theme),
        _ => Style::default().fg(theme.foreground),
    }
}

fn lifetime_usage_text(s: &Session) -> String {
    format!(
        "I {}  O {}  CR {}  CW {}",
        fmt_tokens(s.usage.input),
        fmt_tokens(s.usage.output),
        fmt_tokens(s.usage.cache_read),
        fmt_tokens(s.usage.cache_write),
    )
}

fn session_row(s: &Session, title: &str, now: i64, theme: Theme) -> Row<'static> {
    let model = s.latest_model.clone().unwrap_or_else(|| "—".into());
    let cmp = if s.compaction_count() > 0 {
        format!("⤳{}", s.compaction_count())
    } else {
        "·".to_string()
    };
    let cmp_style = if s.compaction_count() > 0 {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.muted)
    };

    Row::new(vec![
        state_cell(s.state, theme),
        Cell::from(Span::styled(
            title.to_string(),
            Style::default().fg(theme.foreground),
        )),
        Cell::from(Span::styled(model, Style::default().fg(theme.muted))),
        Cell::from(Span::styled(fmt_context(s), context_style(s, theme))),
        Cell::from(Span::styled(
            lifetime_usage_text(s),
            Style::default().fg(theme.foreground),
        )),
        Cell::from(Span::styled(
            fmt_cost(s.cost),
            Style::default().fg(theme.foreground),
        )),
        Cell::from(Span::styled(
            fmt_relative(s.last_activity_ms, now),
            Style::default().fg(theme.muted),
        )),
        Cell::from(Span::styled(cmp, cmp_style)),
    ])
}

fn draw_compact_provider(
    f: &mut Frame,
    area: Rect,
    provider_rows: Vec<ProviderPageRow<'_>>,
    titles: Vec<String>,
    now: i64,
    ui: &Ui,
    theme: Theme,
) {
    let mut lines = vec![
        Line::from(Span::styled(
            "  State  Title               Model               Context",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "         Transcript Lifetime Usage                 Est.$  Updated  Cmp",
            Style::default().fg(theme.muted),
        )),
    ];
    let mut selectable = 0;
    let mut selected_line = 0;
    let mut titles = titles.into_iter();

    for provider_row in provider_rows {
        match provider_row {
            ProviderPageRow::Project(project) => {
                lines.push(Line::from(Span::styled(
                    format!(
                        "PROJECT  {}",
                        project.map_or("—", |project| project.label.as_str())
                    ),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            ProviderPageRow::Session(session) => {
                if selectable == ui.selected {
                    selected_line = lines.len();
                }
                let title = titles.next().expect("every Session row has a Title");
                lines.extend(compact_session_lines(
                    session,
                    &title,
                    now,
                    selectable == ui.selected,
                    theme,
                ));
                selectable += 1;
            }
            ProviderPageRow::Disclosure {
                kind,
                count,
                expanded,
                ..
            } => {
                let selected = selectable == ui.selected;
                if selected {
                    selected_line = lines.len();
                }
                lines.push(Line::from(Span::styled(
                    format!(
                        "{} {} {} ({count})  {}",
                        if selected { ">" } else { " " },
                        if expanded { "-" } else { "+" },
                        kind.label(),
                        if expanded { "Collapse" } else { "Expand" }
                    ),
                    if selected {
                        selected_style(theme)
                    } else {
                        Style::default().fg(theme.muted)
                    },
                )));
                selectable += 1;
            }
        }
    }

    let visible_height = area.height.saturating_sub(2) as usize;
    let scroll = selected_line.saturating_sub(visible_height.saturating_sub(2)) as u16;
    let title = format!(
        " {} SESSIONS | grouped by Project ",
        ui.tab.title().to_uppercase()
    );
    f.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn compact_session_lines(
    session: &Session,
    title: &str,
    now: i64,
    selected: bool,
    theme: Theme,
) -> [Line<'static>; 2] {
    let selected_or = |style| {
        if selected {
            selected_style(theme)
        } else {
            style
        }
    };
    let state = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => "· —",
    };
    let state_style = match session.state {
        SessionState::Busy => Style::default().fg(theme.busy).add_modifier(Modifier::BOLD),
        SessionState::Idle => Style::default().fg(theme.idle),
        SessionState::Exited => Style::default().fg(theme.exited),
    };
    let model = session.latest_model.as_deref().unwrap_or("—");
    let compactions = if session.compaction_count() > 0 {
        format!("⤳{}", session.compaction_count())
    } else {
        "·".into()
    };

    [
        Line::from(vec![
            Span::styled(
                if selected { "> " } else { "  " },
                selected_or(Style::default().fg(theme.foreground)),
            ),
            Span::styled(format!("{state:<7}"), selected_or(state_style)),
            Span::styled(
                fit_title(title, 18, false),
                selected_or(Style::default().fg(theme.foreground)),
            ),
            Span::styled(
                format!("{:<18}", shorten_end(model, 18)),
                selected_or(Style::default().fg(theme.muted)),
            ),
            Span::styled(
                fmt_context(session),
                selected_or(context_style(session, theme)),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("    {}  ", lifetime_usage_text(session)),
                selected_or(Style::default().fg(theme.foreground)),
            ),
            Span::styled(
                format!("{}  ", fmt_cost(session.cost)),
                selected_or(Style::default().fg(theme.foreground)),
            ),
            Span::styled(
                format!("{}  ", fmt_relative(session.last_activity_ms, now)),
                selected_or(Style::default().fg(theme.muted)),
            ),
            Span::styled(compactions, selected_or(Style::default().fg(theme.accent))),
        ]),
    ]
}

// ---- detail child view ----

fn push_usage_summary(
    lines: &mut Vec<Line<'static>>,
    scope: &str,
    usage: &Usage,
    per_model: &[ModelUsage],
    cost: Option<f64>,
    theme: Theme,
) {
    let hdr = |key: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("{key:<16}"), Style::default().fg(theme.muted)),
            Span::styled(value, Style::default().fg(theme.foreground)),
        ])
    };
    lines.push(Line::from(Span::styled(
        format!("{scope} Usage"),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(hdr("Input", fmt_tokens(usage.input)));
    lines.push(hdr("Output", fmt_tokens(usage.output)));
    lines.push(hdr("Cache read", fmt_tokens(usage.cache_read)));
    lines.push(hdr("Cache write", fmt_tokens(usage.cache_write)));
    lines.push(hdr("Est. API cost", fmt_cost(cost)));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{scope} per-model Usage"),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    for model in per_model {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<20}", model.model),
                Style::default().fg(theme.foreground),
            ),
            Span::styled(
                format!(
                    "in {} out {} cr {} cw {}  ",
                    fmt_tokens(model.usage.input),
                    fmt_tokens(model.usage.output),
                    fmt_tokens(model.usage.cache_read),
                    fmt_tokens(model.usage.cache_write)
                ),
                Style::default().fg(theme.muted),
            ),
            Span::styled(fmt_cost(model.cost), Style::default().fg(theme.foreground)),
        ]));
    }
    lines.push(Line::from(""));
}

fn draw_detail(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    if let Some(task) = ui.detail_task(snap) {
        draw_task_detail(f, area, task, snap, ui);
        return;
    }
    let Some(s) = ui.detail_session(snap) else {
        return;
    };
    let theme = ui.theme.palette();

    if area.width <= 120 {
        draw_compact_detail(f, area, snap, ui, s, theme);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let hdr = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<16}"), Style::default().fg(theme.muted)),
            Span::styled(v, Style::default().fg(theme.foreground)),
        ])
    };

    let unavailable_git_field = match s.project.as_ref().map(|project| &project.identity) {
        Some(ProjectIdentity::CanonicalCwd(_)) => "N/A (non-Git Project)",
        _ => "N/A (identity unavailable)",
    };

    lines.push(hdr("Provider", s.provider.label().to_string()));
    lines.push(hdr(
        "Project",
        s.project
            .as_ref()
            .map(|project| project.label.clone())
            .unwrap_or_else(|| "N/A (identity unavailable)".into()),
    ));
    lines.push(hdr(
        "Worktree",
        s.worktree
            .as_ref()
            .map(|worktree| worktree.root.to_string_lossy().into_owned())
            .unwrap_or_else(|| unavailable_git_field.into()),
    ));
    lines.push(hdr(
        "Branch",
        match &s.worktree {
            Some(worktree) => worktree
                .branch
                .clone()
                .unwrap_or_else(|| "N/A (detached HEAD)".into()),
            None => unavailable_git_field.into(),
        },
    ));
    lines.push(hdr("cwd", s.cwd.clone().unwrap_or_else(|| "N/A".into())));
    lines.push(hdr(
        "Source log",
        CopyField::SourceLog
            .value(s)
            .map(Cow::into_owned)
            .unwrap_or_else(|| "N/A".into()),
    ));
    lines.push(hdr("Source format", s.source.format.label().into()));
    lines.push(hdr("Title", ui.labels.title(s.provider, &s.id)));
    lines.push(hdr("Session ID", s.id.clone()));
    lines.push(hdr(
        "Latest model",
        s.latest_model.clone().unwrap_or_else(|| "—".into()),
    ));
    lines.push(hdr(
        "State",
        match s.state {
            SessionState::Busy => "Busy".into(),
            SessionState::Idle => "Idle".into(),
            SessionState::Exited => "exited (no marker)".into(),
        },
    ));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "Current Context",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    let ctx = match s.context.percent() {
        Some(pct) => format!(
            "{} / {} ({:.1}%)",
            fmt_tokens(s.context.tokens),
            s.context
                .window
                .map(fmt_tokens)
                .unwrap_or_else(|| "?".into()),
            pct
        ),
        None => format!("{} / N/A (N/A)", fmt_tokens(s.context.tokens)),
    };
    lines.push(hdr("Occupancy", ctx));
    lines.push(Line::from(""));

    if s.state.is_open() {
        match &s.current_run_usage {
            Some(run) => push_usage_summary(
                &mut lines,
                "Current Run",
                &run.usage,
                &run.per_model,
                run.cost,
                theme,
            ),
            None => {
                lines.push(Line::from(Span::styled(
                    "Current Run Usage",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(hdr("Usage", "N/A".into()));
                lines.push(Line::from(""));
            }
        }
    }

    push_usage_summary(
        &mut lines,
        "Transcript Lifetime",
        &s.usage,
        &s.per_model,
        s.cost,
        theme,
    );

    lines.push(Line::from(Span::styled(
        format!("Compactions ({})", s.compaction_count()),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    if s.compactions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none observed",
            Style::default().fg(theme.muted),
        )));
    } else {
        for c in &s.compactions {
            let trigger = c.trigger.clone().unwrap_or_else(|| "—".into());
            let pre = c.pre_tokens.map(fmt_tokens).unwrap_or_else(|| "?".into());
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}  trigger={}  pre={}",
                    fmt_datetime(c.at_ms),
                    trigger,
                    pre
                ),
                Style::default().fg(theme.foreground),
            )));
        }
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let wrap = Wrap { trim: false };
    let total_lines = Paragraph::new(lines.clone())
        .wrap(wrap)
        .line_count(area.width.saturating_sub(2));
    let max_scroll = total_lines.saturating_sub(inner_height);
    let scroll = ui
        .detail
        .as_ref()
        .map_or(0, |detail| detail.scroll.min(max_scroll));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            " Session detail — Esc to go back ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                format!(
                    " lines {}-{} of {} ",
                    scroll + usize::from(total_lines > 0),
                    (scroll + inner_height).min(total_lines),
                    total_lines
                ),
                Style::default().fg(theme.muted),
            ))
            .right_aligned(),
        );
    let p = Paragraph::new(lines)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .block(block)
        .scroll((scroll as u16, 0))
        .wrap(wrap);
    f.render_widget(p, area);
}

fn draw_task_detail(f: &mut Frame, area: Rect, task: &OperationTask, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let field = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("{label:<24}"), Style::default().fg(theme.muted)),
            Span::styled(value, Style::default().fg(theme.foreground)),
        ])
    };
    let successor_provider = task.successor_provider;
    let successor_is_indexed = task
        .successor_session_id
        .as_ref()
        .is_some_and(|session_id| {
            snap.sessions.iter().any(|session| {
                session.provider == task.successor_provider && &session.id == session_id
            })
        });
    let result_ready = task.state == crate::tasks::OperationTaskState::Ready
        && task.artifact_path.is_some()
        && task.successor_launch_plan.is_some();
    let acknowledgement = match (task.state.is_terminal(), task.acknowledged_at_ms) {
        (true, Some(at_ms)) => format!("Acknowledged {}", fmt_datetime(at_ms)),
        (true, None) => "Unread terminal outcome".into(),
        (false, _) => "Not terminal".into(),
    };
    let failure_code = task.failure_code.as_deref().unwrap_or("None");
    let failure_summary = match task.failure_code.as_deref() {
        Some("worker_start_failed") => "Detached Worker could not start.",
        Some("handoff_skill_missing") => {
            "The managed Handoff Skill was missing before Provider startup."
        }
        Some("handoff_skill_stale") => "The managed Handoff Skill changed after authorization.",
        Some("handoff_skill_unreadable") => {
            "The managed Handoff Skill became unreadable before Provider startup."
        }
        Some("handoff_skill_conflict") => {
            "The managed Handoff Skill path contains a conflicting entry."
        }
        Some("provider_invocation_failed") => "Provider invocation failed.",
        Some("provider_outcome_uncertain") => "Provider outcome could not be established safely.",
        Some("invalid_artifact") => "Candidate Markdown did not satisfy the Artifact contract.",
        Some("artifact_publish_failed") => "Validated Artifact publication failed.",
        Some("current_run_authority_expired") => {
            "Unused Current Run authority expired at its deadline."
        }
        Some("source_current_run_identity_changed") => {
            "The exact source Current Run identity changed."
        }
        Some("source_current_run_control_uncertain") => {
            "The graceful-exit outcome could not be established safely."
        }
        Some("source_current_run_observation_uncertain") => {
            "The source Current Run could not be observed conclusively."
        }
        Some("cancelled_by_user") => "Cancellation completed with no valid Handoff Result.",
        Some("worker_start_deadline_exceeded") => {
            "The detached Worker did not establish startup within 30 seconds."
        }
        Some("running_deadline_exceeded") => {
            "Generation reached 30 minutes and graceful termination completed."
        }
        Some("execution_outcome_uncertain") => {
            "Process and Result evidence could not establish the outcome."
        }
        Some("worker_lost_before_prompt") | Some("worker_lost_before_control") => {
            "The Worker exited before any Provider Prompt or Current Run control."
        }
        Some("worker_lost_after_current_run_control") => {
            "The Worker exited after Current Run control and before generation."
        }
        Some("provider_termination_uncertain") | Some("current_run_reconciliation_uncertain") => {
            "Graceful termination evidence is uncertain."
        }
        Some(_) => "A normalized Task failure was recorded.",
        None => "No failure recorded.",
    };
    let retry_of_task = task
        .retry_of_task_id
        .as_ref()
        .map(|predecessor_id| {
            if snap
                .tasks
                .iter()
                .any(|candidate| &candidate.id == predecessor_id)
            {
                predecessor_id.clone()
            } else {
                format!("{predecessor_id} (unavailable)")
            }
        })
        .unwrap_or_else(|| "None".into());
    let mut action = if task.is_being_deleted() {
        "Esc Back".to_string()
    } else if !task.state.is_terminal() {
        "x Cancel Task | Esc Back".to_string()
    } else if task.offers_retry() {
        "r Retry | d Delete | Esc Back".to_string()
    } else if task.state == crate::tasks::OperationTaskState::Ready {
        "o Open Terminal | c Copy Command | d Delete | Esc Close for Later".to_string()
    } else {
        "d Delete | Esc Back".to_string()
    };
    if successor_is_indexed {
        action = format!("Enter View Successor | {action}");
    }
    let mut lines = vec![
        field("Task ID", task.id.clone()),
        field("Operation", "Handoff".into()),
        field("Retry of Task", retry_of_task),
        Line::from(""),
        field("Source Provider", task.source_provider.label().into()),
        field("Source Session", task.source_session_id.clone()),
        field(
            "Working directory",
            task.source_working_directory.to_string_lossy().into_owned(),
        ),
        field("Successor Provider", successor_provider.label().into()),
        field(
            "Successor Session",
            task.successor_session_id
                .clone()
                .unwrap_or_else(|| "Not linked".into()),
        ),
        field(
            "Successor linkage",
            match (task.successor_session_id.is_some(), successor_is_indexed) {
                (true, true) => "Linked and indexed",
                (true, false) => "Linked, not indexed",
                (false, _) => "Not linked",
            }
            .into(),
        ),
        field(
            "Linked at",
            task.successor_linked_at_ms
                .map(fmt_datetime)
                .unwrap_or_else(|| "None".into()),
        ),
        Line::from(""),
        field("State", task.state.label().into()),
        field("Created", fmt_datetime(task.created_at_ms)),
        field("Latest transition", fmt_datetime(task.state_changed_at_ms)),
        field("Acknowledgement", acknowledgement),
    ];
    if let Some(_requested_at_ms) = task.deletion_requested_at_ms {
        lines.extend([
            field(
                "Deletion",
                if task.deletion_failure_code.is_some() {
                    "Failed; will retry"
                } else {
                    "In progress"
                }
                .into(),
            ),
            field(
                "Deletion failure",
                task.deletion_failure_code
                    .clone()
                    .unwrap_or_else(|| "None".into()),
            ),
        ]);
    }
    if task.end_current_run_authorized {
        let waiting_reason = match task.waiting_reason.as_deref() {
            Some("source_current_run_busy") => "Source Current Run is Busy",
            Some("source_current_run_idle") => "Source Current Run is Idle",
            Some(_) => "Source Current Run is not ready",
            None => "None",
        };
        lines.extend([
            field("Waiting reason", waiting_reason.into()),
            field(
                "Authorization deadline",
                task.waiting_deadline_ms
                    .map(fmt_datetime)
                    .unwrap_or_else(|| "None".into()),
            ),
            field(
                "Current Run control",
                task.control_used_at_ms
                    .map(|at_ms| format!("Used {}", fmt_datetime(at_ms)))
                    .unwrap_or_else(|| "Not used".into()),
            ),
        ]);
    }
    lines.extend([
        Line::from(""),
        field("Failure", failure_code.into()),
        field("Failure detail", failure_summary.into()),
        field(
            "Result",
            if result_ready { "Ready" } else { "Not ready" }.into(),
        ),
        field(
            "Artifact",
            if task.artifact_path.is_some() {
                "Published"
            } else {
                "Not published"
            }
            .into(),
        ),
        field(
            "Launch Plan",
            if task.successor_launch_plan.is_some() {
                "Ready"
            } else {
                "Not ready"
            }
            .into(),
        ),
        Line::from(""),
        field("Available actions", action),
    ]);
    if task.state == crate::tasks::OperationTaskState::Ready {
        lines.push(Line::from(Span::styled(
            "The Handoff Result remains in TASKS after closing.",
            Style::default().fg(theme.muted),
        )));
    }
    let inner_height = area.height.saturating_sub(2) as usize;
    let wrap = Wrap { trim: false };
    let total_lines = Paragraph::new(lines.clone())
        .wrap(wrap)
        .line_count(area.width.saturating_sub(2));
    let max_scroll = total_lines.saturating_sub(inner_height);
    let scroll = ui
        .detail
        .as_ref()
        .map_or(0, |detail| detail.scroll.min(max_scroll));
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .scroll((scroll as u16, 0))
            .wrap(wrap)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .title(Span::styled(
                        " Task detail ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    )),
            ),
        area,
    );
}

fn draw_compact_detail(
    f: &mut Frame,
    area: Rect,
    snap: &Snapshot,
    ui: &Ui,
    session: &Session,
    theme: Theme,
) {
    let field = |key: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("{key:<14}"), Style::default().fg(theme.muted)),
            Span::styled(value, Style::default().fg(theme.foreground)),
        ])
    };
    let section = |label: String| {
        Line::from(Span::styled(
            label,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let state = match session.state {
        SessionState::Busy => "Busy",
        SessionState::Idle => "Idle",
        SessionState::Exited => "Exited",
    };
    let context = match session.context.percent() {
        Some(percent) => format!(
            "{} / {} ({percent:.1}%)",
            fmt_tokens(session.context.tokens),
            session
                .context
                .window
                .map(fmt_tokens)
                .unwrap_or_else(|| "N/A".into())
        ),
        None => format!("{} / N/A (N/A)", fmt_tokens(session.context.tokens)),
    };
    let usage_line = |usage: &Usage, cost: Option<f64>| {
        format!(
            "in {} | out {} | cache read {} | cache create {} | API est. {}",
            fmt_tokens(usage.input),
            fmt_tokens(usage.output),
            fmt_tokens(usage.cache_read),
            fmt_tokens(usage.cache_write),
            fmt_cost(cost),
        )
    };

    let mut lines = vec![
        field(
            "Provider",
            format!("{} / {state}", session.provider.label()),
        ),
        field(
            "Project",
            session
                .project
                .as_ref()
                .map(|project| project.label.clone())
                .unwrap_or_else(|| "N/A".into()),
        ),
        field("Title", ui.labels.title(session.provider, &session.id)),
        field("Session ID", session.id.clone()),
        field(
            "Model",
            session.latest_model.clone().unwrap_or_else(|| "N/A".into()),
        ),
        field("Context", context),
        section("Attention Conditions".into()),
    ];
    let attention = current_attention(snap);
    let conditions = attention.conditions_for(session);
    if session.state == SessionState::Exited {
        lines.push(Line::from(Span::styled(
            "  None - Exited Session has no current Attention Conditions",
            Style::default().fg(theme.muted),
        )));
    } else if conditions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  None - healthy Busy Session",
            Style::default().fg(theme.busy),
        )));
    } else {
        for condition in conditions {
            let (text, style) = match condition {
                AttentionCondition::ContextAlert {
                    severity,
                    percentage,
                    threshold_percent,
                    ..
                } => {
                    let label = match severity {
                        AttentionSeverity::Critical => "CRITICAL",
                        AttentionSeverity::Warning => "WARNING",
                    };
                    let color = match severity {
                        AttentionSeverity::Critical => theme.critical,
                        AttentionSeverity::Warning => theme.warning,
                    };
                    (
                        format!(
                            "  {label} Current Context {percentage:.1}% >= {threshold_percent}%"
                        ),
                        Style::default().fg(color),
                    )
                }
                AttentionCondition::SharedWorktreeRisk {
                    contributing_sessions,
                    ..
                } => (
                    format!("  WARNING Shared Worktree Risk x{contributing_sessions}"),
                    Style::default().fg(theme.warning),
                ),
                AttentionCondition::Idle { since_ms, .. } => (
                    format!(
                        "  IDLE awaiting input for {}",
                        fmt_relative(*since_ms, snapshot_now_ms(snap))
                    ),
                    Style::default().fg(theme.idle),
                ),
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
    }

    if session.state.is_open() {
        lines.push(section("Current Run Usage".into()));
        match &session.current_run_usage {
            Some(run) => lines.push(Line::from(usage_line(&run.usage, run.cost))),
            None => lines.push(Line::from("N/A")),
        }
    }
    lines.push(section("Transcript Lifetime Usage".into()));
    lines.push(Line::from(usage_line(&session.usage, session.cost)));
    let source_width = area.width.saturating_sub(2) as usize;
    push_wrapped_detail_field(
        &mut lines,
        "Worktree",
        &session.worktree.as_ref().map_or_else(
            || "N/A".into(),
            |worktree| {
                format!(
                    "{} @ {}",
                    worktree.root.display(),
                    worktree.branch.as_deref().unwrap_or("detached HEAD")
                )
            },
        ),
        source_width,
        theme,
    );
    push_wrapped_detail_field(
        &mut lines,
        "cwd",
        session.cwd.as_deref().unwrap_or("N/A"),
        source_width,
        theme,
    );
    let source_log = CopyField::SourceLog
        .value(session)
        .map(Cow::into_owned)
        .unwrap_or_else(|| "N/A".into());
    push_wrapped_detail_field(&mut lines, "Source log", &source_log, source_width, theme);
    push_wrapped_detail_field(
        &mut lines,
        "Source format",
        session.source.format.label(),
        source_width,
        theme,
    );
    let compaction_text = |compaction: &crate::domain::Compaction| {
        format!(
            "  {} ago  trigger={}  pre={}",
            fmt_relative(compaction.at_ms, snapshot_now_ms(snap)),
            compaction.trigger.as_deref().unwrap_or("N/A"),
            compaction
                .pre_tokens
                .map(fmt_tokens)
                .unwrap_or_else(|| "N/A".into())
        )
    };
    let mut compaction_heading = vec![Span::styled(
        format!("Compactions ({})", session.compaction_count()),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )];
    if session.compactions.is_empty() {
        compaction_heading.push(Span::styled(
            "  none observed".to_string(),
            Style::default().fg(theme.muted),
        ));
        lines.push(Line::from(compaction_heading));
    } else {
        compaction_heading.push(Span::styled(
            compaction_text(&session.compactions[0]),
            Style::default().fg(theme.foreground),
        ));
        lines.push(Line::from(compaction_heading));
        for compaction in session.compactions.iter().skip(1) {
            lines.push(Line::from(compaction_text(compaction)));
        }
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let wrap = Wrap { trim: false };
    let total_lines = Paragraph::new(lines.clone())
        .wrap(wrap)
        .line_count(area.width.saturating_sub(2));
    let max_scroll = total_lines.saturating_sub(inner_height);
    let scroll = ui
        .detail
        .as_ref()
        .map_or(0, |detail| detail.scroll.min(max_scroll));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            " Session detail - Esc to go back ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                format!(
                    " lines {}-{} of {} ",
                    scroll + usize::from(total_lines > 0),
                    (scroll + inner_height).min(total_lines),
                    total_lines
                ),
                Style::default().fg(theme.muted),
            ))
            .right_aligned(),
        );
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(block)
            .scroll((scroll as u16, 0))
            .wrap(wrap),
        area,
    );
}

fn push_wrapped_detail_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    width: usize,
    theme: Theme,
) {
    const LABEL_WIDTH: usize = 14;
    let value_width = width.saturating_sub(LABEL_WIDTH).max(1);
    let characters = value.chars().collect::<Vec<_>>();
    let chunks = characters.chunks(value_width).collect::<Vec<_>>();
    for (index, chunk) in chunks.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                if index == 0 {
                    format!("{label:<LABEL_WIDTH$}")
                } else {
                    " ".repeat(LABEL_WIDTH)
                },
                Style::default().fg(theme.muted),
            ),
            Span::styled(
                chunk.iter().collect::<String>(),
                Style::default().fg(theme.foreground),
            ),
        ]));
    }
    if chunks.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{label:<LABEL_WIDTH$}"),
            Style::default().fg(theme.muted),
        )));
    }
}

// ---- footer ----

fn draw_footer(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    let compact = area.width < 100;
    let can_edit_title = ui.labels.is_editable() && ui.selected_session(snap).is_some();
    let can_handoff = ui.detail_session(snap).is_some();
    let now = Instant::now();
    let status_notice = ui
        .status_notice
        .as_ref()
        .filter(|(_, until)| now < *until)
        .map(|(message, _)| message.clone());
    let label_error = ui.labels.error().map(str::to_string);
    let task_detail = ui.detail_task(snap);
    let left = if ui.task_retry_confirmation.is_some() {
        "Left/Right Provider | Enter Authorize Retry | Esc Cancel"
    } else if ui.task_cancellation_confirmation.is_some() {
        "Enter Confirm Cancel | Esc Keep Task"
    } else if ui.handoff_confirmation.is_some() {
        "Left/Right Provider | Enter Authorize | Esc Cancel"
    } else if task_detail.is_some_and(OperationTask::is_being_deleted) {
        "Esc Back | j/k Scroll | 1/2/3 | ? Guide | q Quit"
    } else if task_detail.is_some_and(OperationTask::offers_retry) {
        "d Delete | r Retry | Esc | j/k | 1/2/3 | ? | q Quit"
    } else if task_detail.is_some_and(|task| task.state == crate::tasks::OperationTaskState::Ready)
    {
        "d Delete | o Open Terminal | c Copy Command | Esc"
    } else if task_detail.is_some_and(|task| !task.state.is_terminal()) {
        "x Cancel | Esc Back | j/k Scroll | 1/2/3 | ? Guide | q Quit"
    } else if task_detail.is_some() {
        "d Delete | Esc | j/k | 1/2/3 | ? | q Quit"
    } else if compact && status_notice.is_some() {
        ""
    } else if ui.copy_menu_is_open() && area.width < 100 {
        "j/k Select | Enter Copy | Esc Cancel"
    } else if ui.copy_menu_is_open() {
        "Up/Down or j/k Select | Enter Copy | Esc Cancel"
    } else if ui.detail.is_some() && compact && can_edit_title && can_handoff {
        "Esc | j/k | c | e Edit title | h | ? | q Quit"
    } else if ui.detail.is_some() && compact && can_edit_title {
        "Esc | j/k | c Copy | e Edit title | ? Guide | q Quit"
    } else if ui.detail.is_some() && compact {
        "Esc Back | j/k Scroll | c Copy | ? Guide | q Quit"
    } else if ui.detail.is_some() && can_edit_title && can_handoff {
        "Esc | j/k | c | e Edit title | h Handoff | 1/2/3 | ? | q Quit"
    } else if ui.detail.is_some() && can_edit_title {
        "Esc | j/k | c Copy | e Edit title | 1/2/3 | ? Guide | q Quit"
    } else if ui.detail.is_some() {
        "Esc | j/k Scroll | c Copy | 1/2/3 | ? Guide | q Quit"
    } else if compact && ui.tab == Tab::All && can_edit_title {
        "j/k | Enter | e Edit title | ? Guide | q Quit"
    } else if compact && ui.tab == Tab::All {
        "j/k | Enter Inspect | ? Guide | q Quit"
    } else if compact && can_edit_title {
        "j/k | Enter Inspect | e Edit title | ? Guide | q Quit"
    } else if compact {
        "j/k | Enter Inspect/Expand | ? Guide | q Quit"
    } else if ui.tab == Tab::All && can_edit_title {
        "j/k | Enter Inspect | e Edit title | 1/2/3 | r | ? Guide | q Quit"
    } else if ui.tab == Tab::All {
        "j/k | Enter Inspect | 1/2/3 | r | ? Guide | q Quit"
    } else if can_edit_title {
        "j/k | Enter Inspect | e Edit title | 1/2/3 | ? Guide | q Quit"
    } else {
        "j/k | Enter Inspect/Expand | 1/2/3 | ? Guide | q Quit"
    };
    let status = if let Some(status_notice) = status_notice.as_ref() {
        status_notice.clone()
    } else if let Some(label_error) = label_error.as_ref() {
        label_error.clone()
    } else {
        snap.status.clone()
    };
    let shows_price_note = ui.tab != Tab::All || ui.detail.is_some();
    let right = if status_notice.is_some() {
        status
    } else if compact && shows_price_note {
        "Estimated using API rates".to_string()
    } else if ui.tab == Tab::All && ui.detail.is_none() {
        status
    } else {
        format!("{status}   ·   Estimated using API rates")
    };
    let cols = if label_error.is_some() && status_notice.is_none() {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(0), Constraint::Min(1)])
            .split(area)
    } else if compact {
        let available = area
            .width
            .saturating_sub(left.chars().count() as u16)
            .saturating_sub(1);
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length((right.chars().count() as u16).min(available)),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area)
    };
    f.render_widget(
        Paragraph::new(Span::styled(left, Style::default().fg(theme.muted))),
        cols[0],
    );
    let status_color = if status_notice.is_some() {
        theme.accent
    } else if label_error.is_some() {
        theme.critical
    } else {
        theme.muted
    };
    f.render_widget(
        Paragraph::new(Span::styled(right, Style::default().fg(status_color)))
            .alignment(Alignment::Right),
        cols[1],
    );
}

fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::AlertCounts;
    use crate::attention::AttentionEngine;
    use crate::domain::{
        Compaction, ContextState, ModelUsage, Project, ProjectIdentity, Provider, Usage,
        UsageSummary, Worktree,
    };
    use crate::signals::{RecentSignal, RecentSignalKind, SharedWorktreeRiskChange};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_project(label: &str) -> Project {
        Project {
            identity: ProjectIdentity::CanonicalCwd(PathBuf::from(format!("/test/{label}"))),
            label: label.into(),
        }
    }

    fn unused_profile_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "sessionmesh-{name}-{}-{nonce}-profile.json",
            std::process::id(),
        ))
    }

    fn render_terminal_with_snapshot(ui: &Ui, snapshot: &Snapshot) -> Terminal<TestBackend> {
        render_terminal_at(ui, snapshot, 80, 24)
    }

    fn render_terminal_at(
        ui: &Ui,
        snapshot: &Snapshot,
        width: u16,
        height: u16,
    ) -> Terminal<TestBackend> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, snapshot, ui)).unwrap();
        terminal
    }

    fn render_terminal(ui: &Ui) -> Terminal<TestBackend> {
        render_terminal_with_snapshot(ui, &Snapshot::default())
    }

    fn rendered(ui: &Ui) -> String {
        render_terminal(ui)
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn rendered_with_snapshot(ui: &Ui, snapshot: &Snapshot) -> String {
        render_terminal_with_snapshot(ui, snapshot)
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn operation_task(state: crate::tasks::OperationTaskState) -> OperationTask {
        OperationTask {
            id: format!("task-{}", state.label().to_lowercase()),
            state,
            source_provider: Provider::ClaudeCode,
            source_session_id: "source-session".into(),
            source_record_path: PathBuf::from("/private/source.jsonl"),
            source_working_directory: PathBuf::from("/private/project"),
            successor_provider: Provider::ClaudeCode,
            successor_executable: PathBuf::from("/usr/local/bin/claude"),
            created_at_ms: 1_000,
            state_changed_at_ms: 2_000,
            worker_process_identity: None,
            provider_process_identity: None,
            prompt_sent_at_ms: None,
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

    #[test]
    fn dismissing_cancel_confirmation_changes_no_non_terminal_task() {
        for state in [
            crate::tasks::OperationTaskState::Queued,
            crate::tasks::OperationTaskState::Waiting,
            crate::tasks::OperationTaskState::Running,
        ] {
            let task = operation_task(state);
            let snapshot = Snapshot {
                tasks: vec![task.clone()],
                ..Snapshot::default()
            };
            let mut ui = Ui {
                all_region: AllRegion::Tasks,
                ..Ui::default()
            };
            let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
            ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

            assert_eq!(
                ui.handle_key(key(KeyCode::Char('x')), &snapshot).unwrap(),
                UiAction::None
            );
            assert!(rendered_with_snapshot(&ui, &snapshot).contains("Cancel Task?"));
            assert_eq!(
                ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap(),
                UiAction::None
            );
            assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Cancel Task?"));
            assert_eq!(snapshot.tasks, vec![task]);
        }
    }

    #[test]
    fn confirming_cancel_emits_the_selected_non_terminal_task_identity() {
        let task = operation_task(crate::tasks::OperationTaskState::Running);
        let snapshot = Snapshot {
            tasks: vec![task.clone()],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(
            ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
            UiAction::None
        );
        ui.handle_key(key(KeyCode::Char('x')), &snapshot).unwrap();

        assert_eq!(
            ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
            UiAction::CancelTask { task_id: task.id }
        );
    }

    #[test]
    fn declining_irreversible_task_deletion_preserves_the_terminal_task() {
        let mut task = operation_task(crate::tasks::OperationTaskState::Ready);
        let artifact_path = PathBuf::from("/private/artifacts/task-ready/handoff.md");
        task.artifact_path = Some(artifact_path.clone());
        task.successor_launch_plan = Some(crate::tasks::SuccessorLaunchPlan {
            successor_provider: Provider::ClaudeCode,
            working_directory: PathBuf::from("/private/project"),
            executable: PathBuf::from("/usr/local/bin/claude"),
            arguments: vec!["continue".into()],
            artifact_path,
        });
        task.acknowledged_at_ms = Some(2_500);
        let snapshot = Snapshot {
            tasks: vec![task.clone()],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

        assert_eq!(
            ui.handle_key(key(KeyCode::Char('d')), &snapshot).unwrap(),
            UiAction::None
        );
        let confirmation = rendered_with_snapshot(&ui, &snapshot);
        assert!(confirmation.contains("Delete Task and Artifact?"));
        assert!(confirmation.contains(&task.id));
        assert!(confirmation.contains("irreversible"));
        assert!(confirmation.contains("Handoff Artifact"));
        assert!(confirmation.contains("Successor Launch Plan"));

        assert_eq!(
            ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap(),
            UiAction::None
        );
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Delete Task and Artifact?"));
        assert_eq!(snapshot.tasks, vec![task]);
    }

    #[test]
    fn confirming_irreversible_task_deletion_emits_the_selected_task_identity() {
        let task = operation_task(crate::tasks::OperationTaskState::Cancelled);
        let snapshot = Snapshot {
            tasks: vec![task.clone()],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Char('d')), &snapshot).unwrap();

        assert_eq!(
            ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
            UiAction::DeleteTask {
                task_id: task.id.clone()
            }
        );
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Delete Task and Artifact?"));
        assert_eq!(snapshot.tasks, vec![task]);
    }

    #[test]
    fn deleting_task_detail_shows_progress_or_recoverable_failure_without_conflicting_actions() {
        let mut task = operation_task(crate::tasks::OperationTaskState::Ready);
        task.deletion_requested_at_ms = Some(3_000);
        let mut snapshot = Snapshot {
            tasks: vec![task.clone()],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(
            ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
            UiAction::None
        );

        let in_progress = rendered_with_snapshot(&ui, &snapshot);
        assert!(in_progress.contains("Deletion                In progress"));
        for unavailable in ["d Delete", "r Retry", "o Open Terminal", "c Copy Command"] {
            assert!(!in_progress.contains(unavailable));
        }

        task.deletion_failure_code = Some("artifact_remove_failed".into());
        snapshot.tasks = vec![task];
        let failed = rendered_with_snapshot(&ui, &snapshot);
        assert!(failed.contains("Deletion                Failed; will retry"));
        assert!(failed.contains("Deletion failure        artifact_remove_failed"));
        assert!(!failed.contains("d Delete"));
    }

    #[test]
    fn retained_retry_marks_its_deleted_predecessor_identity_as_unavailable() {
        let mut retry = operation_task(crate::tasks::OperationTaskState::Failed);
        retry.id = "retry-task".into();
        retry.retry_of_task_id = Some("deleted-predecessor-uuid".into());
        let snapshot = Snapshot {
            tasks: vec![retry],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        assert!(rendered_with_snapshot(&ui, &snapshot)
            .contains("deleted-predecessor-uuid (unavailable)"));
    }

    #[test]
    fn task_deletion_is_available_only_for_terminal_states() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        for state in [
            crate::tasks::OperationTaskState::Ready,
            crate::tasks::OperationTaskState::Failed,
            crate::tasks::OperationTaskState::Interrupted,
            crate::tasks::OperationTaskState::Cancelled,
        ] {
            let snapshot = Snapshot {
                tasks: vec![operation_task(state)],
                ..Snapshot::default()
            };
            let mut ui = Ui {
                all_region: AllRegion::Tasks,
                ..Ui::default()
            };
            ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
            assert!(rendered_with_snapshot(&ui, &snapshot).contains("d Delete"));
            ui.handle_key(key(KeyCode::Char('d')), &snapshot).unwrap();
            assert!(rendered_with_snapshot(&ui, &snapshot).contains("Delete Task and Artifact?"));
        }

        for state in [
            crate::tasks::OperationTaskState::Queued,
            crate::tasks::OperationTaskState::Waiting,
            crate::tasks::OperationTaskState::Running,
        ] {
            let snapshot = Snapshot {
                tasks: vec![operation_task(state)],
                ..Snapshot::default()
            };
            let mut ui = Ui {
                all_region: AllRegion::Tasks,
                ..Ui::default()
            };
            ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
            assert!(!rendered_with_snapshot(&ui, &snapshot).contains("d Delete"));
            ui.handle_key(key(KeyCode::Char('d')), &snapshot).unwrap();
            assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Delete Task and Artifact?"));
        }
    }

    #[test]
    fn task_detail_offers_retry_only_for_eligible_terminal_outcomes() {
        for state in [
            crate::tasks::OperationTaskState::Failed,
            crate::tasks::OperationTaskState::Cancelled,
        ] {
            let snapshot = Snapshot {
                tasks: vec![operation_task(state)],
                ..Snapshot::default()
            };
            let mut ui = Ui {
                all_region: AllRegion::Tasks,
                ..Ui::default()
            };
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
                .unwrap();
            assert!(rendered_with_snapshot(&ui, &snapshot).contains("r Retry"));
        }

        let mut interrupted = operation_task(crate::tasks::OperationTaskState::Interrupted);
        let mut snapshot = Snapshot {
            tasks: vec![interrupted.clone()],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("r Retry"));

        interrupted.retry_ready_at_ms = Some(3_000);
        snapshot.tasks = vec![interrupted];
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("r Retry"));

        snapshot.tasks = vec![operation_task(crate::tasks::OperationTaskState::Ready)];
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("r Retry"));
    }

    #[test]
    fn retry_requires_fresh_confirmation_before_emitting_an_action() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].state = SessionState::Exited;
        let mut task = operation_task(crate::tasks::OperationTaskState::Failed);
        task.source_provider = snapshot.sessions[0].provider;
        task.source_session_id = snapshot.sessions[0].id.clone();
        snapshot.tasks = vec![task.clone()];
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

        assert_eq!(
            ui.handle_key(key(KeyCode::Char('r')), &snapshot).unwrap(),
            UiAction::None
        );
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("Authorize Retry?"));
        assert_eq!(
            ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap(),
            UiAction::None
        );

        ui.handle_key(key(KeyCode::Char('r')), &snapshot).unwrap();
        assert_eq!(
            ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
            UiAction::RetryTask {
                predecessor_task_id: task.id,
                source_provider: snapshot.sessions[0].provider,
                source_session_id: snapshot.sessions[0].id.clone(),
                successor_provider: snapshot.sessions[0].provider,
                end_current_run: false,
            }
        );
    }

    #[test]
    fn retry_defaults_to_the_source_provider_and_emits_a_fresh_successor_choice() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].state = SessionState::Exited;
        let mut task = operation_task(crate::tasks::OperationTaskState::Failed);
        task.source_provider = snapshot.sessions[0].provider;
        task.source_session_id = snapshot.sessions[0].id.clone();
        task.successor_provider = Provider::CodexCli;
        snapshot.tasks = vec![task.clone()];
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Char('r')), &snapshot).unwrap();

        assert!(
            rendered_with_snapshot(&ui, &snapshot).contains("Successor  [Claude Code]  Codex CLI")
        );
        ui.handle_key(key(KeyCode::Right), &snapshot).unwrap();
        assert!(
            rendered_with_snapshot(&ui, &snapshot).contains("Successor  Claude Code  [Codex CLI]")
        );
        assert_eq!(
            ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap(),
            UiAction::RetryTask {
                predecessor_task_id: task.id,
                source_provider: snapshot.sessions[0].provider,
                source_session_id: snapshot.sessions[0].id.clone(),
                successor_provider: Provider::CodexCli,
                end_current_run: false,
            }
        );
    }

    #[test]
    fn ready_task_offers_open_terminal_and_copy_command_for_the_same_result() {
        let mut task = operation_task(crate::tasks::OperationTaskState::Ready);
        let artifact_path = PathBuf::from("/private/artifacts/task/handoff.md");
        task.artifact_path = Some(artifact_path.clone());
        task.successor_launch_plan = Some(crate::tasks::SuccessorLaunchPlan {
            successor_provider: Provider::ClaudeCode,
            working_directory: PathBuf::from("/private/project"),
            executable: PathBuf::from("/usr/local/bin/claude"),
            arguments: vec!["Read the Handoff Artifact and continue.".into()],
            artifact_path,
        });
        let snapshot = Snapshot {
            tasks: vec![task.clone()],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(detail.contains("o Open Terminal"));
        assert!(detail.contains("c Copy Command"));
        assert_eq!(
            ui.handle_key(key(KeyCode::Char('c')), &snapshot).unwrap(),
            UiAction::CopySuccessorCommand {
                task_id: task.id.clone()
            }
        );
        assert_eq!(
            ui.handle_key(key(KeyCode::Char('o')), &snapshot).unwrap(),
            UiAction::OpenSuccessorTerminal { task_id: task.id }
        );
    }

    #[test]
    fn linked_successor_becomes_navigable_after_the_session_reaches_the_index() {
        let mut task = operation_task(crate::tasks::OperationTaskState::Ready);
        task.successor_provider = Provider::CodexCli;
        task.successor_session_id = Some("indexed-codex-successor".into());
        task.successor_linked_at_ms = Some(3_000);
        let mut snapshot = Snapshot {
            tasks: vec![task],
            ..Snapshot::default()
        };
        let mut ui = Ui {
            all_region: AllRegion::Tasks,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

        let waiting_for_index = rendered_with_snapshot(&ui, &snapshot);
        assert!(waiting_for_index.contains("Successor Session       indexed-codex-successor"));
        assert!(waiting_for_index.contains("Linked, not indexed"));
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("Task ID"));

        let mut successor = demo_snapshot().sessions.remove(0);
        successor.provider = Provider::CodexCli;
        successor.id = "indexed-codex-successor".into();
        snapshot.sessions.push(successor);
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();

        let successor_detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(successor_detail.contains("Session detail"));
        assert!(successor_detail.contains("indexed-codex-successor"));
        assert!(!successor_detail.contains("Task ID"));
    }

    #[test]
    fn non_terminal_task_detail_shows_the_authorized_cross_provider_successor() {
        for state in [
            crate::tasks::OperationTaskState::Queued,
            crate::tasks::OperationTaskState::Waiting,
            crate::tasks::OperationTaskState::Running,
        ] {
            let mut task = operation_task(state);
            task.successor_provider = Provider::CodexCli;
            task.successor_executable = PathBuf::from("/usr/local/bin/codex");
            let snapshot = Snapshot {
                tasks: vec![task],
                ..Snapshot::default()
            };
            let mut ui = Ui {
                all_region: AllRegion::Tasks,
                ..Ui::default()
            };
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
                .unwrap();

            let detail = rendered_with_snapshot(&ui, &snapshot);
            assert!(detail.contains("Successor Provider      Codex CLI"));
        }
    }

    fn rendered_rows_at(ui: &Ui, snapshot: &Snapshot, width: u16, height: u16) -> String {
        let terminal = render_terminal_at(ui, snapshot, width, height);
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn rendered_style_at(terminal: &Terminal<TestBackend>, text: &str) -> Style {
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        let cell_count = text.chars().count();
        for row in buffer.content().chunks(width) {
            if let Some(cells) = row
                .windows(cell_count)
                .find(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == text)
            {
                return cells[0].style();
            }
        }
        panic!("rendered text not found: {text}");
    }

    fn now_cursor_style_at(terminal: &Terminal<TestBackend>, session_text: &str) -> Option<Style> {
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        let now_width = if width >= 120 {
            width * 67 / 100
        } else {
            width
        };
        let cell_count = session_text.chars().count();
        for row in buffer.content().chunks(width) {
            let now_row = &row[..now_width];
            let Some(session_start) = now_row.windows(cell_count).position(|cells| {
                cells.iter().map(|cell| cell.symbol()).collect::<String>() == session_text
            }) else {
                continue;
            };
            return now_row[..session_start]
                .iter()
                .find(|cell| cell.symbol() == ">")
                .map(|cell| cell.style());
        }
        panic!("rendered Session not found: {session_text}");
    }

    fn demo_snapshot() -> Snapshot {
        let session = Session {
            provider: Provider::ClaudeCode,
            id: "demo-session".into(),
            source: Default::default(),
            project: Some(Project {
                identity: ProjectIdentity::GitCommonDir(PathBuf::from(
                    "/Users/x/code/sessionmesh/.git",
                )),
                label: "sessionmesh".into(),
            }),
            worktree: Some(Worktree {
                root: PathBuf::from("/Users/x/code/sessionmesh"),
                branch: Some("main".into()),
                writable: true,
            }),
            cwd: Some("/Users/x/code/sessionmesh".into()),
            latest_model: Some("claude-opus-4-8".into()),
            state: SessionState::Busy,
            context: ContextState {
                tokens: 150_000,
                window: Some(200_000),
            },
            usage: Usage {
                input: 1000,
                output: 500,
                cache_read: 200,
                cache_write: 100,
            },
            per_model: vec![ModelUsage {
                model: "claude-opus-4-8".into(),
                usage: Usage {
                    input: 1000,
                    output: 500,
                    cache_read: 200,
                    cache_write: 100,
                },
                cost: Some(0.42),
            }],
            current_run_usage: None,
            compactions: vec![],
            last_activity_ms: 0,
            cost: Some(0.42),
        };
        Snapshot {
            sessions: vec![session],
            alerts: AlertCounts {
                warning: 1,
                critical: 0,
            },
            status: "watching".into(),
            ready: true,
            ..Default::default()
        }
    }

    fn render_tab(tab: Tab, detail: bool) {
        let snap = demo_snapshot();
        let mut ui = Ui {
            tab,
            selected: 0,
            ..Ui::default()
        };
        if detail {
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snap)
                .unwrap();
        }
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        // Must not panic while laying out or drawing.
        terminal.draw(|f| draw(f, &snap, &ui)).unwrap();
    }

    #[test]
    fn t_key_does_not_change_the_active_theme_or_show_a_notice() {
        let mut ui = Ui::default();
        let snapshot = Snapshot {
            status: "watching".into(),
            ..Snapshot::default()
        };

        assert_eq!(ui.active_theme_name(), "sessionmesh");
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        assert_eq!(ui.active_theme_name(), "sessionmesh");
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Theme:"));
    }

    #[test]
    fn each_theme_supplies_the_selected_page_structural_role() {
        for (name, selected_background) in [
            ("sessionmesh", Color::Rgb(68, 61, 43)),
            ("catppuccin", Color::Rgb(69, 71, 90)),
            ("dracula", Color::Rgb(68, 71, 90)),
            ("gruvbox", Color::Rgb(80, 73, 69)),
            ("light", Color::Rgb(20, 84, 116)),
            ("high-contrast", Color::Rgb(255, 255, 255)),
        ] {
            let profile_path = unused_profile_path(name);
            std::fs::write(
                &profile_path,
                format!(r#"{{"help_seen":true,"theme":"{name}"}}"#),
            )
            .unwrap();
            let ui = Ui::load(&profile_path, None).unwrap();
            let terminal = render_terminal(&ui);
            assert_eq!(
                rendered_style_at(&terminal, "1 2 3").bg,
                Some(selected_background)
            );
            let _ = std::fs::remove_file(profile_path);
        }
    }

    #[test]
    fn sessionmesh_uses_semantic_busy_and_critical_context_roles() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].context.tokens = 180_000;
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut selected = snapshot.sessions[0].clone();
        selected.id = "selected-session".into();
        selected.project = Some(test_project("selected-project"));
        selected.context.tokens = 50_000;
        selected.last_activity_ms = now - 1_000;
        snapshot.sessions.push(selected);
        let ui = Ui {
            tab: Tab::Claude,
            selected: 1,
            ..Ui::default()
        };

        let terminal = render_terminal_with_snapshot(&ui, &snapshot);

        assert_eq!(
            rendered_style_at(&terminal, "Busy").fg,
            Some(Color::Rgb(120, 205, 120))
        );
        assert_eq!(
            rendered_style_at(&terminal, "180.0k (90%)").fg,
            Some(Color::Rgb(255, 95, 109))
        );
    }

    #[test]
    fn sessionmesh_uses_semantic_idle_exited_and_warning_roles() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        snapshot.sessions[0].context.tokens = 50_000;

        let mut idle = snapshot.sessions[0].clone();
        idle.id = "idle-session".into();
        idle.project = Some(test_project("idle-project"));
        idle.state = SessionState::Idle;
        idle.context.tokens = 150_000;
        idle.last_activity_ms = now - 1_000;

        let mut exited = snapshot.sessions[0].clone();
        exited.id = "exited-session".into();
        exited.project = Some(test_project("exited-project"));
        exited.state = SessionState::Exited;
        exited.context.tokens = 50_000;
        exited.last_activity_ms = now - 2_000;
        snapshot.sessions.extend([idle, exited]);

        let ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
        let terminal = render_terminal_with_snapshot(&ui, &snapshot);

        assert_eq!(
            rendered_style_at(&terminal, "Idle").fg,
            Some(Color::Rgb(105, 184, 220))
        );
        assert_eq!(
            rendered_style_at(&terminal, "150.0k (75%)").fg,
            Some(Color::Rgb(238, 190, 70))
        );
        assert_eq!(
            rendered_style_at(&terminal, "· —").fg,
            Some(Color::Rgb(112, 116, 120))
        );
    }

    #[test]
    fn light_theme_supplies_background_border_accent_and_muted_roles() {
        let profile_path = unused_profile_path("light-roles");
        std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
        let mut ui = Ui::load(&profile_path, None).unwrap();
        let snapshot = Snapshot::default();
        ui.tab = Tab::Claude;

        let terminal = render_terminal_with_snapshot(&ui, &snapshot);
        let top_left = &terminal.backend().buffer()[(0, 0)];

        assert_eq!(top_left.style().bg, Some(Color::Rgb(250, 249, 246)));
        assert_eq!(top_left.style().fg, Some(Color::Rgb(126, 130, 133)));
        assert_eq!(
            rendered_style_at(&terminal, "SessionMesh").fg,
            Some(Color::Rgb(25, 91, 130))
        );
        assert_eq!(
            rendered_style_at(&terminal, "Indexing… sessions will appear shortly.").fg,
            Some(Color::Rgb(96, 101, 106))
        );
        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn now_groups_open_sessions_across_providers_and_omits_exited_sessions() {
        let ui = Ui::default();
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("claude-open-model".into());

        let mut codex = snapshot.sessions[0].clone();
        codex.provider = Provider::CodexCli;
        codex.id = "codex-open".into();
        codex.latest_model = Some("codex-open-model".into());
        codex.state = SessionState::Idle;

        let mut exited = snapshot.sessions[0].clone();
        exited.id = "exited-session".into();
        exited.latest_model = Some("exited-model".into());
        exited.state = SessionState::Exited;
        snapshot.sessions.extend([codex, exited]);

        let page = rendered_rows_at(&ui, &snapshot, 120, 30);
        let project = page.find("\n│sessionmesh").unwrap();
        let worktree = page.find("WARNING Shared Worktree Risk x2").unwrap();
        let claude = page.find("Claude -session").unwrap();
        let codex = page.find("Codex  dex-open").unwrap();

        assert!(project < worktree && worktree < claude && claude < codex);
        assert_eq!(
            page.lines()
                .filter(|line| line.starts_with("│sessionmesh"))
                .count(),
            1
        );
        let session_rows = page
            .lines()
            .filter(|line| line.contains("-session") || line.contains("dex-open"))
            .collect::<Vec<_>>();
        assert_eq!(session_rows.len(), 2);
        assert!(session_rows.iter().any(|line| line.contains("-session")));
        assert!(session_rows.iter().any(|line| line.contains("dex-open")));
        assert!(!page.contains("exited-model"));
    }

    #[test]
    fn now_applies_attention_order_to_projects_worktrees_and_sessions() {
        let mut healthy = demo_snapshot().sessions.remove(0);
        healthy.id = "healthy".into();
        healthy.project = Some(test_project("healthy"));
        healthy.worktree.as_mut().unwrap().root = PathBuf::from("/worktrees/healthy");
        healthy.latest_model = Some("healthy-model".into());
        healthy.context.tokens = 80_000;

        let mut warning = healthy.clone();
        warning.id = "warning".into();
        warning.project = Some(test_project("priority"));
        warning.worktree.as_mut().unwrap().root = PathBuf::from("/worktrees/a-warning");
        warning.latest_model = Some("warning-model".into());
        warning.context.tokens = 150_000;

        let mut shared = warning.clone();
        shared.provider = Provider::CodexCli;
        shared.id = "shared".into();
        shared.worktree.as_mut().unwrap().root = PathBuf::from("/worktrees/b-critical");
        shared.latest_model = Some("shared-model".into());
        shared.context.tokens = 80_000;

        let mut critical = shared.clone();
        critical.provider = Provider::ClaudeCode;
        critical.id = "critical".into();
        critical.latest_model = Some("critical-model".into());
        critical.state = SessionState::Idle;
        critical.context.tokens = 180_000;

        let mut snapshot = Snapshot {
            sessions: vec![healthy, warning, shared, critical],
            status: "watching".into(),
            ready: true,
            ..Snapshot::default()
        };
        snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let priority_project = page.find("\n│priority").unwrap();
        let critical_worktree = page.find("WARNING Shared Worktree Risk x2").unwrap();
        let critical_session = page.find("Claude critical").unwrap();
        let shared_session = page.find("Codex  shared").unwrap();
        let warning_worktree = page.find("/worktrees/a-warning").unwrap();
        let warning_session = page.find("Claude warning").unwrap();
        let healthy_project = page.find("\n│healthy").unwrap();
        let healthy_session = page.find("Claude healthy").unwrap();

        assert!(priority_project < critical_worktree);
        assert!(critical_worktree < critical_session && critical_session < shared_session);
        assert!(shared_session < warning_worktree && warning_worktree < warning_session);
        assert!(warning_session < healthy_project && healthy_project < healthy_session);
        let priority_evidence = &page[priority_project..healthy_project];
        assert!(priority_evidence.contains("CRITICAL"));
        assert!(priority_evidence.contains("SHARED"));
        assert!(priority_evidence.contains("WARNING"));
        assert!(priority_evidence
            .lines()
            .find(|line| line.contains("Claude critical"))
            .unwrap()
            .contains("Idle"));
    }

    #[test]
    fn now_shows_shared_worktree_risk_and_every_contributing_session() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].context.tokens = 80_000;
        snapshot.sessions[0].latest_model = Some("claude-member".into());
        let mut codex = snapshot.sessions[0].clone();
        codex.provider = Provider::CodexCli;
        codex.id = "codex-contributor".into();
        codex.latest_model = Some("codex-member".into());
        codex.state = SessionState::Idle;
        snapshot.sessions.push(codex);
        snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let risk_line = page
            .lines()
            .find(|line| line.contains("WARNING Shared Worktree Risk x2"))
            .unwrap();
        let risk = page.find(risk_line).unwrap();
        let claude = page.find("Claude -session").unwrap();
        let codex = page.find("Codex  tributor").unwrap();

        assert!(risk_line.contains("sessionmesh"));
        assert!(risk_line.contains("| main"));
        assert!(risk < claude && risk < codex);
        assert_eq!(
            page.lines()
                .filter(|line| line.contains("Claude -session") || line.contains("Codex  tributor"))
                .count(),
            2
        );
    }

    #[test]
    fn now_shows_critical_context_label_bar_and_exact_percentage() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("critical-model".into());
        snapshot.sessions[0].context.tokens = 180_000;
        snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let critical = page.lines().find(|line| line.contains("-session")).unwrap();

        assert!(critical.contains("CRITICAL"));
        assert!(critical.contains("[###########-] 90%"));
        assert!(critical.contains("Busy"));
        assert!(critical.contains("Claude"));
        assert!(critical.contains("-session"));
    }

    #[test]
    fn now_shows_warning_context_label_bar_and_exact_percentage() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("warning-model".into());
        snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let warning = page.lines().find(|line| line.contains("-session")).unwrap();

        assert!(warning.contains("WARNING"));
        assert!(warning.contains("[#########---] 75%"));
    }

    #[test]
    fn now_shows_the_last_eight_native_session_id_characters() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].id = "12345678remaining-id".into();
        snapshot.sessions[0].latest_model = Some("session-id-model".into());

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let session = page.lines().find(|line| line.contains("ining-id")).unwrap();

        assert!(session.contains("Claude ining-id"));
        assert!(session.contains("sess"));
        assert!(!session.contains("12345678"));
    }

    #[test]
    fn now_preserves_lifecycle_colors_when_attention_overrides_rank() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].context.tokens = 150_000;
        let mut idle = snapshot.sessions[0].clone();
        idle.id = "idle-warning".into();
        idle.project = Some(test_project("idle-warning-project"));
        idle.worktree = Some(Worktree {
            root: PathBuf::from("/Users/x/code/idle-warning"),
            branch: Some("main".into()),
            writable: true,
        });
        idle.cwd = Some("/Users/x/code/idle-warning".into());
        idle.state = SessionState::Idle;
        snapshot.sessions.push(idle);
        let ui = Ui {
            selected: usize::MAX,
            ..Ui::default()
        };

        let terminal = render_terminal_at(&ui, &snapshot, 120, 30);
        let busy = rendered_style_at(&terminal, "Busy");
        let idle = rendered_style_at(&terminal, "Idle");
        let warning = rendered_style_at(&terminal, "WARNING");

        assert_eq!(busy.fg, Some(Color::Rgb(120, 205, 120)));
        assert!(busy.add_modifier.contains(Modifier::BOLD));
        assert_eq!(idle.fg, Some(Color::Rgb(105, 184, 220)));
        assert_eq!(warning.fg, Some(Color::Rgb(238, 190, 70)));
        assert_ne!(busy, warning);
        assert_ne!(idle, warning);
    }

    #[test]
    fn now_shows_idle_elapsed_from_transition_and_last_activity_fallback() {
        let now_ms = Local::now().timestamp_millis();
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].id = "observed-idle".into();
        snapshot.sessions[0].latest_model = Some("observed-idle".into());
        snapshot.sessions[0].context.tokens = 80_000;
        snapshot.sessions[0].last_activity_ms = now_ms - 20 * 60_000;
        let mut engine = AttentionEngine::default();
        engine.update(&snapshot.sessions, now_ms - 5 * 60_000);

        snapshot.sessions[0].state = SessionState::Idle;
        let mut fallback = snapshot.sessions[0].clone();
        fallback.provider = Provider::CodexCli;
        fallback.id = "fallback-idle".into();
        fallback.latest_model = Some("fallback-idle".into());
        fallback.project = Some(test_project("fallback"));
        fallback.worktree.as_mut().unwrap().root = PathBuf::from("/test/fallback");
        fallback.last_activity_ms = now_ms - 11 * 60_000;
        snapshot.sessions.push(fallback);
        snapshot.attention = Some(engine.update(&snapshot.sessions, now_ms - 4 * 60_000));

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let observed = page
            .lines()
            .find(|line| line.contains("Claude ved-idle"))
            .unwrap();
        let fallback = page
            .lines()
            .find(|line| line.contains("Codex  ack-idle"))
            .unwrap();

        assert!(observed.contains("IDLE"));
        assert!(observed.contains("Idle 4m"));
        assert!(fallback.contains("IDLE"));
        assert!(fallback.contains("Idle 11m"));
    }

    #[test]
    fn all_page_selection_round_trips_through_session_detail() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("first-open-model".into());
        let mut second = snapshot.sessions[0].clone();
        second.provider = Provider::CodexCli;
        second.id = "second-open".into();
        second.latest_model = Some("second-open-model".into());
        snapshot.sessions.push(second);
        let mut ui = Ui::default();

        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(detail.contains("second-open"));
        assert!(!detail.contains("first-open-model"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let all = render_terminal_with_snapshot(&ui, &snapshot);
        let cursor = now_cursor_style_at(&all, "ond-open").unwrap();
        assert_eq!(cursor.fg, Some(Color::Rgb(230, 180, 80)));
        assert!(cursor.add_modifier.contains(Modifier::BOLD));
        assert_eq!(now_cursor_style_at(&all, "-session"), None);
    }

    #[test]
    fn now_aligns_known_and_unknown_current_context_evidence() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("known-model".into());
        snapshot.sessions[0].context = ContextState {
            tokens: 140_000,
            window: Some(200_000),
        };
        let mut unknown = snapshot.sessions[0].clone();
        unknown.provider = Provider::CodexCli;
        unknown.id = "unknown-context".into();
        unknown.latest_model = Some("unknown-model".into());
        unknown.context = ContextState {
            tokens: 42_000,
            window: None,
        };
        snapshot.sessions.push(unknown);
        let ui = Ui::default();

        let terminal = render_terminal_at(&ui, &snapshot, 120, 30);
        let page = terminal
            .backend()
            .buffer()
            .content()
            .chunks(120)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>();
        let known = page
            .iter()
            .find(|line| line.contains("Claude -session"))
            .unwrap();
        let unknown = page
            .iter()
            .find(|line| line.contains("Codex  -context"))
            .unwrap();

        assert!(known.contains("[########----] 70%"));
        assert!(unknown.contains("[------------] N/A"));
        assert_eq!(known.find("known-model"), unknown.find("unknown-..."));
        assert_eq!(
            rendered_style_at(&terminal, "[------------] N/A").fg,
            Some(Color::Rgb(132, 136, 141))
        );
    }

    #[test]
    fn now_merges_a_single_worktree_into_the_project_without_context() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].worktree.as_mut().unwrap().branch = Some("feature/shared".into());
        let ui = Ui::default();

        let page = rendered_rows_at(&ui, &snapshot, 120, 30);
        let project = page
            .lines()
            .find(|line| line.starts_with("│sessionmesh |"))
            .unwrap();

        assert!(project.contains("/Users/x/code/sessionmesh | feature/shared"));
        assert!(!project.contains("Context"));
    }

    #[test]
    fn now_keeps_multiple_worktrees_below_the_project_without_context() {
        let mut snapshot = demo_snapshot();
        let mut feature = snapshot.sessions[0].clone();
        feature.id = "feature-session".into();
        feature.worktree = Some(Worktree {
            root: PathBuf::from("/Users/x/worktrees/sessionmesh-feature"),
            branch: Some("feature".into()),
            writable: true,
        });
        feature.cwd = Some("/Users/x/worktrees/sessionmesh-feature".into());
        snapshot.sessions.push(feature);

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let mut lines = page.lines();
        let project = lines.find(|line| line.starts_with("│sessionmesh")).unwrap();
        let main = lines.find(|line| line.contains("| main")).unwrap();
        let feature = lines.find(|line| line.contains("| feature")).unwrap();

        assert_eq!(
            project.split("││").next().unwrap().trim_end(),
            "│sessionmesh"
        );
        assert!(main.starts_with("│  "));
        assert!(feature.starts_with("│  "));
        assert!(!main.contains("Context"));
        assert!(!feature.contains("Context"));
    }

    #[test]
    fn all_page_has_global_usage_without_a_time_filter_or_cost_surface() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].cost = Some(123.45);
        snapshot.sessions[0].usage = Usage {
            input: 987_654,
            output: 123_456,
            cache_read: 456_789,
            cache_write: 12_345,
        };
        let mut ui = Ui::default();

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        let page = rendered_rows_at(&ui, &snapshot, 120, 30);

        for excluded in [
            "Active Today",
            "Last 7 Days",
            "All Sessions",
            "Estimated using API rates",
            "$123",
        ] {
            assert!(
                !page.contains(excluded),
                "unexpected All-page text: {excluded}"
            );
        }
        let header = page.lines().next().unwrap();
        assert!(header.contains("In 987.7k"));
        assert!(header.contains("Out 123.5k"));
    }

    #[test]
    fn now_shares_the_main_page_with_recent_signals_only() {
        let snapshot = demo_snapshot();
        let ui = Ui::default();

        let wide = rendered_rows_at(&ui, &snapshot, 120, 30);
        let wide_top = wide
            .lines()
            .find(|line| line.contains(" PROJECTS "))
            .unwrap();
        assert_eq!(
            wide_top.chars().position(|character| character == '┐'),
            Some(79)
        );

        let compact = rendered_rows_at(&ui, &snapshot, 80, 24);
        assert_eq!(
            compact
                .lines()
                .enumerate()
                .find_map(|(index, line)| (index > 2 && line.starts_with('└')).then_some(index)),
            Some(14)
        );

        assert!(wide.contains("RECENT SIGNALS"));
        assert!(compact.contains("RECENT SIGNALS"));
        for speculative_label in ["FOCUS", "PROJECT PULSE"] {
            assert!(!wide.contains(speculative_label));
            assert!(!compact.contains(speculative_label));
        }
    }

    #[test]
    fn recent_signals_render_newest_first_without_history_controls() {
        let mut snapshot = demo_snapshot();
        snapshot.recent_signals = vec![
            RecentSignal::Session {
                observed_at_ms: 2_000,
                provider: Provider::ClaudeCode,
                session_id: "newest-signal".into(),
                kind: RecentSignalKind::SessionStateChanged {
                    from: SessionState::Busy,
                    to: SessionState::Idle,
                },
            },
            RecentSignal::Session {
                observed_at_ms: 1_000,
                provider: Provider::CodexCli,
                session_id: "older-signal".into(),
                kind: RecentSignalKind::SessionStateChanged {
                    from: SessionState::Idle,
                    to: SessionState::Exited,
                },
            },
        ];

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);

        assert!(page.contains("RECENT SIGNALS"));
        assert!(page.find("t-signal").unwrap() < page.find("r-signal").unwrap());
        assert!(page.contains("Busy -> Idle"));
        assert!(page.contains("Idle -> Exited"));
        for excluded in ["View More", "Filter", "Scroll"] {
            assert!(!page.contains(excluded));
        }
    }

    #[test]
    fn shared_worktree_signal_renders_canonical_root_and_membership_evidence() {
        let mut snapshot = demo_snapshot();
        snapshot.recent_signals = vec![RecentSignal::Worktree {
            observed_at_ms: 2_000,
            worktree_root: "/wt/shared".into(),
            change: SharedWorktreeRiskChange::Detected,
            before: vec![crate::attention::SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "claude-a".into(),
            }],
            after: vec![
                crate::attention::SessionIdentity {
                    provider: Provider::ClaudeCode,
                    native_id: "claude-a".into(),
                },
                crate::attention::SessionIdentity {
                    provider: Provider::CodexCli,
                    native_id: "codex-b".into(),
                },
            ],
        }];

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);

        for evidence in [
            "/wt/shared",
            "Risk detected",
            "Before 1: CL:claude-a",
            "After 2: CL:claude-a, CX:codex-b",
        ] {
            assert!(page.contains(evidence), "missing evidence {evidence:?}");
        }
        assert!(!page.to_lowercase().contains("conflict"));
    }

    #[test]
    fn worktree_signal_locates_the_recommended_now_session_not_its_first_member() {
        let mut snapshot = demo_snapshot();
        let worktree_root = PathBuf::from("/wt/shared");
        let healthy = &mut snapshot.sessions[0];
        healthy.id = "arbitrary-first".into();
        healthy.latest_model = Some("arbitrary-model".into());
        healthy.context.tokens = 80_000;
        healthy.worktree.as_mut().unwrap().root = worktree_root.clone();

        let mut critical = healthy.clone();
        critical.provider = Provider::CodexCli;
        critical.id = "recommended".into();
        critical.latest_model = Some("recommended-model".into());
        critical.context.tokens = 180_000;
        snapshot.sessions.push(critical);
        snapshot.attention = Some(AttentionEngine::default().update(&snapshot.sessions, 2_000));
        snapshot.recent_signals = vec![RecentSignal::Worktree {
            observed_at_ms: 2_000,
            worktree_root,
            change: SharedWorktreeRiskChange::Detected,
            before: vec![crate::attention::SessionIdentity {
                provider: Provider::ClaudeCode,
                native_id: "arbitrary-first".into(),
            }],
            after: vec![
                crate::attention::SessionIdentity {
                    provider: Provider::ClaudeCode,
                    native_id: "arbitrary-first".into(),
                },
                crate::attention::SessionIdentity {
                    provider: Provider::CodexCli,
                    native_id: "recommended".into(),
                },
            ],
        }];
        let mut ui = Ui::default();

        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let located = render_terminal_at(&ui, &snapshot, 120, 30);

        assert_eq!(ui.active_tab(), Tab::All);
        let cursor = now_cursor_style_at(&located, "ommended").unwrap();
        assert_eq!(cursor.fg, Some(Color::Rgb(230, 180, 80)));
        assert_eq!(now_cursor_style_at(&located, "ry-first"), None);
        assert_ne!(
            rendered_style_at(&located, "Risk detected").bg,
            Some(Color::Rgb(68, 61, 43))
        );
    }

    #[test]
    fn cleared_worktree_signal_stays_selected_when_the_worktree_is_no_longer_open() {
        let mut snapshot = demo_snapshot();
        let worktree_root = PathBuf::from("/wt/closed");
        snapshot.sessions[0].state = SessionState::Exited;
        snapshot.sessions[0].worktree.as_mut().unwrap().root = worktree_root.clone();
        snapshot.recent_signals = vec![RecentSignal::Worktree {
            observed_at_ms: 2_000,
            worktree_root,
            change: SharedWorktreeRiskChange::Cleared,
            before: vec![
                crate::attention::SessionIdentity {
                    provider: Provider::ClaudeCode,
                    native_id: "claude-a".into(),
                },
                crate::attention::SessionIdentity {
                    provider: Provider::CodexCli,
                    native_id: "codex-b".into(),
                },
            ],
            after: vec![],
        }];
        let selected_background = Some(Color::Rgb(68, 61, 43));
        let mut ui = Ui::default();

        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let unchanged = render_terminal_at(&ui, &snapshot, 120, 30);
        let page = rendered_rows_at(&ui, &snapshot, 120, 30);

        assert_eq!(ui.active_tab(), Tab::All);
        assert_eq!(
            rendered_style_at(&unchanged, "Risk cleared").bg,
            selected_background
        );
        assert!(page.contains("Worktree is no longer open"));
    }

    #[test]
    fn all_page_focus_moves_between_independently_selected_regions() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("first-now-model".into());
        let mut second = snapshot.sessions[0].clone();
        second.provider = Provider::CodexCli;
        second.id = "second-now".into();
        second.latest_model = Some("second-now-model".into());
        snapshot.sessions.push(second);
        snapshot.recent_signals = vec![
            RecentSignal::Session {
                observed_at_ms: 2_000,
                provider: Provider::ClaudeCode,
                session_id: "first-signal".into(),
                kind: RecentSignalKind::SessionStateChanged {
                    from: SessionState::Busy,
                    to: SessionState::Idle,
                },
            },
            RecentSignal::Session {
                observed_at_ms: 1_000,
                provider: Provider::CodexCli,
                session_id: "second-signal".into(),
                kind: RecentSignalKind::SessionStateChanged {
                    from: SessionState::Idle,
                    to: SessionState::Exited,
                },
            },
        ];
        let selected_background = Some(Color::Rgb(68, 61, 43));
        let mut ui = Ui::default();

        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let signals_focused = render_terminal_at(&ui, &snapshot, 120, 30);
        assert_eq!(
            rendered_style_at(&signals_focused, "d-signal").bg,
            selected_background
        );
        assert_eq!(now_cursor_style_at(&signals_focused, "cond-now"), None);

        ui.handle_key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &snapshot,
        )
        .unwrap();
        let now_focused = render_terminal_at(&ui, &snapshot, 120, 30);
        let cursor = now_cursor_style_at(&now_focused, "cond-now").unwrap();
        assert_eq!(cursor.fg, Some(Color::Rgb(230, 180, 80)));
        assert_ne!(
            rendered_style_at(&now_focused, "d-signal").bg,
            selected_background
        );
    }

    #[test]
    fn signal_detail_returns_to_the_signal_and_preserves_now_selection() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("first-now-model".into());
        let mut target = snapshot.sessions[0].clone();
        target.provider = Provider::CodexCli;
        target.id = "signal-detail-target".into();
        target.latest_model = Some("target-now-model".into());
        snapshot.sessions.push(target);
        snapshot.recent_signals = vec![
            RecentSignal::Session {
                observed_at_ms: 2_000,
                provider: Provider::ClaudeCode,
                session_id: "demo-session".into(),
                kind: RecentSignalKind::SessionStateChanged {
                    from: SessionState::Busy,
                    to: SessionState::Idle,
                },
            },
            RecentSignal::Session {
                observed_at_ms: 1_000,
                provider: Provider::CodexCli,
                session_id: "signal-detail-target".into(),
                kind: RecentSignalKind::SessionStateChanged {
                    from: SessionState::Idle,
                    to: SessionState::Exited,
                },
            },
        ];
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let selected_background = Some(Color::Rgb(68, 61, 43));
        let mut ui = Ui::default();

        ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Tab), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        let detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(detail.contains("signal-detail-target"));
        assert!(detail.contains("target-now-model"));
        assert!(!detail.contains("RECENT SIGNALS"));

        ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
        let restored_signal = render_terminal_at(&ui, &snapshot, 120, 30);
        assert_eq!(
            rendered_style_at(&restored_signal, "l-target").bg,
            selected_background
        );

        ui.handle_key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &snapshot,
        )
        .unwrap();
        let restored_now = render_terminal_at(&ui, &snapshot, 120, 30);
        let cursor = now_cursor_style_at(&restored_now, "l-target").unwrap();
        assert_eq!(cursor.fg, Some(Color::Rgb(230, 180, 80)));
    }

    #[test]
    fn compound_signal_keeps_compaction_and_context_clear_evidence_visible() {
        let mut snapshot = demo_snapshot();
        snapshot.recent_signals = vec![RecentSignal::Session {
            observed_at_ms: 2_000,
            provider: Provider::ClaudeCode,
            session_id: "s".into(),
            kind: RecentSignalKind::CompactionObserved {
                compaction: Compaction {
                    at_ms: 1_900,
                    trigger: Some("auto".into()),
                    pre_tokens: Some(180_000),
                },
                context_before: ContextState {
                    tokens: 180_000,
                    window: Some(200_000),
                },
                context_after: ContextState {
                    tokens: 80_000,
                    window: Some(200_000),
                },
                cleared_alert: Some(ContextAlertLevel::Critical),
            },
        }];

        let page = rendered_rows_at(&Ui::default(), &snapshot, 120, 30);
        let signal = page.lines().find(|line| line.contains("Cmp")).unwrap();

        for evidence in ["auto", "180.0k", "80.0k", "Critical"] {
            assert!(signal.contains(evidence), "missing evidence {evidence:?}");
        }
    }

    #[test]
    fn compact_page_yields_recent_signal_rows_before_reducing_now() {
        let mut snapshot = demo_snapshot();
        snapshot.recent_signals = (1..=7)
            .map(|position| RecentSignal::Session {
                observed_at_ms: 3_000 - position,
                provider: Provider::ClaudeCode,
                session_id: format!("signal-{position}"),
                kind: RecentSignalKind::SessionStateChanged {
                    from: SessionState::Busy,
                    to: SessionState::Idle,
                },
            })
            .collect();

        let page = rendered_rows_at(&Ui::default(), &snapshot, 80, 24);

        assert!(page.contains("Claude -session"));
        assert!(page.contains("claude-o..."));
        for visible in [
            "signal-1", "signal-2", "signal-3", "signal-4", "signal-5", "signal-6",
        ] {
            assert!(page.contains(visible));
        }
        assert!(!page.contains("signal-7"));
    }

    #[test]
    fn now_empty_state_explains_that_there_are_no_open_sessions() {
        let snapshot = Snapshot {
            ready: true,
            status: "watching".into(),
            ..Snapshot::default()
        };
        let ui = Ui::default();

        let page = rendered_rows_at(&ui, &snapshot, 80, 24);

        assert!(page.contains("No Open Sessions."));
        for manufactured_surface in ["Alert", "ATTENTION", "Warning", "Critical"] {
            assert!(!page.contains(manufactured_surface));
        }
    }

    #[test]
    fn now_keeps_healthy_busy_sessions_visible_without_manufacturing_attention() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].latest_model = Some("healthy-busy-model".into());
        snapshot.sessions[0].context.tokens = 80_000;
        let ui = Ui::default();

        let page = rendered_rows_at(&ui, &snapshot, 120, 30);
        let session = page
            .lines()
            .find(|line| line.contains("Claude -session"))
            .unwrap();

        assert!(session.contains("Busy"));
        assert!(session.contains("Claude"));
        assert!(session.contains("[#####-------] 40%"));
        assert!(session.contains("-session"));
        assert_eq!(page.matches("Claude -session").count(), 1);
        for manufactured_condition in ["Alert", "ATTENTION", "Warning", "Critical"] {
            assert!(!page.contains(manufactured_condition));
        }
    }

    #[test]
    fn compact_now_shortens_long_identity_before_operational_evidence() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].worktree = Some(Worktree {
            root: PathBuf::from("/Users/x/code/sessionmesh-worktrees/attention-layout-prototype"),
            branch: Some("prototype/q-007-responsive-terminal-layout".into()),
            writable: true,
        });
        snapshot.sessions[0].latest_model =
            Some("claude-sonnet-with-an-intentionally-long-model-name".into());
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        let ui = Ui::default();

        let page = rendered_rows_at(&ui, &snapshot, 80, 24);
        let worktree = page
            .lines()
            .find(|line| line.contains("attention-layout-prototype"))
            .unwrap();
        let session = page
            .lines()
            .find(|line| line.contains("Claude -session"))
            .unwrap();

        assert!(worktree.contains("prototype/q-007"));
        assert!(!worktree.contains("Context"));
        for evidence in [
            "WARNING",
            "Busy",
            "Claude",
            "-session",
            "[#########---] 75%",
            "claude-s...",
            "0s",
        ] {
            assert!(
                session.contains(evidence),
                "missing compact evidence: {evidence}"
            );
        }
    }

    #[test]
    fn changing_theme_does_not_change_rendered_product_semantics() {
        let sessionmesh_path = unused_profile_path("sessionmesh-semantics");
        let light_path = unused_profile_path("light-semantics");
        std::fs::write(
            &sessionmesh_path,
            br#"{"help_seen":true,"theme":"sessionmesh"}"#,
        )
        .unwrap();
        std::fs::write(&light_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
        let mut sessionmesh = Ui::load(&sessionmesh_path, None).unwrap();
        let mut light = Ui::load(&light_path, None).unwrap();
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.alerts = AlertCounts {
            warning: 3,
            critical: 2,
        };

        assert_eq!(
            rendered_with_snapshot(&sessionmesh, &snapshot),
            rendered_with_snapshot(&light, &snapshot)
        );

        sessionmesh.tab = Tab::Claude;
        light.tab = Tab::Claude;
        assert_eq!(
            rendered_with_snapshot(&sessionmesh, &snapshot),
            rendered_with_snapshot(&light, &snapshot)
        );

        let _ = std::fs::remove_file(sessionmesh_path);
        let _ = std::fs::remove_file(light_path);
    }

    #[test]
    fn provider_inventory_and_detail_use_theme_structural_roles() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut secondary = snapshot.sessions[0].clone();
        secondary.id = "secondary-session".into();
        secondary.project = Some(test_project("second"));
        secondary.latest_model = Some("model-two".into());
        secondary.state = SessionState::Exited;
        secondary.last_activity_ms = now - 1_000;
        snapshot.sessions.push(secondary);
        let profile_path = unused_profile_path("light-provider-roles");
        std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
        let mut ui = Ui::load(&profile_path, None).unwrap();
        ui.tab = Tab::Claude;

        let inventory = render_terminal_with_snapshot(&ui, &snapshot);
        assert_eq!(
            rendered_style_at(&inventory, "State").fg,
            Some(Color::Rgb(25, 91, 130))
        );
        assert_eq!(
            rendered_style_at(&inventory, "second").fg,
            Some(Color::Rgb(25, 91, 130))
        );
        assert_eq!(
            rendered_style_at(&inventory, "model-two").fg,
            Some(Color::Rgb(96, 101, 106))
        );
        assert_eq!(
            inventory.backend().buffer()[(0, 3)].style().fg,
            Some(Color::Rgb(126, 130, 133))
        );

        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let detail = render_terminal_with_snapshot(&ui, &snapshot);
        assert_eq!(
            rendered_style_at(&detail, "Attention Conditions").fg,
            Some(Color::Rgb(25, 91, 130))
        );
        assert_eq!(
            rendered_style_at(&detail, "Provider").fg,
            Some(Color::Rgb(96, 101, 106))
        );
        assert_eq!(
            rendered_style_at(&detail, "demo-session").fg,
            Some(Color::Rgb(28, 31, 34))
        );
        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn guide_uses_the_active_theme_roles() {
        let profile_path = unused_profile_path("themed-guide");
        let ui = Ui::load(&profile_path, Some("light")).unwrap();

        let terminal = render_terminal(&ui);

        assert_eq!(
            rendered_style_at(&terminal, "> Overview").bg,
            Some(Color::Rgb(20, 84, 116))
        );
        assert_eq!(
            rendered_style_at(&terminal, "Guide").fg,
            Some(Color::Rgb(25, 91, 130))
        );
        assert_eq!(
            rendered_style_at(&terminal, "Passive observation is the default.").fg,
            Some(Color::Rgb(28, 31, 34))
        );

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn footer_uses_theme_roles_and_exposes_the_global_guide() {
        let profile_path = unused_profile_path("light-footer");
        std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"light"}"#).unwrap();
        let mut ui = Ui::load(&profile_path, None).unwrap();
        ui.tab = Tab::Claude;
        let snapshot = Snapshot::default();
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &snapshot, &ui)).unwrap();

        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!rendered.contains("Active Today"));
        assert_eq!(
            rendered_style_at(&terminal, "? Guide").fg,
            Some(Color::Rgb(96, 101, 106))
        );
        assert!(!rendered.contains("t Theme"));
        assert!(!rendered.contains("Theme: light"));
        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn saved_theme_selection_is_restored_from_the_profile() {
        let profile_path = unused_profile_path("persisted-theme");
        std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"catppuccin"}"#).unwrap();

        let reloaded = Ui::load(&profile_path, None).unwrap();
        assert_eq!(reloaded.active_theme_name(), "catppuccin");

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn process_theme_override_takes_precedence_without_changing_the_profile() {
        let profile_path = unused_profile_path("theme-override");
        std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"catppuccin"}"#).unwrap();

        let overridden = Ui::load(&profile_path, Some("dracula")).unwrap();
        assert_eq!(overridden.active_theme_name(), "dracula");

        let reloaded = Ui::load(&profile_path, None).unwrap();
        assert_eq!(reloaded.active_theme_name(), "catppuccin");

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn unknown_process_theme_falls_back_without_changing_the_profile() {
        let profile_path = unused_profile_path("unknown-theme-override");
        std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"catppuccin"}"#).unwrap();

        let overridden = Ui::load(&profile_path, Some("neon")).unwrap();
        assert_eq!(overridden.active_theme_name(), "sessionmesh");

        let reloaded = Ui::load(&profile_path, None).unwrap();
        assert_eq!(reloaded.active_theme_name(), "catppuccin");

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn unknown_profile_theme_falls_back_to_sessionmesh_without_blocking_startup() {
        let profile_path = unused_profile_path("unknown-profile-theme");
        std::fs::write(&profile_path, br#"{"help_seen":true,"theme":"neon"}"#).unwrap();

        let ui = Ui::load(&profile_path, None).unwrap();

        assert_eq!(ui.active_theme_name(), "sessionmesh");
        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn first_use_profile_opens_guide_on_overview() {
        let ui = Ui::load(unused_profile_path("first-use"), None).unwrap();

        assert!(rendered(&ui).contains("> Overview"));
    }

    #[test]
    fn dismissing_first_use_guide_is_remembered_by_the_profile() {
        let profile_path = unused_profile_path("dismissed-help");
        let mut ui = Ui::load(&profile_path, None).unwrap();

        ui.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &Snapshot::default(),
        )
        .unwrap();

        let reloaded = Ui::load(&profile_path, None).unwrap();
        assert!(!rendered(&reloaded).contains("> Overview"));

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn deleting_help_preference_restores_first_use_guide() {
        let profile_path = unused_profile_path("deleted-help-preference");
        let mut ui = Ui::load(&profile_path, None).unwrap();
        ui.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &Snapshot::default(),
        )
        .unwrap();
        std::fs::remove_file(&profile_path).unwrap();

        let reloaded = Ui::load(&profile_path, None).unwrap();

        assert!(rendered(&reloaded).contains("> Overview"));
    }

    #[test]
    fn guide_can_be_reopened_and_closed_without_changing_the_active_page() {
        let profile_path = unused_profile_path("reopened-guide");
        let mut ui = Ui::load(&profile_path, None).unwrap();
        let snapshot = Snapshot::default();
        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let page_before_guide = rendered(&ui);

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        assert!(rendered(&ui).contains("> Overview"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert_eq!(rendered(&ui), page_before_guide);

        let _ = std::fs::remove_file(profile_path);
    }

    #[test]
    fn guide_prevents_page_navigation_from_changing_the_underlying_page() {
        let mut ui = Ui::load(unused_profile_path("numeric-pages"), None).unwrap();
        let snapshot = Snapshot::default();

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        assert_eq!(ui.active_tab(), Tab::All);

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        assert_eq!(ui.active_tab(), Tab::Claude);
    }

    #[test]
    fn tab_and_shift_tab_are_no_ops_on_a_single_region_page() {
        let mut ui = Ui::default();
        let snapshot = Snapshot::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        let page_before_focus_keys = rendered(&ui);

        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &snapshot,
        )
        .unwrap();

        assert_eq!(rendered(&ui), page_before_focus_keys);
    }

    #[test]
    fn persistent_navigation_is_numeric_and_highlights_the_active_page() {
        let snapshot = Snapshot::default();
        let mut ui = Ui::default();
        let terminal = render_terminal(&ui);
        let navigation_cells = &terminal.backend().buffer().content()[..80];
        let navigation: String = navigation_cells.iter().map(|cell| cell.symbol()).collect();

        assert_eq!(navigation.matches("1 2 3").count(), 1);
        for page_name in ["All", "Claude Code", "Codex CLI"] {
            assert!(!navigation.contains(page_name));
        }
        assert_eq!(
            rendered_style_at(&terminal, "1 2 3").bg,
            Some(Color::Rgb(68, 61, 43))
        );
        assert_ne!(
            rendered_style_at(&terminal, "2 3").bg,
            Some(Color::Rgb(68, 61, 43))
        );

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        let terminal = render_terminal(&ui);
        assert_eq!(
            rendered_style_at(&terminal, "2 3").bg,
            Some(Color::Rgb(68, 61, 43))
        );
    }

    #[test]
    fn guide_navigation_explains_page_mapping_and_essential_controls() {
        let mut ui = Ui::load(unused_profile_path("guide-content"), None).unwrap();
        ui.handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &Snapshot::default(),
        )
        .unwrap();
        let mut guide = rendered(&ui);
        ui.handle_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &Snapshot::default(),
        )
        .unwrap();
        for _ in 0..20 {
            ui.handle_key(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                &Snapshot::default(),
            )
            .unwrap();
            guide.push_str(&rendered(&ui));
        }

        for expected in [
            "1 / 2 / 3",
            "Tab / Shift+Tab",
            "Up/Down or j/k",
            "Enter",
            "View an indexed linked Successor",
            "Exact launched-process evidence",
            "Refresh",
            "?  Guide",
            "Esc",
            "Quit",
        ] {
            assert!(
                guide.contains(expected),
                "Navigation Guide should contain {expected:?}"
            );
        }
        assert!(!guide.contains("t Theme"));
    }

    #[test]
    fn provider_session_detail_remains_keyboard_accessible() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session detail"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Session detail"));
    }

    #[test]
    fn provider_page_groups_its_sessions_by_project_in_newest_project_order() {
        let now = Local::now().timestamp_millis();
        let mut snapshot = demo_snapshot();
        snapshot.generated_ms = now;
        snapshot.sessions[0].state = SessionState::Exited;
        snapshot.sessions[0].id = "newest-project-recent".into();
        snapshot.sessions[0].project = Some(test_project("newest-project"));
        snapshot.sessions[0].latest_model = Some("new-project-model".into());
        snapshot.sessions[0].last_activity_ms = now - 1_000;

        let mut older_in_same_project = snapshot.sessions[0].clone();
        older_in_same_project.id = "newest-project-second".into();
        older_in_same_project.latest_model = Some("same-project-model".into());
        older_in_same_project.last_activity_ms = now - 3_000;

        let mut older_project = snapshot.sessions[0].clone();
        older_project.id = "older-project-session".into();
        older_project.project = Some(test_project("older-project"));
        older_project.latest_model = Some("older-model".into());
        older_project.last_activity_ms = now - 2_000;

        let mut other_provider = snapshot.sessions[0].clone();
        other_provider.provider = Provider::CodexCli;
        other_provider.id = "codex-must-not-render".into();
        other_provider.project = Some(test_project("codex-project"));
        other_provider.latest_model = Some("codex-must-not-render".into());

        snapshot
            .sessions
            .extend([older_in_same_project, older_project, other_provider]);
        let ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };

        let page = rendered_rows_at(&ui, &snapshot, 120, 30);
        let newest_header = page.find("PROJECT  newest-project").unwrap();
        let older_header = page.find("PROJECT  older-project").unwrap();

        assert!(newest_header < older_header);
        assert_eq!(page.matches("PROJECT  newest-project").count(), 1);
        assert!(page.contains("new-project-model"));
        assert!(page.contains("same-project-model"));
        assert!(page.contains("older-model"));
        assert!(!page.contains("codex-must-not-render"));
    }

    #[test]
    fn provider_project_folds_recent_sessions_after_five_and_enter_expands() {
        let now = Local::now().timestamp_millis();
        let mut snapshot = demo_snapshot();
        snapshot.generated_ms = now;
        snapshot.sessions = (0..7)
            .map(|position| {
                let mut session = snapshot.sessions[0].clone();
                session.id = format!("recent-{position}");
                session.latest_model = Some(format!("recent-model-{position}"));
                session.last_activity_ms = now - position * 1_000;
                session
            })
            .collect();
        let mut ui = Ui {
            tab: Tab::Claude,
            selected: 5,
            ..Ui::default()
        };

        let collapsed = rendered_rows_at(&ui, &snapshot, 120, 30);
        for visible in 0..5 {
            assert!(collapsed.contains(&format!("recent-model-{visible}")));
        }
        assert!(!collapsed.contains("recent-model-5"));
        assert!(!collapsed.contains("recent-model-6"));
        assert!(collapsed.contains("+ MORE SESSIONS (2)"));

        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let expanded = rendered_rows_at(&ui, &snapshot, 120, 30);

        assert!(expanded.contains("recent-model-5"));
        assert!(expanded.contains("recent-model-6"));
        assert!(expanded.contains("- MORE SESSIONS (2)"));
    }

    #[test]
    fn provider_page_collapses_unavailable_cwds_and_hides_only_system_temp_sessions() {
        let now = Local::now().timestamp_millis();
        let mut snapshot = demo_snapshot();
        snapshot.generated_ms = now;
        snapshot.sessions[0].last_activity_ms = now;

        let mut real_private = snapshot.sessions[0].clone();
        real_private.id = "real-private".into();
        real_private.project = Some(test_project("real-private"));
        real_private.cwd = Some("/private/repos/real-project".into());
        real_private.latest_model = Some("real-private-model".into());
        real_private.state = SessionState::Exited;
        real_private.last_activity_ms = now - 1_000;

        let mut missing = snapshot.sessions[0].clone();
        missing.id = "missing-cwd".into();
        missing.project = None;
        missing.worktree = None;
        missing.cwd = Some("/deleted/worktree".into());
        missing.latest_model = Some("missing-cwd-model".into());
        missing.state = SessionState::Exited;
        missing.last_activity_ms = now - 2_000;

        let mut no_cwd = missing.clone();
        no_cwd.id = "no-cwd".into();
        no_cwd.cwd = None;
        no_cwd.latest_model = Some("no-cwd-model".into());
        no_cwd.last_activity_ms = now - 3_000;

        let mut system_temp = snapshot.sessions[0].clone();
        system_temp.id = "system-temp".into();
        system_temp.project = Some(test_project("system-temp"));
        system_temp.cwd = Some("/private/var/folders/zz/agent-workspace".into());
        system_temp.latest_model = Some("system-temp-model".into());
        system_temp.last_activity_ms = now - 4_000;

        snapshot
            .sessions
            .extend([real_private, missing, no_cwd, system_temp]);
        let mut ui = Ui {
            tab: Tab::Claude,
            selected: 2,
            ..Ui::default()
        };

        let collapsed = rendered_rows_at(&ui, &snapshot, 120, 30);
        assert!(collapsed.contains("real-private-model"));
        assert!(collapsed.contains("+ MISSING CWD (2)"));
        assert!(!collapsed.contains("missing-cwd-model"));
        assert!(!collapsed.contains("no-cwd-model"));
        assert!(!collapsed.contains("system-temp-model"));

        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let expanded = rendered_rows_at(&ui, &snapshot, 120, 30);

        assert!(expanded.contains("- MISSING CWD (2)"));
        assert!(expanded.contains("missing-cwd-model"));
        assert!(expanded.contains("no-cwd-model"));
        assert!(!expanded.contains("system-temp-model"));
    }

    #[test]
    fn provider_projects_partition_sessions_at_the_rolling_seven_day_cutoff() {
        const DAY_MS: i64 = 86_400_000;
        let now = 1_800_000_000_000;
        let cutoff = now - 7 * DAY_MS;
        let mut snapshot = demo_snapshot();
        snapshot.generated_ms = now;
        snapshot.sessions[0].state = SessionState::Exited;
        snapshot.sessions[0].id = "cutoff-session".into();
        snapshot.sessions[0].project = Some(test_project("mixed-project"));
        snapshot.sessions[0].latest_model = Some("cutoff-visible".into());
        snapshot.sessions[0].last_activity_ms = cutoff;

        let mut older = snapshot.sessions[0].clone();
        older.id = "older-hidden".into();
        older.latest_model = Some("older-hidden".into());
        older.last_activity_ms = cutoff - 1;

        let mut older_only = snapshot.sessions[0].clone();
        older_only.id = "older-only-hidden".into();
        older_only.project = Some(test_project("older-only-project"));
        older_only.latest_model = Some("older-only-hidden".into());
        older_only.last_activity_ms = cutoff - DAY_MS;

        snapshot.sessions.extend([older, older_only]);
        let ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };

        let page = rendered_rows_at(&ui, &snapshot, 120, 30);

        assert!(page.contains("PROJECT  mixed-project"));
        assert!(page.contains("cutoff-visible"));
        assert!(!page.contains("older-hidden"));
        assert!(page.contains("PROJECT  older-only-project"));
        assert!(!page.contains("older-only-hidden"));
        assert_eq!(page.matches("OLDER SESSIONS (1)").count(), 2);
    }

    #[test]
    fn provider_project_older_disclosures_expand_independently() {
        const DAY_MS: i64 = 86_400_000;
        let now = 1_800_000_000_000;
        let mut snapshot = demo_snapshot();
        snapshot.generated_ms = now;
        snapshot.sessions[0].state = SessionState::Exited;
        snapshot.sessions[0].id = "alpha-older".into();
        snapshot.sessions[0].project = Some(test_project("alpha"));
        snapshot.sessions[0].latest_model = Some("alpha-older-model".into());
        snapshot.sessions[0].last_activity_ms = now - 8 * DAY_MS;

        let mut beta = snapshot.sessions[0].clone();
        beta.id = "beta-older".into();
        beta.project = Some(test_project("beta"));
        beta.latest_model = Some("beta-older-model".into());
        beta.last_activity_ms = now - 9 * DAY_MS;
        snapshot.sessions.push(beta);

        let mut ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        let alpha_expanded = rendered_rows_at(&ui, &snapshot, 120, 30);
        assert!(alpha_expanded.contains("alpha-older-model"));
        assert!(!alpha_expanded.contains("beta-older-model"));

        ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        let both_expanded = rendered_rows_at(&ui, &snapshot, 120, 30);
        assert!(both_expanded.contains("alpha-older-model"));
        assert!(both_expanded.contains("beta-older-model"));

        ui.handle_key(key(KeyCode::Up), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Up), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        let only_beta_expanded = rendered_rows_at(&ui, &snapshot, 120, 30);
        assert!(!only_beta_expanded.contains("alpha-older-model"));
        assert!(only_beta_expanded.contains("beta-older-model"));
    }

    #[test]
    fn expanding_older_sessions_preserves_an_exact_partition_and_lifetime_usage() {
        const DAY_MS: i64 = 86_400_000;
        let now = 1_800_000_000_000;
        let mut snapshot = demo_snapshot();
        snapshot.generated_ms = now;
        snapshot.sessions[0].id = "recent-session".into();
        snapshot.sessions[0].project = Some(test_project("partition-project"));
        snapshot.sessions[0].latest_model = Some("recent-model".into());
        snapshot.sessions[0].last_activity_ms = now - DAY_MS;
        snapshot.sessions[0].usage = Usage {
            input: 11,
            output: 12,
            cache_read: 13,
            cache_write: 14,
        };

        let mut older_a = snapshot.sessions[0].clone();
        older_a.id = "older-a".into();
        older_a.latest_model = Some("older-a-model".into());
        older_a.last_activity_ms = now - 8 * DAY_MS;
        older_a.usage = Usage {
            input: 21,
            output: 22,
            cache_read: 23,
            cache_write: 24,
        };

        let mut older_b = snapshot.sessions[0].clone();
        older_b.id = "older-b".into();
        older_b.latest_model = Some("older-b-model".into());
        older_b.last_activity_ms = now - 9 * DAY_MS;
        older_b.usage = Usage {
            input: 31,
            output: 32,
            cache_read: 33,
            cache_write: 34,
        };
        snapshot.sessions.extend([older_a, older_b]);

        let mut ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let collapsed = rendered_rows_at(&ui, &snapshot, 160, 30);
        assert_eq!(collapsed.matches("recent-model").count(), 1);
        assert!(collapsed.contains("I 11  O 12  CR 13  CW 14"));
        assert_eq!(collapsed.matches("OLDER SESSIONS (2)").count(), 1);
        assert!(!collapsed.contains("older-a-model"));
        assert!(!collapsed.contains("older-b-model"));

        ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        let expanded = rendered_rows_at(&ui, &snapshot, 160, 30);

        for session in ["recent-model", "older-a-model", "older-b-model"] {
            assert_eq!(expanded.matches(session).count(), 1);
        }
        for usage in [
            "I 11  O 12  CR 13  CW 14",
            "I 21  O 22  CR 23  CW 24",
            "I 31  O 32  CR 33  CW 34",
        ] {
            assert_eq!(expanded.matches(usage).count(), 1);
        }
    }

    #[test]
    fn global_keys_return_actions_for_the_event_loop() {
        let mut ui = Ui::default();
        let snapshot = Snapshot::default();

        assert_eq!(
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
                &snapshot
            )
            .unwrap(),
            UiAction::Rescan
        );
        assert_eq!(
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                &snapshot
            )
            .unwrap(),
            UiAction::Quit
        );
        assert_eq!(
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &snapshot,
            )
            .unwrap(),
            UiAction::Quit
        );
    }

    #[test]
    fn provider_pages_have_no_time_filter_and_ignore_the_old_filter_key() {
        let mut ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
        let snapshot = Snapshot::default();
        let before = rendered_with_snapshot(&ui, &snapshot);

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        let after = rendered_with_snapshot(&ui, &snapshot);
        assert_eq!(after, before);
        for removed in [
            "Time",
            "Active Today",
            "Last 7 Days",
            "Last 30 Days",
            "All Sessions",
        ] {
            assert!(!after.contains(removed));
        }
    }

    #[test]
    fn provider_session_row_preserves_complete_session_facts_and_lifetime_usage() {
        let now = Local::now().timestamp_millis();
        let mut snapshot = demo_snapshot();
        snapshot.generated_ms = now;
        snapshot.sessions[0].last_activity_ms = now;
        snapshot.sessions[0].compactions = vec![
            Compaction {
                at_ms: now - 2_000,
                trigger: Some("auto".into()),
                pre_tokens: Some(180_000),
            },
            Compaction {
                at_ms: now - 1_000,
                trigger: Some("manual".into()),
                pre_tokens: Some(170_000),
            },
        ];
        let ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };

        let page = rendered_rows_at(&ui, &snapshot, 160, 30);

        for fact in [
            "PROJECT  sessionmesh",
            "-session",
            "Busy",
            "claude-opus-4-8",
            "150.0k (75%)",
            "I 1.0k  O 500  CR 200  CW 100",
            "$0.42",
            "0s",
            "⤳2",
        ] {
            assert!(page.contains(fact), "Provider row should contain {fact:?}");
        }
    }

    #[test]
    fn compact_provider_session_row_keeps_every_fact_visible() {
        let now = Local::now().timestamp_millis();
        let mut snapshot = demo_snapshot();
        snapshot.generated_ms = now;
        snapshot.sessions[0].last_activity_ms = now;
        snapshot.sessions[0].compactions = vec![Compaction {
            at_ms: now - 1_000,
            trigger: Some("auto".into()),
            pre_tokens: Some(180_000),
        }];
        let ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };

        let page = rendered_rows_at(&ui, &snapshot, 80, 24);

        for fact in [
            "PROJECT  sessionmesh",
            "-session",
            "Busy",
            "claude-opus-4-8",
            "150.0k (75%)",
            "I 1.0k  O 500  CR 200  CW 100",
            "$0.42",
            "0s",
            "⤳1",
        ] {
            assert!(
                page.contains(fact),
                "compact Provider row should contain {fact:?}"
            );
        }
    }

    #[test]
    fn old_open_session_remains_now_and_restores_its_provider_disclosure_after_detail() {
        const DAY_MS: i64 = 86_400_000;
        let now = Local::now().timestamp_millis();
        let mut snapshot = demo_snapshot();
        snapshot.generated_ms = now;
        snapshot.sessions[0].id = "old-open-session".into();
        snapshot.sessions[0].project = Some(test_project("old-open-project"));
        snapshot.sessions[0].latest_model = Some("old-open-model".into());
        snapshot.sessions[0].state = SessionState::Busy;
        snapshot.sessions[0].last_activity_ms = now - 8 * DAY_MS;
        let mut ui = Ui::default();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        assert!(rendered_rows_at(&ui, &snapshot, 80, 24).contains("Claude -session"));

        ui.handle_key(key(KeyCode::Char('2')), &snapshot).unwrap();
        let collapsed = rendered_rows_at(&ui, &snapshot, 80, 24);
        assert!(collapsed.contains("OLDER SESSIONS (1)"));
        assert!(!collapsed.contains("old-open-model"));

        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        ui.handle_key(key(KeyCode::Down), &snapshot).unwrap();
        assert!(rendered_rows_at(&ui, &snapshot, 80, 24).contains("old-open-model"));
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("old-open-session"));

        ui.handle_key(key(KeyCode::Esc), &snapshot).unwrap();
        let restored = rendered_rows_at(&ui, &snapshot, 80, 24);
        assert!(restored.contains("- OLDER SESSIONS (1)  Collapse"));
        assert!(restored.contains("old-open-model"));
        ui.handle_key(key(KeyCode::Enter), &snapshot).unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("old-open-session"));
    }

    #[test]
    fn moving_selection_changes_which_session_opens_in_detail() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut second = snapshot.sessions[0].clone();
        second.id = "chosen-second-session".into();
        second.project = Some(test_project("second-project"));
        second.last_activity_ms = now - 1_000;
        snapshot.sessions.push(second);
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();

        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        assert!(rendered_with_snapshot(&ui, &snapshot).contains("chosen-second-session"));
    }

    #[test]
    fn detail_keeps_provider_and_native_session_identity_when_rows_reorder() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut chosen = snapshot.sessions[0].clone();
        chosen.id = "stable-detail-target".into();
        chosen.project = Some(test_project("chosen-project"));
        chosen.state = SessionState::Exited;
        chosen.last_activity_ms = now - 1_000;
        snapshot.sessions.push(chosen);
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        snapshot.sessions[0].state = SessionState::Exited;
        snapshot.sessions[1].state = SessionState::Busy;
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(detail.contains("stable-detail-target"));
        assert!(!detail.contains("demo-session"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let reopened = rendered_with_snapshot(&ui, &snapshot);
        assert!(reopened.contains("stable-detail-target"));
        assert!(!reopened.contains("demo-session"));
    }

    #[test]
    fn detail_replaces_the_content_area_and_keeps_numeric_navigation_visible() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let terminal = render_terminal_with_snapshot(&ui, &snapshot);
        let buffer = terminal.backend().buffer();
        let navigation: String = buffer.content()[..80]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(navigation.contains("1 2 3"));
        let content_top: String = buffer.content()[80..80 * 2]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content_top.contains("Session detail"));
        assert_eq!(buffer[(0, 1)].symbol(), "┌");
        let footer: String = buffer.content()[80 * 23..80 * 24]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(footer.contains("Esc | j/k"));
        assert!(footer.contains("e Edit title"));
    }

    #[test]
    fn escape_returns_to_the_originating_page_with_its_selection_restored() {
        let mut snapshot = demo_snapshot();
        let now = Local::now().timestamp_millis();
        snapshot.sessions[0].last_activity_ms = now;
        let mut origin = snapshot.sessions[0].clone();
        origin.id = "origin-session".into();
        origin.project = Some(test_project("origin-project"));
        origin.last_activity_ms = now - 1_000;
        let mut third = snapshot.sessions[0].clone();
        third.id = "third-session".into();
        third.project = Some(test_project("third-project"));
        third.last_activity_ms = now - 2_000;
        snapshot.sessions.extend([origin, third]);
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        ui.handle_key(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);
        assert!(detail.contains("origin-session"));
        assert!(!detail.contains("third-session"));
    }

    #[test]
    fn number_keys_leave_detail_for_the_requested_top_level_page() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();

        for (key, expected_page) in [('1', Tab::All), ('2', Tab::Claude), ('3', Tab::Codex)] {
            let mut ui = Ui::default();
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
                .unwrap();
            assert!(rendered_with_snapshot(&ui, &snapshot).contains("Session detail"));

            ui.handle_key(
                KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();

            assert_eq!(ui.active_tab(), expected_page);
            assert!(!rendered_with_snapshot(&ui, &snapshot).contains("Session detail"));
        }
    }

    #[test]
    fn arrow_keys_scroll_detail_without_changing_the_origin_selection() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].id = format!("scroll-session-{}", "x".repeat(260));
        snapshot.sessions[0].project = Some(test_project(&"project".repeat(40)));
        snapshot.sessions[0].latest_model = Some("long-model".repeat(30));
        snapshot.sessions[0].compactions = (0..8)
            .map(|index| Compaction {
                at_ms: 1_700_000_000_000 + index * 1_000,
                trigger: Some(if index == 7 {
                    "scroll-target-trigger".into()
                } else {
                    format!("trigger-{index}")
                }),
                pre_tokens: Some(100_000 + index as u64),
            })
            .collect();
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        for _ in 0..100 {
            ui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &snapshot)
                .unwrap();
        }
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        for _ in 0..100 {
            ui.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &snapshot)
                .unwrap();
        }
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        for _ in 0..100 {
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
        }
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        for _ in 0..100 {
            ui.handle_key(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                &snapshot,
            )
            .unwrap();
        }
        assert!(!rendered_with_snapshot(&ui, &snapshot).contains("scroll-target-trigger"));

        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &snapshot)
            .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        assert!(rendered_with_snapshot(&ui, &snapshot).contains("scroll-session-"));
    }

    #[test]
    fn detail_keeps_required_session_evidence_without_recent_signals() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].compactions = vec![Compaction {
            at_ms: 1_700_000_000_000,
            trigger: Some("manual".into()),
            pre_tokens: Some(146_000),
        }];
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &snapshot, &ui)).unwrap();
        let detail: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        for expected in [
            "Provider",
            "Claude Code",
            "Project",
            "sessionmesh",
            "Worktree",
            "/Users/x/code/sessionmesh",
            "@ main",
            "cwd",
            "Title",
            "-session",
            "Session ID",
            "demo-session",
            "Model",
            "claude-opus-4-8",
            "Busy",
            "Context",
            "150.0k / 200.0k (75.0%)",
            "Transcript Lifetime Usage",
            "in 1.0k",
            "out 500",
            "cache read 200",
            "cache create 100",
            "API est. $0.42",
            "$0.42",
            "Compactions (1)",
            "manual",
            "146.0k",
        ] {
            assert!(
                detail.contains(expected),
                "Detail should contain {expected:?}"
            );
        }
        assert!(!detail.contains("RECENT SIGNALS"));
        assert!(!detail.contains("Recent Signals"));
        assert!(!detail.contains("Recent Signal"));
    }

    #[test]
    fn open_session_detail_labels_current_run_and_transcript_lifetime_usage() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].current_run_usage = Some(UsageSummary {
            usage: Usage {
                input: 111,
                output: 22,
                cache_read: 33,
                cache_write: 44,
            },
            per_model: vec![ModelUsage {
                model: "run-only-model".into(),
                usage: Usage {
                    input: 111,
                    output: 22,
                    cache_read: 33,
                    cache_write: 44,
                },
                cost: Some(0.07),
            }],
            cost: Some(0.07),
        });
        let mut ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_rows_at(&ui, &snapshot, 160, 44);

        for expected in [
            "Current Run Usage",
            "Current Run per-model Usage",
            "run-only-model",
            "$0.07",
            "Transcript Lifetime Usage",
            "Transcript Lifetime per-model Usage",
            "$0.42",
        ] {
            assert!(
                detail.contains(expected),
                "Detail should contain {expected:?}"
            );
        }
    }

    #[test]
    fn open_session_without_a_run_boundary_renders_current_run_usage_as_na() {
        let snapshot = demo_snapshot();
        let mut ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_rows_at(&ui, &snapshot, 160, 40);

        assert!(detail.contains("Current Run Usage"));
        assert!(detail.contains("Usage           N/A"));
        assert!(!detail.contains("Current Run per-model Usage"));
    }

    #[test]
    fn exited_session_detail_shows_lifetime_usage_without_current_or_last_run_usage() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].state = SessionState::Exited;
        snapshot.sessions[0].current_run_usage = Some(UsageSummary {
            usage: Usage {
                input: 111,
                ..Usage::default()
            },
            per_model: vec![],
            cost: None,
        });
        let mut ui = Ui {
            tab: Tab::Claude,
            ..Ui::default()
        };
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_rows_at(&ui, &snapshot, 120, 40);

        assert!(detail.contains("Transcript Lifetime Usage"));
        assert!(!detail.contains("Current Run Usage"));
        assert!(!detail.contains("Last Run Usage"));
    }

    #[test]
    fn non_git_detail_marks_git_only_fields_unavailable() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].project = Some(Project {
            identity: ProjectIdentity::CanonicalCwd(PathBuf::from(
                "/Users/x/Documents/research-notes",
            )),
            label: "research-notes".into(),
        });
        snapshot.sessions[0].worktree = None;
        snapshot.sessions[0].cwd = Some("/Users/x/Documents/research-notes".into());
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);

        assert!(detail.contains("research-notes"));
        assert!(detail.contains("Worktree      N/A"));
    }

    #[test]
    fn unresolved_detail_preserves_cwd_and_marks_identity_unavailable() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].project = None;
        snapshot.sessions[0].worktree = None;
        snapshot.sessions[0].cwd = Some("/missing/session-cwd".into());
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);

        assert!(detail.contains("Project       N/A"));
        assert!(detail.contains("/missing/session-cwd"));
    }

    #[test]
    fn detached_worktree_detail_marks_only_the_branch_unavailable() {
        let mut snapshot = demo_snapshot();
        snapshot.sessions[0].last_activity_ms = Local::now().timestamp_millis();
        snapshot.sessions[0].worktree.as_mut().unwrap().branch = None;
        let mut ui = Ui::default();
        ui.handle_key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &snapshot,
        )
        .unwrap();
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &snapshot)
            .unwrap();

        let detail = rendered_with_snapshot(&ui, &snapshot);

        assert!(detail.contains("@ detached HEAD"));
        assert!(detail.contains("/Users/x/code/sessionmesh"));
    }

    #[test]
    fn renders_all_tab() {
        render_tab(Tab::All, false);
    }

    #[test]
    fn renders_provider_tab_and_detail() {
        render_tab(Tab::Claude, false);
        render_tab(Tab::Claude, true);
    }
}
