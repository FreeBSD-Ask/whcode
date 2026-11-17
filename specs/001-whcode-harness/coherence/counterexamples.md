# 反例台账（M ⊭ γ）

**建立**：2026-11-17（ADR-085） | **判据**：`python .github/scripts/coherence.py --ledger-check` | **单一事实源**：本文件

## 登记规则

1. 登记不等于通过：`status：open` 的条目仍是未满足条款，不得在验收结论里被当作已处理。
2. 每条固定六字段：`id`、`axiom`、`evidence`、`owner`、`exit`、`status`；缺一即台账自身不合法。
3. `closed` 条目必须附修复证据路径；销号不回写历史，保留原条目。
4. 当前运行出现、本台账没有的反例：`pre` 阶段判红；`mid`／`end` 记录并列入报告。
5. 反例按 `axiom` 与本次运行对账；同一公理再次失败而旧条目已 `closed`，按新反例处理。

## 台账

- id：COH-CE-001
  axiom：COH-A1-02
  evidence：2026-11-17 融洽性审计发现 T366 已勾选但无任何 Check 引用（`check_counts.py` 反向覆盖判据建立前的状态）；见 `specs/001-whcode-harness/checklist.md` Check 465 补正段
  owner：过程门禁维护者
  exit：Task T366 被至少一个 Check 引用，且 `check_counts.py` 的反向覆盖判据在 CI 绿
  status：closed（修复证据：Check 465 补正段与 `check_counts.py` 的 `check_completed_tasks_covered_by_checks`）

- id：COH-CE-002
  axiom：COH-A1-05
  evidence：2026-11-17 更名批次取证时工作树与提交树不一致——提交 f5c4e8d 未含 `crates/whcode-web/frontend/styles.css` 与 i18n 值层改名，`cargo test` 对提交树不可复现；见 `specs/001-whcode-harness/checklist.md` Check 462 更正段
  owner：批次执行者
  exit：取证记录写明报告对应的提交哈希，且工作区与提交树差异清零后重跑同一命令
  status：closed（修复证据：Check 462 更正段与同日收口的 CSS／i18n 改名提交）

- id：COH-CE-003
  axiom：COH-A1-05
  evidence：`python .github/scripts/coherence.py --phase mid` 报 19 条已勾 Check 无取证／复核标记（274—287 不含 288、289—290、293、295、298、341）
  owner：批次执行者
  exit：逐条补取证记录，或把证据态降为 present／wired；`coherence.py --phase end --strict` 不再报本条
  status：open

- id：COH-CE-004
  axiom：COH-A1-06
  evidence：`python .github/scripts/coherence.py --phase mid` 报 59 条行为／性能类已勾 Check 缺 `index=` 或 run 引用
  owner：批次执行者
  exit：新增 Check 必须带 `index=` 或 run 引用；存量按批次回填，回填完成前本条保持 open
  status：open

- id：COH-CE-005
  axiom：COH-A1-07
  evidence：`python .github/scripts/coherence.py --phase mid` 报 75 条预算／平台类已勾 Check 缺 `scope=`（含 Check 252／336；2026-11-17 第二批复跑为 75，首批 74，随 Check 新增而变化）
  owner：性能与 GUI 负责人
  exit：预算／平台类 Check 全带 `scope=`，且 Check 252／336 按新作用域完成重判
  status：open

- id：COH-CE-006
  axiom：COH-A2-04
  evidence：`python .github/scripts/coherence.py --phase mid` 报 FR-406 仅由 Check 172（负极性）覆盖
  owner：工具循环负责人
  exit：FR-406 补正面验收 Check，或按缺口登记退出并删除本条
  status：open

- id：COH-CE-007
  axiom：COH-A2-05
  evidence：`python .github/scripts/coherence.py --phase mid` 报 FR-406 的覆盖 Check 172 未声明 `polarity=negative`
  owner：工具循环负责人
  exit：Check 172 声明 `polarity=negative`，并由正面 Check 补位
  status：open
