//! Mudi 终端界面的渲染与交互状态。
//!
//! 本模块使用 Ratatui 构建紧凑的总览界面。渲染过程只读取业务快照
//! [`Snapshot`] 和界面状态 [`Ui`]：前者提供会话、任务和信号等事实，后者保存
//! 选中项、弹窗与滚动位置。提供方特有字段由适配器负责归一化，
//! UI 自己只持久化 Profile 偏好；Session Metadata 来自只读 Snapshot。

mod attention_summary;
mod chrome;
mod context;
mod detail;
mod editor;
mod field;
mod guide;
mod hidden_tree;
mod layout;
mod managed_start;
mod modals;
mod overlay;
mod overview;
mod path_display;
mod project_tree;
mod property_editor;
mod provenance_mark;
mod provider_mark;
mod router;
mod selected_session;
mod session_browser;
mod signals;
mod split_pane;
mod tasks;
mod text;
mod theme;

use crate::application::attention::{AttentionEngine, AttentionSnapshot};
use crate::application::clipboard::ClipboardPort;
use crate::application::managed_session::{
    ManagedLaunchAction, ManagedLaunchTarget, ManagedNewRequest, ManagedResumeRequest,
};
use crate::application::signals::RecentSignal;
use crate::application::tasks::OperationTask;
use crate::domain::{Provider, Session, SessionIdentity, SessionState};
use crate::presentation::app::Snapshot;
use chrono::Local;
use property_editor::{PropertyActivation, PropertyEditor, PropertyEditorEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::Cell as StateCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use theme::{Theme, ThemeName};

/// 支持完整界面的最小终端宽度。
const MIN_TERMINAL_WIDTH: u16 = 80;
/// 支持完整界面的最小终端高度。
const MIN_TERMINAL_HEIGHT: u16 = 24;
/// Guide 尚未渲染、因而无法测量最大滚动位置时使用的哨兵值。
const UNMEASURED_GUIDE_MAX_SCROLL: usize = usize::MAX;
const RECENT_SESSION_WINDOW_MS: i64 = 3 * 86_400_000;

/// 返回当前选中项的统一高亮样式。
fn selected_style(theme: Theme) -> Style {
    Style::default()
        .fg(theme.selected_foreground)
        .bg(theme.selected_background)
        .add_modifier(Modifier::BOLD)
}

/// Return the shared title treatment that distinguishes Overview keyboard focus.
fn region_title_style(theme: Theme, focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(theme.selected_foreground)
            .bg(theme.selected_background)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    }
}

/// Return the shared border treatment for a focusable Overview region.
fn region_border_style(theme: Theme, focused: bool) -> Style {
    Style::default().fg(if focused { theme.accent } else { theme.border })
}

/// Keep a region's selected row visible without replacing its semantic foreground roles.
fn retained_selection<'a>(
    mut line: Line<'a>,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Line<'a> {
    if !selected {
        return line;
    }

    line.push_span(" ".repeat(width.saturating_sub(line.width())));
    line.patch_style(Style::default().bg(theme.selected_background))
}

/// 创建 Handoff 前的确认弹窗状态。
#[derive(Debug, Clone, PartialEq, Eq)]
struct HandoffConfirmation {
    session: SessionIdentity,
    source_state: SessionState,
    successor_provider: Provider,
    end_current_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedResumeConfirmation {
    provider: Provider,
    native_session_id: String,
}

/// 取消非终态 Operation Task 前的确认状态。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskCancellationConfirmation {
    task_id: String,
}

/// 永久删除终态 Operation Task 前的确认状态。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskDeletionConfirmation {
    task_id: String,
}

/// 重试失败或中断任务前，重新收集授权所需的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskRetryConfirmation {
    predecessor_task_id: String,
    session: SessionIdentity,
    source_state: SessionState,
    successor_provider: Provider,
    end_current_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HideConfirmation {
    Session {
        session: SessionIdentity,
        title: String,
    },
    Project {
        project_label: String,
        sessions: Vec<SessionIdentity>,
        open_count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InventoryDeleteConfirmation {
    sessions: Vec<SessionIdentity>,
}

/// Session Detail 的来源、字段选择和滚动状态。
#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailView {
    session: SessionIdentity,
    task_id: Option<String>,
    scroll: StateCell<usize>,
    selected_field: usize,
    origin_signal: Option<RecentSignal>,
}

/// `Edit` 可浏览和编辑的 Session 元数据字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEditorField {
    Title,
    Description,
}

impl SessionEditorField {
    const ALL: [Self; 2] = [Self::Title, Self::Description];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|field| *field == self)
            .expect("Edit property belongs to the editor")
    }

    fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }
}

/// 会话元数据编辑器的草稿、字段焦点和校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionEditor {
    session: SessionIdentity,
    title: String,
    description: String,
    editor: PropertyEditor,
    error: Option<String>,
}

impl SessionEditor {
    fn field(&self) -> SessionEditorField {
        SessionEditorField::from_index(self.editor.focused())
    }

    fn activation(&self) -> PropertyActivation {
        match self.field() {
            SessionEditorField::Title => PropertyActivation::Text(self.title.clone()),
            SessionEditorField::Description => PropertyActivation::Text(self.description.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewSessionField {
    Provider,
    Location,
    Title,
    Description,
}

impl NewSessionField {
    const ALL: [Self; 4] = [
        Self::Provider,
        Self::Location,
        Self::Title,
        Self::Description,
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|field| *field == self)
            .expect("New property belongs to the editor")
    }

    fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NewSessionForm {
    targets: Vec<ManagedLaunchTarget>,
    target_index: usize,
    provider: Option<Provider>,
    title: String,
    description: String,
    editor: PropertyEditor,
    error: Option<String>,
}

#[derive(Debug)]
struct NewSessionValidationError {
    field: NewSessionField,
    message: String,
}

struct DirectoryBrowser {
    explorer: ratatui_explorer::FileExplorer,
}

impl DirectoryBrowser {
    fn new(start_directory: PathBuf, theme: theme::Theme) -> std::io::Result<Self> {
        let explorer_theme = ratatui_explorer::Theme::new()
            .with_block(
                ratatui::widgets::Block::default()
                    .borders(ratatui::widgets::Borders::ALL)
                    .border_style(Style::default().fg(theme.border))
                    .title(ratatui::text::Span::styled(
                        " Choose Directory ",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    )),
            )
            .add_default_title()
            .with_title_bottom(|_| {
                ratatui::text::Line::from(
                    " ↑/↓ or j/k Move · ←/→ or h/l Navigate · y Select · Esc Cancel ",
                )
                .centered()
            })
            .with_style(Style::default().fg(theme.foreground).bg(theme.background))
            .with_dir_style(Style::default().fg(theme.foreground))
            .with_highlight_dir_style(
                Style::default()
                    .fg(theme.selected_foreground)
                    .bg(theme.selected_background)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            )
            .with_highlight_symbol("› ")
            .with_scroll_padding(2);
        let mut explorer = ratatui_explorer::FileExplorer::with_theme(explorer_theme)?;
        explorer.set_cwd(start_directory)?;
        Ok(Self { explorer })
    }
}

impl NewSessionForm {
    fn new(targets: Vec<ManagedLaunchTarget>, target_index: usize) -> Self {
        let target_index = target_index.min(targets.len().saturating_sub(1));
        Self {
            targets,
            target_index,
            provider: None,
            title: String::new(),
            description: String::new(),
            editor: PropertyEditor::new(NewSessionField::ALL.len()),
            error: None,
        }
    }

    fn field(&self) -> NewSessionField {
        NewSessionField::from_index(self.editor.focused())
    }

    fn activation(&self) -> PropertyActivation {
        match self.field() {
            NewSessionField::Provider => PropertyActivation::Choice {
                option_count: 2,
                cursor: 0,
            },
            NewSessionField::Location => PropertyActivation::Choice {
                option_count: self.targets.len() + 1,
                cursor: if self.targets.is_empty() {
                    0
                } else {
                    self.target_index
                },
            },
            NewSessionField::Title => PropertyActivation::Text(self.title.clone()),
            NewSessionField::Description => PropertyActivation::Text(self.description.clone()),
        }
    }

    fn request(&self) -> Result<ManagedNewRequest, NewSessionValidationError> {
        let provider = self.provider.ok_or_else(|| NewSessionValidationError {
            field: NewSessionField::Provider,
            message: "Provider is required".to_string(),
        })?;
        if self.title.trim().is_empty() {
            return Err(NewSessionValidationError {
                field: NewSessionField::Title,
                message: "Title is required".to_string(),
            });
        }
        let title =
            crate::application::session_labels::validate_alias(&self.title).map_err(|error| {
                NewSessionValidationError {
                    field: NewSessionField::Title,
                    message: error.to_string(),
                }
            })?;
        let description = crate::application::session_labels::validate_description(
            &self.description,
        )
        .map_err(|error| NewSessionValidationError {
            field: NewSessionField::Description,
            message: error.to_string(),
        })?;
        let target = self
            .targets
            .get(self.target_index)
            .cloned()
            .ok_or_else(|| NewSessionValidationError {
                field: NewSessionField::Location,
                message: "Location is unavailable".to_string(),
            })?;
        Ok(ManagedNewRequest {
            provider,
            target,
            title,
            description,
            binding_initializer: true,
        })
    }

    fn apply_directory_target(&mut self, target: ManagedLaunchTarget) {
        if let Some(index) = self
            .targets
            .iter()
            .position(|known| known.working_directory == target.working_directory)
        {
            self.target_index = index;
        } else {
            self.targets.push(target);
            self.target_index = self.targets.len() - 1;
        }
        self.editor.focus(NewSessionField::Location.index());
        self.error = None;
    }
}

/// 总览页三个可独立聚焦的区域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllRegion {
    Now,
    RecentSignals,
    Tasks,
}

/// Guide 弹窗左侧的帮助主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuideCategory {
    Overview,
    Navigation,
    Sessions,
    Signals,
    Theme,
}

/// Guide 当前聚焦主题列表还是正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuidePane {
    Categories,
    Content,
}

impl GuideCategory {
    /// Guide 主题的固定显示顺序。
    const ALL: [Self; 5] = [
        Self::Overview,
        Self::Navigation,
        Self::Sessions,
        Self::Signals,
        Self::Theme,
    ];

    /// 返回主题在界面上的名称。
    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Navigation => "Navigation",
            Self::Sessions => "Sessions",
            Self::Signals => "Signals",
            Self::Theme => "Theme",
        }
    }

    /// 移到下一个主题，并停在列表末尾。
    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|category| *category == self)
            .unwrap_or(0);
        Self::ALL[(index + 1).min(Self::ALL.len() - 1)]
    }

    /// 移到上一个主题，并停在列表开头。
    fn previous(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|category| *category == self)
            .unwrap_or(0);
        Self::ALL[index.saturating_sub(1)]
    }
}

/// 由事件循环独占的交互状态。
///
/// 这里不保存业务事实；每次绘制和按键处理都会结合最新 [`Snapshot`] 解析
/// 当前身份，避免列表重排后把选择落到错误的会话或任务上。
pub struct Ui {
    /// All-page Project Tree owns its stable selection and disclosure state.
    project_tree: project_tree::State,
    /// Overview workspace split preference and transient drag state.
    workspace_split: split_pane::State,
    /// 总览页三个区域各自独立的焦点和选中位置。
    all_region: AllRegion,
    signal_selected: usize,
    task_selected: usize,
    /// One explicit state owns both overlay input priority and drawing order.
    overlay: overlay::OverlayState,
    /// Stable selection for the independent Managed Start status region.
    managed_launch_id: Option<String>,
    /// UI 自己拥有的 Profile、主题和短期状态提示。
    profile_path: Option<PathBuf>,
    profile: ProfilePreferences,
    theme: ThemeName,
    status_notice: Option<(String, Instant)>,
}

/// 持久化到本地 Profile 的轻量界面偏好。
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
struct ProfilePreferences {
    #[serde(default)]
    help_seen: bool,
    #[serde(default)]
    theme: ThemeName,
    #[serde(default)]
    overview_left_width: Option<u16>,
}

/// 按键处理交给外层事件循环执行的副作用请求。
///
/// UI 只描述用户明确触发的动作，不直接执行扫描、剪贴板写入、任务控制或退出。
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
    ResolveManagedNewDirectory {
        directory: PathBuf,
    },
    StartManagedNew {
        request: ManagedNewRequest,
    },
    StartManagedResume {
        request: ManagedResumeRequest,
    },
    CancelManagedStart {
        registration_id: String,
    },
    OpenManagedTerminal {
        registration_id: String,
    },
    RetryManagedTerminal {
        registration_id: String,
    },
    CopyManagedLaunchDiagnostics {
        registration_id: String,
    },
    OpenManagedCurrentRunTerminal {
        provider: Provider,
        native_session_id: String,
        opaque_ref: String,
    },
    CopyManagedCurrentRunAttachCommand {
        provider: Provider,
        native_session_id: String,
        opaque_ref: String,
    },
    SaveSessionMetadata {
        session: SessionIdentity,
        title: String,
        description: String,
    },
    HideSessions {
        sessions: Vec<SessionIdentity>,
    },
    RestoreSessions {
        sessions: Vec<SessionIdentity>,
    },
    PermanentlyExcludeSessions {
        sessions: Vec<SessionIdentity>,
    },
    Quit,
}

/// Declarative request emitted by the UI. The application controller owns its effects.
pub use router::route_action;

impl Default for Ui {
    /// 构造首次启动时的默认界面状态。
    fn default() -> Self {
        Ui {
            project_tree: project_tree::State::default(),
            workspace_split: split_pane::State::default(),
            all_region: AllRegion::Now,
            signal_selected: 0,
            task_selected: 0,
            overlay: overlay::OverlayState::None,
            managed_launch_id: None,
            profile_path: None,
            profile: ProfilePreferences::default(),
            theme: ThemeName::default(),
            status_notice: None,
        }
    }
}

impl Ui {
    #[cfg(test)]
    fn test_new_form(&self) -> Option<&NewSessionForm> {
        self.overlay.new_session().map(overlay::NewOverlay::form)
    }

    #[cfg(test)]
    fn test_directory_browser(&self) -> Option<&DirectoryBrowser> {
        match self.overlay.new_session()? {
            overlay::NewOverlay::Directory { browser, .. } => Some(browser),
            overlay::NewOverlay::Form(_) => None,
        }
    }

    #[cfg(test)]
    fn test_set_detail(&mut self, detail: DetailView) {
        self.overlay = overlay::OverlayState::Detail(detail);
    }

    #[cfg(test)]
    fn test_session_editor(&self) -> Option<&SessionEditor> {
        self.overlay.session_editor()
    }

    /// 从本地 Profile 加载 UI 状态。
    pub fn load(profile_path: impl AsRef<Path>) -> std::io::Result<Self> {
        let profile_path = profile_path.as_ref().to_path_buf();
        let profile = match fs::read(&profile_path) {
            Ok(contents) => serde_json::from_slice(&contents).map_err(std::io::Error::other)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ProfilePreferences::default()
            }
            Err(error) => return Err(error),
        };
        let theme = profile.theme;
        let workspace_split = split_pane::State::new(profile.overview_left_width);
        Ok(Self {
            overlay: if profile.help_seen {
                overlay::OverlayState::None
            } else {
                overlay::OverlayState::Guide(overlay::GuideOverlay::new(
                    GuideCategory::Overview,
                    overlay::OverlayState::None,
                ))
            },
            profile_path: Some(profile_path),
            profile,
            workspace_split,
            theme,
            ..Self::default()
        })
    }

    /// 返回当前主题的稳定名称。
    pub fn active_theme_name(&self) -> &'static str {
        self.theme.as_str()
    }

    /// 执行一次用户明确请求的剪贴板写入，并短暂显示结果。
    pub fn copy_to_clipboard(
        &mut self,
        clipboard: &mut dyn ClipboardPort,
        field: &str,
        value: &str,
    ) {
        let message = match clipboard.write_text(value) {
            Ok(()) => format!("Copied {field}"),
            Err(error) => format!("Copy failed: {error}"),
        };
        self.status_notice = Some((message, Instant::now() + Duration::from_secs(2)));
    }

    /// 显示一条四秒后自动失效的状态提示。
    pub fn show_notice(&mut self, message: impl Into<String>) {
        self.status_notice = Some((message.into(), Instant::now() + Duration::from_secs(4)));
    }

    fn push_confirmation(&mut self, confirmation: overlay::ConfirmationState) {
        let underlay = Box::new(std::mem::take(&mut self.overlay));
        self.overlay = overlay::OverlayState::Confirmation {
            underlay,
            confirmation,
        };
    }

    fn open_guide(&mut self, category: GuideCategory) {
        let underlay = std::mem::take(&mut self.overlay);
        self.overlay = overlay::OverlayState::Guide(overlay::GuideOverlay::new(category, underlay));
    }

    fn close_guide(&mut self) {
        let overlay = std::mem::take(&mut self.overlay);
        self.overlay = match overlay {
            overlay::OverlayState::Guide(guide) => *guide.underlay,
            other => other,
        };
    }

    fn close_session_editor(&mut self) {
        let overlay = std::mem::take(&mut self.overlay);
        self.overlay = match overlay {
            overlay::OverlayState::SessionEdit(editor) => *editor.underlay,
            other => other,
        };
    }

    fn open_hidden_inventory(&mut self, snap: &Snapshot) {
        let mut state = hidden_tree::State::default();
        state.reconcile(snap);
        let underlay = Box::new(std::mem::take(&mut self.overlay));
        self.overlay = overlay::OverlayState::HiddenInventory(overlay::HiddenInventoryOverlay {
            state,
            underlay,
        });
    }

    fn close_hidden_inventory(&mut self) {
        let overlay = std::mem::take(&mut self.overlay);
        self.overlay = match overlay {
            overlay::OverlayState::HiddenInventory(inventory) => *inventory.underlay,
            other => other,
        };
    }

    pub fn finish_managed_new_directory_selection(
        &mut self,
        selection: Result<Option<ManagedLaunchTarget>, String>,
    ) {
        let overlay = std::mem::take(&mut self.overlay);
        let mut form = match overlay {
            overlay::OverlayState::New(new) => match new {
                overlay::NewOverlay::Form(form) | overlay::NewOverlay::Directory { form, .. } => {
                    form
                }
            },
            other => {
                self.overlay = other;
                NewSessionForm::new(Vec::new(), 0)
            }
        };
        match selection {
            Ok(Some(target)) => form.apply_directory_target(target),
            Ok(None) => {}
            Err(error) => {
                form.editor.focus(NewSessionField::Location.index());
                form.error = Some(error);
            }
        }
        self.overlay = overlay::OverlayState::New(overlay::NewOverlay::Form(form));
    }

    pub fn finish_session_metadata_save(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.close_session_editor();
                self.status_notice = Some((
                    "Session metadata saved".into(),
                    Instant::now() + Duration::from_secs(2),
                ));
            }
            Err(error) => {
                if let Some(form) = self.overlay.session_editor_mut() {
                    form.error = Some(error);
                } else {
                    self.show_notice(format!("Save failed: {error}"));
                }
            }
        }
    }

    pub fn finish_hide_sessions(&mut self, result: Result<usize, String>) {
        match result {
            Ok(count) => {
                self.show_notice(format!(
                    "{count} Session{} hidden",
                    if count == 1 { "" } else { "s" }
                ));
            }
            Err(error) => self.show_notice(format!("Hide failed: {error}")),
        }
    }

    pub fn finish_restore_sessions(&mut self, result: Result<usize, String>) {
        match result {
            Ok(count) => {
                self.close_hidden_inventory();
                self.show_notice(format!(
                    "{count} Session{} restored",
                    if count == 1 { "" } else { "s" }
                ));
            }
            Err(error) => self.show_notice(format!("Restore failed: {error}")),
        }
    }

    pub fn finish_permanent_exclusion(&mut self, result: Result<usize, String>) {
        match result {
            Ok(count) => {
                self.close_hidden_inventory();
                self.show_notice(format!(
                    "{count} Session{} permanently excluded",
                    if count == 1 { "" } else { "s" }
                ));
            }
            Err(error) => self.show_notice(format!("Delete failed: {error}")),
        }
    }

    /// Route one terminal event through the public UI interaction boundary.
    pub fn handle_event(
        &mut self,
        event: crossterm::event::Event,
        terminal_area: ratatui::layout::Rect,
        snap: &Snapshot,
    ) -> std::io::Result<UiAction> {
        match event {
            crossterm::event::Event::Key(key)
                if key.kind == crossterm::event::KeyEventKind::Press =>
            {
                if key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    && matches!(
                        key.code,
                        crossterm::event::KeyCode::Left | crossterm::event::KeyCode::Right
                    )
                    && self.overview_split_is_interactive(terminal_area)
                {
                    let screen = layout::screen(terminal_area);
                    let workspace = overview::workspace_area(screen.content);
                    let direction = match key.code {
                        crossterm::event::KeyCode::Left => split_pane::ResizeDirection::Left,
                        crossterm::event::KeyCode::Right => split_pane::ResizeDirection::Right,
                        _ => unreachable!("guarded by the key-code match"),
                    };
                    self.workspace_split.adjust(workspace, direction);
                    self.save_workspace_split_preference()?;
                    return Ok(UiAction::None);
                }
                self.handle_key(key, snap)
            }
            crossterm::event::Event::Mouse(mouse) => {
                self.clamp_selections(snap);
                let normal_snapshot = self.normal_snapshot(snap);
                let snap = &normal_snapshot;
                if self.overview_split_is_interactive(terminal_area) {
                    let screen = layout::screen(terminal_area);
                    let workspace = overview::workspace_area(screen.content);
                    let project_scroll = match mouse.kind {
                        crossterm::event::MouseEventKind::ScrollUp => {
                            Some(project_tree::VerticalDirection::Up)
                        }
                        crossterm::event::MouseEventKind::ScrollDown => {
                            Some(project_tree::VerticalDirection::Down)
                        }
                        _ => None,
                    };
                    let pointer = ratatui::layout::Position::new(mouse.column, mouse.row);
                    if self.workspace_split.handle_mouse(mouse, workspace)
                        == split_pane::MouseOutcome::Committed
                    {
                        self.save_workspace_split_preference()?;
                    }
                    if let Some(direction) = project_scroll {
                        let left_pane = self.workspace_split.layout(workspace).left;
                        let project_rows =
                            session_browser::project_rows_area(left_pane, snap, self);
                        if !self.workspace_split.is_dragging() && project_rows.contains(pointer) {
                            self.all_region = AllRegion::Now;
                            self.project_tree.move_vertical(snap, direction);
                        }
                    }
                } else {
                    self.workspace_split.cancel_drag();
                }
                Ok(UiAction::None)
            }
            _ => Ok(UiAction::None),
        }
    }

    fn overview_split_is_interactive(&self, terminal_area: ratatui::layout::Rect) -> bool {
        terminal_area.width >= MIN_TERMINAL_WIDTH
            && terminal_area.height >= MIN_TERMINAL_HEIGHT
            && !modals::blocks_background_interaction(self)
    }

    fn save_workspace_split_preference(&mut self) -> std::io::Result<()> {
        self.profile.overview_left_width = self.workspace_split.preferred_left_width();
        self.save_profile()
    }

    /// 通过 UI 的公共交互入口处理一个按键。
    ///
    /// 模态界面按优先级先消费按键；未被弹窗、编辑器或 Guide 消费的按键才会
    /// 进入全局页签、列表导航和详情操作。返回值只声明外层需要执行的动作。
    pub fn handle_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        snap: &Snapshot,
    ) -> std::io::Result<UiAction> {
        let observed_snapshot = snap;
        // 快照可能在两次按键之间变化，先把所有选择位置收回到有效范围。
        self.clamp_selections(snap);
        let normal_snapshot = self.normal_snapshot(snap);
        let snap = &normal_snapshot;
        if let Some(action) = overlay::handle_modal_key(self, key, observed_snapshot)? {
            return Ok(action);
        }
        // 退出键在 Guide 和详情之上保持全局可用。
        if key.code == crossterm::event::KeyCode::Char('q')
            || (key.code == crossterm::event::KeyCode::Char('c')
                && key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL))
        {
            return Ok(UiAction::Quit);
        }
        if let Some(action) = hidden_tree::handle_key(self, key, observed_snapshot) {
            return Ok(action);
        }
        if guide::handle_key(self, key)? {
            return Ok(UiAction::None);
        }
        if let Some(action) = self.handle_managed_launch_key(key, snap) {
            return Ok(action);
        }
        if let Some(action) = detail::handle_key(self, key, observed_snapshot, snap)? {
            return Ok(action);
        }
        // 下面按上下文分派普通页面和详情的导航动作。
        match key.code {
            crossterm::event::KeyCode::Char('n') => {
                if self.overlay.detail().is_none() {
                    if let Some(selection) = self.project_tree.new_target_selection(snap) {
                        self.overlay = overlay::OverlayState::New(overlay::NewOverlay::Form(
                            NewSessionForm::new(selection.targets, selection.selected),
                        ));
                    } else {
                        self.overlay = overlay::OverlayState::New(overlay::NewOverlay::Form(
                            NewSessionForm::new(Vec::new(), 0),
                        ));
                    }
                }
            }
            crossterm::event::KeyCode::Char('r') => {
                return Ok(UiAction::Rescan);
            }
            crossterm::event::KeyCode::Char('?') => {
                self.open_guide(self.guide_category_for_context());
            }
            crossterm::event::KeyCode::Char('e')
                if self.selected_session(snap).is_some_and(|session| {
                    snap.session_metadata.allows_for(
                        session,
                        crate::application::observation::read_model::SessionMetadataAction::Edit,
                    )
                }) =>
            {
                self.open_session_editor(snap);
            }
            crossterm::event::KeyCode::Char('H')
                if self.overlay.detail().is_none() && !snap.session_metadata.is_editable() =>
            {
                self.show_notice("Curation unavailable while Label Store is degraded");
            }
            crossterm::event::KeyCode::Char('H')
                if self.overlay.detail().is_none() && self.all_region == AllRegion::Now =>
            {
                if let Some(session) = self.project_tree.selected_session(snap) {
                    if snap.session_metadata.allows(
                        session.provider,
                        &session.id,
                        crate::application::observation::read_model::SessionMetadataAction::Hide,
                    ) {
                        self.push_confirmation(overlay::ConfirmationState::Hide(
                            HideConfirmation::Session {
                                session: SessionIdentity::from(session),
                                title: snap.session_metadata.title(session.provider, &session.id),
                            },
                        ));
                    } else {
                        self.show_notice("Open Session cannot be hidden");
                    }
                } else if let Some(project_identity) = self.project_tree.selected_project(snap) {
                    let project_label = snap
                        .sessions
                        .iter()
                        .filter_map(|session| session.project.as_ref())
                        .find(|project| project.identity == project_identity)
                        .map(|project| project.label.clone())
                        .unwrap_or_else(|| project_identity.display_path().display().to_string());
                    let project_sessions = snap
                        .sessions
                        .iter()
                        .filter(|session| {
                            session
                                .project
                                .as_ref()
                                .is_some_and(|project| project.identity == project_identity)
                        })
                        .collect::<Vec<_>>();
                    let sessions = project_sessions
                        .iter()
                        .filter(|session| {
                            snap.session_metadata.allows_for(
                                session,
                                crate::application::observation::read_model::SessionMetadataAction::Hide,
                            )
                        })
                        .map(|session| SessionIdentity::from(*session))
                        .collect::<Vec<_>>();
                    let open_count = project_sessions.len().saturating_sub(sessions.len());
                    if sessions.is_empty() {
                        self.show_notice("No Exited Sessions to hide");
                    } else {
                        self.push_confirmation(overlay::ConfirmationState::Hide(
                            HideConfirmation::Project {
                                project_label,
                                sessions,
                                open_count,
                            },
                        ));
                    }
                }
            }
            crossterm::event::KeyCode::Char('h') if self.overlay.detail().is_none() => {
                if !snap.session_metadata.is_editable() {
                    self.show_notice("Hidden inventory unavailable: Label Store degraded");
                } else if self.hidden_sessions(observed_snapshot).is_empty() {
                    self.show_notice("No hidden sessions");
                } else {
                    self.open_hidden_inventory(observed_snapshot);
                }
            }
            crossterm::event::KeyCode::Tab if self.overlay.detail().is_none() => {
                self.all_region = match self.all_region {
                    AllRegion::Now => AllRegion::RecentSignals,
                    AllRegion::RecentSignals => AllRegion::Tasks,
                    AllRegion::Tasks => AllRegion::Now,
                };
            }
            crossterm::event::KeyCode::BackTab if self.overlay.detail().is_none() => {
                self.all_region = match self.all_region {
                    AllRegion::Now => AllRegion::Tasks,
                    AllRegion::RecentSignals => AllRegion::Now,
                    AllRegion::Tasks => AllRegion::RecentSignals,
                };
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.overlay.detail().is_none()
                    && self.all_region == AllRegion::RecentSignals =>
            {
                if self.signal_selected + 1 < snap.recent_signals.len() {
                    self.signal_selected += 1;
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.overlay.detail().is_none()
                    && self.all_region == AllRegion::RecentSignals =>
            {
                self.signal_selected = self.signal_selected.saturating_sub(1);
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.overlay.detail().is_none() && self.all_region == AllRegion::Tasks =>
            {
                if self.task_selected + 1 < snap.tasks.len() {
                    self.task_selected += 1;
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.overlay.detail().is_none() && self.all_region == AllRegion::Tasks =>
            {
                self.task_selected = self.task_selected.saturating_sub(1);
            }
            crossterm::event::KeyCode::Right
                if self.overlay.detail().is_none() && self.all_region == AllRegion::Now =>
            {
                self.project_tree
                    .move_horizontal(snap, project_tree::HorizontalDirection::Right);
            }
            crossterm::event::KeyCode::Left
                if self.overlay.detail().is_none() && self.all_region == AllRegion::Now =>
            {
                self.project_tree
                    .move_horizontal(snap, project_tree::HorizontalDirection::Left);
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.overlay.detail().is_none() && self.all_region == AllRegion::Now =>
            {
                self.project_tree
                    .move_vertical(snap, project_tree::VerticalDirection::Down);
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.overlay.detail().is_none() && self.all_region == AllRegion::Now =>
            {
                self.project_tree
                    .move_vertical(snap, project_tree::VerticalDirection::Up);
            }
            crossterm::event::KeyCode::Enter
                if self.overlay.detail().is_none() && self.all_region == AllRegion::Tasks =>
            {
                if let Some(action) = self.open_selected_task(snap) {
                    return Ok(action);
                }
            }
            crossterm::event::KeyCode::Enter if self.overlay.detail().is_none() => {
                self.activate_selection(snap);
            }
            _ => {}
        }
        Ok(UiAction::None)
    }

    /// 根据当前焦点选择最贴近上下文的 Guide 默认主题。
    fn guide_category_for_context(&self) -> GuideCategory {
        if self.overlay.detail().is_some() {
            GuideCategory::Sessions
        } else if self.all_region == AllRegion::RecentSignals {
            GuideCategory::Signals
        } else {
            GuideCategory::Overview
        }
    }

    /// 关闭详情页，并恢复来源区域及其稳定身份对应的选择位置。
    fn close_detail(&mut self, snap: &Snapshot) {
        let Some(detail) = self.overlay.detail().cloned() else {
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
            self.overlay = overlay::OverlayState::None;
            return;
        }
        if let Some(task_id) = detail.task_id {
            self.all_region = AllRegion::Tasks;
            if let Some(index) = snap.tasks.iter().position(|task| task.id == task_id) {
                self.task_selected = index;
            }
            self.overlay = overlay::OverlayState::None;
            return;
        }
        let identity = detail.session;
        self.project_tree.select_session(identity);
        self.overlay = overlay::OverlayState::None;
    }

    /// 以 JSON 保存当前 Profile 偏好；没有配置路径时视为无需持久化。
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

    /// 按实际渲染顺序返回当前页可选择的 Session。
    pub fn visible(&self, snap: &Snapshot) -> Vec<Session> {
        let normal_snapshot = self.normal_snapshot(snap);
        let snap = &normal_snapshot;
        self.project_tree
            .open_sessions(snap)
            .into_iter()
            .cloned()
            .collect()
    }

    /// 根据最新快照校正所有区域的独立选中位置。
    pub fn clamp_selections(&mut self, snap: &Snapshot) {
        let observed_snapshot = snap;
        let normal_snapshot = self.normal_snapshot(snap);
        let snap = &normal_snapshot;
        self.project_tree.reconcile(snap);
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
        if self
            .managed_launch_id
            .as_ref()
            .is_none_or(|id| !snap.managed_launches.iter().any(|launch| &launch.id == id))
        {
            self.managed_launch_id = snap
                .managed_launches
                .first()
                .map(|launch| launch.id.clone());
        }
        self.overlay.reconcile(observed_snapshot);
    }

    fn normal_snapshot(&self, snap: &Snapshot) -> Snapshot {
        snap.normal_inventory()
    }

    fn hidden_sessions<'a>(&self, snap: &'a Snapshot) -> Vec<&'a Session> {
        hidden_tree::sessions(snap)
    }

    fn selected_managed_launch<'a>(
        &self,
        snap: &'a Snapshot,
    ) -> Option<&'a crate::application::observation::read_model::ManagedLaunchSummary> {
        self.managed_launch_id
            .as_ref()
            .and_then(|id| snap.managed_launches.iter().find(|launch| &launch.id == id))
            .or_else(|| snap.managed_launches.first())
    }

    fn handle_managed_launch_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        snap: &Snapshot,
    ) -> Option<UiAction> {
        if self.overlay.detail().is_some() || snap.managed_launches.is_empty() {
            return None;
        }
        if matches!(key.code, crossterm::event::KeyCode::Char('[' | ']')) {
            let selected = self
                .selected_managed_launch(snap)
                .and_then(|launch| {
                    snap.managed_launches
                        .iter()
                        .position(|candidate| candidate.id == launch.id)
                })
                .unwrap_or(0);
            let next = match key.code {
                crossterm::event::KeyCode::Char('[') => selected
                    .checked_sub(1)
                    .unwrap_or(snap.managed_launches.len() - 1),
                crossterm::event::KeyCode::Char(']') => {
                    (selected + 1) % snap.managed_launches.len()
                }
                _ => unreachable!("guarded by Managed Start navigation keys"),
            };
            self.managed_launch_id = Some(snap.managed_launches[next].id.clone());
            return Some(UiAction::None);
        }

        let launch = self.selected_managed_launch(snap)?;
        let registration_id = launch.id.clone();
        match key.code {
            crossterm::event::KeyCode::Char('x')
                if launch
                    .available_actions
                    .contains(&ManagedLaunchAction::CancelStart) =>
            {
                Some(UiAction::CancelManagedStart { registration_id })
            }
            crossterm::event::KeyCode::Char('o')
                if launch
                    .available_actions
                    .contains(&ManagedLaunchAction::RetryOpenTerminal) =>
            {
                Some(UiAction::RetryManagedTerminal { registration_id })
            }
            crossterm::event::KeyCode::Char('o')
                if launch
                    .available_actions
                    .contains(&ManagedLaunchAction::OpenTerminal) =>
            {
                Some(UiAction::OpenManagedTerminal { registration_id })
            }
            crossterm::event::KeyCode::Char('c')
                if launch
                    .available_actions
                    .contains(&ManagedLaunchAction::CopyDiagnostics) =>
            {
                Some(UiAction::CopyManagedLaunchDiagnostics { registration_id })
            }
            crossterm::event::KeyCode::Char('x' | 'o' | 'c') => Some(UiAction::None),
            _ => None,
        }
    }

    /// 用稳定身份从最新快照解析详情页 Session。
    fn detail_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        let detail = self.overlay.detail()?;
        if detail.task_id.is_some() {
            return None;
        }
        let identity = &detail.session;
        snap.sessions.iter().find(|session| {
            session.provider == identity.provider && session.id == identity.native_id
        })
    }

    fn detail_managed_current_run<'a>(
        &self,
        snap: &'a Snapshot,
    ) -> Option<&'a crate::application::observation::read_model::ManagedCurrentRunSummary> {
        let session = self.detail_session(snap)?;
        snap.verified_managed_current_runs
            .iter()
            .find(|run| run.is_verified_open_for(session))
    }

    fn detail_managed_resume<'a>(
        &self,
        snap: &'a Snapshot,
    ) -> Option<&'a crate::application::observation::read_model::ManagedResumeSummary> {
        let session = self.detail_session(snap)?;
        snap.resumable_managed_sessions
            .iter()
            .find(|resume| resume.is_verified_exited_for(session))
    }

    /// 用任务 ID 从最新快照解析详情页 Operation Task。
    fn detail_task<'a>(&self, snap: &'a Snapshot) -> Option<&'a OperationTask> {
        let task_id = self.overlay.detail()?.task_id.as_deref()?;
        snap.tasks.iter().find(|task| task.id == task_id)
    }

    /// 返回当前上下文中真正被选中的 Session。
    fn selected_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        if self.overlay.detail().is_some() {
            return self.detail_session(snap);
        }
        if self.all_region != AllRegion::Now {
            return None;
        }
        self.editor_session(snap)
    }

    /// Resolve the All-page editor target from Tree selection even while a status panel has focus.
    fn editor_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        if self.overlay.detail().is_some() {
            return None;
        }
        self.project_tree.selected_session(snap)
    }

    /// 为当前 Session 打开元数据编辑器，并预填已有 Alias 和 Description。
    fn open_session_editor(&mut self, snap: &Snapshot) {
        let Some(session) = self.selected_session(snap) else {
            return;
        };
        let identity = SessionIdentity {
            provider: session.provider,
            native_id: session.id.clone(),
        };
        let title = snap
            .session_metadata
            .alias(session.provider, &session.id)
            .unwrap_or("")
            .to_string();
        let description = snap
            .session_metadata
            .description(session.provider, &session.id)
            .unwrap_or("")
            .to_string();
        let underlay = Box::new(std::mem::take(&mut self.overlay));
        self.overlay = overlay::OverlayState::SessionEdit(overlay::SessionEditOverlay {
            form: SessionEditor {
                session: identity,
                title,
                description,
                editor: PropertyEditor::new(SessionEditorField::ALL.len()),
                error: None,
            },
            underlay,
        });
    }

    /// 打开选中的 Task；未读终态任务同时请求显式记录 Acknowledgement。
    fn open_selected_task(&mut self, snap: &Snapshot) -> Option<UiAction> {
        let task = snap.tasks.get(self.task_selected)?;
        self.overlay = overlay::OverlayState::Detail(DetailView {
            session: SessionIdentity {
                provider: task.source_provider,
                native_id: task.source_session_id.clone(),
            },
            task_id: Some(task.id.clone()),
            scroll: StateCell::new(0),
            selected_field: 0,
            origin_signal: None,
        });
        (task.state.is_terminal() && task.acknowledged_at_ms.is_none() && !task.is_being_deleted())
            .then(|| UiAction::AcknowledgeTask {
                task_id: task.id.clone(),
            })
    }

    /// 激活当前列表项：进入 Session 或 Signal 详情。
    fn activate_selection(&mut self, snap: &Snapshot) {
        if self.all_region == AllRegion::Now {
            if let Some(session) = self.project_tree.activate(snap) {
                self.overlay = overlay::OverlayState::Detail(DetailView {
                    session: SessionIdentity {
                        provider: session.provider,
                        native_id: session.id.clone(),
                    },
                    task_id: None,
                    scroll: StateCell::new(0),
                    selected_field: 0,
                    origin_signal: None,
                });
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
                        self.overlay = overlay::OverlayState::Detail(DetailView {
                            session: SessionIdentity {
                                provider: *provider,
                                native_id: session_id.clone(),
                            },
                            task_id: None,
                            scroll: StateCell::new(0),
                            selected_field: 0,
                            origin_signal: Some(signal.clone()),
                        });
                    }
                }
                RecentSignal::Project { project_path, .. } => {
                    if let Some(session) =
                        self.project_tree
                            .open_sessions(snap)
                            .into_iter()
                            .find(|session| {
                                session.project.as_ref().is_some_and(|project| {
                                    project.identity.display_path() == project_path
                                })
                            })
                    {
                        self.all_region = AllRegion::Now;
                        self.project_tree
                            .select_session(SessionIdentity::from(session));
                        self.overlay = overlay::OverlayState::None;
                        self.status_notice = None;
                    } else {
                        self.status_notice = Some((
                            "Project is no longer open".into(),
                            Instant::now() + Duration::from_secs(2),
                        ));
                    }
                }
            }
        }
    }
}

/// 绘制完整终端画面。
///
/// 先处理最小尺寸保护，再划分页头、主内容和页脚；Session Detail、Guide、
/// 编辑器和确认框按照模态优先级覆盖在原始页面之上。
pub fn draw(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let observed_snapshot = snap;
    let normal_snapshot = ui.normal_snapshot(snap);
    let snap = &normal_snapshot;
    let theme = ui.theme.palette();
    f.render_widget(
        Block::default().style(Style::default().fg(theme.foreground).bg(theme.background)),
        f.area(),
    );
    if f.area().width < MIN_TERMINAL_WIDTH || f.area().height < MIN_TERMINAL_HEIGHT {
        modals::draw_terminal_too_small(f, theme);
        return;
    }
    // 顶层始终保留一行页头和一行页脚，中间区域交给当前页面或 Task Detail。
    let screen = layout::screen(f.area());

    chrome::draw_header(f, screen.header, snap, ui);

    // Task Detail stays a full page. Session Detail is drawn later as a modal so its
    // origin remains visible and can be restored without reconstructing it.
    if let Some(hidden) = ui.overlay.hidden_inventory() {
        hidden_tree::draw(f, screen.content, observed_snapshot, hidden, theme);
    } else if let Some(task) = ui.detail_task(snap) {
        tasks::draw_detail(f, screen.content, task, snap, ui);
    } else {
        overview::draw(f, screen.content, snap, ui);
    }

    chrome::draw_footer(f, screen.footer, snap, ui);

    modals::draw_overlays(f, observed_snapshot, ui);
}

/// 读取快照中的 Attention 结果；旧快照缺失时就地重算。
fn current_attention(snap: &Snapshot) -> Cow<'_, AttentionSnapshot> {
    match snap.attention.as_ref() {
        Some(attention) => Cow::Borrowed(attention),
        None => {
            Cow::Owned(AttentionEngine::default().update(&snap.sessions, snapshot_now_ms(snap)))
        }
    }
}

/// 返回快照的观测时间；测试或空快照缺失时回退到本地当前时间。
fn snapshot_now_ms(snap: &Snapshot) -> i64 {
    if snap.generated_ms > 0 {
        snap.generated_ms
    } else {
        Local::now().timestamp_millis()
    }
}

/// System temporary directories are not part of the Session inventory.
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

#[cfg(test)]
#[path = "ui_test.rs"]
mod tests;
