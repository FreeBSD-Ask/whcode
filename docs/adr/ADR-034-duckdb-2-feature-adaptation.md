# ADR-034：DuckDB 2.0 功能适配（VARIANT、MERGE 与资源策略）

- **状态**：已批准（2026-10-24，用户批准 `migrate-duckdb-200` spec）
- **来源**：`specs` 变更 `migrate-duckdb-200`；ADR-DB-BUILD；ADR-012（memory_limit 基线）；ADR-025（性能预算冻结）；DuckDB 2.0 官方 VARIANT 与 MERGE 文档
- **责任人**：存储与性能负责人
- **退出条件**：DuckDB 2.x 改变 `VARIANT` 语义或 `MERGE` 行为、VARIANT 查询计划出现不可接受回归、导出/脱敏路径失效、预算重新冻结时，须新 ADR 修订本决策

## 决策

1. **`events.payload_json` 改为 `VARIANT`（schema v6）**。写入时必须走“JSON 文本 → JSON → VARIANT”的显式链路（`CAST(? AS JSON)::VARIANT`），不得直接 `CAST(TEXT AS VARIANT)`，否则会得到包着 JSON 的 VARCHAR 变体。读取、导出、压缩与 redacted 路径统一用 `CAST(CAST(payload_json AS JSON) AS VARCHAR)` 取得 JSON 文本。
2. **`stage_artifacts.content` 保持 `TEXT`**。该列可能是任意文本而非 JSON，不能假设可解析。
3. **`session_overview` upsert 改为 `MERGE INTO`**，保留单会话唯一匹配语义；多源匹配同一目标行必须按 2.0 行为报错，不得静默选一行。
4. **数据修改 CTE 不采用**。A/B 实测显示把多表删除合并为单条数据修改 CTE 反而约慢 10%，且 `purge` 需要逐表实际删除行数、`export` 需要逐表校验与错误定位；没有可测收益，保留原实现。
5. **资源策略按实测决定**：利用 2.0 聚合溢写复测 `memory_limit` 128/192/384 MB。实测 128 MB 在 2000 条小样本下无 OOM，但未取得 20000 条同夹具吞吐，不能据此证明 NFR-210，故**默认保持 192 MB**；若后续取得 128 MB 的同规模吞吐证据，再按 ADR-012 修订记录降低。
6. **不采用**：触发器、Quack、C API v2／`duckdb-neo`、其他 JSON 列的 VARIANT 化、嵌套 schema、`NEAREST` joins、JSON mutation functions、扩展仓库、agent mode、ICU 扩展、异步 I/O。采用任一能力须用户确认并修订本 ADR。
7. **扩展安全默认显式收紧**：工作区连接初始化时设置
   `autoinstall_known_extensions=false`、`autoload_known_extensions=false`、
   `allow_community_extensions=false`、`allow_extension_repositories='forbidden'`。
   这是对 DuckDB 2.0 宽松默认值的显式覆盖，避免查询触发联网安装扩展；
   `enable_external_access` 保留 `true`，因为导出/导入依赖文件访问，关闭需单独威胁模型。
   契约测试断言四项设置的实际取值。

## 证据

- **语法与语义**（DuckDB 2.0.0 CLI 实测）：`CAST('{"a":1}' AS JSON)::VARIANT` 得到 `OBJECT`；`CAST('{"a":1}' AS VARIANT)` 得到 `VARCHAR` 变体；`CAST(v AS JSON)` 可得到 JSON 文本；`variant_to_json` 在 2.0.0 中不存在；`ALTER COLUMN ... TYPE VARIANT` 不解析 JSON。
- **迁移**：`0006_events_variant.sql` 重建 `events` 表并用 `CAST(payload_json AS JSON)::VARIANT` 复制；迁移测试覆盖 v0→v6 与 v5→v6，断言 `payload_json` 的 `data_type = 'VARIANT'` 且 `variant_type(payload_json) = 'OBJECT'`。
- **MERGE**：`session_overview` 连续刷新幂等、单行、事务回滚测试通过；多源匹配同一目标行按 2.0 报错。
- **DML CTE A/B**：同一夹具、独立临时 crate、release、20 轮，每轮 1000 会话／5 万任务／20 万叶子行；三条分表 `DELETE` p50 10 893 µs / p95 11 708 µs，合并为单条数据修改 CTE p50 12 008 µs / p95 12 336 µs，慢约 10%。
- **资源**：`duckdb.dll` 由 36 697 896 字节增至 61 311 784 字节；归档由 13 408 787 字节增至 19 833 650 字节。RSS、吞吐与 `memory_limit` 结果见 `artifacts/duckdb-2.0-audit-2026-10-24.md`。

## 正负和解

- **正面**：`VARIANT` 为事件载荷提供原生 shredded 存储，压缩与半结构化查询更好；`MERGE` 让概览 upsert 使用标准语义并显式暴露多源匹配冲突；不引入任何新的网络服务或扩展仓库，安全面不变。
- **负面**：`VARIANT` 对象键会被排序，读写/导出必须显式转换，Rust 侧不再能直接得到原始文本；转换函数与显式 cast 增加了 SQL 复杂度；原型表 `m0_events` 仍为 `TEXT`，与正式 `events` 存在类型差异，需要在文档中说明；DLL 与归档体积显著增大。
- **承担者**：存储与导出路径维护者、审计与脱敏测试维护者、打包与分发维护者。
- **和解理由**：只对高体积、确实为 JSON 的事件载荷使用 `VARIANT`，其余契约 JSON 保持 `TEXT`；用显式转换与测试锁定语义，不用隐式 cast 换取短期简洁。体积增大是官方 2.0 资产的既定代价，记录而不隐藏。

## 替代方案（已否决）

- **保持 `payload_json TEXT`**：放弃 2.0 的核心存储能力，迁移只剩版本号变化；与本 spec 的“功能适配”目标不符。
- **把所有 JSON 列都改为 `VARIANT`**：`stage_artifacts.content` 可能是任意文本；小体积契约 JSON 收益有限，却扩大导出/校验改造面。
- **用触发器维护 `session_overview`**：引入隐式数据库行为，与“事件由应用层显式产生、审计可追踪”的信号链原则冲突；默认不采用。
- **启用 Quack 支持多进程写**：与本地优先、默认回环、资源四件套与单写者边界冲突；如未来确有需求，另立 ADR。

## 影响

- 代码：`crates/whcode-store/migrations/0006_events_variant.sql`、`schema.rs`、`export.rs`、`checkpoints.rs`、`compact.rs`、`monitor.rs`、`overview.rs` 及 app/bin/cui/web 的 payload SQL。
- 测试：迁移、事件往返、导出/导入、redacted、查询计划、资源与崩溃路径。
- 文档：本 ADR、`ADR-DB-BUILD` 修订记录、`ADR-012`（如调整 128 MB）、`ADR-025`（如重新冻结）、矩阵与指南。

## 复审触发

- 2.0.x 补丁改变 `VARIANT`/`MERGE` 行为或新增官方 JSON 转换函数。
- `payload_json` 查询计划出现全表扫描或转换成本高于预算。
- 128 MB `memory_limit` 决策落地后出现 OOM 或吞吐回归。
- 未来需要多进程写、触发器或全量 VARIANT 化时，重新评估并修订本 ADR。
