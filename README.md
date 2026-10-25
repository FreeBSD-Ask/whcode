# WHCode

Rust 原生、本地优先的 LLM 编码 Harness。数据落在一个 DuckDB 文件里，事件可追踪，
规格驱动开发的门禁由程序执行。当前只以 Windows 11 amd64（`x86_64-pc-windows-msvc`）
为验收目标；BSD 的代码与文档保留，验收按 ADR-013 延后。

许可证 BSD-2-Clause（见 [LICENSE](./LICENSE)），版权归 FreeBSD 中文社区。
Cargo 版本 `0.1.0`，尚未发布；变更见 [CHANGELOG.md](./CHANGELOG.md)。

## 当前状态

M0—M6、M8 与四端对等收口已实现；M7 SSH 服务端已实现，四 BSD 现场验收延后；M9 的
5.5 小时稳定性长跑尚无合格数据，真实 LLM 供应商的客户端装配未交付。逐项进度、未完成
原因与证据位置见[项目状态快照](./docs/guides/project-status.md)，权威账本是
[tasks.md](./specs/001-whcode-harness/tasks.md) 与
[checklist.md](./specs/001-whcode-harness/checklist.md)。已知限制见
[compatibility.md](./docs/matrices/compatibility.md)。

## 构建与运行

需要 Rust 1.99.1（`rust-toolchain.toml` 已锁定）、Python 3 与 MSVC 工具链；
不使用任何 GNU 工具。

```powershell
# 首次构建前拉取并校验 DuckDB 动态库
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# 环境自检
cargo run -p whcode-bin --release -- doctor

# 交互界面（无参数启动）
cargo run -p whcode-bin --release --

# 命令行
cargo run -p whcode-bin --release -- task list --session <会话 ID>
cargo run -p whcode-bin --release -- doctor --json

# Web（默认 HTTPS，自签证书；旧脚本用 --tls off 回到明文）
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

无参数启动时，若标准输入是真实控制台，进入 TUI：`1`—`6` 切换六个视图，`q` 退出。
在 Git Bash／mintty 这类伪终端里运行，或在控制台窗口中点选进入选择（QuickEdit）模式时，
键盘输入不会送达程序——请用 Windows Terminal 或 conhost 运行，并保持窗口处于输入状态。
完整键位与视图说明见 [tui.md](./docs/guides/tui.md)。

## 界面入口

| 入口 | 说明 |
|---|---|
| TUI | `whcode`（无参数）；六视图只读，写操作用两段确认 |
| CUI | `whcode <子命令>`；`--json` 统一信封与稳定退出码，见 [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web`；默认仅回环，入站 TLS 默认启用，见 [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh`；受限入口，四平台现场验收延后 |
| GUI | `whcode gui`；原生只读界面，不内嵌浏览器引擎 |

## 文档

- [docs/guides/README.md](./docs/guides/README.md)：全部指南的索引（构建、TUI、CUI、Web、
  并发与性能、资源预算、工程过程）
- [specs/001-whcode-harness/](./specs/001-whcode-harness/)：需求基线、任务、验收清单与契约
- [docs/adr/](./docs/adr/)：架构决策记录
- [docs/matrices/](./docs/matrices/)：平台矩阵、决策台账、兼容性与能力对等矩阵
- [CONTRIBUTING.md](./CONTRIBUTING.md)：贡献流程与每 PR 必过的核对清单

## 参与

接受 Issue 与 PR。提交即表示同意以 BSD-2-Clause 授权本项目。项目由 FreeBSD 中文社区
维护，欢迎讨论 BSD 移植、中文本地化与终端体验。
