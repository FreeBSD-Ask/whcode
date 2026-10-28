# 域 D：会话分支与回滚

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 对话分支 | `runtime/methods/session-fork.ts`。**公共实现** `commitAtomicConversationFork`（`:585-809`）：`parentID: runtime.sessionId`（`:169`）、child 继承 projectID/workspaceID/traceID；消息深克隆 + 一次性预分配 child-local 身份（`createForkIdentityMap` `:219-315`）；**原子提交** `store.commitForkBundle`（`:748-770`）child+message+goal+选型+执行态+commandFact 同一事务；fork 完成父会话 append `SessionForked`（`:773-797`）。`forkConversationBeforeMessage`（`:1091-1117`）是 compact-covered 编辑专用入口 | 无会话分叉/回退 | 空白 |
| 对话回退 | `rewind-message.ts:404-438` → `buildConversationRewindPlan`（`:440-540`）→ `applyConversationRewindPlan`（`:542-626`）。**就地切断**（不建新会话）：`setRevert`（`:566-579`）持久化 `keptMessageIDs`、`branchCutAfterMessageID`、`targetMessageID`，存于 **`session.revert`**；**branchGeneration 递增**（`:529`）持久化 + 写 `runtimeTaskRegistry.setActiveBranchGeneration`（`:580-581`）；旧代后台任务经 `cancelRemovedBranchBackgroundTasks`（`:628-650`）**stop（fail-closed，任一失败抛错不提交）**；派生状态重建 `rebuildConversationDerivedState`（`:675-736`） | 无 | 空白 |
| 文件回滚 | `file-rewind.ts`。三键预览/回滚 `resolveTargetCheckpoints`（`:480-521`）：checkpointId/messageId/turnId；**前像结构**（`contracts/src/rewind/index.ts:46-68,100-124`）：`{checkpointId, messageId, scope, snapshotRef, diffRef?}`，Artifact `kind="workspace_file_before_change"`、`files[]={path, existedBefore, beforeContent(可空), afterContent?, structuredPatch[]}`——**前像是完整 beforeContent + structuredPatch，非仅 diff**；安全回滚 `buildWorkspaceFileRewindPlan`（`:277-474`）按 sha256 校验当前 afterContent 与期望一致（`external_modified` 拒绝 `:432-441`），多文件逆序写 + **journal 补偿**（`:253-275`）；冷恢复经 `workspace-checkpoint-persistence.ts` | `checkpoints` 表为**任务状态机**单步差分（delta_json ≤64KiB + 产物引用），恢复只按引用（检查点行 + 产物元数据 + 消息游标），**不含文件前像** | **精化**：回滚细分两条路径——对话剪枝（session.revert）与工作区文件恢复（artifactStore 前像）。WHCode 检查点是状态机持久化，补齐需新增文件快照表（威胁模型：快照膨胀，须有上限） |
| 崩溃恢复 | 无 | `tool_exec.recover_uncertain`、检查点同事务原子提交（任务状态+事件+检查点三表）、写后裁剪 retain 上限 | **WHCode 先手**（确认） |

## 2. WHCode 先手崩溃恢复确认

- `recover_uncertain`（`crates/whcode-store/src/tool_exec.rs:263-307`）：把 `prepared`/`running`
  标记为 `unknown`，**单事务、幂等、不重放副作用**（tests/crash/README.md 注入点 4 个 +
  标记文件验证行数仍为 1）。
- **三表原子提交确认**：`task_transition`（`crates/whcode-store/src/checkpoints.rs:276`）在
  **同一事务**内 `UPDATE tasks`（`:317`）+ `INSERT events`（`:329`）+ `INSERT checkpoints`，
  任一失败整体回滚（`:270` 注释）。

## 3. 与 WHCode 差异修正（相对原差异书）

- 原判定**基本成立**，一处精化：ZCode 分叉/回退/文件快照三机制经源码确认存在；WHCode
  无对应会话分叉/回退——WHCode 数据模型 `specs/001-whcode-harness/data-model.md` 无
  session fork/revert 字段。
- **WHCode 检查点确为状态机单步差分、不含文件前像**：`checkpoints` 表字段
  `id, task_id, step, delta_json, message_cursor, artifact_refs_json, ts`
  （data-model.md:62），`delta_json` 为任务状态增量、`artifact_refs_json` 为引用，无
  `beforeContent`/snapshot 文件内容。→ 原判“”成立。
- **精化点**：差异书称 rewind“”仅指 conversation scope；ZCode 另有
  `rewindWorkspaceToMessage/rewindWorkspaceToCheckpoint`（rewind.ts:202-343）与
  `rewindCascadeToMessage`（rewind-message.ts:74-119）走 checkpoint artifact 恢复文件。
  域 D 的“”应细分为**对话剪枝**（session.revert）与**工作区文件恢复**（artifactStore 前像）
  两条路径。

## 4. 存疑项

- 差异书“”——实际是**任务 stop**（fail-closed）而非事件丢弃；removed
  分支事件由 `branchGeneration` 门控 + active branch 选择器过滤。`[待验证]` 是否另有事件级
  丢弃路径（未在本次直读范围内）。
- WHCode 无文件前像表为**否定性结论**：基于 schema.rs 与 data-model.md 直读，未逐一枚举
  crates 全部表；建议在补快照表 ADR 时以 `check_counts`/迁移清单复核。
