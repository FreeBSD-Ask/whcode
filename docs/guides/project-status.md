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
  全部延后；CI 的 FreeBSD/NetBSD 任务只产出平台证据（依赖记录、非 DuckDB 包测试），
  不进入验收结论。恢复须新 ADR、真实平台环境与用户批准。
- **5.5 小时稳定性（NFR-204）**：T116 与 Check 116/195。2026-10-25 的 CI 长跑（run
  40154046118）已取得 65 个样本，但 RSS 斜率 0.6987／0.943 MB/h 高于 0.5 MB/h 预算，
  且写者队列深度、最老队列项年龄两项指标未取得；分析见
  `artifacts/nfr204-disposition-2026-10-25.md`（含判据分辨力不足的论证与 ADR-025 修订提议）。
- **真实 LLM 供应商**：T162—T167。变更 spec 已于 2026-10-25 补登记
  （[`specs/002-add-real-llm-provider/`](../../specs/002-add-real-llm-provider/)），
  profile/协议/帧/错误层有实现；ureq 调用点、凭据装配与四端接线未交付，`llm.run` 在四端
  登记表仍为 Reserved（ADR-032）。
- **内置 TLS 证书库与 Web 证书设置面**：T184—T192 与 Check 274—298。代码已合入
  （ADR-040），检查点等待真实浏览器握手与设置页端到端证据。
- **ADR 未决项**：Check 134 依赖 BSD 范围恢复后把三份部分批准 ADR 转正。

## 能力快照

- 存储与事件：单文件 DuckDB（2.0.0 引擎，预编译动态库 + SHA256 校验）、14 表版本化 schema
  与可重复迁移、单写者有界队列、稳定游标分页、归档/压缩/导入导出/监控。
- 应用服务：命令统一经 `whcode-app`，错误信封与快照四端同源，三级事件背压。
- SDD 门禁：Constitution、spec/tasks 解析与纠偏、门禁强制与 force-advance 审计、双向追溯。
- LLM：Provider 语义、四级追踪与脱敏、传输层事件；真实 HTTP 调用未接通（见上）。
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
| `ci.yml` | push main、PR、手动 | 门禁与测试、五平台矩阵（Windows 必过；BSD 只作平台证据）、短测量；性能与桌面型测试由 `workflow_dispatch` 输入触发 |
| `build-release.yml` | push、tag `v*`、手动 | debug 与 release 矩阵构建、打包安装验证、冒烟、SHA256；tag 时更新 GitHub Release |
| `long-run.yml` | 仅手动 | 5.5 小时稳定性长跑（ADR-033），一次约消耗 5.5 小时 Windows runner 配额 |

平台范围与性能预算见 [`docs/matrices/platform-matrix.md`](../matrices/platform-matrix.md)；
已知限制见 [`docs/matrices/compatibility.md`](../matrices/compatibility.md)。
