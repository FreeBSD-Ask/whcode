# 域 A：Agent 核心循环

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 主循环 | `apps/zcode-cli/packages/core/src/runtime/methods/turn-loop.ts` `runRegularTurnLoop`：`while(true)` 循环——中止检查→pending 命令/记忆消费→压缩检查→MCP 初始化与工具刷新→工具收集→reminder/公告注入→消息组装→模型 step→工具执行→回填→终止判定 | 无 agent 循环。`crates/whcode-app/src/llm.rs` 的 `run()` 为单次模型调用编排（TUI L 键/CUI llm run/Web POST llm/run 同源），模型返回即结束，无工具执行阶段 | **结构性缺失**：WHCode 无 turn 概念。补齐即“”（P0-1） |
| 状态机 | `apps/zcode-cli/packages/core/src/agent/turn-state.ts:27-38` `TurnPhase` 恰为 10 相：Idle→ProcessingInput→AwaitingModelResponse→Streaming→SchedulingTools→ExecutingTools→AggregatingResults→AwaitingPermission→Completing→Error；`:233-266` `canTransitionTo` 严格迁移表 | 无 | **修正**：状态机是**事件流之上的相变投影**，非事实源；权威事实是 SessionEvent 流（`events.ts`：TurnStarted→ModelRequest→ModelComplete→tool ledger→TurnComplete），`TurnMachineImpl` 仅在非法迁移抛 `InvalidTurnPhase` |
| 事件事实源 | SessionEvent 流为唯一事实源，工具回填经 `tool-result-settlement.ts` 生成 `role:"tool"` 条目 | `whcode-domain` EventRecord（seq 单调 + persistence_level 四级）已具备事件溯源，但无消费者驱动的循环 | WHCode 事件层已备，缺编排消费者 |
| Task 语义 | Task = 会话内一轮 turn | Task = SDD 规格任务（9 态状态机），与 LLM 会话解耦 | 语义不同，不直接可比 |

## 2. 循环结构（`turn-loop.ts`）

- L53 `while (true)` 开头；L54 `throwIfTurnAborted`（中止检查）；L55 `refreshOffPeakTurnAfterTakeover`（闲时抢占交接）。
- L62-72 排空 pending 命令/记忆：`drainPendingRuntimeCommandsForActiveLoop`；L74-81 消费已结算的 project memory recall。
- L83-121 压缩：L85 `microcompactIfNeeded`、L94 `autoCompactIfNeeded`（含 rapid-refill 阻断 L109-116）。
- L123-127 MCP：`initializeMcp` + `refreshMcpToolsIfChanged`（tools/list_changed 仅在回合前生效）。
- L129-131 工具收集：`getTurnTools`；L133-205 各类 reminder/announcement 注入。
- L208-243 消息组装：`prepareRuntimeProviderRequest` + cache-control projection；L244-249 重建 `TurnMachine.startModelRequest`。
- L262-271 `runModelBackedTurnStep`（模型 step）；L273-275 `if (result === "break") break` 终止。

## 3. 终止判定（`turn-model-step.ts`）

- L786-794 无工具调用（`executableToolCalls.length===0`）→ `finishModelStepWithoutToolCalls`
  （`turn-stop.ts:157-237`）：L195 `drainInlineGuideForNextRequest`→continue；L201 `runStopHooks`、
  L208 `shouldContinueAfterStopHooks`→continue；否则 L233-236 `complete(...,"success")`→`"break"`。
- 上下文超窗：L505-516 与 L576-599 `recoverModelStepAfterContextExceeded`（L821-884，reactive
  compact + rapid-refill breaker）；break 另一路径在 `turn-tools.ts:241-280`（`stopTurnResult`→break）。
- 输出 token 续写 `turn-output-token-continuation.ts` 独立于工具边界。

## 4. 与 WHCode 差异修正（相对原差异书）

- 原判定“”**确认成立**。
- **ZCode 结构比原书更复杂**，新增/未列入模块：`runtime-command-active-loop.ts`（活动循环内
  排空后台通知，`control-only-turn` 作屏障）、`control-only-turn.ts`（`emitControlOnlyUserTurn`
  落 `TurnStarted{executionKind:"controlOnly"}` + `TurnComplete{response:""}` 无模型输出边界）、
  `target-continuation-loop.ts`、`steering.ts`（50KB）、`streaming-tool-coordinator.ts`、
  `streaming-recovery.ts`、`turn-execution-fallback.ts`、`dynamic-workflow-*`。
- 命令队列串行准入：`runtime-command-queue.ts:50-89` 串行 drain + `runtime-command-submit.ts` 可取消入队。
- **WHCode 侧“”结论维持**；ADR-044 已立 P0-1 实施方案
  （`specs/001-whcode-harness/plan-p0-1-turn-loop.md`）。

## 5. 存疑项

- ZCode `TurnMachineImpl` 的 `.state.phase` 仅在 `turn.ts:1046` 被 `appendTurnOutcomeEvent`
  用作日志字段——状态机作为投影的证据充分，但其是否约束执行顺序以 `[待验证]` 标注
  （本次未逐一读全部 `canTransitionTo` 调用点）。
