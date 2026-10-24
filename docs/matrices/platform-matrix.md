# 平台矩阵（M0，2026-10-21）

本文件记录 tasks.md T028 要求的平台矩阵与性能预算冻结状态。矩阵只登记**已取得证据**的结论；未验证能力标注为“未取得”或“阻塞”，不以推断代替实测（宪法原则 VII、ADR-010、G-01）。

承诺边界：平台承诺由“精确 OS 版本 + CPU 架构 + 目标三元组”共同限定；表中未列出的版本不在承诺范围内，也不得以“Windows”或“BSD”作无限范围承诺（任务书 §9.1）。

## 平台矩阵

**范围（ADR-013，2026-10-21 用户决策；2026-10-24 两次修订 BSD CI）**：唯一验收目标是
**Windows 11 amd64**；不考虑任何 32 位平台；FreeBSD/NetBSD/OpenBSD/DragonFly 全部**延后**
（保留代码与文档，不进入验收结论）。四个 BSD 的构建与测试都在 GitHub CI 的单一矩阵
工作流 `.github/workflows/platform-matrix.yml` 中随 `main` 自动运行；矩阵设置
`fail-fast: false` 并行执行，单平台失败不取消其他平台（仅作平台证据）。版本口径为各平台
**最新 release**（FreeBSD `15`、NetBSD `11`、OpenBSD `8` 前缀自动取最新点版本；DragonFly
精确 `6.4.2`）；**`-current` 未覆盖**——上游 vmactions action 只提供 release 表、无快照输入
（ADR-013 修订记录），不得以 release 结果冒充 current 证据。工具链低于 MSRV 的发行版按
环境限制显式 SKIP，不伪造通过。下表“延后”行的状态仅作历史记录，**不得**引用为通过。

| 平台 | 精确版本 | 架构 | 目标三元组 | 范围 | 构建状态 | 运行验证状态 | 证据引用 | 备注 |
|---|---|---|---|---|---|---|---|---|
| Windows | Windows 11（10.0.26300） | x86_64 | `x86_64-pc-windows-msvc` | **在范围内（唯一验收目标）** | 通过（本机实测） | 通过（数据库冒烟 4 步、引擎版本探测与工作区测试） | `artifacts/deps-windows.txt`；`docs/adr/ADR-DB-BUILD.md` | rustc 1.99.1（LLVM 23.1.1）、MSVC 工具集 14.44.35207、Windows SDK 10.0.26100.0；不使用任何 GNU 工具链；DuckDB 引擎 `v1.5.6` |
| FreeBSD | 未固定 | x86_64 | `x86_64-unknown-freebsd` | **延后（ADR-013，不验收）** | 历史：代码级交叉检查通过（排除 3 个 crate） | 未取得（延后，不再作为阻塞项） | `docs/adr/ADR-013-platform-scope-windows-only.md`；`docs/adr/ADR-DB-BUILD.md` | 恢复时须新 ADR 并补齐 D-14 要求的证据链 |
| NetBSD | 未固定 | x86_64 | `x86_64-unknown-netbsd` | **延后（ADR-013，不验收）** | 历史：代码级交叉检查通过（排除 3 个 crate） | 未取得（延后） | 同上 | 同上 |
| OpenBSD | 未固定 | x86_64 | `x86_64-unknown-openbsd` | **延后（ADR-013；本就不作原生目标，见 ADR-011）** | 不适用 | 未取得（延后） | `docs/adr/ADR-011-platform-commitment-revision.md`；`docs/adr/ADR-013-platform-scope-windows-only.md` | 客户端能力属 M7，本轮不验收 |
| DragonFly BSD | 未固定 | x86_64 | `x86_64-unknown-dragonfly` | **延后（ADR-013；本就不作原生目标，见 ADR-011）** | 不适用 | 未取得（延后） | 同上 | 同上 |

**32 位平台**：明确不在范围内（ADR-013）。不提供 `i686` 目标、不测量、不在 CI 中构建；
任何 32 位结果都不得作为承诺依据。

### 行内证据说明

- Windows 的运行验证来自 2026-10-21 本机 `cargo test --workspace`（90 例通过、0 失败），其中 `whcode-store` 含 5 例（冒烟四步、引擎版本探测与缺失父目录错误路径）与 10 例写者原型；依赖记录由 `python .github/scripts/platform-matrix/record_deps.py` 产出为 `artifacts/deps-windows.txt`。
- FreeBSD 与 NetBSD 的“构建状态”仅指**代码级交叉检查**：`cargo check` 不链接，也不运行目标平台二进制。它不覆盖 DuckDB 原生库、TLS 依赖、SSH、终端与运行时 payload，因此**不能**作为 V-002 的通过证据（ADR-010、G-01）。
- 内置 SSH 客户端的四平台连通性冒烟**尚未执行**：`crates/whcode-ssh/src/proto.rs` 的测试为回环测试，只能证明客户端路径可运行，不构成 Windows、FreeBSD、NetBSD、OpenBSD 或 DragonFly 任一平台的节点证据（FR-611、V-004）。
- `whcode-store` 与 `whcode-ssh` 无法参与交叉检查：前者经 `libduckdb-sys` 的构建依赖链引入 `aws-lc-sys`，后者经 `russh` 引入 `aws-lc-rs`，均需要目标平台的 C 工具链（CMake、编译器与汇编器）。这是**阻塞项**，不是代码缺陷，也不构成任何平台结论。

## 性能预算冻结状态

**结论：已冻结（2026-10-23，ADR-025；范围＝Windows 11 amd64）。**

- 冻结口径：预算数值沿用 spec 候选值（NFR-201—210）；**只在 Windows 11 amd64 上作为发布门槛**；
  BSD 无运行证据，按 ADR-013 不参与，标“未验证”，不与他平台合并或平均。
- 已达标（有归档实测）：**NFR-208** 概览查询 p95 6 071 µs ≤ 50 ms（`artifacts/overview-perf-m2.json`，
  2026-10-23 冻结后复跑；报告 `budget.frozen=true`）；
  **NFR-210** 写入吞吐 6 921 条/秒 ≥ 2 000（192 MB 当前口径，`artifacts/baseline-192mb.json`）。
- **T115 测量战役（2026-10-24）补齐七项**（`artifacts/nfr-budget-report.json` +
  `tests/stability/test_nfr_budget.rs`，方法与样本数见报告 discipline 字段）：
  NFR-201 空载 RSS 中位 23.12 MB ≤ 35；NFR-202 单会话活跃 RSS 中位 27.33 MB ≤ 80；
  NFR-203 切换 20 次最大增量 0.32 MB ≤ 10；NFR-205 TUI 空闲 CPU 0.0% ≤ 0.5%；
  NFR-206 限速流式（25 chunks/s）CPU 0.682% ≤ 8%（满喂容量 ≈ 1.5M chunks/s/核为余量佐证）；
  NFR-207 切换 p50 7.1 / p95 12.9 / p99 13.1 ms ≤ 200；NFR-209 单页 p95 **19.8 ms** ≤ 30
  （优化前同夹具 p95 40.8 ms 超标 → 热点定位 EXPLAIN 证实 SEQ_SCAN+TOP_N 多列 top-k，
  两阶段分页优化后达标——同夹具前后对比见 tasks.md T115 记录）。
- 保持候选（未取得合规测量）：**NFR-204 缺 5.5 小时序列**（T116 阻塞；窗口按 ADR-033）。
  上述项不得以“未复现失败”代替测量，发布前补齐或经 ADR 修订。
- 依据与责任：`docs/adr/ADR-025-performance-budget-freeze.md`；性能与存储负责人。

历史缺口（冻结前状态，保留供复盘）：

- **跨平台数据缺失**：FreeBSD/NetBSD 无运行环境，OpenBSD/DragonFly 不作为原生目标；ADR-013 已将其移出验收范围。
- **长时数据缺失**：没有 72 小时 RSS 时间序列与斜率（NFR-204、§11.5）。
- **测量纪律字段**：基线报告已含 OS、硬件、编译类型、工具链、数据库版本、数据规模、采样窗口与脚本；
  `.github/scripts/benchmarks/` 已建立并产出归档。

**首批基线实测（2026-10-21，Windows x86_64，仅供冻结决策参考，不构成冻结）**：

- 产物：`artifacts/baseline-windows-amd64.json`（由 `python .github/scripts/benchmarks/run-baseline.py` 生成）。
- 吞吐：三次重复分别为 158.1 / 142.4 / 159.6 条/秒，中位数 **158.1 条/秒**。
- 关键事件提交延迟（微秒，nearest-rank，600 样本）：p50 **7 034.9**、p95 **12 305.2**、p99 **20 172.6**。
- 与候选值的关系：**显著低于 NFR-210 的 ≥2 000 条/秒**。按任务书要求，此处只登记偏差，不判定达标；修订预算或做优化都必须先有热点分析并经 ADR 记录（D-06、V-006）。
- 已识别的直接原因（待正式分析确认）：M0 写者对每条事件在事务内做“插入 + 回读 `seq`”，且批次窗口未被触发（仅行数阈值），因此吞吐受逐条往返支配。

**优化后复测（2026-10-21，Windows x86_64，同为单机样本）**：

- 优化内容：写者改为“1 次分块 IN 查询（256 键/次）已存在键 + 分块多行 `VALUES` 参数化插入（64 行/语句）+ 插入前分配连续 `seq`、**不再回读**”（原为每条 1 次插入 + 1 次回读，即 2N 次往返）。
- 产物：`artifacts/baseline-after-opt.json`（384MB 配置）与 `artifacts/baseline-192mb.json`（192MB 基线配置；**当前口径**）。
- 吞吐（384MB 配置）：3 359.1 / 2 965.0 / 3 997.2 条/秒，中位数 **3 359.1**（较优化前 158.1 提升约 21 倍）。
- 吞吐（**192MB 基线配置**，夹具更正后）：7 427.5 / 6 921.2 / 6 242.1 条/秒，中位数 **6 921.2 条/秒**。
- 关键事件提交延迟：192MB 组 p50 **3 942.7** / p95 **5 586.7** / p99 **10 994.8 µs**（600 样本）。
- 与候选值的关系：**已超过 NFR-210 候选值 ≥2 000 条/秒**。当时预算未冻结（缺分平台与 72 小时数据）；
  2026-10-23 由 ADR-025 在 Windows 11 amd64 上冻结，BSD 按 ADR-013 不参与。
- 口径约束：192MB 与 384MB 两组的差异是单机三样本观测，**不得**推断“调低 `memory_limit` 会提速”；`memory_limit` 只约束 DuckDB 缓冲管理器，不代表进程 RSS（ADR-012、§5.2）。
- 注意：冻结只覆盖 Windows 11 amd64（ADR-025），不得把本次提升当作跨平台结论（G-01、ADR-010）。

冻结条件与达成状态（ADR-025）：

1. **固定夹具**：10 万事件会话与 1 万任务数据集、固定阶段分布与固定操作序列（T053、T027）——**已满足**；
2. **分平台报告**：范围内平台独立报告分位数（p50/p95/p99），不合并平均；延后平台（ADR-013）豁免，恢复时补齐——**按此豁免口径满足**；
3. **测量纪律字段齐备**：OS、硬件、编译类型、工具链、DuckDB 版本、数据库参数、数据规模、采样窗口、脚本随报告归档——**已满足**（`artifacts/baseline-*.json` 的 `measurement_discipline`）；
4. **5.5 小时斜率（ADR-033）**：RSS 斜率、CPU、队列深度、提交与查询延迟时间序列（NFR-204、§11.5）——**未满足**（T116 阻塞，故 NFR-204 保持候选）；
5. **偏差记录**：基线与候选预算的任何偏差经 ADR 或修订记录正式登记（V-006、SC-021）——**已满足**（ADR-025 登记 158→3 359→6 921 条/秒的演进与差异）。

冻结后口径：NFR-208／210 作为 Windows 发布门槛；NFR-201—203、205—207、209 仍只能以“候选”引用，
补齐测量或经 ADR 修订后方可提升为门槛；不得以单机样本推断跨平台性能（G-01、ADR-010）。

## 交叉编译不等于运行验证

纪律：交叉检查只做类型检查，不链接、不运行目标平台二进制。通过仅表示“代码级兼容”，**不能**证明 DuckDB 原生库、TLS、SSH、终端或运行时 payload 在该平台可构建、可加载或可运行（ADR-010、G-01）。任何平台结论必须来自该平台的真实构建与运行证据；无环境时登记为阻塞。

实际可执行（2026-10-21 本机复核通过；排除需要目标 C 工具链的 crate）：

```
cargo check --workspace --exclude whcode-store --exclude whcode-bin --exclude whcode-ssh --target x86_64-unknown-freebsd
cargo check --workspace --exclude whcode-store --exclude whcode-bin --exclude whcode-ssh --target x86_64-unknown-netbsd
```

不可执行（预期阻塞，不产生平台结论）：

```
cargo check -p whcode-store --target x86_64-unknown-freebsd
cargo check -p whcode-ssh --target x86_64-unknown-freebsd
```

原因：`whcode-store` 的 `libduckdb-sys` 构建依赖链包含 `aws-lc-sys`，`whcode-ssh` 的 `russh` 依赖 `aws-lc-rs`；两者均需目标平台的 C 工具链（CMake、编译器、汇编器），本机不具备 BSD 交叉 C 工具链。命令失败属**环境阻塞**，不得据此判定代码不可移植，也不得据此判定原生依赖不可用。

OpenBSD 与 DragonFly 已移出原生安装目标（ADR-011），不对其执行原生构建或交叉运行验收；其客户端证据须在真实节点上取得，归属 M7。

## 复核与更新纪律

- 每次依赖升级、工具链升级或平台承诺变更，须同 PR 更新本矩阵与 `ADR-DB-BUILD.md` 的版本/哈希记录（§15 PR 清单）。
- 新增平台或版本前必须先有实测证据；不得先写入矩阵再补证据。
- 平台限制必须可查询（NFR-107），并与 `docs/matrices/decisions.md` 的 D-01、D-06、D-13 保持一致。
