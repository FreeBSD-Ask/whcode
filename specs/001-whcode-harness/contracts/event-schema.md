# Contract: 事件 schema、分级与脱敏

**Feature**: [spec.md](../spec.md) | **Date**: 2026-10-20

**来源**：任务书 §4.2（事件可靠性与追踪）、§8.3（事件结构示例）。事件是跨端唯一的状态变更通道。

## 1. 事件类型（统一枚举，FR-201）

请求开始、流式增量、请求完成/失败、上下文状态、进度变化、工具调用开始/结束、运行时终止、持久化异常。

示例（字段与语义必须与实现一致；`schema_version` 必备）：

```json
{"schema_version":1,"kind":"RequestStarted","event_id":"E-100","request_id":"R-889","session_id":"S-001","ts":"2026-10-20T06:00:00Z"}
{"schema_version":1,"kind":"StreamingChunk","event_id":"E-101","request_id":"R-889","seq":12,"persistence":"omitted_by_policy"}
{"schema_version":1,"kind":"RequestCompleted","event_id":"E-102","request_id":"R-889","prompt_tokens":2100,"completion_tokens":540,"latency_ms":7321,"cost_estimate":0.0042,"cost_is_estimate":true}
{"schema_version":1,"kind":"RuntimeTerminated","event_id":"E-103","runtime":"python","reason":"timeout","state":"timed_out","stderr_ref":"A-900"}
```

## 2. 必需字段与关联链

| 字段 | 必需性 | 依据 |
|---|---|---|
| `schema_version` | 必须 | §8.1/§8.3 |
| `kind` | 必须 | FR-201 |
| `event_id` | 必须 | 事件唯一标识 |
| `session_id` | 必须（会话内事件） | §7.1 |
| `request_id` / `task_id` / `tool_call_id` | 相关事件必须 | FR-204 链路串联 |
| `ts` | 必须 | §7.1 |
| `persistence_level` | 必须（持久化记录） | §7.1 |

## 3. 可靠性分级与背压（§4.2）

| 级别 | 内容 | 饱和语义 |
|---|---|---|
| 关键事件 | 任务状态转换、请求最终结果、工具执行意图与结果、门禁结果、人工强制放行、审计事件 | 不得静默丢弃；队列饱和时背压、失败返回或触发安全停止 |
| 内容事件 | 请求/响应正文与流式增量 | 由追踪级别控制是否持久化；省略或采样必须留下可查询标记 |
| 展示快照 | 帧率、重复进度通知等可重建信息 | 允许合并或丢弃 |

**禁止表述**：不得把"实时广播 + 数据库写入"描述为自动原子双写。实现必须明确事件何时被确认持久化，并处理两种半态："广播已发出、写入失败"与"写入已成功、客户端未收到"。

## 4. 追踪级别与脱敏（FR-203/207/208、§10.1）

| 级别 | 语义 |
|---|---|
| `off` | 不记录追踪内容 |
| `metadata` | 仅元数据（时长、token 数、状态），无正文 |
| `redacted`（默认，D-05） | 记录内容但对疑似秘密字段脱敏 |
| `full` | 记录完整内容；必须在 UI 警告潜在敏感信息写入，并提供访问控制、清理与导出策略 |

- 估算的 token/成本必须标记为估算（`cost_is_estimate` 等），供应商返回的计量值另行保存（FR-207）
- 追踪记录必须提供完整性标记：是否存在缺失 chunk、脱敏、降采样或持久化失败（FR-208）

### 4.1 导出语义与四端复用（FR-205、FR-622、ADR-032）

- `events export` 的 JSON／JSONL 输出与脱敏注明（FR-205）由 CUI 与 Web 复用同一导出
  路径与同一脱敏策略；导出响应必须注明追踪级别与脱敏标记，不得因前端不同而改变
  脱敏口径（默认仍为 `redacted`，D-05）。
- 导出能力在登记表中登记为预留或排除的端，被调用时返回 `not_supported`
  （CUI 退出码 55／Web HTTP 501，见 `contracts/app-service.md` §4 与 ADR-032），
  不得以 404 或空输出冒充“无事件”。
- 单事件查询（`events show`）应用层接口未交付，登记为预留：任何端请求均得
  `not_supported` 与里程碑指引，不得伪造单事件响应。

## 5. Schema 演进规则（§8.3）

- 事件 schema 必须声明字段必需性、兼容规则、敏感字段与最大 payload 尺寸
- 不得把大段响应正文重复塞进每个进度事件；大正文走内容表/引用（R-05 对策）
- 错误、stderr、模型输出与工具返回均视为不可信数据，不得作为控制指令执行（§10.1）
- 字段语义变化必须提升 `schema_version` 并做兼容审查（§15 PR 清单）
- **存储归一化（ADR-034/036）**：库内 `payload_json` 为 `VARIANT`；读回 JSON 文本会去除多余空白
  并排序对象键。线格式与字段语义不变，导出与断言按 JSON 值比较，不要求与写入原文逐字节相同。

## 6. 验证点

- 断线重连：按持久化游标补齐后，关键事件集合与在线流一致（V-103）
- 脱敏：`redacted` 级别下疑似秘密字段不出现在导出与测试快照中（V-110）
- 完整性标记：采样/脱敏/降采样事件可被查询识别（FR-208）
- 饱和：构造积压后关键事件无丢失，展示快照合并可观测（FR-206）
