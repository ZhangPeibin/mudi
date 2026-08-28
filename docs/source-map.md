# 源码地图（src/）

本文按模块说明 `src/` 下每个 `.rs` 文件的职责，方便新读者建立整体认知。
术语以根目录 `CONTEXT.md` 为准，架构决策见 `docs/adr/`。文件自身的 `//!`
模块注释是第一手资料，本文是它们的中文索引与展开。

## 数据流一览

观察侧（只读）：

> Provider 源 JSONL 日志 → `adapters/providers/` 适配器归一化 →
> `adapters/persistence/index/` 增量缓存 → `application/observation/collect.rs` 产生
> `ObservationFrame` → `application/observation/reconcile.rs` 对账已有授权记录 →
> `application/observation/project.rs` 只读投影 →
> `application/observation/publish.rs` 原子发布 →
> `application/observation/read_model.rs` 读模型 → `presentation/tui/` 渲染

控制侧（显式授权）：

> `presentation/tui/` 产生声明式动作 → `presentation/tui/router.rs` 路由 →
> `application/handlers.rs` 与工作流服务校验和推进状态 → `adapters/` 执行持久化、
> Provider、进程、tmux、Terminal.app、Clipboard 与通知操作

## 架构门禁

`python3 scripts/check_architecture.py` 是 CI 使用的仓库级架构检查入口。它对生产 Rust
源码执行七组透明检查：Domain 和 Application 的向内依赖、Presentation 不拥有权威状态
或系统实现、唯一 `StateDatabase` Schema Owner、Repository 无机会式 Schema 迁移、
Collect/Project 无权威写入，以及具体 SQLite/进程/macOS 实现只位于 Adapter。失败会输出
`ARCH001` 至 `ARCH007`、违规文件和行号；契约 fixture 由
`python3 -m unittest scripts/test_check_architecture.py` 运行。

扫描范围是 `src/**/*.rs`。仅排除路径段为 `tests`/`fixtures` 的源码和 `*_test.rs`，因为它们
不参与生产 ownership；内联测试仍接受门禁检查。`src/adapters/persistence/index/` 只在权威
Schema 与迁移规则中排除，因为 `index.db` 是可丢弃索引，不属于 `state.db` 的 Schema
Owner。Presentation 中面向用户的 Terminal.app/tmux 文案不算具体实现；类型依赖、命令构造
和系统 Adapter 定义仍会失败。

## 入口

| 文件 | 职责 |
| --- | --- |
| `src/lib.rs` | crate 根：声明 `domain`、`application`、`adapters`、`presentation` 四个顶层模块；逻辑模块路径与物理目录一致 |
| `src/main.rs` | Composition Root 与程序入口：构造 Application Handler 和具体 Adapter，分发非 TUI 进程模式；TUI 立即打开，后台任务约每 2 秒刷新，渲染循环每帧只读最新快照 |

## 领域模型与配置

| 文件 | 职责 |
| --- | --- |
| `src/domain/mod.rs` | 领域模块入口；声明子模块并重导出现有 `domain::*` 公共类型 |
| `src/domain/provider.rs` | Session Provider、Provider 源格式与只含路径/格式的源元数据 |
| `src/domain/process.rs` | 防止 PID 复用误判所需的精确进程身份（PID 与启动时间） |
| `src/domain/usage.rs` | Current Context、Processed Usage、模型用量汇总与 Compaction 元数据 |
| `src/domain/project.rs` | 基于规范目录路径的 Project 稳定身份与展示元数据 |
| `src/domain/session.rs` | Session 生命周期、来源、稳定身份与归一化 Session 聚合 |
| `src/adapters/project.rs` | 把 Session 工作目录解析为规范的 Project 身份；Git 使用 top-level，非 Git 使用工作目录 |
| `src/application/session_labels.rs` | Session Metadata/Curation 的 Query 与 Repository port、验证规则和只读内存 View；不包含 SQLite |
| `src/adapters/models.rs` | 按 Model-Provider 划分的模型目录：价格、批量价格与上下文窗口，仅用于估算与描述性元数据 |
| `src/adapters/config.rs` | 配置：数据目录、环境变量覆盖、告警阈值与可配置的额外扫描目录 |

## 数据采集与索引

| 文件 | 职责 |
| --- | --- |
| `src/adapters/observation.rs` | Collector Adapter：发现 Provider 源文件、驱动增量索引并构建归一化 Observation Batch |
| `src/adapters/persistence/index/mod.rs` | 可随时删除重建的增量 SQLite 索引：源日志之上的缓存，只存使用量元数据，绝不存提示词、摘要或文件内容；按 `(mtime, size)` 指纹跳过未变文件 |
| `src/adapters/providers/mod.rs` | Provider 适配器层：每个适配器把源 JSONL 转成 `ParsedSession`，Provider 特有字段名不得逃出本模块 |
| `src/adapters/providers/claude.rs` | Claude Code 适配器：transcript 路径、当前上下文计算规则、按 `message.id` 去重、Compaction 识别等 |
| `src/adapters/providers/codex.rs` | Codex CLI 适配器：累计 token 用量取最新值不累加、缓存输入不重复计费、按模型拆分、Compaction 合并等 |
| `src/adapters/providers/state.rs` | macOS 进程状态检测：Claude 按 PID 状态文件、Codex 按持有 rollout 文件的进程判断 Open；全部只读并校验进程启动时间以防 PID 复用 |

## 刷新、告警与关注状态

| 文件 | 职责 |
| --- | --- |
| `src/application/observation/frame.rs` | 不可变 `ObservationFrame`：一次周期内的规范化 Session、Binding 与进程证据、Managed Current Run/Resume eligibility 和 Collect 诊断；不含 Provider 对话内容 |
| `src/application/observation/collect.rs` | Collect 服务与只读 port：组合可丢弃 Index、Provider 源、权威候选记录和一次批量进程证据采样，不写权威状态 |
| `src/application/observation/reconcile.rs` | Reconciliation 服务：仅根据 Frame 推进已有 Managed Launch/Binding 与 Operation Task 权威记录；无授权记录时不能建立控制关系 |
| `src/application/observation/project.rs` | Project 服务与只读 Query port：组合 Frame、Managed provenance、Registration 与 Task，计算 Attention 和 Recent Signals，不写权威状态 |
| `src/application/observation/publish.rs` | Publish 服务：完整 Snapshot 构造成功后才原子替换 `ReadModelCache`，并在发布边界更新 Alert 通知状态 |
| `src/application/observation/refresh.rs` | TUI-owned `RefreshCycle`：只负责 Collect -> Reconcile -> Project -> Publish 调用顺序和周期级生命周期 |
| `src/application/observation/read_model.rs` | 四阶段诊断、Refresh health、UI 消费的结构化 Metadata-only Snapshot 与原子内存 Cache |
| `src/adapters/refresh.rs` | 四阶段生产组合：为各阶段分别打开只读/读写 Repository 端口，并接入 Index、Provider/进程证据、Task recovery、通知和系统时钟 Adapter |
| `src/application/alerts.rs` | 告警政策与通知 port：上下文阈值跨越（Warning 70% / Critical 85%）时请求通知，每个阈值只通知一次，低于阈值后重置 |
| `src/application/attention.rs` | 从会话快照推导并排序当前 Attention Conditions |
| `src/application/signals.rs` | 记录进程生命周期内有意义的 Session 状态转变（Recent Signal） |

## 控制面：显式授权与执行

| 文件 | 职责 |
| --- | --- |
| `src/application/mod.rs` | Application 模块入口：声明工作流、政策、port 与读模型模块 |
| `src/application/handlers.rs` | TUI 发起命令的 typed Handler：接收稳定身份和 typed request，返回 typed outcome/error；不包含界面文案或系统实现 |
| `src/application/clipboard.rs` | Clipboard 写入能力 port；何时复制及结果文案由 Presentation 决定 |
| `src/application/provider_runtime.rs` | Provider 可执行文件解析 port 与 Provider-neutral executable 集合 |
| `src/application/managed_sessions.rs` | Managed New/Resume、Binding 与 Current Run 的领域记录、应用政策和外部能力 port |
| `src/application/successor.rs` | Successor Launch Plan 的校验、命令渲染与 runtime/Terminal port |
| `src/application/tasks.rs` | 用户已授权 Operation Task 合约与 Repository port |
| `src/application/handoff/mod.rs` | Assisted Handoff 的显式授权与执行入口 |
| `src/application/handoff/application.rs` | Handoff 授权、重试、取消与对账策略 |
| `src/application/handoff/artifact.rs` | Handoff Artifact 内容契约与校验规则 |
| `src/application/handoff/contract.rs` | Handoff 应用与 Worker 之间的数据契约 |
| `src/application/handoff/ports.rs` | Handoff 应用与 Worker 策略依赖的可替换边界（端口） |
| `src/adapters/skills/mod.rs` | Mudi 托管 Skill 的通用边界：Skill 目录与安装器 trait |
| `src/adapters/skills/handoff.rs` | 把 `mudi-handoff` Skill 安装/校验到 Codex CLI 与 Claude Code 的 skills 目录（权限 0700/0600、防符号链接、区分 Missing/Stale/Unreadable/Conflict） |

## 系统适配器

| 文件 | 职责 |
| --- | --- |
| `src/adapters/persistence/tasks.rs` | Operation Task Repository 的 SQLite 实现 |
| `src/adapters/persistence/managed_sessions.rs` | Managed Session、Binding 与 Current Run Repository 的 SQLite 实现 |
| `src/adapters/persistence/session_metadata.rs` | Session Metadata/Curation Query 与 Repository 的 SQLite 实现 |
| `src/adapters/handoff.rs` | Handoff 服务生产组合、任务 ID、Worker 启动、Current Run 控制与完成通知 Adapter |
| `src/adapters/handoff_worker.rs` | 分离式 Handoff Worker 生命周期、私有 Artifact 发布与 Provider 进程执行 |
| `src/adapters/managed_sessions.rs` | Managed Session 的 Provider、进程证据、tmux 与 Terminal.app Adapter |
| `src/adapters/successor.rs` | Successor Provider runtime、进程启动与 Terminal.app Adapter |
| `src/adapters/macos/clipboard.rs` | 基于 `/usr/bin/pbcopy` 的 Clipboard port 实现 |
| `src/adapters/macos/notification.rs` | 基于 AppleScript 的 best-effort macOS 通知实现 |

## UI 层（src/presentation/tui/）

约定：除 `ui_test.rs` 外，每个 `foo_test.rs` 都是同名模块 `foo.rs` 的配对
单元测试（例如 `detail_test.rs` 测试 `detail.rs`），下表不再单独列出。
`src/presentation/app.rs` 提供 UI 面向的共享快照别名与格式化辅助，渲染实现都在
`src/presentation/tui/`。

| 文件 | 职责 |
| --- | --- |
| `src/presentation/app.rs` | UI 面向的共享快照别名与小型格式化辅助 |
| `src/presentation/tui/mod.rs` | TUI 顶层：`Ui` 临时交互状态、事件处理、页签与弹窗调度，以及声明式 `UiAction` |
| `src/presentation/tui/router.rs` | 将 `UiAction` 路由到 typed Application Handler，并拥有界面文案、Clipboard 写入和重新投影请求 |
| `src/presentation/tui/chrome.rs` | 全局页头、页脚和上下文状态提示 |
| `src/presentation/tui/overview.rs` | 总览页布局组合 |
| `src/presentation/tui/session_browser.rs` | All 页左栏组合：导航与选中会话事实 |
| `src/presentation/tui/editor.rs` | All 页的会话工作区编辑器 |
| `src/presentation/tui/detail.rs` | 可选择的 Session Detail 弹窗及其面向证据的内容模型 |
| `src/presentation/tui/project_tree/mod.rs` | Project Tree 组件入口 |
| `src/presentation/tui/project_tree/model.rs` | Project Tree 稳定身份、行模型、分组与排序 |
| `src/presentation/tui/project_tree/state.rs` | Project Tree 交互状态与导航 |
| `src/presentation/tui/project_tree/view.rs` | Project Tree 行的 Ratatui 渲染 |
| `src/presentation/tui/project_tree/tests.rs` | Project Tree 单元测试 |
| `src/presentation/tui/split_pane/mod.rs` | 可复用的双栏工作区布局，带一条稳定的垂直分隔条 |
| `src/presentation/tui/split_pane/layout.rs` | 双栏几何计算：最小宽度、默认比例与各区域 Rect |
| `src/presentation/tui/split_pane/state.rs` | 分隔条拖拽、键盘调整等交互状态 |
| `src/presentation/tui/split_pane/view.rs` | 分隔条与边框渲染 |
| `src/presentation/tui/tasks.rs` | Operation Task 界面实现，拥有首页任务状态面板 |
| `src/presentation/tui/signals.rs` | Recent Signal 的格式化与首页面板 |
| `src/presentation/tui/context.rs` | Current Context 的语义角色与共享进度呈现 |
| `src/presentation/tui/attention_summary.rs` | 选中 Project Session 关注标记的紧凑解释 |
| `src/presentation/tui/selected_session.rs` | 补充（而非重复）选中 Tree 行的紧凑事实 |
| `src/presentation/tui/field.rs` | 相邻 Session 面板共用的紧凑键/值行 |
| `src/presentation/tui/provider_mark.rs` | 终端安全的 Provider 标记（Claude / Codex 轮廓） |
| `src/presentation/tui/provenance_mark.rs` | Project Tree 上外部观察 Session 的紧凑后缀标记 |
| `src/presentation/tui/hidden_tree.rs` | 当前可见 Hidden Sessions 的专用管理视图 |
| `src/presentation/tui/managed_start.rs` | 未完成 Managed 启动的仅元数据状态与恢复控件 |
| `src/presentation/tui/modals.rs` | 全局模态覆盖层、编辑器与终端尺寸保护 |
| `src/presentation/tui/guide.rs` | Guide 弹窗、主题内容与滚动测量 |
| `src/presentation/tui/layout.rs` | 多个 UI 功能共用的几何布局辅助函数 |
| `src/presentation/tui/path_display.rs` | 本地文件系统路径的共享展示辅助 |
| `src/presentation/tui/text.rs` | 文本宽度与截断工具：终端显示宽度不等于字符数，统一处理对齐与缩写 |
| `src/presentation/tui/theme.rs` | 主题颜色定义与主题名（Catppuccin、Dracula、Gruvbox 等） |
| `src/presentation/tui/ui_test.rs` | UI 共享单元测试：状态转换、响应式布局、语义颜色与稳定身份导航 |
