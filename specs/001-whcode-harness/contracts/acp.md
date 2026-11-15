# ACP automation-only 契约（P6.3／FR-789／ADR-081）

本契约把 Agent Client Protocol v1（agentclientprotocol.com，2026-11-16 取读）的 Agent 侧
automation-only 形态固化为可判定的接口约定。上游语义变更以标准基线行（`standards-baseline.md`）
的复审触发为准。

## 1. 传输与形态

- 传输：stdio 上的 JSON-RPC 2.0；每帧一行 JSON；帧 ≤64 KiB（超限整帧丢弃并回
  `-32600`）；单活跃 turn；单工作区（`--workspace` 或配置，装配期绑定）。
- 启动：`whcode acp serve [--provider NAME]`（客户端以子进程方式启动；stdin EOF 即结束）。
- `clientInfo`／`agentInfo` 是自报信息，不用于任何安全决策；审计身份固定 `acp-local`。

## 2. 方法（Agent 侧实现）

| 方法 | 语义 | 结果 |
|---|---|---|
| `initialize` | 版本协商＋能力交换 | `protocolVersion:1`、`agentCapabilities{loadSession:false, promptCapabilities{image/audio/embeddedContext:false}, _meta.whcode.automation_only:true}`、`agentInfo`、`authMethods:[]` |
| `session/new` | 创建会话 | `{"sessionId": "<本仓会话 ID>"}`；`cwd` 必须与绑定工作区一致（否则 `-32602`）；`mcpServers` 非空即拒绝（`-32602`） |
| `session/prompt` | 执行一个 turn（阻塞至终态） | 先 `session/update`（`agent_message_chunk` 一条＋工具通知），后 `{"stopReason": ..., "_meta.whcode": {termination_reason, request_id}}` |
| `session/cancel`（通知） | 取消活跃 turn | 无响应；原始 `session/prompt` 以 `stopReason:"cancelled"` 应答；无活跃 turn 时忽略 |
| `$/cancel_request`（通知） | 同上（JSON-RPC 通用） | 同上 |

错误码：`-32600` 未初始化／坏帧、`-32601` 未知方法、`-32602` 参数与能力拒绝、`-32603`
内部错误、`-32001` 服务端状态（活跃 turn 占用）。`-32000`（ACP `auth_required`）不使用——
automation-only 无鉴权流程。

## 3. 能力边界（显式不实现）

能力位为 false 或缺失即不支持（ACP 口径）：`session/load`、`fs/*`、`terminal/*`、
elicitation、MCP 能力、逐块流式一律不提供；客户端发来即 `-32601`。非 text 内容块
（image／audio／resource link）→ `-32602`（本端只广告 text）。

## 4. 审批口径（本契约核心）

- 工具审批由**规则**决策：`config.approval.to_policy()`（非交互口径）＋ shell 默认拒绝种子
  （仅当配置未显式写 `tools.shell`）＋ `approval_rules` 表（最后叠加，优先生效）。
- 规则解析为 `ask` 的调用以 `approval_unavailable` 结构化失败（不中断 turn、审计在案）——
  **`session/request_permission` 永不发出**，决策不出进程。
- 想要无人值守放行某工具（含 shell）：配置 `approval.tools.<tool> = "allow"`，或经既有规则面
  登记 `approval_rules`（GUI 撤销面板／CUI）。**不存在协议层的“全部允许”**。
- 每条决策（allow／deny／unavailable）落 audit（store-backed 闸门；审计失败阻断执行）。

## 5. 通知形状

```json
{"jsonrpc":"2.0","method":"session/update","params":{
  "sessionId":"S-1",
  "update":{"sessionUpdate":"tool_call","toolCallId":"call-1","title":"shell",
            "name":"shell","kind":"execute","status":"in_progress"}}}
```

- `tool_call`（开始，`in_progress`）与 `tool_call_update`（终态：`ok`→`completed`，
  其余→`failed`；v1 状态枚举 pending／in_progress／completed／failed）。
- `agent_message_chunk`：turn 终态后一次送达（v1 无逐块流式，ADR-081 已知限制）。
- `stopReason`：`cancelled` 直传；其余终态一律 `end_turn`，精确原因在
  `result._meta.whcode.termination_reason`（ACP `_meta` 扩展点；凭据不进 `_meta`）。

## 6. 验证点

- 协议：`cargo test -p whcode-bin --bin whcode acp`（7 项：解析三分、能力最小集、
  prompt 提取、映射、两条端到端）。
- 审批口径：`automation_policy_seeds_deny_and_honors_configured_and_stored_rules`
  （shell 种子拒绝、配置优先、已存规则覆盖）。
- 契约测试：`Command::Acp` 穷尽映射 `acp.serve`（`surface_map` 编译期强制）＋
  `feature_parity_matrix_rows_match_registry`。
