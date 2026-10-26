# ADR-042：DuckDB 2.0.0 连接线程开销与 GUI 线程预算

- **状态**：已批准（2026-10-26，方案 A；FR-712／ADR-027 预算值不变）
- **日期**：2026-10-25
- **责任**：资源与 GUI 负责人
- **相关**：FR-712、NFR-201、NFR-211、ADR-025、ADR-027、ADR-034、ADR-035

## 1. 事实与证据

- 2026-10-24 的 GUI 基线（DuckDB 1.5.6）为空闲 24 线程、241 句柄
  （`artifacts/gui-run-2026-10-24/summary.json`）。
- 2026-10-25 用 release 构建重复采样：GUI 空闲 **88 线程、297—330 句柄、RSS 84.5—99.7 MB**；
  私有内存 62.1 MB、空闲 CPU 0.0%、真灰比例 ≤2%。
- 对照实验：DuckDB 库可打开时 88 线程；库不可打开时 21 线程（差 +67）；
  与 `db.threads = 1／2／4` 无关；直接用 DuckDB 2.0.0 单连接 `open` 后即约 80 线程。
- 因此线程增量来自 DuckDB 2.0.0 引擎的线程池／后台线程，不是 GUI 界面改动，
  也不是本项目新增的常驻线程。
- 同一批证据中，NFR-201 主进程空载 RSS 中位 35.69 MB（≤36 MB）通过；
  Web 冒烟 5/5 通过。严格 1200×800 口径与 CI 证据尚未取得。

## 2. 决策（提议）

1. **不掩盖、不改代码凑数**：不通过减少 GUI 功能、关闭数据库连接或在探针中排除线程来
   让读数“达标”。
2. **预算不静默放宽**：FR-712 的 24 线程／256 句柄仍为现行门槛；在用户批准本 ADR 前，
   GUI 资源判据保持**未通过**。
3. **优先验证可配置性**：先测量 DuckDB 2.0.0 是否有受支持的线程数／后台线程开关
   （配置项、连接参数或编译特性），并给出同夹具前后对比；只有在不可配置时才考虑修订预算。
4. **备选处置**（按优先级）：
   - A. 通过 DuckDB 配置把空闲连接的后台线程降到预算内，GUI 仍按需打开连接；
   - B. 修订 FR-712 的线程／句柄预算，附威胁模型、代价、承担者与复审触发；
   - C. 维持预算并接受 GUI 资源判据未通过，发布说明标注该偏差。
5. **正面**：保留 DuckDB 2.0.0 的查询能力与既有优化；**负面**：空闲线程与句柄显著高于
   1.5.6 基线；**承担者**：资源负责人与 GUI 用户；**和解理由**：先证明是否可配置，
   再决定预算或实现，不以未经验证的“引擎开销不可控”直接放宽门槛。

## 3. 三重时间

- **过去来源**：ADR-034／ADR-035 将 DuckDB 升级到 2.0.0；ADR-027 冻结 GUI 预算时基线是 1.5.6。
- **现在证据**：2026-10-25 的 release 探针与库可打开／不可打开对照（见上）。
- **将来失效条件**：DuckDB 提供受支持的线程配置；或 GUI 改为按需连接／连接池；
  或用户批准修订预算。复审触发：DuckDB 版本变更、GUI 连接策略变更、CI 资源测量上线。

## 4. 复审触发

- 取得 `db.threads` 之外的 DuckDB 线程配置证据时；
- GUI 启动／关闭数据库连接的策略变更时；
- 下一次 GUI 资源预算复核（不晚于 2026-11-25）或 DuckDB 小版本升级时。

## 修订记录（2026-10-26：批准方案 A 并实施）

**效力**：用户批准选**方案 A**（配置收敛）。FR-712 的 24 线程／256 句柄与 ADR-027 预算值
**均不变**；GUI 资源判据的线程项仍未通过（见下），不产生达标声明。

### 可配置性验证（决策第 3 条的先行证据）

用官方 CLI 2.0.0（`local/duckdb-cli-2.0.0`）枚举 `duckdb_settings()` 并逐步设置后数线程
（脚本与输出：`local/duckdb_thread_config_probe.py`、
`local/duckdb-thread-config-probe-2026-10-26.txt`）：

- DuckDB 2.0.0 受支持的线程配置项共 7 项：`threads`（= `worker_threads`）、
  **`async_threads`（默认 64，"tasks like I/O"）**、`external_threads`（默认 1）、
  `pin_threads`、`scheduler_process_partial`、`allocator_background_threads`（默认 false）。
- **原第 1 节“与 `db.threads = 1／2／4` 无关”的结论不完整**：遗漏的旋钮是 `async_threads`。
  16（worker 默认）+ 64（async 默认）≈ 80，正是“单连接 open 后约 80 线程”与 +67 增量的来源。
- CLI 判别：默认池建立后 80 线程；设 `threads=1; SET async_threads=4;
  SET external_threads=0` 后进程线程降为 **5**；三项均为运行期 `SET` 生效。

### 实施（同夹具三处同步）

| 位置 | 改动 |
|---|---|
| `crates/whcode-bin/src/workspace.rs` `open_workspace_db` | 同批 `SET` 增加 `async_threads = db.threads`（与 `threads` 同预算 min(4, 逻辑核数)） |
| `tests/stability/test_stability_run.rs` 长跑夹具 | 同步 `SET async_threads`（与产品装配同口径，ADR-025 包络） |
| `crates/whcode-bin/src/bench.rs` 基准夹具 | 同步 `SET async_threads` 并更新 measurement_discipline 文字 |

`workspace_connection_denies_extension_installs_and_repositories` 增补
`async_threads` 回读断言。`external_threads` 保持默认 1（设 0 被引擎钳位）。

### 同夹具复测（2026-10-26，release，单机单样本不构成冻结）

| 指标 | 改前（2026-10-25 探针） | 改后（本日） | FR-712 预算 | 判定 |
|---|---|---|---|---|
| GUI 空闲线程 | 88 | **28** | ≤ 24 | **未通过（差 4）** |
| GUI 句柄 | 297—330 | **233** | ≤ 256 | 通过 |
| GUI RSS | 84.5—99.7 MB | 77.6—94.9 MB | ≤ 96 MB | 通过（贴近上缘） |
| GUI 私有内存 | 60.8—62.1 MB | 43.6—45.0 MB | ≤ 80 MB | 通过 |
| NFR-210 写吞吐 | 冻结 4 054.6 条/秒 | **4 625.5 条/秒**（3 跑中位，`artifacts/adr042-bench-after-async-threads.json`） | ≥ 2 000 条/秒 | 通过，预算不修订 |

GUI 探针输出：`local/gui-thread-after-adr042-2026-10-26.txt`（两次独立启动）。

### 结论与剩余缺口

1. 方案 A 成立且已实施：线程 −60（88→28），句柄与内存同时改善，吞吐不降反升；
   增量确属 DuckDB 2.0.0 默认 `async_threads`，属配置问题而非引擎不可控。
2. **FR-712 线程判据仍未通过（28 > 24）**。剩余 4 线程 ≈ 引擎 `threads=4` 的 worker 池
   （DB 打不开的 GUI 对照为 21 线程）。收敛到预算内的两条路径：
   - A′：全局 `threads=1`（引擎约 3 线程）——改变 NFR-210 冻结夹具口径，须按 ADR-025
     决策 2 以同夹具前后对比另立修订并复测吞吐，**尚未实施**；
   - P1 进程拆分（`artifacts/gui-memory-analysis-2026-10-26.md` 推荐主路径）：
     DuckDB 移出 GUI 进程，须新 ADR。
3. 本 ADR 第 2 节决策第 2 条继续有效：预算不静默放宽；在 A′ 或 P1 落地前，
   GUI 资源判据的线程项保持未通过。
