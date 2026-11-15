# ADR-081：ACP automation-only 服务端（P6.3）

- **状态**：已批准（2026-11-16，proposal-v3 P6.3；用户指令“批准”）
- **来源**：`research/synthesis/proposal-v3.md` P6.3（域 I/C：“把‘无人值守’从‘绕过审批’改成
  ‘换一个可审计的决策者’”）；ACP v1 一手规范（agentclientprotocol.com，2026-11-16 取读：
  overview／initialization／session-setup／prompt-turn／tool-calls／cancellation／security）；
  ADR-031／ADR-018（分层）；ADR-070（shell 审批）；本仓 SDK 通道（ADR-071，`sdk.rs`）的
  网关与传输骨架
- **责任人**：互操作面维护者、审批与审计维护者
- **退出条件**：见 §6（绑定命令与失效条件）

## 1. 问题与边界

编辑器生态通过 ACP 驱动编码代理。本仓已能被“人”驱动（四端），但无人值守场景（CI、脚本、
编辑器后台）只有两条路：绕过审批（被 ADR-070 拒绝）或每件工具都失败（`approval_unavailable`，
安全但不可用）。P6.3 的价值：把无人值守的**决策者显式化**——决策者是“配置规则表＋本地已存
规则”，每条决策落审计，无规则命中即显式失败；同时在 ACP 线上把这一切以协议形态暴露给
自动化客户端。

边界（非目标，均显式登记）：

- 只实现 **Agent 侧**；不做 ACP 客户端（不去驱动别的代理）；
- 不做 `session/load`／`fs/*`／`terminal/*`／elicitation／MCP 能力（能力位恒 false，客户端
  MUST NOT 调用——ACP initialization 的口径）；
- 不做逐块流式（v1 的 turn 网关是阻塞式，正文在终态后一次送达；`tool_call` 通知是实时的）；
- 不做 HTTP／WebSocket 传输（stdio，与 SDK 通道同纪律）；
- **永不发出 `session/request_permission`**：决策不出进程，客户端不是决策者。

## 2. 决策

1. **形态**：`whcode acp serve`（stdio 上的 JSON-RPC 2.0；帧 ≤64 KiB；单活跃 turn）。
   协议版本只支持 `1`：客户端请求更大版本时按 ACP 规则响应本仓支持的最新版（`1`）。
2. **能力最小集**（`initialize` 响应）：`loadSession:false`、
   `promptCapabilities{image:false,audio:false,embeddedContext:false}`、`authMethods:[]`、
   `agentCapabilities._meta.whcode.automation_only:true`——**用 ACP 扩展点显式声明
   automation-only**，不做隐藏旁路。能力位为 false 的输入（非 text 内容块、`session/load`）
   一律显式错误，不降级猜测。
3. **会话映射**：工作区在装配时绑定（`--workspace` 或配置），`session/new` 的 `cwd` 必须
   与绑定工作区一致（不一致即 `invalid params` 显式拒绝——本仓的库与工具根都在装配期确定，
   不做“首个会话决定工作区”的隐式行为）；`mcpServers` 非空即拒绝（本仓 MCP 是**客户端**
   装配，不接受客户端注入服务器清单——提示注入面的直接关闭）；ACP sessionId 即本仓会话 ID。
4. **prompt turn**：`session/prompt` 的 text 块按序合并为 turn prompt；turn 经
   `run_turn_with_approval` 执行，观察者把 `ToolStarted`／`ToolFinished` 实时映射为
   `session/update` 的 `tool_call`／`tool_call_update`（kind 近似映射：
   读→`read`、写→`edit`、shell→`execute`、检索→`search`、其余→`other`；status 映射
   `ok`→`completed`、其余→`failed`——v1 的 `ToolCallStatus` 枚举经 schema 源码与 docs.rs
   v1.4.0 双源确认为 pending／in_progress／completed／failed）；正文在终态后以
   单条 `agent_message_chunk` 送达；响应 `stopReason`：`cancelled`↔`cancelled`，其余终态→
   `end_turn`，精确原因经 `result._meta.whcode.termination_reason` 透传（ACP extensibility
   允许 `_meta` 自定义数据；凭据不进 `_meta`——安全节口径）。
5. **审批口径（本 ADR 核心）**：闸门策略＝`config.approval.to_policy()`（**非交互口径**）＋
   **shell 默认拒绝种子**（仅当配置未显式写 `tools.shell` 时——无人值守新面的默认收紧，
   显式、可改、可撤销）＋ `approval_rules` 表（GUI／CUI 管理的规则，最后叠加、优先生效）；
   应答器恒为 `None`——规则解析为 `ask` 的调用以 `approval_unavailable` 结构化失败
   （不中断 turn、审计在案）。决策者＝显式配置＋规则表，每条决策落 audit；想要无人值守
   放行某工具（含 shell），就走既有规则面（配置或 `approval rule` 命令），**不存在协议层的
   “全部允许”**。
6. **取消**：`session/cancel` 通知 → 与 `llm cancel` 同路径写取消标记；原始 `session/prompt`
   以 `stopReason:cancelled` 应答；无活跃 turn 时通知忽略（不报错）。
7. **审计身份**：固定 `acp-local`；`clientInfo`／`agentInfo` 是自报信息，不用于任何安全决策
   （ACP 安全节口径）。

## 3. 威胁模型（涉安全条目，proposal §6.4 要求）

| 维度 | 事实与处置 |
|---|---|
| 伪造（S） | `clientInfo` 自报 → 只存日志字段，不参与鉴权；审计身份固定 `acp-local`，与实际执行者绑定 |
| 篡改（T） | 传输是本机 stdio 直连；不做 TCP／代理；中间人＝本机其他进程，超出威胁模型（ACP 安全节同口径） |
| 抵赖（R） | 每条审批决策（allow／deny／unavailable）经闸门 store-backed 路径落 audit；审计失败阻断执行（T264 口径） |
| 信息泄露（I） | prompt 不进事件（既有）；错误文本不带凭据；`_meta` 只放 `termination_reason`；工具输出经既有脱敏管线后才进通知 |
| 拒绝服务（D） | 帧 ≤64 KiB；单活跃 turn（第二次 `session/prompt` 显式拒绝）；取消不触库（标记文件） |
| 提权（E） | shell 默认拒绝（配置或规则显式放行才生效）；规则为 `ask` 时 `approval_unavailable`（无应答面），客户端不可覆盖；`mcpServers` 注入拒绝；`session/request_permission` 永不发出 |

**0 信任校验**：能力最小集、版本协商、非 text 拒绝、未知方法、无规则 unavailable、规则命中
放行——全部由测试钉住（Check 442—445）。

## 4. 正负和解

**正面**：无人值守从“不可用”或“绕过”变成“显式规则驱动的自动化”；编辑器生态可直连；决策
全程可审计。

**负面与承担者**：

- 无人值守放行某工具必须预先写规则——承担者是运维者（这正是目的：把决策成本前置）；
- v1 无逐块流式（长回复一次送达）——承担者是 ACP 客户端（收到的是完整正文）；
- `mcpServers` 注入被拒——承担者是想要 ACP 内联 MCP 的用户（用本仓 `.zcode/mcp.json`）。

**和解理由**：审批语义不因“对面是机器”而改变，只把“谁来答”从人换成规则表——这既保住
ADR-070 的边界，又让自动化真实可用。

## 5. 三重时间与回退

- **过去**：SDK 通道（ADR-071）已把 turn 编排、取消、审计身份做成可注入形态；ADR-070 把
  shell 免审批收口到白名单；交互装配的 `shell = ask` 种子是“破坏性工具默认确认”的现行
  和解——automation-only 把它换成“默认拒绝”，因为没有人可问；proposal §5 反对“绕过审批”。
- **现在**：`crates/whcode-bin/src/acp.rs`（协议层＋服务循环＋网关）＋ CUI `acp serve`＋
  surface `acp.serve`；判据见 Check 442—445。
- **将来失效条件**（绑定命令＋判定）：`cargo test -p whcode-bin acp` 红 → 停止交付；上游
  schema release 升 v2 或改 v1 语义（标准基线行复审触发）→ 复审本 ADR；出现“自动化需要
  逐块流式”的真实需求 → 复审决策 4（需要给网关加进度通道，非本批）。
- **回退**：删除 `acp.rs`、CUI 命令、surface 条目与矩阵行即可；无 schema 变更、无数据迁移。

## 6. 已知限制与失效条件

- **无逐块流式**：正文在 turn 终态后一次送达（决策 4）；`tool_call` 通知是实时的；
- **无会话恢复**：`loadSession:false`，连接断开即客户端需重新 `session/new`（本仓会话数据
  仍在库里，但 ACP 层不提供重放）；
- **单连接单工作区**：工作区在装配时绑定，`session/new` 只接受一致的 `cwd`（与 `serve`
  一致）；
- **不支持 ACP 内联 MCP**：`mcpServers` 非空即拒绝（决策 3）；
- **`stopReason` 精确值在 `_meta`**：ACP v1 枚举未覆盖 `iteration_limit`／`budget_exceeded`
  等本仓终态，统一映射 `end_turn`，精确值在 `_meta.whcode.termination_reason`（客户端按
  ACP 规则可忽略 `_meta`，信息不丢失仅不可见）；
- **停止原因枚举为 [待验证] 子集**：本批只使用 `end_turn`／`cancelled` 两个一手确认过的值；
  其余枚举值（如 refusal）未逐一取读，使用处显式标注。

## 7. 文献依据（标准研读记录，§1.6）

- **ACP v1（agentclientprotocol.com，2026-11-16 取读）**：初始化与版本协商（MUST 响应代理
  支持的最新版）、能力缺失即不支持（MUST 视为 UNSUPPORTED）、`session/cancel` 后原始请求
  MUST 以 `cancelled` 或 `-32800` 应答、`session/request_permission` 是客户端方法且
  “Permission requests are advisory: the Agent performs its own operations”——本仓的
  fail-closed 闸门正是“Agent 负责执行决定”的实现；
- **ACP security.md**：agent 内容不可信、凭据不进 `_meta`、代理写入审批控制文件前应请求
  权限——本仓以规则表承载同一诉求；
- ISO/IEC/IEEE 29148:2018（已登记）：判据可判定——Check 442—445 以命令与红灯表述；
- 一手先例：ADR-071（SDK 的 JSON-RPC stdio 纪律与帧上限）、ADR-070（shell 免审批边界）。

## 8. 形式审查

- **真实矛盾**：无人值守要么绕过审批（违宪）要么全失败（不可用）；规则表决策者同时解决
  两个矛盾；
- **正负和解**：见 §4，成本前置到规则配置，承担者明确；
- **0 信任校验**：见 §3，测试钉住；
- **知难行易**：方向＝P6.3“机器审批 answerer 显式化”；目标＝ACP 线上无人值守可用且每条
  决策可审计；判据＝协议测试＋审批口径测试全绿；非目标＝客户端实现、流式、fs/terminal
  能力、MCP 注入。
