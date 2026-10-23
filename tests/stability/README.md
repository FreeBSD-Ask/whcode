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
- 编排脚本：`ci/benchmarks/run-baseline.py`，产出
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

## 如何运行

```powershell
# 仅运行基线夹具正确性检查（不判定性能达标）
cargo test -p whcode-tests --test baseline_check

# 零残留验证（Windows 上会真实启停进程树）
cargo test -p whcode-tests --test process_cleanup

# 卸载内存观测（需要 Windows 工作集读取）
cargo test -p whcode-tests --test residency_memory -- --nocapture

# 四类故障情形（真实满盘需先设置 WHCODE_DISK_FULL_DIR）
cargo test -p whcode-tests --test failure_modes -- --nocapture

# 本机基线测量（release 构建；结果写入 artifacts/baseline-windows-amd64.json）
python ci/benchmarks/run-baseline.py
```

## 尚未实现（明确标记）

- **24 小时稳定性测试未实现**；
- **候选发布版本的 72 小时压力测试未实现**；
- 本目录已有基线夹具正确性检查、零残留验证、卸载内存观测与四类故障情形用例；
  仍缺长时间 RSS/CPU 时间序列、慢客户端、重启与迁移中断等场景（T116 交付）；
  真实满盘写入需专用环境（`WHCODE_DISK_FULL_DIR`）。

## 纪律（§11.5、R-12）

- 先完成常规功能测试，再运行至少 24 小时稳定性测试；候选发布版本运行 72 小时压力测试；
- 稳定性报告必须记录：RSS 时间序列、CPU、线程数、文件大小、WAL 大小、写者队列深度、
  最老队列项年龄、提交延迟、查询延迟、事件缺失计数、失败任务数；
- 必须覆盖：慢客户端、LLM 请求失败、断网、磁盘空间不足、数据库写入失败、重启、终止子进程、
  迁移中断；
- 报告须区分产品缺陷、环境限制与测试工具偏差；**不得以“没有崩溃”代替一致性验证**；
- 测量记录必须含 OS、硬件、编译类型、工具链、DuckDB 版本、数据库参数、数据规模、采样窗口、
  脚本；不同平台分别报告，不混平均；分位数不得以平均值代替；
- 基线数值是**单机单次样本**，不构成预算冻结；预算冻结或修订由 T028 依据固定夹具与
  分平台证据决定。
