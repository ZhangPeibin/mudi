# Codex app-server 研究

## 文档信息

- 调研对象：OpenAI Codex CLI 的 `codex app-server`
- 固定版本：[`codex-cli 0.146.0`](https://github.com/openai/codex/releases/tag/rust-v0.146.0)
- 调研日期：2026-08-05
- 主要依据：版本化官方源码、官方 app-server README，以及由本机相同版本生成的 JSON Schema
- 目的：说明 app-server 是什么、如何承载 Agent 能力、Skills 如何接入，并判断它与
  `agent-manager`、Codex UI 和 SessionMesh 的关系

固定版本很重要。app-server 命令和部分协议仍标记为 experimental，不同 Codex 版本生成的
协议 Schema 可能不同。本文描述的是 `0.146.0`，不是对未来版本的兼容性承诺。

## 一句话定义

`codex app-server` 是 Codex Core 面向富客户端的无界面协议层：客户端通过双向 JSON-RPC
创建或恢复对话、发起一轮 Agent 执行、接收流式事件，并处理命令、文件修改和权限审批。

它使 VS Code 扩展、自定义 UI 或远程 TUI 不必解析终端画面，也不必自己实现模型调用、
上下文、工具执行和会话持久化。[官方 README](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/app-server/README.md)
开头直接说明：app-server 是 Codex 用来支撑 VS Code 扩展等 rich interface 的接口。

## 它是什么，不是什么

| 判断 | 说明 |
|---|---|
| 是 Codex 的客户端后端接口 | 把 Codex Core 的会话、Agent 执行、工具、审批、Skills、配置和认证能力暴露成结构化协议 |
| 是无 UI 的本地进程或 daemon | 默认可由客户端作为子进程启动，也支持 Unix socket 和实验性 WebSocket |
| 是双向协议 | 客户端可以发请求；服务端也会反向请求客户端审批或补充用户输入 |
| 不是模型 | 真正的推理仍由配置的模型提供方完成 |
| 不是 OpenAI API 代理 | 它没有把自己伪装成 `/v1/responses` 一类通用模型 HTTP 服务 |
| 不是 MCP server | 虽然协议形式与 MCP 相似，但方法、对象和生命周期是 Codex 专用的；Codex 另有独立的 `codex mcp-server` 命令 |
| 不是 UI | 它不负责列表、输入框、diff viewer、审批弹窗等视觉呈现 |
| 不是 `agent-manager` 式终端编排器 | 它直接提供结构化 Codex 协议，不靠 tmux 粘贴 Prompt 或抓取终端文本判断状态 |

## 放在 Codex 架构中的位置

```text
VS Code extension / custom UI / remote Codex TUI
                    |
                    | Codex JSON-RPC
                    | request / response / notification
                    v
             codex app-server
        transport + protocol + orchestration
                    |
                    v
                Codex Core
      thread / agent loop / tools / sandbox
        skills / MCP / config / auth / hooks
             |                  |
             v                  v
      model provider       local machine
      Responses stream     shell / files / Git

Persistence beside the runtime:
  rollout JSONL + state/thread-store SQLite + config/auth files
```

从官方 crate 依赖可以看到，app-server 直接依赖 `codex-core`、`codex-rollout`、
`codex-state`、`codex-thread-store`、`codex-skills`、`codex-mcp` 和
`codex-sandboxing`。因此它不是向另一个 Codex CLI 终端转发文本，而是直接承载并编排
Codex runtime。[Cargo.toml](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/app-server/Cargo.toml)

实现技术以 Rust 为主：Tokio 负责异步任务和有界 channel，Serde 负责协议序列化，Axum
负责 HTTP Upgrade/WebSocket 传输，Codex Core 提供 Agent 核心能力。协议类型和 Schema
生成单独放在
[`app-server-protocol`](https://github.com/openai/codex/tree/rust-v0.146.0/codex-rs/app-server-protocol)。

## `Codex = Codex UI + app-server` 吗

只在特定部署形态下可以这样画，不能把它当作 Codex 的完整定义。

- 对 VS Code 扩展或连接远程后端的 TUI，可以近似理解为
  `Codex client UI -> app-server -> Codex Core`。
- `app-server` 自己已经链接并协调 Codex Core，不是一个只负责转发到另一个 `codex`
  进程的薄壳。
- `codex` CLI 二进制还包含本地交互 TUI、`exec`、`review`、登录、MCP 管理、sandbox、
  app-server 等多个 surface。它不能整体简化成一个 UI。
- 更准确的抽象是：**Codex 是 Agent runtime 与多个客户端 surface 的集合；app-server 是
  其中供富客户端使用的一种正式接口边界。**

当前 CLI 已支持 `codex --remote ws://...` 或 `codex --remote unix://...`，这时 TUI 明确是
app-server 的客户端。至于某个具体本地 surface 是否走进程内接口或独立 app-server 进程，
应以该版本的入口实现为准，不应从产品名称反推。

## 协议模型

### JSON-RPC 形式

协议使用双向 JSON-RPC 2.0，但线上消息省略通常的 `"jsonrpc": "2.0"` 字段。

```json
{"method":"skills/list","id":10,"params":{"cwds":[],"forceReload":false}}
{"id":10,"result":{"data":[]}}
{"method":"skills/changed","params":{}}
```

这里有三类消息：

| 类型 | 方向 | 是否有 `id` | 用途 |
|---|---|---:|---|
| Client Request | client -> app-server | 是 | 启动 Thread/Turn、查询 Skills、读取配置、控制进程等 |
| Server Request | app-server -> client | 是 | 请求命令审批、文件修改审批、权限或用户输入；客户端必须回传结果 |
| Server Notification | app-server -> client | 否 | 流式输出、Item 生命周期、状态、diff、计划、token usage、Skills 失效通知等 |

双向性是 app-server 能承载交互式 Agent 的关键。客户端不只是提交 Prompt 后等一段文本，
还参与每次敏感动作的授权闭环。

### 三个核心对象

| 对象 | 含义 | 典型内容 |
|---|---|---|
| Thread | 一段用户与 Codex Agent 的对话 | 多个 Turn、工作目录、模型、sandbox、持久化 ID |
| Turn | 一次用户到 Agent 的交互 | 从用户输入开始，到完成、失败或中断结束 |
| Item | Turn 中可持久化、可流式展示的工作单元 | 用户消息、推理、Agent 消息、shell 命令、文件修改、MCP 调用等 |

客户端应使用 Thread API 管理会话，用 Turn API 驱动 Agent，并使用 Item 事件绘制执行过程。
这比“把整个终端屏幕当成一个字符串”保留了更完整的语义。

## 一次完整交互

```text
Client                                      app-server / Codex Core
   |                                                 |
   | initialize(clientInfo, capabilities)            |
   |------------------------------------------------>|
   |<------------------------------------------ result|
   | initialized                                     |
   |------------------------------------------------>|
   |                                                 |
   | thread/start  (或 thread/resume/thread/fork)     |
   |------------------------------------------------>|
   |<--------------------------------------- Thread   |
   |<--------------------------- thread/started       |
   |                                                 |
   | turn/start(threadId, input)                      |
   |------------------------------------------------>|
   |<----------------------------------------- Turn   |
   |<----------------------------- turn/started       |
   |<----------------------------- item/started       |
   |<------------------ item/agentMessage/delta       |
   |<---------- requestApproval (需要时，带 id)        |
   |------------------------ approval result -------->|
   |<--------------------------- item/completed       |
   |<--------------------------- turn/completed       |
```

规则如下：

1. 每条连接只执行一次 `initialize`，随后发送 `initialized` notification。握手前的其他请求会
   被拒绝。
2. 使用 `thread/start` 创建会话，或使用 `thread/resume`、`thread/fork` 继续或派生会话。
3. 使用 `turn/start` 发送文本、图片、本地图片、Skill 等输入项，并可覆盖 cwd、模型、
   sandbox/permissions 和审批策略。
4. Item 的标准生命周期是 `item/started -> 零到多个 delta -> item/completed`。
5. `turn/completed` 是该轮结束边界，状态为 `completed`、`interrupted` 或 `failed`。
6. `turn/interrupt` 是异步中断请求；应等待最终 `turn/completed` 确认完成。

## 主要 API 面

下面不是完整列表，而是建立 app-server 心智模型所需的主要分组。

| 分组 | 代表方法或事件 | 作用 |
|---|---|---|
| Thread | `thread/start`、`resume`、`fork`、`list`、`read`、`archive`、`delete` | 会话生命周期和持久化管理 |
| Turn | `turn/start`、`turn/steer`、`turn/interrupt`、`turn/completed` | 发起、追加或终止一轮 Agent 工作 |
| Item/Event | `item/started`、`item/completed`、`item/agentMessage/delta` | 流式呈现消息和工具执行 |
| Review | `review/start` | 发起 Codex review |
| Skills | `skills/list`、`skills/config/write`、`skills/extraRoots/set`、`skills/changed` | 查询、配置和监听 Skills |
| Models/MCP | `model/list`、`mcpServerStatus/list` | 查询可用模型和 MCP 状态 |
| Config/Auth | `config/read`、`account/read`、`account/login/start` | 管理 Codex 配置与登录状态 |
| Process | `command/exec`、`process/outputDelta`、`process/exited` | 执行并观察客户端请求的进程 |
| Approval | `item/commandExecution/requestApproval`、`item/fileChange/requestApproval`、`item/permissions/requestApproval` | 让客户端明确决定敏感动作 |
| User Input | `item/tool/requestUserInput`、`mcpServer/elicitation/request` | Agent 或 MCP 在 Turn 中向用户补充提问 |

`turn/plan/updated`、`turn/diff/updated`、`thread/status/changed` 和
`thread/tokenUsage/updated` 等通知允许 UI 直接显示结构化状态，不需要从 Agent 的自然语言
输出猜测计划、diff 或 token 使用量。

## 传输与运行形态

### stdio

默认是 `codex app-server --listen stdio://`。每行一个 JSON-RPC 消息，即 JSONL。典型客户端
将 app-server 作为子进程启动，通过 stdin/stdout 通信，并单独读取 stderr 日志。

### Unix socket

`--listen unix://` 或 `--listen unix://PATH` 在 Unix domain socket 上接受使用标准 HTTP
Upgrade 的 WebSocket 连接。`codex app-server proxy` 可以把 stdin/stdout 字节代理到默认或
指定的 control socket。

### WebSocket

`--listen ws://IP:PORT` 每个 WebSocket text frame 承载一条 JSON-RPC 消息，同一 listener
还提供 `/readyz` 和 `/healthz`。但 `0.146.0` 官方 README 明确把 WebSocket 标为
**experimental / unsupported for production**。非 loopback listener 还必须配置 capability
token 或 signed bearer token 等认证方式。

### daemon 与远程 TUI

当前 CLI 提供 `codex app-server daemon bootstrap/start/restart/stop/version`、
`codex app-server proxy` 和 remote-control 相关命令；TUI 可通过 `codex --remote` 连接
WebSocket 或 Unix socket 地址。这提供了“UI 与 Agent runtime 分进程”的部署方式，但远程
控制的授权、版本和故障恢复责任也会转移给集成方。

### 背压

传输入口、请求处理和输出写入之间使用有界队列。入口饱和时返回 JSON-RPC 错误 `-32001`
和 `Server overloaded; retry later.`；客户端应使用带 jitter 的指数退避。慢连接的输出队列
填满时，服务端实现还可能断开该连接。

## Skills 到底怎样工作

### 1. 发现 Skills 是结构化查询

客户端使用真实的 `skills/list` API，而不是向 tmux 或聊天输入框发送“列出 Skills”的文字：

```json
{
  "method": "skills/list",
  "id": 25,
  "params": {
    "cwds": ["/Users/me/project"],
    "forceReload": true
  }
}
```

结果按 cwd 分组。每个 Skill 可包含 `name`、`description`、绝对 `path`、`enabled`、
`scope`、UI metadata 和 tool dependencies；结果还单列加载错误。`scope` 的 Schema 枚举为
`user`、`repo`、`system`、`admin`。

app-server 可以缓存每个 cwd 的扫描结果。`forceReload: true` 会绕过缓存重新扫描磁盘。
本地 Skill 文件变化后，服务端清空 cache 并广播 `skills/changed`；该 notification 只是失效
信号，客户端收到后应使用原参数再次调用 `skills/list`。

### 2. 执行 Skill 有两种输入方式

兼容方式是在文本输入里写 `$<skill-name>`：

```json
{
  "type": "text",
  "text": "$skill-creator Add a skill for triaging flaky CI."
}
```

推荐方式是在 `turn/start.input` 中同时增加结构化 `skill` item：

```json
{
  "method": "turn/start",
  "id": 101,
  "params": {
    "threadId": "thread-1",
    "input": [
      {
        "type": "text",
        "text": "$skill-creator Add a skill for triaging flaky CI."
      },
      {
        "type": "skill",
        "name": "skill-creator",
        "path": "/Users/me/.codex/skills/skill-creator/SKILL.md"
      }
    ]
  }
}
```

结构化 item 让 backend 直接定位 Skill 并注入完整指令，避免模型仅凭 `$name` 再查找路径所
产生的额外延迟。也就是说：**Skill 名称可以出现在 Prompt 文字中，但 app-server 的实现不
是“只把 Skill 名称发进 tmux”。** app-server 根本不依赖 tmux；它还提供发现、路径解析、
缓存、配置、文件监听和 backend 指令注入。

### 3. 配置和额外根目录

- `skills/config/write` 按绝对路径或名称启用、禁用 Skill。
- `skills/extraRoots/set` 为当前 app-server 进程替换额外的 standalone Skill roots；进程退出
  后失效。
- 每个 root 使用 `<root>/<skill-directory>/SKILL.md` 布局。
- Skill 的实际可见集合与 cwd、配置层、插件、系统 Skill 和运行环境有关，因此客户端不应
  把某次 `skills/list` 当作全局永久目录。

## 审批与安全边界

当 Codex Core 需要执行可能越权的动作时，app-server 向客户端发带 `id` 的反向请求，而不
是自己弹 UI：

- shell：`item/commandExecution/requestApproval`
- 文件修改：`item/fileChange/requestApproval`
- 额外文件系统或网络权限：`item/permissions/requestApproval`
- 工具补充输入：`item/tool/requestUserInput`
- MCP elicitation：`mcpServer/elicitation/request`

客户端负责把请求呈现给用户并回传同意、拒绝或允许的权限子集。`request_permissions`
只认可原请求中的权限，响应新增的权限会被忽略；省略的权限视为拒绝。某些 grant 可以限制
为当前 Turn，或明确选择 session scope。

因此，app-server 并没有消除 sandbox 和审批策略。相反，它把授权交互变成客户端必须正确
实现的协议责任。一个只实现 `turn/start`、却没有处理 server request 的客户端，不是完整的
交互式 Codex 客户端。

## 会话、配置和认证

- Thread 通常持久化到 Codex rollout JSONL，并配合 state/thread-store SQLite 元数据；
  `ephemeral: true` 的 Thread 只存在于内存，`thread.path` 为 `null`。
- `thread/resume` 使用持久化 ID 恢复；`thread/fork` 复制历史并生成新 Thread ID；
  `thread/archive` 移动 rollout；`thread/delete` 是硬删除。
- `config/read`、配置写入 API 和 `config.toml` 仍属于 Codex 配置体系。
- `account/read`、login/logout 和 account notifications 暴露 Codex 的认证状态。ChatGPT managed
  模式由 Codex 完成 OAuth、落盘和 token refresh；app-server 客户端不应另造一套认证状态。
- MCP server 是 Codex runtime 的下游工具来源。app-server 可以查询 MCP server 状态、触发
  OAuth 和转发 elicitation，但这不等于 app-server 自己采用 MCP 协议对客户端服务。

## 稳定 API 与实验 API

`0.146.0` 同时有 stable surface 和 experimental surface：

```bash
# 仅稳定协议，默认
codex app-server generate-json-schema --out DIR
codex app-server generate-ts --out DIR

# 包含实验方法和字段
codex app-server generate-json-schema --experimental --out DIR
codex app-server generate-ts --experimental --out DIR
```

运行时实验客户端必须在第一次 `initialize` 中声明：

```json
{
  "capabilities": {
    "experimentalApi": true
  }
}
```

未 opt in 时调用实验方法或发送实验字段会收到协议错误。Schema 生成结果与执行命令的
Codex 版本严格对应，因此正确的客户端策略是：

1. 固定支持的 Codex 版本或版本范围。
2. 从目标版本生成 bindings/Schema，不复制网页上“最新版”的字段。
3. 默认只依赖 stable Schema。
4. 对实验能力显式协商，并设计字段缺失、方法不可用和断线降级。
5. 把 WebSocket transport 的实验状态与具体方法是否 stable 分开判断。

需要注意：顶层 `codex app-server` 命令本身在 CLI help 中仍标为 experimental，即使协议内
已经划分 stable/experimental surface，也不能由此推导整个集成已经具有长期兼容保证。

## 与 agent-manager 的本质区别

| 维度 | `agent-manager` | `codex app-server` |
|---|---|---|
| 定位 | 多种现有 Agent CLI 的 tmux 编排器 | Codex Core 的结构化客户端协议层 |
| Prompt | 写 tmux buffer、paste、发送 Enter | `turn/start` 的 typed input |
| 状态 | 进程树、终端抓屏、正则和部分 Hooks 推断 | `thread/status/*`、`turn/*`、`item/*` 原生事件 |
| 输出 | 终端文本和 ANSI UI | Agent message delta、命令、文件修改、diff、plan 等结构化 Item |
| Skills | 最终依赖被包装 CLI 对 Prompt 的处理 | `skills/list`、结构化 Skill input、watch/config API |
| 审批 | 依赖原 CLI 当前终端 UI | server-to-client JSON-RPC request |
| 会话恢复 | 捕获 provider ID 后执行 `codex resume/fork` CLI | `thread/resume`、`thread/fork` API |
| 持久运行 | tmux session | 子进程、Unix socket daemon 或实验 WebSocket |
| Provider 范围 | Claude、Codex、Gemini 等多个 CLI | Codex 专用 |
| 稳定性风险 | 终端文案、布局和按键状态变化 | app-server 版本和实验协议变化 |

因此，`agent-manager` 中“把 Skill 名称随 Prompt 粘贴到 tmux”只描述了它驱动现有 Codex
CLI 的外层方式，不能用来解释 Codex app-server 内部怎样发现和加载 Skill。

## 对 SessionMesh 的判断

SessionMesh 当前以 provider JSONL 为观察事实源，以可重建 SQLite 为索引，并要求所有控制
操作由用户显式授权。app-server 对 Codex 集成很有价值，但它改变的不只是数据格式，还会
改变 SessionMesh 的职责边界。

### 可以获得的能力

- 直接获得 Thread、Turn、Item 和 completion 边界，减少轮询 JSONL 和状态推断。
- 可靠接收流式消息、命令、文件变更、diff、plan、token usage 和失败原因。
- 使用 native `thread/resume/fork/interrupt`，不再拼接部分 Codex CLI 命令。
- 使用 `skills/list` 构建准确的 cwd-scoped Skill picker。
- 在 SessionMesh UI 中承载 Codex 的审批和补充提问。

### 新增的责任和风险

- SessionMesh 会从被动 observer 变成 Codex 的控制客户端，需要管理连接、重连、订阅、
  反向请求、审批超时、背压和 app-server 生命周期。
- app-server 只解决 Codex，不会自动给 Claude Code 提供同构协议。
- 顶层命令和 WebSocket transport 仍是 experimental，需要版本锁定和兼容性测试。
- 事件流适合 live control，但不应未经论证就替代现有可离线重建的 JSONL 索引。
- 当前 ADR 选择 detached non-interactive Operation Task worker，并拒绝依赖 tmux/按键注入；
  是否改用 app-server 是新的架构决策，不能作为实现细节偷偷替换。
- 审批 request 必须继续服从“显式授权”，连接 app-server 不等于授权执行所有控制动作。

### 建议的集成边界

```text
ProviderAdapter (passive, existing)
  `-- JSONL discovery / parse / live evidence

CodexAppServerClient (optional, opt-in control plane)
  |-- connection + initialize + version capability
  |-- Thread / Turn / Item event projection
  |-- Skills query
  `-- approval requests -> explicit SessionMesh authorization UI

OperationTask / SuccessorLauncher
  `-- keep existing ownership until a dedicated ADR changes it
```

第一阶段更适合把 app-server 做成 **Codex-only、显式开启的附加控制通道**，继续保留 JSONL
作为离线观察和重建路径。待 daemon 生命周期、断线恢复、审批 UX 和版本矩阵有实证后，再
决定是否让它承接现有 Operation Task 或 successor launch 的职责。

这与现有架构约束一致：

- [`ADR 0001`](adr/0001-provider-adapter-seam.md)：provider 差异应留在 adapter 边界。
- [`ADR 0002`](adr/0002-explicitly-authorized-control.md)：所有控制必须由用户明确授权。
- [`ADR 0003`](adr/0003-detached-operation-task-workers.md)：当前 Operation Task 使用 detached
  worker，不使用 tmux 或终端按键注入。
- [`ADR 0006`](adr/0006-separate-successor-launch-from-terminal-adapters.md)：successor launch
  与 terminal adapter 分离，不能因新增传输方式而重新耦合。

## 最小协议试验

只做研究时，可以让 app-server 使用默认 stdio，然后逐行发送 JSON：

```bash
codex app-server --listen stdio://
```

最小客户端至少需要：

1. 管理 app-server 子进程的 stdin、stdout 和 stderr。
2. 为 request 生成并关联唯一 `id`。
3. 完成 `initialize` / `initialized` 握手。
4. 持续读取 notification，不能在等待某个 response 时停止消费输出。
5. 处理 app-server 发起的 approval/user-input request。
6. 以 `turn/completed` 而不是最后一个文本 delta 判断 Turn 结束。
7. 限制输入、输出队列并处理 `-32001` 和断线。

生产实现不应手写一个长期不更新的协议 enum。应在构建或版本升级流程中从目标 Codex 版本
生成 TypeScript bindings 或 JSON Schema，并用协议 fixture 覆盖握手、Turn、审批、Skills、
中断、重连和未知字段。

## 结论

app-server 的核心价值不是“让 Codex 在后台运行”，而是给 UI 一个结构化、双向、可流式
驱动 Codex Agent runtime 的协议。它把 Thread/Turn/Item、工具执行、Skills、MCP、审批、
配置、认证和持久化统一在 Codex Core 之上。

对之前几个问题，最终答案是：

1. app-server 是 Codex 的 headless client backend，不是模型、MCP 或 tmux wrapper。
2. Codex 的 Agent 能力主要在 Codex Core；app-server 将这些能力变成富客户端可使用的协议。
3. Skills 不只是把名称发到 tmux。发现走 `skills/list`；执行推荐使用文本 marker 加结构化
   `skill` item，由 backend 定位并注入完整 Skill 指令。
4. “Codex = UI + app-server”只适合描述部分部署拓扑。更准确的是
   `client surface -> app-server -> Codex Core -> model/tools`。
5. SessionMesh 可以利用这条协议减少 Codex 的状态推断，但应先作为可选控制通道接入，
   不应破坏被动观察、显式授权和现有 worker ownership。

## 官方资料与可复现依据

### 版本与主文档

- [Codex `rust-v0.146.0` release](https://github.com/openai/codex/releases/tag/rust-v0.146.0)
- [app-server README，固定到 `rust-v0.146.0`](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/app-server/README.md)
- [app-server 源码目录](https://github.com/openai/codex/tree/rust-v0.146.0/codex-rs/app-server)
- [app-server protocol 源码目录](https://github.com/openai/codex/tree/rust-v0.146.0/codex-rs/app-server-protocol)

### 关键实现

- [app-server crate 依赖](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/app-server/Cargo.toml)
- [主 runtime 与 transport/message processor 装配](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/app-server/src/lib.rs)
- [进程内 app-server client/runtime](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/app-server/src/in_process.rs)
- [transport 路由和慢连接处理](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/app-server/src/transport.rs)
- [JSON-RPC message processor](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/app-server/src/message_processor.rs)
- [Skills 文件监听与 cache invalidation](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/app-server/src/skills_watcher.rs)
- [v2 协议类型目录](https://github.com/openai/codex/tree/rust-v0.146.0/codex-rs/app-server-protocol/src/protocol/v2)

### 本机验证命令

```bash
codex --version
codex app-server --help
codex app-server daemon --help
codex app-server generate-json-schema --out /tmp/codex-app-server-schema
codex app-server generate-json-schema --experimental --out /tmp/codex-app-server-schema-experimental
```

本文实际验证版本输出为 `codex-cli 0.146.0`，并重点核对生成结果中的
`ClientRequest.json`、`ServerRequest.json`、`ServerNotification.json`、
`v2/SkillsListParams.json` 和 `v2/SkillsListResponse.json`。

OpenAI Codex manual helper 在本次环境中返回 HTTP 403，已安装的 OpenAI Docs MCP 需要重启
当前 Codex 会话后才能暴露工具。因此本文没有用非官方博客补缺，而是以同版本官方仓库和
本机生成 Schema 为主，避免引用无法验证或跨版本的协议细节。
