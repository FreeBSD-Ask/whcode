# ADR-056：四端功能对等——GUI 交互化（只读 v1 → 对话与写命令）

- **状态**：批准（2026-11-03，用户指令「所有界面功能对等。立项并实现」）
- **来源**：用户指令（2026-11-03）；FR-711（v1 只读口径的修订）、FR-623（`GUI_READ_ONLY_V1` 登记）、ADR-032（四端对等登记表 GUI 行）；任务书 §2
- **责任人**：GUI 前端负责人（实现）；四端登记维护者（ADR-032 矩阵）
- **退出条件**：GUI 具备与 TUI 同语义的 ①LLM 对话（剖面选择 → prompt → `run_turn` → 摘要；cancel）②会话写命令（new／rename／archive／delete，破坏性命令预览＋确认两段式）③任务操作（retry／abort）；ADR-032 登记表 GUI 行翻转并在 CI 取证
- **未决条件**：无（Phase 1 本 ADR 内交付；Phase 2 见「后续」）

## 问题

GUI v1（FR-711）是只读六视图浏览器：能看、能选、能刷新，不能对话、不能写。用户实测后
指出「所有界面功能对等」——四端（CUI／TUI／GUI／Web）中 GUI 是唯一缺口：

- TUI 已有三端同语义的 LLM 与写命令 UX（`llm` 两步输入＋cancel、`CommandExecutor`
  全部写命令），装配根 `TuiLlmExecutor`／`TuiCommandExecutor` 早已就绪（ADR-032 修订
  记录里「TUI 仍 Reserved」的表述已过时，本轮更正）；
- GUI 的 `RunOptions` 不接收执行器、无对话视图、无写操作入口。

## 决策

1. **GUI 交互化，语义与 TUI 逐条对齐**（同一 `CommandExecutor`／同一 `run_turn`／同一
   预览＋确认两段式），不另造语义：
   - LLM：新增第 7 视图「对话」（`ViewId::Chat`）——剖面名输入 → 提示词输入 → 同步
     `run_turn`（与 TUI／CUI／Web 的 `llm run` 同为同步等待完成）→ 摘要呈现（不带正文，
     事件与日志不复制模型输出）；cancel 沿用请求 ID 语义；
   - 写面：会话 new／rename／archive／delete（archive／delete 必须预览 → 确认两段式，
     对齐 FR-623「破坏性命令不得无预览、无确认直接执行」）；任务 retry／abort；
   - 执行器由装配根注入（`RunOptions` 增 `command_executor`／`llm_executor`），`None`
     时显式提示 `not_supported`（不静默无响应，与 TUI 同口径）。
2. **六视图 → 七视图**：`VIEW_COUNT` 6→7，`ViewId::Chat` 追加为第 7 视图（TUI 同步获得
   该视图——呈现其既有 LLM 状态与输入提示，不新增 TUI 语义）；`SIX_VIEW_SCOPE` 登记码
   修订为「TUI／GUI 固定视图集以登记表为准（现为七视图，含对话）」。
3. **同步执行的取舍（Phase 1）**：`run_turn` 在 UI 线程同步执行，turn 期间界面阻塞——
   与 TUI 完全同语义（TUI 文档：「同步等待完成——与 CUI／Web 的 llm run 一致」）。
   不在本轮引入后台线程：连接为 `Rc<RefCell<Connection>>`（非 `Send`），且 DuckDB 2.0
   拒绝同进程对同一文件的二次打开（`artifacts/memory-limit-experiment.json` 实测），
   后台执行必须先把装配改成 `Arc<Mutex<Connection>>`——列为 Phase 2，不在本轮混入。
4. **登记翻转**：ADR-032 GUI 行 `GuiReadOnlyV1` → `Implemented`（LLM 与写面）；
   FR-623 的 `GUI_READ_ONLY_V1` → `GUI_INTERACTIVE`；FR-711 的「v1 为只读」修订为
   「只读浏览（v1.0）→ 对话与写命令（v1.1，ADR-056）」，破坏性命令仍必须预览＋确认。

## 理由与约束

- **为什么对齐 TUI 而不是 Web**：TUI 与 GUI 共用装配根与执行器（`FrontendParts`），
  语义与实现均已在位；Web 的 LLM 是 HTTP 请求-响应（异步服务器循环），GUI 照搬会引入
  第二套语义。对等判据是「同能力、同门禁、同审计」，不是「同传输」。
- **资源纪律不放松**：本轮不新增常驻线程、不加周期重绘；对话视图的渲染走既有脏标记
  路径；ADR-027 预算（空闲 RSS ≤96 MB／线程 ≤24／句柄 ≤256／CPU ≤0.2%）在 CI 探针复核。
- **审计与权限不绕行**：写命令全部经 `whcode-app::commands::execute_command`
  （Capability 判定＋审计字段），GUI 不新增任何绕过路径。

## 替代方案

- 「保持 GUI 只读，需要交互用 TUI/Web」——被用户指令否决（对等是明确要求）。
- 「GUI 直接调 `whcode-llm`」——绕过 `run_turn` 的事件化与工具循环，破坏 ADR-044/048
  语义，否决。
- 「后台线程执行 turn」——见决策 3，Phase 2。

## 影响

- `whcode-app::view_source`：`ViewId::Chat`、`VIEW_COUNT = 7`（TUI/GUI 共享枚举，两端的
  穷尽 match 同步更新）。
- `whcode-gui`：`RunOptions`、`State`、`Message`、对话视图、会话/任务操作与确认流、
  `surface_map` 登记扩充。
- `whcode-tui`：第 7 视图的标题键与最小呈现（其 LLM 能力已存在，不新增语义）。
- `whcode-bin`：`gui_command` 传入两个执行器（装配根已构建，纯接线）。
- ADR-032：GUI 行翻转＋`SIX_VIEW_SCOPE` 修订；FR-623/711 注记。
- i18n：`gui.chat.*`、`gui.view.chat`、`gui.session.*` 词条（zh-CN/en 同步）。

## 正负和解

- **正面**：四端对等缺口清零；GUI 从「只能看」变为可用工作面；全部写路径仍走唯一门面
  （审计/权限零绕行）。
- **负面**：turn 期间 GUI 界面阻塞（与 TUI 同，但 GUI 用户对冻结更敏感）；第 7 视图
  改变了 TUI 用户的既有键位习惯（`7` 新增，`1`—`6` 不变）；登记表与文档多处同步成本。
  承担者＝GUI 用户（阻塞期等待）与四端登记维护者（矩阵同步）。
- **和解理由**：用户明确指令对等；阻塞语义与 TUI 一致已是既定和解（先例在先），
  Phase 2 的后台化是体验优化而非语义变更，可独立推进。

## 三重时间

- **过去**：FR-711 v1 只读（M0 平台可行性口径）、ADR-032 的四端登记、T167 的三端接线。
- **现在**：TUI 执行器已在装配根就绪（本轮侦察证实），GUI 缺口为纯前端工作。
- **将来失效**：GUI 若引入后台执行（Phase 2）须修订决策 3；`run_turn` 语义变化
  （ADR-044/048/052）时两端同步跟进；`MAX_PAYLOAD_BYTES`／预算变化不影响本 ADR。

## 复审触发（绑定命令＋判定）

- `cargo test --locked -p whcode-gui`：对话/确认流单测失败即复审；
- `python .github/scripts/gui-probe.py --exe target/release/whcode.exe --out artifacts/gui-run-<date>`：
  `theme_cycle.ok` 或资源预算失守即复审（对话视图不得破坏 ADR-027 预算）；
- ADR-032 登记矩阵断言（`four-end parity` 契约测试）失败即复审。

## 后续（Phase 2，另行立项）

- 后台执行 turn（`Arc<Mutex<Connection>>` 装配改造）＋对话流式增量呈现；
- GUI 事件视图的游标推进（FR-624 的 GUI 侧从手动刷新到游标可见）；
- TUI 对话视图的输入整合（当前为全局输入模式）。
