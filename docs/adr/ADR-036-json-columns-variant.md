# ADR-036：JSON 语义列全面 VARIANT 化与 2.0 能力逐项取舍

- **状态**：已批准（2026-10-24，用户批准 `extend-duckdb2-adaptation` spec）
- **来源**：`migrate-duckdb-200` 与 ADR-034（初版只转换 `events.payload_json`）；DuckDB 2.0.0
  官方 `VARIANT`、`MERGE`、数据修改 CTE 与 `variant_*` 文档；本地 CLI 2.0.0 实测
- **责任人**：存储与导出路径维护者
- **退出条件**：2.x 改变 `VARIANT` 归一化或 `variant_*` 语义、列转换导致预算越界、
  导出/导入往返出现语义差异、需要跨进程访问这些列时，重新评估本决策

## 背景

ADR-034 的边界是“`VARIANT` 只用于 `events.payload_json`”，理由是当时只有事件载荷被证明
“高体积且确为 JSON”。该边界没有逐列核对读写代价，导致其余 6 个 JSON 语义列仍以文本存储，
也把 `MERGE`／数据修改 CTE／`variant_*` 的使用停在单点。本 ADR 记录逐项审计后的新边界。

## 决策

1. **schema v7：6 个 JSON 语义列改为 `VARIANT`**——`tasks.acceptance_json`、
   `stage_artifacts.refs_json`、`session_overview.by_stage_json`、`checkpoints.delta_json`、
   `checkpoints.artifact_refs_json`、`audit.detail_json`。迁移 `0007_json_variant.sql` 逐表重建，
   以 `CAST(col AS JSON)::VARIANT` 复制；非法 JSON 与顶层 JSON `null` 由 CAST／NOT NULL 拒绝，
   整个事务回滚且不登记版本。
2. **文本归一化是既定语义**：`VARIANT` 读回 JSON 文本会去除多余空白并排序对象键。
   对外契约仍是 JSON；导出文件与测试不得再断言“与写入原文逐字节相同”，须按 JSON 值比较。
3. **读写口径唯一**：写入 `CAST(? AS JSON)::VARIANT`；读取/导出/压缩/展示
   `CAST(CAST(col AS JSON) AS VARCHAR)`。JSON 文本层的合法性、`delta_json` ≤ 64 KiB、
   `artifact_refs_json` 条目上限等校验全部保留。
4. **边界不变**：`stage_artifacts.content`（任意文本）与原型表 `m0_events.payload_json`
   保持 `TEXT`；不启用触发器、Quack、C API v2、扩展仓库、嵌套 schema。
5. **MERGE 全量审计结论**：`session_overview` 已是 `MERGE INTO`；`events` 幂等键与
   `schema_migrations` 是 `INSERT … ON CONFLICT DO NOTHING`（追加/去重，不是 upsert），
   `smoke_kv` 属 M0 冒烟夹具，均**不改为 MERGE**。无其余 upsert 位置。
6. **数据修改 CTE 不采用**（新证据）：`purge` 删除＋审计在 20 万事件／40 会话夹具上，
   两条语句中位 9.922 s，单条数据修改 CTE 中位 10.004 s（+0.83%，三轮）；采用门槛
   “p95 不劣化且语句数减少”未满足。且 `purge` 需要逐表实际删除行数与错误定位，
   合并会降低可诊断性。与 ADR-034 的三表删除 A/B（约 +10%）方向一致。
7. **`variant_*` 采用清单**：
   - `variant_type`：schema 测试断言事件载荷全为 `OBJECT`，迁移测试断言 6 列顶层类型
     （对象/数组），作为写入链路缺陷的可归因信号；
   - `variant_keys` / `variant_extract`：迁移测试验证对象列键集合与字段值可在引擎侧查询；
   - **不采用**把“事件摘要、redaction”等动态键路径下推 SQL：这些路径按事件类型读取任意键
     并做两种脱敏，SQL 侧需要按 kind 分支或拉平为多列，收益无法测量且扩大 SQL 面；
     记录为书面否决，未来有实测收益再修订。

## 证据

- **迁移**：`crates/whcode-store/migrations/0007_json_variant.sql`；
  `tests/integration/test_migration.rs` 的 v6→v7 用例断言 6 列 `data_type='VARIANT'`、
  数据按 JSON 语义读回、索引重建齐全、`stage_artifacts.content` 仍为文本。
- **健康断言**：`crates/whcode-store/src/schema.rs` 的
  `event_payloads_are_object_variants`（非 OBJECT 计数为 0）与
  `json_columns_are_variant_and_text_columns_stay_text`。
- **DML CTE A/B**：`local/dml_cte_ab.py`，20 万事件 / 40 会话 / 40 轮 / 3 次重复；
  A（两条语句）中位 9.922 s（样本 10.186、9.922、8.855），B（数据修改 CTE）中位 10.004 s
  （10.004、10.333、9.080），B 相对 A +0.83%；两侧均校验 `events=0`、`audit=40`。
- **MERGE 审计**：`crates/whcode-store/src/overview.rs`（MERGE）、`writer.rs` 与
  `smoke.rs`（ON CONFLICT DO NOTHING）、`schema.rs` 迁移登记（ON CONFLICT DO NOTHING）。

## 正负和解

- **正面**：所有 JSON 语义列统一为原生 VARIANT，存储与未来结构化查询不再受文本解析限制；
  读写口径统一为两条可检索规则，减少“哪些列是 JSON”的隐含知识；`variant_*` 让迁移与
  健康检查可直接在引擎侧断言，不必在 Rust 里重建解析路径。
- **负面**：每次读写多一层显式转换；文本归一化使“原文逐字节保存”不再成立，导出比对与
  审计展示需要按 JSON 值判断；5 张表重建带来一次性迁移成本；读取路径若漏改会在运行期
  报类型错误，测试面必须覆盖每条投影。
- **承担者**：存储读写与导出入入口维护者、审计与测试断言维护者、迁移评审人。
- **和解理由**：以“统一的类型语义 + 显式转换 + 文本层校验前移”换取可查询性与一致性；
  归一化差异被写入文档与测试，不用隐式兼容或双写掩盖。对无收益的 DML CTE 与动态键下推
  明确否决并留数字，不以“新语法更好”作为采用理由。

## 替代方案（已否决）

- **维持 ADR-034 的仅 events 边界**：无需重建 5 张表，但 6 列继续以文本存储，
  `variant_*` 无法用于这些列，类型语义长期分裂；本 spec 的用户指令要求逐项评估与改造。
- **用 `ALTER COLUMN ... TYPE VARIANT` 原地转换**：2.0 不解析 JSON 文本，只会得到
  VARCHAR 变体，语义错误。
- **把 `stage_artifacts.content` 一并转换**：该列是任意文本，不保证 JSON，转换会拒绝合法内容。
- **用触发器维护概览或审计**：引入隐式行为，与“事件显式产生、审计可追踪”冲突（沿用 ADR-034）。

## 影响

- 代码：`whcode-store`（迁移、schema、tasks、checkpoints、overview、export、purge、
  compact、monitor、smoke）、`whcode-app`、`whcode-web`、`whcode-bin` 的投影与夹具。
- 测试：迁移、导出/导入、脱敏、压缩、空间统计、查询计划与稳定性夹具中的 JSON 文本断言。
- 文档：本 ADR、ADR-034 修订记录、`data-model.md`、`module-design.md`、`AGENTS.md` 存储规则、
  `tests/integration/README.md`、`CHANGELOG.md`、`tasks.md` T160 与 `checklist.md` Check 262。

## 复审触发

- 2.x 改变 `VARIANT` 的键序/空白归一化或 `variant_*` 返回类型；
- 受影响查询出现额外全表扫描，或 NFR-201/209/210 越界；
- 需要跨进程读取这些列（当前单写者边界下不成立）；
- 事件摘要或脱敏路径出现可测量的 SQL 下推收益。
