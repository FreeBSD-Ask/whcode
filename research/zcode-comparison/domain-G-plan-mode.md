# 域 G：计划模式

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 会话内计划 | `tool/handlers/plan-mode.ts`：`enterPlanModeHandler`（`:36-58`）解析空 schema → `sessionModePort.enterPlanMode`（`:40-48`）；`exitPlanModeHandler`（`:60-104`）解析 `{plan, allowedPrompts}` → 非 plan 态抛 `InvalidStateTransition`（`:64-78`）→ `persistApprovedPlanFileBeforeExitPlanMode`（`:80-83`，批准后先写 plan 文件）→ `sessionModePort.exitPlanMode()`（`:85-93`）。ToolEntry：Enter `needsApproval:false`、Exit `needsApproval:true`、均 `sideEffectScope:"session"`（`:106-191`） | 无会话内计划模式 | 空白 |
| 模式切换 | `runtime/session-mode-port.ts:5-36`：`getMode` 读 `runtime.config.mode ?? "build"`（`:8`），`isPlanEnabled` 读 `readRuntimeExecutionState(runtime).planEnabled`（`:10`），`enterPlanMode` 设 `planEnabled:true`（`:11-19`），`exitPlanMode` 前置校验 `previous.planEnabled` 否则抛错（`:20-34`）；**plan 文件写入** `runtime/helpers/plan-file-continuity.ts` `writeApprovedPlanFile`（`:30-57`）**原子写** `.zcode/plans/plan-<sanitizedSessionId>.md`（`resolveApprovedPlanFilePath` `:18-28`），空 plan 抛 `InvalidInput`（`:38-42`） | 无 | 空白 |
| 只读强制 | `permission/service.ts:447-489` `checkPlanMode`：仅 `capability.readOnly && !capability.destructive` 放行（`:451-458`，ruleId `mode.plan.readOnly`）；非破坏性 MCP 放行（`:460-467`）；显式会话内非破坏免批放行（`:469-481`）；**其余一律 deny**（`:483-488`，`mode.plan.nonReadOnly`）。入口 `service.ts:211-213`：planEnabled 时**优先于 yolo 直通**（`:134` 注释“”） | 无 | 空白 |
| 规格门禁 | 无（spec 是文档流程，无运行时强制；全仓 Grep 无 `StageTransition\|GateId\|force_advance`） | GateId 四类门禁（`crates/whcode-domain/src/gate.rs:14-23`：Constitution/Clarify/Analyze/Checklist）+ `crates/whcode-app/src/gates.rs` `request_transition`（`:54-106`，含 `transition_allowed` 状态机校验 + clarify/analyze/constitution 三道门禁 + `update_stage`）与 `force_advance`（`:127-170`，**reason 必填否则拒绝** `:137-141`，审计含 actor/reason/skipped_gates/artifact_version）；`transition_allowed` 实现在 `crates/whcode-speckit/src/workflow.rs:220-225` | **WHCode 先手**：规格驱动门禁是 ZCode 没有的“”。补齐方向：plan 模式产出接入 gate 校验，两机制合流 |

## 2. 与 WHCode 差异修正（相对原差异书）

- 原判定**基本成立**，一处修正：ZCode 的“”确实存在（planEnabled +
  sessionModePort + checkPlanMode）；原书称“”——实为
  `PermissionService.checkPlanMode`（service.ts），命名精确。
- **WHCode 先手规格门禁确认存在**：`gate.rs` GateId 四类、`gates.rs` `request_transition` +
  `force_advance`（reason 必填 + 审计）、`workflow.rs:220-225` `transition_allowed`。
  ZCode 全仓 Grep 无对应匹配——确认 ZCode **没有**元级规格门禁（无强制 reason 的强制放行、
  无审计型 GateOverride、无状态机转换校验）。

## 3. 存疑项

- ZCode 的 `mode`（build/plan/yolo/guarded/auto）与 `planEnabled` 双轨并存：`planEnabled`
  是运行时执行态标志，`mode==="plan"` 是配置态；`session-mode-port.ts:8-10` 中
  `getPrePlanMode` 恒返回 `undefined`——prePlanMode 从未在真实运行态设置，仅 `service.ts`/
  `permission-input-recheck.ts` 读取，`[待验证]`（可能影响 Exit 后的模式恢复路径）。
- `turn-control.ts:53` 的 plan 收紧分支具体拒绝逻辑未逐行读，`[待验证]`，仅确认存在该分支。
