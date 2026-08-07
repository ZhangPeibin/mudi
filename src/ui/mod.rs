//! SessionMesh 终端界面的渲染与交互状态。
//!
//! 本模块使用 Ratatui 构建紧凑的三页式界面。渲染过程只读取业务快照
//! [`Snapshot`] 和界面状态 [`Ui`]：前者提供会话、任务和信号等事实，后者保存
//! 当前页签、选中项、弹窗与滚动位置。提供方特有字段由适配器负责归一化，
//! UI 自己只持久化 Profile 偏好与用户显式设置的会话标题。

mod chrome;
mod context;
mod detail;
mod editor;
mod guide;
mod layout;
mod modals;
mod overview;
mod projects;
mod provider;
mod selected_session;
mod session_browser;
mod signals;
mod tasks;
mod text;
mod theme;

use crate::app::{Snapshot, Tab};
use crate::attention::{AttentionEngine, AttentionSnapshot};
use crate::clipboard::Clipboard;
use crate::domain::{Project, ProjectIdentity, Provider, Session, SessionState};
use crate::session_labels::SessionLabels;
use crate::signals::RecentSignal;
use crate::tasks::OperationTask;
use chrono::Local;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Block;
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::Cell as StateCell;
use std::collections::HashSet;
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

/// 返回当前选中项的统一高亮样式。
fn selected_style(theme: Theme) -> Style {
    Style::default()
        .fg(theme.selected_foreground)
        .bg(theme.selected_background)
        .add_modifier(Modifier::BOLD)
}

/// 用“提供方 + 原生会话 ID”稳定标识一个会话。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionIdentity {
    provider: Provider,
    native_id: String,
}

/// Session Tree 中 Exited Session 的滚动时间分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SessionAgeBand {
    Recent,
    Older,
}

/// Project Tree 顶层的非 Project 分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ProjectTreeSection {
    OlderProjects,
    Unavailable,
}

/// All 页面 Session Tree 中一个可见节点的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ProjectTreeKey {
    Section(ProjectTreeSection),
    Project(ProjectIdentity),
    Worktree {
        project: ProjectIdentity,
        root: PathBuf,
    },
    Disclosure {
        project: Option<ProjectIdentity>,
        worktree_root: Option<PathBuf>,
        band: SessionAgeBand,
    },
    Session(SessionIdentity),
}

impl ProjectTreeKey {
    /// 历史分组默认收起；Project、Worktree 与 Unavailable 默认展开。
    fn uses_disclosure_state(&self) -> bool {
        matches!(
            self,
            Self::Disclosure { .. } | Self::Section(ProjectTreeSection::OlderProjects)
        )
    }
}

/// 创建 Handoff 前的确认弹窗状态。
#[derive(Debug, Clone, PartialEq, Eq)]
struct HandoffConfirmation {
    session: SessionIdentity,
    source_state: SessionState,
    successor_provider: Provider,
    end_current_run: bool,
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

/// 详情页的来源、滚动位置及其子菜单状态。
#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailView {
    session: SessionIdentity,
    task_id: Option<String>,
    scroll: usize,
    origin_signal: Option<RecentSignal>,
    copy_menu_selected: Option<usize>,
}

/// 会话标题编辑器的草稿和校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TitleEditor {
    session: SessionIdentity,
    input: String,
    error: Option<String>,
}

/// 会话详情中允许显式复制的本地元数据字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyField {
    SourceLog,
    Worktree,
    Cwd,
    SessionId,
}

impl CopyField {
    /// 复制菜单中的固定字段顺序。
    const ALL: [Self; 4] = [Self::SourceLog, Self::Worktree, Self::Cwd, Self::SessionId];

    /// 返回字段在界面上的英文标签。
    fn label(self) -> &'static str {
        match self {
            Self::SourceLog => "Source log",
            Self::Worktree => "Worktree",
            Self::Cwd => "cwd",
            Self::SessionId => "Session ID",
        }
    }

    /// 从会话中提取字段值；不可用字段返回 `None`，菜单会跳过它。
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

/// 提供方页面中一个 Project 分组的稳定键。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderProjectKey {
    provider: Provider,
    project: Option<ProjectIdentity>,
}

/// 可展开分组的身份；用于跨重绘保留展开状态。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ProviderDisclosureKey {
    MoreRecent(ProviderProjectKey),
    Older(ProviderProjectKey),
    UnavailableCwd(Provider),
}

/// 可展开行的展示类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderDisclosureKind {
    MoreRecent,
    Older,
    UnavailableCwd,
}

impl ProviderDisclosureKind {
    /// 返回展开行的界面标签。
    fn label(self) -> &'static str {
        match self {
            Self::MoreRecent => "MORE SESSIONS",
            Self::Older => "OLDER SESSIONS",
            Self::UnavailableCwd => "MISSING CWD",
        }
    }
}

/// 提供方页面当前选中行可执行的目标。
#[derive(Clone)]
enum ProviderTarget<'a> {
    Session(&'a Session),
    Disclosure(ProviderDisclosureKey),
}

/// 提供方页面扁平化后的渲染行。
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
    /// 将纯展示行过滤掉，只保留可进入详情或可展开的目标。
    fn into_target(self) -> Option<ProviderTarget<'a>> {
        match self {
            ProviderPageRow::Project(_) => None,
            ProviderPageRow::Session(session) => Some(ProviderTarget::Session(session)),
            ProviderPageRow::Disclosure { key, .. } => Some(ProviderTarget::Disclosure(key)),
        }
    }
}

/// 由事件循环独占的交互状态。
///
/// 这里不保存业务事实；每次绘制和按键处理都会结合最新 [`Snapshot`] 解析
/// 当前身份，避免列表重排后把选择落到错误的会话或任务上。
pub struct Ui {
    /// 当前顶层页签与 Provider 页面选中位置。
    pub tab: Tab,
    pub selected: usize,
    /// All 页面 Session Tree 使用稳定节点身份，不依赖 Attention 排序位置。
    project_tree_selection: Option<ProjectTreeKey>,
    collapsed_project_tree_nodes: HashSet<ProjectTreeKey>,
    expanded_project_tree_disclosures: HashSet<ProjectTreeKey>,
    /// 总览页三个区域各自独立的焦点和选中位置。
    all_region: AllRegion,
    signal_selected: usize,
    task_selected: usize,
    /// 详情、展开分组及 Guide 的临时导航状态。
    detail: Option<DetailView>,
    expanded_provider_disclosures: HashSet<ProviderDisclosureKey>,
    guide_open: bool,
    guide_category: GuideCategory,
    guide_pane: GuidePane,
    guide_scroll: usize,
    // 文本换行取决于实际视口，所以绘制阶段会回写最近一次测得的最大滚动位置。
    guide_max_scroll: StateCell<usize>,
    /// 编辑器及所有需要显式确认的模态状态。
    edit_title: Option<TitleEditor>,
    handoff_confirmation: Option<HandoffConfirmation>,
    task_cancellation_confirmation: Option<TaskCancellationConfirmation>,
    task_deletion_confirmation: Option<TaskDeletionConfirmation>,
    task_retry_confirmation: Option<TaskRetryConfirmation>,
    /// UI 自己拥有的标题、Profile、主题和短期状态提示。
    labels: SessionLabels,
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
    Quit,
}

impl Default for Ui {
    /// 构造首次启动时的默认界面状态。
    fn default() -> Self {
        Ui {
            tab: Tab::All,
            selected: 0,
            project_tree_selection: None,
            collapsed_project_tree_nodes: HashSet::new(),
            expanded_project_tree_disclosures: HashSet::new(),
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
        Ok(Self {
            guide_open: !profile.help_seen,
            labels: SessionLabels::load(label_path),
            profile_path: Some(profile_path),
            profile,
            theme,
            ..Self::default()
        })
    }

    /// 返回当前顶层页签。
    pub fn active_tab(&self) -> Tab {
        self.tab
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

    /// 通过 UI 的公共交互入口处理一个按键。
    ///
    /// 模态界面按优先级先消费按键；未被弹窗、编辑器或 Guide 消费的按键才会
    /// 进入全局页签、列表导航和详情操作。返回值只声明外层需要执行的动作。
    pub fn handle_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        snap: &Snapshot,
    ) -> std::io::Result<UiAction> {
        // 快照可能在两次按键之间变化，先把所有选择位置收回到有效范围。
        self.clamp_selections(snap);
        // 标题编辑器是最高优先级模态状态，打开时不会泄漏全局快捷键。
        if self.edit_title.is_some() {
            self.handle_title_editor_key(key);
            return Ok(UiAction::None);
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
        // Guide 打开后拥有其余按键，防止帮助导航改变底层页面状态。
        if self.guide_open {
            self.handle_guide_key(key)?;
            return Ok(UiAction::None);
        }
        // 下面按上下文分派普通页面、详情和复制菜单的导航动作。
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
            crossterm::event::KeyCode::Right
                if self.detail.is_none()
                    && self.tab == Tab::All
                    && self.all_region == AllRegion::Now =>
            {
                self.move_project_tree_horizontally(snap, true);
            }
            crossterm::event::KeyCode::Left
                if self.detail.is_none()
                    && self.tab == Tab::All
                    && self.all_region == AllRegion::Now =>
            {
                self.move_project_tree_horizontally(snap, false);
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if self.detail.is_none()
                    && self.tab == Tab::All
                    && self.all_region == AllRegion::Now =>
            {
                self.move_project_tree_selection(snap, true);
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if self.detail.is_none()
                    && self.tab == Tab::All
                    && self.all_region == AllRegion::Now =>
            {
                self.move_project_tree_selection(snap, false);
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

    /// 切换 Guide 主题后重置滚动，并等待下一次绘制重新测量上限。
    fn reset_guide_scroll(&mut self) {
        self.guide_scroll = 0;
        self.guide_max_scroll.set(UNMEASURED_GUIDE_MAX_SCROLL);
    }

    /// 切换顶层页签，并清除只属于旧页面的临时导航状态。
    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.selected = 0;
        self.project_tree_selection = None;
        self.all_region = AllRegion::Now;
        self.signal_selected = 0;
        self.task_selected = 0;
        self.detail = None;
        self.handoff_confirmation = None;
        self.task_cancellation_confirmation = None;
        self.task_deletion_confirmation = None;
        self.task_retry_confirmation = None;
    }

    /// 根据当前页面和焦点选择最贴近上下文的 Guide 默认主题。
    fn guide_category_for_context(&self) -> GuideCategory {
        if self.detail.is_some() || self.tab != Tab::All {
            GuideCategory::Sessions
        } else if self.all_region == AllRegion::RecentSignals {
            GuideCategory::Signals
        } else {
            GuideCategory::Overview
        }
    }

    /// 判断详情页复制菜单当前是否打开。
    fn copy_menu_is_open(&self) -> bool {
        self.detail
            .as_ref()
            .is_some_and(|detail| detail.copy_menu_selected.is_some())
    }

    /// 在复制菜单中移动到下一个实际有值的字段。
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
        if self.tab.provider().is_none() {
            self.project_tree_selection = Some(ProjectTreeKey::Session(identity));
        } else if let Some(index) = self.provider_targets(snap).iter().position(|target| {
            matches!(target, ProviderTarget::Session(session)
                    if session.provider == identity.provider && session.id == identity.native_id)
        }) {
            self.selected = index;
        }
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

    /// 根据当前展开状态建立 All 页面实际可见的 Session Tree 行。
    fn project_tree_rows<'a>(&self, snap: &'a Snapshot) -> Vec<projects::ProjectTreeRow<'a>> {
        let attention = current_attention(snap);
        projects::rows(
            &snap.sessions,
            attention.as_ref(),
            snapshot_now_ms(snap),
            &self.collapsed_project_tree_nodes,
            &self.expanded_project_tree_disclosures,
        )
    }

    /// 解析当前稳定选择；首次进入或目标消失时按 Attention 顺序回退到 Session。
    fn effective_project_tree_selection(&self, snap: &Snapshot) -> Option<ProjectTreeKey> {
        let rows = self.project_tree_rows(snap);
        if let Some(selection) = self
            .project_tree_selection
            .as_ref()
            .filter(|selection| rows.iter().any(|row| row.key() == *selection))
        {
            return Some(selection.clone());
        }
        rows.iter()
            .filter_map(projects::ProjectTreeRow::session_key)
            .nth(self.selected)
            .or_else(|| rows.iter().find_map(projects::ProjectTreeRow::session_key))
            .or_else(|| rows.first().map(|row| row.key().clone()))
    }

    /// 将已消失或被隐藏的节点选择收敛到确定性的可见回退目标。
    fn reconcile_project_tree_selection(&mut self, snap: &Snapshot) {
        self.project_tree_selection = self.effective_project_tree_selection(snap);
    }

    /// 在 Session Tree 的可见节点之间移动，不会落到已折叠的 Session。
    fn move_project_tree_selection(&mut self, snap: &Snapshot, forward: bool) {
        let rows = self.project_tree_rows(snap);
        let Some(selection) = self.effective_project_tree_selection(snap) else {
            return;
        };
        let Some(index) = rows.iter().position(|row| row.key() == &selection) else {
            return;
        };
        let next = if forward {
            (index + 1).min(rows.len().saturating_sub(1))
        } else {
            index.saturating_sub(1)
        };
        self.project_tree_selection = rows.get(next).map(|row| row.key().clone());
    }

    /// 实现常规 Tree 的 Left/Right：折叠、展开、回到父节点或进入首个子节点。
    fn move_project_tree_horizontally(&mut self, snap: &Snapshot, right: bool) {
        let rows = self.project_tree_rows(snap);
        let Some(selection) = self.effective_project_tree_selection(snap) else {
            return;
        };
        let Some(index) = rows.iter().position(|row| row.key() == &selection) else {
            return;
        };
        let row = &rows[index];
        if right {
            if row.is_group() && !row.is_expanded() {
                self.expand_project_tree_node(row.key());
                self.project_tree_selection = Some(selection);
            } else if row.is_group()
                && rows
                    .get(index + 1)
                    .is_some_and(|child| child.depth() > row.depth())
            {
                self.project_tree_selection = rows.get(index + 1).map(|child| child.key().clone());
            }
        } else if row.is_group() && row.is_expanded() {
            self.collapse_project_tree_node(&selection);
            self.project_tree_selection = Some(selection);
        } else if let Some(parent) = row.parent_key() {
            self.project_tree_selection = Some(parent.clone());
        }
    }

    fn expand_project_tree_node(&mut self, key: &ProjectTreeKey) {
        if key.uses_disclosure_state() {
            self.expanded_project_tree_disclosures.insert(key.clone());
        } else {
            self.collapsed_project_tree_nodes.remove(key);
        }
    }

    fn collapse_project_tree_node(&mut self, key: &ProjectTreeKey) {
        if key.uses_disclosure_state() {
            self.expanded_project_tree_disclosures.remove(key);
        } else {
            self.collapsed_project_tree_nodes.insert(key.clone());
        }
    }

    /// 切换当前顶层分组、Project、Worktree 或 Session disclosure 的展开状态。
    fn toggle_project_tree_group(&mut self, key: ProjectTreeKey) {
        if key.uses_disclosure_state() {
            if !self.expanded_project_tree_disclosures.remove(&key) {
                self.expanded_project_tree_disclosures.insert(key.clone());
            }
        } else if !self.collapsed_project_tree_nodes.remove(&key) {
            self.collapsed_project_tree_nodes.insert(key.clone());
        }
        self.project_tree_selection = Some(key);
    }

    /// 按实际渲染顺序返回当前页可选择的 Session。
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
            None => projects::sessions(snap).into_iter().cloned().collect(),
        }
    }

    /// 返回当前页可选择行的数量。
    pub fn selectable_len(&self, snap: &Snapshot) -> usize {
        if self.tab.provider().is_some() {
            self.provider_targets(snap).len()
        } else {
            self.project_tree_rows(snap).len()
        }
    }

    /// 把普通列表选中位置限制在给定长度内。
    pub fn clamp_selection(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    /// 根据最新快照校正所有区域的独立选中位置。
    pub fn clamp_selections(&mut self, snap: &Snapshot) {
        if self.tab == Tab::All {
            self.reconcile_project_tree_selection(snap);
        } else {
            self.clamp_selection(self.selectable_len(snap));
        }
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
        if self.tab == Tab::All {
            if self.all_region != AllRegion::Now {
                return None;
            }
            return self.editor_session(snap);
        }
        match self.provider_targets(snap).into_iter().nth(self.selected) {
            Some(ProviderTarget::Session(session)) => Some(session),
            _ => None,
        }
    }

    /// Resolve the All-page editor target from Tree selection even while a status panel has focus.
    fn editor_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        if self.tab != Tab::All || self.detail.is_some() {
            return None;
        }
        let ProjectTreeKey::Session(identity) = self.effective_project_tree_selection(snap)? else {
            return None;
        };
        snap.sessions.iter().find(|session| {
            session.provider == identity.provider && session.id == identity.native_id
        })
    }

    /// 为当前 Session 打开标题编辑器，并预填已有 Alias。
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

    /// 处理标题编辑器的输入、删除、保存和取消。
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

    /// 返回提供方页面中可交互的 Session 与展开行。
    fn provider_targets<'a>(&self, snap: &'a Snapshot) -> Vec<ProviderTarget<'a>> {
        self.provider_page_rows(snap)
            .into_iter()
            .filter_map(ProviderPageRow::into_target)
            .collect()
    }

    /// 为当前提供方建立已应用展开状态的扁平行列表。
    fn provider_page_rows<'a>(&self, snap: &'a Snapshot) -> Vec<ProviderPageRow<'a>> {
        let Some(provider) = self.tab.provider() else {
            return Vec::new();
        };
        provider::rows(
            &snap.sessions,
            provider,
            snap.generated_ms,
            &self.expanded_provider_disclosures,
        )
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
            scroll: 0,
            origin_signal: None,
            copy_menu_selected: None,
        });
        (task.state.is_terminal() && task.acknowledged_at_ms.is_none() && !task.is_being_deleted())
            .then(|| UiAction::AcknowledgeTask {
                task_id: task.id.clone(),
            })
    }

    /// 激活当前列表项：进入 Session/Signal 详情或切换提供方分组展开状态。
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

        if self.all_region == AllRegion::Now {
            let Some(selection) = self.effective_project_tree_selection(snap) else {
                return;
            };
            match selection {
                ProjectTreeKey::Session(identity) => {
                    self.detail = Some(DetailView {
                        session: identity,
                        task_id: None,
                        scroll: 0,
                        origin_signal: None,
                        copy_menu_selected: None,
                    });
                }
                group @ (ProjectTreeKey::Section(_)
                | ProjectTreeKey::Project(_)
                | ProjectTreeKey::Worktree { .. }
                | ProjectTreeKey::Disclosure { .. }) => {
                    self.toggle_project_tree_group(group);
                }
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
                    if let Some(session) = projects::sessions(snap).into_iter().find(|session| {
                        session
                            .worktree
                            .as_ref()
                            .is_some_and(|worktree| worktree.root == *worktree_root)
                    }) {
                        self.tab = Tab::All;
                        self.all_region = AllRegion::Now;
                        self.project_tree_selection =
                            Some(ProjectTreeKey::Session(SessionIdentity {
                                provider: session.provider,
                                native_id: session.id.clone(),
                            }));
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

    /// 切换一个提供方分组的展开状态。
    fn toggle_provider_disclosure(&mut self, disclosure: ProviderDisclosureKey) {
        if !self.expanded_provider_disclosures.remove(&disclosure) {
            self.expanded_provider_disclosures.insert(disclosure);
        }
    }
}

/// 绘制完整终端画面。
///
/// 先处理最小尺寸保护，再划分页头、主内容和页脚；详情页替换主内容，Guide、
/// 编辑器和确认框按照模态优先级覆盖在最上层。
pub fn draw(f: &mut Frame, snap: &Snapshot, ui: &Ui) {
    let theme = ui.theme.palette();
    f.render_widget(
        Block::default().style(Style::default().fg(theme.foreground).bg(theme.background)),
        f.area(),
    );
    if f.area().width < MIN_TERMINAL_WIDTH || f.area().height < MIN_TERMINAL_HEIGHT {
        modals::draw_terminal_too_small(f, theme);
        return;
    }
    // 顶层始终保留一行页头和一行页脚，中间区域交给当前页面或详情页。
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // 全局页头
            Constraint::Min(3),    // 主内容区
            Constraint::Length(1), // 全局页脚
        ])
        .split(f.area());

    chrome::draw_header(f, chunks[0], snap, ui);

    // 详情页会完整替换当前页签的主内容，但不会遮住全局页头和页脚。
    if let Some(task) = ui.detail_task(snap) {
        tasks::draw_detail(f, chunks[1], task, snap, ui);
    } else if ui.detail.is_some() {
        detail::draw(f, chunks[1], snap, ui);
    } else {
        match ui.tab {
            Tab::All => overview::draw(f, chunks[1], snap, ui),
            _ => provider::draw(f, chunks[1], snap, ui),
        }
    }

    chrome::draw_footer(f, chunks[2], snap, ui);

    modals::draw_overlays(f, snap, ui);
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
