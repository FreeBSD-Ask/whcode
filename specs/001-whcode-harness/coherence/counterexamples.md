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
  evidence：`python .github/scripts/coherence.py --phase mid` 报行为／性能类已勾 Check 缺 `index=` 或 run 引用（数量随 Check 增删变化，当前值以最近一次 mid 报告的 `counterexamples` 段为准，不在本字段硬编码；2026-11-23 实跑读数见 `artifacts/coherence/run-mid-*.json`）
  owner：批次执行者
  exit：新增 Check 必须带 `index=` 或 run 引用；存量按批次回填，回填完成前本条保持 open
  status：open

- id：COH-CE-005
  axiom：COH-A1-07
  evidence：`python .github/scripts/coherence.py --phase mid` 报预算／平台类已勾 Check 缺 `scope=`（含 Check 252／336；数量随 Check 增删变化，当前值以最近一次 mid 报告的 `counterexamples` 段为准，不在本字段硬编码；2026-11-17 批次曾记 74／75）
  owner：性能与 GUI 负责人
  exit：预算／平台类 Check 全带 `scope=`，且 Check 252／336 按新作用域完成重判
  status：open

- id：COH-CE-006
  axiom：COH-A2-04
  evidence：`python .github/scripts/coherence.py --phase mid` 报 FR-406 仅由 Check 172（负极性）覆盖
  owner：工具循环负责人
  exit：FR-406 补正面验收 Check，或按缺口登记退出并删除本条
  status：closed（修复证据：FR-406 在 `spec.md` 显式标 `polarity=conditional`——它是延后能力，其“未通过前置条件时不得声称交付”本质是负极性验收；Check 172 合法覆盖。2026-11-23 mid 的 COH-A2-04 转 satisfied）

- id：COH-CE-007
  axiom：COH-A2-05
  evidence：`python .github/scripts/coherence.py --phase mid` 报 FR-406 的覆盖 Check 172 未声明 `polarity=negative`
  owner：工具循环负责人
  exit：Check 172 声明 `polarity=negative`，并由正面 Check 补位
  status：closed（修复证据：Check 172 增 `polarity=negative` 与 `implies_fr=FR-406` 字段，且 `coherence.py` 增 `implies_fr` 一致性谓词 COH-A2-06；2026-11-23 mid 的 COH-A2-05 转 satisfied）

- id：COH-CE-008
  axiom：COH-M1-07
  evidence：推理规则 R 编号曾在三处指不同命题——`docs/guides/axiom-system.md` §3 原 R1—R7（判定／Hoare 对应／…）与 `docs/analysis/software-engineering-logic-sufficiency.md` §4.2 的 R1—R8（门禁合成／追溯闭包／…）及 `axioms.toml` 的 COH-R4/R5/R6/R8 语义分叉
  owner：规则体系维护者
  exit：`symbols.toml` 注册两套命名空间；axiom-system.md §3 改用 CR1—CR7；`coherence.py --selftest` 越界负向探针通过且 `--phase pre` 的 COH-M1-07 为 satisfied
  status：closed（修复证据：`coherence/symbols.toml`、`docs/guides/axiom-system.md` §3、`coherence.py` 的 `symbol_unambiguous` 谓词与 selftest；2026-11-23 pre 报 total=47 counterexample=0）

- id：COH-CE-009
  axiom：COH-A6-01
  evidence：ADR-100 文件存在且被 `crates/whcode-app/src/turn.rs`、`crates/whcode-domain/src/event.rs` 注释引用，但在 spec／tasks／checklist 账本中零引用；其注释引用的 FR-808 在 `spec.md` 中不存在（最高 FR-807）。两项均为未提交工作树内容
  owner：ADR-100 作者（本轮不改动其未提交源码）
  exit：FR-808 写入 spec.md 并由任务与 Check 覆盖，或 ADR-100 改挂到既有 FR；账本引用非零
  status：closed（修复证据：`spec.md` 立 FR-808，`tasks.md` T426 与 `checklist.md` Check 498 覆盖；ADR-100 主循环接线完成；2026-11-23 `traceability.py` 零断链含 FR-808）

- id：COH-CE-010
  axiom：COH-A6-03
  evidence：`python .github/scripts/coherence.py --phase mid` 报已跟踪源码注释引用 spec 不存在的 FR：`crates/whcode-llm/src/http/profile.rs:FR-209`、`crates/whcode-llm/src/http/protocols.rs:FR-209`、`crates/whcode-llm/src/http/protocols.rs:FR-210`（spec 的 FR 在 208 后跳至 718）；`event.rs:FR-808` 由 COH-CE-009 单独持有
  owner：LLM 领域维护者
  exit：三处引用改挂到实际存在的 FR 编号，或按 ADR 补立 FR-209／FR-210 并由任务与 Check 覆盖
  status：closed（修复证据：四处测试断言消息去掉历史 FR 编号、改为描述性文案，不虚构映射；2026-11-23 `coherence.py --phase mid` 的 COH-A6-03 转 satisfied）
