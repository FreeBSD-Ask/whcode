# ADR-089：四端对等全量清零——预留面交付与运行时管理落地

**日期**：2026-11-19 | **状态**：已批准（2026-11-19 用户指令「实现多种界面的功能对等」，
范围选择为「全量清零（含 B 类）」）| **关联**：ADR-032、ADR-056、ADR-024、ADR-013、
ADR-065／067／068、FR-102／201—208／301—314／601—625／701—710／769—776

## 任务认知卡

- **方向**：把对等矩阵中全部「预留」格清零（17 面 49 格），使已交付业务能力在四端
  均有真实锚点或正当排除；「部分」排除格按理由码保留，不属缺口。
- **目标**：`tests/contract/test_frontend_parity.rs` 全绿且矩阵零「预留」行；新增面
  有正向与失败路径测试；spec.*/runtime.* 从「能力未交付」变为可执行命令。
- **用户意图**：用户要求多界面（CUI／TUI／GUI／Web；SSH 为 Web/CUI 透明传输）功能
  对等，已明确选择全量清零（含 spec 生命周期写面与 runtime 管理提前出 M8）。
- **非目标／边界**：不改既有理由码排除（`cli_local_admin`、`six_view_scope`、
  `web_settings_only`、`cui_command_surface`、`transport_specific` 等正当差异）；
  不扩 GUI 写白名单至 spec.* 门禁（ADR-056 口径不变）；runtime 安装不做网络下载
  （只校验本地 payload）；不引入任何 GNU 工具链；不动既有色值与主题语义。
- **现有证据**：对等矩阵 `docs/matrices/feature-parity.md`（2026-11-15 版，49 格预留）；
  app 层已交付 `session::fork_session`／`switch_session`、`rewind`、`preimage::restore`、
  `plan::set`、`deliverables::list`、`coherence::latest`、`llm::model_catalogue`、
  `gates::request_transition`／`force_advance`（T074/T078）；whcode-speckit 已有
  `parse_artifact`／`analyze`／`converge`／`default_workflow`；Web 已预登记守卫路由
  （`/api/spec/*`、`/api/runtimes/*`、`/api/events/show`，501 not_supported）；
  runtime 装配策略 `whcode-bin::runtimes::enable_policy`（四项默认关闭，FR-704）。
- **手段**（后置）：store v17 `runtime_state` 表；app 新增 `runtime_mgmt.rs` 与
  `spec.rs`；FrontendCommand 扩展四个写面；TUI/GUI 新增前像与充分性两视图（`9`/`0`）。

## 问题

对等矩阵（FR-619—621、ADR-032）尚有 49 格「预留」、17 个面未清零，分两类：

1. **A 类（业务能力已交付、缺前端锚点）**：`session.switch`、`events.show`、
   `session.fork`（TUI/Web）、`session.rewind`（TUI/GUI）、`session.restore`
   （Web/TUI/GUI）、`session.plan`（TUI/GUI）、`view.preimages`（TUI/GUI）、
   `view.deliverables`（TUI/GUI）、`view.coherence`（TUI/GUI）、`llm.models`
   （TUI/GUI）。
2. **B 类（能力本身未交付）**：`spec.next/validate/analyze/converge/force-advance`
   （FR-301—314 写面，app 层门禁函数已交付但工作区产物上下文装载缺位）；
   `runtime.status/enable/disable/install`（FR-701—710，登记 M8，无持久化启用状态）。

用户指令要求全量清零：两类都交付。

## 决策

### A. A 类接线（app 已有，前端补锚点）

- **app 补口**：新增单事件查询 `events::show_event`（按事件 ID，脱敏口径与列表同源）；
  `FrontendCommand` 扩展 `SessionFork`／`SessionRewind`／`SessionRestore`／
  `SessionPlanSet` 四个写面（TUI/GUI 写命令统一走 `commands` 门面，FR-623 单点）；
  `ViewSource` 扩展 `switch_session`／`event_detail` 读面方法（TUI/GUI 共享）。
- **ViewId 扩容**：`Preimages`（切换键 `9`）、`Coherence`（切换键 `0`），
  `VIEW_COUNT` 8→10；`view.deliverables` 的 TUI/GUI 锚点为台账视图交付物节
  （`LedgerSection::Deliverable` 已随 FR-787 交付，不新增视图）。
- **CUI**：`session switch` 子命令与 `events show` 接线（CLI 形状已在）。
- **Web**：新路由 `POST /api/sessions/fork`、`POST /api/preimages/restore`
  （预览→确认两段式）、`POST /api/sessions/switch`、`GET /api/events/show?id=`；
  同时修复 surface_map 孤儿（`/api/plan`、`/api/rewind`、`/api/preimages`、
  `/api/deliverables`、`/api/ledger` 补登记）。
- **TUI**：`F` 分叉、`W` 回退、`V` 前像恢复（预览→确认）、计划模式切换、`s` 切换
  会话、`M` 模型目录、`e` 事件详情（键位以实现时的键位表校验为准，避开既有证据键）。
- **GUI**：同套动作经既有 OpMode（输入→预览→确认）机制；前像与充分性两视图入
  视图切换（GUI 数字键经 `ViewId::ALL` 索引自动扩展）。

### B. spec.* 五命令（SDD 门禁命令面）

- app 新增 `spec.rs`：SDD 工作区产物上下文装载（`<workspace>/specs/` 下的
  constitution／spec／plan／tasks 产物经 `whcode_speckit::parse_artifact` 解析，
  解析失败显式报告不静默丢弃，FR-309）＋五命令：
  - `spec.validate`：当前阶段必需产物存在且解析干净（FR-302／303）。
  - `spec.analyze`：`whcode_speckit::analyze` 五类缺陷检出＋门禁结果（FR-306）。
  - `spec.converge`：任务验收条件 × 结果状态 → `ConvergeReport` 差异报告
    （FR-310/311，与 `overview` 的收敛口径同源）。
  - `spec.next`：`gates::request_transition`，`TransitionContext` 从解析产物显式
    提供（FR-308/313 禁止推断）；非法转换与门禁失败保持 `gate_failed` 语义。
  - `spec.force-advance`：`gates::force_advance`（`--reason` 必填，审计含完整要素，
    FR-312）；TUI/Web 端均要求理由文本，确认语义 `ReasonRequired`。
- 权限沿用 `Capability::SpecAdvance`；TUI/Web 与 CUI 同语义（FR-314 同一份状态）。
- GUI 保持 `gui_read_only_v1` 排除（ADR-056 白名单不扩至门禁写面）。

### C. runtime.* 四命令（运行时管理，M8 提前）

- store：**复用** `0001_initial.sql` 里已存在的 `runtime_state` 骨架表
  （`name`／`enabled`／`backend`／`version`／`installed_at`／`payload_path`），
  不新增迁移、不提升 schema 版本——运行时启用状态的唯一持久化点，空闲态不新增
  常驻资源（一行一运行时，四行封顶）。`whcode-store::runtime_state` 只做适配。
- app 新增 `runtime_mgmt.rs`：
  - `runtime.status`：声明＋可选探测＋持久化状态（与 `runtime.list` 同形）。
  - `runtime.enable`／`disable`：写 `runtime_state`＋审计（`AuditAction` 复用既有
    写动作）；`js`／`bsdshell` 拒绝启用（结构化错误：js 内置引擎不交付 ADR-024
    决策 1；bsdshell M7 延后 ADR-013）——不伪造「已启用」。
  - `runtime.install`：只接受**本地 payload**，先做 `fnv64` 哈希比对（声明值
    与本地文件不一致即拒绝），通过后登记版本与 payload 路径；不做网络下载
    （资源优先、安全优先：隐式联网属暗黑模式禁项）。来源白名单与平台兼容的
    执行面校验随 M8 执行链路（见已知限制 6）。
- 新能力 `Capability::RuntimeManage`（稳定名 `runtime.manage`），入
  `KNOWN_CAPABILITIES` 与 `actor_with_all_capabilities`；CUI/Web 装配根 actor
  即全能力调用者，行为不变。
- 接线范围：CUI 与 Web（`/api/runtimes/status|enable|disable|install`，守卫路由
  已预登记）；TUI/GUI 保持 `six_view_scope` 排除（与 `db.*` 本地管理族同口径）。

## 理由与约束

- 「预留」是对等矩阵中唯一代表**缺口**的状态（ADR-032 四态定义）；清零后矩阵只剩
  已实现与正当排除，对等主张首次完全可判定（FR-621 门禁五维）。
- runtime 启用状态落 store 而非配置文件：本仓无运行时可写配置文件通道，store 是
  唯一持久化点（单文件 DuckDB）；表规模有界（运行时清单固定四项）。
- `runtime.install` 不做下载：payload 独立分发（FR-703）属交付物职责，harness 只
  做校验与登记；联网拉取引入供应链面，收益不抵代价（正负和解见下）。
- spec.* 的 GUI 排除与 runtime.* 的 TUI/GUI 排除均为**正当差异**（登记理由码），
  对等判据是业务能力可达或正当排除，不是四端像素对齐（ADR-009/032）。

## 已知限制与失效条件（复审触发，绑定命令＋判定）

1. **工作流配置取内置版本**：`spec.rs` v1 用 `whcode_speckit::default_workflow()`，
   不读取工作区自定义 workflow 配置文件（FR-302 的「版本化配置」以内置版本承载）。
   失效条件：`cargo test -p whcode-app spec_` 出现需要自定义工作流的用例，或用户
   提出自定义阶段定义需求 → 补配置文件装载与 ADR 修订。
2. **runtime.install 不联网**：只校验本地 payload。失效条件：出现需要远端分发的
   用户场景 → 新 ADR（供应链威胁模型先行，ADR-070 同口径）。
3. **`python`／`gitbash` enable 后可用性仍取决于本机解释器/payload**：enable 只写
   状态不创造能力；执行路径继续按探测结果返回 `runtime_unavailable`。
   判定命令：`cargo run -p whcode-bin --release -- runtime status --name python`。
4. **TUI `view.deliverables` 复用台账视图**：交付物节随 FR-787 交付，若后续台账
   结构变化导致交付物节消失，该锚点失效——判定命令
   `cargo test -p whcode-tui ledger`，红即复审。
5. **键位可用性**：新增键位避开既有证据键（q/1—8/j/k/r/m/d/A/R/X/p/t/L/C/i/P/
   Esc/a/b）；实现批次内以 TUI 键位表测试钉死，冲突即改键并在矩阵锚点同步。
6. **runtime.install 只做哈希比对**：来源白名单（`SourceAllowlist`）与平台兼容
   校验未在 install 路径执行，留给 M8 执行链路。失效条件：`cargo test -p
   whcode-app runtime` 出现要求来源/平台校验的用例，或用户提出离线载荷信任需求
   → 补校验与该维度测试。

## 替代方案

- **只做 A 类**（被否）：用户明确选择全量清零；B 类才是账本上真正的功能空白。
- **runtime 启用状态放配置文件**（被否）：本仓无该通道；为此新建配置文件机制
  超出本变更范围且引入第二持久化点。
- **GUI 全量接 spec.***（被否）：ADR-056 白名单口径刚经 GUI 交互化翻转固定，
  门禁写面入 GUI 需先做确认界面设计，本批不扩。
- **runtime.enable 直接探测即启用**（被否）：启用是持久决策，必须落库＋审计，
  否则重启后状态漂移（0 信任：状态可查询、可撤销）。

## 影响

- store：无新增迁移（`runtime_state` 复用既有骨架表，schema 仍为 v16，
  `TABLE_COUNT` 不变）。
- `Capability` 新增 `RuntimeManage`；`KNOWN_CAPABILITIES` 16→17。
- `ViewId` 8→10；TUI/GUI 视图切换键扩至 `9`/`0`；对等矩阵 17 行翻转＋再生。
- FrontendCommand 新增 4 变体；ViewSource 新增读面方法；Web 新增 4 路由。
- i18n：新增用户可见文案必须十三套目录同批补齐（`all_catalogs_define_same_keys`）。

## 正负和解

- **正面**：对等矩阵首次零预留；SDD 门禁命令面（FR-307—312）从「库内函数」变为
  「四端可执行」；运行时启用从「不可见配置」变为「可查询、可审计、可撤销」。
- **负面**：schema 迁移与能力表扩张（承担者：后续迁移维护者）；TUI/GUI 新视图的
  渲染与测试面（承担者：前端 crate）；runtime 管理命令面进 Web 的攻击面增量
  （承担者：安全边界——回环/局域网认证边界不变，写面统一审计）。
- **和解理由**：缺口是显式登记的债务，清零成本一次性且可测试；运行时管理的
  「不可见性」代价（用户无法知道某运行时为何不可用）当前已由 `enable_policy`
  文本部分补偿，但不可撤销、不可审计，落库后两缺一并消除。

## 三重时间

- **过去**：ADR-032（对等四态与理由码）、ADR-056（GUI 交互化翻转）、ADR-024
  （运行时能力等级与默认关闭）、ADR-013（bsdshell 延后）、T074/T078（门禁函数）、
  T282/T285—T289（fork/deliverables/plan/rewind/restore 交付批次）。
- **现在**：2026-11-19 矩阵 49 格预留实测；Web 守卫路由预登记；`enable_policy`
  四项默认关闭实测。
- **将来**：spec 自定义工作流、runtime 远端分发、GUI 门禁写面三个方向都会使本
  决策失效——失效条件与判定命令已写入上文；M8 里程碑相应条目在 tasks.md 收口时
  标注提前交付。

## 关联

- FR：102、201—208、301—314、601—625、701—710、769—776、787、795
- ADR：009、013、024、032、056、065、067、068、070、085
- 任务：T390—T396（specs/001-whcode-harness/tasks.md 追加）

## 修订记录

- **2026-11-21（T395/T396 收口，以磁盘事实修正）**：
  1. `runtime_state` 由“新表 v17”改为**复用既有骨架表**——`0001_initial.sql` 已有
     同名表（列集合不同），新建 0017 迁移会与之冲突；schema 维持 v16，
     `TABLE_COUNT` 不变。
  2. `runtime.install` 的四重校验收窄为「本地 payload 哈希比对＋登记」；来源
     白名单与平台兼容校验登记为已知限制 6，不保留未经实现的声明。
  3. TUI 的 `runtime.*` 由历史 `Reserved { M8 }` 对齐为本节已声明的
     `six_view_scope` 排除——「预留」是矩阵中唯一代表缺口的状态，ADR 决策与登记
     表必须一致（`cargo test -p whcode-app surface` 全表零预留断言钉死）。
