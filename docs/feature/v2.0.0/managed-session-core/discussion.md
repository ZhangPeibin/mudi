# Managed Session Core - 需求讨论

**Status:** ready-for-prd
**Release:** v2.0.0
**Feature:** managed-session-core
**Scope status:** `feature/v2.0.0/README.md` is the accepted capability boundary. This
discussion refines a later PRD and must not silently expand that boundary.

## 问题

Mudi 已能被动观察 Claude Code 与 Codex CLI 的 provider-native Session，但仅凭
JSONL、PID 或终端名称，无法安全地把一个正在运行的 Provider 进程当作可控制对象。开发者
缺少一个可靠入口来启动、辨认、重新访问并精确恢复自己的本地 Coding Session；TUI 重启、
PID 重用或外部终端启动都会使“我是否仍有控制权”变得不确定。

## 用户与结果

面向同时在多个 Project 与 Worktree 中使用 Codex CLI 或 Claude Code 的开发者。

完成后，开发者能够：

- 在已选择的 Project/Worktree 中显式启动一个 Managed Session；
- 始终区分 Provider-native Session、Current Run 与 tmux 运行环境；
- 确认创建后，自动在独立原生终端中进入该 Session，并在前一个 Run 已退出时精确恢复同一原生
  Session；
- 在 Mudi 正常退出或重启后，看见已验证的受管状态，而不是由名称、时间或目录猜测；
- 继续观察外部启动的 Session，但不会误以为它们已受 Mudi 控制。

## 当前体验

当前代码已具备 Provider JSONL 观察、Project/Worktree 解析、live-process 证据，以及
Handoff Operation Task 的部分基础。它尚未建立由 Mudi 启动并持有的 Managed
Current Run、私有 tmux 环境或这三者的启动绑定证据。

因此，观察到一个 Session 不等于能控制它；在 TUI 重启后，也不能通过“最新的进程”或
目录位置安全地重新取得控制权。创建流程中的首条 Prompt 也不属于 Mudi；它由开发者
进入原生 Provider CLI 后输入。该 Prompt 让 Provider 创建 native Session 与其 source JSONL；
在它发生前，Mudi 没有可用于绑定的 Provider source record。

## 范围

### In Scope

- 通过全局快捷键 `n` 执行 Provider-native `New`：已选中的 Project 或 Worktree 作为创建上下文；
  没有选择上下文时，先选择本地目录。目录选择复用成熟、公开的第三方库。
- 对于 `New`，提供默认关闭、由开发者显式启用的 Binding Initializer；它可在开发者输入真实任务前
  促使 Provider 写出 source JSONL，从而加速绑定。
- 选择 Project，以及 Git Project 中的特定 Worktree。
- 以 Codex CLI 或 Claude Code 的原生 `New` 创建一个 Session，并保存开发者给出的 Title 和
  Description；Title 是创建前必须填写的 Session Alias。Binding Receipt 验证后，该 Session 才成为
  Managed Session。
- 为每个 Managed Session 创建私有 tmux 执行环境。
- 用启动、Provider、进程与 source-record 证据，绑定 Project、Worktree、Provider、
  native Session ID、Current Run 和 tmux pane。
- 列出、检查并从外部终端 attach/detach 已验证的 Open Managed Current Run；被动观察外部启动的
  Session 继续可见。
- 在上一个 Current Run 已退出时，显式授权精确恢复同一 provider-native Session，并形成新的
  Managed Current Run。
- 正常退出 TUI 时保留 Managed Session 的 tmux 环境；重启后重新协调 Session、Current Run
  和进程状态。

### Out of Scope

- Execution Plan、Workflow Run、依赖调度与自动 Session-to-Session 协作。
- Skill-backed Action dispatch、结构化 Action Result、`needs_input` 与自动后继 Action。
- 把外部启动的 Current Run 变为可控对象，或通过 tmux 名称、目录、时间戳猜测身份。
- 由 Mudi 接收、存储或投递开发者的首条 Prompt，或提供对已有 Managed Session 的通用 Prompt
  发送接口。仅 New 时、由开发者显式启用的 Binding Initializer 是受限例外。
- 长驻 Mudi daemon、内嵌终端模拟器、MCP/Agent 回调通道。
- 依赖 Codex app-server 作为共同的 Managed Session 基础。

## 所需行为

1. **New 与登记**：全局快捷键 `n` 执行用户可见的 `New` 流程。若 Project Tree 已选中 Worktree，创建表单
   以其作为上下文预填；若只选中 Project，则默认其主 Worktree，开发者仍可在表单中切换。若没有
   选择上下文，系统先通过成熟、公开的第三方目录选择库要求开发者选择本地目录。开发者选择 Provider、
   补齐 Project/Worktree，填写必填的 Title；Description 可选，且可在创建后补充或编辑。Provider 每次
   都必须由开发者显式选择，不记忆或预选上次使用的 Provider。显式确认后，
   系统创建持久的 Managed Launch Registration，并通过 Provider launch adapter 执行对应 Provider 的原生
   `New`。开发者可同时显式启用默认关闭的 Binding Initializer；它只为促使 Provider 生成 source JSONL，
   不是开发者任务。在没有 Provider-native 无对话初始化能力时，它固定发送下列内容，且不附带 Title、
   路径、Session ID 或其他动态数据：

   ```text
   Mudi initialization only.
   Do not inspect files, run commands, or make changes.
   Reply with exactly: READY.
   ```

   Registration 记录这次启动意图和最小证据；它不是 Session，也不授予控制能力。
   在 Binding Receipt 签发前，开发者可显式执行 `Cancel Start` 来停止本次 Provider/tmux 启动并移除
   Managed Launch Registration。
   目录选择器接受任意本地目录：Git 子目录必须解析到其 canonical Worktree 根目录与 Project；非 Git
   目录本身是 Project。无法 canonicalize 或无法可靠解析 Git 身份时，系统阻止 New 并报告错误，不猜测
   父目录、Project 或 Worktree。
2. **启动与绑定**：Launch Broker 在私有 tmux 中执行对应 Provider 的原生 `New` 命令。只有 Provider
   source record、启动进程与 native Session ID 全部形成一致证据，Evidence Binder 才签发 Binding
   Receipt，并原子创建 Managed Session 与 Managed Current Run。tmux 名称或 pane 不是 Session
   identity。开发者在原生 CLI 输入第一条 Prompt 前，Provider 尚未写出 source JSONL；Managed Launch
   Registration 必须保持 `Binding`，不进入 Project Tree，也不得被当成 Session。若开发者启用了
   Binding Initializer，Provider adapter 可在此时触发它来产生 source JSONL；它仍须经过完整证据验证，
   不能直接升级为 Managed。
3. **访问**：确认创建后，首个 Terminal Access Adapter 自动打开独立的 macOS `Terminal.app`
   窗口，并 attach 到本次 Managed Launch Registration 所创建的私有 tmux。开发者在该窗口直接使用
   Codex/Claude；Mudi TUI 留在原窗口。终端在 tmux 与 Provider CLI 启动后立即打开，
   不等待 Binding Receipt；绑定在后台继续。该终端不是内嵌终端，detach 或正常关闭 TUI 不会
   终止 tmux 环境或 Provider 进程。若 Terminal.app 打开或 attach 失败，开发者可显式执行
   `Retry Open Terminal`；它只重试终端访问，不重启 Provider、不重建 tmux，也不重复绑定。对于
   已验证且仍 Open 的 Managed Current Run，开发者还可使用 `Copy Attach Command`，复制
   `mudi attach <opaque-run-ref>` 后在自己已有的终端执行。该命令由 Mudi 解析并重新
   验证目标后才 attach；它不暴露 tmux 名称或 socket 路径，不接管当前终端，也不启动 Provider。
4. **精确恢复**：用户可见的 `Resume` 直接调用对应 Provider 的原生恢复命令，并以指定 native Session
   ID 启动新的 Managed Current Run。已验证为 Open 的 Managed Current Run 只能重新打开终端进入，不能
   Resume 或被自动结束。只有已验证为 Exited 的同一 native Session 才显示 Resume；开发者再次显式授权后，
   Launch Broker 执行该 Provider 的原生 `Resume`，并保持同一 native Session ID。
5. **重启协调**：重启后的 Mudi 必须能找回它自己创建且已验证的 Managed Session 与
   Managed Current Run。它只根据本机持久的最小管理元数据和实时证据重建关联；任何不确定的结果保持
   未关联，而不是自动接管或重试。
6. **Tree 识别**：Project Tree 的每个已验证 Session 行都显示紧凑的控制权图形标识，区分
   Managed 与 Observed。该标识使用不同图形和颜色，不使用 `M/O` 或文字标签。初版采用双色像素
   印记：闭环编织表示 Managed，单向轨迹表示 Observed；形状本身可在无色条件下区分。该视觉细节
   可作为后续常规 UI 优化调整，不改变来源语义。

## 模块边界

Managed Session 的完整生命周期应由明确、可替换的边界模块组成，而不是由 TUI、tmux 命令和
Provider 特例相互穿透。每个模块只接受其所需的结构化输入，声明允许的副作用，并返回可验证
的结果或明确失败：

```text
用户意图与显式授权
  -> Managed Launch Registration
  -> Launch Broker
  -> private tmux / Provider launch adapter
  -> Provider source + process evidence binder
  -> Binding Receipt
  -> Managed Session registry and restart reconciler
  -> external-terminal access adapter
```

- **Managed Launch Registration** 是持久的内部启动意图：它暂存 Title、Description、请求的
  Provider/Project/Worktree 与启动证据，但在绑定完成前不是 Session。
- **Launch Broker** 是创建与精确恢复的应用边界：它协调启动、绑定和登记，但不处理 Prompt
  内容。
- **Provider launch adapter** 隔离 Claude Code 与 Codex CLI 的命令、启动和 source-record 差异。
- **Evidence binder** 只有在 launch、process 与 Provider source 证据一致时才签发 Binding
  Receipt；它不根据 tmux 名称、路径或时间猜测。Receipt 是创建 Managed Session 和 Managed
  Current Run 的唯一依据。
- **Registry and reconciler** 持久化最小管理元数据，并在重启后依据实时证据恢复已验证关系。
- **Terminal Access Adapter** 的首个实现打开独立的 macOS `Terminal.app` 窗口并 attach/detach；
  对已验证的 Open Managed Current Run，它还提供由 `mudi attach <opaque-run-ref>` 使用的
  受验证 attach 路径。它不取得 Provider 对话控制权，也不向 Provider 投递 Prompt。其他终端
  适配器不属于 v2.0.0。
- 用户所见的 `New` 与 `Resume` 只对应 Provider-native 操作；为提供终端访问而新开、复用或关闭多少
  原生终端窗口是 Terminal Access Adapter 的实现选择，不是另一套用户流程。
- 未来 **Prompt Control/Manager** 依赖已验证的 Launcher/Managed Current Run 边界；它自身拥有
  Prompt 输入、授权、投递和结果语义，不能绕过这些边界。

## 边界与失败情形

- Provider 启动失败、native Session ID 未出现、source record 与启动进程不匹配，或进程身份
  无法验证时，不能签发 Binding Receipt 或把该 Run 标记为 Managed。
- 原生 CLI 已打开但开发者尚未输入第一条 Prompt 时，不存在可绑定的 Provider source JSONL；该
  Managed Launch Registration 保持 `Binding`。它不出现在 Project Tree，也不被误报为启动失败。
- 启用 Binding Initializer 后仍未取得可验证 source record 时，不能假定初始化成功或创建 Session；
  后续状态仍只由进程与证据决定。系统不自动重发，也不提供 `Retry Binding Initializer`；原生终端保持
  可用，开发者可以输入真实任务来产生 source record。
- `Cancel Start` 仅在 `Binding` 时可用。它在停止本次 Provider/tmux 启动后移除 Managed Launch
  Registration；已经写出的 Provider source 保留为被动观察数据，绝不伪装为 Managed Session。`Needs
  Review` 不提供此快捷取消，因为它保留可能仍在运行且证据矛盾的现场。
- 已确认没有 Provider 进程时，Managed Launch Registration 以 `Failed` 结束，并自动清理空的私有
  tmux 环境。
- Provider 进程可能仍在但不能签发 Receipt 时，Managed Launch Registration 进入 `Needs Review`；它保留现场，
  但不出现在正常 Session 列表、不授予 Managed 权限，也不会被自动终止或重试。
- `Needs Review` 只提供 `Open Terminal`（上次终端访问失败时为 `Retry Open Terminal`）和 `Copy
  Diagnostics`。前者仅访问已知 tmux 现场；后者只含 Provider、Project/Worktree、进程/证据状态和错误，
  不含 Prompt 或 Provider 对话。它不提供 Resume、Cancel Start 或接管为 Managed 的操作。
- Terminal.app 打开或 attach 失败不改变 Provider、tmux、Managed Launch Registration 或 Binding
  Receipt 的状态。Mudi 保留具体错误并提供显式 `Retry Open Terminal`；该 Retry 不自动执行，也不
  重试任何 Provider 启动或绑定步骤。
- 已复制的 attach 命令在执行时若其 opaque Run reference 已失效、Run 不再 Open 或证据无法重新验证，
  必须失败且不启动、终止或修改 Provider/tmux。它不能回退为直接暴露 tmux 名称或 socket 路径。
- PID 重用、tmux pane 重用或重启后的模糊证据不能恢复管理关系。
- 发现一个已存在的 Externally Started Current Run 只更新观察状态，不能赋予控制能力。
- 对仍为 Open 的 Current Run 不发起同一 Session 的精确恢复；恢复前必须得到其已退出的验证。
- 已验证为 Open 的 Managed Current Run 只能执行终端访问；Mudi 不会为了 Resume 自动
  结束 Provider 进程或并行启动第二个受管 Run。
- 正常 TUI 退出只 detach；异常退出后的状态必须通过下一次协调确定，不能假定仍然运行或已经
  结束。

## 约束与依赖

- 遵守 ADR 0009：每个 Managed Current Run 必须运行在 Mudi 创建的私有 tmux 环境中。
- 遵守 ADR 0002：启动与恢复都需要一次明确、目标明确的授权；观察、Alert 与阈值不构成授权。
- 遵守 ADR 0001：Provider 差异封装在 adapter 内，产品层不依赖 Provider 私有字段。
- Source JSONL 是观察状态的权威来源；衍生索引可重建。控制权还必须有启动和进程证据。
- 为使 Mudi 重启后找回它自己创建的已验证运行，本机持久化最小管理元数据：
  Project/Worktree、Provider、native Session ID、Title/Description、Managed Current Run 与 tmux
  受管标识、Binding Receipt 和必要时间戳。不得持久化 Prompt、终端正文、Provider 对话、工具
  输入输出或项目文件内容。
- Codex CLI 与 Claude Code 都是本版本的必需路径；Codex app-server 只能是未来可选增强。
- 不持久化 Prompt、Provider 工具参数/输出、终端正文或项目文件内容。
- 未来可由独立 Prompt Control/Manager 通过启动边界接入 Managed Session；除 New 时受限的 Binding
  Initializer 外，本版本不定义开发者 Prompt 的输入、传输或控制接口。
- `Managed Launch Registration`、`Binding Receipt` 与 `Needs Review` 是正式领域术语；它们不得混入
  现有 Session、Managed Current Run 或 Operation Task State 词汇。
- Binding Initializer 默认关闭，只能在 `New` 的显式授权中使用。Provider adapter 应优先使用该 Provider
  已文档化的非对话初始化能力；若没有该能力，只能在本次显式选择后投递固定、系统拥有的初始化消息。
  无论哪种方式，Mudi 都不得保存该消息或 Provider 回复；消息若被采用，仍会保留在 Provider
  自己的原生对话与 source JSONL 中。
- 当前固定 Binding Initializer 消息为：`Mudi initialization only. Do not inspect files, run
  commands, or make changes. Reply with exactly: READY.` 它不得携带动态用户、Project、Worktree、Title
  或 Session 数据。`READY` 或任意 Provider 回复均不构成 Binding Receipt 的证据。
- 没有 Tree 选择上下文时的目录选择必须采用成熟、公开的第三方库；具体库由后续技术方案依据 Rust
  TUI、macOS 与测试兼容性确定，不在产品需求中锁定。

## 已确认决定

- `v2.0.0` 是 Managed Session Core，不是通用 Action Runtime。
- native Session ID 是 Session identity；tmux 只承担受控执行和外部终端访问。
- 一个 Managed Session 可有多个 Current Run；管理来源属于 Run，不能由同一 Session 的历史状态
  推断。
- Handoff 创建的是不同的 Successor Session，不继承源 Session 的身份或本地元数据。
- 模糊证据保持未关联，外部启动的 Run 保持只读观察。
- 创建表单不接收首条 Prompt；Mudi 只启动并绑定原生 Provider CLI，开发者在该 CLI
  中输入 Prompt。
- 用户可见的 `New` 和 `Resume` 分别直接执行所选 Provider 的原生新建与原生恢复；`Managed` 是绑定
  验证后的管理状态，不是用户需要额外理解或选择的第二套 New/Resume。终端窗口数量不改变该语义。
- Binding Initializer 是默认关闭、仅适用于 `New` 的显式选项；它只加速 Provider source JSONL 的出现，
  不接收、存储或替代开发者的真实 Prompt，也不适用于 `Resume` 或已有 Session。
- Binding Initializer 每次 New 最多执行一次；它失败时不自动或手动重试，以免向原生对话重复写入系统
  初始化消息。开发者可以输入真实任务，后续仍按正常证据绑定。
- 只有 `Binding` 中的 Managed Launch Registration 可执行 `Cancel Start`。取消会停止这次启动并清理
  Mudi 的登记与 tmux；任何已写出的 Provider source 继续按只读观察处理。`Needs Review` 没有
  快捷取消或自动终止。
- `Needs Review` 仅允许终端现场检查与无内容的诊断复制：`Open Terminal`/`Retry Open Terminal` 和
  `Copy Diagnostics`。它没有 Resume、Cancel Start、自动重试或接管为 Managed 的入口。
- 创建 Managed Session 必须有开发者提供的 Title；它立即成为 Session Alias，不能回退为
  Short Session ID 或自动生成的名称。
- Description 在创建时可选；它不是 Prompt、当前任务状态或 Provider-native 元数据。
- Description 可在创建后补充或编辑；修改只更新 Mudi 本地元数据。
- Prompt Control/Manager 是后续独立能力。Managed Session 的启动边界应允许将来接入该 seam，
  但 v2.0.0 不暴露 Prompt 投递或控制接口。
- 整个 Managed Session 生命周期采用明确的模块边界：授权、启动、Provider adapter、证据绑定、
  登记/协调与终端访问各自负责自己的输入、允许副作用和可验证输出；后续能力只能通过这些边界
  接入。
- 确认创建后，Mudi 自动打开独立的原生终端并 attach 到私有 tmux；这是进入 Managed
  Session 的主路径，Mudi TUI 保留在原终端。
- v2.0.0 的首个自动终端入口固定为 macOS `Terminal.app`，不引入用户终端偏好设置。
- 终端访问失败与 Provider 启动失败分离：前者可由开发者显式 `Retry Open Terminal`，且 Retry
  只重试终端访问。
- 对已验证且仍 Open 的 Managed Current Run，除 `Open Terminal` 外还提供 `Copy Attach Command`。
  它复制 `mudi attach <opaque-run-ref>`，供开发者在自己已有的终端显式执行；Mudi 必须
  在执行时重新验证目标，且不得把 tmux 内部标识暴露为稳定的对外接口。
- 使用持久的 Managed Launch Registration 记录启动意图；只有 Evidence Binder 签发 Binding Receipt
  后，才原子升级为 Managed Session 与 Managed Current Run。
- 绑定不确定但可能仍有 Provider 进程时，Managed Launch Registration 进入 `Needs Review`，而非产生
  未关联 Session；它不自动接管、终止或重试。
- 同一 native Session 存在 Open Managed Current Run 时只提供终端访问；Resume 仅在该 Run
  已验证 Exited 后出现，并需新的显式授权。
- Mudi 重启后应找回它自己创建且已验证的 Managed Session 与 Managed Current Run；这依赖
  仅存于本机的最小管理元数据和实时证据重新验证，绝不通过名称、时间或目录猜测。
- Project Tree 中每个已验证 Session 行使用非文字的图形加颜色标识其 Managed 或 Observed
  来源；图形本身必须可区分，颜色只用于强化快速扫描。
- 初版来源标识采用双色像素印记：闭环编织表示 Managed，单向轨迹表示 Observed。它们不使用文字，
  在无色时仍可区分；标识的精细视觉可以作为常规 UI 工作继续调整，不能改变其来源语义。
- `n` 是用户可见 `New` 的全局快捷键。Project Tree 有 Project 或 Worktree 选择时，它将选择带入
  创建表单；只选中 Project 时默认其主 Worktree，但仍可切换；没有选择时，先要求开发者通过第三方目录
  选择器选择本地目录。
- New 表单中的 Provider 没有默认值或“上次使用”预填；开发者每次都必须明确选择 Codex CLI 或
  Claude Code。
- 没有 Tree 选择上下文时，目录选择器可接受任意本地目录。Git 子目录自动归入其 canonical Worktree
  根目录和 Project；非 Git 目录本身是 Project。不能可靠解析的路径阻止 New，不做路径猜测。

## 待决问题

暂无。

## 证据

- `feature/v2.0.0/README.md` - accepted v2.0.0 capability boundary.
- `docs/adr/0009-private-tmux-for-managed-current-runs.md` - required private tmux boundary.
- `docs/adr/0001-provider-adapter-seam.md` and `docs/adr/0002-explicitly-authorized-control.md`
  - adapter and authorization constraints.
- [Official Codex CLI documentation](https://developers.openai.com/codex/cli) and
  [Developer commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli) - checked
  on 2026-08-10: `codex` is the interactive new-session entry and `codex resume` restores an
  interactive Session. The documented command reference has no general empty-session
  initialization command, so Binding Initializer behavior must remain Provider-adapter-specific.
- `src/adapters/providers/state.rs` - current read-only live-process evidence and PID-reuse defense.
- `src/adapters/project.rs` - current Project/Worktree resolution.
- `src/application/tasks.rs`, `src/application/handoff/`, and `tests/handoff_tasks.rs` - existing
  Operation Task and Handoff infrastructure that v2.0.0 must not redefine.

## Session Log

### 2026-08-10

- Created the v2.0.0 Managed Session Core pre-PRD discussion from the accepted feature scope,
  current code facts, `CONTEXT.md`, and ADR 0001/0002/0003/0009.
- Recorded that Managed Session identity and control evidence are the central gap; no version
  scope, PRD, issue, or implementation was changed.
- Confirmed launch option A: Mudi starts and binds the native CLI, while the developer
  enters the first Prompt inside that CLI. A future Prompt Control/Manager may integrate at
  the startup seam, but Prompt transport remains outside v2.0.0.
- Confirmed that the full Managed Session lifecycle is modularized at explicit boundaries,
  rather than only adding a launch seam. Future Prompt Control/Manager work must depend on
  those boundaries and cannot bypass identity, authorization, or evidence rules.
- Confirmed that Title is mandatory at creation and immediately becomes the Session Alias;
  no automatic or Short Session ID fallback is allowed for a newly created Managed Session.
- Confirmed that Description is optional at creation.
- Confirmed that Description may be added or edited after creation as Mudi-owned local
  metadata; it never changes the Provider-native Session.
- Replaced the provisional launch-attempt model with a durable Managed Launch Registration and
  Binding Receipt: only a Receipt upgrades a Managed Launch Registration atomically to a
  Managed Session and Managed Current Run. A potentially live but unprovable launch remains
  `Needs Review`, not an unlinked Session.
- Confirmed the primary access flow: after the developer confirms creation, Mudi opens
  a separate native terminal window and attaches it to the Managed Launch Registration's
  private tmux, while leaving the Mudi TUI visible in its original terminal.
- Confirmed that the native terminal opens immediately after tmux and the Provider CLI start;
  Binding Receipt validation continues in the background and must not delay first Prompt entry.
- Confirmed that v2.0.0 opens macOS Terminal.app as its first Terminal Access Adapter; terminal
  preference and additional adapters are deferred.
- Confirmed that a Terminal.app failure preserves the existing backend state and exposes an
  explicit `Retry Open Terminal` action. The Retry is scoped only to terminal access and never
  restarts the Provider, tmux, or binding workflow.
- Confirmed that an Open Managed Current Run can only be re-entered through terminal access.
  Resume appears only after the Run is verified Exited and starts a distinct Managed Current
  Run for the same native Session; v2.0.0 never auto-ends or parallel-resumes it.
- Confirmed that every verified Project Tree Session row uses an interesting non-text
  management-provenance marker with a distinct shape plus color for Managed versus Observed;
  `M/O` and textual labels are excluded from the row. Color must aid quick scanning, while the
  silhouette itself remains distinguishable for users who cannot rely on color.
- Confirmed the `n` creation shortcut. It uses the selected Project or Worktree as its creation
  context; without a selection it first asks for a local directory. The directory picker must
  be supplied by a mature, public third-party library rather than custom-built.
- Confirmed the Project-only default: pressing `n` with a Project selected pre-fills its main
  Worktree. The creation form keeps the Worktree visible and switchable before confirmation.
- Confirmed restart recovery: after Mudi itself exits and reopens, it should find its
  own previously created, verified Managed Sessions and Current Runs. The local registry stores
  only the minimum identity, management, receipt, and timing metadata needed to revalidate that
  relationship; it never stores Prompt, terminal, Provider conversation, tool, or project-file
  content.
- Confirmed formal domain language: `Managed Launch Registration` is the pre-binding launch
  intent and replaces the inaccurate `Managed Run Registration`; `Binding Receipt` is the sole
  conclusive binding record; and `Needs Review` is a Managed Launch Registration status, never
  a Session, Managed Current Run, or Operation Task State. Added all three to `CONTEXT.md`.
- Corrected the user-facing command model: `New` directly invokes the chosen Provider's native
  new-session command, and `Resume` directly invokes its native resume command for the selected
  native Session ID. `Managed` is the evidence-verified management status that follows, not a
  second user-facing command layer; terminal-window allocation is an adapter detail.
- Clarified why the first Prompt is necessary: it remains entirely inside Codex/Claude, but it
  causes the Provider to create the native Session and source JSONL. Before that record exists,
  the Managed Launch Registration remains `Binding` and cannot safely appear as a Session in the
  Project Tree.
- Confirmed the opt-in Binding Initializer for `New`: it is disabled by default and may induce
  the Provider source JSONL before the developer supplies a real task. It remains a narrowly
  scoped adapter capability, not Prompt Control/Manager; it must still produce conclusive
  binding evidence and is never used for Resume. Added the formal term to `CONTEXT.md`.
- Confirmed the fixed Binding Initializer fallback text. It contains no dynamic user or Project
  data, forbids inspection, commands, and changes, and asks the Provider to reply only `READY`.
  Mudi never treats that reply as proof; JSONL, process, and native Session-ID evidence
  remain mandatory.
- Confirmed failure behavior for Binding Initializer: it is one-shot per New. Mudi neither
  automatically resends it nor exposes a manual retry; the native terminal remains available for
  the developer's real task to produce the source record.
- Confirmed `Cancel Start`: it is available only while a Registration is `Binding`, stops the
  known launch and cleans its Mudi-owned tmux/registration state, while preserving any
  Provider source as observation. It is intentionally unavailable in `Needs Review`.
- Confirmed `Needs Review` inspection surface: it offers only terminal access and metadata-only
  diagnostics. Resume, cancel, automatic retry, and promotion to Managed remain unavailable
  until conclusive evidence exists.
- Confirmed Provider selection: every New requires an explicit Codex CLI or Claude Code choice.
  Mudi does not default to or remember the Provider used by a previous launch.
- Confirmed directory selection resolution: any chosen Git subdirectory resolves to its canonical
  Worktree root and Project, while a non-Git directory is its own Project. An unresolvable path
  blocks New rather than guessing a parent directory or identity.
- Selected the initial Project Tree provenance marks without expanding version scope: a two-color
  pixel closed weave for Managed and a two-color pixel open trace for Observed. They do not use
  text and remain distinguishable without color; routine visual refinement may change their
  appearance later without changing the underlying provenance semantics.
- Confirmed re-entry access for a verified Open Managed Current Run: `Open Terminal` remains the
  default Terminal.app path, while `Copy Attach Command` copies `mudi attach <opaque-run-ref>`
  for an explicitly chosen existing terminal. Mudi resolves and revalidates that reference
  when the command runs; tmux names and socket paths are never a public command contract.
- The user confirmed the analysis is ready for a formal PRD and invoked `$to-spec`. The PRD will
  be published as a GitHub Issue for `ZhangPeibin/mudi` with the `ready-for-agent` label.
