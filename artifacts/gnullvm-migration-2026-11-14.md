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

## 未取得（显式缺口）

- gnullvm 的**发布包**（build-release 的 amd64 两条 lane）：只在 tag 或手动 dispatch 时运行，
  本批未触发。本机的 `package-windows.py --verify` 已通过（记录在
  `artifacts/install-verify-windows-amd64.txt`），但按 ADR-037 不作为 CI 交付结论。
