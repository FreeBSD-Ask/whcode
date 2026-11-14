# gnullvm 迁移证据（2026-11-14）

**范围**：Windows 验收目标三元组由 `x86_64-pc-windows-msvc` 改为
`x86_64-pc-windows-gnullvm`（ADR-073）。本文件只记录**本机可复现命令与输出**；
平台与 CI 结论以 GitHub Actions 日志与 artifact 为准（ADR-037），未取得者显式写“待回填”。

## 环境

| 项 | 值 |
|---|---|
| OS | Windows 11（10.0.26300） |
| 主机 | amd64 |
| rustup 工具链 | `1.100.0-x86_64-pc-windows-gnullvm`；`rustc 1.100.0 (0ef1bc386 2026-11-09)` |
| C 工具链 | llvm-mingw 20261104（UCRT，x86_64 宿主包） |
| llvm-mingw SHA256 | `552191b5754c49a5a20723276a6fc56541ea1922c4c50b7b669022333fc1b40f`（191 037 046 字节） |
| 链接器 | `x86_64-w64-mingw32-clang`（llvm-mingw 驱动，内部用 lld） |
| DuckDB 载荷 | `vendor/duckdb/2.0.0/x86_64-pc-windows-msvc`（官方 MSVC 包，C ABI） |

## 命令与结果

| # | 命令 | 结果 |
|---|---|---|
| 1 | `cargo build --workspace --target x86_64-pc-windows-gnullvm` | EXIT=0（2 分 07 秒；含 GUI／russh／ring／aws-lc-sys） |
| 2 | `cargo build -p whcode-bin`（host=gnullvm，不传 `--target`） | EXIT=0；产物 `target/debug/whcode.exe` ＋ `target/debug/duckdb.dll` |
| 3 | `cargo test --workspace`（host=gnullvm） | EXIT=0；**1205 通过／0 失败／17 忽略**（含 `layering`、`layout` 契约） |
| 4 | `cargo clippy --workspace --all-targets -- -D warnings` | EXIT=0，零告警 |
| 5 | release 构建 → 删 `whcode.exe` → 再构建，比对 SHA256 | 两次一致：`f64f8ebbb30535a803f3bca7360e3e576a805e003207fcda90d6139c631f0f71` |
| 6 | `whcode.exe doctor --json`（仓库外临时目录） | `duckdb_engine_version: v2.0.0`、`duckdb_error: null` |
| 7 | `llvm-readobj --coff-imports target/.../whcode.exe` | 系统 DLL ＋ `api-ms-win-crt-*` ＋ `duckdb.dll`；**无 vcruntime 依赖** |
| 8 | `llvm-readobj --coff-imports duckdb.dll` | `KERNEL32.dll`、`RstrtMgr.DLL`、`WS2_32.dll`（无 vcruntime） |
| 9 | `python .github/scripts/setup-windows-llvm.py --print-env` | 缓存复用；输出 PATH 与 `RUSTUP_TOOLCHAIN` 导出命令 |
| 10 | 固定版工具链 `RUSTUP_TOOLCHAIN=1.100.0-x86_64-pc-windows-gnullvm cargo build -p whcode-platform` | EXIT=0（验证 CI 用的工具链名可用） |

## 排除的歧路（留档防重蹈）

1. `linker = "rust-lld"` ＋ 手动 `-Lnative` 指向 Windows SDK 导入库：链接通过，但 `ring` 的 C
   编译因缺 mingw 头（`assert.h`）失败；且 `[target.*] rustflags` 不作用于 host 构建脚本。
2. 把 `advapi32.lib` 等复制进工具链 `self-contained/`：制造“能过但不可复现”的工具链污染，弃用。
3. 结论：缺的是**头文件与驱动**，不是库；只有 llvm-mingw（或等价的 mingw-w64 sysroot）可用。

## CI 结论（2026-11-15 回填）

| 运行 | 提交 | 作业 | 结论 |
|---|---|---|---|
| `43615687323` | `c9b2755` | platform-windows | success（62 步零失败；含安装 llvm-mingw＋gnullvm、fmt、clippy、全量测试、SSH 回环、TLS、Web、计数与追溯门禁） |
| `43615687323` | `c9b2755` | measurements | success（17 步） |
| `43615687323` | `c9b2755` | platform-windows-arm64 | success（msvc 通道，ADR-043） |
| `43614186664` | `ae23ce5` | platform-windows | failure：ARM64 交叉检查步被 cc-rs 选中 llvm-mingw 的 clang 去编 msvc 目标（`assert.h` 缺失）；`c9b2755` 在该步按 run 摘除 llvm-mingw 后复绿 |

## 发布包结论（2026-11-15，build-release run `43630790885`，提交 `1b588de`）

手动 dispatch 触发，三条 lane 全 success：

| lane | 结论 | 证据 |
|---|---|---|
| package-amd64-debug | success | `deps-windows` artifact（依赖记录） |
| package-amd64-release | success | `whcode-0.1.0-windows-amd64.zip` 33 079 605 字节，sha256 `1c2777094c7d2e0fc5b5af117fc0e732caf6f06b81def7c1ddede95d5e37075e`；安装验证记录 `target_triple = x86_64-pc-windows-gnullvm`（归档 `artifacts/install-verify-windows-amd64.txt`） |
| package-arm64-release | success | `whcode-0.1.0-windows-arm64.zip` 31 119 633 字节，sha256 `a68373135e297a0500931dbe8bd3235927092a5ed23fd63a7b04e9ac9d581a72`；安装验证记录 `target_triple = aarch64-pc-windows-gnullvm`、`machine = ARM64`（归档 `artifacts/install-verify-windows-arm64.txt`） |

首轮 run `43627842215`：构建、打包、安装验证、SHA256 均 success，唯 arm64 冒烟步仍指
`target/release/whcode.exe`（交叉产物在 `target/<三元组>/release/`）而红；`1b588de` 修正路径后复绿。

即：**amd64 与 arm64 的 gnullvm 发布包均已在 CI 出包并完成仓库外安装验证**。

## ARM64 交叉尖峰（2026-11-15，本机 x64 宿主）

| 命令 | 结果 |
|---|---|
| `rustup target add aarch64-pc-windows-gnullvm` | ok（std 组件） |
| `cargo build -p whcode-bin --target aarch64-pc-windows-gnullvm`（`DUCKDB_LIB_DIR` 指 arm64 载荷） | EXIT=0（2 分 29 秒） |
| `llvm-readobj --file-headers` | `Machine: IMAGE_FILE_MACHINE_ARM64 (0xAA64)` |
| `llvm-readobj --coff-imports` | 无 vcruntime／msvcp；`duckdb.dll`（arm64，56 043 304 字节）随构建就位 |
