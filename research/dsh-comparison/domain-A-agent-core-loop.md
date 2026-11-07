# 域 A：Agent 核心循环

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`，2026-11-04，MIT，0.2.2-rc.1）；WHCode 证据根：本仓库。
> DSH 侧引用省略前缀，实际路径为 `D:\WHCode\local\dsh-src\...`。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 循环单位 | **turn ⊃ step**：turn 是最小可持久边界，step = 一次模型请求＋其工具批（`packages/core/agent-loop/src/agent.ts:327`、`:435`） | turn 内 `loop` 逐次迭代，迭代 = 一次模型调用＋工具批（`crates/whcode-app/src/turn.rs:454`—`550`） | **同构**，DSH 把 turn/step 提升为持久事件层级 |
| 可替换性 | `AgentLoop extends Service implements AgentFactory`，挂 `ctx.agentLoop`；扩展插件只依赖 `dsh-agent`，不依赖具体 loop（`packages/core/agent-loop/src/index.ts:290`；`docs/subsystems/core.md`） | `run_turn()` 是 `whcode-app` 的自由函数，四端直接调用（`turn.rs:240`、`:263`） | **DSH 领先**：loop 是插件，WHCode 是编译期唯一实现 |
| 输入队列 | 单一 inbox，目标边界 `next-turn`／`next-step`，`send/followup/steer/inject` 四个固定档（`packages/core/agent-loop/src/inbox.ts`；`docs/subsystems/core.md` 的 `Agent` 接口） | 无队列；调用方一次给 `LlmRequest.messages`，turn 中途不可投喂 | **结构性缺失（WHCode）**：无 steer/inject 语义 |
| 步前拦截 | `agent/pre-step` waterfall 可 accept/rewrite/reject，可声明 `startsRequestSeries`（`agent.ts:297`） | 无；只有编译期固定的消息组装 | **DSH 领先** |
| 流式期间状态 | 每个 attempt 落 `assistant/message`（正常）或 `assistant/attempt`（失败／取消，不进模型历史），流本体随消息持久（`agent.ts:500`—`540`） | chunk 只进内存缓冲与 WS 增量，落表正文按步聚合；被截断按 `error` 终止并标 `truncated`（`turn.rs:624`—`626`） | **DSH 领先**：失败尝试也是可审计事实 |
| 失败补洞 | 步抛错时用 `ToolCallRecovery` 把已发生但未回填的工具结果补写进日志，再抛（`agent.ts:373`—`383`） | 工具三步登记 `recover_uncertain` 补终态（`crates/whcode-store/src/tool_exec.rs`），turn 层不再补消息 | **同构**，层次不同：DSH 补日志事件，WHCode 补数据库终态 |
| 重试位置 | `agent/request-error` waterfall 返回 `retry` 才继续同一 step 的 `while(true)`；装配与 pre-step 不重跑（`agent.ts:546`—`566`） | 重试在 LLM decorator 内，只重试“首 chunk 前”的失败（ADR-050；`crates/whcode-llm/src/retry.rs`） | **同构**：都保证不重复副作用，位置不同 |
| 取消语义 | 两阶段：`prepareCall()` 与流式；任一异步阶段取消则 system/user 均不提交（`agent.ts:613`—`660`；`docs/architecture.md` Turn flow） | 每轮迭代前与每个 chunk 前查 `.cancel` 标记，跨进程生效（`turn.rs:454`—`460`、`:597`—`598`） | **同构**：DSH 靠 AbortSignal 事务化，WHCode 靠标记文件跨进程 |
| 终止判定 | 无工具调用→`completed`；`max-tokens` 黏住不降级；`turn-stopping` serial 钩子无 `next()`；`turn/end` 必落（`agent.ts:583`—`590`、`:396`、`:422`） | 六种终止原因显式落 `RuntimeTerminated`（`turn.rs:55`—`80`）：model_finished/iteration_limit/cancelled/budget_exceeded/tool_not_wired/error | **WHCode 领先**：终止原因是稳定枚举，可查询 |
| 上限 | 每步并行工具调用默认 10（`packages/core/agent-loop/src/constants.ts:6`）；无 turn 迭代硬上限（靠上下文压缩收口） | 迭代硬上限 16、预算 128k token、子代理 2（`turn.rs:43`、`:45`、`:49`） | **WHCode 领先**：硬上限在代码常量，DSH 靠预算软约束 |
| 线程模型 | 全异步，无“占线程”概念；每 agent 一个 driver fiber | 同步阻塞；`POST /api/llm/run` 由 `turn-worker` 线程承载（ADR-058；`crates/whcode-bin/src/serve.rs:42`—`47`） | **语义不同不可比**：DSH 异步原语 vs WHCode 线程＋有界通道 |

## 2. DSH 实现解剖

### 2.1 turn 边界：先开日志再谈工作

`turn()` 第一件事是 `session.append('turn/start', {turn})`，随后才进 `while(true)` 取 step
（`agent.ts:336`）。因此**空 turn 也是事实**：若 `pre-step` 拒绝则 `turnEnds={kind:'blocked'}`
（`:349`—`351`）；若首个 enter 被改写成空消息，则直接 `completed` 且不花一次模型调用
（`:359`—`364`）。`turn/end` 在 `finally` 中无条件追加（`:422`），异常路径也带上
`aborted`／`error` 结构化原因（`:404`—`420`）。这套“先落边界、后做事”的次序，使重放
不依赖内存状态。

### 2.2 step 前：可重写的准入

`preStep()` 先 `inbox.claim(target, turn)` 取走本步输入，再 `tools.resolve()` 生成准入工具集
（`agent.ts:289`—`292`），最后 `systemPrompt.assemble()`；`agent/pre-step` waterfall 拿到
`{messages, turn, step, signal, modelSelection}`，可返回 reject 或改写后的 messages，并可置
`startsRequestSeries`（`:297`—`305`）。准入集 `decision.toolSet` 在本步 `finally` 中
`dispose()`（`:390` 附近），即**工具准入按步失效**，不是按会话。

### 2.3 step 内：请求系列与冻结

`prepareRequest()` 在提交任何 model-visible 输入**之前**绑定适配器：`llm.prepareCall()` 失败
（非 `NO_ADAPTER`）直接抛，且 `agent/request` 之后禁止改 provider/model/effort，改了即抛
（`agent.ts:628`—`640`）。绑定成功后才 `systemPrompt.project()` 落 `system/message`、落
`user/message`，再由 `buildRequest()` 计算 `request/header`（初始／change／series 三种 reason，
`:676`—`690`）。重试复用同一 header，不重跑装配。

### 2.4 失败、取消、重试三者的分界

- 流启动前失败：直接抛，不落 attempt（`agent.ts:495`—`497`）。
- 流启动后失败：落 `assistant/attempt`；若已取消且有内容，则落 `interrupted: true` 的
  `assistant/message`（`:500`—`530`）。
- 失败后可经 `agent/request-error` 请求重试，只有显式 `retry` 才 `continue`
  （`:546`—`566`）。
- 取消原因经 `abortedCancelCause(signal)` 转成 `turn/end{kind:'aborted', reason}`
  （`:404`—`:409`）。

### 2.5 驱动与唤醒

`kick()` 是唯一驱动点：`while (await this.turn()) {}`，单次异常在驱动边界被吞并由
`agent/error` 报告（`agent.ts:268`—`280`）。turn 结束后若 inbox 尚有 pending，则换新
`AbortController`、`wakeRequested=false`、`step=0`，返回 true 继续下一 turn（`:424`—`432`）。
这把“队列空了”和“需要新 turn”统一成一个返回值。

## 3. WHCode 现状核对

- 已实现：多轮 turn、工具回填、预算折叠、跨进程取消、六种终止原因、同事务落表。
  `run_turn()`（`turn.rs:240`）与 `run_turn_with_progress()`（`:263`）双入口，后者供 GUI/Web
  增量渲染（T250/T251）。
- 已实现（ADR-058）：`POST /api/llm/run` 的 turn 由 `turn-worker` 线程执行，增量经有界
  `sync_channel` 回主循环，完成经 `done` 通道保证送达（`crates/whcode-bin/src/serve.rs:575`—`584`）。
- 已实现：`MAX_TURN_ITERATIONS=16`（`turn.rs:43`）、`DEFAULT_TURN_BUDGET_TOKENS=128_000`（`:45`）、
  `MAX_SUBAGENTS_PER_TURN=2`（`:49`）。
- 缺失（检索 `steer`、`inject`、`inbox`、`pre_step` 全仓无命中）：turn 中途投喂、步前拦截、
  请求系列概念。
- 缺失：loop 的替换点。`run_turn` 被 `whcode-tui`/`whcode-cui`/`whcode-web`/`whcode-gui`
  直接调用，无 trait 抽象（检索 `trait.*Turn|TurnLoop` 无命中）。

## 4. 差异判定与可借鉴点

1. **先落 turn 边界，再执行**（DSH `agent.ts:336`／`:422`）。WHCode 已有事件落表，但 turn
   边界事件与迭代事件在同一批次提交（`turn.rs:224` 的 `append_turn`），崩溃时边界不可见。
   可借鉴为“边界事件单独先提交”，代价是多一次写事务；对 WHCode 的崩溃恢复与审计是净收益。
2. **准入工具集按步失效**（DSH `toolSet.dispose()`）。WHCode 工具清单在 turn 开始时固定
   （`turn.rs` 组装 `LlmRequest.tools`）。可改写采纳：每步重解析工具清单，代价是每步一次
   注册表查询，收益是技能/MCP 中途变化可被感知。
3. **失败尝试持久化**（`assistant/attempt`）。WHCode 目前只存成功正文与被截断的错误标记，
   失败的模型尝试在事件里只有 `RuntimeTerminated(error)`。可采纳：失败尝试单列事件，
   代价是事件量增长，需同步事件上限与清理策略。
4. **`agent/pre-step` 式可重写准入**不适合 WHCode 现状：引入 waterfall 需要动态派发与
   插件面，与“零 unsafe＋无运行期插件”的静态分层冲突；若只为 steer 需求，可用有界
   命令通道实现，不必复制整套钩子体系。
5. **不采纳**：DSH 的“无 turn 迭代硬上限”。WHCode 的资源铁律要求有界，`MAX_TURN_ITERATIONS=16`
   与 `MAX_SUBAGENTS_PER_TURN=2` 应保留（NFR-211/216）。

## 5. 待验证项

1. `agent/turn-stopping` 的“serial 无 next()”是否意味着监听者无法否决收尾——只读了
   `agent.ts:396` 调用点与 `docs/architecture.md` 的措辞，未读 `dsh-agent` 的 serial 派发实现
   （`packages/core/agent/src/**`）。取证：读 `AgentEventDispatch.serial` 定义与并发语义。
2. DSH 是否真的没有 turn 迭代上限：`agent.ts` 内未见计数，且全仓检索
   `maxSteps|maxIterations|MAX_STEPS|iterationLimit`（排除测试）**零命中**；仍不能排除某个
   插件用事件注入终止条件。取证：审 `packages/guard/**` 与 `packages/experimental/**` 的
   终止相关事件监听。
3. WHCode 的 `turn-worker` 在客户端断开时的收尾：背压容量已确认是
   `TURN_PROGRESS_CAPACITY = 64`（`crates/whcode-bin/src/serve.rs:1850`、`:1881`），但
   “客户端中途断开后 worker 是否被 join、还是跑完丢弃”未读。取证：读 `serve.rs` 的
   `TurnPending` 分支与 `worker: Option<JoinHandle>` 的释放点（`:575`—`584` 起）。

## 6. 证据路径清单

- `packages/core/agent-loop/src/agent.ts:268`（kick 驱动）
- `packages/core/agent-loop/src/agent.ts:283`（preStep）
- `packages/core/agent-loop/src/agent.ts:327`（turn 边界）
- `packages/core/agent-loop/src/agent.ts:373`（失败补工具结果）
- `packages/core/agent-loop/src/agent.ts:396`（turn-stopping）
- `packages/core/agent-loop/src/agent.ts:422`（turn/end 必落）
- `packages/core/agent-loop/src/agent.ts:435`（step）
- `packages/core/agent-loop/src/agent.ts:613`（prepareRequest）
- `packages/core/agent-loop/src/index.ts:290`（AgentLoop 服务）
- `packages/core/agent-loop/src/constants.ts:6`（并行工具上限 10）
- `crates/whcode-app/src/turn.rs:43`（迭代上限）
- `crates/whcode-app/src/turn.rs:224`（append_turn 同事务）
- `crates/whcode-app/src/turn.rs:454`（主循环与取消检查）
- `crates/whcode-bin/src/serve.rs:575`（ADR-058 工作线程通道）
