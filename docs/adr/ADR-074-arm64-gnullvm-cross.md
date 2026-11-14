# ADR-074：ARM64 交付目标迁移到 aarch64-pc-windows-gnullvm（交叉构建）

- **状态**：已批准（2026-11-15，用户指令“继续解决123”——允许在 ADR-073 遗留的显式缺口上继续）
- **来源**：ADR-073 §2.5 的显式缺口（ARM64 本轮不切，登记为后续修订）；ADR-043（ARM64 为构建交付目标）；ADR-069／ADR-073（工具链 1.100.0 与 amd64 gnullvm 迁移）
- **责任人**：平台矩阵负责人
- **退出条件**：交叉链在 CI 反复不可复现、或上游弃用目标时回退 `aarch64-pc-windows-msvc`

## 1. 问题与边界

ADR-073 把 amd64 验收链换成 LLVM 系列（gnullvm），ARM64 交付链（ADR-043）当时保留 msvc，
留下“同一仓库两套 Windows 工具链”的口径差。本决策把 ARM64 交付目标也换成
`aarch64-pc-windows-gnullvm`，使三条 Windows lane（amd64 debug／amd64 release／arm64 release）
全部落在 LLVM 工具链上。

边界：

- rustup **没有** `aarch64-pc-windows-gnullvm` 的宿主工具链（gnullvm 宿主只有 x86_64），
  所以这是**交叉构建**：windows-11-arm runner 的主机保持 `aarch64-pc-windows-msvc`
  （rust-toolchain.toml 的 channel 1.100.0 自动解析），目标为
  `--target aarch64-pc-windows-gnullvm`（std 由 `rustup target add` 补装）；
- C 编译器用 llvm-mingw 包内的 `aarch64-w64-mingw32-clang`。该可执行文件是 **x64 程序**，
  在 ARM64 runner 上以 Windows x64 模拟运行（Windows 11 ARM64 自带 x64 模拟）；这是
  工具链执行的模拟，**不是**产物的模拟——产物是原生 ARM64，全部运行证据（测试、SSH 回环、
  安装验证）仍在原生 runner 上取得；
- 验收边界不变：ARM64 仍是构建交付目标，不进验收结论（ADR-043／ADR-013）。

## 2. 决策

1. **目标三元组**：`aarch64-pc-windows-gnullvm`（构建交付目标）；`.cargo/config.toml`
   显式 `linker = "aarch64-w64-mingw32-clang"`；`aarch64-pc-windows-msvc` 保留为回退目标；
2. **安装脚本扩展**（ADR-073 脚本通用化）：`--rustup-toolchain`（ARM64 传
   `1.100.0-aarch64-pc-windows-msvc`）与 `--rustup-target`（`aarch64-pc-windows-gnullvm`）；
   同时移除安装命令的 `--no-self-update`——安装被中断时它会留下没有 update-hash 的半装
   工具链，之后每次 `rustc` 调用都重复“syncing channel updates → downloading components”
   （ADR-073 尖峰期实测踩过）；
3. **`--verify` 放行口径改按架构**：`package-windows.py` 的 `can_execute_locally` 由
   “三元组相等”改为“架构标签相等”——ARM64 交付包由 aarch64 主机交叉构建出 gnullvm 目标，
   产物是原生 ARM64，可在同一 runner 上直接运行；拿三元组比较会把这种本可验证的组合误判
   成“交叉不可验”（ADR-013 G-01 的反义：该场景不是交叉编译冒充运行，而是真运行）；
4. **CI 通道**：
   - `platform-verification.yml` 的 windows-arm64 lane：装工具链（msvc 主机＋gnullvm 目标），
     测试／SSH 回环／feature 门控全部带 `--target aarch64-pc-windows-gnullvm`（测试二进制
     在 ARM64 runner 上原生执行）；
   - `build-release.yml` 的 arm64 lane：matrix target 改 gnullvm，发布构建带 `--target`，
     `DUCKDB_LIB_DIR` 用绝对路径指 arm64 载荷（ADR-069 修订口径）；
5. **DuckDB 载荷不变**：官方只发 MSVC 载荷（含 arm64），`PAYLOAD_TRIPLE` 已把
   `aarch64-pc-windows-gnullvm` 映射到 `aarch64-pc-windows-msvc` 载荷（ADR-073 §2.4 同口径）。

## 3. 尖峰证据（2026-11-15 本机 x64 交叉）

| 证据 | 命令（要点） | 结果 |
|---|---|---|
| 交叉构建 | `cargo build -p whcode-bin --target aarch64-pc-windows-gnullvm`（x64 宿主） | EXIT=0（2 分 29 秒；含 russh 0.65.1、aws-lc-sys、libduckdb-sys 链接） |
| 机器类型 | `llvm-readobj --file-headers` | `Machine: IMAGE_FILE_MACHINE_ARM64 (0xAA64)` |
| 运行时依赖 | `llvm-readobj --coff-imports` | 无 vcruntime／msvcp 导入；`duckdb.dll`（arm64 载荷，56 043 304 字节）随构建就位 |
| CI 运行验证 | run `43623479584`（`6202e54`，手动 dispatch） | package-amd64-release／package-arm64-release／package-amd64-debug 全 success（**arm64 该轮仍为 msvc 通道**，gnullvm 交叉通道结论见台账回填） |

排除过的歧路：让 ARM64 lane 使用 x64 gnullvm 宿主工具链（`RUSTUP_TOOLCHAIN=x86_64…gnullvm`）——
会把宿主也换成“在 ARM64 上跑 x64 模拟的 rustc”，连编译器本体都进模拟，证据口径更差；
正确做法是宿主保持原生 aarch64-msvc，只把**目标**换成 gnullvm。

## 4. 正负和解

**正面**：三条 Windows lane 工具链统一为 LLVM；产物无 VC Redist 前提；`--verify` 口径
更准确（按架构）。**负面与承担者**：aarch64 编译器以 x64 模拟运行，构建时间增加、依赖
Windows 11 ARM64 的 x64 模拟层；arm64 交叉构建引入“宿主≠目标”的目录布局（`binary_path`
已按此路由）；两者由平台矩阵负责人承担。**和解理由**：统一性消除“同一仓库两套 Windows
工具链”的口径差，且证据边界（原生运行）不受影响。

## 5. 三重时间

- **过去**：ADR-043 定 ARM64 为构建交付目标（msvc）；ADR-073 把 amd64 切 gnullvm 并显式
  留下 ARM64 缺口；
- **现在**：§3 的本机交叉尖峰与本轮 CI 回填；
- **将来失效条件与复审触发**（绑定命令＋判定）：llvm-mingw 上游弃用 aarch64 交叉（
  `local/toolcache/.../bin/aarch64-w64-mingw32-clang.exe` 缺失即触发）；Windows ARM64
  移除 x64 模拟（CI 安装步骤或构建步红即触发）；rustup 下架
  `aarch64-pc-windows-gnullvm` std（`rustup target list` 无该项即触发）——任一发生
  回退 `aarch64-pc-windows-msvc` 并重开 ADR。

## 6. 迁移面（文件级）

| 文件 | 改动 |
|---|---|
| `.cargo/config.toml` | `[target.aarch64-pc-windows-gnullvm] linker` |
| `.github/scripts/setup-windows-llvm.py` | `--rustup-toolchain`／`--rustup-target`；移除 `--no-self-update` |
| `.github/scripts/package-windows.py` | `SUPPORTED_TARGETS` 增 arm64 gnullvm；`can_execute_locally` 按架构比较 |
| `.github/workflows/platform-verification.yml` | windows-arm64 lane 装工具链＋全步骤 `--target` |
| `.github/workflows/build-release.yml` | arm64 lane matrix target、发布构建 `--target`、绝对 DUCKDB_LIB_DIR |
| `docs/adr/ADR-043`、ADR-013 | 修订记录指向本 ADR |
| `docs/matrices/platform-matrix.md` | ARM64 行三元组更新 |
| `AGENTS.md` | 硬边界第 2 条三元组更新 |
| 台账 | T308—T310、Check 401—403 |

## 7. 回退方案

matrix target 改回 `aarch64-pc-windows-msvc`（或本地 override 直接回 msvc 构建）；
`.cargo/config.toml` 的 `[target.aarch64-pc-windows-gnullvm]` 节删除；脚本与工作流
改动均向后兼容 msvc 目标（安装步骤照常，多余 target std 不影响），回退不需要恢复代码。

## 8. 已知限制与失效条件

- **工具链执行在 x64 模拟下**：`aarch64-w64-mingw32-clang` 为 x64 程序，构建期性能与
  可用性依赖 Windows 11 ARM64 的 x64 模拟；若 CI 构建步报“找不到可执行文件／模拟失败”
  触发复审；
- **交叉目录布局**：产物在 `target/aarch64-pc-windows-gnullvm/release/`，凡按
  `target/release/` 找产物的脚本须走 `binary_path()`（已覆盖），新增脚本不得硬编码路径；
- **测试是“交叉构建＋原生运行”**：报告口径必须写全两者，不得只说“原生测试”；
- build-release 的 arm64 gnullvm 包结论以该工作流实跑为准（本机不冒充 CI 结论，ADR-037）。

## 9. 文献依据

- rustc 平台支持页：`aarch64-pc-windows-gnullvm` 为 Tier 2 std target（无 host tools），
  `x86_64-pc-windows-gnullvm` 是唯一提供 host tools 的 gnullvm 三元组；
- llvm-mingw 项目：单包多目标（x86_64 宿主，含 aarch64-w64-mingw32 交叉驱动）；
- Microsoft Windows on ARM 文档：Windows 11 ARM64 内置 x64 模拟（Prism）。

## 10. 形式审查

- **真实矛盾**：ADR-073 留下的“amd64 已 LLVM、ARM64 还在 MSVC”口径差；不切则两条 Windows
  交付链依赖两种工具链生态，维护与证据口径都不一致；
- **正负和解**：见 §4，模拟成本与承担者显式；
- **0 信任校验**：交叉构建、机器类型、导入表均为本机命令实测；CI 结论以 run 编号引用，
  本机结果不作平台结论（ADR-037）；
- **知难行易**：方向＝用户指令“继续解决123”的缺口 2；目标＝ARM64 交付链统一 LLVM 且
  原生运行证据不变；手段＝交叉＋模拟编译器，是“rustup 无该宿主工具链”约束下的唯一解，
  选型晚于认知。
