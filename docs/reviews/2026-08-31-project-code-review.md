# Mudi 项目代码审查

- 审查日期：2026-08-31
- 被审查仓库 HEAD：`547dece76859206a053185a2453086ca02f7d3bd`
- 冻结快照提交：`1cce45d76c887a77ce524730d9f2b982f9a42349`
- Scope ID：`commit:sha256:fd0dfbf9f3a6bb1f1232600a8eb7e1daf2f7ccb594175598c7426dc1f433530e`
- Graph revision：`sha256:998c1cb056dc49caf5a76ddb6800b5279a9d25ce0ede44bfc403058be852f4c9`
- 结论：确认 23 个问题，其中高严重度 9 个、中严重度 13 个、低严重度 1 个

## 高严重度

### 1. 不完整的进程扫描被当成确定退出

- 严重度/置信度：高/高
- 触发条件：Handoff Worker 正在等待 Busy Current Run 时，`ps`、`pgrep`、`lsof`、
  状态目录或状态文件扫描失败。检测器虽然记录了扫描不完整，却仍把空结果解释为 `Exited`。
- 影响：Handoff 可能在没有确认原 Busy 进程结束的情况下开始生成，绕过必需的等待和进程重验证。
- 证据：[`src/adapters/providers/state.rs`](../../src/adapters/providers/state.rs#L73)
  与 Handoff 观察路径没有保留 `Unknown`；Managed Resume 路径则明确检查了 `scan_complete`。
- 修正方向：在 `StateDetector` 和 `RuntimeControlPort` 中保留 `Unknown`，或者要求所有控制调用方
  在把缺失解释为 `Exited` 之前拒绝不完整扫描。
- 记录：`record-5f9d3c7e0dfc16c1cf65`
- 锚点：`{kind: line, side: new, path: src/adapters/providers/state.rs, line: 1}`

### 2. 重复的 Claude 状态记录会任意选择一个进程用于控制

- 严重度/置信度：高/高
- 触发条件：两个实时 Claude 进程发布了相同原生 Session ID 的状态文件。`detect_claude`
  将它们写入 `HashMap<String, LiveState>`，最后一次 `insert` 覆盖之前的值。
- 影响：授权的结束 Current Run 操作可能向任意选中的进程发送信号，而同一 Session 的另一个实例仍存活。
- 证据：[`src/adapters/providers/state.rs`](../../src/adapters/providers/state.rs#L109)
  会分别验证每个 PID，却不检测重复 Session ID。
- 修正方向：保留所有已验证候选，仅在恰好存在一个候选时暴露可控制 Current Run；多个候选应视为歧义证据。
- 记录：`record-5f9d3c7e0dfc16c1cf65`
- 锚点：`{kind: line, side: new, path: src/adapters/providers/state.rs, line: 1}`

### 3. Cancel Start 把 tmux namespace 当成控制权限

- 严重度/置信度：高/高
- 触发条件：原私有 tmux server 消失后 namespace 被复用，或者 tmux 因连接、权限等错误返回状态码 1。
  取消逻辑只依赖持久化 namespace 执行 `kill-server`，并把所有状态码 1 都视为成功。
- 影响：Mudi 可能终止无关的 tmux server，或在 Provider 仍运行时删除唯一的持久启动注册。
- 证据：[`src/adapters/system/tmux.rs`](../../src/adapters/system/tmux.rs#L112)
  在取消前没有验证已记录的进程身份或 pane/server 归属。
- 修正方向：重新验证精确的进程启动身份和 tmux 成员关系；任何歧义或非零结果都保留注册并进入 Needs Review。
- 记录：`record-89bcb81e28af0778698f`
- 锚点：`{kind: line, side: new, path: src/adapters/system/tmux.rs, line: 1}`

### 4. Cancel Start 与 Binding Receipt 签发存在竞态

- 严重度/置信度：高/高
- 触发条件：后台 binder 正在验证 Binding 注册时，UI 并发开始取消。取消先终止 tmux、后删除注册，
  binder 则可能独立提交 Receipt 和 Managed 记录。
- 影响：在 Receipt 签发前已被取消的启动仍可能获得持久 Managed provenance，并指向刚被取消终止的进程。
- 证据：[`src/application/managed_session/reconciliation.rs`](../../src/application/managed_session/reconciliation.rs#L333)
  没有持久化一个 Receipt 签发流程必须观察的取消声明。
- 修正方向：外部清理前原子转换为 cancellation-in-progress，禁止该状态签发 Receipt，再完成删除。
- 记录：`record-40028f035c19fd473fea`
- 锚点：`{kind: line, side: new, path: src/application/managed_session/reconciliation.rs, line: 1}`

### 5. Handoff 取消会在 Worker 副作用尚不明确时进入终态

- 严重度/置信度：高/高
- 触发条件：Worker 已把 Task 标记为 Running，但还没记录 Provider PID；或已经执行源进程控制，
  但效果尚未持久化。`cancel_task` 看到没有 Provider identity 后立即标记 Cancelled。
- 影响：Task 声称确定取消后，未跟踪的 Provider 进程或源进程终止仍可能发生；Retry 随后可能与旧执行重叠。
- 证据：[`src/application/handoff/application.rs`](../../src/application/handoff/application.rs#L271)
  在没有确认 Worker 消失的情况下，把缺少 Provider PID 当作充分确定性。
- 修正方向：保留取消意图，直到 Worker、源控制、Provider 和结果证据都确认停止；否则进入 Interrupted。
- 记录：`record-27c5ffc8083043a4e21a`
- 锚点：`{kind: line, side: new, path: src/application/handoff/application.rs, line: 1}`

### 6. Detached Worker 不自行执行 30 分钟期限

- 严重度/置信度：高/高
- 触发条件：Detached Worker 启动一个不退出的 Provider 后阻塞在 `wait_with_output`；TUI 关闭后没有 reconciler 请求终止。
- 影响：已授权工作流可能无限越过声明期限，违反 detached lifetime 和有界授权契约。
- 证据：[`src/adapters/handoff_worker.rs`](../../src/adapters/handoff_worker.rs#L580)
  同步等待 Provider，且没有期限计时或自终止路径。
- 修正方向：由 Detached Worker 自己执行持久化期限，发起优雅退出、观察结果，并按证据写入 Failed 或 Interrupted。
- 记录：`record-5fab7e52bf1bc253f339`
- 锚点：`{kind: line, side: new, path: src/adapters/handoff_worker.rs, line: 1}`

### 7. 重复启动 Successor 会把一个进程与另一个 Session 配对

- 严重度/置信度：高/高
- 触发条件：Ready Task 启动进程 A 并关联 Session A 后再次启动。第二次启动覆盖 PID，
  但 `successor_session_id IS NULL` 条件阻止替换旧 Session link。
- 影响：权威 Task 元数据会把进程 B 与 Session A 表示成已证实关系，实际从未存在该关系。
- 证据：[`src/adapters/persistence/tasks.rs`](../../src/adapters/persistence/tasks.rs#L532)
  覆盖进程和退出字段，却没有启动基数约束，也没有把退出更新绑定到进程身份。
- 修正方向：原子声明每个 Task 仅能启动一次；若支持多次启动，则为每次启动单独持久化进程、退出和 Session 证据。
- 记录：`record-cc5e1b543ed5238641b8`
- 锚点：`{kind: line, side: new, path: src/adapters/persistence/tasks.rs, line: 1}`

### 8. Session metadata 保存会擦除并发 Alias 更新

- 严重度/置信度：高/高
- 触发条件：两个连接分别加载元数据、修改不同 Session，再并发保存。每次保存都会删除所有行，
  然后重新插入各自过期的内存快照。
- 影响：后提交者会静默擦除另一方的 Alias、Description 或 curation 更新，包括启动关联的 Session Title。
- 证据：[`src/adapters/persistence/session_metadata.rs`](../../src/adapters/persistence/session_metadata.rs#L405)
  在每次保存事务中无条件执行 `DELETE FROM session_metadata`。
- 修正方向：使用行级 UPSERT/delete，或在同一事务内读取并应用变更的 compare-and-swap。
- 记录：`record-63981a9f3064425d2941`
- 锚点：`{kind: line, side: new, path: src/adapters/persistence/session_metadata.rs, line: 1}`

### 9. New 在没有显式用户选择时强制启用 Binding Initializer

- 严重度/置信度：高/高
- 触发条件：用户填写可见 New 字段并授权启动。表单没有 Binding Initializer 选项，
  但请求构造器和应用服务都把它强制设为 `true`。
- 影响：Mudi 向 Provider 发送用户没有显式启用的输入，违反 Binding Initializer 默认关闭且必须 opt-in 的契约。
- 证据：[`src/presentation/tui/mod.rs`](../../src/presentation/tui/mod.rs#L339) 构造
  `binding_initializer: true`；[`start_new.rs`](../../src/application/managed_session/start_new.rs#L50)
  又在服务层覆盖一次。
- 修正方向：在 New 中增加明确披露、默认关闭的选择，完整传递该选择，并停止服务层覆盖。
- 记录：`record-9c5c8a99e4bf41b01d68`
- 锚点：`{kind: line, side: new, path: src/presentation/tui/mod.rs, line: 1}`

## 中严重度

### 10. Claude 缺少进程启动证据时仍被标记为 Open

- 严重度/置信度：中/高
- 触发条件：Claude 状态文件缺少 `startedAt`，但其中 PID 恰好指向任意实时进程。
  `process_matches` 退化为仅检查存活并返回 true。
- 影响：过期 Session 会基于从未匹配身份的进程显示为 Busy/Idle，产生错误状态、Alert 和 Signal。
- 证据：[`src/adapters/providers/state.rs`](../../src/adapters/providers/state.rs#L193)
  对缺少期望启动时间明确执行 `None => Some(true)`。
- 修正方向：仅在存在记录的启动时间且匹配成功时发布 Busy/Idle。
- 记录：`record-5f9d3c7e0dfc16c1cf65`
- 锚点：`{kind: line, side: new, path: src/adapters/providers/state.rs, line: 1}`

### 11. Provider token 计数可能在 Usage 聚合时溢出

- 严重度/置信度：中/高
- 触发条件：Provider 提供的各个计数都是合法 `u64`，但跨消息、类别或模型桶求和超过 `u64::MAX`。
- 影响：debug 构建在刷新时 panic；release 构建回绕为看似有效的小 Usage 值。
- 证据：[`src/domain/usage.rs`](../../src/domain/usage.rs#L13) 在 `add` 和 `total` 中使用未检查加法。
- 修正方向：在 Provider 解析边界使用 checked arithmetic，溢出记录应被拒绝或产生诊断，不能回绕或静默饱和。
- 记录：`record-49b8091d638ce54fe6b4`
- 锚点：`{kind: line, side: new, path: src/domain/usage.rs, line: 1}`

### 12. 部分 Provider 遍历被误当成源文件删除

- 严重度/置信度：中/高
- 触发条件：配置的 Provider 根目录或子树暂时不可读。`WalkDir` 错误被丢弃，刷新随后把不完整路径集当成完整结果。
- 影响：已有索引行被 prune，有效 Session 从仍显示为 Fresh 的 Snapshot 中消失。
- 证据：[`src/adapters/providers/mod.rs`](../../src/adapters/providers/mod.rs#L276)
  使用 `filter_map(Result::ok)`，刷新随后无条件调用 `prune_missing`。
- 修正方向：返回发现完整性和诊断，跳过受影响范围的 prune，并发布 degraded Snapshot。
- 记录：`record-da52fb9d88b2318f8ea9`
- 锚点：`{kind: line, side: new, path: src/adapters/providers/mod.rs, line: 1}`

### 13. 启动前采集的 PID 0 样本会终结成功注册

- 严重度/置信度：中/高
- 触发条件：Collector 在 broker 写入 launch evidence 前读到 Binding 注册并用 PID 0 占位；
  启动成功后，reconciliation 又把这个旧样本应用到新的真实 PID。
- 影响：有效启动会进入 Needs Review 或 Failed，无法自动 Binding，但 Provider 仍在私有 tmux 中运行。
- 证据：[`src/application/observation/collect.rs`](../../src/application/observation/collect.rs#L90)
  采集 PID 0；reconciliation 重载当前注册时没有验证样本是否属于相同启动身份或 generation。
- 修正方向：每个样本必须绑定精确 launch identity 或注册 generation；旧样本和无证据样本只能返回 Waiting。
- 记录：`record-40028f035c19fd473fea`
- 锚点：`{kind: line, side: new, path: src/application/managed_session/reconciliation.rs, line: 1}`

### 14. 已逾期且进程死亡的执行会永久停在 Running

- 严重度/置信度：中/高
- 触发条件：首次 reconciliation 已晚于 `running_deadline_ms`，且 Provider PID 缺失或已经死亡。
  deadline 分支直接 continue 或不做转换，后面的存活恢复分支无法执行。
- 影响：Task 无限保持非终态，并阻止同一源 Session 创建任何新的 Operation Task。
- 证据：[`src/application/handoff/application.rs`](../../src/application/handoff/application.rs#L378)
  在 deadline 分支中只处理已验证的实时 Provider。
- 修正方向：仅对实时 Provider 请求终止；缺失或死亡证据应落入正常 Failed/Interrupted 分类。
- 记录：`record-27c5ffc8083043a4e21a`
- 锚点：`{kind: line, side: new, path: src/application/handoff/application.rs, line: 1}`

### 15. Artifact 发布会留下无引用的敏感内容

- 严重度/置信度：中/高
- 触发条件：Worker 原子 rename `handoff.md` 后，launch plan 构建或 `mark_ready` 因取消、
  其他状态转换或 SQLite 故障失败。
- 影响：Task 仍为 `artifact_path = NULL`，显式删除 Task 时只删除记录，私有工作上下文文件永久成为孤儿。
- 证据：[`src/adapters/handoff_worker.rs`](../../src/adapters/handoff_worker.rs#L330)
  先发布文件、后提交 Ready metadata，失败时没有补偿路径。
- 修正方向：提交失败后重新读取 Task；仅当同一 Task 已用相同路径进入 Ready 时保留文件，
  否则安全删除或持久化可供重启 reconciliation 完成的清理意图。
- 记录：`record-5fab7e52bf1bc253f339`
- 锚点：`{kind: line, side: new, path: src/adapters/handoff_worker.rs, line: 1}`

### 16. Task 持久化会损坏非 UTF-8 文件系统路径

- 严重度/置信度：中/高
- 触发条件：source、工作目录、可执行文件或 Artifact 路径包含 macOS 允许的非 UTF-8 字节。
  TaskStore 用 `to_string_lossy` 写入 TEXT，再从变化后的字符串重建 `PathBuf`。
- 影响：Detached Worker 可能观察错误 source，或使用不再指向已创建 Artifact/可执行文件的 launch plan。
- 证据：[`src/adapters/persistence/tasks.rs`](../../src/adapters/persistence/tasks.rs#L23)
  对相关路径字段全部使用有损转换。
- 修正方向：使用 SQLite BLOB 等可逆的平台字节表示持久化路径，并无 Unicode 转换地重建 `OsString`/`PathBuf`。
- 记录：`record-cc5e1b543ed5238641b8`
- 锚点：`{kind: line, side: new, path: src/adapters/persistence/tasks.rs, line: 1}`

### 17. 架构门禁漏掉 Application 文件系统边界

- 严重度/置信度：中/高
- 触发条件：Application 代码直接引入 `std::fs`、`tokio::fs`、`walkdir` 或 `dirs`。
- 影响：尽管仓库把文件系统实现归属 adapters，CI 仍会通过，已声明的向内依赖规则可能无提示地回退。
- 证据：[`scripts/check_architecture.py`](../../scripts/check_architecture.py#L54)
  在 Application 中禁止进程构造和外层具体依赖，却没有文件系统模式。
- 修正方向：把文件系统实现模式加入 `APPLICATION_PATTERNS`，并增加正反向架构检查测试。
- 记录：`record-03a49d83787054ba8306`
- 锚点：`{kind: line, side: new, path: scripts/check_architecture.py, line: 1}`

### 18. 可执行包没有提交 Cargo lockfile

- 严重度/置信度：中/高
- 触发条件：registry 出现符合宽泛版本范围的新依赖版本后重新构建。
- 影响：CI 和发布构建可能解析出不同于审查版本的依赖图，把未经审查的传递依赖变化带入二进制。
- 证据：[`Cargo.toml`](../../Cargo.toml#L1) 定义可执行包；[`.gitignore`](../../.gitignore#L1)
  明确忽略 `Cargo.lock`，仓库中也没有跟踪该文件。
- 修正方向：生成并提交 `Cargo.lock`，CI 使用 locked dependency resolution。
- 记录：`record-ffaeab2306f1313043ae`
- 锚点：`{kind: line, side: new, path: Cargo.toml, line: 1}`

### 19. Task 选择在刷新之间只按列表索引保存

- 严重度/置信度：中/高
- 触发条件：用户选中一条 Task 后列表刷新并重排。选择状态只做数字范围收缩，原索引现在指向另一条 Task。
- 影响：按 Enter 可能打开并 Acknowledge 用户没有选择的另一个终态 Task。
- 证据：[`src/presentation/tui/mod.rs`](../../src/presentation/tui/mod.rs#L1183)
  保存 `task_selected: usize`，`open_selected_task` 也仅按该索引解析当前项。
- 修正方向：按 Task ID 持久选择并在最新 Snapshot 中重新解析，索引只作为渲染细节。
- 记录：`record-9c5c8a99e4bf41b01d68`
- 锚点：`{kind: line, side: new, path: src/presentation/tui/mod.rs, line: 1}`

### 20. Session Detail 漏掉 cache read 和 cache creation Usage

- 严重度/置信度：中/高
- 触发条件：Session 存在非零 `cache_read` 或 `cache_write` Usage，用户打开 Detail。
- 影响：Current Run、Transcript Lifetime 和 per-model 行都隐藏了领域契约要求保持独立的 Provider Usage。
- 证据：[`src/presentation/tui/detail.rs`](../../src/presentation/tui/detail.rs#L372)
  只渲染 input、output 和 cost，虽然 `Usage` 保留了两个 cache 类别。
- 修正方向：在两个 scope 和 per-model 分解中分别渲染 cache read 与 cache creation。
- 记录：`record-ba3239902cd6fa90b4ae`
- 锚点：`{kind: line, side: new, path: src/presentation/tui/detail.rs, line: 1}`

### 21. Unicode 截断按字符数而不是终端列宽计算

- 严重度/置信度：中/高
- 触发条件：Project path、Session title 或 compaction trigger 包含双列宽字符，并在窄面板渲染。
- 影响：生成行超出 `Rect`，Project 计数或 Recent Signal 尾部的 `Busy -> Idle` 等状态证据可能被裁掉。
- 证据：[`src/presentation/tui/text.rs`](../../src/presentation/tui/text.rs#L18)
  使用 `chars().count()`，而调用方传入的是终端列宽预算。
- 修正方向：用 `UnicodeWidthChar`/`UnicodeWidthStr` 实现两种截断方向，并增加窄宽 CJK 渲染用例。
- 记录：`record-415a40aa3ca64bd552c8`
- 锚点：`{kind: line, side: new, path: src/presentation/tui/text.rs, line: 1}`

### 22. 启动时 Binding registry 读取失败会禁用重启安全 reconciliation

- 严重度/置信度：中/中
- 触发条件：初始 `binding_registrations` 枚举短暂失败，但后续 Collector 访问成功；启动失败已被转换为空重启集合。
- 影响：重启前的 Binding 会被当成本进程新建，确定缺失策略可能清理其启动环境并标记 Failed，而不是 Needs Review。
- 证据：[`src/application/observation/reconcile.rs`](../../src/application/observation/reconcile.rs#L41)
  使用 `.ok().and_then(... .ok()).unwrap_or_default()`，永久丢失错误与空结果的区别。
- 修正方向：保留加载错误并显示诊断；成功加载已有注册集合前，阻止破坏性 Binding reconciliation。
- 记录：`record-7bc5d8b842edcd874cf6`
- 锚点：`{kind: line, side: new, path: src/application/observation/reconcile.rs, line: 1}`

## 低严重度

### 23. Project Signal 漏掉已移除成员的当前 Label

- 严重度/置信度：低/高
- 触发条件：源文件删除清除了 Project risk，被移除 Session 只保留在 Project Signal 的 `before` 成员中。
- 影响：保留的历史观察退回 Short Session ID，无法解析该 Session 当前的 Alias 或 Generated Label。
- 证据：[`src/application/observation/project.rs`](../../src/application/observation/project.rs#L126)
  为 Session Signal 提供 identity，却对 Project Signal 明确返回 `None`；renderer 本身已支持解析 before/after 成员。
- 修正方向：把 Project Signal 的全部 before/after 成员 identity 加入 metadata projection。
- 记录：`record-e29be53c22ca11254bce`
- 锚点：`{kind: line, side: new, path: src/application/observation/project.rs, line: 1}`

## 验证结果

以下命令均在仓库根目录通过：

```text
cargo fmt --check
python3 -m unittest scripts/test_check_architecture.py
python3 scripts/check_architecture.py
cargo clippy --all-targets -- -D warnings
cargo test
git diff --check
```

这些检查证明格式、lint、架构检查和现有测试均健康。上述问题主要位于现有测试尚未覆盖的并发交错、
不完整 OS 观察、持久化失败边界和 UI 状态路径。

## 覆盖范围与排除项

- 审查单元：Observation、Managed Control、Handoff Tasks、Platform Metadata、UI Interaction、UI Rendering。
- 双角色状态：完整；每个单元的 internal-behavior 与 external-contract 均已完成。
- 变更记录覆盖：163/163，无未覆盖记录。
- 审查使用列出 HEAD 对应的不可变跟踪源码快照，未审计完整 Git 历史或外部服务。
- 用户原有的四项未提交文档修改不在审查快照内，本次也没有改动它们。
- 审查期间没有修改源码或测试文件。
