# ADR-033：稳定性长跑窗口改为 5.5 小时（GitHub CI 可承载）

- **状态**：**已批准（2026-10-24，用户指令）**
- **来源**：T116、NFR-204、spec §11.5、SC-020；GitHub 托管 runner 单 job 上限 6 小时
- **责任人**：性能与稳定性负责人
- **退出条件**：GitHub 托管 runner 时长上限变化、出现长时泄漏证据，或发布要求恢复更长窗口时，
  须新 ADR 修订本决策

## 决策

1. **稳定性窗口改为 5.5 小时（19 800 秒）**，采样间隔 300 秒（约 66 个样本）。原“24 小时
   （候选发布 72 小时）”不再作为 CI 验收门槛。
2. **CI 入口**：`.github/workflows/stability-long.yml`（`stability-long-run`，仅手动
   `workflow_dispatch`），Windows 托管 runner，`timeout-minutes: 355`（5 h 55 min）：
   330 min 长跑 + 准备（检出、门禁、DuckDB、仅编译长跑夹具）≤ 25 min + 产物上传，
   均在 6 h（360 min）硬上限内；产物 `timeseries.jsonl` 与 `run.log` 作为 artifact 归档。
   准备阶段用 `cargo test -p whcode-tests --test stability_run --no-run` 只编译夹具所需
   依赖（不含 GUI／发布壳层），避免冷构建挤占长跑预算。
3. 5.5 小时序列用于 NFR-204 的 RSS 斜率（仍按 ≤ 0.5 MB/h 判定）与 §11.5 规定指标；
   **不得再表述为“24/72 小时已验证”**。
4. 24/72 小时深检保留为**可选的 self-hosted／本机**运行，不再阻塞 CI；如需恢复为验收门槛，
   必须新 ADR 并重新定义证据。

## 理由（正负和解）

- **正面**：GitHub 托管 CI 可直接执行（≤ 6 小时硬上限），无需自托管；5.5 小时留有余量，
  跨多个采样周期，可观察趋势。
- **负面**：窗口缩短会降低低频泄漏与长尾问题的检出灵敏度；5.5 小时不能等价 24 小时，
  更不能等价 72 小时。承担者：性能与稳定性负责人，以及对“稳定”表述有期待的读者——
  文档必须显式写明窗口与限制。
- **和解理由**：把“能在 CI 重复执行的长跑”落地，比维持“本地 24/72 小时永远未跑”的
  名义要求更有证据价值；长时深检作为可选补充而非门槛。

## 影响

- `spec.md`：NFR-204、§11.5 验收场景、SC-020 的窗口改为 5.5 小时；历史 24/72 小时表述
  保留在历史记录中并标注由本 ADR 修订。
- `tasks.md` T116 与 `checklist.md` Check 116 的判据改为 5.5 小时报告。
- `tests/stability/run_stability.sh` 默认窗口改为 5.5 小时（采样间隔 300 秒）；
  `test_stability_run.rs` 未显式给出小时数时默认 19 800 秒。
- `docs/matrices/compatibility.md`、`README.md`、`tests/stability/README.md` 同步窗口与
  “不得冒充 24/72 小时”的限制。
