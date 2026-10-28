# 域 F：子代理 / 多智能体

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 实现 | `runtime/methods/subagent.ts`：子会话 id=`createSessionId("subagent_"+agentId)`（`subagent/runner.ts:811`）；共享父 eventStore（`subagent.ts:337` 传给子 `new AgentRuntime`）；子 runtime 仍用 `request.sessionId`（=childSessionId）落库（`:344-347`）；父引用经 `parentSessionId`（`:305`）与 `deriveChildClientPorts`（`:233-251`）。**每子代理一个 AgentRuntime 实例，进程内构造，无独立进程** | 无 | 空白 |
| 事件镜像 | `subagent/tool-event-mirror.ts`：镜像集合 `ToolCallScheduled/Started/Progress/Result/Error`（`:19-25`）与 `PermissionRequested/Resolved/Denied`（`:27-31`），方向**子→父**；toolCallId 重写 `tool_subagent_<agentId>_<childToolCallId>`（`:164-166`）；镜像事件 `source:"subagent"` 带 childSessionId/childToolCallId/parentToolCallId（`:64-77`）；**幂等性**——子 runtime 事件按 childSessionId 已落库，父 sink 只通知不再 append（`:379-407`） | 无 | 空白 |
| 生命周期 | `subagent/runner.ts`：**inactivity watchdog**（`:1202-1278`）默认超时 `DEFAULT_MODEL_STREAM_IDLE_TIMEOUT_MS`（`:218`），超时 `abort(error)` 终止任务（`:1264`）——**watchdog 是终止/超时，不是自动后台化**；**自动后台化**由独立 `autoBackgroundMs`（`:129,141`）→ `createAutoBackgroundTimer`（`:665-704`）调 `registry.requestBackground`（`:694`）承担；产物写 `join(tmpdir(),"zcode-agents",sessionId,agentId)` 下 metadata.json/output.txt/task.output（`:813-820`） | 无 | 空白 |
| 协调工具 | `SendMessage`：`tool/handlers/send-message.ts:76-133` → `sendMessageToLocalAgent`（runner.ts:905）→ `steerTurn` 注入父 runtime（message-steering.ts:29-49）；`RespondToCoordinator`：`tool/handlers/respond-to-coordinator.ts:52-117`（需 runtimeScope===“” + coordinatorResponsePort `:30`）→ 入队父会话（subagent-messages.ts:25-75） | 无 | 空白 |
| 并发上限 | **无硬上限**：`Agent` 工具 `concurrentSafe: true`（agent.ts:224）无并发字段；`maxConcurrency` 仅用于工具调度器（tool/scheduler.ts:44-56）与 workflow `max_concurrency`（`normalizeWorkflowMaxConcurrency` 明确注释“” create-workflow-source.ts:60-71）；`InMemoryRuntimeTaskRegistry.tasks: Map` 无界（registry.ts:131）；仅 `maxSubagents=64`（get-workflow-run-roster.ts:19-28）是 **workflow roster 展示层截断**，`truncated` 标记告知“”（get-workflow-run-roster-output.ts:42-68）——**非并发约束** | 无 | 补齐时**必须**加硬上限（WHCode 资源铁律：空闲态不得随会话数线性增长；ZCode 此处是弱点，勿照抄） |

## 2. 与 WHCode 差异修正（相对原差异书）

- 原判定**成立**：ZCode 进程内构造子 AgentRuntime、子会话 id=`subagent_<agentId>`、共享父
  eventStore、工具事件经 tool-event-mirror 镜像回父会话、产物写 tmpdir/zcode-agents、
  协调经 SendMessage/RespondToCoordinator。
- **修正两点**：(a) watchdog 是**超时终止**而非自动后台化——自动后台化由独立的
  `autoBackgroundMs` timer 承担；(b) 无硬并发上限确认成立（弱点成立），WHCode 补齐时
  **必须**加硬上限（如 MAX_SUBAGENTS + registry 有界），ZCode 的 workflow 层已有展示层截断
  但无执行层限制，不可照抄。

## 3. 存疑项

- `DEFAULT_MODEL_STREAM_IDLE_TIMEOUT_MS` 具体数值与 workflow `defaultConcurrency` 默认值
  `[待验证]`（在 contracts/shared 定义，本次未逐一核实）。
