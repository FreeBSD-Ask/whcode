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
