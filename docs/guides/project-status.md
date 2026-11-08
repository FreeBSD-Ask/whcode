# 项目状态快照（2026-11-08）

本页是**带日期的状态快照**，供了解进度用。权威账本始终是
[`tasks.md`](../../specs/001-whcode-harness/tasks.md)（任务勾选）与
[`checklist.md`](../../specs/001-whcode-harness/checklist.md)（验收检查点）。
两者与 `spec.md` 的 FR 三方追溯由 `python .github/scripts/traceability.py` 与
`check_counts.py` 在 CI 中强制；本页数字来自这两个文件在 2026-11-08 的实测统计，
过期即失效，不得引用为验收结论。CI 结论以最新 run 为准，本页不写“当前红／绿”。

## 总量

| 项 | 数量 |
|---|---|
| 功能需求 FR | 146 |
| 任务 | 281（已完成 270，未完成 11） |
| 验收检查点 | 376（已有复核记录 366，missing 10） |

证据态分布（ADR-046，`python .github/scripts/check_counts.py` 实测）：exercised 317、
outcome-supported 49、missing 10。missing 集中在 Phase 24 的 B1 余项与 B3—B5
（Check 347—350、354—358）与 ADR-062 的 TUI/GUI RTL 镜像（Check 374）。
并行会话在途（截至本快照）：ADR-058 于 2026-11-06 转已批准（附条件），条件 M2—M5／
M9—M11 已落地、M7 用例 1—6 已落地；T251 勾选与 Check 347 取证**仍未回填**（缺三指标
归档与新一轮 CI 取证）。ADR-060 Phase 1（T264—T275）已全收口；proposal-v3 Phase 2 的
P3.4 迁移前备份（T280，ADR-063）与 P7.4 工具目录（T281，ADR-045 修订）已交付。
回填后须重跑上述脚本并更新本表，不得沿用旧数字。

## 四端能力面

`docs/matrices/feature-parity.md` 为受校验产物（来源 `whcode-app::surface::SURFACES`），
共 **58 条**能力面（48 Command＋10 View）；状态符号为 已实现、部分、预留，
每端另有 1—2 行“不提供”（该端按理由码排除）：

| 端 | 已实现 | 部分 | 预留 | 不提供 |
|---|---|---|---|---|
| CUI | 33 | 12 | 11 | 2 |
| TUI | 27 | 19 | 11 | 1 |
| GUI | 27 | 29 | 1 | 1 |
| Web | 31 | 14 | 11 | 2 |

## 里程碑

| 里程碑 | 状态 |
|---|---|
| M0 平台可行性与基线冻结 | Windows 工程基线有条件通过（ADR-016）；V-004 延后至 M7 |
| M1 存储与事件基座、M2 会话与任务、M3 LLM 与上下文、M4 SDD 门禁、M5 TUI/CUI、M6 Web | 实现完成，测试在库 |
| M7 SSH | 服务端实现完成（russh 0.64.1，ADR-030）；FreeBSD/NetBSD SSH 回环验收 GO（客户端＋服务端四项，run `41481740766`）；V-004 外部节点延后至 M7；NetBSD/OpenBSD 定位为 SSH 远程开发目标（ADR-013 修订 2026-11-03） |
| M8 可选执行运行时 | 实现完成，feature 默认关闭 |
| M9 优化与发布 | 已交付：兼容矩阵、运行时清单、热点优化（T115）、发布包与指南；稳定性（NFR-204）以 24h 深检收口通过（T116/Check 116/195，2026-11-02） |
| Phase 13 跨里程碑收尾 | 已完成（安全复查、quickstart 全流程、追溯审计） |
| 四端对等（FR-619—625） | 已完成（ADR-032）；58 条能力登记表（2026-11-08 实测）为单一事实源 |
| Phase 24 ZCode 全量体验对齐（ADR-057） | 进行中：立项任务 T244—T263 共 20 项、完成 10 项（T244—T250、T252—T254）；其后治理与工程批次 T264—T275（ADR-060 Phase 1）、T276—T278、T280—T281（P3.4 备份、P7.4 工具目录）已全部转勾；余项为 T251（WS 逐块，ADR-058 已批准附条件，缺三指标归档）、B3—B5（T255—T263）与 T279（TUI/GUI RTL 镜像，ADR-062） |

## 未完成项与原因

- **BSD 验收（2026-11-03 已收口一批）**：Check 22/25/96/99/195/336 与 T099/T240 已转勾（run `41481740766` 硬门禁首次全绿，证据落库 `artifacts/ssh-acceptance/` 与 `artifacts/store-acceptance/`）。ADR-013 修订（2026-10-31）设 FreeBSD 为完整平台证据通道（硬门禁）；修订（2026-11-03，用户指令）定 **NetBSD/OpenBSD/DragonFly 仅支持 SSH 远程开发**（被 Windows/FreeBSD 内置 SSH 客户端连入的目标，不要求原生工具链/引擎；NetBSD 原生 SSH GO 维持为证据，NetBSD 原生 store 豁免——上游 DuckDB 2.0.0 依赖 glibc `features.h`；OpenBSD/DragonFly 节点证据走真实连入取证，DragonFly 同日追加同待遇）。**仍开放**：V-004 外部节点连通性（M7）；性能分位数未取得（perf 通道仅 Windows）。**2026-11-06 观察**：FreeBSD 通道在 run `42143683382` 出现 `store_gate = NO-GO`（DuckDB 源码构建报 `platform.hpp:22 'features.h' file not found`），与 2026-11-03 run `41481740766` 的 GO 不一致，**根因未归因**，不得读作代码回归；CI 结论以最新 run 为准，细节见 `docs/matrices/compatibility.md`。
- **稳定性（NFR-204，已收口）**：T116/Check 116/195。5.5h CI 窗口对 0.5 MB/h 无分辨力（run 40529349945 无权判定）后按 ADR-025 升级 24h 本机深检——287 样本、多口径一致负斜率（warmup_ols −0.3551、phase_aligned −0.2681、稳态 −1.24）、线程/句柄无持续增长，**verdict=通过**（2026-11-02）；判据增补见 ADR-025 修订记录；证据 `artifacts/nfr204-verdict-24h-2026-11-02.md`。
- **真实 LLM 供应商（已接四端）**：T162—T167 完成。T162—T166 通过 77 项 `http` 测试与
  本地回环端到端（`artifacts/llm-e2e-2026-10-25.md`）；T167 于 2026-10-26 收口——CUI／Web／
  TUI 接线后，GUI 经 ADR-056（2026-11-03 用户批准“所有界面功能对等”）翻转，`llm.run`／
  `llm.cancel` 四端均登记 `Implemented`（`docs/matrices/feature-parity.md`）。
- **内置 TLS 证书库与 Web 证书设置面（已收口）**：T184—T192 与 Check 274—288 全部
  [exercised]。T184—T191 复核通过；Check 284 补齐部分写续传（400 KiB）、明文读取上限、
  TLS 第 97 条连接直接关闭与 HTTP 超限测试，并修复密文暂存跨 tick 清空的缺陷
  （2026-10-26，CI run `40273055639`）；T192 浏览器实机证据（Chrome 信任自签、页面指纹
  三方一致、设置页预览与重新生成、`restart_required`）见
  `artifacts/tls-browser-2026-10-26/tls-browser-evidence.md`。
- **GUI 线程预算**：DuckDB 2.0.0 单连接打开约 80 线程（`async_threads` 默认 64 所致），
  GUI 空闲首批实测 88 线程／297—330 句柄，超过 ADR-027 的 24／256。ADR-042 已于
  2026-10-26 批准方案 A 并实施（`SET async_threads = db.threads`，与 `db.threads` 同预算），
  同夹具复测 88→28 线程、句柄 233、RSS／私有内存与写入吞吐同时改善；**FR-712 线程判据
  仍未通过（28 > 24）**，A′（全局 `threads=1`，须另立修订并复测吞吐）与 P1（DuckDB 移出
  GUI 进程，须新 ADR）均未实施，预算不静默放宽。
- **GUI 空闲线程待查（2026-11-03 观察）**：本机 release 构建、DPI 2.0、客户区 1100×900 下
  空闲线程实测 **28**（CI 托管 runner 同条件实测 17），高于 ADR-027 的「线程 ≤ 24」。
  该数值与 ADR-042 方案 A 实施后的同夹具复测一致（28）；尚未定位残余 4 线程的具体来源
  （可能为高 DPI 渲染路径或本机 16 核的运行时线程池规模差异），**不作为结论**、也不据此
  改预算。复查方式：`python .github/scripts/gui-probe.py --exe target/release/whcode.exe
  --out artifacts/gui-run-<date>`，比较 `resources.threads` 与 `dpi_scale`／核数。
- **ADR 未决项**：无。ADR-058（turn 执行移出 serve 循环线程，WS 逐块增量转发）于
  2026-11-06 经用户「批准开发」转**已批准（附条件）**；条件 M1 复绿（`942dd14`）、
  M2 鉴权前置、M3 计数原子化、M4 busy 语义、M5 有损通道、M9 帧字段、M11 构造器收敛已
  落地并有 6 条测试；**未闭环**：M7 用例 7—11 与三指标归档（`artifacts/turn-ws-relay-*`）。
  ADR-042 已批准（2026-10-26 方案 A），残余缺口是 FR-712 线程判据
  28 > 24（见上）。Check 134 于 2026-11-03 转勾——ADR-005、ADR-010、ADR-012 三份
  “部分批准”经证据收口转正（SSH 回环＋real-sshd 互操作、FreeBSD 源码构建 DuckDB 2.0.0、
  大载荷边界量化＋24h 深检），ADR-001—014 无未决条件；M1 基准校准仍按 ADR-012 原退出
  条件执行（属里程碑工作，非 ADR 未决项）。

## 能力快照

- 存储与事件：单文件 DuckDB（2.0.0 引擎，预编译动态库 + SHA256 校验）、14 表版本化 schema
  与可重复迁移、单写者有界队列、稳定游标分页、归档/压缩/导入导出/监控。
- 应用服务：命令统一经 `whcode-app`，错误信封与快照四端同源，三级事件背压。
- SDD 门禁：Constitution、spec/tasks 解析与纠偏、门禁强制与 force-advance 审计、双向追溯。
- LLM：Provider 语义、四级追踪与脱敏、传输层事件；真实 HTTP 客户端与 CUI／TUI／GUI／Web
  四端调用均已接通（T167＋ADR-056）。
- GUI：七视图（含对话）接真实数据、六套主题（amoled-night／eink-day／eink-night／
  terminal-green／zcode-day／zcode-night，zcode 系 2026-11-06 冻结，默认 `zcode-night`）、
  脏标记零重绘、最小化挂起与恢复；对话与写命令已接线（ADR-056 四端功能对等）；
  turn 后台执行（界面不冻结，Phase 2）。
- CUI：clap 命令树、`--json` 信封、稳定退出码；未接线命令显式失败。
- Web：编译期嵌入前端，默认仅回环且入站 TLS 默认启用（ADR-040，`--tls off` 可显式关闭）；
  证书设置页 `/settings/certs`。
- SSH：内置客户端（公钥认证、受限命令、超时与输出上限、known_hosts TOFU）；服务端受限入口。
- 运行时（默认关闭）：能力等级、payload 校验、检查点与执行缓存；JS/Python/Git Bash 已接入。

## 质量门禁快照

本地提交前至少跑通：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets
-- -D warnings`、`cargo test --locked --workspace`，以及 `layering`、`layout` 两个契约测试与
`check_counts.py`／`traceability.py`／`check-eol.py`／`check-workflows.py`。
测试结论以 GitHub Actions 日志与 artifact 为准（ADR-037），本地运行只作复现与排障。

## CI 工作流

| 工作流 | 触发 | 职责 |
|---|---|---|
| `platform-verification.yml` | push main、PR、手动 | 门禁与测试、平台矩阵（Windows 必过＝唯一验收；FreeBSD 硬门禁证据通道；NetBSD 自动证据＋SSH 远程开发目标；OpenBSD/DragonFly SSH 远程开发目标）、短测量、Windows SSH 回环／TLS 专项／Playwright tablist 验收；性能与桌面型测试由 `workflow_dispatch` 输入触发 |
| `build-release.yml` | push、tag `v*`、手动 | debug 与 release 矩阵构建、打包安装验证、冒烟、SHA256；tag 时更新 GitHub Release |
| `long-run.yml` | 手动，或 push 改动 `artifacts/long-run-request-*.md` | 5.5 小时稳定性长跑（ADR-033，release 剖面、300 s 采样），一次约消耗 5.5 小时 Windows runner 配额；摘要以公开注解公布 |

平台范围与性能预算见 [`docs/matrices/platform-matrix.md`](../matrices/platform-matrix.md)；
已知限制见 [`docs/matrices/compatibility.md`](../matrices/compatibility.md)。
