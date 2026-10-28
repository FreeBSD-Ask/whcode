# ADR-044：Agent Turn 循环编排器（把 LlmRun 升级为多轮循环）

- **状态**：已批准（2026-10-28，用户审阅通过；G0—G5 落入 spec/tasks/checklist 随批准执行）
- **日期**：2026-10-28
- **责任**：Agent 核心循环负责人
- **相关**：差异比较书 `local/diff-zcode-vs-whcode.md` §4 P0-1（不入库）、ADR-007、ADR-018、
  ADR-031、ADR-032、FR-105/107、FR-201—208、FR-410、NFR-215/216

## 1. 事实与证据

### 1.1 WHCode 现状（源码直读）

- `crates/whcode-app/src/llm.rs`：`run()` 为**单次模型调用**编排——权限/会话/剖面/凭据
  校验后执行一次 Provider 流式调用，模型返回即结束；无工具执行、无循环、无 turn 概念。
  四端共用同一入口（TUI L 键、CUI `llm run`、Web POST `llm/run`、SSH）。
- `crates/whcode-llm/src/provider.rs`：`LlmRequest` 只有单个 `prompt: String`；
  `crates/whcode-llm/src/http/protocols.rs` 五个协议（openai/anthropic/gemini/ollama/cohere）
  的请求体均构造为单条 user 消息（`messages_role_user`）。
- `crates/whcode-domain/src/event.rs`：`EventKind` 十类已含 `ToolCallStarted`/
  `ToolCallFinished`；`EventRecord` 已带 `request_id`/`tool_call_id` 关联字段（FR-204 链路已建模）。
- `crates/whcode-store/src/tool_exec.rs`：工具执行三步登记（`record_intent`→`mark_running`→
  `record_result`）与 `recover_uncertain` 崩溃恢复已交付（FR-410、ADR-007）。
- `crates/whcode-tools/src/lib.rs`：仅 62 行文档，声明 5 条协议（schema 校验/prepared/running/
  结果登记/unknown 禁重放），无任何类型与实现（属 P0-2 范围）。
- `crates/whcode-app/src/context.rs`：`load_context` 预算加载已交付（默认 128 000 token、
  分页有界、`truncated_reason` 可观测）；`compress_context` 只记摘要指针，摘要生成未接线（P1-3 范围）。
- 命名口径问题：`llm.rs` 返回信封的 `events` 数组用 `llm.request_started` 等小写名；
  `events` 表 kind 用 domain `EventKind` PascalCase 名（`RequestStarted`）——同一链路两套命名，
  违反任务书 §8.3“三处不得各用一套命名”。
- `llm run` 当前**不写 `events` 表**（事件只出现在返回信封里）；app 层尚无任何 events 表
  写入路径（store 写者服务 T026/T034 已交付，待接线）。

### 1.2 ZCode 参照（源码直读，`local/zcode-src` commit 3c5b227，2026-10-20）

- `apps/zcode-cli/packages/core/src/runtime/methods/turn-loop.ts` `runRegularTurnLoop`：
  `while(true)` 循环——中止检查→pending 命令/记忆消费→microcompact/autoCompact 压缩检查→
  MCP 初始化与工具刷新→工具收集→reminder/todo/公告注入→消息组装（`prepareRuntimeProviderRequest`，
  带 cache-control）→`TurnMachine.startModelRequest`→`runModelBackedTurnStep`；
  `result === "break"` 退出。
- `turn-model-step.ts:579`：模型 step 返回 `toolCalls.length === 0` 且非终结时 turn 终止；
  上下文超窗（context-exceeded finish）走 `recoverModelStepAfterContextExceeded` 压缩恢复后
  `continue`；工具执行经 `turn-tools.ts` `executeToolCallsForModelStep`，结果追加进
  turnRequestState，下一轮迭代重建消息。
- `agent/turn-machine.ts`：10 阶段状态机（Idle→ProcessingInput→…→Completing/Error）是
  **UI 投影**，不是循环的事实源；事实源是 SessionEvent 流。
- 判定：ZCode 循环实质 = ①中止/预算守卫 ②消息组装 ③模型 step ④工具执行与结果回填
  ⑤无工具调用即终止。WHCode 已备事件、登记、预算三块基础设施，**缺编排消费者**。

## 2. 决策（提议）

1. **落点与语义**：`whcode-app` 新增 `turn.rs` 编排模块；`llm run` 语义升级为“执行一次
   agent turn”（多轮循环），命令名与四端入口不变，返回信封扩展 turn 字段。单次模型调用
   是迭代数 = 1 的退化情形，FR-201—208 验收口径不变。
2. **循环结构（裁剪）**：P0-1 期循环 = 中止检查 → 预算检查（`load_context`）→ 消息组装 →
   模型 step → 终止判定 → 工具执行与结果回填（P0-2 接线点）。**不引入** ZCode 的
   microcompact/autoCompact（归 P1-3）、MCP 初始化与工具刷新（归 P2-2）、
   reminder/todo/公告注入（暂不采纳，需另行提案）。
3. **消息契约升级**：`LlmRequest.prompt: String` → `messages: Vec<LlmMessage>`
   （system/user/assistant/tool 四角色）；五协议 request_body 同步改（单条 user 消息即原行为）。
   消息类型放 `whcode-llm`（domain 保持叶子零依赖）。
4. **终止与有界性（资源铁律）**：终止条件 = ①模型返回无工具调用 ②迭代达硬上限
   `MAX_TURN_ITERATIONS`（默认 16，可配置）③用户取消 ④上下文预算超限。全部终止显式留
   可查询事件标记，不静默截断（FR-208 口径）。每模型 step 沿用现有 chunk 数（1024）与
   输出字节（1 MiB）上限；turn 内消息累积由预算检查天然有界。
5. **事实源**：turn 关键事件经 store 写者落 `events` 表（复用 domain `EventKind`：
   `RequestStarted`/`StreamingChunk`/`RequestCompleted`/`RequestFailed`/`ToolCallStarted`/
   `ToolCallFinished`），payload 不含正文（FR-203）；turn 内消息轮次落 `messages` 表
   （role ∈ user/assistant/tool，schema 已有）。信封 `events` 数组同步收敛到 `EventKind`
   稳定名，消除两套命名。
6. **P0-1 与 P0-2 边界**：P0-1 **不含**工具实现。模型返回工具调用时：发 `ToolCallStarted`
   与 `ToolCallFinished`（state=`unknown`，注明“工具执行未接线”）并终止 turn——显式、
   可查询、不伪造执行结果；P0-2 落地后以真实调度与回填替换该路径。
7. **并发与取消**：单会话同进程至多一个活跃 turn（内存登记，有界 + 上限测试）；取消沿用
   `.live`/`.cancel` 标记机制，在**每轮迭代与每个 chunk 前**检查。不新增常驻线程
   （同步占用调用线程，与现行 `llm run` 一致，NFR-216）。同机多前端并发同一会话不在
   P0-1 互斥范围（诚实标注；`events.seq` 保证落表有序）。
8. **权限**：P0-1 无工具执行，权限面不变（`Capability::SessionCreate`）；工具级权限与
   交互 broker 归 P0-2/P1-4。
9. **正负和解**：**正面**——补齐“无 turn 循环即非编码 Harness”的结构性缺失（比较书 §4
   P0 判定），四端同步获得多轮能力，事件/登记/预算三块已有基础设施全部接通；
   **负面**——`llm run` 单次调用语义变化（迭代数可能 > 1，耗时上限放大 16 倍），
   `whcode-llm` 契约变更波及五协议与既有测试，events 表写入引入新的落盘量；
   **承担者**——四端用户（信封字段变化）、LLM 层维护者（契约迁移）；
   **和解理由**——P0-1 是项目成立要件，不做则其余 P0/P1 全部失去挂载点；单次语义以
   退化情形保留，契约变更在 0.1.0 未发布窗口内完成，无兼容包袱。

## 3. 三重时间

- **过去来源**：ADR-031/032 交付 LLM 单次调用与四端共用入口；ADR-007/FR-410 交付工具登记
  协议；FR-105/107 交付上下文预算加载；比较书 §4 判定 turn 循环为 P0（决定项目成立）。
- **现在证据**：上节双方源码直读；`StubProvider` 可扩展为多步剧本驱动循环测试（无网络、
  可复现）。
- **将来失效条件**：P0-2 工具系统落地（“未接线终止”路径被真实执行替换）；P1-3 自动压缩
  落地（预算检查升级为压缩检查）；P2-1 子代理引入嵌套 turn（迭代上限与活跃 turn 登记
  语义需修订）。复审触发：上述任一落地、或 ZCode 上游 turn-loop 结构变更（重读对应文件）。

## 4. 复审触发

- P0-2 工具系统接线时（重新审视终止判定、事件载荷与 tool_exec 登记衔接）；
- P1-3 自动压缩接线时（预算检查 → 压缩检查）；
- 引入子代理或会话级并行 turn 时；
- 资源预算复核（不晚于 2026-11-28）或 `whcode-llm` 协议层下次变更时。

## 5. 实施摘要

实施计划草案见 `local/plan-p0-1-turn-loop.md`（不入库；批准后按 G0—G5 落入
spec.md/tasks.md/checklist.md）。要点：

- 需求增量 FR-718—724（turn 编排/迭代上限/多消息契约/预算终止/事件事实源/单活跃
  turn/未接线显式终止），已落入 spec.md；
- 任务 T204—T212（`whcode-llm` 契约与五协议映射 → StubProvider 剧本 → `turn.rs` 循环骨架 →
  事件落表接线 → 预算检查 → 信封与四端适配 → 测试 → 文档同步），已落入 tasks.md Phase 22；
- 验收 Check 300—308（每任务一检查点：契约/协议映射/剧本夹具/终止五态/事件事实源/预算/
  四端对等/测试集/门禁全绿），已落入 checklist.md 第十三层；
- 测试验收入口按 ADR-037（GitHub Actions 为准；本机无远端时登记缺口）。

## 修订记录（2026-10-28：批准并落入 G0—G5）

**编号说明**：本 ADR 起草时为 ADR-043，因并行工作单元占用 043
（`ADR-043-windows-arm64-build-target.md`），按并行占用顺延为 ADR-044（同 ADR-041 先例）。

**效力**：用户审阅通过，九条决策按原样批准。批准同日完成规格落地：spec.md 新增
FR-718—724、tasks.md 新增 Phase 22（T204—T212）、checklist.md 新增第十三层（Check 300—308）。
相对实施计划草案的一处调整：Check 由 300—306（7 项）扩为 300—308（9 项，每任务一检查点），
以维持“第一层实现完整性对应 tasks.md 全部任务”的既有口径；检查内容与草案 §4 清单一一对应，
无增删判据。
