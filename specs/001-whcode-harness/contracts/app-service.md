# Contract: 应用服务命令 / 快照 / 事件接口

**Feature**: [spec.md](../spec.md) | **Date**: 2026-10-20

**来源**：任务书 §6.1（逻辑分层）、§8.4（Provider/Tool/Runtime 接口约束）、ADR-003/004/005/009。
**适用方**：`whcode-tui`、`whcode-cui`、`whcode-web`、`whcode-ssh` 四个前端适配器，以及内部调用方（如 SDD 状态机）。

## 1. 边界规则

- 前端**只**通过本契约访问状态与命令；不得直接打开数据库或调用 `whcode-store` 写入路径（ADR-003/005）。
- 命令经权限检查、状态校验（含 SDD 阶段门禁）后由应用服务编排，返回**事件流**或**状态快照**。
- 三端对等的判据是业务能力、权限检查、状态迁移、错误语义与可审计性一致，不要求视觉/交互一致（ADR-009）。
- 任一前端产生的更改须对其他在线端可见；断线端按持久化游标补齐（FR-605）。

## 2. 命令面（能力清单，四端共用）

| 域 | 命令 | 说明 |
|---|---|---|
| session | new / list / show / rename / archive / delete / switch | 会话生命周期（FR-101/102） |
| task | list / show / retry / abort | 任务查询与控制（FR-103/104） |
| context | budget / sources / load / compact | 上下文预算与装入来源（FR-105/107/108） |
| llm | run / cancel | 发起与取消模型调用（FR-201—208） |
| spec | status / next / validate / analyze / converge / force-advance | SDD 阶段与门禁（FR-301—314） |
| events | list / show / export | 事件查询与导出（FR-202/205） |
| db | stats / check / purge / checkpoint / compact / archive / explain | 维护与清理（FR-507—510） |
| runtime | list / status / enable / disable / install | 运行时管理（FR-701—710） |
| serve | web / ssh | 传输入口（FR-603/607） |
| doctor | — | 环境与配置自检 |

**统一语义**（四端必须一致，FR-604）：

1. 破坏性命令（delete/archive/purge/compact/force-advance）必须支持 dry-run 或确认机制，并写审计
2. `spec force-advance` 必须要求 `--reason`，仅可越过明确列出的门禁（FR-312、§8.1）
3. 权限拒绝、阶段门禁失败、能力缺失均返回**结构化错误**，不得静默降级或跳过
4. 涉及外部副作用的操作遵循"意图—执行—登记"（ADR-007）：意图与检查点先提交

## 3. 状态快照（四端同源）

字段与语义见 [state-snapshot.md](./state-snapshot.md)；快照必须携带 `schema_version`、`event_cursor`，使前端可用游标增量补齐（FR-202/605）。

## 4. 失败与门禁语义

| 情形 | 返回 | 依据 |
|---|---|---|
| 权限不足 | 错误 `code=permission_denied`，`retryable=false` | §10.2、FR-604 |
| SDD 门禁失败 | 错误 `code=gate_failed` + 未通过门禁清单与产物版本 | FR-307/308 |
| 高优先级澄清未解决 | 拒绝 Plan 转换，`details` 含澄清项 ID | FR-307 |
| 运行时能力不足 | 错误 `code=runtime_unavailable` + 安装/替代建议 | FR-708/710 |
| 写者队列饱和 | 关键命令 `code=backpressure`（`retryable=true`） | R-04、§7.3-2 |
| 外部操作不确定 | 状态 `unknown` + 核对/幂等重试/人工决策提示 | FR-410 |

## 5. 内部接口语义（§8.4，trait 签名留实现设计）

- **LLM Provider**：模型能力探测、请求取消、流式结果、计量来源、超时和供应商错误归一化
- **Tool Executor**：参数 schema 校验、权限策略、执行意图持久化、超时/取消、stdout/stderr 限制与结果引用
- **Runtime Provider**：`name`、`capability`、`is_available`、`env_fingerprint`、`execute`、`terminate`；返回退出码、持续时间、资源观察值与输出引用（详见 [runtime-protocol.md](./runtime-protocol.md)）
- 所有异步执行操作必须有取消语义；底层无法立即取消时，须说明取消请求何时生效

## 6. 一致性测试要求

- 命令级契约测试：同一命令在四端的最终状态、权限判定与错误码一致（V-109）
- 事件协议测试：广播与持久化游标补齐结果一致，关键事件无缺失（V-103）
- 能力矩阵同步：新能力必须在同一 PR 内定义 API、CLI 行为、权限与测试（FR-610）
