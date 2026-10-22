## 1. 使用的构建系统

仓库是纯 Rust 项目，使用 **Cargo workspace**（resolver = "3"）作为唯一构建入口，没有 Makefile、Dockerfile 或其他脚本层。所有编译、测试、格式检查、依赖锁定均通过 `cargo` 子命令完成。

- 工作区成员：`crates/whcode-domain`、`whcode-app`、`whcode-llm`、`whcode-store`、`whcode-speckit`、`whcode-tools`、`whcode-runtime`、`whcode-tui`、`whcode-cui`、`whcode-web`、`whcode-ssh`、`whcode-platform`、`whcode-bin` 以及根 `tests` crate，共 14 个成员（见根 `Cargo.toml`）。
- 工具链锁定：`rust-toolchain.toml` 固定 channel 为 `stable`；注释说明 M0 基线将替换为精确版本 `1.99.0`，当前未写死版本号以避免额外下载（V-001 / T021）。
- 共享元数据：`[workspace.package]` 统一声明 `version = "0.1.0"`、`edition = "2024"`、`rust-version = "1.99"`、`publish = false`。
- 共享依赖：`[workspace.dependencies]` 仅声明 `serde`、`serde_json`、`thiserror`，并明确约定“可选运行时（JS/Python/Shell）的依赖不在此声明”，保留在各自 crate 的 feature 之后，默认构建不引入（FR-704、NFR-102）。
- Lint 策略：`[workspace.lints.rust]` 禁止 unsafe 代码（`forbid`），`[workspace.lints.clippy]` 对 `dbg_macro`、`todo` 告警。

## 2. 关键文件

| 文件 | 作用 |
|---|---|
| `Cargo.toml` | workspace 定义、共享 package/metadata/dependencies/lints |
| `rust-toolchain.toml` | 锁定 stable 工具链通道 |
| `rustfmt.toml` | 代码风格基线 |
| `ci/platform-matrix/windows.yml` | GitHub Actions 构建 + 测试 + clippy + `cargo tree` 产物上传 |
| `ci/platform-matrix/freebsd.yml` | FreeBSD 平台骨架 |
| `ci/platform-matrix/openbsd.yml` | OpenBSD 平台骨架 |
| `ci/platform-matrix/netbsd.yml` | NetBSD 平台骨架 |
| `ci/platform-matrix/dragonfly.yml` | DragonFly BSD 平台骨架 |
| `specs/001-whcode-harness/tasks.md` | 任务编号（T002/T003/T005/T009/T020–T028、V-001/V-002）引用来源 |
| `docs/adr/ADR-003.md`、`ADR-010.md` | 架构决策（依赖方向、平台差异记录纪律） |

## 3. 架构与约定

### 构建流程
CI 中每个平台 job 执行相同四步流水线：
1. `rustup show` — 打印已锁定的工具链。
2. `cargo build --locked --workspace` — 按 `Cargo.lock` 构建整个 workspace。
3. `cargo test --locked --workspace` — 运行全部测试。
4. `cargo fmt --all --check` + `cargo clippy --locked --workspace --all-targets -- -D warnings` — 格式与静态检查以 warning-as-error 模式执行。

完成后用 `cargo tree --locked --workspace > deps-<platform>.txt` 生成依赖快照并通过 `actions/upload-artifact@v4` 归档（V-001）。

### 平台矩阵
`ci/platform-matrix/*.yml` 为各平台的 GitHub Actions 工作流骨架，目前 Windows 实现完整，BSD 系列（FreeBSD/OpenBSD/NetBSD/DragonFly）为占位骨架。README 中写明：“构建、测试、格式/静态检查与依赖记录步骤可直接执行；数据库/TUI/SSH 冒烟为 TODO，随 T022—T025 落地。”

### 版本与发布
- `workspace.package.publish = false`：当前阶段不发布到 crates.io。
- 版本号集中维护于 `Cargo.toml` 的 `[workspace.package]`，所有成员 crate 继承。
- 工具链与依赖图均要求 `--locked`，禁止 CI 上解包任意依赖版本。

### 目录结构约定
- 业务 crate 位于 `crates/whcode-*`，遵循 `domain → app → 适配器（llm/store/tui/web/ssh/...）` 的依赖方向（根 `Cargo.toml` 注释引用 ADR-003）。
- `tests` crate 作为集成/契约/稳定性等测试集合的 Cargo 包根。

## 4. 观察到的约定与规则

- **必须使用 `--locked` 执行 cargo**：CI 三步全部带 `--locked`，由 `Cargo.lock` 约束依赖图，避免非确定性构建。
- **unsafe 代码被禁止**：`[workspace.lints.rust] unsafe_code = "forbid"` 在编译期强制拒绝。
- **clippy 警告即失败**：`-- -D warnings` 把 warn 提升为 error。
- **工具链 channel 固定在 root `rust-toolchain.toml`**，不在 CI 脚本中再次指定，由 rustup 自动解析。
- **Windows 目标区分 msvc 与 gnu**：Windows yml 注释明确指出 runner host 为 `x86_64-pc-windows-msvc`，与本机 GNU 环境不同，且“两者结论不得互相代替”，差异需记入 M0 校验记录（ADR-010、报告纪律）。
- **可选运行时依赖不得进入 workspace.dependencies**：根 `Cargo.toml` 显式约定 JS/Python/Shell 相关依赖留在各自 crate 的 feature gate 之后，默认构建不引入（FR-704、NFR-102）。
- **构建产物归档**：每次 CI 都会产出 `deps-<platform>.txt` 作为 V-001 依赖基线。
- **尚未实现的步骤**：数据库冒烟（T022）、TUI 最小闭环（T024）、性能报告（T027）、平台矩阵预算冻结归档（T028）仍标记为 TODO。

当前仓库没有 Dockerfile、Makefile、Release 脚本或版本自动化流水线；构建与发布仍处于 M0 基线阶段。