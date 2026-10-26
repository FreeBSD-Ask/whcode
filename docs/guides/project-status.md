# 项目状态快照（2026-10-25）

本页是**带日期的状态快照**，供了解进度用。权威账本始终是
[`tasks.md`](../../specs/001-whcode-harness/tasks.md)（任务勾选）与
[`checklist.md`](../../specs/001-whcode-harness/checklist.md)（验收检查点）。
两者与 `spec.md` 的 FR 三方追溯由 `python .github/scripts/traceability.py` 与
`check_counts.py` 在 CI 中强制；本页数字来自这两个文件的实测统计，过期即失效，
不得引用为验收结论。

## 总量

| 项 | 数量 |
|---|---|
| 功能需求 FR | 95 |
| 任务 | 203（已完成 186，未完成 17） |
| 验收检查点 | 299（已通过 264，未通过 35） |

## 里程碑

| 里程碑 | 状态 |
|---|---|
| M0 平台可行性与基线冻结 | Windows 工程基线有条件通过（ADR-016）；V-004 延后至 M7 |
| M1 存储与事件基座、M2 会话与任务、M3 LLM 与上下文、M4 SDD 门禁、M5 TUI/CUI、M6 Web | 实现完成，测试在库 |
| M7 SSH | 服务端实现完成（russh 0.64.1，ADR-030）；四 BSD 现场验收延后（ADR-013） |
| M8 可选执行运行时 | 实现完成，feature 默认关闭 |
| M9 优化与发布 | 部分完成：兼容矩阵、运行时清单、热点优化（T115）、发布包与指南已交付；5.5 小时稳定性（T116）尚无合格数据 |
| Phase 13 跨里程碑收尾 | 已完成（安全复查、quickstart 全流程、追溯审计） |
| 四端对等（FR-619—625） | 已完成（ADR-032）；54 条能力登记表为单一事实源 |

## 未完成项与原因

- **BSD 现场验收**：T099，以及 Check 20/21/22/25/96/99/109/201 的 BSD 侧。ADR-013 将四 BSD
  全部延后；CI 的 FreeBSD/NetBSD 任务只产出平台证据，不进入验收结论。2026-10-26 的
  FreeBSD run `40267545791` 已给出 `ssh_loopback_gate = GO`（客户端＋服务端回环）；
  DuckDB 2.0 源码构建在 `cmake --install` 阶段失败（静态库体积/安装规则），已改为只构建
  `libduckdb.so` 并手工复制共享库与头文件，NetBSD 的 rustup PATH 回退也已修复，待下一轮
  CI 判定。所有 BSD 判据以公开 check-run 注解与 `*-progress.txt` 公布，无需登录即可复核。
- **5.5 小时稳定性（NFR-204）**：T116 与 Check 116/195。2026-10-25 的 CI 长跑（run
  40154046118）已取得 65 个样本，但 RSS 斜率 0.6987／0.943 MB/h 高于 0.5 MB/h 预算；
  2026-10-25 已把长跑夹具改用正式 `EventWriter` 并输出队列深度／最老项年龄（目标表
  `m0_events`，`events` 表仍直连，已显式标注）；分析见
  `artifacts/nfr204-disposition-2026-10-25.md`（含判据分辨力不足的论证与 ADR-025 修订提议）。
- **真实 LLM 供应商**：T162—T167。T162—T166 已落地并通过 77 项 `http` 测试与本地回环端到端
  （`artifacts/llm-e2e-2026-10-25.md`）；T167 仅 CUI／Web 接线，TUI／GUI 仍为 `Reserved`
  （ADR-032），四端全翻转未完成。
- **内置 TLS 证书库与 Web 证书设置面**：T184—T192 与 Check 274—288。T184—T191 已复核通过，
  Check 274—283／285—288 有证据（CI run 40225169689 success）；284 已补部分写续传、
  明文读取上限、TLS 第 97 条连接直接关闭与 HTTP 超限测试，并修复密文暂存跨 tick 清空的
  真实缺陷，待本次推送的 CI 回填；T192 的浏览器实机信任/设置页交互仍未取得。
- **GUI 线程预算**：DuckDB 2.0.0 单连接打开约 80 线程，GUI 空闲实测 88 线程／297—330 句柄，
  超过 ADR-027 的 24／256；处置见 `docs/adr/ADR-042-duckdb-connection-thread-budget.md`
  （提议，待用户批准）。
- **ADR 未决项**：Check 134 依赖 BSD 范围恢复后把三份部分批准 ADR 转正。

## 能力快照

- 存储与事件：单文件 DuckDB（2.0.0 引擎，预编译动态库 + SHA256 校验）、14 表版本化 schema
  与可重复迁移、单写者有界队列、稳定游标分页、归档/压缩/导入导出/监控。
- 应用服务：命令统一经 `whcode-app`，错误信封与快照四端同源，三级事件背压。
- SDD 门禁：Constitution、spec/tasks 解析与纠偏、门禁强制与 force-advance 审计、双向追溯。
- LLM：Provider 语义、四级追踪与脱敏、传输层事件；真实 HTTP 客户端与 CUI／Web 调用已接通，
  TUI／GUI 仍为预留（见上）。
- TUI：六视图接真实数据、四主题、脏标记零重绘、最小化挂起与恢复。
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
| `ci.yml` | push main、PR、手动 | 门禁与测试、五平台矩阵（Windows 必过；BSD 只作平台证据）、短测量、Windows SSH 回环／TLS 专项／Playwright tablist 验收；性能与桌面型测试由 `workflow_dispatch` 输入触发 |
| `build-release.yml` | push、tag `v*`、手动 | debug 与 release 矩阵构建、打包安装验证、冒烟、SHA256；tag 时更新 GitHub Release |
| `long-run.yml` | 手动，或 push 改动 `artifacts/long-run-request-*.md` | 5.5 小时稳定性长跑（ADR-033，release 剖面、300 s 采样），一次约消耗 5.5 小时 Windows runner 配额；摘要以公开注解公布 |

平台范围与性能预算见 [`docs/matrices/platform-matrix.md`](../matrices/platform-matrix.md)；
已知限制见 [`docs/matrices/compatibility.md`](../matrices/compatibility.md)。
