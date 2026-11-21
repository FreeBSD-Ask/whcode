# ADR-091：GUI 读面排除理由码收敛与交互补齐

- **状态**：已批准（2026-11-21，用户指令“全部批准，立刻开发”；书面判定 `artifacts/gui-web-parity-assessment-2026-11-21.md`）
- **编号认领**：ADR-091（`ls docs/adr/` 复核，090 之后无占用）；FR-799；T400—T403；Check 484—485
- **关联**：ADR-032、ADR-056、ADR-089、FR-619—621、FR-711／714、FR-799

## 任务认知卡

- **方向**：把 4 个被 `gui_read_only_v1` 误排除的 Read 面（events.show、spec.validate、spec.analyze、spec.converge）在 GUI 端收敛为已实现，并补齐所需交互。
- **目标**：登记表与矩阵中这 4 面 GUI 列由“部分”转“已实现”，GUI 具备可核验的事件详情与 Spec 门禁摘要交互；四端对等契约测试全绿。
- **用户意图**：用户要求收敛应用族（Web/GUI）之间的功能差异；本 ADR 只覆盖其中 4 个无硬冲突的读面，其余 12 面维持登记状态并写明理由。
- **非目标／边界**：不动 9 面硬约束与 3 面需新增能力；不放宽 ADR-026／040／089 与 FR-711／714；不引入 GUI 定时器（FR-714）；不扩大 GUI 写白名单。
- **现有证据**：判定报告 16 面逐项表；app 层读接口与装配根实现已在库；TUI 同语义键位已在库；GUI 的 Spec 视图 0 行、无事件选中态。
- **手段（后置）**：扩展 GUI `State`（选中索引 + 详情文本 + 门禁摘要，均有界）→ 新增 Message 与键位 → 复用 ViewSource 读接口 → 翻登记表与矩阵 → 十三套 i18n 与 PO 同步 → 契约测试与全链门禁。

## 问题

ADR-032 理由码全表把 `GUI_READ_ONLY_V1` 定义为“GUI 全部写命令”。但登记表中有 4 个 `EffectClass::Read` 的面引用该码排除 GUI：events.show、spec.validate、spec.analyze、spec.converge。其中 validate／analyze／converge 是只读门禁计算（ADR-089 已注明 converge 为只读计算），events.show 是脱敏单事件查询。四者的业务能力在 app 层与装配根均已交付，TUI 已有同语义键位，仅 GUI 缺锚点。

这是登记一致性问题，不是能力缺口；但收敛需要新建 GUI 交互，故须设计先行。

## 决策

1. **理由码适用范围修订**：ADR-032 明确 `gui_read_only_v1` 仅适用于 `EffectClass::Write` 与 `Destructive` 面；现有 4 个 Read 面改用真实锚点翻转为 `Implemented`。保留该码在码表中，继续覆盖 `spec.next`／`spec.force-advance`／`settings.cert.write`。
2. **GUI 交互**：
   - `ViewId::Events` 增加选中索引与详情区；选中事件后按 `e` 经 `ViewSource::event_detail` 取摘要，显示于有界详情区（上限 200 字符，与 TUI 同口径）。
   - `ViewId::Spec` 由 0 行升级为门禁摘要视图；`v`／`z`／`o` 分别调用 `spec_gate_summary(session, validate|analyze|converge)`，结果存有界字段并显示。键位避开已占用的 `1`—`8`／`j`／`k`／`r`／`t`／`s`／`q`。
3. **数据管线**：走命令式按需查询（不新增常驻线程、不引入定时器）；详情与摘要字段设字符上限（各 200 字符），窗口最小化时随绘制停止而保留缓存，不轮询。
4. **登记与文档**：同步 `surface.rs` 四行、`surface_map.rs` 的 GuiAction 映射与测试、受校验矩阵、spec FR／Check／tasks、CHANGELOG、十三套 i18n 与 PO；新增面有正向与失败路径测试（无数据源时返回结构化说明，不伪造空）。

## 理由与约束

- **为何收敛**：理由码语义与实际效果冲突，属于登记缺陷；“部分”掩盖了已交付的后端能力，与“物质—意识”条款相悖。
- **为何不全做**：其余 12 面中 9 面有 ADR／FR 硬约束，3 面需新增视图或重审 ADR-040；全部翻转会伪造能力状态。
- **承担者**：GUI 维护者（交互与测试）、四端登记维护者（理由码与矩阵）、用户（批准新 ADR 与键位）。

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | GUI 用户可在端内查看事件详情与门禁摘要；登记语义自洽；不新增后端能力 |
| 负面 | GUI `State` 增加三个有界字段；新增键位占用 `e`／`v`／`z`／`o`；十三套 i18n 同步成本 |
| 承担者 | GUI 维护者与登记维护者承担实现；用户承担新键位学习 |
| 和解理由 | 用有限的 GUI 状态增量换取四端读面语义一致；字段有界且无定时器，资源增量可控 |

## 三重时间

- **过去来源**：ADR-032 理由码表、ADR-089 的 spec.* 只读计算说明、T392／T394 的 app 读接口与 TUI 键位、判定报告 16 面表。
- **现在证据**：登记表 16 面提取结果；GUI Spec 视图 0 行、无事件选中态；装配根已实现两个读接口。
- **已知限制与失效条件**：①GUI 实机交互未在本 ADR 阶段验证，以自动化 scripted 测试取证，实机层标 `[待验证]`；②若 `ViewSource` 读接口签名变更，本决策适配层须复审；③若 GUI 引入事件订阅，FR-714 的定时器禁令须重审；④详情与摘要字段各 200 字符上限，超出截断，不伪造完整。
- **复审触发（绑定命令＋判定）**：
  1. `cargo test -p whcode-tests --test frontend_parity` 出现这 4 面不一致即复审登记表与矩阵；
  2. `python .github/scripts/i18n-po.py --check` 漂移即复审 i18n 同步；
  3. `python .github/scripts/check-frontend-budget.py` 超预算即复审 GUI 状态字段与渲染路径；
  4. ADR-032／089 或 FR-711／714 正文修订即重跑 16 面判定。

## 影响

- 正面：GUI 读面与 TUI 同语义；理由码表恢复自洽。
- 负面：GUI 交互面扩大；新键位需与已有键位表核对。
- 不做什么：不改 9 面硬约束与 3 面需新增能力；不改后端读接口语义。
