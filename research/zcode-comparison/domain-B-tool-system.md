# 域 B：工具系统

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 注册 | `apps/zcode-cli/packages/core/src/tool/registry.ts:30-130`：`tools: Map<name, ToolEntry>` + `aliases: Map<alias, canonical>`；`register` 冲突拒 alias 保 canonical；`toContracts()` 过滤 `providerVisible!==false` 后映射 `ModelToolContract[]`，`execute` 恒置 `undefined` | **非“”**：`crates/whcode-store/src/tool_exec.rs` 已有完整 `tool_executions` 表三步登记 SQL 建模（意图—执行—登记状态机 + 不确定恢复），但**未被任何 app 层 crate 引用**（grep 仅命中 whcode-store 自身） | **修正**：WHCode 是“”，非“” |
| 内建工具 | `handlers/index.ts:80-148` `builtInTools` 数组，经 `registerBuiltInTools` 按 allowed/disallowed/灰度门过滤（`:212-286`）。实际存在：Read `read.ts:468`、Write `write.ts:214`、Edit `edit.ts:257`、Bash `bash.ts:479`、Glob `glob.ts:87`、Grep `grep.ts:143`、WebFetch `webfetch.ts`、WebSearch `websearch.ts:29`、TodoRead/TodoWrite `todo.ts:78/136`、AskUserQuestion、Skill `skill.ts:101`、Agent/Task `agent.ts:220`、Cron\*4 `cron.ts`、Enter/ExitPlanMode、submit_result/escalate/TaskOutput/TaskStop/SendMessage/RespondToCoordinator | 无 | 完全空白 |
| 调度 | `tool/scheduler.ts:48` `DEFAULT_MAX_CONCURRENCY=10`；按依赖拓扑排序（`:105-152`）、分组并行强制上限（`:154-202`）、环检测（`:204-226`）；只读集合 `READ_ONLY_TOOLS`（`:233-243`） | `crates/whcode-runtime` 有子进程 ConcurrencyGate（默认 min(4,物理核预算)），仅约束执行不约束工具调度 | 空白；runtime 闸门可复用 |
| 结果回填 | `runtime/methods/tool-result-settlement.ts:38-240` `settleToolResults`：`commitTurnRequestEntries` 先用 `modelContentForToolResult` 把结果同步写入两份模型历史（`:54-66`），经 `TurnMachineImpl.completeTool` + `aggregateResults` 注入 turn 状态（`:80-91`），最后 `persistPart` 落库（`:122-176`） | 无 | 空白 |
| 执行登记 | 无独立登记表（靠事件流） | `store.tool_exec` 三步登记（prepared→running→终态）+ `recover_uncertain` 崩溃恢复为 unknown、不重放 | **WHCode 先手**：登记协议更严格（幂等 + 崩溃语义），是落地工具系统的现成骨架 |

## 2. ZCode 有而 WHCode 先手协议没有的机制

- **输入校验三层**：`input-normalization.ts`（JSON 字符串归一化 + 运行时 schema）、
  `tool-input-validation-issues.ts`（结构化 issue 类型与格式化）、
  `input-validation-model-content.ts`（`<tool_use_error>` 投影，`:12-22`）。
- **结果持久化格式**：`result-persistence-format.ts`——`<persisted-output>` 信封 + 2,000 字符预览。
- **diff/patch**：`patch-parser.ts`(162行)、`patch-apply.ts`(140行)、`diff.ts`(46行)、
  `edit-matchers.ts`(411行)——Edit 结构化匹配。
- **executor 权限流**（approval-gate、permission-flow、permission-rules）与**后台任务**
  （background-tasks、bash-background-policy）。

## 3. 与 WHCode 差异修正（相对原差异书）

- 原差异书“”**需修正**：`whcode-store` 已有完整
  `tool_exec` 三步登记协议 SQL 建模（`tool_executions` 表、意图—执行—登记状态机、不确定恢复），
  但**未被任何 app 层 crate 引用**——确认“”而非“”。P0-2 落地时可直接接线。
- ZCode 侧**远超原判定**：不止注册/调度/回填，还有完整内建工具全家桶、输入校验三层、
  diff/patch、权限流、后台任务。
- **缺失/未注册**：ApplyPatch 有 handler 但被注释（`index.ts:86` `// applyPatchToolEntry`）、
  Workflow 被注释（`index.ts:147`）；ReadImage/ReadPdf/ReadVideo/NodeRepl(js)/GoalRead 等有
  handler 但不在 `builtInTools`（经插件/门控）。

## 4. 存疑项

- `ToolCompleted` 事件名未在 core 直接出现（实际以 `tool_complete`/`tool_result_committed`
  流式事件 + `settleToolResults` 落地）——原书事件名 `[待验证]`。
- ApplyPatch/Workflow 工具是否经插件恢复 `[待验证]`（当前默认注释关闭）。
