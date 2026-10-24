# WHCode（旺皇大模型执行框架）— Rust 原生 LLM 编码 Harness

面向 Windows 11 amd64 的本地优先 LLM 编码智能体底座（保留 BSD 代码兼容，BSD 验收延后，ADR-013）：可追踪、可验证、可恢复，
指标可复现。以《WHCode 技术任务书 v3.0》为需求基线，规格驱动开发（spec-driven），
门禁由程序实施而非提示词。设计、并发与安全以 [philosophy.md](./docs/guides/philosophy.md)
与 [yike-philosophy.md](./docs/guides/yike-philosophy.md) 为方法论基线。

**许可证**：BSD-2-Clause（见 [LICENSE](./LICENSE)）｜ **版权**：FreeBSD 中文社区（FreeBSD Chinese Community）

**版本**：`whcode --version` 与 `doctor` 报告**二进制构建时间**（本地时间、精确到分钟并带
时区偏移，如 `2026-10-23T01:17+08:00`；平台时间命令不可用时回退 UTC `...Z`）；Cargo 包
版本仍为 `0.1.0`（M0 基线，**未发布**；变更见 [CHANGELOG.md](./CHANGELOG.md)）

## 当前状态

**里程碑进度**（权威口径：[tasks.md](./specs/001-whcode-harness/tasks.md)）：

| 里程碑 | 状态 |
|---|---|
| M0 平台可行性与基线冻结 | Windows 工程基线**有条件通过**（ADR-016）；V-004 延后至 M7、V-006 预算不冻结 |
| M1 存储与事件基座、M2 会话与任务服务、M3 LLM 与上下文、M4 SDD 门禁、M5 TUI/CUI、M6 Web | **实现完成**（各里程碑实现记录与测试在库） |
| M7 SSH | **服务端实现完成**（`russh 0.64`，ADR-030；公钥认证、受限入口、单写者执行器，进程内 41 项测试通过）；BSD 与四平台现场验收**延后**（ADR-013） |
| M8 可选执行运行时 | **实现完成（feature 默认关闭）**：Provider 契约、能力等级、检查点、缓存、受控子进程与门控；JS（Boa 受限引擎，ADR-029）、Python worker、Git Bash payload 已接入 |
| M9 优化与发布 | **部分完成**：兼容矩阵、运行时清单、热点优化（T115）、发布包（T118）与三份指南（T119）已交付；24/72 小时稳定性（T116）阻塞 |
| Phase 13 跨里程碑收尾 | **已完成**：D 台账、目录门禁与准则文档、安全复查（T123）、quickstart 全流程（T124）、三方断链审计（T127）均取得证据 |

158 条任务中 156 条完成，2 条未完成：T099（BSD 现场验收证据，ADR-013 延后）、
T116（24/72 小时稳定性阻塞）。原因逐条写在任务行。

**验收口径**：`checklist.md` 的 260 个检查点**全部有复核记录**——**242 通过 / 18 未通过**
（其中 14 项通过记录来自 2026-10-21/22 的用户复核，其余由逐条复核与里程碑批次补录给出）。
未通过项集中在：BSD 平台证据与 M7 现场验收（ADR-013 延后）、72 h 长跑（T116）、无远端
CI（Check 3）、10 万事件内存判据不可满足（Check 47）、进程组零残留（Check 109/201）、
ADR 未决项（Check 134）、Web 挂起与 `prefers-reduced-motion` 运行时证据（Check 220/227/228）、
无 JS 时只读视图不可用（Check 229）。
勾选权归复核人，**不得以任务完成率代替验收结论。**
2026-10-23 的进度复核记录见 `artifacts/progress-review-2026-10-23.md`（本地证据，默认不入库）。

**工程门禁（2026-10-23 实测）**：`cargo fmt --all --check` 无差异、
`cargo clippy --workspace --all-targets -D warnings` 零告警、
`cargo test --locked --workspace` 全绿（47 个测试目标、403 例通过、4 例默认忽略的
实验记录器）、`layout`/`layering` 契约测试、EOL 与 workflow 审计、前端预算与资源铁律均通过。

**平台范围（ADR-013）**：唯一验收目标是 **Windows 11 amd64**；不考虑任何 32 位平台；
FreeBSD/NetBSD/OpenBSD/DragonFly 全部**延后**（代码与文档保留，不验收，CI 手工触发）。

| 平台 | 范围 | 结论 |
|---|---|---|
| Windows 11 x86_64（msvc） | **在范围内（唯一验收目标）** | **Go**——构建/冒烟/基准/测试全链路实测 |
| FreeBSD / NetBSD / OpenBSD / DragonFly | 延后（ADR-013） | 不作验收结论；恢复须新 ADR 并补齐证据链 |

**M0 判定：Windows 工程基线有条件通过（ADR-016）**：工程侧 Go；V-003 桌面自动化记录已取得并经用户复核接受，V-004 延后至 M7，V-006 预算不冻结。条件未满足时自动退回“未通过”；不得对外写“M0 通过”。详见 `artifacts/m0-report.md` 与 `docs/adr/ADR-016-m0-conditional-closure.md`。

## 核心能力（已交付部分）

- **存储与事件基座**：单文件 DuckDB 持久化（官方预编译动态库，SHA256 校验）、14 张表版本化
  schema 与可重复迁移、单写者服务（有界队列/幂等键/批次提交/关键事件确认）、稳定排序游标分页、
  预聚合概览、归档清理/压缩/导入导出/数据库监控
- **应用服务**：会话/任务/事件/上下文/状态/SDD 命令统一经 `whcode-app`；结构化错误与
  `schema_version` 信封四端共用；快照对四端同源；三级事件背压（关键/内容/展示）
- **SDD 门禁状态机**：Constitution、spec/tasks 解析与纠偏重试、门禁强制执行与
  force-advance 审计、双向追溯与断链检出、Analyze 五类缺陷、Converge 四态判定
- **LLM 层**：Provider 语义（能力探测/取消/流式/计量/超时/错误归一化）、四级追踪与双层脱敏、
  传输层进度事件；**真实 HTTP Provider 未接线**（当前为桩实现）
- **TUI**：六视图（会话/任务/事件/上下文/Spec/运行时）接真实数据源、四套主题、
  脏标记零重绘与最小化挂起、终端尺寸变化恢复（FR-617）
- **CUI**：命令树经应用服务执行，`--json` 统一信封与稳定退出码；未接线命令显式失败，不静默降级
- **Web**：`serve --web` 编译期嵌入静态前端，**仅限回环或 RFC1918 私网**且局域网强制认证
  （ADR-023）；i18n（zh-CN/en，含 RTL 标记）；无外部 CDN 与第三方字体
- **可选执行运行时**（feature 默认关闭，FR-704）：能力等级、payload 校验、检查点、
  受控子进程与执行缓存策略
- **SSH**：内置客户端（公钥认证、受限命令、超时与输出上限、known_hosts TOFU 校验、按
  “意图—执行—登记”协议留痕）；M7 服务端已实现（`serve --ssh`，russh + 受限入口 + 单写者执行器，
  ADR-030），四平台现场验收按 ADR-013 延后
- 五平台 CI 定义：Windows 绿路径必过；BSD 手工触发、原生构建作为显式 Go/No-Go 门禁（延后，不作验收结论）

已知限制（NFR-204 的 72 小时稳定性未测、真实 HTTP Provider 未接线、BSD 现场验收延后、
Web 无 JS 只读视图缺失等）逐条列在
[compatibility.md](./docs/matrices/compatibility.md) 第 2 节，随包发布时须一并给出。

## 工程原则（摘要）

- **资源**：内存、CPU、GPU、线程、句柄可测量；新增固定占用需 ADR；禁止无界队列与缓存。
- **内存安全**：普通 Rust 零 `unsafe`（workspace `unsafe_code = "forbid"`）；禁止 `mem::forget`；
  汇编仅按 ADR-015 例外（独立隔离、安全包装、回退与差分测试）。
- **并发**：设计先行，禁止原型先行；至少 3 篇一手文献 + 并发设计档案，验证计划可判定。
- **安全**：默认回环监听、`redacted`、最小权限；不以“最佳安全实践/合规”替代威胁模型与证据。
- **自由**：完整导出、离线核心能力与退出路径必须可用；禁止私有格式锁定、强制账户/联网、暗黑模式。
- **不伪造**：工具与模型输出可追溯；未知标 `unknown`/`[待验证]`；LLM 输出不作唯一证据。
- **反进步叙事**：采用率、星数、主流共识不是正确性证据；结论以测量与复现为准。
- **文档时效**：文档与 ADR 写明假设、适用范围与复审日期；知识会过时。

详见 [performance-and-concurrency.md](./docs/guides/performance-and-concurrency.md)、
[design-principles.md](./docs/guides/design-principles.md) 与
[software-engineering-process.md](./docs/guides/software-engineering-process.md)。

## 快速开始

前置：Rust 工具链锁定 1.99.1（`rust-toolchain.toml` 自动生效）；Python 3（拉取 DuckDB）；
Windows 使用 msvc，不依赖任何 GNU 工具链。

```bash
# 1) 拉取并校验预编译 DuckDB（构建前置，SHA256 校验失败即中止）
python ci/fetch-duckdb.py

# 2) 构建与测试
cargo build --locked --workspace
cargo test --locked --workspace

# 2.1) 质量门禁（提交前必须全绿）
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p whcode-tests --test layering   # 分层 / 平台分支 / 前端不触库
cargo test -p whcode-tests --test layout     # 目录分类
python script/normalize_eol.py               # LF + 无 BOM

# 3) 运行
cargo run -p whcode-bin --release -- doctor          # 环境自检（含引擎版本实测）
cargo run -p whcode-bin --release -- doctor --json
cargo run -p whcode-bin --release -- bench baseline --out artifacts/baseline.json --events 2000
cargo run -p whcode-bin --release --                 # 有控制台时进入 TUI；按 q 退出
```

## 仓库导航

| 位置 | 内容 |
|---|---|
| [docs/guides/philosophy.md](./docs/guides/philosophy.md) | 核心哲学与工程方法论（形式审查/0 信任） |
| [docs/guides/repository-layout.md](./docs/guides/repository-layout.md) | 目录分类原则（由契约测试强制） |
| [docs/adr/](./docs/adr/) | 架构决策记录（ADR-001—026 + DB 构建决策） |
| [docs/matrices/](./docs/matrices/) | 平台矩阵、决策台账（D-01—D-15） |
| [docs/guides/bsd-interface.md](./docs/guides/bsd-interface.md) | BSD 平台接口预留契约 |
| [docs/guides/software-engineering-process.md](./docs/guides/software-engineering-process.md) | 软件工程过程与 G0—G5 门禁 |
| [docs/guides/requirements-engineering.md](./docs/guides/requirements-engineering.md) | 需求工程规范（模板/追溯/变更控制） |
| [docs/guides/design-principles.md](./docs/guides/design-principles.md) | 设计原则与工程哲学 |
| [docs/guides/module-design.md](./docs/guides/module-design.md) | 模块功能设计（crate 级） |
| [docs/guides/ui-design.md](./docs/guides/ui-design.md) | UI 设计规范（主题/墨水屏/i18n） |
| [docs/guides/tui-manual-smoke.md](./docs/guides/tui-manual-smoke.md) | TUI 人工冒烟协议（resize/Ctrl-C） |
| [docs/guides/test-model-sensenova.md](./docs/guides/test-model-sensenova.md) | 测试模型 SenseNova（含图像输入） |
| [docs/guides/performance-and-concurrency.md](./docs/guides/performance-and-concurrency.md) | 性能与并发：设计先行、文献门槛、汇编例外 |
| [docs/guides/yike-philosophy.md](./docs/guides/yike-philosophy.md) | 一刻哲学源流、二十九章命题与工程融入 |
| [engineering-doctrine/](./engineering-doctrine/) | 可移植工程哲学与开发准则（供其他项目复制） |
| [docs/guides/doctrine-application.md](./docs/guides/doctrine-application.md) | 工程准则应用记录（审计/改动/缺口） |
| [docs/guides/pr-checklist.md](./docs/guides/pr-checklist.md) | 每 PR 必过的核对清单 |
| [specs/001-whcode-harness/](./specs/001-whcode-harness/) | 需求基线、任务、验收清单、契约 |

## 贡献

接受 Issue 与 PR，流程见 [CONTRIBUTING.md](./CONTRIBUTING.md)。提交即表示同意以
BSD-2-Clause 授权本项目。

## 中文社区

本项目由 FreeBSD 中文社区维护。欢迎在社区渠道讨论 FreeBSD/NetBSD 移植、中文本地化与
终端体验适配。
