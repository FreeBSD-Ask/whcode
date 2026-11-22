# Contract: 状态快照 schema

**Feature**: [spec.md](../spec.md) | **Date**: 2026-10-20

**来源**：任务书 §8.2（状态快照示例）。TUI、CUI、Web 三端显示同一份阶段状态、门禁结果和产物版本（FR-314）。

## 1. 快照结构

```json
{
  "schema_version": 1,
  "session": {"id": "S-001", "title": "Example", "stage": "Implement", "status": "running"},
  "overview": {"total_tasks": 42, "done": 30, "blocked": 2},
  "active_task": {"id": "T-017", "status": "running", "retry_count": 1},
  "context": {"estimated_tokens": 18432, "budget": 128000, "truncated": false},
  "llm": {"request_id": "R-889", "state": "streaming", "elapsed_ms": 1840},
  "event_cursor": 9912,
  "runtimes": {"javascript": {"enabled": false}, "python": {"enabled": false}}
}
```

## 2. 字段语义与约束

| 字段 | 语义 | 约束 |
|---|---|---|
| `schema_version` | 快照 schema 版本 | 必备；变更需兼容审查 |
| `session.stage` | 当前 SDD 阶段 | 取值来自状态机枚举；四端一致（FR-314） |
| `session.status` | 会话状态 | 与会话表一致 |
| `overview` | 任务概览（阶段分布/完成度/阻塞/活跃） | 可来自预聚合表，须标注最终一致字段（FR-506、§7.3-8） |
| `active_task` | 当前活跃任务与重试计数 | 无活跃任务时为 null |
| `context` | 上下文预算、已装入估算 token、是否截断 | `truncated` 为真时必须可查截断原因（FR-108、§4.1 契约） |
| `llm` | 当前请求 ID、状态、已耗时 | 允许合并展示更新，但状态转换不得丢失（FR-206） |
| `event_cursor` | 持久化事件游标 | 客户端据此增量补齐（FR-202/605） |
| `runtimes` | 各运行时启用状态与限制 | 未启用者不得出现在可执行工具列表中（FR-704）；能力等级 Full/Restricted/ShellOnly（FR-702） |

## 3. 使用规则

- 快照只读；任何状态变更必须经应用服务命令（ADR-003/005）
- 前端不得自行从完整历史切片填充快照字段（§4.1 约束）
- 概览字段允许延迟更新，但必须可重建，且不得作为唯一事实源（FR-506）
- 快照中的权限相关字段（如可执行动作集合）由应用服务按调用者身份裁剪，不由前端推断

## 4. 验证点

- 三端显示同一阶段、门禁结果与产物版本（V-207）
- `event_cursor` 补齐后前端状态与快照一致（V-103）
- 断线期间产生的更改在重连后可见（FR-605）

## 5. 实现注记（ADR-094，2026-11-22）

`context`／`llm`／`runtimes` 三字段由 `whcode-app::build_snapshot` 同源派生，四端
（CUI/TUI/Web/SSH）经同一函数取值，不在前端填充：

- `context`：`history_messages`（尾部优先、rewind 边界、budget 剪枝）装入的历史，
  按 `estimate_tokens` 累加得 `estimated_tokens`；`budget` 取 `DEFAULT_TURN_BUDGET_TOKENS`，
  `truncated` 由全量 token 和是否超预算判定，为真时附 `truncate_reason`。
- `llm`：从 `events` 的 `request_id` 与 `TurnStarted`/`TurnFinished` 派生，不读内存态
  （跨端一致）；`state` 为 `streaming`（仅 started）或 `completed`（配对 finished），
  `elapsed_ms` 取两者 `epoch_us(ts)` 差；无事件为 `None`（无 LLM 活动）。
- `runtimes`：`inventory(false)` 静态清单 × `runtime_state` 启用状态，附能力等级
  （`full`／`restricted`／`shell_only`）与可用性；未启用者仍列出，快照是状态视图，
  “未启用者不得出现在可执行工具列表”由工具层承担。

`context.breakdown`（ADR-098）：`compact_summary` 与 `conversation` 两类之和严格等于
`estimated_tokens`；`tool_schemas` 为默认九件注册表估算（不参与等式，`note` 标注近似）。
**`system` 提示不落 `messages` 表**（`turn.rs:214` 过滤），故不在分解内——`note` 必须
同时说明该缺口，读者不得把 `breakdown` 之和理解为全部上下文。

限制：`llm` 状态来自已落盘事件（非实时进度），`context` 是估算 token（非供应商计数），
`runtimes.available` 在 `probe=false` 下为 `None`（静态清单）。
