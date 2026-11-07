# 域 S：资源纪律与性能

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 资源目标 | 无公开的 RSS/线程/句柄预算；有性能基准门槛（时间、堆、伸缩）与内存压力基准（`docs/testing.md`；`benchmarks/`） | 资源占用列为**第一优先序**；NFR-201—211 有具体数值与实测证据（`docs/adr/ADR-025-performance-budget-freeze.md`） | **WHCode 领先**：预算可判定 |
| 实测数值 | 未见发布级 RSS/CPU 数字 | 主进程空载 RSS ≤ 36 MB（实测中位 35.07 MB）、单会话活跃 ≤ 80 MB（27.33）、切换增量 ≤ 10 MB（3.18）、TUI 空闲 CPU ≤ 0.5%（0.0%）、切换 p99 13.1 ms、概览 p95 6.07 ms、历史页 p95 19.8 ms（ADR-025 表） | **WHCode 独有**：数值+样本+artifact |
| 基准套件 | `benchmarks/`：`session-open`、`agent-continuation`、`conversation-fold`、`long-session-browser`、`session-corpus`、`terminal-io`、`user-text`、`active-stream-reconnect` 等，PR 必需 Linux 门禁（`docs/testing.md`） | 自研 `measure-resources.py` 与 `bench baseline` 命令；性能分位数当前仅 Windows 通道（AGENTS.md §1） | **DSH 领先**：基准面更广，且进 PR 门禁 |
| 有界性 | 通过配置与实现自律：spill 溢出、`session-query` 有界读、jobs/terminal 资源回收；未见统一“每会话线程数”约束 | 硬约束：队列/缓冲/缓存必须有容量上限与上限测试；每会话不得新增常驻线程；句柄 RAII＋释放测试（NFR-215，AGENTS.md §4.1） | **WHCode 领先**：约束成文且可测 |
| 空闲行为 | 文档强调“空闲 agent 不唤醒”“任务变更不唤醒 inbox”（域 F/G）；前端未见空闲重绘约束 | 窗口最小化停止绘制、恢复去抖 ≥250 ms、单次全量重绘（FR-617）；GUI 空闲 CPU ≤ 0.2% 单核（ADR-027） | **WHCode 领先**：可判定 |
| 内存上限 | 无统一进程级上限；Node 堆由运行时管理 | `memory_limit` 192 MB 只是 DuckDB 缓冲管理器基线，**明确不等于进程 RSS**（ADR-012/025） | **WHCode 领先**：口径不混淆 |
| 长时稳定性 | 未见 5 小时级长跑门禁 | ADR-033：5.5 小时窗口、300 s 采样、RSS 斜率 ≤ 0.5 MB/h，组合判据（ADR-025 修订） | **WHCode 独有** |
| 泄漏回归 | 覆盖率门禁＋ HMR 安全测试（dispose 后断言清理）（`docs/testing.md`） | 句柄/线程释放测试＋长时斜率；`recover_uncertain` 与检查点上限 | **同构**，WHCode 更强调长时 |
| 前端预算 | 未见 JS/CSS/DOM 预算 | JS ≤ 50 KB gzip、CSS ≤ 20 KB、DOM ≤ 2000，脚本强制（域 M） | **WHCode 领先** |

## 2. DSH 实现解剖

### 2.1 基准是 PR 门禁

`benchmarks/` 按用户路径分组：会话打开、agent 续跑、对话折叠、长会话浏览器、会话语料、
终端 IO、用户文本、活跃流重连。合成输入强制时间、堆与伸缩预算；包内 `.perf.ts` 只作诊断
（`docs/testing.md`）。也就是说“性能退化”会让 PR 变红，而不是等到发版。

### 2.2 内存压力基准独立

`scripts/run-memory-pressure-bench.ts`（`package.json` 的 `test:bench:memory`）单独跑内存压力，
与常规基准分通道；说明作者把“峰值内存”与“吞吐”当作两类问题。

### 2.3 有界性靠机制而非统一预算

DSH 没有“每会话线程/句柄上限”这类成文铁律，但各子系统自带上限：spill 的 `maxInlineTokens`、
session-query 的有界读、jobs 的 owner 授权与回收、terminal 的 owner 绑定。这是**分散式有界**，
每条缝自己负责；代价是没有全局可判定的总量。

### 2.4 覆盖率与 HMR 安全

每个 registry 必须有一个 HMR 安全测试：dispose 贡献 fiber 后断言清理完成（`docs/testing.md`）。
这把“卸载泄漏”变成了必须写的测试，而不是靠 review 发现。

## 3. WHCode 现状核对

- 已实现：NFR-201—211 数值与实测（ADR-025 表：35.07 MB／27.33 MB／3.18 MB／0.0%／13.1 ms 等），
  证据落 `artifacts/`。
- 已实现：资源铁律（有界容器、每会话不加线程、RAII 释放测试、禁止 `mem::forget`）写入
  `AGENTS.md` §4.1 并由 clippy/契约测试部分强制。
- 已实现：`measure-resources.py`（RSS/CPU 达标）、`bench baseline --out`、GUI 空闲预算
  （RSS ≤ 96 MB/线程 ≤ 24/句柄 ≤ 256/CPU ≤ 0.2%，ADR-027）。
- 已实现：ADR-033 稳定性窗口（5.5 h、300 s 采样、斜率 ≤ 0.5 MB/h）。
- 不足：NFR-204 斜率当前“待复测”（首末 0.6987／OLS 0.943 MB/h，按组合判据无权判定，
  ADR-025 表末行）；性能分位数仅 Windows 通道；无前端长会话基准（DSH 有
  `long-session-browser`）。
- 缺失：PR 级性能门禁（WHCode 的 benchmark 不进 CI 阻断）、内存压力基准。

## 4. 差异判定与可借鉴点

1. **基准进 PR 门禁**：建议采纳（择要）。WHCode 已有 `bench baseline`，但只在发布前跑；
   选 2—3 个高频路径（turn 首 token、会话切换、历史页查询）设阈值进 CI，可及早发现退化。
   代价：CI 时间与 runner 噪声（需多轮取中位）。
2. **内存压力基准**：可采纳。WHCode 只有稳态 RSS，缺“峰值/压力下”的观察；DSH 的独立压力
   通道值得抄。
3. **HMR 安全测试 → 释放测试**：WHCode 已有释放测试要求，可进一步要求“每个注册面必须有
   对称注销测试”（域 J 已指出 `ToolRegistry` 无 `unregister`）。
4. **分散有界 → 全局不变量**：不采纳 DSH 的分散式做法；WHCode 的“统一铁律＋上限测试”更适合
   其审计目标，应保持。
5. **不采纳** Node 堆式“交给运行时”：WHCode 的 RSS/句柄/线程三线必须继续显式测量。
6. **长会话浏览器基准**：若 Web 前端继续增长，可借鉴；当前 Web 预算（50 KB JS）已限制规模。

## 5. 待验证项

1. DSH 基准门禁的具体阈值来源与噪声处理（取中位？多次？）：`docs/testing.md` 只说“强制预算”。
   取证：读 `benchmarks/session-open/**` 与 `scripts/run-bench-suite.ts`。
2. DSH 是否有任何进程级内存上限：`BENCHMARK.md` 仅指向 Python SDK 用法。取证：
   `rg -n "max-old-space|memoryLimit|heap" scripts/ benchmarks/`。
3. WHCode `measure-resources.py` 是否进 CI 阻断：`AGENTS.md` §3 列为验证链，但未确认是否
   在 `ci.yml` 中作为失败门禁。取证：`rg -n "measure-resources" .github/workflows/ci.yml`。

## 6. 证据路径清单

- `docs/testing.md`（HMR 安全测试）
- `docs/testing.md`（性能基准门禁）
- `package.json`（`test:bench`、`test:bench:memory`）
- `benchmarks/`（八组基准）
- `docs/adr/ADR-025-performance-budget-freeze.md`（NFR-201 实测）
- `docs/adr/ADR-025-performance-budget-freeze.md`（NFR-204 待复测）
- `docs/adr/ADR-033-stability-window-5h30.md`（5.5 h 窗口）
- `docs/adr/ADR-027-native-gui-framework.md`（GUI 预算）
- `.github/scripts/benchmarks/measure-resources.py`（资源实测）
- `AGENTS.md` §4.1（有界性铁律）
