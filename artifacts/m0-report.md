# M0 退出报告：平台可行性与基线冻结

**Feature**：`specs/001-whcode-harness`（WHCode — Rust 原生 LLM 编码 Harness）
**日期**：2026-10-21
**依据**：《WHCode 技术任务书 v3.0》§11.1（M0 退出条件）、§9（平台矩阵）、§5.2（测量纪律）；ADR-010（分开验收）、ADR-011（平台承诺修订）、ADR-016（M0 有条件收口）
**判据**：V-001—V-007 全部通过方可判定 M0 通过

## 1. 结论（Go / No-Go）

**M0 整体判定：有条件通过（Windows 工程基线，ADR-016）。**

> **条件**：V-003 的 resize 与 Ctrl-C 人工证据未取得前，不得改写为“M0 正式通过”；
> V-004 延后至 M7；V-006 预算不冻结。条件未满足时自动退回“未通过”。

> **平台范围修订（ADR-013，2026-10-21）**：验收目标收敛为 **Windows 11 amd64**（不考虑任何
> 32 位平台）；FreeBSD/NetBSD/OpenBSD/DragonFly 全部延后，不再作为阻塞项。本报告下表保留
> 修订前的分平台结论作为历史证据；**当前验收口径**见 ADR-013 与 `docs/matrices/platform-matrix.md`。

| 平台 | 结论 | 依据 |
|---|---|---|
| Windows 11（10.0.26300）x86_64 | **Go** | 构建、依赖记录、数据库冒烟四步、TUI 逻辑层、写者原型、基线夹具均取得本机实测证据 |
| FreeBSD x86_64 | **No-Go（环境阻塞）** | 无 FreeBSD 运行环境；官方无 DuckDB 预编译包；仅取得“代码级类型检查（排除 3 个 crate）”证据，不构成运行验证 |
| NetBSD x86_64 | **No-Go（环境阻塞）** | 同上 |
| OpenBSD x86_64 | **不适用（已移出原生目标）** | ADR-011：改由 WHCode 内置 SSH 客户端控制；本报告不主张任何原生构建结论 |
| DragonFly BSD x86_64 | **不适用（已移出原生目标）** | 同上 |

**据此允许进入 M1**（ADR-016），但不得宣称 M0 正式通过。未收口项：

1. V-003 的人工 resize/Ctrl-C 证据未取得（自动冒烟已通过其余四项，见 `docs/guides/tui-manual-smoke.md`）；
2. V-004 按 ADR-011/ADR-013 延后至 M7，客户端原型不构成节点验收；
3. V-006 的候选预算未冻结：优化后吞吐已超候选值，但仍缺分平台与 72 小时数据（§6、§6.1）。

FreeBSD/NetBSD 按 ADR-013 不再作为 M0 阻塞项；其历史结论保留在本报告下方。

## 2. 测量纪律（NFR-201—210、§5.2）

| 字段 | 值 |
|---|---|
| OS | Windows 11，内核 `10.0.26300` |
| CPU | 12th Gen Intel Core i7-1260P，物理 12 核 / 逻辑 16 核 |
| 内存 | 16 930 598 912 字节（约 15.8 GiB） |
| 编译类型 | release（基准）/ debug（测试） |
| Rust 工具链 | `rustc 1.99.1 (58cf8f9c3 2026-10-13)`，host `x86_64-pc-windows-msvc`，LLVM 23.1.1 |
| C/C++ 工具链 | MSVC 14.44.35207、Windows SDK 10.0.26100.0；**不使用任何 GNU 工具链**（D-01 修订） |
| DuckDB | crate `1.10506.1`（MIT）；官方预编译动态库 `1.5.6`；运行期 `SELECT version()` 实测 `v1.5.6` |
| 数据库参数 | 冒烟/写者：基线默认；基准夹具：`memory_limit` 回读 366.2 MiB（`SET 384MB`）、`threads = 4` |
| 数据规模 | 基准：每次 20 000 条内容事件 + 200 条关键事件；批次 500 行、窗口 3 600 000 ms（仅行数阈值触发）；重复 3 次 |
| 采样窗口 | 吞吐为端到端墙钟；延迟为逐条提交墙钟；微秒；nearest-rank 分位数 |
| 脚本 | `ci/platform-matrix/record_deps.py`、`ci/benchmarks/run-baseline.py`、`crates/whcode-bin/src/bench.rs` |
| 生成时间（UTC） | 见 `artifacts/deps-windows.txt` 的 `generated_at_utc` 与 `artifacts/baseline-windows-amd64.json` |

**不得合并平均**：以上全部为 Windows x86_64 单平台数据，不得与任何 BSD 结果混合。

## 3. 证据包

| 证据 | 路径 | 内容 |
|---|---|---|
| 依赖记录（V-001） | `artifacts/deps-windows.txt` | 平台/工具链/编译器/DuckDB/测量纪律字段 + `cargo tree --locked` 全量依赖树 |
| 交叉检查（代码级） | `artifacts/cross-check-windows.json` | FreeBSD/NetBSD 子集类型检查通过；完整工作区预期失败原因 `failed to find tool "cc"`（aws-lc-sys 需目标 C 工具链）；每条结果标注 `is_runtime_evidence: false` |
| 数据库冒烟（V-002） | `crates/whcode-store/src/smoke.rs` 及内联测试 | 创建 / 事务写读 / 关闭重启 / 迁移幂等四步 |
| 写者原型（V-005） | `crates/whcode-store/src/writer.rs` 及内联测试 | 幂等键、批次阈值、关键事件确认、错误传播与批次保留 |
| 基线夹具（V-006 输入） | `crates/whcode-bin/src/bench.rs`、`ci/benchmarks/run-baseline.py`、`tests/stability/baseline_check.rs`、`artifacts/baseline-windows-amd64.json` | 固定夹具 + 实测指标 + 测量纪律 |
| TUI 逻辑层（V-003 部分） | `crates/whcode-tui/src/{lib,state,event,session}.rs` | 尺寸探测、进入/恢复守卫、键盘、resize、退出、异常恢复 |
| SSH 客户端原型（V-004 部分） | `crates/whcode-ssh/src/proto.rs` | 校验→意图→连接→执行→登记；无退出状态判 `unknown`；输出尾部 64 KiB 有界 |
| ADR | `docs/adr/ADR-001.md`—`ADR-011-*.md`、`ADR-DB-BUILD.md` | 批准记录与 DuckDB 预编译决策 |
| 平台矩阵 | `docs/matrices/platform-matrix.md` | 平台角色、版本、证据、冻结状态 |
| 决策台账 | `docs/matrices/decisions.md` | D-01—D-13 |

## 4. 逐项判定（V-001—V-007）

| 编号 | 判定 | 说明 |
|---|---|---|
| V-001 五平台构建与依赖记录 | **部分通过** | Windows 通过（`artifacts/deps-windows.txt`，含四类版本与测量纪律）；FreeBSD/NetBSD 未取得；OpenBSD/DragonFly 不适用（ADR-011） |
| V-002 数据库冒烟四步 | **部分通过** | Windows 通过（4 例测试，含迁移重复应用后 `schema_migrations` 仅一行）；BSD 未取得 |
| V-003 TUI 五步闭环 | **通过（桌面自动化，用户复核接受 2026-10-21）** | 启动、字符计数、resize、Ctrl-C、q 均有记录；方向键/选中项未实现，不计入通过项；非人工逐键；用户 2026-10-21 复核接受。证据：`artifacts/tui-manual-2026-10-21.txt` |
| V-004 SSH 四平台验收 | **延后（ADR-016）** | ADR-011 后判据为“FreeBSD/NetBSD 服务端 + 四平台客户端连通性”；M0 仅客户端原型（9 例，含回环端到端），服务端未实现 |
| V-005 写者四项行为 | **通过（Windows）** | 幂等去重、批次阈值、关键事件确认、错误传播与批次保留均有测试 |
| V-006 基线测量与预算冻结 | **未冻结（延后，ADR-016）** | 夹具与单机样本已产出；**预算未冻结**。优化后吞吐中位数 3 359.1 条/秒已超 NFR-210 候选值，但缺跨平台与 72 小时数据（§6.1、D-06） |
| V-007 ADR-001—016 批准 | **部分通过** | 8 份已批准；ADR-005、ADR-010 为部分批准（未决条件已写明）；另有 ADR-011 与 ADR-DB-BUILD 已批准 |

## 5. 代码质量门禁

| 检查 | 结果 |
|---|---|
| `cargo fmt --all --check` | 通过（退出码 0，无差异） |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过（零告警） |
| `cargo test --workspace` | **107 例通过 / 0 失败** |
| 交叉检查（`--exclude whcode-store --exclude whcode-bin --exclude whcode-ssh`） | FreeBSD、NetBSD 均通过（代码级类型检查，产物 `artifacts/cross-check-windows.json`） |
| 完整工作区交叉检查 | **预期失败**：`aws-lc-sys 0.46.0` 的构建脚本报 `ToolNotFound: failed to find tool "cc"`（来自 `russh → aws-lc-rs`，另有 `libduckdb-sys → ureq → aws-lc-sys`）。这证明 BSD 侧缺的是目标 C 工具链，而非代码不兼容 |

**交叉检查的边界**：`cargo check` 只做类型检查、不链接、不运行，**不构成** V-001/V-002/V-004 的平台通过结论（ADR-010、G-01）。

**待复核**：`memory_limit` 的 OOM 断言正在独立复核，结论未定；复核完成前不得据此改动 §5.4 基线或相关 ADR。

## 6. 性能实测与偏差

产物：`artifacts/baseline-windows-amd64.json`（单机、单次会话、三次重复）。

| 指标 | 实测 | 候选目标 | 判定 |
|---|---|---|---|
| 事件写入吞吐（NFR-210） | 158.1 / 142.4 / 159.6 条/秒，中位数 **158.1** | ≥ 2 000 条/秒 | **未达标（差约 12.6 倍）** |
| 关键事件提交延迟 | p50 **7 034.9 µs**、p95 **12 305.2 µs**、p99 **20 172.6 µs**（600 样本） | 未设候选值 | 仅登记 |
| 数据库文件 | 1 585 152 / 1 847 296 / 1 585 152 字节 | — | 仅登记 |

**直接原因（待正式热点分析确认）**：M0 写者对每条事件在事务内执行“插入 + 回读 `seq`”，且基准中批次窗口未被触发（窗口设为 3 600 000 ms，仅行数阈值生效），因此吞吐受逐条往返支配。这不是“架构结论”，而是可修复的实现形态。

**另一项实测约束**：基准夹具在使用 §5.4 基线 `memory_limit = 192 MB` 且写入约 1 000 条时触发 DuckDB 内存不足，故改用候选上限 384 MB。该现象**必须**在 M1 基准校准中复核（ADR-DB-BUILD 与 §5.4 的 192 MB 基线可能需要修订 ADR）。

**处置纪律**：按任务书要求，先做热点分析，再经 ADR 修订目标或架构；**不得**直接宣布 “2000 条/秒不现实” 而改用新数字，也不得在未分析前下调指标。

### 6.1 优化后复测（2026-10-21）

上述 §6 表格为**优化前**首批实测结果，作为历史证据保留，不替换、不删除。优化内容：写者 `flush_entries` 由“逐条 INSERT + 逐条回读 seq”（每批 2N 次往返）改为“1 次分块 IN 查询已存在键 + 分块多行 VALUES 批量插入（64 行/语句）+ 插入前分配连续 seq、不再回读”。

| 指标 | 优化前（保留） | 优化后（384MB 配置） | 优化后（**192MB 基线配置**，当前口径） |
|---|---|---|---|
| 事件写入吞吐（三次样本） | 158.1 / 142.4 / 159.6 条/秒 | 3 359.1 / 2 965.0 / 3 997.2 条/秒 | 7 427.5 / 6 921.2 / 6 242.1 条/秒 |
| 吞吐中位数（NFR-210 口径） | **158.1 条/秒** | **3 359.1 条/秒**（约 21 倍） | **6 921.2 条/秒** |
| 关键事件延迟 p50 | 7 034.9 µs | 6 975.4 µs | 3 942.7 µs |
| 关键事件延迟 p95 | 12 305.2 µs | 14 117.9 µs | 5 586.7 µs |
| 关键事件延迟 p99 | 20 172.6 µs | 20 727.4 µs | 10 994.8 µs |
| 每次落库行数 | 20 200 | 20 200 | 20 200 |
| 数据库文件 | 1 585 152 / 1 847 296 / 1 585 152 字节 | 1 847 296 字节（三次一致） | 1 847 296 字节（三次一致） |

样本量与测量纪律同 §2（延迟 600 样本 = 3 次 × 200）。证据文件：`artifacts/baseline-windows-amd64.json`（优化前）、
`artifacts/baseline-after-opt.json`（384MB）、`artifacts/baseline-192mb.json`（192MB 基线，**当前口径**）；三者均在本地，不入库。

**夹具内存参数已更正**：T027 曾以“192MB 会 OOM”为由把夹具设为 384MB，该断言经独立实验**未能复现**
（192MB 下 2 000 条与 20 000 条全部成功，无 DuckDB 错误），故夹具已改回 §5.4 基线 192MB。三组对照与
取证要点见 [ADR-012](../docs/adr/ADR-012-memory-limit-baseline.md)。

**口径约束**：192MB 与 384MB 两组的差异是单机三样本观测，**不得**据此推断“调低 `memory_limit` 会提速”；
`memory_limit` 只约束 DuckDB 缓冲管理器，**不代表**进程 RSS（§5.2）。

**结论口径**：192MB 基线配置下吞吐中位数 **6 921.2 条/秒**（384MB 配置为 3 359.1），**已超过 NFR-210 候选值 ≥ 2 000 条/秒**；但**预算仍不冻结**——缺跨平台数据与 72 小时时间序列，D-06 的冻结条件未满足。不得据此宣布预算达标或冻结。

## 7. 阻塞项清单

| # | 阻塞项 | 影响判据 | 解除条件 | 类型 |
|---|---|---|---|---|
| B-01 | 无 FreeBSD 运行环境 | V-001/V-002/V-004 | 提供设备或 CI runner，跑 `record-deps.sh` + `smoke::run` 并归档 | 环境 |
| B-02 | 无 NetBSD 运行环境 | 同上 | 同上 | 环境 |
| B-03 | BSD 无 DuckDB 官方预编译包 | V-002 | 以 `pkg`/ports 安装并核对头文件 ABI，或 `bundled-cmake` 源码构建；结论写入 ADR-DB-BUILD | 依赖 |
| B-04 | `aws-lc-sys` 需目标 C 工具链 | 交叉检查（完整工作区） | 在 BSD 上具备 `cc`/CMake，或将 TLS 后端替换为纯 Rust（需 ADR） | 依赖 |
| B-05 | **自动项大部分已通过**：`script/tui_manual_smoke.py` 在真实控制台注入按键，验证启动首帧、键盘计数（a/b→2）、q 退出（退出码 0）、强杀后控制台可读四项；`script/tui_smoke.py` 另验管道 stdin 优雅退出。**resize 需人工**（本机 conhost 拒绝调整缓冲，错误 87；合成尺寸事件不触发重绘），**Ctrl-C 未自动验证** | V-003 | 人工仅需：真实终端拖拽改变窗口，确认尺寸行实时更新且无花屏；另可按 Ctrl-C 确认退出与回显恢复（自动证据见 `artifacts/tui-manual-smoke/<时间戳>/summary.json`、`artifacts/tui-smoke/<时间戳>/summary.json`） | 证据 |
| B-06 | SSH 服务端未实现 | V-004 | M7/T096—T099 交付 | 里程碑 |
| B-07 | 吞吐主项已解除（优化后中位数 3 359.1 条/秒 > 候选值 2 000）；冻结条件仍缺（跨平台 + 72 小时） | V-006、D-06 | 补齐分平台基线数据与 72 小时时间序列后，由 T028 依 D-06 冻结或修订预算 | 性能 |
| B-08 | 24/72 小时稳定性测试未实现 | §11.5 | T116 落地 | 里程碑 |

## 8. 建议的下一步

1. **不要**直接进入 M1 大规模实现；先就 B-01/B-02 与 B-07 做出明确裁定：是补环境，还是正式缩小平台承诺（需 ADR）。
2. 若接受“Windows 先行”：把 B-07 作为 M1 的首要性能任务（写者批次与 `seq` 分配策略），并在 M1 基准中复测；同时把 192 MB `memory_limit` 基线一并校准。
3. B-05 成本最低、价值最高（一次人工冒烟），建议立即补齐。

## 9. 未主张事项（明确否定）

- 未主张任何 BSD 平台的构建或运行通过。
- 未主张性能预算已冻结或达标。
- 未主张 Web（M6）、SSH 服务端（M7）、运行时（M8）、稳定性（M9）已具备能力。
- 未主张 TUI 六视图、CUI 命令树、三端对等契约测试已交付。
