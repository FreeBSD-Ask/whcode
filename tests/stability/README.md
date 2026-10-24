# tests/stability

稳定性与基准测试（tasks.md T008、T027、T116；任务书 §11.5、§5.2 测量纪律）。

## 当前包含

- `baseline_check.rs`：基线基准夹具的**正确性**检查（T027、V-006）。固定数据规模
  （500 条/次 × 3 次重复 + 每次 200 个关键事件延迟样本）下验证：
  - 写入后事件行数等于吞吐样本与延迟样本之和；
  - 同一幂等键重复提交不增加行数（V-106）；
  - 报告分位数单调（p50 ≤ p95 ≤ p99）且样本数正确；
  - 运行期 DuckDB 引擎版本非空，报告含全部测量纪律字段。
- 夹具实现：`crates/whcode-bin/src/bench.rs`（`whcode bench baseline` 子命令）。
  本目录的测试通过 `#[path]` 引入**同一份源码**，不复制第二套逻辑。
- 编排脚本：`.github/scripts/benchmarks/run-baseline.py`，产出
  `artifacts/baseline-<系统>-<体系结构>.json`（本机为 `baseline-windows-amd64.json`）。
- `test_process_cleanup.rs`：进程树终止后**零残留**验证（Check 201、FR-409、§10.2）。
  启动真实的 `cmd → ping` 进程树，经 `whcode_platform::terminate_tree` 终止后逐个断言
  父、子 PID 消失。平台边界：非 Windows 的 `child_pids`/`process_exists` 返回结构化
  “延后”错误，用例显式跳过并打印原因，不产生通过结论。
- `test_residency_memory.rs`：空闲会话卸载后**内存可观测下降**（Check 221、FR-614）。
  装载 128 个会话 × 240 KiB 事件缓冲（占用统计约 30 MiB），卸载后断言占用归零、
  工作集（`whcode_platform::current_rss_bytes`）下降 ≥ 8 MiB，唤醒不预分配缓冲。
  平台边界：非 Windows 的 RSS 读取返回结构化错误，此时只跑占用统计断言。
- `test_failure_modes.rs`：四类稳定性故障（Check 206、§11.5）——LLM 请求失败、
  断网（临时端口 + 补拉无重复无缺失）、只读库写入失败（零半完成状态）、磁盘不足
  （`WhError::io` 分类 `StorageFull`/Windows 112）。真实满盘写入需设置
  `WHCODE_DISK_FULL_DIR` 指向已耗尽卷；未设置时用例打印“未执行”，不伪造通过。
- `test_stability_run.rs`：T116 长跑采集器——**单进程**持续写入/查询/切换/清理，
  按间隔自采样 RSS、累计 CPU 与单核占比、线程数、句柄数、数据库与 WAL 大小、
  事件缺失计数、failed 任务数、提交/查询/切换延迟，逐行追加 JSONL（中断不丢已采
  样本）。写者队列深度与最老队列项年龄为 `null` 并附原因：夹具直连 `Connection`，
  不经 `WriterService`；不得以 0 冒充“队列为空”。
- `run_stability.sh`：长跑入口，创建 `artifacts/stability-<N>h-<时间戳>/`，启动上述
  采集器并写 `run.log`；`STABILITY_SMOKE=1` 为 30 秒冒烟（验证采集链路，不作结论）。

## 如何运行

### GitHub Actions（2026-10-24 起）

`.github/workflows/stability.yml`（`stability-measurements`）每周一与手动触发，运行
`writer_stress`、`memory_limit`、`query_plan`、`nfr_budget --ignored`（NFR-203/206）与
`stability_run` 的 30 秒冒烟，并把日志与 JSON 产物作为 artifact 归档。

**5.5 小时长跑在 CI 内**（ADR-033）：`.github/workflows/stability-long.yml`
（`stability-long-run`，仅手动触发，`timeout-minutes: 355`）运行 19 800 秒、300 秒采样，
产物归档。24/72 小时深检保留为可选的 self-hosted／本机运行，不再作为 CI 门槛；
**5.5 h 不得表述为 24/72 小时已验证**。

### 本机

```powershell
# 仅运行基线夹具正确性检查（不判定性能达标）
cargo test -p whcode-tests --test baseline_check

# 零残留验证（Windows 上会真实启停进程树）
cargo test -p whcode-tests --test process_cleanup

# 卸载内存观测（需要 Windows 工作集读取）
cargo test -p whcode-tests --test residency_memory -- --nocapture

# 四类故障情形（真实满盘需先设置 WHCODE_DISK_FULL_DIR）
cargo test -p whcode-tests --test failure_modes -- --nocapture

# 长跑采集器（默认 5.5 小时，ADR-033；冒烟用 STABILITY_SMOKE=1）
./tests/stability/run_stability.sh
STABILITY_SMOKE=1 ./tests/stability/run_stability.sh

# 本机基线测量（release 构建；结果写入 artifacts/baseline-windows-amd64.json）
python .github/scripts/benchmarks/run-baseline.py
```

## 尚未实现（明确标记）

- **5.5 小时结论尚未取得**：采集器与 CI 长跑入口已交付（T116、ADR-033），数据须跑满后
  由 `timeseries.jsonl` 的 summary 行判定；24/72 小时深检为可选 self-hosted／本机运行；
- **写者队列深度/最老队列项年龄未取得**：长跑夹具直连 `Connection`，不经真实事件
  写者服务；该两项在 JSONL 中为 `null` 并附原因，待真实写者接线后补齐；
- 慢客户端、重启与迁移中断场景仍需并入长跑编排；
- 真实满盘写入需专用环境（`WHCODE_DISK_FULL_DIR`）。

## 纪律（§11.5、R-12）

- 先完成常规功能测试，再运行 5.5 小时稳定性测试（ADR-033）；24/72 小时深检可选；
- 稳定性报告必须记录：RSS 时间序列、CPU、线程数、文件大小、WAL 大小、写者队列深度、
  最老队列项年龄、提交延迟、查询延迟、事件缺失计数、失败任务数；
- 必须覆盖：慢客户端、LLM 请求失败、断网、磁盘空间不足、数据库写入失败、重启、终止子进程、
  迁移中断；
- 报告须区分产品缺陷、环境限制与测试工具偏差；**不得以“没有崩溃”代替一致性验证**；
- 测量记录必须含 OS、硬件、编译类型、工具链、DuckDB 版本、数据库参数、数据规模、采样窗口、
  脚本；不同平台分别报告，不混平均；分位数不得以平均值代替；
- 基线数值是**单机单次样本**，不构成预算冻结；预算冻结或修订由 T028 依据固定夹具与
  分平台证据决定。
