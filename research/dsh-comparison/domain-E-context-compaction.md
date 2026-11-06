# 域 E：上下文管理与压缩

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 上下文组成 | system-prompt 分段装配＋runtime context（工作区指令、时间、引用）＋历史消息（`packages/core/system-prompt/src/**`；`packages/context/src/**`） | 系统提示、历史消息、工具清单；`fold_messages_for_budget()` 只处理消息数组（`crates/whcode-app/src/context.rs:287`） | **DSH 领先**：分段与独立注入点 |
| 预算 | 由路由声明的窗口＋`thresholdRatio`/`retainRatio` 推导；`maxTokens` 缺省取 headroom（`packages/compaction/compaction-basic/src/config.ts:76`—`80`） | 单值 `DEFAULT_CONTEXT_BUDGET_TOKENS = 128_000`（`crates/whcode-app/src/context.rs:21`），请求可调低 | **DSH 领先**：预算随供应商窗口变化 |
| 触发 | 自动（step 边界压力＋溢出恢复监听，默认开启）、手动命令 `compact`（`packages/compaction/compaction-basic/src/types.ts:49`—`50`；`packages/compaction/command-compact`） | turn 内每轮迭代前检查一次预算，超预算即折叠；无手动压缩命令、无溢出恢复 | **DSH 领先** |
| 摘要来源 | **真实模型调用**生成摘要，`compaction/summary` 记录 provider/model/maxTokens/usage，可重放（`docs/subsystems/compaction.md`） | **程序合成摘要**：只写“折叠了 N 条消息（约 X token）”＋若干首行样例，不调用模型（`context.rs:314`—`340`） | **结构性差异**：DSH 语义压缩，WHCode 结构折叠 |
| 落地方式 | 摘要是 `user/message`，带 `surfaceOp: {op:'replace', startSeq, endSeq}` 替换被遮蔽区间；原始事件不删（`compaction.md`） | `context_windows` 表只存指针与区间，`compress_context()` 不删原文（`context.rs:7`—`8`、`:161`—`205`） | **同构**：都不销毁原始历史 |
| 崩溃一致性 | `compaction/start` 先行、`end` 最后；中途崩溃留下**可检测的孤儿锁**（`compaction.md`） | 无锁；折叠只影响内存数组与一次指针写入 | **DSH 领先** |
| 图像卸载 | `compaction-image-offload` 用 `image/offload` 事件记录精确图像出现位置，不携 `surfaceOp`（`compaction.md`） | 无图像输入路径 | **DSH 领先** |
| 计量 | token-meter：优先用供应商 usage 锚点，否则完整估价；`surfaceDeltaTokens` 保留增减（`docs/subsystems/token-meter.md`） | `estimate_tokens()` 字符启发式（`context.rs`），`total_tokens = folded_count*4` 的粗估（`turn.rs:498`—`500`） | **DSH 领先**：可复用真实用量 |

## 2. DSH 实现解剖

### 2.1 压缩是一个可换后端的能力缝

`ctx.compaction` 是 Service Definition，`compaction-basic` 是 Provider，`command-compact` 是
Consumer（`docs/subsystems/compaction.md`）。压缩**不属于 agent-loop 主干**，因此
“换一种摘要器（tokenizer/模板）”不需要动循环。

### 2.2 三个日志事件构成一次可审计的压缩

- `compaction/start {turn}`：抢日志级锁，先落盘。
- `compaction/summary {...}`：摘要正文、原始输出、被遮蔽区间与 seq 列表、估算 token、
  provider/model/maxTokens/usage 全记录（`compaction.md`）。
- `compaction/end {turn, error?}`：释放锁，失败也记。

锁“先开后关”，使崩溃表现为孤儿 `start`，而不是假的“压缩完成”。未匹配的 `start` 若早于
更新的 `session/end-seed` 则视为上一生命周期的陈旧证据（`compaction.md`）。

### 2.3 摘要如何进入模型视野

三个事件都是 log-only，不扩 `SurfaceEventType`；摘要本体作为独立 `user/message` 追加，
并用 `surfaceOp replace(startSeq, endSeq)` 遮蔽原区间（`compaction.md`）。也就是说
**“模型看到什么”仍由 surface 决定，日志保留全部原文**。

### 2.4 压力与保留比例

默认 `thresholdRatio = 0.8`（达到窗口 80% 触发）、`retainRatio = 0.16`（保留最近 16% 上下文）、
`auto = true`、余量不足时可再试 1 次（`packages/compaction/compaction-basic/src/config.ts:20`—
`33`；`types.ts:18`—`50`）。这是“按窗口比例”而不是“按固定 token 数”。

## 3. WHCode 现状核对

- 已实现：按预算折叠（`fold_messages_for_budget`，`context.rs:287`—`342`）；折叠保留首个
  user 锚点与摘要，样例取每条首行前若干字符；锚点自身超预算则放弃折叠。
- 已实现：`compress_context()` 只写 `context_windows` 指针，原文保留（`context.rs:7`—`8`、
  `:161`—`205`）；`load_summary_pointer()` 可回查（`:212`）。
- 已实现：turn 内在每轮迭代前检查预算，超预算折叠后 `continue`，无法折叠则
  `BudgetExceeded` 终止（`turn.rs:491`—`530`）。
- 缺失：模型生成摘要（`summary_ref` 是 `in-memory:<turn>:iteration-N`，没有可读摘要文本）、
  手动压缩命令、溢出恢复、图像卸载、真实用量锚点。
- 语义差异：WHCode 的摘要**不含语义信息**，只有“折叠了几条”；模型看到的是结构提示，
  不是对话要点的压缩。ADR-051 的“摘要回填”目前只是指针回填。

## 4. 差异判定与可借鉴点

1. **真实摘要生成**：建议采纳（P1 后段）。折叠摘要不携带语义，长会话里模型会丢失任务状态；
   DSH 的做法（单独一次模型调用＋完整 envelope 入日志）可直接照搬。代价：多一次调用成本，
   且需要摘要失败时的回退（DSH 的做法是 `compaction/end {error}` 后放弃本次压缩）。
2. **锁 + 先开后关**：可采纳为 `compaction` 事件对。WHCode 的折叠在内存里做，崩溃后没有
   “压缩到一半”的概念；一旦摘要改为模型生成，就需要这个锁。
3. **按窗口比例而非固定 token**：可采纳。`128_000` 是 5 个协议的保守下限；若路由声明窗口
   （如 256K），固定 128K 会过早折叠。代价：需要每个 profile 声明 `context_window`。
4. **`surfaceOp replace` 语义**：不采纳。WHCode 的历史在 `messages` 表，折叠是“替换发送数组”
   而不是“改表面”；引入 surface 概念会让事件溯源复杂化。保持“原文可查、发送时折叠”。
5. **图像卸载**：暂不采纳（无图像输入）；若接图像，必须先定“卸载后如何重放”的事件形状。
6. **不采纳** `retainRatio=0.16` 这类比例参数直接照搬：WHCode 的锚点策略（保留首条 user）
   更简单，且已有测试覆盖；改比例参数需先补“锚点超预算”的失败路径测试。

## 5. 待验证项

1. DSH 自动压缩的触发点是否真在 step 边界：`types.ts:49` 写“step-boundary pressure and
   overflow-recovery listeners”，未读监听器实现。取证：读 `packages/compaction/compaction-basic/src/index.ts`。
2. `maxTokens` 缺省取 `headroomTokens` 时，`headroomTokens` 由谁提供（adapter 声明的窗口？）。
   取证：读 `packages/llm/**` 的路由窗口声明与 `compaction-basic` 的解析。
3. WHCode `estimate_tokens()` 的口径（字符/4？）未读实现。取证：`rg -n "fn estimate_tokens" -A10 crates/whcode-app/src/context.rs`。

## 6. 证据路径清单

- `docs/subsystems/compaction.md`（surfaceOp replace）
- `docs/subsystems/compaction.md`（summary 事件字段）
- `docs/subsystems/compaction.md`（锁与孤儿检测）
- `packages/compaction/compaction-basic/src/config.ts:20`（0.8/0.16 默认）
- `packages/compaction/compaction-basic/src/types.ts:49`（auto 默认 true）
- `docs/subsystems/token-meter.md`（usage 锚点）
- `crates/whcode-app/src/context.rs:287`（折叠函数）
- `crates/whcode-app/src/context.rs:314`（合成摘要格式）
- `crates/whcode-app/src/context.rs:161`（compress_context 指针）
- `crates/whcode-app/src/turn.rs:491`（turn 内预算分支）
- `crates/whcode-app/src/turn.rs:528`（BudgetExceeded）
