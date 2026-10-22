# tests/integration

端到端集成测试（tasks.md T008 起）：存储—事件—检查点—恢复—门禁链路。

计划：
- `test_migration.rs` —— v0→v3 / v1→v3 / v2→v3 迁移与中断回滚（T033、T036）
- `test_writer_stress.rs` —— 写者服务并发压力实验（T034、ADR-017）
- `test_query_plan.rs` —— 索引与查询计划基准实验（T036、§7.2）
- `test_durability.rs` —— 已确认事务的进程终止/重启一致性（T043）
- `test_idempotency.rs` —— 重复幂等键（T044）
- `test_space_report.rs` —— 清理/压缩空间统计（T045）
- `test_session_switch.rs` —— 10 万事件会话的有界加载与游标稳定性（T054）
- `test_overview_perf.rs` —— 概览查询 p50/p95/p99（T055）
- `test_sdd_gates.rs` —— V-201—V-207（T081）
- `test_runtime_acceptance.rs` —— V-301—V-307（T114）

当前状态：schema v3、迁移测试（T032/T033）、写者服务（T034）、游标分页（T035）与索引基准（T036）已落地；其余测试随 M1 任务推进。
