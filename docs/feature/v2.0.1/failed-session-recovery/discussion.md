# 失效 Session 恢复 - 需求讨论

**Status:** analyzing
**Release:** v2.0.1
**Feature:** failed-session-recovery
**Scope status:** standalone candidate small requirement within Work Continuity; it does
not expand the accepted Work Continuity boundary until the user confirms that amendment.

## 核心价值

当 Source Session 因 Current Context 超限、压缩失败、Provider CLI 卡死、Skill
无法解析或其他原因而不能正常交互时，Mudi 仍可从本地证据恢复其有效工作
上下文，生成 Handoff Artifact，并让用户启动一个新的 Successor Session 继续工作。

该能力不依赖 Source Session 再回答一次问题。它提供的是故障恢复，而不只是正常交接。

## 已验证的人工恢复案例

一次 Claude Code Session 在约 36.2 万 token 时无法完成 `/compact`，继续恢复原
Session 也不可靠。一个独立、健康的 Codex Session 完成了以下流程：

1. 直接读取 Source Session 的 JSONL，检查记录结构、最近有效消息、使用量和中断点。
2. 读取 Source 工作目录的 Git 状态、已有设计文档和相关代码证据。
3. 不复制完整 Transcript 或大段工具输出，只提取目标、当前状态、决策、已完成工作、
   剩余事项、验证证据、风险和下一步。
4. 在 Source Project 之外生成精简的 Handoff Artifact。
5. 保留旧 JSONL 原件，不覆盖、不截断，也不要求失效 Session 自己生成摘要。
6. 创建新的 Claude Session，让它读取 Artifact 和 Source 工作目录中的权威文档。
7. 验证新 Session 能准确说明原任务断点和第一项具体行动后，才把它交给用户继续。

案例结果：旧 Session 约 3.7 MB，新 Session 初始上下文约 3.8 万 token，并成功
恢复到正确的工作断点。

## 与当前 Handoff 的区别

当前 Handoff 生成路径会恢复 Source Session，并要求 Source Provider 执行托管
Handoff Skill。该路径适合 Source Session 仍能正常处理 Prompt 的情况，但在以下
场景中可能失效：

- Current Context 已超过 Provider 或推理网关上限；
- `/compact` 本身也无法执行；
- Provider CLI 仍退出为成功，但实际没有执行 Handoff Skill；
- Source Session 的本地命令、Skill 或工具状态损坏；
- Source Session 已无法稳定地产生符合 Artifact contract 的输出。

本功能新增一条独立恢复路径：由一个健康的、与 Source Session 生命周期隔离的生成
执行读取本地证据并生成候选 Artifact。Source Session 的 JSONL 是观察证据，而不是
必须再次运行的执行环境。

## 建议工作流

```text
Source Session JSONL
        +
Source 工作目录的只读状态和权威文档
        +
Provider/Process/Task 元数据
        |
        v
独立 Handoff 恢复生成路径
        |
        v
候选 Markdown -> Artifact contract 校验 -> 原子发布
        |
        v
Successor Launch Plan -> 用户显式启动 -> 进程证据关联 Successor Session
```

### 1. 授权

- 用户必须明确选择 Source Session 并授权一次恢复 Operation Task。
- Attention Condition、Current Context 阈值或失败检测只能推荐恢复，不能自动启动。
- 授权只覆盖这一次 Artifact 生成，不自动授权结束 Current Run 或启动 Successor。

### 2. 获取稳定证据

- 记录 Source Session、Provider、由工作目录派生的 Project、精确工作目录和源 JSONL 路径。
- 以明确的记录边界读取 JSONL，忽略边界之后追加的数据。
- 读取 Source 工作目录时保持严格只读。
- 优先识别已经落盘的 PRD、ADR、设计文档、测试结果和 Git diff；这些证据通常比
  Transcript 中重复的讨论更权威。
- 不把完整 Transcript、无关工具输出或秘密材料送入 Artifact。

### 3. 独立生成

- 生成过程不得依赖 `resume Source Session` 成功。
- 生成器可以使用健康的 Provider Session，但必须与 Source Session 的上下文和进程
  生命周期隔离。
- 长 Transcript 必须结构化筛选和分段读取，不能直接整体注入新的模型上下文。
- 候选输出仍须满足现有 Handoff Artifact contract。

### 4. 验证与发布

- Worker 校验固定标题、必填内容、UTF-8 和秘密材料规则。
- 额外检查 Artifact 是否包含可执行的 `Next Action` 和 Source 工作目录的实际状态。
- 只有校验成功后，才能由 Worker 在 Mudi 私有数据目录原子发布 Artifact。
- Task Store 只保存结果元数据和 Artifact 路径，不保存正文。

### 5. 启动与接手确认

- Artifact Ready 与启动 Successor 保持为两个独立用户动作。
- Successor Launch Plan 显式授予 Provider 对 Artifact 所在目录的只读访问。
- Successor 的首次 Prompt 要求读取 Artifact，并简要返回 Objective、Current State 和
  Next Action，用于确认接手质量。
- Successor Session 只能通过已启动进程的 Provider 证据进行关联，不能通过工作目录、
  时间戳或预分配 ID 猜测。

## 两级产品能力

### 正常交接

Source Session 可正常响应时，继续使用托管 Handoff Skill，利用其已有上下文直接生成
候选 Artifact。这条路径成本较低，信息密度通常更高。

### 故障恢复

Source Session 无法正常响应，或正常交接已出现可恢复失败时，向用户提供独立恢复。
它从 JSONL、Source 工作目录和权威文档重建上下文，不要求 Source Session 再运行。

故障恢复不应在正常交接失败后静默自动执行；它是具有不同证据来源和成本的独立、
显式授权 Operation Task。

## 状态和错误要求

失败必须保留最具体的原因，不能全部归一为 `invalid_artifact`。至少需要区分：

- Source record 不可读或格式损坏；
- 没有找到可恢复的有效工作上下文；
- 独立生成 Provider 启动失败；
- 生成超出上下文限制；
- 候选 Artifact contract 校验失败，并保留具体校验原因；
- Artifact 发布失败；
- Successor 接手确认失败；
- 生成结果存在，但执行结果无法确定。

Provider stdout、stderr 和完整 Transcript 不进入 Task Store；但 Task 应保存归一化错误码
和不包含敏感内容的具体失败阶段。

## 安全和既有 ADR 约束

- 遵守 ADR 0002：恢复必须显式授权，不能由阈值自动触发。
- 遵守 ADR 0003：使用 detached Operation Task Worker，并进行进程和结果协调。
- 遵守 ADR 0004：如果需要结束 Open Current Run，必须单独披露授权并安全结束；读取
  JSONL 本身不构成结束 Current Run 的授权。
- 遵守 ADR 0005：Artifact 保存在 Mudi 私有数据目录，不写入 Source Project。
- 遵守 ADR 0006：Artifact Ready 与终端启动解耦。
- 遵守 ADR 0007：人工案例使用了预分配 Claude Session ID，但产品实现不能照搬；必须
  使用实际启动进程证据关联 Successor Session。

## 验收场景

1. Source Session 可正常响应时，正常交接仍能成功。
2. Source Session 因上下文超限无法响应时，独立恢复能生成有效 Artifact。
3. Source Session 的 Handoff Skill 未识别时，用户能看到具体失败原因并选择独立恢复。
4. Source JSONL 持续增长时，恢复使用固定读取边界，不产生新旧记录混合的 Artifact。
5. Source 工作目录有未提交修改时，Artifact 能准确记录相关文件和验证状态，但不修改它们。
6. Source JSONL 不完整或没有足够上下文时，Task 明确失败，不伪造恢复成功。
7. Successor 读取 Artifact 后能复述 Objective、Current State 和 Next Action。
8. 旧 Session JSONL 保持不变，新旧 Session 不共享 Provider-native Session ID。
9. Mudi 重启后仍能从 Task Store、进程证据和 Artifact 结果协调任务状态。

## 待设计问题

- 独立生成路径使用哪个 Provider，以及用户是否可以选择不同于 Source Provider 的生成器？
- 如何在不整体加载长 Transcript 的情况下确定最小、稳定、可审计的证据集合？
- 是否需要把独立生成执行定义成新的正式领域术语；如需要，应通过 domain modeling 更新
  `CONTEXT.md`。
- 接手确认是 Artifact 生成的一部分，还是 Successor Launch 后的独立验证状态？
- 正常交接失败后，UI 如何解释两条路径的成本、权限和信息质量差异？
- 如何测试 Context 超限、损坏 JSONL、并发追加和 Provider 命令误解析等真实故障？

## 非目标

- 修复、截断或重写失效 Source Session 的 Provider-native JSONL；
- 自动替用户结束 Busy Current Run；
- 自动启动 Successor Session；
- 复制完整 Transcript 到 Artifact；
- 根据最新文件时间猜测 Successor Session；
- 把恢复功能变成无人授权的后台自动化。

## Session Log

### 2026-08-10

- 将原 `new_feature/失效Session恢复功能.md` 迁移为 v2.0.1 下的候选小需求讨论稿。
- 保留其独立恢复路径、证据边界、显式授权和 Handoff Artifact 约束；尚未修改
  `feature/v2.0.1/README.md` 的已接受范围。
- 确认以 Work Continuity 下的独立小需求继续分析，而非新建 v2.x 版本或直接进入开发。
