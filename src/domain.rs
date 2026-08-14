//! 全应用共享的归一化领域模型。
//!
//! 设计原则：Provider 特有的 JSONL 字段名只允许出现在 `providers/` 适配器内部；
//! 本文件里的所有类型都使用 `CONTEXT.md` 定义的统一词汇（Session、Project、
//! Worktree、Current Run 等）。这样 UI、索引、控制面都只依赖这一套模型，
//! 将来新增 Provider 时只需新增适配器，不用改全局结构。
//!
//! 学习提示：本文件几乎每个类型都带一长串 `#[derive(...)]`。derive 会让编译器
//! 自动实现对应 trait，常见含义如下：
//!
//! - `Debug`：支持 `{:?}` 打印，方便日志和调试；
//! - `Clone`：可以显式复制（`.clone()`）；
//! - `Copy`：按位复制即可“复制”，赋值/传参不会移走原值（只适用于没有堆内存的类型）；
//! - `Default`：提供 `default()` 默认值；
//! - `PartialEq` / `Eq`：支持 `==` 比较；
//! - `Hash`：可作为 `HashMap` / `HashSet` 的键；
//! - `Serialize` / `Deserialize`：serde 支持，可序列化成 JSON 或存进 SQLite。

// serde 的派生宏需要这两个 trait 在作用域内。
use serde::{Deserialize, Serialize};
// `PathBuf` 是“拥有所有权的路径字符串”（堆分配、可修改）；
// 相对地，`&Path` 是借用视角的路径。文件路径需要存进结构体时用 `PathBuf`。
use std::path::PathBuf;

/// Provider（会话提供方）：Claude Code 或 Codex CLI。
///
/// 领域规则：Session 永远按 Provider 区分，不同 Provider 的会话绝不合并，
/// 即使它们时间相近、目录相同。
///
/// 这个枚举没有字段（两个“单元变体”），内存里就是一个小整数，所以能 derive `Copy`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    ClaudeCode,
    CodexCli,
}

/// Provider 本地 Session 源日志的编码格式。
///
/// 目前只有 JSONL 一种；做成枚举是为了将来出现新格式时，编译器能通过
/// `match` 穷尽检查强制所有处理点补上新分支。
///
/// `#[default]` 标在变体上，配合 `Default` derive：
/// `SourceFormat::default()` 会返回 `JsonLines`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceFormat {
    #[default]
    JsonLines,
}

impl SourceFormat {
    /// 给用户看的格式名称。
    ///
    /// - 参数是 `self`（按值）而不是 `&self`：因为类型是 `Copy`，传值零成本；
    /// - 返回 `&'static str`：字符串字面量编译进二进制，生命周期是“静态”的，
    ///   不需要在堆上分配 `String`。
    pub fn label(self) -> &'static str {
        match self {
            SourceFormat::JsonLines => "JSONL",
        }
    }
}

/// 某个 Session 源日志的位置与编码：索引器靠它定位并解析日志文件。
///
/// 注意：这里只存“日志在哪、什么格式”，绝不存日志内容本身（隐私规则）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMetadata {
    /// 源日志文件的绝对路径。
    pub path: PathBuf,
    /// 日志编码格式。
    pub format: SourceFormat,
}

impl Provider {
    /// 稳定的机器可读名称，用于持久化存储和内部标识。
    ///
    /// `as_str` 是 Rust 社区常见命名：把类型转成字符串切片。
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "claude",
            Provider::CodexCli => "codex",
        }
    }

    /// 给用户看的显示名称。
    pub fn label(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "Claude Code",
            Provider::CodexCli => "Codex CLI",
        }
    }

    /// 从字符串解析 Provider；无法识别时返回 `None`。
    ///
    /// 注意签名里没有 `self`：这是“关联函数”而不是方法，调用方式是
    /// `Provider::parse("claude")`，而不是在某个实例上调用。
    /// 返回 `Option<Provider>` 用类型表达“可能解析失败”，调用方必须处理 `None`。
    pub fn parse(s: &str) -> Option<Provider> {
        match s {
            "claude" => Some(Provider::ClaudeCode),
            "codex" => Some(Provider::CodexCli),
            // `_` 是通配分支，兜住所有其他字符串。
            _ => None,
        }
    }
}

/// Session 的实时生命周期状态。`Exited` 在界面上不显示状态标记。
///
/// 语义（见 `CONTEXT.md`）：Busy = 正在执行工作；Idle = 进程开着但在等待；
/// Exited = 进程已退出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Busy,
    Idle,
    Exited,
}

impl SessionState {
    /// 排序权重：Busy 最前（0），Idle 次之（1），Exited 最后（2）。
    ///
    /// 把“业务排序规则”编码成数字，列表排序时直接比较这个值即可。
    pub fn sort_rank(self) -> u8 {
        match self {
            SessionState::Busy => 0,
            SessionState::Idle => 1,
            SessionState::Exited => 2,
        }
    }

    /// 进程是否还开着（Busy 或 Idle 都算 Open）。
    ///
    /// `matches!` 是标准库宏：`matches!(值, 模式)` 返回 bool，
    /// 比手写 `match` 返回 true/false 更简洁。
    pub fn is_open(self) -> bool {
        !matches!(self, SessionState::Exited)
    }
}

/// Session 的“出身”：Mudi 只是观察到的，还是通过 Binding Receipt
/// 建立了管理关系。观察永远是默认值——这是产品的安全底线。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionProvenance {
    /// 默认：外部启动、Mudi 只读观察。
    #[default]
    Observed,
    /// Mudi 持有完整启动证据、可精确恢复的 Managed Session。
    Managed,
}

/// 累计 token 处理量（Provider 报告的口径）。
///
/// 关键设计：它与 [`ContextState`] 严格分开。Usage 是“历史累计流量”，
/// ContextState 是“当前窗口占用”，两者语义完全不同，混用会算错钱、算错占比。
/// 只有当 Provider 真的报告了某类数据，对应字段才会有非零值。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// 输入 token。
    pub input: u64,
    /// 输出 token。
    pub output: u64,
    /// 命中缓存读取的 token（Claude 的 cache_read）。
    pub cache_read: u64,
    /// 写入缓存的 token（Claude 的 cache_creation）。
    pub cache_write: u64,
}

impl Usage {
    /// 把另一个 Usage 累加到自身。
    ///
    /// `&mut self`：方法要修改自身，所以需要可变借用；
    /// `other: &Usage`：只读借用别人的数据，不拿走所有权。
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// 四类 token 的总和。
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// 模型上下文窗口的“最新占用量”。注意这不是累计值：
/// 每次请求后窗口占用会被最新值覆盖。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextState {
    /// 最近一次占用的 token 数。
    pub tokens: u64,
    /// 模型的上下文窗口大小；未知时为 `None`（有些 Provider 不报告）。
    pub window: Option<u64>,
}

impl ContextState {
    /// 窗口使用百分比；窗口未知（或为 0）时返回 `None`。
    ///
    /// `match` 守卫写法：`Some(w) if w > 0` 表示“是 Some 且值大于 0”。
    /// `as f64` 是显式数值类型转换（Rust 不做隐式转换，避免精度意外）。
    pub fn percent(&self) -> Option<f64> {
        match self.window {
            Some(w) if w > 0 => Some((self.tokens as f64 / w as f64) * 100.0),
            _ => None,
        }
    }
}

/// 单个模型的用量汇总。混合模型的 Session 会在这里按模型拆分，
/// 这样不同价格模型的 token 才能分别计费。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    /// 模型名。用 `String` 而不是 `&str`：结构体要拥有这份数据
    /// （`&str` 是借用，必须有别人持有原文本，不适合存进长寿命结构体）。
    pub model: String,
    pub usage: Usage,
    /// 按 API 价格估算的美元费用；模型价格未知时为 `None`。
    /// `Option<f64>` 强制调用方处理“算不出钱”的情况，而不是默默显示 $0。
    pub cost: Option<f64>,
}

/// 某个明确命名范围（如一次 Current Run）内的汇总用量与费用估算。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    /// 该范围内的累计用量。
    pub usage: Usage,
    /// 按模型拆分的明细。`Vec<T>` 是可变长数组（堆分配）。
    pub per_model: Vec<ModelUsage>,
    /// 美元费用估算；只要有任何一个参与模型价格未知，整体就是 `None`，
    /// 避免给出误导性的部分合计。
    pub cost: Option<f64>,
}

/// 一次上下文压缩（Compaction）的记录。触发原因保留 Provider 的原始字符串，
/// 不做二次解释（不透明转发）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Compaction {
    /// 观察到压缩的时刻（Unix 纪元毫秒）。
    pub at_ms: i64,
    /// Provider 报告的触发方式（如 "auto"、"manual"）；没有则为 `None`。
    pub trigger: Option<String>,
    /// 压缩前的 token 占用（若 Provider 有报告）。
    pub pre_tokens: Option<u64>,
}

/// Project 的稳定身份。
///
/// 这是“带数据的枚举”（每个变体携带一个 `PathBuf`）：
///
/// - Git 项目：用规范化的 Git common directory（所以一个仓库的多个 worktree
///   归属同一个 Project）；
/// - 非 Git 项目：用规范化的工作目录本身。
///
/// 身份用枚举而不是字符串前缀，编译器就能强制所有使用点区分这两种情况。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProjectIdentity {
    GitCommonDir(PathBuf),
    CanonicalCwd(PathBuf),
}

impl ProjectIdentity {
    /// 推导人类可读 Project 标签时使用的稳定路径。
    ///
    /// Git 情况下的链式写法值得学习：`file_name()`、`filter()`、`and_then()`
    /// 都返回 `Option`，可以像流水线一样串起来，任何一步得到 `None` 就短路：
    ///
    /// 1. `file_name()`：取路径最后一段（如 `/repo/.git` → `.git`）；
    /// 2. `filter(...)`：只有最后一段确实是 `.git` 才保留，否则变 `None`；
    /// 3. `and_then(|_| path.parent())`：是 `.git` 就退到父目录（真正的仓库根）；
    /// 4. `unwrap_or(path)`：前面任何一步失败，就退回原始路径。
    pub fn display_path(&self) -> &std::path::Path {
        match self {
            ProjectIdentity::GitCommonDir(path) => path
                .file_name()
                .filter(|name| *name == ".git")
                .and_then(|_| path.parent())
                .unwrap_or(path),
            ProjectIdentity::CanonicalCwd(path) => path,
        }
    }
}

/// 解析完成的 Project：稳定身份 + 显示标签。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub identity: ProjectIdentity,
    /// 给用户看的名称（通常是目录名；重名时会补充父路径上下文）。
    pub label: String,
}

/// 解析完成的 Git Worktree。规范化的根目录就是它的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    /// worktree 根目录（绝对路径）。
    pub root: PathBuf,
    /// 当前分支名；分离头指针（detached HEAD）等情况下为 `None`。
    pub branch: Option<String>,
    /// 是否可写（只读 worktree 上不能发起会产生修改的操作）。
    pub writable: bool,
}

/// 一个归一化后的 Session——UI 渲染的核心数据结构。
///
/// 它由 Provider 适配器产出、经索引与刷新管道传递，最终供界面展示。
/// 注意大量字段是 `Option<_>`：观察型工具必须容忍信息缺失，
/// 用类型系统承认“不知道”，而不是编造默认值。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub provider: Provider,
    /// Provider 原生会话 ID（本应用不发明自己的 ID）。
    pub id: String,
    /// 控制出身（Observed / Managed）。
    /// `#[serde(default)]`：反序列化旧数据时若缺这个字段，用 `Default` 补齐，
    /// 即默认为 `Observed`——这是向后兼容的常用手段。
    #[serde(default)]
    pub provenance: SessionProvenance,
    /// 被索引源日志的元数据。这里永远不暴露日志内容。
    pub source: SourceMetadata,
    /// 解析出的 Project（身份 + 显示标签）；解析失败时为 `None`。
    pub project: Option<Project>,
    /// Git Worktree 元数据；非 Git 目录或无法确定时为 `None`。
    pub worktree: Option<Worktree>,
    /// 会话的绝对工作目录（若可知）。
    pub cwd: Option<String>,
    /// 会话最近使用的模型（混合模型会话只记“最新”，明细在 per_model）。
    pub latest_model: Option<String>,
    pub state: SessionState,
    pub context: ContextState,
    /// 整个会话生命周期的累计用量。
    pub usage: Usage,
    /// 按模型拆分的用量明细。
    pub per_model: Vec<ModelUsage>,
    /// 自“已验证的 Current Run 起点”以来的用量；`None` 表示 Run 边界
    /// 或源用量不可得。
    #[serde(default)]
    pub current_run_usage: Option<UsageSummary>,
    /// 历史压缩事件列表。
    pub compactions: Vec<Compaction>,
    /// 源日志最后一次活动的时间（Unix 纪元毫秒）。
    pub last_activity_ms: i64,
    /// 估算总费用；任何一个参与模型价格未知则为 `None`。
    pub cost: Option<f64>,
}

/// Session 的稳定身份：Provider 范围内的原生 ID。
///
/// 与 `Session` 分开的原因：很多场景（去重、查找、关联）只需要“它是谁”，
/// 不需要拖着整个会话的所有观测数据。derive 了 `Hash` 所以能当 Map 的键。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionIdentity {
    pub provider: Provider,
    pub native_id: String,
}

/// 实现标准库的 `From` trait：允许 `SessionIdentity::from(&session)`
/// 或 `let id: SessionIdentity = (&session).into()` 这样的惯用转换。
///
/// 注意：`Session` 没有 derive `Hash`（含 `Vec`、`f64` 等不便哈希的字段），
/// 所以要当键时先转成这个精简的身份类型。
impl From<&Session> for SessionIdentity {
    fn from(session: &Session) -> Self {
        Self {
            // `Provider` 是 `Copy` 的，直接复制。
            provider: session.provider,
            // `String` 不是 `Copy` 的，必须 `.clone()` 出一份新数据。
            native_id: session.id.clone(),
        }
    }
}

impl Session {
    /// Project 显示标签；没有 Project 时用破折号占位。
    ///
    /// `map_or(默认值, 闭包)` 是 `Option` 的常用收敛方法：
    /// `None` → 返回默认值；`Some(x)` → 返回闭包(x) 的结果。
    pub fn project_label(&self) -> &str {
        self.project
            .as_ref()
            .map_or("—", |project| project.label.as_str())
    }

    /// 压缩次数。
    pub fn compaction_count(&self) -> usize {
        self.compactions.len()
    }
}
