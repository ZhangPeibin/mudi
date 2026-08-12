//! SessionMesh 终端界面的渲染与交互状态。
//!
//! 本模块使用 Ratatui 构建紧凑的总览界面。渲染过程只读取业务快照
//! [`Snapshot`] 和界面状态 [`Ui`]：前者提供会话、任务和信号等事实，后者保存
//! 选中项、弹窗与滚动位置。提供方特有字段由适配器负责归一化，
//! UI 自己只持久化 Profile 偏好与用户显式设置的会话元数据。

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
mod overview;
mod path_display;
mod project_tree;
mod provenance_mark;
mod provider_mark;
mod selected_session;
mod session_browser;
mod signals;
mod split_pane;
mod tasks;
mod text;
mod theme;

use crate::app::Snapshot;
use crate::attention::{AttentionEngine, AttentionSnapshot};
use crate::clipboard::Clipboard;
use crate::domain::{Provider, Session, SessionIdentity, SessionState};
use crate::managed_sessions::{
    ManagedLaunchAction, ManagedLaunchTarget, ManagedNewRequest, ManagedResumeRequest,
};
use crate::session_labels::SessionLabels;
use crate::signals::RecentSignal;
use crate::tasks::OperationTask;
use chrono::Local;
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

/// `Edit Session` 当前聚焦的元数据字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEditorField {
    Title,
    Description,
}

impl SessionEditorField {
    fn next(self) -> Self {
        match self {
            Self::Title => Self::Description,
            Self::Description => Self::Title,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }
}

/// 会话元数据编辑器的草稿、字段焦点和校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionEditor {
    session: SessionIdentity,
    title: String,
    description: String,
    field: SessionEditorField,
    error: Option<String>,
}

impl SessionEditor {
    fn active_input_mut(&mut self) -> &mut String {
        match self.field {
            SessionEditorField::Title => &mut self.title,
            SessionEditorField::Description => &mut self.description,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewSessionField {
    Provider,
    Target,
    Title,
    Description,
    BindingInitializer,
}

impl NewSessionField {
    const ALL: [Self; 5] = [
        Self::Provider,
        Self::Target,
        Self::Title,
        Self::Description,
        Self::BindingInitializer,
    ];

    fn move_by(self, offset: isize) -> Self {
        let index = Self::ALL
            .iter()
            .position(|field| *field == self)
            .unwrap_or(0) as isize;
        let next = (index + offset).rem_euclid(Self::ALL.len() as isize);
        Self::ALL[next as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NewSessionForm {
    targets: Vec<ManagedLaunchTarget>,
    target_index: usize,
    directory_context: bool,
    provider: Option<Provider>,
    title: String,
    description: String,
    binding_initializer: bool,
    field: NewSessionField,
    error: Option<String>,
}

impl NewSessionForm {
    fn new(
        targets: Vec<ManagedLaunchTarget>,
        target_index: usize,
        directory_context: bool,
    ) -> Self {
        Self {
            targets,
            target_index,
            directory_context,
            provider: None,
            title: String::new(),
            description: String::new(),
            binding_initializer: false,
            field: NewSessionField::Provider,
            error: None,
        }
    }

    fn active_input_mut(&mut self) -> Option<&mut String> {
        match self.field {
            NewSessionField::Title => Some(&mut self.title),
            NewSessionField::Description => Some(&mut self.description),
            _ => None,
        }
    }

    fn request(&self) -> Result<ManagedNewRequest, String> {
        let provider = self.provider.ok_or("Provider is required".to_string())?;
        if self.title.trim().is_empty() {
            return Err("Title is required".to_string());
        }
        let title = crate::session_labels::validate_alias(&self.title)
            .map_err(|error| error.to_string())?;
        let description = crate::session_labels::validate_description(&self.description)
            .map_err(|error| error.to_string())?;
        let target = self
            .targets
            .get(self.target_index)
            .cloned()
            .ok_or("Target is unavailable".to_string())?;
        Ok(ManagedNewRequest {
            provider,
            target,
            title,
            description,
            binding_initializer: self.binding_initializer,
        })
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
    /// 详情、展开分组及 Guide 的临时导航状态。
    detail: Option<DetailView>,
    hidden_tree_open: bool,
    hidden_tree: hidden_tree::State,
    guide_open: bool,
    guide_category: GuideCategory,
    guide_pane: GuidePane,
    guide_scroll: usize,
    // 文本换行取决于实际视口，所以绘制阶段会回写最近一次测得的最大滚动位置。
    guide_max_scroll: StateCell<usize>,
    /// 编辑器及所有需要显式确认的模态状态。
    new_session: Option<NewSessionForm>,
    edit_session: Option<SessionEditor>,
    handoff_confirmation: Option<HandoffConfirmation>,
    managed_resume_confirmation: Option<ManagedResumeConfirmation>,
    task_cancellation_confirmation: Option<TaskCancellationConfirmation>,
    task_deletion_confirmation: Option<TaskDeletionConfirmation>,
    task_retry_confirmation: Option<TaskRetryConfirmation>,
    hide_confirmation: Option<HideConfirmation>,
    inventory_delete_confirmation: Option<InventoryDeleteConfirmation>,
    /// Stable selection for the independent Managed Start status region.
    managed_launch_id: Option<String>,
    /// UI 自己拥有的 Session 元数据、Profile、主题和短期状态提示。
    labels: SessionLabels,
    labels_snapshot_revision: u64,
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
    ChooseManagedNewDirectory,
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
    Quit,
}

/// Declarative request emitted by the UI. The application controller owns its effects.
pub type UiIntent = UiAction;

impl Default for Ui {
    /// 构造首次启动时的默认界面状态。
    fn default() -> Self {
        Ui {
            project_tree: project_tree::State::default(),
            workspace_split: split_pane::State::default(),
            all_region: AllRegion::Now,
            signal_selected: 0,
            task_selected: 0,
            detail: None,
            hidden_tree_open: false,
            hidden_tree: hidden_tree::State::default(),
            guide_open: false,
            guide_category: GuideCategory::Overview,
            guide_pane: GuidePane::Categories,
            guide_scroll: 0,
            guide_max_scroll: StateCell::new(UNMEASURED_GUIDE_MAX_SCROLL),
            new_session: None,
            edit_session: None,
            handoff_confirmation: None,
            managed_resume_confirmation: None,
            task_cancellation_confirmation: None,
            task_deletion_confirmation: None,
            task_retry_confirmation: None,
            hide_confirmation: None,
            inventory_delete_confirmation: None,
            managed_launch_id: None,
            labels: SessionLabels::default(),
            labels_snapshot_revision: 0,
            profile_path: None,
            profile: ProfilePreferences::default(),
            theme: ThemeName::default(),
            status_notice: None,
        }
    }
}

impl Ui {
    /// 从本地 Profile 加载 UI 状态，并应用可选的进程级主题覆盖。
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
        let workspace_split = split_pane::State::new(profile.overview_left_width);
        Ok(Self {
            guide_open: !profile.help_seen,
            labels: SessionLabels::load(label_path),
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
    pub fn copy_to_clipboard(&mut self, clipboard: &mut dyn Clipboard, field: &str, value: &str) {
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

    pub fn finish_managed_new_directory_selection(
        &mut self,
        selection: Result<Option<ManagedLaunchTarget>, String>,
    ) {
        match selection {
            Ok(Some(target)) => {
                if let Some(form) = self
                    .new_session
                    .as_mut()
                    .filter(|form| form.directory_context)
                {
                    form.targets = vec![target];
                    form.target_index = 0;
                    form.error = None;
                } else {
                    self.new_session = Some(NewSessionForm::new(vec![target], 0, true));
                }
            }
            Ok(None) => {}
            Err(error) => {
                if let Some(form) = self
                    .new_session
                    .as_mut()
                    .filter(|form| form.directory_context)
                {
                    form.targets.clear();
                    form.target_index = 0;
                    form.error = Some(error);
                    form.field = NewSessionField::Target;
                } else {
                    let mut form = NewSessionForm::new(Vec::new(), 0, true);
                    form.field = NewSessionField::Target;
                    form.error = Some(error);
                    self.new_session = Some(form);
                }
            }
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
            && self.detail.is_none()
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
        if self.new_session.is_some() {
            return Ok(self.handle_new_session_key(key));
        }
        // Session 编辑器是最高优先级模态状态，打开时不会泄漏全局快捷键。
        if self.edit_session.is_some() {
            self.handle_session_editor_key(key);
            return Ok(UiAction::None);
        }
        if let Some(target) = self.managed_resume_confirmation.clone() {
            return match key.code {
                crossterm::event::KeyCode::Esc => {
                    self.managed_resume_confirmation = None;
                    Ok(UiAction::None)
                }
                crossterm::event::KeyCode::Enter => {
                    self.managed_resume_confirmation = None;
                    Ok(UiAction::StartManagedResume {
                        request: ManagedResumeRequest {
                            provider: target.provider,
                            native_session_id: target.native_session_id,
                        },
                    })
                }
                _ => Ok(UiAction::None),
            };
        }
        // 每一种确认框都只接受自身声明的按键，确保一次授权只对应一次操作。
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
        // Retry 和首次 Handoff 一样，需要重新选择 Successor Provider 并显式授权。
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
        if let Some(target) = self.hide_confirmation.clone() {
            return match key.code {
                crossterm::event::KeyCode::Esc => {
                    self.hide_confirmation = None;
                    Ok(UiAction::None)
                }
                crossterm::event::KeyCode::Enter => {
                    let requested = match &target {
                        HideConfirmation::Session { session, .. } => std::slice::from_ref(session),
                        HideConfirmation::Project { sessions, .. } => sessions.as_slice(),
                    };
                    let sessions = requested
                        .iter()
                        .filter(|identity| {
                            observed_snapshot.sessions.iter().any(|session| {
                                session.provider == identity.provider
                                    && session.id == identity.native_id
                                    && !session.state.is_open()
                            })
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let count = sessions.len();
                    self.hide_confirmation = None;
                    if sessions.is_empty() {
                        let became_open = requested.iter().any(|identity| {
                            observed_snapshot.sessions.iter().any(|session| {
                                session.provider == identity.provider
                                    && session.id == identity.native_id
                                    && session.state.is_open()
                            })
                        });
                        self.show_notice(if became_open {
                            "Open Session cannot be hidden"
                        } else {
                            "No Exited Sessions to hide"
                        });
                        return Ok(UiAction::None);
                    }
                    let result = self.labels.hide_sessions(&sessions);
                    match result {
                        Ok(()) => self.show_notice(format!(
                            "{count} Session{} hidden",
                            if count == 1 { "" } else { "s" }
                        )),
                        Err(error) => self.show_notice(format!("Hide failed: {error}")),
                    }
                    Ok(UiAction::None)
                }
                _ => Ok(UiAction::None),
            };
        }
        if let Some(target) = self.inventory_delete_confirmation.clone() {
            return match key.code {
                crossterm::event::KeyCode::Esc => {
                    self.inventory_delete_confirmation = None;
                    Ok(UiAction::None)
                }
                crossterm::event::KeyCode::Enter => {
                    let count = target.sessions.len();
                    let result = self.labels.permanently_exclude_sessions(&target.sessions);
                    self.inventory_delete_confirmation = None;
                    match result {
                        Ok(()) => {
                            if self.hidden_sessions(observed_snapshot).is_empty() {
                                self.hidden_tree_open = false;
                            } else {
                                self.hidden_tree.reconcile(observed_snapshot, &self.labels);
                            }
                            self.show_notice(format!(
                                "{count} Session{} permanently excluded",
                                if count == 1 { "" } else { "s" }
                            ));
                        }
                        Err(error) => self.show_notice(format!("Delete failed: {error}")),
                    }
                    Ok(UiAction::None)
                }
                _ => Ok(UiAction::None),
            };
        }
        // Retry 快捷键先建立确认状态，不会直接创建新的 Operation Task。
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
        // 退出键在 Guide 和详情之上保持全局可用。
        if key.code == crossterm::event::KeyCode::Char('q')
            || (key.code == crossterm::event::KeyCode::Char('c')
                && key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL))
        {
            return Ok(UiAction::Quit);
        }
        if self.hidden_tree_open {
            match key.code {
                crossterm::event::KeyCode::Esc => self.hidden_tree_open = false,
                crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                    self.hidden_tree
                        .move_vertical(observed_snapshot, &self.labels, true);
                }
                crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                    self.hidden_tree
                        .move_vertical(observed_snapshot, &self.labels, false);
                }
                crossterm::event::KeyCode::Enter => {
                    self.hidden_tree.activate(observed_snapshot, &self.labels);
                }
                crossterm::event::KeyCode::Char('u') => {
                    if let Some(sessions) = self
                        .hidden_tree
                        .selected_identities(observed_snapshot, &self.labels)
                    {
                        let count = sessions.len();
                        match self.labels.remove_from_hide(&sessions) {
                            Ok(()) => {
                                if self.hidden_sessions(observed_snapshot).is_empty() {
                                    self.hidden_tree_open = false;
                                } else {
                                    self.hidden_tree.reconcile(observed_snapshot, &self.labels);
                                }
                                self.show_notice(format!(
                                    "{count} Session{} restored",
                                    if count == 1 { "" } else { "s" }
                                ));
                            }
                            Err(error) => {
                                self.show_notice(format!("Restore failed: {error}"));
                            }
                        }
                    }
                }
                crossterm::event::KeyCode::Char('d') => {
                    if let Some(sessions) = self
                        .hidden_tree
                        .selected_identities(observed_snapshot, &self.labels)
                    {
                        self.inventory_delete_confirmation =
                            Some(InventoryDeleteConfirmation { sessions });
                    }
                }
                _ => {}
            }
            return Ok(UiAction::None);
        }
        // Guide 打开后拥有其余按键，防止帮助导航改变底层页面状态。
        if self.guide_open {
            self.handle_guide_key(key)?;
            return Ok(UiAction::None);
        }
        if let Some(action) = self.handle_managed_launch_key(key, snap) {
            return Ok(action);
        }
        // 下面按上下文分派普通页面和详情的导航动作。
        match key.code {
            crossterm::event::KeyCode::Char('n') => {
                if self.detail.is_none() {
                    if let Some(selection) = self.project_tree.new_target_selection(snap) {
                        self.new_session = Some(NewSessionForm::new(
                            selection.targets,
                            selection.selected,
                            false,
                        ));
                    } else {
                        let mut form = NewSessionForm::new(Vec::new(), 0, true);
                        form.field = NewSessionField::Target;
                        self.new_session = Some(form);
                    }
                }
            }
            crossterm::event::KeyCode::Char('r') if self.detail_managed_resume(snap).is_some() => {
                let resume = self.detail_managed_resume(snap).expect("checked above");
                self.managed_resume_confirmation = Some(ManagedResumeConfirmation {
                    provider: resume.provider,
                    native_session_id: resume.native_session_id.clone(),
                });
            }
            crossterm::event::KeyCode::Char('r') => {
                self.labels.reload();
                return Ok(UiAction::Rescan);
            }
            crossterm::event::KeyCode::Char('?') => {
                self.guide_category = self.guide_category_for_context();
                self.guide_pane = GuidePane::Categories;
                self.reset_guide_scroll();
                self.guide_open = true;
            }
            crossterm::event::KeyCode::Char('e') if self.labels.is_editable() => {
                self.open_session_editor(snap);
            }
            crossterm::event::KeyCode::Char('H')
                if self.detail.is_none() && !self.labels.is_editable() =>
            {
                self.show_notice("Curation unavailable while Label Store is degraded");
            }
            crossterm::event::KeyCode::Char('H')
                if self.detail.is_none() && self.all_region == AllRegion::Now =>
            {
                if let Some(session) = self.project_tree.selected_session(snap) {
                    if session.state.is_open() {
                        self.show_notice("Open Session cannot be hidden");
                    } else {
                        self.hide_confirmation = Some(HideConfirmation::Session {
                            session: SessionIdentity::from(session),
                            title: self.labels.title(session.provider, &session.id),
                        });
                    }
                } else if let Some(project_identity) = self.project_tree.selected_project(snap) {
                    let project_label = snap
                        .sessions
                        .iter()
                        .filter_map(|session| session.project.as_ref())
                        .find(|project| project.identity == project_identity)
                        .map(|project| project.label.clone())
                        .unwrap_or_else(|| project_identity.display_path().display().to_string());
                    let project_sessions = snap.sessions.iter().filter(|session| {
                        session
                            .project
                            .as_ref()
                            .is_some_and(|project| project.identity == project_identity)
                    });
                    let open_count = project_sessions
                        .clone()
                        .filter(|session| session.state.is_open())
                        .count();
                    let sessions = project_sessions
                        .filter(|session| !session.state.is_open())
                        .map(SessionIdentity::from)
                        .collect::<Vec<_>>();
                    if sessions.is_empty() {
                        self.show_notice("No Exited Sessions to hide");
                    } else {
                        self.hide_confirmation = Some(HideConfirmation::Project {
                            project_label,
                            sessions,
                            open_count,
                        });
                    }
                }
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
            crossterm::event::KeyCode::Char('h') if self.detail.is_none() => {
                if !self.labels.is_editable() {
                    self.show_notice("Hidden inventory unavailable: Label Store degraded");
                } else if self.hidden_sessions(observed_snapshot).is_empty() {
                    self.show_notice("No hidden sessions");
                } else {
                    self.hidden_tree.reconcile(observed_snapshot, &self.labels);
                    self.hidden_tree_open = true;
                }
            }
            crossterm::event::KeyCode::Char('o')
                if self.detail_managed_current_run(snap).is_some() =>
            {
                let run = self
                    .detail_managed_current_run(snap)
                    .expect("checked above");
                return Ok(UiAction::OpenManagedCurrentRunTerminal {
                    provider: run.provider,
                    native_session_id: run.native_session_id.clone(),
                    opaque_ref: run.opaque_ref.clone(),
                });
            }
            crossterm::event::KeyCode::Char('a')
                if self.detail_managed_current_run(snap).is_some() =>
            {
                let run = self
                    .detail_managed_current_run(snap)
                    .expect("checked above");
                return Ok(UiAction::CopyManagedCurrentRunAttachCommand {
                    provider: run.provider,
                    native_session_id: run.native_session_id.clone(),
                    opaque_ref: run.opaque_ref.clone(),
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
            crossterm::event::KeyCode::Tab if self.detail.is_none() => {
                self.all_region = match self.all_region {
                    AllRegion::Now => AllRegion::RecentSignals,
                    AllRegion::RecentSignals => AllRegion::Tasks,
                    AllRegion::Tasks => AllRegion::Now,
                };
            }
            crossterm::event::KeyCode::BackTab if self.detail.is_none() => {
                self.all_region = match self.all_region {
                    AllRegion::Now => AllRegion::Tasks,
                    AllRegion::RecentSignals => AllRegion::Now,
                    AllRegion::Tasks => AllRegion::RecentSignals,
                };
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail_session(snap).is_some() =>
            {
                detail::move_selected_field(self, observed_snapshot, true);
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail_session(snap).is_some() =>
            {
                detail::move_selected_field(self, observed_snapshot, false);
            }
            crossterm::event::KeyCode::Char('c') if self.detail_session(snap).is_some() => {
                if let Some((field, value)) = detail::selected_copy_request(observed_snapshot, self)
                {
                    return Ok(UiAction::Copy { field, value });
                }
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_none() && self.all_region == AllRegion::RecentSignals =>
            {
                if self.signal_selected + 1 < snap.recent_signals.len() {
                    self.signal_selected += 1;
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_none() && self.all_region == AllRegion::RecentSignals =>
            {
                self.signal_selected = self.signal_selected.saturating_sub(1);
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_none() && self.all_region == AllRegion::Tasks =>
            {
                if self.task_selected + 1 < snap.tasks.len() {
                    self.task_selected += 1;
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_none() && self.all_region == AllRegion::Tasks =>
            {
                self.task_selected = self.task_selected.saturating_sub(1);
            }
            crossterm::event::KeyCode::Right
                if self.detail.is_none() && self.all_region == AllRegion::Now =>
            {
                self.project_tree
                    .move_horizontal(snap, project_tree::HorizontalDirection::Right);
            }
            crossterm::event::KeyCode::Left
                if self.detail.is_none() && self.all_region == AllRegion::Now =>
            {
                self.project_tree
                    .move_horizontal(snap, project_tree::HorizontalDirection::Left);
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_none() && self.all_region == AllRegion::Now =>
            {
                self.project_tree
                    .move_vertical(snap, project_tree::VerticalDirection::Down);
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_none() && self.all_region == AllRegion::Now =>
            {
                self.project_tree
                    .move_vertical(snap, project_tree::VerticalDirection::Up);
            }
            crossterm::event::KeyCode::Enter
                if self.detail.is_none() && self.all_region == AllRegion::Tasks =>
            {
                if let Some(action) = self.open_selected_task(snap) {
                    return Ok(action);
                }
            }
            crossterm::event::KeyCode::Enter if self.detail.is_none() => {
                self.activate_selection(snap);
            }
            crossterm::event::KeyCode::Enter
                if detail::selected_opens_hidden_list(observed_snapshot, self) =>
            {
                self.hidden_tree.reconcile(observed_snapshot, &self.labels);
                self.hidden_tree_open = true;
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
                        scroll: StateCell::new(0),
                        selected_field: 0,
                        origin_signal: None,
                    });
                }
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail_task(snap).is_some() =>
            {
                if let Some(detail) = self.detail.as_ref() {
                    detail.scroll.set(detail.scroll.get().saturating_add(1));
                }
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail_task(snap).is_some() =>
            {
                if let Some(detail) = self.detail.as_ref() {
                    detail.scroll.set(detail.scroll.get().saturating_sub(1));
                }
            }
            crossterm::event::KeyCode::Esc => {
                self.close_detail(snap);
            }
            _ => {}
        }
        Ok(UiAction::None)
    }

    /// 处理 Guide 内部的主题切换、焦点移动和正文滚动。
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

    fn handle_new_session_key(&mut self, key: crossterm::event::KeyEvent) -> UiAction {
        let Some(form) = self.new_session.as_mut() else {
            return UiAction::None;
        };
        form.error = None;
        let is_text_field = matches!(
            form.field,
            NewSessionField::Title | NewSessionField::Description
        );
        match key.code {
            crossterm::event::KeyCode::Esc => self.new_session = None,
            crossterm::event::KeyCode::Up => form.field = form.field.move_by(-1),
            crossterm::event::KeyCode::Down => form.field = form.field.move_by(1),
            crossterm::event::KeyCode::Tab => form.field = form.field.move_by(1),
            crossterm::event::KeyCode::BackTab => form.field = form.field.move_by(-1),
            crossterm::event::KeyCode::Char('k') if !is_text_field => {
                form.field = form.field.move_by(-1)
            }
            crossterm::event::KeyCode::Char('j') if !is_text_field => {
                form.field = form.field.move_by(1)
            }
            crossterm::event::KeyCode::Char('n') if !is_text_field => self.new_session = None,
            crossterm::event::KeyCode::Char('y') if !is_text_field => match form.request() {
                Ok(request) => {
                    self.new_session = None;
                    return UiAction::StartManagedNew { request };
                }
                Err(error) => form.error = Some(error),
            },
            crossterm::event::KeyCode::Left | crossterm::event::KeyCode::Right
                if form.field == NewSessionField::Provider =>
            {
                form.provider = match key.code {
                    crossterm::event::KeyCode::Left => Some(Provider::ClaudeCode),
                    crossterm::event::KeyCode::Right => Some(Provider::CodexCli),
                    _ => unreachable!(),
                };
            }
            crossterm::event::KeyCode::Left
                if form.field == NewSessionField::Target && !form.targets.is_empty() =>
            {
                form.target_index = form
                    .target_index
                    .checked_sub(1)
                    .unwrap_or(form.targets.len() - 1);
            }
            crossterm::event::KeyCode::Right
                if form.field == NewSessionField::Target && !form.targets.is_empty() =>
            {
                form.target_index = (form.target_index + 1) % form.targets.len();
            }
            crossterm::event::KeyCode::Left
                if form.field == NewSessionField::BindingInitializer =>
            {
                form.binding_initializer = false;
            }
            crossterm::event::KeyCode::Right
                if form.field == NewSessionField::BindingInitializer =>
            {
                form.binding_initializer = true;
            }
            crossterm::event::KeyCode::Char(' ')
                if form.field == NewSessionField::BindingInitializer =>
            {
                form.binding_initializer = !form.binding_initializer;
            }
            crossterm::event::KeyCode::Backspace => {
                if let Some(input) = form.active_input_mut() {
                    input.pop();
                }
            }
            crossterm::event::KeyCode::Char(character)
                if !character.is_control()
                    && !key.modifiers.intersects(
                        crossterm::event::KeyModifiers::CONTROL
                            | crossterm::event::KeyModifiers::ALT,
                    ) =>
            {
                if let Some(input) = form.active_input_mut() {
                    input.push(character);
                }
            }
            crossterm::event::KeyCode::Enter
                if form.field == NewSessionField::Target && form.directory_context =>
            {
                return UiAction::ChooseManagedNewDirectory;
            }
            crossterm::event::KeyCode::Enter => form.field = form.field.move_by(1),
            _ => {}
        }
        UiAction::None
    }

    /// 切换 Guide 主题后重置滚动，并等待下一次绘制重新测量上限。
    fn reset_guide_scroll(&mut self) {
        self.guide_scroll = 0;
        self.guide_max_scroll.set(UNMEASURED_GUIDE_MAX_SCROLL);
    }

    /// 根据当前焦点选择最贴近上下文的 Guide 默认主题。
    fn guide_category_for_context(&self) -> GuideCategory {
        if self.detail.is_some() {
            GuideCategory::Sessions
        } else if self.all_region == AllRegion::RecentSignals {
            GuideCategory::Signals
        } else {
            GuideCategory::Overview
        }
    }

    /// 关闭详情页，并恢复来源区域及其稳定身份对应的选择位置。
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
        self.project_tree.select_session(identity);
        self.detail = None;
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
        if self.labels_snapshot_revision != snap.revision {
            self.labels.reload();
            self.labels_snapshot_revision = snap.revision;
        }
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
    }

    fn normal_snapshot(&self, snap: &Snapshot) -> Snapshot {
        let mut normal = snap.clone();
        let observed_session_count = normal.sessions.len();
        normal
            .sessions
            .retain(|session| self.session_is_normally_visible(session));
        let excluded_count = observed_session_count.saturating_sub(normal.sessions.len());
        normal.stats.discovered = normal.stats.discovered.saturating_sub(excluded_count);
        normal.recent_signals.retain(|signal| match signal {
            RecentSignal::Session {
                provider,
                session_id,
                ..
            } => match self.labels.disposition(*provider, session_id) {
                Some(crate::session_labels::CurationDisposition::Hidden) => snap
                    .sessions
                    .iter()
                    .find(|session| session.provider == *provider && session.id == *session_id)
                    .is_some_and(|session| session.state.is_open()),
                Some(crate::session_labels::CurationDisposition::PermanentlyExcluded) => false,
                None => true,
            },
            RecentSignal::Worktree { .. } => true,
        });
        normal
    }

    fn session_is_normally_visible(&self, session: &Session) -> bool {
        match self.labels.disposition(session.provider, &session.id) {
            Some(crate::session_labels::CurationDisposition::Hidden) => session.state.is_open(),
            Some(crate::session_labels::CurationDisposition::PermanentlyExcluded) => false,
            None => true,
        }
    }

    fn hidden_sessions<'a>(&self, snap: &'a Snapshot) -> Vec<&'a Session> {
        hidden_tree::sessions(snap, &self.labels)
    }

    fn selected_managed_launch<'a>(
        &self,
        snap: &'a Snapshot,
    ) -> Option<&'a crate::read_model::ManagedLaunchSummary> {
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
        if self.detail.is_some() || snap.managed_launches.is_empty() {
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
        let detail = self.detail.as_ref()?;
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
    ) -> Option<&'a crate::read_model::ManagedCurrentRunSummary> {
        let session = self.detail_session(snap)?;
        snap.verified_managed_current_runs
            .iter()
            .find(|run| run.is_verified_open_for(session))
    }

    fn detail_managed_resume<'a>(
        &self,
        snap: &'a Snapshot,
    ) -> Option<&'a crate::read_model::ManagedResumeSummary> {
        let session = self.detail_session(snap)?;
        snap.resumable_managed_sessions
            .iter()
            .find(|resume| resume.is_verified_exited_for(session))
    }

    /// 用任务 ID 从最新快照解析详情页 Operation Task。
    fn detail_task<'a>(&self, snap: &'a Snapshot) -> Option<&'a OperationTask> {
        let task_id = self.detail.as_ref()?.task_id.as_deref()?;
        snap.tasks.iter().find(|task| task.id == task_id)
    }

    /// 返回当前上下文中真正被选中的 Session。
    fn selected_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        if self.detail.is_some() {
            return self.detail_session(snap);
        }
        if self.all_region != AllRegion::Now {
            return None;
        }
        self.editor_session(snap)
    }

    /// Resolve the All-page editor target from Tree selection even while a status panel has focus.
    fn editor_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        if self.detail.is_some() {
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
        let title = self
            .labels
            .alias(session.provider, &session.id)
            .unwrap_or("")
            .to_string();
        let description = self
            .labels
            .description(session.provider, &session.id)
            .unwrap_or("")
            .to_string();
        self.edit_session = Some(SessionEditor {
            session: identity,
            title,
            description,
            field: SessionEditorField::Title,
            error: None,
        });
    }

    /// 处理 Session 编辑器的字段切换、输入、保存和取消。
    fn handle_session_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        match key.code {
            crossterm::event::KeyCode::Esc => self.edit_session = None,
            crossterm::event::KeyCode::Tab => {
                if let Some(editor) = self.edit_session.as_mut() {
                    editor.field = editor.field.next();
                    editor.error = None;
                }
            }
            crossterm::event::KeyCode::BackTab => {
                if let Some(editor) = self.edit_session.as_mut() {
                    editor.field = editor.field.previous();
                    editor.error = None;
                }
            }
            crossterm::event::KeyCode::Backspace => {
                if let Some(editor) = self.edit_session.as_mut() {
                    editor.active_input_mut().pop();
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
                if let Some(editor) = self.edit_session.as_mut() {
                    editor.active_input_mut().push(character);
                    editor.error = None;
                }
            }
            crossterm::event::KeyCode::Enter => {
                let Some(editor) = self.edit_session.as_ref() else {
                    return;
                };
                let provider = editor.session.provider;
                let native_id = editor.session.native_id.clone();
                let title = editor.title.clone();
                let description = editor.description.clone();
                let result = self
                    .labels
                    .set_metadata(provider, &native_id, &title, &description);
                match result {
                    Ok(()) => {
                        self.edit_session = None;
                        self.status_notice = Some((
                            "Session metadata saved".into(),
                            Instant::now() + Duration::from_secs(2),
                        ));
                    }
                    Err(error) => {
                        if let Some(editor) = self.edit_session.as_mut() {
                            editor.error = Some(error.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// 打开选中的 Task；未读终态任务同时请求显式记录 Acknowledgement。
    fn open_selected_task(&mut self, snap: &Snapshot) -> Option<UiAction> {
        let task = snap.tasks.get(self.task_selected)?;
        self.detail = Some(DetailView {
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
                self.detail = Some(DetailView {
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
                        self.detail = Some(DetailView {
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
                RecentSignal::Worktree { worktree_root, .. } => {
                    if let Some(session) =
                        self.project_tree
                            .open_sessions(snap)
                            .into_iter()
                            .find(|session| {
                                session
                                    .worktree
                                    .as_ref()
                                    .is_some_and(|worktree| worktree.root == *worktree_root)
                            })
                    {
                        self.all_region = AllRegion::Now;
                        self.project_tree
                            .select_session(SessionIdentity::from(session));
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
    if ui.hidden_tree_open {
        hidden_tree::draw(
            f,
            screen.content,
            observed_snapshot,
            &ui.hidden_tree,
            &ui.labels,
            theme,
        );
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
