# ADR-080：评审发现台账（P2.2，四要件＋修复乐观锁）

- **状态**：已批准（2026-11-15，proposal-v3 P2.2；用户指令“先做其他开发”）
- **来源**：`research/synthesis/README.md` §2 R3（BH“发现工程：finding 四要件＋修复乐观锁＋
  独立复评”）；`research/synthesis/proposal.md` P0-5（finding 登记模板：被检视缺口＋有界影响＋
  最小 owner 修复＋验证路线）；ADR-002（SQL 只在 store）、ADR-018（权限在 app）、ADR-046
  （证据态）、ADR-048 §7.1（会话内工具与 SDD 账本隔离的先例）
- **责任人**：评审与门禁维护者、存储负责人
- **退出条件**：见“已知限制与失效条件”——本台账被真实评审流程弃用，或发现字段无法承载
  实际发现类型时复审

## 1. 问题与边界

本仓的“发现”现状是**非结构化**：评审结论散落在 `artifacts/` 的复盘文档、Check 的 `**取证**`
段落与 commit 正文里，可读但不可查询、不可加锁、无法回答“这条发现修没修、什么时候修的、
谁改过”。synthesis §2 R3 的判定是“通过（缺口登记惯例非结构化）”，P2.2 要把它补成台账。

边界：

- **本批只做库与命令层**（schema v12＋`whcode-store::findings`＋`whcode-app::findings`）；
  模型工具面（类似 `todo_write` 的 finding 工具）与 CUI/Web 命令入口属 P2.2b，不在本批声称交付；
- 不做“独立复评者”的自动化（BH 的复评是多代理流程，本仓无多代理）；`verification` 字段
  记录**验证路线**，由人执行；
- 不与 SDD `tasks` 表混用：任务是交付单位，发现是“评审看到的缺口”；两者可互相引用
  （`owner_fix` 里写任务号），但台账不合并（ADR-048 §7.1 的同构理由）。

## 2. 决策

1. **表与迁移**：schema v12 新增 `findings`（迁移 `0012_findings.sql`），列＝
   `id, session_id, title, gap, impact, owner_fix, verification, status, severity, version,
   created_at, updated_at, fixed_at`；`id` 由序列 `whcode_finding_id_seq` 生成 `FND-<n>`
   （与 `S-<n>` 同构）；索引 `(status, created_at, id)`。加入 `EXPORT_TABLES`（工作区数据）。
2. **四要件必填**：`gap`（缺口）、`impact`（有界影响）、`owner_fix`（最小修复）、
   `verification`（验证路线）四列 NOT NULL，且 store 层校验“trim 后非空、≤2000 字符”；
   标题 ≤200 字符。缺一即 `invalid_state`，错误消息点名缺哪一项——**只有缺口的抱怨不可执行，
   只有修复的想法无法复核**。
3. **修复乐观锁**：每次状态迁移必须带 `expected_version`；`UPDATE ... WHERE id = ? AND
   version = ?`，影响行数为 0 时分两种事实——记录不存在（`not_found`）与版本冲突
   （`invalid_state`，消息给期望与实际版本）。并发修改不静默覆盖；**失败的迁移不消耗版本**
   （非法迁移在 UPDATE 前拒绝）。
4. **状态机**（封闭四态与迁移表）：`open → fixing|fixed|dismissed`、
   `fixing → fixed|dismissed|open`、`fixed → open`（重开）、`dismissed → open`；同态不算迁移，
   其余组合 `invalid_state`。`fixed` 时落 `fixed_at`。
5. **严重度只三档**（`low`／`medium`／`high`）：五档或十分制是假精度（synthesis §2 R1 的
   “拒绝分数天花板”同理）；未知取值显式报错，不静默取默认。
6. **权限**：登记与迁移按写类（`SessionCreate`），列表按读类（`SessionRead`）；与待办/交付物
   同口径（发现台账跨会话，但写入仍是会话内动作）。
7. **字符串枚举在 app 层收敛**：存储层用枚举（`FindingStatus`／`FindingSeverity`），库里出现
   未知值时读取即 `parse_error`（库内容漂移必须显式，不能当成 `open`）。
8. **非目标**：模型工具面与前端命令（P2.2b）、复评自动化、与 `tasks` 表的双向同步、发现
   附件的二进制内容。

## 3. 正负和解

**正面**：发现从“文档段落”变成可查询、可加锁、可导出的记录；四要件强制让每条发现可执行、
可复核；乐观锁把“两人同时改”从静默覆盖变成显式冲突。

**负面与承担者**：

- 四要件提高登记门槛：随手记一句就不行了——承担者是登记者（但门槛正是本 ADR 的目的）；
- 需要人守迁移表与字段上限，字段不够时得走 schema 迁移——承担者是维护者；
- 乐观锁让调用方必须重读再提交（多一次往返）——承担者是调用方（P2.2b 的工具会封装重试提示）。

**和解理由**：非结构化发现的可读性已经足够，缺的是**可判定性**（修没修、谁改过）；台账用
最小字段集换可判定性，成本集中在登记时而非事后追问。

## 4. 三重时间

- **过去**：synthesis R3 与 proposal P0-5 已把四要件模板写进评审协议候选；本仓 Check 的
  `**取证**` 段落是它的手工前身；
- **现在**：schema v12＋store/app 命令与 9 项测试（四要件、乐观锁、非法迁移、过滤排序、权限、
  上限）；`EXPORT_TABLES` 13 张的完整性断言通过；
- **将来失效条件**（绑定命令＋判定）：`cargo test -p whcode-store findings` 或
  `cargo test -p whcode-app findings` 红 → 停止交付；发现字段不足以表达一类真实发现
  （例如需要“证据附件引用”）→ 复审本 ADR 并走 schema 迁移；P2.2b 工具化后发现登记率长期为 0
  → 复审“台账是否值得维护”。

## 5. 回退方案

纯新增表与模块：删除 `crates/whcode-store/migrations/0012_findings.sql` 与两个模块、从
`EXPORT_TABLES` 移除 `findings` 即可；但已迁移库的 `schema_migrations` 留有 v12 记录，
**回退需同批提供降级说明**（本仓无自动降级，按 ADR-038 的备份恢复路径处理）。

## 6. 已知限制与失效条件

- **无自动复评**：`verification` 是路线不是结果；是否修好由人/CI 判定；
- **失败迁移不消耗版本**，但**成功的迁移与业务写入之间没有跨表事务**（单写者服务保证顺序，
  读取-迁移之间仍有窗口；窗口内被改会得到冲突而不是覆盖——这是设计意图，不是缺陷）；
- **无软删除**：`dismissed` 是终态之一，删除记录需直接改库（未提供命令）；
- **导出包含发现**：分享/导出会把评审缺口文本带出去，其中可能含敏感上下文——导出侧沿用
  既有脱敏口径，未在本表加额外脱敏；
- **已知缺口（本批发现）**：`approval_rules` 未出现在 `EXPORT_TABLES` 也未在
  `EXCLUDED_TABLES` 登记，导出/导入会丢审批规则——待单独确认是缺陷还是有意排除（不在本批改）。

## 7. 文献依据（标准研读记录，§1.6 口径）

- ISO/IEC/IEEE 29148:2018（已登记，已覆盖）：需求/发现的可验证性——“每条陈述必须有可判定
  的验证方式”，本 ADR 的 `verification` 必填即该条目的工程化；
- ISO/IEC/IEEE 15026-2:2022（已登记，已覆盖）：claim→argument→evidence——四要件即
  gap(claim)／impact(argument)／owner_fix＋verification(evidence 路径)；
- ISO/IEC/IEEE 15939:2017（已登记，已覆盖）：指标→测量→判定——发现状态与迁移表即 decision
  criteria，乐观锁保证判定不被静默改写；
- 一手先例：本仓 ADR-048 §7.1（待办工具“整表替换＋与 SDD 账本隔离”）与 ADR-065（分叉的
  边界校验），复用其“状态显式、边界显式”的口径。

## 8. 形式审查

- **真实矛盾**：评审发现不可查询、不可加锁，复盘时只能靠人记；而“加一个表”就能让它可判定；
- **正负和解**：见 §3，登记门槛与往返成本显式，承担者明确；
- **0 信任校验**：状态机与乐观锁以测试钉住（过期版本、非法迁移、失败不消耗版本）；
  导出完整性由既有 `EXPORT_TABLES` 断言覆盖；
- **知难行易**：方向＝P2.2“finding 四要件＋修复乐观锁”；目标＝库/命令层可登记、可迁移、
  可导出；判据＝store 5 项＋app 4 项测试；非目标＝工具面（P2.2b）、复评自动化、与 tasks 同步。
