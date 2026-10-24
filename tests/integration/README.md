# tests/integration

端到端集成测试（tasks.md T008 起）：存储—事件—检查点—恢复—门禁链路。
经本包 `Cargo.toml` 的 `[[test]]` 显式注册（嵌套目录必须显式声明）。

## 已建立

常规回归（直接 `cargo test` 执行）：

- `test_migration.rs`（`migration`）——v0→v7、v1→v7、v2→v7、v5→v7、v6→v7 迁移与 v2／v3 中断回滚；
  现行 schema v7：`events.payload_json` 与其余 6 个 JSON 语义列为 `VARIANT`，
  `stage_artifacts.content` 仍为 `TEXT`（T033、T036；ADR-034/036）。
- `test_durability.rs`（`durability`）——已确认事务的进程终止/重启一致性（T043）；子进程用例
  带 `#[ignore]`，只由父测试以环境变量调用。
- `test_idempotency.rs`（`idempotency`）——重复幂等键跨重启不增行（T044）。
- `test_space_report.rs`（`space_report`）——清理/压缩空间四项统计与 VACUUM 不承诺（T045）。
- `test_session_switch.rs`（`session_switch`）——10 万事件会话的有界加载与游标稳定性（T054）。
- `test_overview_perf.rs`（`overview_perf`）——1 万任务概览 p50/p95/p99，对照 ADR-025 冻结值（T055）。
- `test_trace_and_context.rs`（`trace_and_context`）——脱敏、链路串联、预算截断与重连补拉
  （T067、V-103/V-104）。
- `test_extreme_scale.rs`（`extreme_scale`）——1M 事件极限规模 X1—X5；进程峰值 RSS 由
  `.github/scripts/benchmarks/measure-resources.py` 外部测量。
- `test_sdd_gates.rs`（`sdd_gates`）——V-201—V-207。
- `test_runtime_acceptance.rs`（`runtime_acceptance`）——V-301—V-307。

实验记录器（`#[ignore]`，需 `-- --ignored` 显式运行）：

- `test_writer_stress.rs`（`writer_stress`）——写者服务并发压力实验（T034、ADR-017），
  结果写 `artifacts/writer-service-stress.json`；由 stability workflow 驱动。
- `test_query_plan.rs`（`query_plan`）——索引与查询计划基准（T036、§7.2），含 VARIANT 用例，
  报告写 `artifacts/query-plan-report.json`；由 stability workflow 驱动。
- `test_memory_limit.rs`（`memory_limit`）——DuckDB `memory_limit` 边界实验（ADR-012），
  128／192／384／1024 MB × 2000／20000 条；由 stability workflow 驱动。

夹具生成器（非验收断言）：

- `seed_web_demo.rs`（`seed_web_demo`）——生成 `artifacts/web-demo/whcode.duckdb` 演示工作区；
  打开前删除旧库与 WAL，避免旧存储版本无法容纳 `VARIANT` 列。

## 运行

```powershell
# 常规回归
cargo test -p whcode-tests --test migration
cargo test -p whcode-tests --test durability

# 实验记录器（独占运行，避免资源测量互相污染）
cargo test -p whcode-tests --test writer_stress -- --ignored --nocapture
cargo test -p whcode-tests --test query_plan -- --ignored --nocapture
cargo test -p whcode-tests --test memory_limit -- --ignored --nocapture

# 夹具生成
cargo test -p whcode-tests --test seed_web_demo
```

运行期产物只写 `artifacts/`（如 `artifacts/query-plan-report.json`），不写回 `tests/`。
