# ADR-064: 工具级交互审批接线（T255／FR-744／Check 349、ADR-057 B3a）

- **状态**：已实施（2026-11-09）；审批 UI（工具卡＋审批对话框）属 B3b，不在本 ADR 范围。
- **日期**：2026-11-09
- **关联**：ADR-057（决策 6：B3a 后端／B3b GUI 与 Web UI 分工）、ADR-060（项 A 类型与 broker；
  §2.2 执行接线冻结）、ADR-048（§2.4 登记边界、§2.7 权限面）、ADR-054（MCP 工具路径）、
  ADR-018（SQL 边界）、FR-744／745（spec `001-whcode-harness`）。
- **范围边界**：只做**后端执行接线**与规则解析；不做任何界面，不改四端渲染，不新增线程、
  crate 或依赖，不把“记住规则”做成持久化。ADR-060 正文不改，本 ADR 只说明解除其 §2.2 冻结的
  范围（见决策 7）。

## 问题

1. **类型齐、接线缺**：T264／ADR-060 项 A 已交付封闭四态、`ApprovalRule`、`ApprovalBroker`
   与成对审计，但 `execute_tool_call` 不调用它——审批在真实 turn 路径上**不存在**，
   规格里的 FR-744 只有类型证据，没有行为证据。
2. **默认值两难**：若接线时把默认规则设成 `ask`，无 UI 的 CUI／TUI／Web／SSH 场景下所有
   既有工具立刻变 `unavailable`（功能性回归，ADR-060 §2.2 的理由）；若完全不接线，
   “允许／询问／拒绝”三规则仍无处生效。
3. **规则来源缺失**：没有配置面就没有“显式设为 deny／ask”的入口，规则只能写死在代码里。
4. **失败语义易混**：拒绝、无人可问、审计失败三种情况都必须阻止执行，但它们在审计与模型
   语义上不同（拒绝＝明确拒绝；无人可问＝不可判定；审计失败＝系统无法留痕），
   二值化会丢掉可判定性。
5. **旁路风险**：工具面不止内置注册表——MCP 投影工具与子代理是另外两条执行入口，
   只在内置路径加闸门等于留后门。

## 决策

### 1. 规则解析顺序

1. **per-tool 覆盖**（`[approval].tools` 的 `<工具稳定名>` → 规则）优先；
2. 否则取**副作用范围默认**：`read_file`／`glob`／`grep` 等 `ReadOnly` 取
   `default_read_only`，`write_file`／`edit_file`／`shell` 等 `WorkspaceWrite` 取
   `default_workspace_write`；
3. 范围由 `ToolDescriptor.side_effect_scope` 给出，不另立映射表（避免两处事实源漂移）。

规则表在装配期构造一次（`ApprovalPolicy`），运行期只做查表；覆盖数量上限
`MAX_APPROVAL_TOOL_RULES = 256`，超限拒绝构造而不是静默截断。

### 2. 默认值与行为兼容

- `default_read_only = allow`、`default_workspace_write = allow`、`tools = {}`；
- 即**默认行为与接线前完全一致**：既有 turn 工具无条件放行，不产生审批审计；
- `ask`／`deny` **只能**由显式配置生效（`crates/whcode-bin/src/config.rs` 的 `[approval]` 段）；
- 破坏性工具（`shell`）默认切 `ask` 属交互体验决策，留给 B3b 与 UI 同批交付，
  届时须以“当批 UX 证据＋默认值可改可撤销”为前提另记修订，不在本 ADR 预设。

### 3. 无应答者语义（fail-closed 但不含糊）

- `allow` → `allowed_once`，直接执行；
- `deny` → `rejected`；`ask` + 明确拒绝 → `rejected`；
- `ask` + 无应答者／应答者报错／超预算 → `unavailable`；
- 三者**都阻止执行**，但只有后两者写审计的 `approval_decided`，且 `unavailable`
  不得在展示面被渲染为“用户拒绝”（`ApprovalOutcome` 四态封闭，禁止新增第五态）。

应答者经 `ApprovalAnswerer` trait 注入（装配根／测试），**同步调用、不建线程**；等待预算
`wait_ms` 默认 120 s、硬上限 600 s（越界按上限收口），由闸门在应答返回后复核实际耗时，
超预算的迟到应答被丢弃并按 `unavailable` 结算——本地不引入可中断等待所需的新线程。

### 4. 工具执行时序（唯一接入点）

`execute_tool_call` 内的顺序固定为：

1. 工具存在性 → 2. 权限面（`Capability`）→ 3. 参数 JSON 解析与 schema 校验 →
4. **审批闸门**（`ApprovalGate::authorize`）→ 5. 工作区绑定 → 6. 三步登记
（prepared→running→终态）→ 7. 执行 → 8. `role:tool` 回填。

闸门位于“参数校验之后、三步登记之前”：受理前拒绝不产生登记行（ADR-048 §2.4），
被拒绝的调用不接触执行面；参数校验先行使闸门看到的调用一定是可执行的调用，
避免“问了人却不执行”。MCP 投影工具在同一位置接入，并按 `WorkspaceWrite` 范围解析规则
（外部工具不区分读写，ADR-054 §2.8 口径一致）。

### 5. 审计口径

- `deny` 与 `ask` 写 `approval_asked` + `approval_decided` **成对**记录（含 `exec_id`、
  工具名、规则、答案、四态结论、静态备注）；
- `allow` **不写**审批审计：没有人参与就不产生审批记录，否则每次工具调用固定两份审计会
  稀释真正的审批痕迹（审计价值随条数下降，不是随条数上升）；
- 审计写入失败→ 闸门返回错误→ 工具调用以 `approval_audit_failed` 结构化失败，**不执行**
  （沿用 T264“审计失败阻断”口径：无法留痕的动作不得发生）；
- 审计与事件都只带 `exec_id`／工具名／规则，不带工具参数正文（见威胁模型 TOCTOU）。

### 6. 接入面与调用方

- 新增 `run_turn_with_approval(...)`；`run_turn`／`run_turn_with_progress` 签名不变，
  内部委托同一主体并传 `None`，因此四端与既有测试零改动、零行为变化；
- `ToolCallFinished` 事件新增 `error_code` 字段（`permission_denied`／`approval_unavailable`／
  `approval_cancelled`／`approval_audit_failed`／`tool_timeout`／`mcp_tool_error` 等），
  让“失败原因”可判定，不必从自然语言正文里猜；
- 子代理内部 `run_turn` 递归不带闸门：子 turn 的工具调用按父 turn 已放行的会话身份执行，
  本批不把审批请求二次下推（见已知限制）。

### 7. 解除 ADR-060 §2.2 冻结的范围

ADR-060 §2.2 的“A 的接线冻结”**只在本 ADR 决策 1—6 的范围内解除**：

- 解除：把 T264 的类型／broker 接入 `execute_tool_call`，并给出配置面与测试；
- 不解除：在默认配置下强制 `ask`、把默认值改成 `deny`、做参数级规则与“记住规则”持久化、
  做任何界面。ADR-060 正文保持原样，其 §2.2 第 1 条（“T255/B3b 接入审批 UI 时冻结解除，
  须重审默认策略与放行矩阵”）在本 ADR 中以“默认全 allow＋显式配置生效”兑现了一次重审。

## 威胁模型

| 威胁 | 路径 | 处置 |
|---|---|---|
| 审批绕过（内置工具） | 只读工具被当作无需审批 | 规则按 `side_effect_scope` 解析，`deny` 对只读工具同样生效 |
| 审批绕过（MCP） | 只在内置注册表加闸门 | MCP 路径同位置接入，按 `WorkspaceWrite` 解析（fail-closed） |
| 审批绕过（子代理） | 子 turn 另起执行 | 子 turn 不新建会话身份、复用父 turn 的权限面；审批不再下推（已知限制） |
| 审批绕过（默认值） | 默认 `allow` 被当成“已批准” | 默认 `allow` 是**行为兼容**而非授权结论：`ask`／`deny` 一旦配置即生效，审计区分 `allowed_once` 与“无人可问” |
| TOCTOU：展示面与执行面参数漂移 | 审批请求复制一份参数，展示后被替换 | 请求**只带** `exec_id`，参数只在已展示的工具调用上保留一份；闸门不持有参数副本 |
| 审计完整性 | 先执行后补审计，或审计失败仍执行 | 请求先写 `approval_asked` 再询问；审计失败直接阻断（请求不入表／结论不生效） |
| 参数入审计／日志 | 把工具参数塞进 `detail_json` 或错误消息 | 审计只带 `exec_id`／工具名／规则／静态备注；错误消息只引用工具名与规则名，不复述参数 |
| 模型诱导 | 模型把“无人可问”当作“已批准重试” | `unavailable` 与 `rejected` 都回填 `permission_denied`／`approval_unavailable` 且正文写明不可重试；`run_turn` 不把审批失败升级为 turn 终止，模型无法靠重试绕过 |
| 应答面被撑爆 | 队列无界或等待无界 | 待审批表 `MAX_PENDING_APPROVALS = 64`（满即 `backpressure`）；等待预算上限 600 s；规则表上限 256 |

## 正负和解

- **正面**：FR-744 从“类型存在”变为“行为存在”；规则可配置、可审计、可复现；
  失败语义四分（允许／拒绝／撤回／不可用）不再二值化；默认中性使接线本身不制造回归；
  `error_code` 让四端与测试都能判定失败原因。
- **负面**：新增一次同步等待路径（有 UI 时人会卡住 turn 线程）；`ask` 在无应答者时
  变成 `unavailable`，用户若配置了 `ask` 却没接 UI 会看到工具失败而不是弹窗；
  审计条数随审批次数线性增长；`allow` 不写审计意味着“为什么这个工具被允许”只能从配置面回答。
- **承担者**：装配根维护者（把配置读进 `ApprovalPolicy` 并注入应答者）、B3b 界面维护者
  （真正的人机应答面与“记住规则”）、运维／用户（配置错误由人修正，错误消息给出范围与上限）。
- **和解理由**：把“是否问人”降级为**配置**而不是代码分支，既保住无 UI 场景的可用性，
  又让有 UI 的场景能立刻收紧；代价是配置面多了一段需要维护的语义，因此把校验放在加载期
  并以“非法值拒绝、不静默回落”收口。

## 三重时间

- **过去来源**：ADR-057 决策 6（B3a／B3b 分工）、ADR-060 项 A（四态与 broker）、
  ADR-048 §2.4／§2.7（登记边界与权限面）、ADR-054 §2.8（MCP fail-closed）、
  ZCode 域 C 审批交互勘察（`research/zcode-comparison/`）。
- **现在证据**（2026-11-09，本地定向复跑；按 ADR-037 只作复现证据，不作验收结论）：
  - `cargo test -p whcode-app --lib` → 197 passed / 0 failed（含审批闸门 12 项与 turn 级 4 项：
    `approval_deny_fails_tool_with_permission_denied_and_turn_continues`、
    `approval_ask_without_answerer_is_unavailable_and_turn_continues`、
    `approval_ask_with_approve_executes_tool`、
    `approval_default_policy_keeps_existing_tool_behavior`）；
  - `cargo test -p whcode-domain` → 69 passed / 0 failed；
  - `cargo test -p whcode-bin --bin whcode config` → 12 passed / 0 failed（含审批默认值、
    per-tool 解析、非法值拒绝三例）；
  - `cargo clippy -p whcode-app -p whcode-domain -p whcode-bin --all-targets -- -D warnings`
    → 零告警；
  - `cargo fmt -p whcode-app -p whcode-domain --check` 与 `rustfmt --check`
    （`crates/whcode-bin/src/config.rs`）→ 无差异；
  - **同期已有失败不属于本 ADR**：`cargo test -p whcode-bin` 中
    `serve::tests::slow_consumer_loss_is_published_and_events_survive` 与
    `serve::tests::turn_ws_relay_first_delta_busy_codes_and_event_missing` 失败于
    “帧为 JSON”，属并行任务对 `crates/whcode-bin/src/serve.rs`（未提交，+897 行）的
    在途改动；本 ADR 的改动不触及该文件与 WS 中继路径。
- **将来失效条件与复审触发**（命令＋判定，日期仅作辅助）：
  1. `cargo test -p whcode-app --lib turn::tests::approval` 失败 → 接入位置或失败语义漂移，
     冻结本 ADR 决策 4 的新改动直至复现并修正；
  2. `[approval]` 默认值被改为 `ask`／`deny`（`crates/whcode-bin/src/config.rs` 的 `Default`
     或 `ApprovalPolicy::default()` 断言测试失败）→ 必须重审“无 UI 可用性”并同步 ADR-060 §2.2
     与 B3b 交付节奏；
  3. MCP 或新增工具类别出现第三、第四条执行路径（`execute_tool_call` 以外的工具执行入口）
     → 审批闸门覆盖面复审，未接闸门的路径必须显式登记为缺口；
  4. 应答者实现开始创建线程或执行阻塞 I/O 超过等待预算 → 资源铁律（NFR-215／216）复审，
     本 ADR 的“无线程”前提失效；
  5. 真实会话中 `approval_unavailable` 占工具失败的比例长期偏高（>30%，按事件统计判定）
     → 默认值或应答面装配复审。

## 已知限制与失效条件

- **无 UI 时 `ask` 等于不可用**：用户显式把某工具设为 `ask` 但没有应答者（CUI／TUI／Web／SSH
  当前均未注入 UI 应答者），该工具每次调用都会 `unavailable` 失败。这是设计选择而非缺陷：
  不得把“无人可问”静默当作允许；修复方向是 B3b 交付应答面，不是放宽语义。
- **装配根尚未接线前 `[approval]` 不生效**：本 ADR 交付的是规则解析、闸门与 turn 入口
  （`run_turn_with_approval`），生产调用方（`crates/whcode-bin` 的四个 turn 入口）仍走
  `run_turn`／`run_turn_with_progress`，因此配置文件里的 `[approval]` 段当前**只有解析与校验的
  消费者，没有生效路径**——这是显式登记的缺口（死配置），不是“已生效但没测到”。
  接线由装配批次（Lead 收口，`serve.rs`／`llm_provider.rs` 归 turn-relay 写域）完成：
  读 `config.approval` → `ApprovalPolicy` → `ApprovalGate`（无应答者＋默认 allow，故行为中性）。
  **该节在装配根接线完成后失效**，接线批次须在此追加修订记录并把本条从限制中移除。
- **子代理不走二次审批**：`spawn_subagent` 内部的工具调用不带闸门，审批只发生在父 turn 的
  调用上；若将来允许子代理跨会话执行，必须先补闸门再放开。
- **默认 `allow` 不产生审批审计**：这是审计噪声与审计完整性的取舍；审计能回答“谁被问过、
  答案是什么”，不能回答“哪些工具被默认放行”——后者以配置面为准。
- **等待不可中断**：应答者在调用线程上同步执行，若实现自身阻塞超过预算，闸门只能**事后**
  丢弃结论（工具失败），不能**事前**抢占；无 UI 线程化前不得把预算当作硬实时保证。
- **审计失败后请求可能滞留**：结算阶段审计失败时请求按 T264 口径保留在待审批表中以便重试，
  连续失败可占满 64 条上限并使后续审批进入 `backpressure`；恢复手段是修复审计写入后重新结算。
- **失效条件**：出现第二条参数来源（审批请求开始复制参数）、规则默认值改为 `ask`／`deny`、
  或应答面被改成异步／多线程，则本 ADR 决策 2、3、4 的前提不成立，必须先修订本 ADR 再改代码。

## 复审触发

1. `python .github/scripts/check_counts.py`：`test_approval` 用例数与 FR-744／Check 349 追溯
   不一致即冻结勾选；
2. `cargo test -p whcode-app --lib approval && cargo test -p whcode-bin --bin whcode config`：
   任一失败即触发本 ADR 复审（判定：退出码非 0）；
3. B3b 交付应答面时，必须同步重审决策 2（破坏性工具默认值）与决策 3（等待与取消语义）。

## 实施记录

- 2026-11-09（T255／Check 349）：
  - `crates/whcode-app/src/approval.rs`：新增 `ApprovalAnswerer`、`ApprovalPolicy`
    （per-tool 覆盖 > 范围默认）、`ApprovalGate`（规则→请求→应答→四态→成对审计）、
    `ApprovalDecision`，以及 `DEFAULT_APPROVAL_WAIT_MS`／`MAX_APPROVAL_WAIT_MS`／
    `MAX_APPROVAL_TOOL_RULES` 三个有界常量；`pub use` 再导出 `SideEffectScope` 供装配根。
  - `crates/whcode-app/src/turn.rs`：`run_turn_with_approval`（新增入口）与
    `run_turn_inner`（共享主体，既有入口传 `None`）；`execute_tool_call` 在参数校验之后、
    三步登记之前判定；MCP 路径同位置接入；返回值扩为五元组，`ToolCallFinished` 事件新增
    `error_code`。
  - `crates/whcode-bin/src/config.rs`：新增 `[approval]` 段（`default_read_only`、
    `default_workspace_write`、`tools`、`wait_ms`），加载期校验，非法值拒绝。
  - 测试：app `approval` 19 项（其中闸门新增 12 项）与 turn 级 4 项；bin `config` 3 项。
  - 验证链实测（2026-11-09，逐条本机复跑）：
    | 命令 | 结果 |
    |---|---|
    | `cargo test -p whcode-app --lib` | 197 passed / 0 failed |
    | `cargo test -p whcode-domain` | 69 passed / 0 failed |
    | `cargo test -p whcode-bin --bin whcode config` | 12 passed / 0 failed |
    | `cargo clippy -p whcode-app -p whcode-domain -p whcode-bin --all-targets -- -D warnings` | 零告警 |
    | `cargo fmt -p whcode-app -p whcode-domain -p whcode-bin -- --check` | 本批写域文件无差异；`crates/whcode-bin/src/serve.rs` 仍有差异，属 turn-relay 在途改动（不在本批写域，未改动） |
  - 未闭环：B3b 的人机应答面、按工具记住规则、四端渲染；装配根的 `[approval]` 消费方
    （见“已知限制与失效条件”第 2 条）；审批审计的持久化 `AuditSink` 实现仍由装配根在装配批次
    注入（本批以测试内存 sink 证明契约）。

## 修订记录

- 2026-11-09（首版）：规则解析、默认值、无应答者语义、执行时序、审计口径与威胁模型成文；
  明确解除 ADR-060 §2.2 冻结的范围，并把“破坏性工具默认 ask”留待 B3b。
- 2026-11-09（补记）：登记“装配根尚未接线前 `[approval]` 不生效”的缺口与失效条件；
  写明装配批次接线后须在本节追加修订并移除该限制。
- 2026-11-10（B3b 后端前置，四态与规则存储）：按用户决定落地——四态＝
  「批准（本回合内该工具不再问）／仅本次／拒绝（写 deny 规则）／记住规则（写 allow 规则）」；
  破坏性默认确认只落 `shell`，且仅在启用交互面的装配中生效。
  - `ApprovalAnswer` 扩展 `ApproveTurn`／`ApproveRemember`／`RejectRemember`（旧三态保留，
    CUI／TUI 行为不变）；`ApprovalGate` 增加回合放行集合与 `ApprovalRuleWriter` 端口；
    记住类应答在**执行线程、登记前**落库，存储未接线或写失败一律阻断执行。
  - schema v9 新增 `approval_rules`（只存 `allow`／`deny`，一条工具一行，上限 256，
    `CHECK` 拒绝 `ask`）；`whcode-store::approval_rules` 提供 list／upsert／delete。
  - 生产 `AuditSink`（`StoreAuditSink`）与规则写入器（`StoreRuleWriter`）落地，闭合
    T255 登记的 F4“生产无 AuditSink”缺口；`AuditSink` 的 `Send + Sync` 约束删除并写明
    “同步、单线程、可持有非 Sync 连接”（DuckDB 连接不得跨线程共享）。
  - `ApprovalPolicy::with_builtin_destructive_ask`／`with_stored_rules`：装配顺序为
    范围默认 → 内置 shell=ask（可选） → 配置覆盖 → 本地已存规则（后者优先）。
  - 新增能力 `approval.rule.read`／`approval.rule.write` 与 `app::approval_rules`
    门面（设置页用），变更写 `permission_change` 审计。
  - **装配接线（F3 的 Web 段）**：`serve_web` 启动时按“配置 → 本地已存规则”构造策略
    （内置 `shell = ask` 留待四态按钮落地后启用，避免用户没有可点的应答面）；
    turn 工作线程改用 `run_turn_with_approval`，闸门经 `authorize_with_conn` 用 turn
    自身连接写审计与规则（DuckDB 连接不可跨线程共享）；`InteractiveApprovals`
    （Mutex + Condvar，不建线程）承载人机应答；`GET /api/approvals/pending` 与
    `POST /api/approvals/decide` 不触库，turn 期间仍可达。
  - 未闭环（B3b UI 批次）：GUI／Web 工具卡与四态按钮、设置页入口；T256／T257 与
    Check 348／350 保持未勾。
