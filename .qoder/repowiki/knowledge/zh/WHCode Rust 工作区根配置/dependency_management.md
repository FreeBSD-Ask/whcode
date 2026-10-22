## 1. 使用的系统与工具

- **包管理器**：Cargo / Cargo Workspace。仓库采用单仓多 crate 结构，根 `Cargo.toml` 通过 `[workspace]` 声明 13 个成员 crate 以及 `tests` 包。
- **版本解析器**：`resolver = "3"`，启用 Cargo 2024 edition 的依赖图行为。
- **版本与工具链锁定**：根 `rust-toolchain.toml` 固定 channel 为 `stable`；注释明确说明当前实测值对应 1.99.0，且 M0 基线将以精确版本号替换。
- **无 vendor/私有注册表**：未发现 `.cargo/config.toml`、`vendor/` 目录或 GOPRIVATE 类配置；依赖全部来自 crates.io（由 `Cargo.lock` 管理）。未看到任何自定义 registry 或 `source` 重定向。

## 2. 关键文件

- `Cargo.toml`（根）：workspace 定义、`[workspace.dependencies]` 共享依赖、`[workspace.lints.*]` 全局 lint 规则。
- `Cargo.lock`：锁定所有传递依赖的精确版本。
- `rust-toolchain.toml`：工具链 channel 锁定。
- 各 crate 的 `Cargo.toml`：仅引用 `path = "../whcode-*"` 的内部 crate 和 `*.workspace = true` 的外部依赖。
- `tests/Cargo.toml`：测试包同样走 workspace 继承，并通过 `[[test]]` 显式声明非标准路径下的集成测试目标。

## 3. 架构与约定

### 3.1 共享依赖集中声明
根 `Cargo.toml` 的 `[workspace.dependencies]` 段集中声明了三个外部依赖：
- `serde`（带 `derive` feature）
- `serde_json`
- `thiserror`

各成员 crate 通过 `{ serde.workspace = true }` 等语法引用，确保全仓使用同一语义化版本区间。

### 3.2 内部 crate 通过 `path` 引用
成员 crate 之间一律使用相对 `path = "../whcode-*"` 引用，不发布到 crates.io（根 `[workspace.package] publish = false`），且每个子包的 `version`、`edition`、`rust-version`、`publish` 均通过 `workspace = true` 继承。

### 3.3 可选运行时依赖按 feature 隔离
根注释明确约定：可选运行时（JS/Python/Shell）的依赖**不**放在 `[workspace.dependencies]`，而是留在具体运行时 crate 的 feature 之后，默认构建不引入。`crates/whcode-bin/Cargo.toml` 中已预留 `runtime-js`、`runtime-python`、`runtime-gitbash` 三个空 feature（FR-704、D-07），真正的运行时装配在 M8（tasks.md T103）接入 `whcode-runtime`。

### 3.4 依赖方向约束
根注释与 ADR-003 共同约束依赖方向：`domain → app → 各适配器`，前端不得直接触库。该约束通过 Cargo 的 crate 可见性实现（若前端 crate 不依赖后端 crate，则编译期即失败）。

### 3.5 全局 Lint 作为“可维护性依赖”约束
根 `Cargo.toml` 的 `[workspace.lints.rust] unsafe_code = "forbid"` 禁止所有 crate 使用 `unsafe`；`[workspace.lints.clippy]` 将 `dbg_macro` 与 `todo` 设为 warn。这些 lint 通过各 crate 的 `[lints] workspace = true` 继承生效。

## 4. 约定与约束

- **共享依赖必须集中在根 `[workspace.dependencies]`**：根注释标注来源为 tasks.md T009，成员 crate 统一通过 `.workspace = true` 引用。
- **可选运行时依赖不得出现在共享段**：根注释明确禁止，要求留在运行时 crate 的 feature 之后，以避免默认构建引入（依据 FR-704、NFR-102）。
- **工具链版本以 rust-toolchain.toml 锁定**：tasks.md T002 要求固定；M0 基线需写入精确版本号（T021/V-001）。
- **Windows 平台差异需在 M0 校验记录中显式说明**：任务书 §9 指定首选 `x86_64-pc-windows-msvc`，但本机 host 为 GNU，该差异不得被忽略（rust-toolchain.toml 注释中明确列出 D-01/V-001）。
- **内部 crate 不发布**：`publish = false` 作用于整个 workspace，子 crate 也继承此值。
- **测试包通过 `[[test]]` 显式声明非标准路径测试**：`tests/Cargo.toml` 中的 `contract/test_layering.rs` 不在 cargo 自动发现范围内，需手动声明。

## 5. 观察到的缺失

- 未发现 Cargo 镜像源、私有 registry 或 `CARGO_NET_*` 相关配置。
- 未发现 `cargo-deny`、`cargo-audit`、`cargo-outdated` 等依赖审计/更新工具的配置。
- 未发现 CI 中运行 `cargo update` 或 PR 自动检查依赖变更的流程（CI 目录仅含 platform-matrix YAML，未见依赖管理步骤）。