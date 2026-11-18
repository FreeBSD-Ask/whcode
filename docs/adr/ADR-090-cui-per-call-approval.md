# ADR-090：CUI 写类工具逐次显式授权（--approve）

- **状态**：已批准（2026-11-19，用户对“P1 写类工具 CLI 审批装配”方向的“批准”指令）
- **编号认领**：ADR-090（089 归并行批次四端对等清零，`ls docs/adr/` 复核）；FR-798；
  T397—T399；Check 481

## 任务认知卡

- **方向**：让模型经 CUI 真正“写”代码——写类工具（write_file/edit_file）在非交互
  CLI 从结构性不可达变为逐次显式授权可达；默认拒绝不变。
- **目标**：`llm run --approve workspace-write` 下真模型完成一次文件写入全链
  （授权→审批 Allow→工具执行→diff 留痕→续答）；未授权行为与现状逐字一致。
- **用户意图复述**：用户对“P1 写类工具 CLI 审批装配（让模型真正写代码而不只是读）”
  的方向批准；信任姿态维持项目铁律（默认拒绝、逐次显式、审计落表）。
- **非目标**：持久化授权配置文件（逐次显式优于常驻授权，登记边界）；shell 的免白名单
  自动化（shell.json 白名单闸门组合不变）；TUI/GUI 交互审批卡（已有）；SDK/ACP 面。
- **判据**：真模型 e2e（文件真实产生＋审计事件＋diff 留痕）；未授权回归（现状逐字）；
  未知范围显式拒绝。
- **手段（后置）**：既有 `ApprovalGate::for_store`＋`ApprovalPolicy` 逐范围 Allow；
  `run_turn_with_approval` 替换 `run_turn`（闸门审计照常）。

## 问题

CUI `llm run` 无审批闸门（`run_turn` 不带 gate）——写类工具按“无人可问”结构性
fail-closed，模型在 CLI 只能读不能写。TUI/GUI 已有交互审批面（T255—T257），CLI 自动
化面缺一个**显式、逐次、可审计**的授权通道。

## 决策

1. **逐次显式授权**：`llm run --approve <范围>`（逗号分隔，可多值）——
   `workspace-write`（write_file/edit_file，`SideEffectScope::WorkspaceWrite`）与
   `process`（shell，`SideEffectScope::ProcessSpawn`）。授权范围在**本次调用内**生效，
   不落任何持久化授权状态。
2. **策略构造**：授权范围 → `ApprovalPolicy` 对应默认规则 `Allow`；未授权写类
   `Ask`＋无应答者 → `unavailable`（fail-closed 与现状一致）；`with_builtin_destructive_ask`
   与 `with_stored_rules`（DB 规则）照常叠加；shell 额外受 `.whcode/shell.json`
   白名单闸门组合约束（`shell_rule`：Allow 仅在白名单命中时生效——进程执行的
   双闸门不变）。
3. **闸门与审计**：授权时以 `run_turn_with_approval` 替换 `run_turn`（闸门审计照常
   落表）；未授权时保持 `run_turn`（行为逐字一致，零额外开销）。
4. **显式拒绝未知范围**：未知 `--approve` 值直接结构化报错（不静默忽略）。

## 理由与约束

- 信任姿态：默认拒绝不变；授权是**每次调用的显式用户动作**（不隐藏、不持久、可撤销
  ＝不传即可）；审计落表使每次授权可追溯。
- shell 双闸门（`--approve process` ＋ shell.json 白名单）保持 ADR-070 的进程执行
  约束；仅 `--approve process` 而无白名单时 shell 仍 fail-closed。
- 资源纪律：无新增常驻结构；闸门复用既有实现。

## 修订记录

- **2026-11-19（前提失真修正，0 信任自查）**：本 ADR 的问题陈述有误——写类工具在 CUI
  **并非** fail-closed：闸门检查为 `if let Some(gate)`（turn.rs 通用闸门段），无闸门＝
  写工具直接执行（`ApprovalConfig` 默认 write=Allow；TUI/GUI 闸门规则亦默认 Allow，
  四端在“写”上事实对称）。真实差异是 **CUI 无闸门＝无审计**（TUI/GUI 的 Allow 也落
  审计）。因此本批的真实价值重述为：①给 CUI 补上**审计闸门通道**（`--approve` 时
  闸门存在且 Allow 决策落审计——原本无闸门即无审计）；②`--approve` 的授权语义为
  调用级显式宣告（与默认 Allow 的差异＝留痕而非收紧）。原“未授权 fail-closed”表述
  撤销；字段测试①的相应结论同此修正。四端“写是否需要闸门/审计”的统一姿态为显式
  安全决策，提交用户裁定（安全反教条：以证据与代价分析呈现，不以默认惯例结项）。

## 已知限制与失效条件（复审触发，绑定命令＋判定）

- **已知限制**：①授权是调用级全有全无（无参数级约束）——参数级策略属动态权限
  broker（P2 范围）；②持久化授权配置未做（自动化高频场景需逐次传参）；③
  `--approve process` 不解除 shell.json 白名单要求。
- **将来失效条件与复审触发（绑定命令＋判定）**：
  1. 未授权回归用例任一断言失败 → 回滚本批装配分支；
  2. 引入参数级权限 broker（P2）时，本批的调用级授权成为其子集——复审合并口径；
  3. GUI/TUI 审批卡的策略构造变化（`to_policy_interactive`）→ 本批策略构造对齐复审。

## 替代方案

- **持久化授权配置文件**（`.whcode/autonomy.json`）：高频自动化更便利，但常驻授权
  与“逐次显式”冲突且易被遗忘——否决为本批方案，登记为后续可选项。
- **TUI 交互卡复用**：CUI 无终端 UI 面；否决。

## 影响

- **正面**：模型经 CLI 可真实写代码（功能对齐线的关键一步）；授权语义与 TUI/GUI
  同源（闸门＋审计）；自动化脚本可按需授予。
- **负面**：`llm run` 增授权面（误用 `--approve` 即真写——文档与 `--help` 必须醒目）。
- **承担者**：bin/cui 维护者、用户（授权决策）。

## 三重时间

- **过去来源**：ADR-070（shell 双闸门）、ADR-064（审批 broker 四态）、ADR-088/089
  （工具链路与四端对等）、字段测试②（写类不可达的实证背景）。
- **现在证据**：`ApprovalGate`/`ApprovalPolicy` 在库并有 TUI/GUI 消费方；
  `run_turn_with_approval` 在库。
- **将来失效条件**：见“已知限制与失效条件”。

## 关联

- 需求：FR-798；任务 T397—T399；验收 Check 481。
- 字段测试：`fieldtest2-2026-11-19.md`（读链路已通；写链路本批补全）。
