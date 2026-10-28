# P0-1 Agent Turn 循环编排器 — 实施计划

- **日期**：2026-10-28
- **状态**：已批准并入库（2026-10-28 用户审阅通过 ADR-044；同日 FR-718—724／T204—T212／
  Check 300—308 落入 spec.md／tasks.md／checklist.md，本计划按规则入
  `specs/001-whcode-harness/`）
- **来源**：`research/zcode-comparison/` §4 P0-1（域 A／actionable-gaps.md P0，2026-10-28
  按 ADR-045 入库）+
  `docs/adr/ADR-044-agent-turn-loop-orchestrator.md`（已批准）
- **边界**：本文件是 P0-1 的实施计划；不复制 ADR-044 全文，冲突时以 ADR-044 与 specs 为准。

---

## 0. G0 范围

**做什么**：把 `whcode-app/src/llm.rs` 的单次 `llm run` 升级为 agent turn 循环编排器——
中止检查 → 预算检查 → 消息组装 → 模型 step → 终止判定 →（工具执行回填为 P0-2 接线点）。
`whcode-llm` 请求契约升级为多消息数组；turn 关键事件落 `events` 表；消息轮次落 `messages` 表；
信封扩展 turn 字段并收敛事件命名；四端同步适配。

**不做什么**（显式排除，均有归属）：

| 排除项 | 归属 |
|---|---|
| 工具注册/调度/执行/结果回填实现 | P0-2（whcode-tools 5 协议落地） |
| 自动上下文压缩（摘要生成） | P1-3 |
| 工具级权限规则 + 交互 broker | P1-4 |
| MCP 客户端、技能加载、子代理 | P2-2 / P1-2 / P2-1 |
| 流式生成重试、会话分享 | P1-8 / P1-9 |
| 跨进程/跨前端同一会话的 turn 互斥 | 不在范围（诚实标注） |
| reminder/todo/公告等模型注入 | 暂不采纳，需另行提案 |

**正负和解**（G0 口径，摘要；详见 ADR-044 决策 9）：正面 = 补齐项目成立要件、三块既有
基础设施接通；负面 = 单次调用语义变化、契约变更波及五协议、新增落盘量；承担者 = 四端
用户与 LLM 层维护者；和解 = 0.1.0 未发布窗口内完成，无兼容包袱。

---

## 1. G1 需求增量（FR 草案，编号接续 FR-717）

- **FR-718**（P0）：一次 `llm run` 编排一轮 agent turn：循环执行“中止检查→预算检查→
  消息组装→模型 step→终止判定→工具执行与结果回填（接线后）”；终止条件为模型返回无
  工具调用、迭代上限、用户取消或预算超限；所有终止均以可查询事件显式记录，不静默截断。
- **FR-719**（P0）：turn 迭代硬上限默认 16（可配置），达上限显式终止并记录事件；每轮
  模型 step 沿用单次调用的 chunk 数（1024）与输出字节（1 MiB）上限，超限显式标记
  `truncated`（FR-208 口径）。
- **FR-720**（P0）：模型请求支持 system/user/assistant/tool 四角色消息数组；openai/
  anthropic/gemini/ollama/cohere 五协议请求体映射一致；单条 user 消息与现行行为等价。
- **FR-721**（P0）：每轮消息组装按上下文预算加载（FR-105 契约：items/next_cursor/
  consumed_budget/truncated_reason）；预算不足以容纳下一轮时显式终止 turn 并报告原因；
  不静默压缩（自动压缩属后续需求，P1-3）。
- **FR-722**（P0）：turn 内全部关键事件（请求开始/完成/失败、工具调用开始/结束）以
  domain `EventKind` 稳定名经 store 写者落 `events` 表，payload 不含正文；turn 内
  user/assistant/tool 消息轮次落 `messages` 表；`turn_id` 贯穿 turn 内事件与信封
  （扩展 FR-204 关联链）；信封 `events` 数组命名与 `EventKind` 稳定名收敛。
- **FR-723**（P0）：单会话同进程至多一个活跃 turn；用户取消经现有 `.live`/`.cancel`
  标记机制在每轮迭代与每个 chunk 前生效（“下一 chunk 边界生效”语义不变）。
- **FR-724**（P0）：模型返回工具调用而工具执行未接线（P0-2 前）时，记录
  `ToolCallStarted`/`ToolCallFinished`（state=`unknown`、未接线标记）并终止 turn；
  不得静默忽略或伪造执行结果；P0-2 接线后以真实执行替换。

**既有需求复用（不改写）**：FR-201—208（事件/链路/追踪口径）、FR-105/107（上下文预算）、
FR-410（工具登记与 unknown 语义）、FR-409（超时/中断）、NFR-215/216（有界/无新线程）、
FR-604（权限统一拒绝）。

---

## 2. G2 设计

### 2.1 模块落点与分层合规（test_layering 口径）

| 改动 | crate | 说明 |
|---|---|---|
| `LlmMessage{role, content}`、`LlmRequest.messages` | `whcode-llm`（provider.rs） | 契约类型；domain 保持叶子零依赖不动 |
| 五协议 request_body 多消息映射 | `whcode-llm`（http/protocols.rs） | 每协议单测 |
| `StubProvider` 多步剧本 | `whcode-llm`（provider.rs） | 默认行为不变，现有测试不破 |
| `turn.rs` 编排模块 | `whcode-app` | 不内嵌 SQL（ADR-018），存储经 store 公共 API |
| 事件落表、messages 轮次落表 | `whcode-app` → `whcode-store` 公共 API | 写者服务 T026/T034 已交付；具体 API 形态在 G3 起步时核对 `writer.rs`/`service.rs` |
| 信封字段与事件命名 | `whcode-app`（llm.rs/turn.rs） | `LlmRunOutcome` 扩展 |
| 四端渲染与 surface 登记 | `whcode-tui`/`whcode-cui`/`whcode-web`/`whcode-ssh` + `surface.rs` | 前端不得触库 |

方向性检查：无反向依赖、无跨层捷径、无 `cfg(target_os)` 出 platform、无新 crate。

### 2.2 循环结构与终止判定

```
run_turn(conn, policy, actor, context, request, now_ms):
  require(Capability::SessionCreate)            # 权限面不变（ADR-044 决策 8）
  session 存在性 / prompt 非空 / 剖面 / 凭据    # 复用 llm.rs 前置校验
  turn_id = 生成；活跃 turn 登记（单会话单活跃，登记冲突 → invalid_state）
  messages = [user(prompt)]
  for iteration in 0..MAX_TURN_ITERATIONS:
    取消检查（.live/.cancel 标记）
    load_context 预算检查                        # 超限 → 终止(termination_reason=budget_exceeded)
    组装本轮消息（messages + 预算内历史）
    model step：新 request_id；事件 RequestStarted → StreamingChunk* → RequestCompleted/Failed
    解析工具调用（finish_reason="tool_call" 时）
    if 无工具调用: 终止(termination_reason=model_finished)   # 对齐 ZCode turn-model-step.ts:579
    if 工具执行未接线（P0-1）:
      事件 ToolCallStarted + ToolCallFinished(state=unknown, 未接线标记)
      messages += assistant(tool_call 声明) + tool(unknown 结果说明)
      终止(termination_reason=tool_not_wired)                # FR-724；P0-2 替换为真实执行并 continue
  达上限 → 终止(termination_reason=iteration_limit)          # FR-719，显式事件标记
  注销活跃 turn；返回扩展信封
```

终止原因枚举（`termination_reason` 稳定名）：`model_finished` / `iteration_limit` /
`cancelled` / `budget_exceeded` / `tool_not_wired` / `error`。

### 2.3 消息契约与五协议映射

- `LlmMessage { role: Role, content: String }`，`Role ∈ system/user/assistant/tool`；
  `LlmRequest.prompt` 删除，`messages: Vec<LlmMessage>` 取代（非空校验上移到调用方）。
- 五协议 request_body：openai/ollama/cohere 走 `messages` 数组直映；anthropic 与 gemini
  按既有单消息映射扩为多段（system 段 + 交替轮次），实现期以每协议单测断言角色与顺序。
- `StubProvider` 增加剧本模式：`Vec<StubStep>`（每步含 chunk 序列、finish_reason、
  可选 tool_calls 列表）；无剧本时保持现行单步行为。

### 2.4 事件与持久化（事实源）

- 落表事件 kind 用 domain `EventKind` 稳定名（`RequestStarted`/`StreamingChunk`/
  `RequestCompleted`/`RequestFailed`/`ToolCallStarted`/`ToolCallFinished`）；payload 只带
  标识与计数（request_id/turn_id/tool_call_id/chars/finish_reason），不含正文（FR-203）。
- 载荷新增 `turn_id` 字段（会话内单调，如 `turn-<session>-<n>`；生成规则实现期定稿，
  必须可复现、不依赖时钟）。
- messages 轮次落表：role ∈ user/assistant/tool；`content_ref` 超 1 MiB 走引用（R-05
  对策），实现期核对 store 是否已有内容表 API，无则登记缺口或按上限直存（≤1 MiB）。
- 信封 `events` 数组 kind 值收敛为 `EventKind` 稳定名（现行 `llm.request_started` 等
  小写名废弃）；四端渲染按新名适配（与落表口径一致，任务书 §8.3“不得各用一套命名”）。

### 2.5 信封与四端适配

`LlmRunOutcome` 增加：`turn_id: String`、`iterations: u64`、`termination_reason: String`；
既有字段（chunks/text/finish_reason/usage/transport…）语义不变（末轮模型 step 的聚合）。
`llm cancel` 语义不变（取消整个活跃 turn 的当前模型 step）。四端：
TUI 结果视图、CUI `llm run` JSON 信封（cui-cli.md 契约同步）、Web `llm/run` 响应、
SSH 白名单命令输出；`surface.rs` 登记与对等契约门禁测试同步更新。

### 2.6 测试策略与夹具

- **夹具**：StubProvider 多步剧本（无网络、可复现）：①两轮剧本（第一轮 tool_call →
  第二轮无工具调用）②单轮无工具调用（退化 = 现行行为）③超上限剧本（>16 轮）④取消
  剧本⑤预算超限剧本。
- **正向路径**：多轮循环至 `model_finished`；事件 seq 单调、turn_id 贯穿、messages 表
  轮次完整可重建。
- **失败路径**：迭代上限、预算超限、tool_not_wired、取消、权限拒绝、未知会话、
  空提示词；全部断言显式终止标记与事件存在。
- **安全断言**：事件/信封序列化不含提示词、正文、凭据（FR-203 口径，参照现有
  `credential_value_never_appears_in_results_or_debug`）。
- **资源断言**：单会话单活跃 turn（并发登记拒绝 + 释放）；无新线程（turn 同步占用
  调用线程）；无无界容器累积（消息/事件随迭代有界）。

### 2.7 资源与安全核对

| 铁律 | 核对 |
|---|---|
| NFR-215 有界 | 迭代上限 16、chunk/输出上限沿用、预算检查天然有界、登记表有界 |
| NFR-216 无新线程 | turn 同步执行，不引入线程/异步运行时 |
| 每会话不加常驻占用 | 活跃 turn 登记随 turn 结束释放（RAII） |
| 凭据/正文不落事件与日志 | payload 只带标识与计数；redacted 默认 |
| 零 unsafe / clippy 零告警 | 普通 Rust 实现，无例外 |

---

## 3. G3 实施任务拆分（T 草案，编号接续 T203）

| # | 任务 | 落点 | 依赖 |
|---|---|---|---|
| T204 [P] | `LlmMessage`/`Role` 类型与 `LlmRequest.messages` 契约；`StubProvider` 兼容单消息 | `crates/whcode-llm/src/provider.rs` | — |
| T205 [P] | 五协议 request_body 多消息映射 + 每协议角色/顺序单测 | `crates/whcode-llm/src/http/protocols.rs` | T204 |
| T206 [P] | `StubProvider` 多步剧本（finish_reason/tool_calls 可编程；默认行为不变） | `crates/whcode-llm/src/provider.rs` | T204 |
| T207 | `turn.rs` 循环骨架：前置校验复用 llm.rs、循环与终止判定表、迭代上限、取消检查、活跃 turn 登记（RAII） | `crates/whcode-app/src/turn.rs` | T204 |
| T208 | 事件落表接线（EventKind 稳定名、turn_id）与 messages 轮次落表；信封 events 命名收敛 | `crates/whcode-app` + store 公共 API | T207 |
| T209 | 预算检查接线（load_context + 超限终止 + truncated_reason 事件） | `crates/whcode-app/src/turn.rs` | T207 |
| T210 | 信封扩展（turn_id/iterations/termination_reason）与四端渲染、surface 登记、cui-cli.md 契约同步 | 四前端 + `surface.rs` + contracts | T207/T208 |
| T211 | 正向/失败路径测试集（§2.6 全清单），含安全与资源断言 | `tests/`（unit/integration） | T207—T210 |
| T212 | 文档同步：spec.md FR-718—724、tasks.md 勾选、checklist.md Check、CHANGELOG、实现记录 | `specs/` `docs/` | 全部 |

**门禁链**（提交前）：`fetch-duckdb.py` → `cargo build --locked --workspace` →
`cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` →
`cargo test --locked --workspace` → layering/layout 门禁 → `traceability.py` +
`check_counts.py`（FR 计数声明同步）。

---

## 4. G4 验收（Check 草案，编号接续 Check 299）

> **2026-10-28 已批准并落入 checklist.md**：Check 编号定为 **300—308**（每任务一检查点，
> 维持“第一层实现完整性对应 tasks.md 全部任务”口径），内容与下表一一对应。

- **Check 300**：`LlmMessage`/四角色枚举与 `LlmRequest.messages` 契约落地，StubProvider
  兼容且默认行为不变（来源：Task T204、FR-720）。
- **Check 301**：五协议消息数组映射测试全绿（每协议角色与顺序断言）（来源：Task T205、FR-720）。
- **Check 302**：StubProvider 多步剧本可编程、默认行为不变（来源：Task T206、FR-718）。
- **Check 303**：turn 五种终止（model_finished/iteration_limit/cancelled/budget_exceeded/
  tool_not_wired）各有测试，且终止均留可查询事件（来源：Task T207、FR-718／719／723／724）。
- **Check 304**：事件落表（EventKind 稳定名、payload 不含正文、turn_id 贯穿）、messages
  轮次落表、信封命名收敛；序列化不含正文凭据（来源：Task T208、FR-722）。
- **Check 305**：预算检查接线：超限显式终止 + truncated_reason（来源：Task T209、FR-721）。
- **Check 306**：信封扩展与四端一致、surface 对等契约门禁通过（来源：Task T210、FR-718／722）。
- **Check 307**：正向/失败路径测试集全绿，安全与资源断言通过（来源：Task T211、FR-718—724）。
- **Check 308**：全量测试与 layering/layout/traceability/check_counts 门禁全绿；验收入口按
  ADR-037（来源：Task T212、FR-305 口径）。

---

## 5. G5 发布与文档

- `CHANGELOG.md`：记录 `llm run` 语义升级（turn 循环）、信封字段扩展、事件命名收敛
  （均为 0.1.0 未发布窗口内变更，无兼容承诺）。
- README/quickstart 受影响处同步；`docs/matrices/` 能力矩阵按实际勾选。

---

## 6. 缺口与风险登记

1. **实施门禁**：本计划已批准并落入 spec/tasks/checklist（FR-718—724、T204—T212、
   Check 300—308）；T204 起步即进入 G3，须按 §3 门禁链逐项全绿；测试验收入口按 ADR-037
   （见第 5 条）。
2. **P0-2 依赖**：`tool_not_wired` 终止路径是过渡行为，P0-2 落地后必须替换（ADR-044
   复审触发 1）；替换时补 tool_exec 三步登记衔接（store 已备，未接线）。
3. **事件写者接线形态**：app → store 写者公共 API 的具体形态（`Writer` 生命周期、
   谁持有写者句柄）在 T208 起步时核对 `writer.rs`/`service.rs`，可能与 CUI 命令队列
   （`queue.rs`）的串行准入组合；若写者需独立线程，按 NFR 以工作区级单例计，不随会话增长。
4. **ZCode 快照时效**：ZCode 结论来自 2026-10-20 commit；T204 起步前重读
   `turn-loop.ts`/`turn-model-step.ts` 上游最新（0 信任）。
5. **验收口径**：测试结论以 GitHub Actions 为准（ADR-037）；本机无远端仓库时登记缺口。
6. **messages 表 content_ref**：大正文引用需核对 store 内容表 API 现状；无则按 ≤1 MiB
   直存并在 spec 标注，不得静默截断。

---

*生成方式：本文件由主代理基于 `research/zcode-comparison/`（域 A、actionable-gaps.md P0）、
ADR-044 与 WHCode/ZCode 双侧源码直读起草，2026-10-28 经用户批准后按规则入库 specs。*
