# 5.5 小时长跑请求标记（2026-10-26）

本文件只用于触发 `long-run.yml` 的一次性运行（`push.paths` 过滤）；内容变更即执行，
删除后不再触发自动长跑。

- 目的：取得 release 剖面、单 PID、300 s 采样的 5.5 小时（19 800 s）稳定性序列，
  用于 NFR-204 RSS 斜率与 §11.5 全部指标（Check 116／195）。
- 触发提交：长跑工作流改为 release 构建并公布公开摘要注解。
- 判据：`timeseries.jsonl` 含 65 个 `sample` 行与 1 个 `summary` 行；
  `writer_queue_depth_peak`／`oldest_queue_item_age_ms` 非 `null`；
  `event_missing` 恒 0；summary 给出 RSS 首末／最小二乘斜率。
- 预算：ADR-025 冻结 NFR-204 ≤ 0.5 MB/h，但 5.5 小时窗口的分辨力争议见
  `artifacts/nfr204-disposition-2026-10-25.md`；本次只取证，判定按 ADR-025
  修订提议（待批准）执行。
- 成本：一次运行约 5.5 小时 Windows runner 配额；仅在需要新序列时修改本文件。
