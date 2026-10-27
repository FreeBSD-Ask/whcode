# 5.5 小时长跑请求标记（2026-10-27，第三次）

本文件只用于触发 `long-run.yml` 的一次性运行（`push.paths` 过滤）；内容变更即执行，
删除后不再触发自动长跑。

- 目的：与 2026-10-26 请求完全相同——release 剖面、单 PID、300 s 采样、ADR-025
  包络（`memory_limit='192MB'`／`threads=min(4, 逻辑核数)`）下的 5.5 小时（19 800 s）
  稳定性序列，用于 NFR-204 组合判据与 §11.5 全部指标（Check 116／195）。
- 本次重跑原因：上一轮 run `40315332443` 测试本身 ok（65 样本＋summary 齐备），但
  timeseries.jsonl 因测试二进制 cwd 为 whcode-tests 包根（相对 `WHCODE_STABILITY_OUT`
  落在 `tests/artifacts/`）从未写到 workflow 期望路径，判据分析跳过。
  `long-run.yml` 已改传 `github.workspace` 绝对路径（提交 `d072c46`），本机 8 s 冒烟
  复现并验证了路径行为。
- 用户批准：2026-10-27（“全部批准”）。
- 判据：按 ADR-025 修订记录（2026-10-26 批准）执行——前置（release、样本 ≥ 60、
  单 PID、`event_missing` 全程 0、线程/句柄恒定）＋五要素＋三态结论；verdict 由
  `analyze-nfr204.py` 产出 `nfr204-verdict.json` 并入公开注解；无权判定须按判据
  登记缺口，不得写成通过或失败。
- 成本：一次运行约 5.5 小时 Windows runner 配额；仅在需要新序列时修改本文件。
