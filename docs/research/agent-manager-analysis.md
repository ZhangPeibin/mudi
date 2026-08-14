# agent-manager 的 Agent 功能实现分析

## 文档信息

- 调研对象：[YoanWai/agent-manager](https://github.com/YoanWai/agent-manager)
- 固定版本：[a575982](https://github.com/YoanWai/agent-manager/commit/a57598200a4ec59dea937b67af5ff067062ddabc)
- 调研日期：2026-08-05
- 目的：说明该项目如何实现 Agent 管理能力，并判断哪些设计适合 Mudi 借鉴。

## 结论摘要

`agent-manager` 没有实现自己的 Agent runtime。它不负责调用模型 API，也不实现推理、
规划、上下文管理或工具调用循环。真正执行任务的 Agent 仍然是用户已经安装并登录的
Claude Code、Codex、OpenCode、Grok、Gemini CLI 或 Pi。

它实现的是一个基于 tmux 的多 Agent 会话编排器：

1. 根据 provider 配置拼接现有 Agent CLI 的启动命令。
2. 在私有 tmux server 中为每个 Agent 创建持久终端会话。
3. 通过 tmux 向 Agent 的终端输入框投递 Prompt。
4. 结合进程树、终端内容、正则规则和 Claude Code Hooks 推断运行状态。
5. 在 SQLite 中保存管理器元数据和 provider-native conversation ID。
6. 通过 CLI 原生的 resume/fork 命令恢复或派生指定对话。
7. 向支持 MCP 的 Agent 注册一个小型管理接口，使 Agent 能回报会话名称和 diff 范围。

因此，它的核心不是“实现 Agent”，而是“把多个现成的交互式 Agent CLI 包装成可统一
观察和操作的持久会话”。项目 README 也明确称其为 existing CLI 之上的 thin layer，
并说明登录、订阅、配置和 MCP 都继续由原 CLI 自己负责。[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/README.md#L24-L32)

## 总体架构

```text
Bubble Tea TUI
    |
    | create / prompt / attach / kill / revive / fork
    v
Session orchestration
    |-- Tool config: command, resume, fork, status rules
    |-- SQLite: manager session metadata
    |-- MCP/mailbox: agent -> manager metadata
    v
Private tmux server: agentmgr
    |-- am_<manager-session-id> -> Claude Code
    |-- am_<manager-session-id> -> Codex
    |-- am_<manager-session-id> -> Gemini CLI
    `-- ...
          |
          | provider-native login, model access, tools and conversation
          v
      Existing Agent CLI runtime
```

主要依赖包括 Bubble Tea/Lip Gloss、tmux、SQLite、MCP Go SDK 和系统进程采样库。
[依赖清单](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/go.mod#L5-L21)

## 端到端实现链路

### 1. Provider 配置描述 CLI 能力

`config.Tool` 把不同 Agent CLI 的差异声明为配置：

| 字段 | 作用 |
|---|---|
| `command` | 新会话启动命令 |
| `prompt_flag` | 首个 Prompt 需要使用的参数名 |
| `session_id_flag` | 启动时向 CLI 指定 conversation ID |
| `session_store` | CLI 自己生成 ID 时使用的捕获器 |
| `resume_by_id_command` | 恢复指定原生对话 |
| `revive_command` | 没有原生 ID 时的降级恢复命令 |
| `fork_command` | 从已有对话派生新对话 |
| `status_source` | 是否使用 provider 事件源，例如 Claude Hooks |
| `activity_cutoff`、`turn_end`、`rules` 等 | 从终端内容推断状态 |

字段定义见 [`internal/config/config.go`](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/config/config.go#L17-L52)，内置 Claude、Codex 等配置见[同文件](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/config/config.go#L223-L377)。

这种设计实现了数据驱动的 provider 差异，但还不是完整的 provider adapter。会话 ID 捕获、
MCP 注册和部分生命周期行为仍然通过 provider 名称分支实现。

### 2. 创建并启动会话

新建会话的调用链是：

```text
submit form / quick spawn
    -> spawnSession
    -> launchNewSession
    -> buildLaunch
    -> tmux.Create
    -> store.CreateSession
```

`spawnSession` 会：

1. 生成 manager session ID。
2. 按需创建独立 Git worktree。
3. 将首个 Prompt 安全地加入 CLI 命令。
4. 对支持指定 ID 的 CLI 生成 provider-native UUID。
5. 建立状态为 `starting` 的 Session 记录。

实现见 [`internal/ui/form.go`](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/ui/form.go#L570-L630)。

`buildLaunch` 继续注入 `AGENT_MANAGER_SESSION_ID`、MCP 配置，以及 Claude Code 的 Hook
settings 和状态文件路径。[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/ui/form.go#L632-L657)

`launchNewSession` 最终创建 tmux 会话，再将记录写入 SQLite；数据库写入失败时会关闭刚
创建的 tmux 会话并清理相关文件。[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/ui/sessionlaunch.go#L15-L48)

### 3. tmux 是 Agent 进程容器

所有 Agent 都运行在名为 `agentmgr` 的私有 tmux server 中，session 名称统一使用
`am_<id>`。它不会与用户自己的默认 tmux server 混合。

创建时使用 detached session，并设置 cwd 和初始终端尺寸。实际 CLI 命令先写入一个临时
shell 脚本，再由 tmux pane 执行。这样避免长 Prompt 或路径被 `send-keys` 的长度限制截断。
[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/tmux/tmux.go#L14-L20)
[创建实现](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/tmux/tmux.go#L73-L105)

tmux 同时带来两个产品能力：

- Manager TUI 退出后 Agent 进程仍然继续运行。
- 用户可以随时 attach 到原始交互终端，完整使用 provider CLI 的所有能力。

### 4. Prompt 通过终端投递

给正在运行的 Agent 发送 Prompt 时，项目不调用 provider API，也没有结构化 IPC。它执行：

```text
Prompt
  -> 写入临时文件
  -> tmux load-buffer
  -> tmux paste-buffer -p
  -> tmux send-keys Enter
```

使用 tmux buffer 而不是逐键发送，可以支持长文本和多行内容；`-p` 保留 bracketed paste
边界，避免 Codex 把紧随其后的 Enter 当作粘贴内容。[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/tmux/tmux.go#L206-L258)

Quick Prompt 的发送入口在 [`internal/ui/quick.go`](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/ui/quick.go#L218-L257)。选择 Group 时，同一个入口会改为创建新 Agent，并把输入作为首个 Prompt。

### 5. 状态来自进程、终端和 Hooks

统一状态集合为：

```text
starting / working / waiting / finished / errored / idle / dead
```

Poller 周期性执行以下工作：

1. 获取各 tmux pane 的 PID。
2. 采样 pane shell 的进程树、CPU 和内存。
3. 捕获 pane 的可见终端内容。
4. 去除 ANSI 控制序列。
5. 使用 provider-specific 正则识别 spinner、权限弹窗、错误和 turn end。
6. 比较两次 activity region 的 hash；持续变化表示正在输出。
7. 对停止变化的运行中区域等待 quiet grace，再推断 `finished` 或 `waiting`。
8. 没有有效进程时标记为 `dead`。

轮询主循环见 [`internal/ui/poller.go`](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/ui/poller.go#L266-L347)，状态派生见[同文件](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/ui/poller.go#L590-L704)。

状态规则只匹配最新 turn，防止历史输出中出现的 spinner 或权限提示被误认为当前状态。
如果 turn 最后一条有效内容包含问号，则倾向判断为 `waiting`；否则为 `finished`。
[规则引擎](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/status/status.go#L85-L117)
[turn 结束判断](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/status/status.go#L227-L279)

Claude Code 额外使用 Hooks：`UserPromptSubmit`、`PreToolUse` 和 `PostToolUse` 写入
`working`，阻塞型 Notification 写入 `waiting`，`Stop` 写入 `finished`，`SessionEnd`
删除状态文件。Poller 会把 Hook 结果与 pane 规则再做一次协调，处理普通文本提问、后台
subagent 和异常退出等 Hook 无法完整表达的情况。
[Hook 配置](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/hooks/hooks.go#L60-L89)

### 6. 保存并恢复准确的 provider 对话

SQLite 的 Session 同时保存 manager ID 和 `AgentSessionID`。后者是 Claude/Gemini 等
provider 的 session UUID、Codex rollout ID 或 OpenCode session ID。
[数据模型](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/store/store.go#L20-L40)

原生 ID 有两种获取方式：

- Claude、Grok、Gemini 和 Pi 支持 `--session-id`，Manager 在启动前生成 ID。
- Codex 和 OpenCode 自己生成 ID，Manager 在启动后捕获。

Codex 捕获器扫描 `~/.codex/sessions/**/rollout-*.jsonl`，读取第一行 `session_meta`，
根据 cwd、启动时间和是否已被其他 Session 占用来选择候选 ID。
[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/agentsession/capture.go#L240-L297)

OpenCode 则通过 `opencode session list` 和 `opencode export <id>` 获取公开元数据，避免
依赖其私有磁盘格式。[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/agentsession/capture.go#L80-L138)

Revive 优先执行 `resume_by_id_command`，例如 `codex resume <id>`。缺少 ID 时才退化到
`codex resume --last` 或 `claude --continue`，同时提示可能恢复到同 cwd 下的错误对话。
[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/ui/lifecycle.go#L168-L223)

Fork 也不复制 transcript，而是调用 provider CLI 的原生命令模板，例如
`claude --resume {id} --fork-session ...` 或 `codex fork {id}`。
[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/ui/fork.go#L63-L142)

### 7. MCP 是 Agent 到 Manager 的元数据通道

Manager 会把自身二进制以 stdio MCP server 的形式注册到支持的 Agent 会话中。不同 CLI
使用不同注册方式：启动参数、生成配置文件、环境变量或一次性的用户级注册。
[注册实现](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/mcpreg/mcpreg.go#L47-L88)

MCP server 只提供四个管理工具：

- `rename`
- `review_repo`
- `review_base`
- `review_mode`

这些工具不会驱动 Agent 执行任务。它们将结果写入 session-scoped mailbox 文件，由 Poller
在下一轮读取、更新 SQLite 并删除文件。
[MCP tools](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/mcpserver/mcpserver.go#L34-L86)
[Mailbox 实现](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/internal/sessioncmd/sessioncmd.go#L32-L53)

这是一条“Agent 向 Manager 声明元数据”的窄通道，不是 Agent 间通信，也不是 Manager
实现的通用 Agent tool runtime。

## 能力边界

`agent-manager` 实现了：

- 多个交互式 Agent CLI 的并行启动、持久化和统一展示。
- Prompt 快速投递、attach、kill、revive 和 provider-native fork。
- 基于终端和进程证据的 attention 状态。
- Git worktree 隔离及 diff review 工作流。
- Agent 到 Manager 的少量 MCP 元数据回报。

它没有实现：

- 模型 API client 或统一模型抽象。
- Agent 的推理、规划、上下文压缩、memory 或 RAG。
- 通用 tool execution loop。
- Agent 之间的自动通信、任务分派或依赖调度。
- provider 无关的结构化运行事件协议。

README 也明确把 agent-to-agent communication 列为尚未实现的能力。
[源码依据](https://github.com/YoanWai/agent-manager/blob/a57598200a4ec59dea937b67af5ff067062ddabc/README.md#L24-L32)

## 设计优点与风险

### 优点

1. **兼容性高。** 直接运行用户原有 CLI，因此登录、订阅、配置、工具和 MCP 行为自然保留。
2. **持久性强。** tmux 使 Agent 生命周期不依赖 Manager TUI 生命周期。
3. **渐进接入。** 新 CLI 最低只需一个启动命令；增加规则后即可获得更准确的状态。
4. **原生对话恢复。** 独立保存 provider-native ID，避免同 cwd 多会话时恢复错误。
5. **热路径本地化。** 主要操作是本机进程、tmux、SQLite 和文件读写。

### 风险与代价

1. **终端内容不是稳定协议。** CLI 文案、布局、Unicode 字符或版本变化都可能破坏正则。
2. **Prompt 投递依赖当前终端状态。** pane 若不在 composer，粘贴和 Enter 可能被弹窗或其他
   交互界面消费。
3. **provider 分支仍有扩散。** 配置、ID 捕获、MCP 注册和 Hooks 分散在不同模块。
4. **Manager 成为进程所有者。** 这比被动观察器拥有更大的状态变更面和故障责任。
5. **部分 MCP 注册修改用户级配置。** Grok/Gemini 使用一次性 user-scope 注册，产品需要
   明确处理授权、升级和撤销语义。
6. **状态是启发式组合。** Hook、pane 和进程证据可能互相冲突，需要持续维护协调规则。

## 对 Mudi 的判断

Mudi 当前的权威模型规定：provider JSONL 是观察状态的 source of truth，SQLite 索引
可重建；被动观察是默认行为，任何 Control Operation 都必须由用户显式授权。现有 ADR 还
决定 Operation Task 使用非交互式 detached worker，并明确拒绝把 tmux 和终端按键注入作为
该执行路径。

因此，适合借鉴的是概念和边界，而不是直接复制其 tmux 控制架构。

| agent-manager 设计 | Mudi 判断 |
|---|---|
| 保存 manager ID 与 provider-native ID 的映射 | 值得采用；Mudi 已以 native Session ID 作为身份核心 |
| OS liveness 与 provider 状态共同判断 | 值得采用；与 `ProviderAdapter::live_state` 方向一致 |
| provider-specific command/resume/fork 配置 | 可抽象为 Provider Adapter 或 Launch Plan，不应泄漏到 UI |
| 私有 tmux server 承载所有 Agent | 不适合作为 Mudi 的默认观察模型 |
| 向 pane 粘贴 Prompt 并按 Enter | 不适合 Operation Task；与 ADR 0003 的非交互 worker 决策冲突 |
| 终端正则作为主状态源 | 可作为低置信度 fallback，不应替代 JSONL 和进程证据 |
| Agent 调用 MCP 写入窄 mailbox | 可以借鉴，但必须限定能力并服从显式授权模型 |
| 自动写入用户级 MCP 配置 | 不应静默执行；需要独立、明确且可撤销的用户授权 |
| tmux session 在 TUI 退出后继续运行 | Operation Task 已由 detached worker 解决，不需要引入 tmux 依赖 |

相关 Mudi 决策：

- [`ADR 0001`](../adr/0001-provider-adapter-seam.md)：provider-specific discovery、解析和 live state
  必须收进 `ProviderAdapter`。
- [`ADR 0002`](../adr/0002-explicitly-authorized-control.md)：任何 session control 都需要选定目标和
  显式授权。
- [`ADR 0003`](../adr/0003-detached-operation-task-workers.md)：Operation Task 使用 detached、
  non-interactive worker，拒绝 tmux 和按键注入依赖。
- [`ADR 0006`](../adr/0006-separate-successor-launch-from-terminal-adapters.md)：Successor Launch Plan
  保存结构化 executable、args、cwd 和 Artifact，而不是拼接 shell 字符串。

## 建议

如果 Mudi 后续扩展更多会话管理能力，建议吸收以下做法：

1. 在 Provider Adapter 内统一 native Session ID 发现、恢复命令和活跃状态证据。
2. 始终使用明确的 native Session ID 恢复，不依赖 `--last` 或 cwd 最近会话。
3. 保持 OS liveness 与 provider-native 日志状态的组合判断，任何一方都不单独代表真实状态。
4. 若需要 Agent 向 Mudi 回报信息，只提供很窄的声明式接口，并采用 mailbox 或类似的
   可审计边界。
5. 将 pane scraping 仅视为兼容旧 provider 的低置信度补充，不进入权威索引。
6. 保持 Control Operation 的显式授权、detached worker 和结构化 Launch Plan，不因为参考
   项目使用 tmux 就改变现有 ADR。

最终判断：`agent-manager` 是一个完成度较高的“交互式 Agent CLI 进程管理器”，它对
Mudi 最有价值的部分是会话身份、生命周期和 provider 差异处理经验；它不是可直接
移植的 Agent runtime，也不应成为 Mudi Control Operation 的执行架构模板。
