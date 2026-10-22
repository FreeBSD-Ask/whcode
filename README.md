# WHCode — Rust 原生 LLM 编码 Harness

面向 Windows 11 amd64 的本地优先 LLM 编码智能体底座（保留 BSD 代码兼容，BSD 验收延后，ADR-013）：可追踪、可验证、可恢复，
指标可复现。以《WHCode 技术任务书 v3.0》为需求基线，规格驱动开发（spec-driven），
门禁由程序实施而非提示词。设计、并发与安全以 [philosophy.md](./docs/guides/philosophy.md)
与 [yike-philosophy.md](./docs/guides/yike-philosophy.md) 为方法论基线。

**许可证**：BSD-2-Clause（见 [LICENSE](./LICENSE)）｜ **版权**：FreeBSD 中文社区（FreeBSD Chinese Community）

**版本**：`0.1.0`（M0 基线，**未发布**；版本约定与变更见 [CHANGELOG.md](./CHANGELOG.md)）

## 当前状态

M0（平台可行性与基线冻结）已完成 Windows 侧工程交付，整体判定与逐平台结论见
[artifacts/m0-report.md](./artifacts/m0-report.md)：

**平台范围（ADR-013）**：唯一验收目标是 **Windows 11 amd64**；不考虑任何 32 位平台；
FreeBSD/NetBSD/OpenBSD/DragonFly 全部**延后**（代码与文档保留，不验收，CI 手工触发）。

| 平台 | 范围 | 结论 |
|---|---|---|
| Windows 11 x86_64（msvc） | **在范围内（唯一验收目标）** | **Go**——构建/冒烟/基准/测试全链路实测 |
| FreeBSD / NetBSD / OpenBSD / DragonFly | 延后（ADR-013） | 不作验收结论；恢复须新 ADR 并补齐证据链 |

**M0 判定：Windows 工程基线有条件通过（ADR-016）**：工程侧 Go；V-003 桌面自动化记录已取得并经用户复核接受，V-004 延后至 M7，V-006 预算不冻结。条件未满足时自动退回“未通过”；不得对外写“M0 通过”。详见 `artifacts/m0-report.md` 与 `docs/adr/ADR-016-m0-conditional-closure.md`。

## 核心能力（已交付部分）

- 单文件 DuckDB 持久化（官方预编译动态库，SHA256 校验），单写者服务：有界队列、幂等键、
  批次提交、关键事件确认
- 结构化错误与 `schema_version` 信封（四端共用）；事件体系按关键/内容/展示三级背压
- 最小 TUI 闭环（ratatui/crossterm，真实控制台渲染实测）
- 内置 SSH 客户端原型：公钥认证、受限命令、超时与输出上限、按“意图—执行—登记”协议留痕
- 五平台 CI 定义：Windows 绿路径必过；BSD 手工触发、原生构建作为显式 Go/No-Go 门禁（延后，不作验收结论）

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
| [docs/adr/](./docs/adr/) | 架构决策记录（ADR-001—017 + DB 构建决策） |
| [docs/matrices/](./docs/matrices/) | 平台矩阵、决策台账（D-01—D-14） |
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
