# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![平台验证](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

简体中文 | [繁體中文](README.zh-TW.md) | [English](README.en.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md) | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | [বাংলা](README.bn.md)

Rust 原生、本地优先的 LLM 编码 Harness：数据落在一个 DuckDB 文件里，事件可追踪，
规格驱动开发的门禁由程序执行。原生 GUI、TUI、CUI、Web、SSH 多前端共享同一套应用服务。

- **本地优先**：单文件 DuckDB 持久化，数据可随时带走；不强制账户与联网。
- **事件可追踪**：工具执行按“意图—执行—登记”留痕；默认追踪级别 `redacted`，凭据不落日志与事件。
- **多前端**：GUI／TUI／CUI／Web／SSH 共享同一应用服务；桌面界面原生渲染，不内嵌浏览器引擎。
- **规格驱动**：需求、任务与验收三方追溯，门禁由测试与脚本强制执行。

## 当前状态

版本 `0.1.0`，尚未发布；变更见 [CHANGELOG.md](./CHANGELOG.md)。

M0—M6、M8 与四端对等收口已实现，M7 SSH 服务端已实现。M9 的 5.5 小时稳定性长跑
尚无合格数据，真实 LLM 供应商的客户端装配未交付。逐项进度、未完成原因与证据位置见
[项目状态快照](./docs/guides/project-status.md)，权威账本是
[tasks.md](./specs/001-whcode-harness/tasks.md) 与
[checklist.md](./specs/001-whcode-harness/checklist.md)，已知限制见
[compatibility.md](./docs/matrices/compatibility.md)。

## 平台支持

| 平台 | 定位 |
|---|---|
| Windows 11 amd64 | **唯一验收目标**（`x86_64-pc-windows-msvc`，ADR-013） |
| Windows ARM64 | 构建交付目标（ADR-043）：CI 结果仅作平台证据，不进验收 |
| FreeBSD | 完整平台证据通道（硬门禁）：CI 随 main 全量构建、测试与门禁，任一失败即红；不作验收结论 |
| NetBSD | CI 随 main 自动运行，仅平台证据 |
| OpenBSD／DragonFly | 手工占位；与 NetBSD 同为 SSH 远程开发目标（作为被连入端，不要求原生工具链） |

性能分位数目前仅取自 Windows 通道。平台口径详见
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) 与
[compatibility.md](./docs/matrices/compatibility.md)。

## 构建与运行

需要 Rust 1.99.1（`rust-toolchain.toml` 已锁定）、MSVC 工具链与 Python 3；
不使用任何 GNU 工具链。

```powershell
# 首次构建前拉取并校验 DuckDB 动态库（SHA256）
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# 环境自检
cargo run -p whcode-bin --release -- doctor

# 界面：Windows 默认 GUI；--tui 终端界面；--cli 命令行
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <会话 ID>

# Web（默认 HTTPS，自签证书；--tls off 回到明文）
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

无参数启动时，Windows 默认打开原生 GUI；加 `--tui` 进入终端界面（`1`—`6` 切换视图，
`q` 退出），加 `--cli` 使用命令行。TUI 在伪终端（Git Bash／mintty）或控制台 QuickEdit
选择模式下收不到键盘输入，请用 Windows Terminal 或 conhost 运行；
完整键位与视图说明见 [tui.md](./docs/guides/tui.md)。

没有可用控制台时（CI、管道、重定向），程序不启动界面，打印提示后以退出码 0 结束。

## 界面入口

| 入口 | 启动 | 说明 |
|---|---|---|
| GUI | `whcode`（Windows 默认）或 `whcode gui` | 原生只读界面，不内嵌浏览器引擎 |
| TUI | `whcode --tui` | 六视图只读，写操作两段确认，见 [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <子命令>` 或直接 `whcode <子命令>` | `--json` 统一信封与稳定退出码，见 [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | 默认仅回环，入站 TLS 默认启用，见 [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | 受限入口，不默认授予系统 Shell；四平台现场验收延后 |

## 文档

- [docs/guides/README.md](./docs/guides/README.md)：全部指南的索引（构建、TUI、CUI、Web、
  并发与性能、资源预算、工程过程）
- [specs/001-whcode-harness/](./specs/001-whcode-harness/)：需求基线、任务、验收清单与接口契约
- [docs/adr/](./docs/adr/)：架构决策记录
- [docs/matrices/](./docs/matrices/)：平台矩阵、决策台账、兼容性与能力对等矩阵
- [docs/guides/project-status.md](./docs/guides/project-status.md)：项目状态快照

## 参与

接受 Issue 与 PR，贡献流程与每 PR 必过的核对清单见 [CONTRIBUTING.md](./CONTRIBUTING.md)。
提交即表示同意以 BSD-2-Clause 授权本项目。项目由 FreeBSD 中文社区维护，
欢迎讨论 BSD 移植、中文本地化与终端体验。

## 安全

请勿通过公开 Issue 报告安全漏洞，报告渠道与安全不变量见 [SECURITY.md](./SECURITY.md)。

## 许可证

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD 中文社区（FreeBSD Chinese Community）。
