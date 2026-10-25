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
| cert | status / modes / ca / server / verify | TLS 证书状态与编辑（FR-626／FR-627，Web 设置面；CUI／TUI／GUI 按 ADR-032 排除） |
| doctor | — | 环境与配置自检 |

**统一语义**（四端必须一致，FR-604）：

1. 破坏性命令（delete/archive/purge/compact/force-advance）必须支持 dry-run 或确认机制，并写审计
2. `spec force-advance` 必须要求 `--reason`，仅可越过明确列出的门禁（FR-312、§8.1）
3. 权限拒绝、阶段门禁失败、能力缺失均返回**结构化错误**，不得静默降级或跳过
4. 涉及外部副作用的操作遵循"意图—执行—登记"（ADR-007）：意图与检查点先提交

### 2.1 能力登记表与四端状态（FR-619—621、ADR-032）

命令面由 `whcode-app::surface` 的机器可读登记表定义为单一事实源，本节命令清单与
`docs/matrices/feature-parity.md` 均以它为校验基准。每条登记包含：

| 字段 | 说明 |
|---|---|
| `SurfaceId` | 稳定唯一标识（命令／视图） |
| `SurfaceKind` | Command／View |
| `Capability` | 执行所需能力（经 `authz::require` 统一判定，四端不得自行判权） |
| `EffectClass` | Read／Write／Destructive |
| `Confirmation` | None／PreviewConfirm／ReasonRequired |
| `Audit` | 是否强制审计 |
| 四端状态 | CUI／TUI／GUI／Web 各自为：已实现（带前端锚点）／已排除（理由码 + 锚点）／预留（里程碑）／不适用 |

四态判定表与理由码全表见 ADR-032；登记为排除或预留的能力在该端被调用时按 §4 返回
`not_supported`。任何前端不得在登记表之外自行宣布支持或不支持某能力；矩阵文件由
登记表校验，手工漂移即对等门禁失败。

### 2.2 LLM 调用：`llm run` / `llm cancel`（T167、FR-201—208、ADR-031）

**接线状态（2026-10-25）**：CUI（`cui_dispatch::llm_command`）与 Web（`POST /api/llm/run`、
`POST /api/llm/cancel`）已登记为已实现；TUI 仍为 `Reserved`（`M3-llm`），GUI 仍按
`gui_read_only_v1` 排除——两端被调用时按 §2.1 返回 `not_supported`。真实 HTTP 传输
（`whcode-llm` 的阻塞式客户端，T163）尚未交付：装配路径返回结构化 `invalid_state`，
**不伪造调用成功**；本节的传输语义在客户端交付前不生效。

**命令形状**（应用层 `whcode-app::llm`，四端共用；呈现层只做参数与信封）：

| 字段 | `llm run` | `llm cancel` |
|---|---|---|
| `session` | 必填；会话必须存在（否则 `not_found`） | — |
| `provider` | 必填；剖面名（未注册 → `invalid_state` 并列出已注册名） | — |
| `prompt` | 必填且非空（不隐式读取已存内容） | — |
| `model` / `max_tokens` | 可选覆盖（缺省取剖面的 `model` / `max_tokens`） | — |
| `request_id` | 可选预置（缺省按会话／剖面／提示词／时间生成）；`llm cancel` 的键 | 必填 |
| `trace_level` | CUI 可选 `off`/`metadata`/`redacted`/`full`（缺省 `redacted`；未知值 `parse_error`） | — |

CUI 形态：`whcode llm run --session ID --provider NAME --prompt TEXT [--model M]
[--max-tokens N] [--request-id R] [--trace-level LEVEL]`；`whcode llm cancel --request R`。
Web 形态：JSON 体同表字段（`session`／`provider`／`prompt` 必填）。

**凭据来源**（ADR-031 决策 6、NFR-213）：

- 剖面文件路径只来自环境变量 `WHCODE_LLM_PROFILES`（仓库外 JSON；≤ 64 KiB、≤ 32 个剖面）；
  未设置或无效时 `llm run` 返回 `invalid_state`（默认离线：无剖面即无请求）。
- 剖面 `auth` 只允许 `CredentialRef::Env`（环境变量名）或 `CredentialRef::File`
  （仓库外文件路径）；值封装在 `Secret`，`Debug`／`Serialize` 恒为 `[REDACTED]`，
  不进入事件、日志、错误 `details` 与输出。
- 取消标记目录：`WHCODE_LLM_CANCEL_DIR`，缺省系统临时目录下 `whcode-llm-cancel`；
  `llm run` 写 `<request_id>.live`（RAII 清理），`llm cancel` 只在 `.live` 存在时写
  `<request_id>.cancel`，未知请求返回 `not_found`（不制造无主标记）。

**超时默认值**（剖面可覆盖，ADR-031 决策 7）：连接 10 s、读取 120 s（两次读取之间）、
总时限 300 s；`max_tokens` 默认 1024。流式有界：chunk ≤ 1024、正文累计 ≤ 1 MiB，
超限显式标记 `truncated`（FR-208）。

**取消语义**：`llm cancel` 在**下一个 chunk 之前**生效（`details.effective =
before_next_chunk`）；真实 HTTP 读循环中的取消归 T166，当前不宣称立即中止。
Web 为单线程服务循环，同进程请求不能互相取消；跨进程（CUI `llm cancel`）经标记文件生效。

**错误码映射**（四端同源；CUI 退出码见 [cui-cli.md](./cui-cli.md) §2，Web 为同码 JSON 信封）：

| 情形 | `code` | CUI 退出码 | Web |
|---|---|---|---|
| 权限不足（`Capability::SessionCreate`） | `permission_denied` | 10 | 403 |
| 会话不存在 / 取消未知请求 | `not_found` | 60 | 404 |
| 未配置剖面、未知剖面、空提示词、凭据缺失、端点非法、传输层未就绪 | `invalid_state` | 50 | 400 |
| 未知 `trace_level`、请求体非 JSON、`max_tokens` 类型错误 | `parse_error` | 80 | 400 |
| 该端登记为预留／排除（TUI、GUI） | `not_supported` | 55 | 501 |
| 供应商 429 | `backpressure`（`retryable=true`，含 `retry_after_ms`） | 40 | 500（现状映射） |
| 供应商 401/403 | `permission_denied` | 10 | 403 |
| 供应商 5xx | `internal`（`retryable=true`） | 99 | 500 |
| 连接 / TLS / 读取超时 | `io_error` / `timeout` | 70 / 90 | 500（现状映射） |

**有界事件**（`llm.run` 结果中的 `events`，只含标识与计数，不含提示词或密钥）：
`llm.request_started`（`request_id`／`provider`／`protocol`／`model`／`max_tokens`）、
每 chunk 一条 `llm.delta`（`index`／`chars`）、`llm.finished`（`chunks`／`completed`／
`truncated`／`integrity_gaps`／`finish_reason`／`usage`）。`completed = false` 或
`integrity_gaps > 0` 表示完整性缺口（FR-208），不得据“有正文”推断成功。

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
| 登记为预留或排除的能力在该端被调用 | 错误 `code=not_supported`，`retryable=false`，`details` 含 `surface_id`、`state`（reserved／excluded）与 `milestone` 或 `reason_code`；CUI 退出码 55，Web 返回 HTTP 501 + 同码 JSON 信封；**未登记路径仍为 404**，不得以 404 或静默成功掩盖已登记能力的不可用 | FR-620、ADR-032 |

## 5. 内部接口语义（§8.4，trait 签名留实现设计）

- **LLM Provider**：模型能力探测、请求取消、流式结果、计量来源、超时和供应商错误归一化
- **Tool Executor**：参数 schema 校验、权限策略、执行意图持久化、超时/取消、stdout/stderr 限制与结果引用
- **Runtime Provider**：`name`、`capability`、`is_available`、`env_fingerprint`、`execute`、`terminate`；返回退出码、持续时间、资源观察值与输出引用（详见 [runtime-protocol.md](./runtime-protocol.md)）
- 所有异步执行操作必须有取消语义；底层无法立即取消时，须说明取消请求何时生效

## 6. 一致性测试要求

- 命令级契约测试：同一命令在四端的最终状态、权限判定与错误码一致（V-109）
- 事件协议测试：广播与持久化游标补齐结果一致，关键事件无缺失（V-103）
- 能力矩阵同步：新能力必须在同一 PR 内定义 API、CLI 行为、权限与测试（FR-610）
