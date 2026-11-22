# ADR-094：状态快照三字段从契约占位改为真实数据

- **状态**：已批准（2026-11-22，用户"继续其他功能开发"；本 ADR 是对代码/契约失同步的修复立项）
- **关联**：`contracts/state-snapshot.md` §2；FR-314；V-207（四端同源）；ADR-003/005（只读投影）
- **编号**：ADR-093 之后无占用

## 任务认知卡

- **方向**：把 `StateSnapshot` 的 `context`／`llm`／`runtimes` 三字段从恒 `None`／空 map 改为按契约语义派生的真实数据。
- **目标**：四端（CUI/TUI/Web/SSH）经同一 `build_snapshot` 得到相同的三字段；快照仍只读、不新增写操作、不改签名。
- **用户意图**：用户要求继续功能开发；本次修复"契约声明已实现里程碑能力、代码仍标未实现占位"的失同步，是有实证支撑的真实缺口。
- **非目标／边界**：不改 `build_snapshot` 签名；不引入新配置或新表；不把示例值（128000 budget 等）当硬编码伪数据；不改变 `schema_version`（字段类型不变，仍为 serde_json::Value／BTreeMap）；不触碰四端各自的渲染层。
- **现有证据**：`snapshot.rs:15-18,85-92` 注释写"M3/M8 未实现"但 M3/M8 早已交付；契约 §2 声明三字段完整语义；`context::history_messages`＋`DEFAULT_TURN_BUDGET_TOKENS=128000`＋`events.ts_ms/request_id`＋`runtime_state::get`＋`whcode-runtime::inventory` 均已就绪。
- **手段（后置）**：snapshot.rs 内三个纯派生辅助函数 → 接入 build_snapshot → 单测（有数据／无数据／权限拒绝）→ 契约与四端同源验证。

## 问题

`build_snapshot` 是四端同源的唯一入口（FR-314/V-207），但 `context`／`llm`／`runtimes` 三字段恒返回占位值，且注释仍写"属 M3/M8 未实现"（里程碑早已收口）。这违反"物质—意识"条款——契约声明了语义、代码却声称未实现，四端用户看不到本应可见的上下文预算、LLM 状态与运行时启用状态。

## 决策

1. **`context`**：取 `history_messages(conn, policy, actor, session, DEFAULT_TURN_BUDGET_TOKENS)`，用 `context::estimate_tokens` 累加消息正文得 `estimated_tokens`；`budget` = 128000；`truncated` = 实际装入是否少于全量消息（由 history_messages 的剪枝行为判定，不自行猜测）；为真时附 `truncate_reason`。无历史时 `estimated_tokens=0`、`truncated=false`（如实零，不省略字段）。
2. **`llm`**：从 `events` 表按 session 取最近一条带 `request_id` 的 `TurnStarted`/`TurnFinished` 配对——`request_id`、`state`（streaming/completed/failed，按事件类型与 payload 判定）、`elapsed_ms`（`ts_ms` 差值）。无请求时 `None`（无 LLM 活动是真实状态，不是缺口）。派生只读，不引入内存态（跨进程/跨端一致）。
3. **`runtimes`**：遍历 `whcode_runtime::inventory(probe=false)`，对每个名称查 `runtime_state::get(conn, name)`，填 `enabled` 与 `capability`（Full/Restricted/ShellOnly 的 `as_str()`）；`available` 来自 inventory；未启用者仍出现在快照（快照是状态视图，不是可执行工具列表——"未启用者不得出现在可执行工具列表"约束由工具层而非快照承担）。
4. **只读与同源**：三字段全部由 `conn` 派生，`build_snapshot` 签名不变；四端调用同一函数自动同源。

## 理由与约束

- 为何用 DEFAULT_TURN_BUDGET_TOKENS：契约示例的 128000 与 turn 实际预算同源，避免凭空引入新常量；若用户显式传了别的 budget，那是 turn 请求级参数，不属于会话快照的"默认预算"语义。
- 为何 llm 不读内存态：快照要跨端/跨进程一致；内存态只在单进程可见，会造成四端不一致（违反 V-207）。
- 承担者：app 快照维护者（派生逻辑与测试）；四端无需改动（同一函数）。

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | 契约与代码一致；四端获得上下文/LLM/运行时真实状态；不改签名、不新增表 |
| 负面 | build_snapshot 增加读查询（history_messages + events + runtime_state），成本随消息量增长；llm 状态为事件派生（非实时内存态），有事件落盘延迟 |
| 承担者 | 快照调用方承担少量查询开销；用户承担事件持久化延迟带来的状态滞后 |
| 和解理由 | 查询在快照准备层（非帧路径），有界（history_messages 已按 budget 剪枝）；事件滞后如实标注，不伪造实时 |

## 三重时间

- **过去来源**：契约 state-snapshot.md §2（字段语义）；FR-314/V-207（四端同源）；M3/M8 已交付而注释未更新的事实。
- **现在证据**：snapshot.rs 三字段恒占位；history_messages/estimate_tokens/runtime_state::get/inventory 全部在库且签名匹配。
- **已知限制与失效条件**：①llm 状态来自已落盘事件，streaming 中的实时进度不在快照（按 ADR-044 事件流呈现）；②context 的 token 是估算（estimate_tokens），不是供应商实际计数；③runtimes 用 probe=false（不探测外部运行时可用性），`available` 反映静态清单而非实时探针。
- **复审触发**：①`contracts/state-snapshot.md` §2 字段语义变更→重审派生逻辑；②`build_snapshot` 签名或 `history_messages` 行为变更→重跑四端同源测试；③契约测试 `snapshot_contract_fields_match` 失败→回滚本 ADR。

## 影响

- 正面：三字段真实可用，四端一致；"物质—意识"失同步消除。
- 负面：快照查询增加；llm 状态非实时。
- 不做什么：不改签名、不新增表、不改四端渲染、不引入配置。
