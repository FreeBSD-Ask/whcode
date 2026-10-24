# tests/fixtures

跨 crate 测试夹具与数据集生成器（tasks.md T008、T053、T081）。

## 已建立

- `gen_dataset.rs`——10 万事件会话与 1 万任务（固定阶段/状态分布）数据集生成器（T053）。
  作为模块被 `test_session_switch`、`test_overview_perf`、`test_extreme_scale`、
  `test_nfr_budget` 等经 `#[path = "../fixtures/gen_dataset.rs"]` 引入；cargo 只自动发现
  `tests/*.rs` 与 `tests/*/main.rs`，因此 fixtures 目录必须显式引入。
- `sdd/`——SDD 门禁违规产物夹具（T081），供 `test_sdd_gates` 使用：
  - `constitution.md`——合法 Constitution 基线；
  - `spec-with-clarify.md`——含未解决澄清标记的规格；
  - `tasks-clean.md`——无违规任务表（对照组）；
  - `tasks-violating.md`——含循环依赖与孤儿任务的违规任务表。

## 约定

- 夹具必须可由代码或脚本重建，禁止手工维护二进制大文件进入版本库；
- 生成器必须确定性：同参数两次生成的数据集内容一致（V-101/V-102 的可复现前提）；
- 崩溃注入夹具的注入时点说明见 `tests/crash/README.md`（T114、Check 196/207）。
