# 域 B：工具系统

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`，2026-11-04）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 工具定义 | `ToolDefinition = ToolSchema + 强制 output 契约（schema + render）+ execute + 宿主调度元数据 + UI presenter`（`packages/core/tools/src/index.ts:30`—`75`） | `trait Tool`：`descriptor()` + `execute(workspace, arguments) -> ToolOutput`（`crates/whcode-tools/src/registry.rs:118`—`121`） | **DSH 领先**：输出契约与 UI 投影也在工具上 |
| 注册表 | Cordis 服务 `ctx.tools`，按 agent 作用域可裁；`schemas()` 白名单，`execute/timeoutMs` 绝不外泄给模型（`packages/core/tools/src/index.ts:1`—`28`） | `BTreeMap<String, Arc<dyn Tool>>`，重名拒绝，`provider_manifest()` 只导 name/description/parameters（`registry.rs:126`—`185`） | **同构**；DSH 多了按 agent 准入与 UI presenter |
| 参数校验 | `defineTool` + typed schema DSL（zod 式），解析失败即拒（`packages/core/tools/src/schema.ts`） | 手写一层校验：对象形态、白名单、必填、类型、拒绝未知字段（`registry.rs:195` 起） | **同构**，DSH 表达力更强 |
| 并发 | 每步并行安全调用默认上限 10（`packages/core/agent-loop/src/constants.ts:6`）；以 `isConcurrencySafe` 判定并行/独占批（`packages/core/tools/src/pipeline.ts:109`—`112`） | **严格串行**：`for (tool_index, tool_call) in step_tool_calls.iter().enumerate()` 逐个执行（`crates/whcode-app/src/turn.rs:728`） | **DSH 领先**：有并行安全分类；WHCode 会话级串行（NFR-216 的保守取舍） |
| 执行钩子 | 三道 waterfall：`tools/pre-execute`（可拒/可改参数）、`tools/execute`（包裹调度）、`tools/post-execute`（可换 value/content，失败结果不可换 value）（`packages/core/tools/src/events.ts:18`—`41`；`pipeline.ts:502`—`510`） | 无钩子；`execute_tool_call()` 固定路径（`crates/whcode-app/src/turn.rs:1095`） | **结构性缺失（WHCode）** |
| 超时 | 工具自声明 `timeoutMs`，由 `guard/timeout-policy` 作为 `tools/execute` wrapper 执行；超时返回 `tool call timed out after Xms`（`packages/guard/timeout-policy/src/index.ts:41`—`74`） | 无单工具超时；取消靠跨进程 `.cancel` 标记（`turn.rs:454`—`460`） | **DSH 领先** |
| 重复调用防护 | `repeat-tool-reminder`：相同调用连续 3/5/8 次时注入提醒（`packages/guard/repeat-tool-reminder/src/index.ts:36`、`:53`） | 无 | **DSH 领先** |
| 输出上限 | 工具结果超 `maxInlineTokens` 则头尾保留＋spill 落盘，返回可检索地址；保存失败保持内联（`docs/subsystems/spill.md`） | 统一 64 KiB 截断（`crates/whcode-tools/src/lib.rs:19`），read 1 MiB、walk 500 项/200 目录、grep 200 条（`builtins.rs:26`—`31`） | **WHCode 领先**：上限是编译期常量，简单可证；DSH 机制更强但值可配 |
| 交付物 | `present` 工具声明文件为交付物（每次至多 4 个，落 `deliverables/presented` 事件）；`workspace-changes` 记录每 turn 的 git 差异（`packages/deliverables/tool-present/src/index.ts:39`—`49`、`:106`） | 无交付物概念；工具有 `side_effect` 标记（`turn.rs:749`、`:783`） | **DSH 领先** |

## 2. DSH 实现解剖

### 2.1 工具即契约

`ToolDefinition` 不是“名字＋函数”，而是四段式：模型面 `ToolSchema`（name/description/parameters）、
**输出契约** `output.schema` 与 `output.render`（把 canonical JSON 值投影成 `ContentBlock[]`）、
执行体 `execute`、宿主元数据（`timeoutMs`、`isConcurrencySafe`、`presentCall/presentResult`）。
注册表 `schemas()` 用显式白名单导出模型面字段，`execute`、`output`、`timeoutMs` 等永不进入请求体
（`packages/core/tools/src/index.ts:1`—`28`）。这条“元数据不进模型请求”的边界，避免把调度策略
泄露成提示词面。

### 2.2 批执行：并行安全是工具自证

调度器先对每个调用取 `isConcurrencySafe(args)`，只有返回严格 `true` 才进并行组，其余进独占组
（`pipeline.ts:109`—`112`）；批上限由 executor 配置或本批 `maxParallelToolCalls` 覆盖
（`packages/core/tools/src/executor.ts:23`—`27`）。工具若声明并行安全，就必须承诺不改父级状态、
共享状态可交换或 fail-closed（`index.ts:60`—`70` 的 JSDoc 契约）。

### 2.3 三道钩子的分工

- `pre-execute`：拿到冻结参数与 `exec.signal`，可返回拒绝或改参（`events.ts:18`）。
- `execute`：包裹实际调度，超时与重试策略都挂这里（`events.ts:29`）。
- `post-execute`：可替换 `value` 或 `content`，但**失败结果的 value 不可替换**
  （`pipeline.ts:510`），保证“失败就是失败”。替换内容为空串时统一给出
  `tool result blocked by post-execute policy`（`pipeline.ts:669`）。

### 2.4 溢出与交付物

`spill` 是“存储 seam + 策略消费者”两件：`SpillStore.saveText` 只负责完整落盘，
`spill-policy` 决定何时把内联结果换成“头尾＋地址”（`docs/subsystems/spill.md`）。
`deliverables` 是产品语义层：`present` 工具把文件登记为交付物，`workspace-changes` 在 turn
边界抓 git 差异，二者都不改工具结果本身。

## 3. WHCode 现状核对

- 已实现：六件套 `read_file`/`write_file`/`edit_file`/`list_dir`/`glob`/`grep`
  （`crates/whcode-tools/src/builtins.rs:148`、`:178`、`:233`、`:300`、`:345`、`:470`），
  注册表重名拒绝（`registry.rs:142`—`150`），参数校验（`registry.rs:195` 起），
  工作区路径绑定拒绝绝对路径与 `..`（`crates/whcode-tools/src/paths.rs:20`—`45`）。
- 已实现：工具执行三步登记（prepared→running→终态）＋`recover_uncertain`
  （`crates/whcode-store/src/tool_exec.rs`；`turn.rs:1600` 一带的登记边界注释）。
- 已实现：MCP 投影工具走独立路径，权限面 fail-closed，不区分读写统一按写权限
  （`turn.rs:1231`—`1249`）。
- 缺失（检索 `spill`、`tool-timeout`、`repeat`、`concurrency` 于 `crates/whcode-tools` 与
  `crates/whcode-app` 无对应实现）：溢出落盘、单工具超时、重复调用提醒、并行批、交付物。
- 语义差异：WHCode 每步工具调用数**无显式上限**，靠迭代上限 16 与单结果 64 KiB 截断兜底；
  DSH 有并行批上限但无每步调用数上限。两侧都属于“靠预算而不是计数”的取舍。

## 4. 差异判定与可借鉴点

1. **输出契约（schema + render）**：可采纳。WHCode 的 `ToolOutput` 只有 `content: String` 与
   `truncated`（`registry.rs:103`—`110`），模型面与展示面共用一个字符串；加 `render` 层能让
   GUI/Web 卡片与模型正文分离（ADR-057 B3 工具卡正需要）。代价：每工具多一个纯函数。
2. **`tools/post-execute` 式收口**：不采纳整套 waterfall（与静态分层冲突），但可采纳其
   **单点收口**做法：在 `execute_tool_call()` 末尾加一个固定策略函数（截断＋脱敏＋溢出），
   不引入动态监听者。代价小，收益是策略只有一处。
3. **单工具超时**：可采纳。WHCode 只有跨进程取消，没有“此工具最多 30 s”的预算；对 MCP
   与 shell 类工具是明显缺口。实现可在 `execute_tool_call` 内用现有取消标记＋deadline 检查。
4. **重复调用提醒**：改写采纳。DSH 阈值 `[3,5,8]` 注入提示词；WHCode 可只做事件
   （`ToolCallRepeated`）＋ GUI/Web 提示，不注入模型，避免提示词面继续膨胀。
5. **并行批**：不采纳（当前）。DSH 的并行安全靠工具自证，WHCode 的工具面还小，串行更容易
   证明无竞态；若将来引入，必须先写并发设计档案（ADR-015）。
6. **spill 落盘**：不采纳（当前）。WHCode 的 64 KiB 截断已保证有界；spill 会引入新的磁盘
   增长面与清理责任，收益只在“大输出可回读”，暂不抵成本。若采纳，须同时给出保留期与上限。

## 5. 待验证项

1. DSH `spill` 的 `maxInlineTokens` 是否有默认值：`Config` 只声明 `z.number()` 无 `.default()`
   （`packages/spill/spill-policy/src/index.ts:35`），实际取值应由 bundle 配置给出；未读
   `packages/bundle/base/**` 的 patch。取证：读 base bundle 的 spill 行配置。
2. WHCode 是否存在“每步 8 次工具调用”的隐式上限：`turn.rs` 与 `whcode-tools` 均未见计数，
   但任务书 T216 曾写“每轮上限 8”。取证：`rg -n "上限 8|MAX_TOOL_CALLS" specs/ crates/`。
3. DSH 并行批的实际并发实现（是否真并发、上限 10 的验证点）只读了 executor 契约与常量，
   未读 `batch.ts:40`—`120` 的 Promise 调度。取证：读 `batch.ts` 全文。

## 6. 证据路径清单

- `packages/core/tools/src/index.ts:30`（ToolDefinition/output 契约）
- `packages/core/tools/src/pipeline.ts:109`（并行安全分类）
- `packages/core/tools/src/pipeline.ts:502`（post-execute 替换约束）
- `packages/core/tools/src/events.ts:18`（pre-execute）
- `packages/core/tools/src/events.ts:41`（post-execute）
- `packages/core/agent-loop/src/constants.ts:6`（并行上限 10）
- `packages/guard/timeout-policy/src/index.ts:41`（超时结果）
- `packages/guard/repeat-tool-reminder/src/index.ts:53`（阈值 3/5/8）
- `packages/deliverables/tool-present/src/index.ts:39`（present 工具）
- `crates/whcode-tools/src/registry.rs:126`（注册表）
- `crates/whcode-tools/src/registry.rs:195`（参数校验）
- `crates/whcode-tools/src/paths.rs:20`（路径绑定）
- `crates/whcode-tools/src/builtins.rs:148`（read_file）
- `crates/whcode-tools/src/lib.rs:19`（64 KiB 上限）
- `crates/whcode-app/src/turn.rs:728`（串行工具执行）
