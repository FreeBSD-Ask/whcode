# Phase 1 Data Model: WHCode — Rust 原生 LLM 编码 Harness

**Feature**: [spec.md](./spec.md) | **Date**: 2026-10-20

**来源**：任务书 §3（术语与状态模型）、§7（数据模型与持久化）。字段为逻辑模型，具体类型/DDL 在实现期确定；每张表都必须有 schema 版本与迁移脚本（FR-501、NFR-103）。

## 1. 核心实体（14 张表）

### workspace_meta
- 字段：`schema_version, workspace_id, created_at`
- 约束：工作区与 schema 版本的唯一事实源；一行
- 校验：`schema_version` 必须与 `schema_migrations` 最大已应用版本一致

### sessions
- 字段：`id, title, created_at, last_active_at, model, archived, status`
- 约束：会话元数据，不含完整历史（G-04、FR-102）
- 校验：`id` 工作区内唯一；`archived` 会话不参与活跃尾部

### tasks
- 字段：`id, session_id, title, stage, status, parent_id, acceptance_json, retry_count, updated_at`
- 约束：状态、依赖和验收条件可查询；`parent_id` 支持子任务层级（FR-406）
- 校验：`status` ∈ 任务状态枚举（§2.1）；`acceptance_json` 每条含可执行断言或人工检查清单说明（§4.3.3）

### stage_artifacts
- 字段：`id, session_id, task_id, stage, artifact_type, content, refs_json, parser_version, validated, version, created_at`
- 约束：内容、规范化引用和版本可追溯（FR-303/305）
- 校验：`refs_json` 引用的需求/任务 ID 必须存在（FR-306 检查无效引用）；`validated` 与门禁结果一致

### events
- 字段：`seq, id, session_id, task_id, request_id, tool_call_id, kind, payload_json, ts, persistence_level`
- 约束：`seq` 在工作区内唯一且单调递增（§7.1）
- 校验：`kind` ∈ 事件类型枚举（contracts/event-schema.md）；`persistence_level` ∈ `full/metadata/redacted/omitted_by_policy`；payload 尺寸有上限，大正文走引用（R-05 风险对策）

### messages
- 字段：`id, session_id, role, content_ref, token_count, ts, parent_id`
- 约束：超大正文使用内容表或分块存储；查询必须有界（§7.1）
- 校验：游标键 `(session_id, ts, id)` 或等价稳定全序（§7.2）

### context_windows
- 字段：`id, session_id, window_start_seq, window_end_seq, total_tokens, summary_ref, created_at`
- 约束：记录摘要与原文范围关系（FR-107）
- 校验：`window_start_seq ≤ window_end_seq`；压缩后原始历史仍可查（FR-107）

### session_overview
- 字段：`session_id, by_stage_json, total, done, blocked, refreshed_at`
- 约束：可重建的预聚合数据，不作为唯一事实源（FR-506、§7.1）
- 校验：明确"强一致字段"与"最终一致概览字段"；提供重建逻辑（§7.3-8）
- 实现（T037）：`session_id` 为强一致键；`by_stage_json`、`total`、`done`、`blocked`、
  `refreshed_at` 为最终一致字段，可由 `tasks` 重建（`overview.rs`）

### checkpoints
- 字段：`id, task_id, step, delta_json, message_cursor, artifact_refs_json, ts`
- 约束：增量差分与引用，避免每步复制完整历史（FR-402）
- 校验：与对应状态变化在同一事务提交（FR-403、ADR-007）

### tool_executions
- 字段：`id, tool_call_id, state, input_hash, started_at, finished_at, output_ref, side_effect_ref`
- 约束：支持恢复与不确定操作核对（FR-410、ADR-007）
- 校验：`state` ∈ `prepared/running/completed/failed/timed_out/interrupted/unknown`；副作用发生后未登记结果的记录必须可被恢复流程识别为 `unknown`

### audit
- 字段：`id, actor, action, target, detail_json, ts`
- 约束：强制放行、权限敏感操作和清理必须记录（§10.3）
- 校验：不随普通会话 TTL 自动清除，除非管理员显式设置审计保留策略

### runtime_state
- 字段：`name, enabled, backend, version, installed_at, payload_path`
- 约束：明确功能、后端和安装状态（§7.1）
- 校验：`enabled=false` 的运行时不得被工具注册表加载（FR-704）

### runtime_executions
- 字段：`id, task_id, runtime, script_hash, input_hash, runtime_fingerprint, state, exit_code, duration_ms, peak_mb, stdout_ref, stderr_ref, ts`
- 约束：执行追踪；内容按大小上限保存（§7.1）
- 校验：重复执行缓存仅用于纯函数或显式启用缓存的任务（FR-707）；stdout/stderr 引用遵守输出上限（FR-705）

### schema_migrations
- 字段：`version, applied_at, checksum`
- 约束：迁移可审计且不可重复误执行（§7.1）
- 校验：迁移失败时数据库不得被标记为新版本（§7.4）；从上两个受支持 schema 版本可迁移（NFR-103）

## 2. 状态机（§3.1 状态常量）

| 状态机 | 状态集 | 约束 |
|---|---|---|
| 任务 Task | `pending, ready, running, blocked, completed, failed, aborted, interrupted, unknown` | 每次转换记录来源阶段、目标阶段、执行者、校验结果与时间戳；失败或未测试的验收条件不得使任务静默变为 completed（FR-311） |
| LLM 请求 | `queued, started, streaming, completed, failed, interrupted` | 流式期间可合并展示更新，但状态转换不得丢失（FR-206） |
| 运行时执行 | `prepared, running, completed, failed, timed_out, interrupted, unknown` | 超时/终止时记录原因、退出状态、stderr 尾部（FR-409）；副作用不确定 → `unknown`（FR-410） |
| SDD 阶段 | Constitution → Specify → Plan → Tasks → Implement → Converge（含 Clarify/Analyze/Checklist 门禁） | 应用层状态机；转换由工作流配置驱动并经应用服务校验（FR-302/308） |

状态名在代码、数据库、CUI JSON Schema 和 Web 协议中使用同一套枚举定义（§3.1）；枚举以 `whcode-domain` 为唯一出处。

## 3. 索引与游标（§7.2）

- `events`：至少 `(session_id, seq)`、`request_id`、`task_id`
- `messages`：`(session_id, ts, id)` 或等价稳定游标键
- `tasks`：`(session_id, status)`、`stage`、依赖引用
- 游标必须包含能形成稳定全序的键；向前/向后翻页使用不同明确的比较方向；不得仅以时间戳分页
- 索引与查询计划通过真实基准验证；不规定"所有全表扫描都失败"（小表或完整聚合的扫描可能是正确计划）

## 4. 事务与写入队列（§7.3）

1. 写者服务接收结构化命令，命令携带 `idempotency_key` 与期望的状态版本（适用时）
2. 队列有界；超容量时关键命令返回明确背压/失败，禁止无限堆积
3. 普通写入合并为事务：达到 500 行或 50 ms 任一阈值即提交（FR-503，M0/M1 基准校准）
4. 关键事件与危险操作意图提供显式 flush/提交确认
5. 数据库事务成功后才返回持久化确认；失败返回错误并保留可重试信息
6. 同一幂等键重复到达返回既有结果或确定冲突（V-106）
7. 查询结果流式迭代或有界批次，禁止长历史路径调用等价 `fetch_all` 的无界收集
8. 预聚合表可异步更新，但须区分强一致/最终一致并提供重建逻辑

## 5. 备份、导出与迁移（§7.4）

- 导出格式含版本、工作区元数据、产物、事件和内容清单；大对象使用可校验分块格式
- 导入前检查 schema 兼容性、文件路径、重复 ID、哈希和剩余磁盘空间
- 迁移前生成可恢复备份或确认已有备份；失败时数据库不得被误标记为新版本
- 压缩/重建需独占维护锁 → 先验证副本 → 再原子切换文件；Windows 文件替换与 BSD 文件锁语义分别测试（R-09）
