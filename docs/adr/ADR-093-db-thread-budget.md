# ADR-093：DB 连接线程预算收敛（ADR-042 A′ 路径）

- **状态**：已批准（2026-11-22，用户指令“不计成本最佳答案执行”）
- **编号说明**：正式 ADR-093；与 `local/parity-batch/ADR-093-draft.md`（对话框钩子草案，未批准、不入库）无关，两者主题不同，正式编号以本文件为准。
- **关联**：ADR-042（DuckDB 连接线程开销）、ADR-025（性能预算冻结与夹具纪律）、ADR-027（GUI 资源预算）、FR-712（GUI 预算）、NFR-210（写入吞吐 ≥2000 条/秒）

## 任务认知卡

- **方向**：把 GUI 空闲线程从 28 收敛到 ≤24，满足 FR-712／ADR-027 的硬预算；不改变运行时 worker 并发。
- **目标**：DuckDB 连接的 `threads`／`async_threads` 设为 1；改动后实测 GUI 线程 ≤24、吞吐 ≥2000 条/秒。
- **用户意图**：用户以“不计成本最佳答案执行”授权自主选择最佳方案并推进，不再逐项确认。
- **非目标／边界**：不改 `resources::thread_budget`（runtime worker 仍按 min(4, 核数)）；不放宽 FR-712／ADR-027 预算；不改 DuckDB 引擎版本；不掩盖未达标数据。
- **现有证据**：ADR-042 方案 A 已把 88→28 线程（`async_threads` 配置）；残余 4 线程 ≈ worker 池；本批改动前基线吞吐中位 4123.3 条/秒（`artifacts/adr093-baseline-before.json`）、GUI 空闲 28 线程／251 句柄／81.9 MB（`artifacts/gui-run-2026-11-21/summary.json`）。
- **手段（后置）**：config 新增 DB 专用预算常量 → workspace 与 bench 夹具同步 → release 基准与 GUI 资源探针复测 → 全链门禁。

## 问题

ADR-042 方案 A 通过 `SET async_threads = threads = min(4, 核数)` 把 GUI 空闲线程从 88 降到 28，但 FR-712 的硬预算是 ≤24，仍差 4。ADR-042 结论 2 给出 A′ 路径（全局 `threads=1`）与 P1 路径（DuckDB 移出 GUI 进程，须新 ADR）。P1 改动过大；A′ 在本机 CLI 探针中已证明 `threads=1` 可把引擎线程降到个位数，是最小改动。

## 决策

1. **DB 连接线程预算独立化**：`crates/whcode-bin/src/config.rs` 新增常量 `DB_CONNECTION_THREAD_BUDGET: u32 = 1`，仅用于 DuckDB 连接的 `threads`／`async_threads`；`whcode_app` 配置默认值由 `resources::thread_budget(cores)` 改为该常量。
2. **runtime worker 不变**：`resources::thread_budget`（clamp 1..=4）保持原样，`whcode-runtime::worker` 的并发上限不受影响。这是与 ADR-042 原文“全局 threads=1”的精确化——原文语境是 DuckDB 引擎参数，非 Rust worker 池。
3. **夹具同步**：`bench.rs` 的 `threads_budget` 同步改为 1，使基准与产品装配同口径（ADR-025 决策 2：夹具保真度）；文字说明同步更新。
4. **实测判据**：改动后 release 基准 NFR-210 吞吐 ≥2000 条/秒、GUI 空闲线程 ≤24。任一不达标即不宣称收敛完成，按回退路径处理。
5. **可配置性**：`config.db.threads` 仍可由用户在配置中显式覆盖（透传值），常量只改默认；不强制锁死用户选择。

## 理由与约束

- **为何只改 DB 线程**：残余 4 线程来自 DuckDB worker 池，与 Rust 运行时 worker 无关；全局收敛会无谓降低运行时并发。
- **为何不放宽预算**：ADR-042 决策 2 明确“预算不静默放宽”，且 ADR-025 要求夹具变更前同口径复测。
- **承担者**：资源与 GUI 负责人（实测）、app/bin 维护者（配置与夹具）；用户接受单连接吞吐可能下降（以实测为准）。

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | GUI 线程有望 ≤24（FR-712 达标）；句柄与 RSS 可能进一步下降；运行时并发不变 |
| 负面 | 单连接写入吞吐可能下降；默认配置从 min(4,核数) 变 1，改变行为；需 release 复测 |
| 承担者 | 资源负责人承担实测；用户承担潜在吞吐下降 |
| 和解理由 | 以同夹具前后对比量化代价；吞吐只要 ≥2000 条/秒（冻结预算）即接受；不达标则回退 |

## 三重时间

- **过去来源**：ADR-042 方案 A（88→28）、FR-712／ADR-027 预算、NFR-210 冻结值。
- **现在证据**：改动前吞吐中位 4123.3/s（3 跑）、GUI 28 线程／251 句柄／81.9 MB。
- **已知限制与失效条件**：①单机单样本不构成预算冻结（R-12），达标结论须 CI 复核；②GUI 资源探针为窗口标题与进程采样，实机交互未测；③DuckDB 小版本升级可能改变线程模型，须重审。
- **复审触发（绑定命令＋判定）**：
  1. `whcode bench baseline` 改动后吞吐 < 2000 条/秒 → 回退本 ADR；
  2. GUI 资源探针改动后线程 > 24 → 回退本 ADR 并转 P1 路径（须新 ADR）；
  3. `cargo test -p whcode-platform -p whcode-bin --lib` 任一失败 → 定位并回退；
  4. DuckDB 升级或 ADR-025/027 正文修订 → 重审本 ADR。

## 实测结论（2026-11-22，本机）

- 吞吐中位：改动前 4123.3/s → 改动后 4194.0/s（≥ 2000，NFR-210 通过；差异 1.7% 在
  噪声带内，**不宣称提速**）。证据 `artifacts/adr093-baseline-{before,after}.json`。
- GUI 空闲线程：28 → **22**（≤24，FR-712 通过；ADR-042 遗留缺口闭合）；句柄 251→245、
  RSS 81.9→81.3 MB、空闲 CPU 0.0%。证据 `artifacts/gui-run-2026-11-22/summary.json`。
- 限制：探针为资源专用模式（无截屏/按键注入），像素与键位未取证；单机样本不构成
  预算冻结（R-12），CI 按 ADR-037 待回填。详见 `artifacts/adr093-db-thread-verification-2026-11-22.md`。

## 影响

- 正面：GUI 资源判据线程项首次达标（22 ≤24）；保持零 unsafe 与原生铁律不变；
  runtime worker 并发不变。
- 负面：默认 DB 并发从 4 降 1；基准夹具口径变更需留证据；单机结论待 CI 复核。
- 不做什么：不改 runtime worker；不放宽预算；不替换引擎。
