# 源码地图（src/）

本文按模块说明 `src/` 下每个 `.rs` 文件的职责，方便新读者建立整体认知。
术语以根目录 `CONTEXT.md` 为准，架构决策见 `docs/adr/`。文件自身的 `//!`
模块注释是第一手资料，本文是它们的中文索引与展开。

## 数据流一览

观察侧（只读）：

> Provider 源 JSONL 日志 → `providers/` 适配器归一化 → `index/` 增量缓存 →
> `collector.rs` 组装快照 → `refresh.rs` 发布 → `read_model.rs` 读模型 → `ui/` 渲染

控制侧（显式授权）：

> `ui/` 产生声明式意图 → `application.rs` 控制器分发 → `managed_sessions.rs` /
> `handoff/` / `skills/` 在用户明确授权后执行操作

## 入口

| 文件 | 职责 |
| --- | --- |
| `src/lib.rs` | crate 根：声明所有模块与公共导出，集成测试经由此访问内部能力 |
| `src/main.rs` | 程序入口：TUI 立即打开；后台任务完成首次历史索引（带进度）后每约 2 秒增量扫描；渲染循环每帧只读最新快照，从不阻塞在 I/O |

## 领域模型与配置

| 文件 | 职责 |
| --- | --- |
| `src/domain.rs` | 全应用共享的归一化领域模型；Provider 特有字段名不允许出现在 `providers/` 之外，词汇统一遵循 `CONTEXT.md` |
| `src/project.rs` | 把 Session 工作目录解析为规范的 Project 与 Worktree 身份（同一 Git common directory 归为一个 Project） |
| `src/session_labels.rs` | 持久化 Mudi 拥有的会话元数据（Session Alias、Session Description 等） |
| `src/models.rs` | 按 Model-Provider 划分的模型目录：价格、批量价格与上下文窗口，仅用于估算与描述性元数据 |
| `src/config.rs` | 配置：数据目录、环境变量覆盖、告警阈值与可配置的额外扫描目录 |

## 数据采集与索引

| 文件 | 职责 |
| --- | --- |
| `src/collector.rs` | Collector：发现源文件、驱动增量索引、构建 UI 渲染所需的归一化快照 |
| `src/index/mod.rs` | 可随时删除重建的增量 SQLite 索引：源日志之上的缓存，只存使用量元数据，绝不存提示词、摘要或文件内容；按 `(mtime, size)` 指纹跳过未变文件 |
| `src/providers/mod.rs` | Provider 适配器层：每个适配器把源 JSONL 转成 `ParsedSession`，Provider 特有字段名不得逃出本模块 |
| `src/providers/claude.rs` | Claude Code 适配器：transcript 路径、当前上下文计算规则、按 `message.id` 去重、Compaction 识别等 |
| `src/providers/codex.rs` | Codex CLI 适配器：累计 token 用量取最新值不累加、缓存输入不重复计费、按模型拆分、Compaction 合并等 |
| `src/providers/state.rs` | macOS 进程状态检测：Claude 按 PID 状态文件、Codex 按持有 rollout 文件的进程判断 Open；全部只读并校验进程启动时间以防 PID 复用 |

## 刷新、告警与关注状态

| 文件 | 职责 |
| --- | --- |
| `src/refresh.rs` | Refresh 应用服务：UI 读模型的唯一发布者 |
| `src/read_model.rs` | refresh 发布、UI 消费的结构化只含元数据的投影（Snapshot） |
| `src/alerts.rs` | 告警引擎：上下文阈值跨越（Warning 70% / Critical 85%）时发送 macOS 通知，每个阈值只通知一次，低于阈值后重置 |
| `src/attention.rs` | 从会话快照推导并排序当前 Attention Conditions |
| `src/signals.rs` | 记录进程生命周期内有意义的 Session 状态转变（Recent Signal） |

## 控制面：显式授权与执行

| 文件 | 职责 |
| --- | --- |
| `src/application.rs` | 应用控制器：处理声明式 UI 意图（Copy、AuthorizeHandoff、Rescan 等的分发中心） |
| `src/managed_sessions.rs` | 显式授权的 Managed Session 启动注册与原生 New 启动 |
| `src/successor.rs` | Successor Launch Plan 的校验与安全命令渲染 |
| `src/tasks.rs` | 用户已授权 Operation Task 的权威持久化 |
| `src/handoff/mod.rs` | Assisted Handoff 的显式授权与执行入口 |
| `src/handoff/application.rs` | Handoff 授权、重试、取消与对账策略 |
| `src/handoff/artifact.rs` | Handoff Artifact 校验与仅限属主的发布 |
| `src/handoff/contract.rs` | Handoff 应用与 Worker 之间的数据契约 |
| `src/handoff/ports.rs` | Handoff 应用与 Worker 策略依赖的可替换边界（端口） |
| `src/handoff/worker.rs` | 分离式 Handoff Worker 的生命周期与 Provider 执行 |
| `src/skills/mod.rs` | Mudi 托管 Skill 的通用边界：Skill 目录与安装器 trait |
| `src/skills/handoff.rs` | 把 `mudi-handoff` Skill 安装/校验到 Codex CLI 与 Claude Code 的 skills 目录（权限 0700/0600、防符号链接、区分 Missing/Stale/Unreadable/Conflict） |

## 系统适配器

| 文件 | 职责 |
| --- | --- |
| `src/clipboard.rs` | macOS 剪贴板写入适配器：`Clipboard` trait + 基于 `/usr/bin/pbcopy` 的实现，含详细的 Rust 学习注释 |

## UI 层（src/ui/）

约定：除 `ui_test.rs` 外，每个 `foo_test.rs` 都是同名模块 `foo.rs` 的配对
单元测试（例如 `detail_test.rs` 测试 `detail.rs`），下表不再单独列出。
`src/app.rs` 属于 UI 面向的顶层状态（页签、共享快照、格式化辅助），渲染实现都在 `ui/`。

| 文件 | 职责 |
| --- | --- |
| `src/app.rs` | UI 面向的应用状态：页签、共享快照与小型格式化辅助 |
| `src/ui/mod.rs` | TUI 顶层：`Ui` 交互状态、事件处理、页签与弹窗调度、剪贴板复制入口 |
| `src/ui/chrome.rs` | 全局页头、页脚和上下文状态提示 |
| `src/ui/overview.rs` | 总览页布局组合 |
| `src/ui/session_browser.rs` | All 页左栏组合：导航与选中会话事实 |
| `src/ui/editor.rs` | All 页的会话工作区编辑器 |
| `src/ui/detail.rs` | 可选择的 Session Detail 弹窗及其面向证据的内容模型 |
| `src/ui/project_tree/mod.rs` | Project Tree 组件入口 |
| `src/ui/project_tree/model.rs` | Project Tree 稳定身份、行模型、分组与排序 |
| `src/ui/project_tree/state.rs` | Project Tree 交互状态与导航 |
| `src/ui/project_tree/view.rs` | Project Tree 行的 Ratatui 渲染 |
| `src/ui/project_tree/tests.rs` | Project Tree 单元测试 |
| `src/ui/split_pane/mod.rs` | 可复用的双栏工作区布局，带一条稳定的垂直分隔条 |
| `src/ui/split_pane/layout.rs` | 双栏几何计算：最小宽度、默认比例与各区域 Rect |
| `src/ui/split_pane/state.rs` | 分隔条拖拽、键盘调整等交互状态 |
| `src/ui/split_pane/view.rs` | 分隔条与边框渲染 |
| `src/ui/tasks.rs` | Operation Task 界面实现，拥有首页任务状态面板 |
| `src/ui/signals.rs` | Recent Signal 的格式化与首页面板 |
| `src/ui/context.rs` | Current Context 的语义角色与共享进度呈现 |
| `src/ui/attention_summary.rs` | 选中 Project Session 关注标记的紧凑解释 |
| `src/ui/selected_session.rs` | 补充（而非重复）选中 Tree 行的紧凑事实 |
| `src/ui/field.rs` | 相邻 Session 面板共用的紧凑键/值行 |
| `src/ui/provider.rs` | Claude Code 与 Codex CLI 的 Provider 会话清单 |
| `src/ui/provider_mark.rs` | 终端安全的 Provider 标记（Claude / Codex 轮廓） |
| `src/ui/provenance_mark.rs` | Project Tree 上外部观察 Session 的紧凑后缀标记 |
| `src/ui/hidden_tree.rs` | 当前可见 Hidden Sessions 的专用管理视图 |
| `src/ui/managed_start.rs` | 未完成 Managed 启动的仅元数据状态与恢复控件 |
| `src/ui/modals.rs` | 全局模态覆盖层、编辑器与终端尺寸保护 |
| `src/ui/guide.rs` | Guide 弹窗、主题内容与滚动测量 |
| `src/ui/layout.rs` | 多个 UI 功能共用的几何布局辅助函数 |
| `src/ui/path_display.rs` | 本地文件系统路径的共享展示辅助 |
| `src/ui/text.rs` | 文本宽度与截断工具：终端显示宽度不等于字符数，统一处理对齐与缩写 |
| `src/ui/theme.rs` | 主题颜色定义与主题名（Catppuccin、Dracula、Gruvbox 等） |
| `src/ui/ui_test.rs` | UI 共享单元测试：状态转换、响应式布局、语义颜色与稳定身份导航 |
