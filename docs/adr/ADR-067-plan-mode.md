# ADR-067：计划模式（软引导＋可重放状态＋退出人工复核）

- **状态**：已批准（2026-11-13，proposal-v3 P4.5／FR-774／T287；用户“同时进行”指令下与 P4.8 并行落地）
- **来源**：`research/synthesis/proposal-v3.md` P4.5、`research/dsh-comparison/domain-G-plan-goal-todo-schedule.md`
  §2.1—§2.2／§4.1—§4.3（`plan/mode` log-only 事件、软引导不假装强制、`exit_plan_mode` 人复核）；
  FR-774／T287／Check 382
- **责任人**：turn 循环维护者、审批面维护者、四端前端维护者
- **退出条件**：需要“计划模式真正强制工具白名单”时，本 ADR 的软引导口径必须废止（强制属于
  沙箱／审批面，另立 ADR）；需要跨会话或宿主级计划（非会话状态）时，落点改为新表而非事件

## 背景

WHCode 已有会话待办（`todo_write`，整表快照）与交付物登记（`present_deliverables`），
但“先出计划、经人认可、再动手”这一动作没有落点：模型无法声明“现在处于只讨论计划的模式”，
用户也无法在四端看到或改变这个状态。DSH 的对应物是 `plan/mode`（整值替换的 log-only 事件，
激活时注入部署自定义引导段）与 `exit_plan_mode`（提交完整计划、经人复核后退出）。两者都
**不强制**任何工具调用——强制由沙箱与审批独立执行；把“计划模式”写成强制会与 WHCode 的
门禁语义（FR-3xx 阶段门禁）混淆。

## 决策

1. **状态是事件，不是内存开关**：新增 `EventKind::PlanModeChanged`（log-only、Critical；
   `Critical` 的依据与 `TodoUpdated` 相同——状态唯一持久副本，丢弃即丢状态）。载荷
   `{active, source, tool_call_id?}`，`source` 取 `user`／`model`（`user` 来自命令与 Web 端点，
   `model` 来自工具调用并带 `tool_call_id`）。当前状态 = 本会话**最新一条**该事件（复用
   `store::query::latest_event_of_kind`），分叉与重放自动继承，不新增状态列、不改 schema。
2. **软引导，不假装强制**：turn 开始时若状态为激活，在消息装配区（技能与 MCP 告示之后、
   user 消息之前）注入一条 system 消息，内容是固定的计划引导段（常量文本，≤
   [`MAX_PLAN_GUIDANCE_CHARS`]）；引导段显式写明“本消息只是引导，不阻止任何工具调用；限制由
   审批与沙箱独立执行”。**不拦截**任何工具——不做工具白名单、不改审批规则、不阻断文件写。
3. **模型侧工具 `plan_mode`**（`{active: bool}`，必填、无未知字段）：激活直接生效；
   **退出（`active: false`）必须经人复核**——在审批面用伪工具名 `plan_mode_exit` 判定，
   交互装配（Web／GUI）把它登记为内置 `ask`，无人应答＝`unavailable`（本次调用未执行，
   不得视为已允许），拒绝＝`failed` 并回填反馈让模型修订重提。非交互装配（CUI／TUI 无应答者）
   一律 fail-closed，错误消息指明用户命令（决策 4）。
4. **用户侧通道**：CUI `whcode session plan --session S --on|--off` 与 Web
   `POST /api/plan {session, active}`；用户即决策者，**不经审批**，但仍落 `PlanModeChanged`
   （`source = user`）。读面 `whcode_app::plan::current` 与 `GET /api/plan?session=`。
5. **有界**：引导段与错误文案长度受常量约束；事件载荷只有三个短字段；读取面单条查询、
   不新增线程、不新增常驻资源（NFR-211 不受影响）。
6. **语义边界**：计划模式**不是**交付门禁、不产生 SDD 任务、不写 `tasks`、不参与验收；
   与 `spec` 阶段门禁无耦合（两者可以同时启用，互不解释对方的拒绝）。

## 正负和解

- **正面**：会话里第一次有“现在在讨论计划还是执行”的可重放大事实，四端与审计看到同一状态；
  模型可以主动进入计划模式并在退出时被迫征求人的同意；分叉／重放／恢复都不丢状态。
- **负面**：软引导对不遵守提示的模型没有约束力（这是刻意的——约束属于审批与沙箱）；
  退出路径引入一个伪工具名（`plan_mode_exit`）的审批规则，读代码的人需要知道它不对应真实工具；
  非交互前端（CUI／TUI）模型无法自行退出，必须由用户发命令。
- **承担者**：使用者（需要理解“计划模式不强制”）、turn 维护者（引导段与退出分支）、
  审批面维护者（伪工具名规则）、前端维护者（TUI／GUI 视图未排期）。
- **和解理由**：以“软引导＋显式不强制”换取与门禁语义的清晰分离；以“退出走审批”换取人
  对“开始动手”的最终控制权；以事件存储换取分叉／重放的零成本一致性。

## 三重时间

- **过去来源**：DSH `docs/subsystems/plan.md`（软引导、pending 选择、exit 协议）；
  proposal-v3 P4.5；ADR-044（turn 循环）、ADR-046（Check 证据态）、ADR-048（SessionState
  副作用范围）、ADR-064（审批接线与四态）。
- **现在证据**：`crates/whcode-domain/src/event.rs`、`crates/whcode-app/src/plan.rs`、
  `crates/whcode-tools/src/plan.rs`、`crates/whcode-app/src/turn.rs` 的引导注入与退出分支、
  Check 382 的取证段。
- **将来失效条件**：需要强制力（工具白名单、写操作阻断）→ 废止决策 2；需要宿主级或跨会话
  计划 → 事件模型不够，改新表；需要“计划审批留档成文件” → 与本 ADR 的事件口径并存需另立。

## 复审触发（绑定命令＋判定）

1. `cargo test -p whcode-app -- plan` 或 `cargo test -p whcode-tools -- plan` 不再全绿 → 语义复核；
2. `rg -n "plan_mode_exit" crates/` 在审批装配与 turn 分支中不再同时命中 → 退出复核被拆掉；
3. 引入任何“计划模式下阻断工具”的实现 → 必须先废止决策 2 并另立 ADR；
4. TUI／GUI 计划模式视图交付 → 更新 `session.plan` 的支持矩阵与本文展示面口径；
5. 2027-05-07（与 D15 复审同批）→ 复查引导段文本长度与退出审批的实际摩擦。

## 已知限制

- 计划模式不阻止任何工具调用；把它当“只读模式”使用是错误用法，文档与引导段都须明写。
- 无计划正文存储：DSH 的“提交完整计划供复核”在 WHCode v1 未实现——退出审批只审“是否退出”，
  计划内容仍在对话里（不单独持久化成字段）。
- CUI／TUI 无计划状态视图（`events list` 可见 `PlanModeChanged`）；TUI 键位未排期。
- 事件为 log-only，不进入模型上下文；模型只通过下一 turn 的引导段感知当前状态。
