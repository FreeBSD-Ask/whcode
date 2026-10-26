# ADR-032：四端对等判据与差异登记（ensure-frontend-parity）

- **状态**：已批准（2026-10-24；G2 设计基线，随 ensure-frontend-parity 变更 Task 168 落档）
- **来源**：FR-604／FR-610、FR-619—625（`specs/001-whcode-harness/spec.md` 前端四端对等节）、
  ADR-009（对等按业务能力定义，本 ADR 细化其判据与差异资格）、ADR-014（极简前端）、
  ADR-019（CUI 命令面）、ADR-023（Web 安全边界）、ADR-026／ADR-027（原生 GUI 与只读 v1）、
  ADR-028（字体）；`.trae/specs/ensure-frontend-parity/spec.md`（变更总纲与对等基线表）
- **责任人**：前端对等负责人（四端接口与登记表维护）
- **退出条件**：任一复审触发（见“三重时间”将来项）即复审本 ADR、理由码表与登记表；
  不得沿用过期理由码，不得以“另一个变更”为由绕过登记

## 决策

1. **对等判据（五维）**：CUI／TUI／GUI／Web 四端（SSH 为独立传输入口，见理由码
   `TRANSPORT_SPECIFIC`）的对等只按**业务能力、权限检查、状态迁移、错误语义、可审计性**
   五维判定；视觉、交互形态、视图数量、按键与布局不作为对等判据。这是 ADR-009
   原判据的细化，不放宽。
2. **单一事实源**：`whcode-app::surface` 能力登记表是能力面的唯一定义（`SurfaceId`、
   类别、所需 `Capability`、效果类别、确认与审计要求、四端状态）；各前端只声明到
   `SurfaceId` 的映射，不得各自定义“支持什么”；`docs/matrices/feature-parity.md`
   是登记表的受校验产物，手工漂移即门禁失败。
3. **四态判定**：每个能力在每个前端有且只有四种状态（见判定表），不存在第五种
   “说不清”。已实现必须有前端锚点（函数名或路由）；已排除必须有理由码与锚点；
   预留必须有里程碑；不适用必须写明判定依据。
4. **统一不可用语义**：登记为预留或排除的能力在该端被调用时返回结构化
   `not_supported`（`retryable = false`，`details` 含 `surface_id`、`state` 与
   `milestone` 或 `reason_code`）；CUI 退出码 **55**，Web 返回 **HTTP 501**（同码
   JSON）；未知路由仍为 404。禁止把“已登记但该端不提供”伪装成 404 或静默成功。
5. **对等门禁**：登记表 × 四端清单 × 矩阵三向一致的契约测试进 `cargo test` 与 CI，
   并含可失败的负向用例（缺映射、孤儿路由／动作、矩阵篡改、过期排除）；同一写命令经
   多端执行的行为等价（最终状态、审计、权限拒绝码、错误码）由共享夹具证明。

## 四态判定表

| 状态 | 判定 | 必备登记项 | 门禁行为 |
|---|---|---|---|
| 已实现（Implemented） | 该端存在到登记能力的可用映射 | 前端锚点（函数名或路由） | 行为等价夹具覆盖（写命令） |
| 已排除（Excluded） | 该端按本 ADR 登记的理由不提供该能力 | 理由码 + FR／ADR 锚点 | 调用得 `not_supported`；理由码过期即失败 |
| 预留（Reserved） | 功能未交付，等待归属里程碑 | 对应 FR 与里程碑 | 调用得 `not_supported`（`details.milestone`） |
| 不适用（NotApplicable） | 能力不属于该端的形态范围 | 判定依据（如 `TRANSPORT_SPECIFIC`） | 不参与对等计数，但登记必须存在 |

## 理由码全表（差异只允许这些理由）

| 理由码 | 适用范围 | 锚点 | 理由与承担者 | 复审触发 |
|---|---|---|---|---|
| `GUI_READ_ONLY_V1` | GUI 全部写命令 | FR-711／ADR-026／ADR-027 | v1 先固定资源与可验证性；写能力缺口由 CUI／TUI／Web 承担 | GUI 写需求立项或 FR-711 修订 |
| `CLI_LOCAL_ADMIN` | db 维护、font、serve、doctor、bench | FR-507—510、FR-717、ADR-019、ADR-028 | 文件路径、本机注册与传输入口属本地管理；远程暴露扩大攻击面 | Web 远程管理需求 + 威胁模型与认证方案 |
| `CUI_COMMAND_SURFACE` | 六视图与 diff 浏览 | FR-602、ADR-019 | CUI 是稳定命令与 JSON 输出面，不是屏幕视图 | CUI 需要结构化视图命令时 |
| `VIEW_SUMMARY_ONLY` | task show 五类详情 | FR-103／104、ADR-009 | 视图面共享“查询任务”能力，详情深度是交互差异；CUI 保留深查 | 视图面需要完整详情时 |
| `COMMAND_OUTPUT_ONLY` | events export、需要文件路径的命令 | FR-205、FR-601／711 | 文件输出需要路径选择与覆盖确认，属命令面操作 | TUI／GUI 需要文件导出时 |
| `CUI_NO_THEME` | 四套主题 | FR-612、ADR-014 | 终端配色由终端模拟器控制，程序不得声称强制改色 | CUI 引入受控配色输出时 |
| `SIX_VIEW_SCOPE` | runtime 清单 | FR-601／711 | TUI／GUI 固定六视图；运行时可经 CUI／Web 管理 | 运行时视图立项 |
| `CUI_REQUEST_ONLY` | 其他端更改自动可见 | FR-602 | CUI 为一次性请求响应，不存在在线订阅 | CUI 引入长驻会话 |
| `GUI_MANUAL_REFRESH` | 自动刷新 | FR-711／714 | GUI v1 以显式刷新为准，禁止定时器驱动重绘 | GUI 事件订阅立项 |
| `RESERVED_MILESTONE` | 预留命令 | 对应 FR 与里程碑 | 功能未交付，显式失败，不得伪造 | 里程碑交付时同 PR 更新登记表与矩阵 |
| `WEB_SETTINGS_ONLY` | 证书设置读写（`settings.cert.read`／`settings.cert.write`） | FR-627／ADR-040 | 证书编辑必须在 Web 服务设置页完成：页面要呈现重启提示、当前信任来源与证书指纹，而这些状态只存在于运行中的 Web 服务；CUI／TUI／GUI 无服务设置面，若各自实现会分裂重启语义与审计口径。承担者：需要轮换证书的运维用户、四端登记维护者 | 需要 headless 自动化证书轮换（CUI／脚本驱动）时，评估新增命令面并重审本码 |
| `TRANSPORT_SPECIFIC` | SSH／serve 等传输入口 | FR-603／607／608 | 独立传输面，不属于四端视图或命令对等范围 | 不适用 |

表内理由码沿用本 ADR 的大写下划线写法；登记表与代码中的稳定字符串是对应的小写蛇形码
（`crates/whcode-app/src/surface.rs` 的 `ExclusionReason::as_str`，例如
`GUI_READ_ONLY_V1` ↔ `gui_read_only_v1`）。2026-10-25 新增行同此口径：
`WEB_SETTINGS_ONLY` ↔ `web_settings_only`，表格文字与实现字符串一一对应，
门禁按小写串比对；同一理由不得出现两种拼写。

## 理由（正负和解）

- **正面**：能力漂移从“评审记忆”变成可证伪事实——故意删除一个前端映射、篡改矩阵
  一行，门禁即失败；既有差异（GUI 只读、CUI 命令面、本地管理命令、预留命令）从
  隐性假设变成带锚点与复审触发的显式登记；新命令漏登记在测试期被拦截，而不是在
  用户端表现为三种互不相同的“未交付”。
- **负面**：每个新命令承担“登记表 ×4 端状态 + 矩阵”的登记税；矩阵由受校验产物
  取代自由编辑；写命令门面迁移触及已验收行为（风险 R-15）。
- **承担者**：四端维护者与新增功能作者（登记税）；依赖 CUI 退出码的脚本作者
  （50 → 55 迁移）；矩阵手工编辑者（失去自由编辑）。
- **和解理由**：登记税由门禁自动化兜底（漏登记即测试失败，不靠记忆）；“说不清”
  的状态是矩阵自相矛盾与 Check 93／183 长期无法判定这类事故的直接根因；显式差异
  比隐性假设更符合退出权与不伪造原则（NFR-218／NFR-220）。

## 三重时间

- **过去（来源）**：FR-604（三端命令校验、权限、门禁与业务结果一致）、FR-610（共用
  能力矩阵、新能力同 PR 定义）与 ADR-009（对等按业务能力定义）早已把对等定为要求；
  但矩阵为手工维护且已发生可复现的自相矛盾——同一文件头部称 Web 写命令已接入
  `POST /api/sessions/delete`，表格行仍把 Web 标为 ⛔；同一“未交付”在 CUI
  （`invalid_state`）、Web（纯文本 404）与 TUI／GUI（无入口）语义互不相同。人工承诺
  已被证伪为不可执行。
- **现在（证据）**：登记表与契约门禁设计见 `.trae/specs/ensure-frontend-parity/spec.md`
  与 Task 169—171；本 ADR 为其 G2 决策记录；现状缺口（CUI 无 `context` 命令、
  Web 缺上下文／diff／导出端点、TUI 无写操作与运行时主题切换）在变更 spec 中逐项
  登记，并给出对等基线表。
- **将来（复审触发）**：① 新前端立项（登记表新增 `FrontendId`）；② 新命令类出现
  （新增效果类别或确认语义）；③ `add-real-llm-provider`（ADR-031／D-16）合入时重新
  登记 `llm run／cancel`；④ GUI 写需求立项或 FR-711 修订；⑤ CUI 引入长驻会话或
  受控配色输出；⑥ Web 远程管理需求（须附威胁模型与认证方案）；⑦ 登记表与清单漂移
  却门禁通过（负向自测失效，立即复审门禁本身）。

## 替代方案（已否决）

- **维持自由编辑矩阵 + 评审把关**：已证伪——同一文件头部与表格行的矛盾存续数周
  未被任何评审发现。
- **只做正向一致性测试**：无法证明门禁本身有效；缺映射的漏报正是本次要消除的
  漂移通道，故负向用例（缺映射、篡改、过期排除）是门禁的必要组成。
- **以“功能清单截图”作对等验收**：不可程序化验证，且无法发现权限与错误语义分叉
  （ADR-009 已否决，此处为门禁层面的重申）。

## 影响

- 需求：`specs/001-whcode-harness/spec.md` 前端四端对等节（预留编号 619—625）、
  US6／US7 场景、V-112、SC-022；任务与验收挂钩及正式编号登记由收口任务统一更新。
- 代码：`whcode-domain`（`NotSupported`）、`whcode-app`（`surface`／`commands`）、
  四端清单映射与 `tests/contract/test_frontend_parity.rs`；GUI 不新增写能力，
  只登记只读边界。
- 文档：`docs/matrices/feature-parity.md`（GUI 列与逐行重写）、ADR-009（修订记录）、
  `contracts/app-service.md`／`cui-cli.md`／`event-schema.md`。
- 风险：R-14（登记维护税）、R-15（门面迁移回归面）挂 `docs/matrices/decisions.md`。

## 回退路径

- 登记表与门禁：纯增量，整体 revert 即恢复自由编辑矩阵（须同时撤销矩阵校验测试）。
- `not_supported` 语义：CUI 退出码回退＝恢复 `invalid_state`（50）旧映射，并在
  CHANGELOG 撤销 BREAKING 注记；Web 501 守卫摘除后未交付路径回到 404。
- Web 新端点与四主题：独立路由与 CSS，可单独摘除（前端预算门禁随之回退）。
- TUI 写操作：执行器未注入时动作显式不可用；回退＝不注入执行器即恢复只读形态。

## 与 ADR-031／D-16 的编号与协调

- D-16／ADR-031 属 `add-real-llm-provider`（LLM 真实供应商接入）；该 ADR 已于 2026-10-25
  补录（`ADR-031-llm-provider-network-and-credentials.md`），客户端与 `llm run` 装配仍未交付；
  本 ADR 占用 **D-17／ADR-032**，编号相邻、语义不冲突。
- `llm run／cancel` 的四端登记状态以 `add-real-llm-provider` 实际交付为准：该变更
  合入前登记为预留（`RESERVED_MILESTONE`）；合入时必须同 PR 更新登记表、矩阵与
  对等门禁。两个变更不得对 `llm.run` 状态各自表述。

## 修订记录

### 2026-10-24（T181 收口）

- **账本转正**：FR-619—625 由预留编号转为正式编号并计入 FR 计数；任务 T168—T181 与
  Check 264—271 在主账本挂钩（`tasks.md`／`checklist.md`），`check_counts.py` 与
  `traceability.py` 全绿。T162—T167 为并行变更 `add-real-llm-provider` 的占位任务
  （未勾选，其 spec 与 ADR-031 尚未入库），编号连续性由其承载。
- **登记表翻转**：本变更交付的 13 个前端格（`T173`×2、`T174`×1、`T175`×1、`T177`×7、
  `T178`×2）由 `Reserved` 改为 `Implemented` 并补真实锚点；对应测试由“断言预留”改为
  “断言已实现”，防止绿灯下的事实漂移。
- **矩阵受校验**：`docs/matrices/feature-parity.md` 由 14 行手工“域／命令行”快照重写为
  54 行逐条表（列：`SurfaceId`／类别／效果／四端符号／依据与理由）；门禁逐行比对状态、
  类别、效果、理由码与里程碑，并新增篡改符号／理由码的负向用例。矩阵不再保留手工状态列。
- **文档回填**：`tui.md` §2／§3／§4／§5／§8、`ui-design.md` §3／§3.1、
  `web-deployment-security.md` §4.1、`cui-manual.md` §1／§5／§9 由“交付后生效”改为现状；
  CUI 手册补退出码 55 与 `context budget`／`runtime list`。
- **新发现的既有缺口（不在本收口内擅改）**：`session.archive` 登记为
  `PreviewConfirm` + `audited = true`（契约 §2-1 破坏族），但四端均无确认步骤，store
  注明“可逆元数据标记，不写 `db_purge` 审计”。该冲突已在 `CHANGELOG.md` 已知未完成
  第 3 项登记（责任人：对等门禁维护者；退出条件：独立 ADR 裁决确认语义与审计口径，
  四端同 PR 对齐）。本 ADR 的 `audited` 字段仍是**契约要求**，不因现状请假而改写。
  **关闭（2026-10-25，ADR-038／T182）**：裁决为不放宽本 ADR 与契约，以实现补齐——
  四端统一预览→确认，`set_archived` 同事务写 `session_archive` 审计；登记表字段与实现
  自此一致（见本节末的 2026-10-25 修订记录）。
- **“未排期”口径**：登记表中 22 个 `Reserved { milestone: "未排期" }` 是变更前既有口径
  （应用层已交付、四端接线未排期），本收口不改变其业务语义；其里程碑命名是否收敛为
  具体 FR／里程碑，留待相应能力立项时处理（复审触发①）。

### 2026-10-25（`add-real-llm-provider` 合入后的 `llm.run` 口径）

- **事实**：`add-real-llm-provider`（3f9564c）已在 `whcode-llm` 交付可选 HTTP Provider
  （ureq/rustls，仅在装配根启用），但 **CUI／TUI／Web 均未接线**——`cui_dispatch` 与
  `/api/llm/run` 仍走 `not_supported`。因此 `llm.run`／`llm.cancel` 的四端登记保持
  `Reserved { milestone: "M3-llm" }`（GUI 仍 `GuiReadOnlyV1`），矩阵与门禁断言不变。
- **为何不按“合入即翻转”执行**：本 ADR 的触发条件是**前端可调用**，不是 crate 内有实现。
  提前翻转会让登记表宣称一个调用仍返回 55／501 的能力，属绿灯下的事实漂移。
- **退出条件**：任一前端接线 `llm run`／`llm cancel` 的同 PR，翻转对应格为 `Implemented`
  并补真实锚点、更新矩阵行与 `llm_run_stays_reserved_while_no_frontend_wires_it` 断言；
  在此之前不得把 `M3-llm` 改写为已完成里程碑。

### 2026-10-25（`session.archive` 确认与审计关闭）

- **关闭路径**：ADR-038 裁决不放宽本 ADR 的登记字段与契约 §2-1，改为实现补齐——
  CUI 默认 dry-run／`--yes`、TUI `A` 预览→确认、Web 缺 `confirm` 只预览，store 在同一事务
  写 `session_archive` 审计；`session.archive` 的 `PreviewConfirm` 与 `audited = true`
  自此与实现一致（T182／Check 272）。
- **影响**：CUI 归档属 BREAKING 行为变更；TUI 尚无反归档键，作为已知不对称留在 ADR-038，
  不影响本 ADR 的四端状态判定（三端均为 `Implemented`，GUI 仍 `GuiReadOnlyV1`）。

### 2026-10-26（T167 收口：`llm.run`／`llm.cancel` 的 TUI 接线与三端齐备）

- **事实**：TUI 新增 `L`（llm.run：剖面名→提示词两步输入，会话取当前选中会话）与
  `C`（llm.cancel：请求 ID 输入）键位，经新增 `llm_provider::TuiLlmExecutor`（装配根
  `llm_provider.rs`，复用 `ServeLlm` 装配）真实编排——与 CUI/Web 同一 `whcode_app::llm`
  入口、同一权限（`Capability::SessionCreate`）与会话校验；连接与 TUI 写命令执行器
  共享同一条工作区连接（Check 98 口径）。登记表 TUI 格由 `Reserved { M3-llm }` 翻转为
  `Implemented` 并补前端锚点；矩阵 feature-parity.md 两行同步（本批同 PR）。
- **同步语义**：三端均为同步等待完成（CUI `llm run`、Web `POST /api/llm/run`、TUI
  键位均阻塞到调用结束）——与既有三端形态一致，不引入异步包装。
- **未翻转**：GUI 仍按 `GUI_READ_ONLY_V1` 排除（写能力由 CUI/TUI/Web 承担，属登记的
  有意差异而非未交付）。
- **退出条件已满足**：上一节“任一前端接线即翻转对应格”在本批完成——TUI 键位测试
  2 项（空数据源显式提示“未选择会话”不得调用执行器、取消请求到达执行器），
  `cargo test -p whcode-tui` 69 项通过；`llm_run_stays_reserved_while_no_frontend_wires_it`
  口径自此关闭。
- **复审触发**：真实 Provider 端到端证据（T192 之外的浏览器/终端实测）取得时复核
  事件载荷与取消语义；GUI 写需求立项时重审本码。

### 2026-10-25（T167：`llm.run`／`llm.cancel` 的 CUI／Web 接线与登记翻转）

- **事实**：`whcode-app::llm` 已成为 `llm run`／`llm cancel` 的唯一编排入口（权限用既有
  `Capability::SessionCreate`、会话存在性、剖面注册表、凭据求值、有界流式事件与取消标记）；
  CUI（`cui_dispatch::llm_command`）与 Web（`POST /api/llm/run`、`POST /api/llm/cancel`，
  `handle_request_with_llm`）已真实接线，登记表对应两格由 `Reserved { M3-llm }` 翻转为
  `Implemented` 并补前端锚点，矩阵与对等门禁同 PR 同步。
- **未翻转**：TUI 仍为 `Reserved { M3-llm }`（本轮未接线）；GUI 仍按 `GUI_READ_ONLY_V1`
  排除。两者被调用时仍返回结构化 `not_supported`（CUI 55／Web 501 的语义不变，GUI 无入口）。
- **传输状态**：`whcode-llm` 的阻塞式 HTTP 客户端（T163）尚未交付，装配根工厂返回结构化
  `invalid_state` 并注明待接线点；因此本轮的 `Implemented` 指**前端映射与应用层编排可用**，
  不等于端到端模型调用可用。shell 必须在报告与 spec 中保留该缺口，不得改写为“已交付”。
- **复审触发**：T163 客户端交付后，需复核 `llm.run` 的事件载荷、超时与取消语义并补端到端
  证据（ADR-031 决策 8 的重试仍不适用于流式生成）；TUI 接线时翻转其预留格。

### 2026-10-25（视觉语言分层与 `CUI_NO_THEME` 口径）

- **事实**：ADR-041 把四端分为终端族（CUI／TUI）与应用族（Web／GUI）两套视觉语法；CUI 在
  交互终端下允许**非颜色**的 ANSI 强调（粗体／弱化），并在管道／`NO_COLOR`／`TERM=dumb`／
  `--json` 下退化为纯 ASCII。
- **对本 ADR 的影响**：无。理由码 `CUI_NO_THEME` 的语义是“终端配色由终端模拟器控制、CUI 不
  提供主题选择”，本 ADR 不因此放宽或收紧；粗体与弱化属排版属性，不是配色，也不是新能力。
  `GUI_READ_ONLY_V1` 同样不变：GUI 新增的侧栏点击仅切换视图，仍为 `EffectClass::Read`。
- **对等判据**：视觉、布局、交互形态（第 1 条）继续不参与对等判定；本轮改动不新增业务能力，
  故登记表与 `docs/matrices/feature-parity.md` 无需改动。
- **退出条件**：CUI 若引入受控配色、长驻会话，或 GUI 引入写命令／定时器刷新，即触发本 ADR
  与 ADR-041 的联合复审。
