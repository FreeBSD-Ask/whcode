# ADR-073：Windows 验收目标迁移到 x86_64-pc-windows-gnullvm（LLVM 系列工具链）

- **状态**：已批准（2026-11-14，用户指令“能切 llvm 系列的都切，不用 GNU 那套”＋“仓库目标也换 gnullvm”）
- **来源**：用户指令（同上）；ADR-069（工具链 1.100.0）；ADR-047（链接确定性）；ADR-013（平台范围）；D-01（禁 GNU 工具链）
- **责任人**：工具链与平台矩阵负责人
- **退出条件**：gnullvm 通道出现不可修复的回归（链接失败、运行期不兼容、上游目标被弃）时回退 msvc，见“回退方案”与“已知限制与失效条件”

## 1. 问题与边界

D-01 只禁止 **GNU 工具链**（GCC／ld／binutils 一系），而当前验收目标三元组
`x86_64-pc-windows-msvc` 绑定的是 **MSVC 工具链**（cl.exe／link.exe／Windows SDK 导入库）。
用户指令要求：在不用 GNU 系列的前提下，把一切可切到 LLVM 系列的工具链都切过去——即
rustc 后端目标换 `x86_64-pc-windows-gnullvm`（LLVM `lld` 链接、LLVM 自包含 CRT 配套），
C 依赖（`ring`、`aws-lc-sys`、`libduckdb-sys` 的头文件消费方）的编译器换 clang
（llvm-mingw 发行版，LLVM 系列驱动＋mingw-w64 UCRT 头与导入库，不是 GCC）。

边界：

- 验收目标（Windows 11 amd64）**不变**，变的只是该目标的三元组与工具链来源；
- 不得引入任何 GCC／binutils 组件（llvm-mingw 的驱动是 clang，归档器是 llvm-ar，无 GNU 依赖）；
- FreeBSD／NetBSD 通道与 rustup 工具链版本锁定（1.100.0）不受本决策影响。

## 2. 决策

1. **主机与验收目标统一为 `x86_64-pc-windows-gnullvm`**：Windows 开发与 CI 的 Windows
   amd64 通道使用 rustup 工具链 `1.100.0-x86_64-pc-windows-gnullvm`（机器级 override，
   机制与此前 msvc 相同），`cargo build`／`cargo test` 不传 `--target`，产物在
   `target/debug`／`target/release`；
2. **C 依赖工具链固定 llvm-mingw `20261104`（UCRT，x86_64 宿主包）**：zip SHA256
   `552191b5754c49a5a20723276a6fc56541ea1922c4c50b7b669022333fc1b40f`，其 `bin` 目录
   前置于 PATH；链接器显式声明 `x86_64-w64-mingw32-clang`（llvm-mingw 驱动，负责找到
   mingw-w64 UCRT 头／自包含 CRT／全部 Windows 导入库）。rust-mingw 自包含组件只含
   `crt2.o`／`libkernel32.a`，不含 `advapi32`／`ole32` 等，**不足以独立支撑链接**——
   这是尖峰实测的第一阻塞，llvm-mingw 是唯一合规解；
3. **`.cargo/config.toml` 增加 `[target.x86_64-pc-windows-gnullvm]`**：显式 `linker`；
   不设 `/Brepro` 类链接参数——lld 默认确定性输出，两次全新链接 SHA256 一致（实测），
   ADR-047 由此继续满足；
4. **DuckDB 载荷不变**：官方只发布 Windows 的 MSVC 载荷（`vendor/duckdb/2.0.0/x86_64-pc-windows-msvc`）。
   libduckdb 是 **C ABI**（`duckdb.h`），MSVC 导入库（`.lib`）在 lld 下链接成功、
   运行期在 Windows 11 正常加载（doctor 实测 `duckdb_engine_version: v2.0.0`、
   `duckdb_error: null`）。载荷目录名保留 `msvc` 字样并在脚本中显式登记
   “gnullvm 目标取 msvc 载荷”的映射，禁止静默改名；
5. **ARM64 交付目标暂不切换**（`aarch64-pc-windows-msvc` 维持 ADR-043 口径）：llvm-mingw
   官方 Windows 包只有 x86_64 宿主版，在 `windows-11-arm` runner 上以 x64 模拟运行会把
   “原生 runner 平台证据”变成“模拟工具链证据”，证据口径需另立修订。登记为后续 ADR，不静默遗留；
6. **msvc 通道保留为回退通道**：`[target.x86_64-pc-windows-msvc]` 的 `/Brepro` 与相关脚本
   分支保留，验收结论不再从该通道取得。

## 3. 尖峰证据（2026-11-14 本机 Windows 11 amd64）

| 证据 | 命令（要点） | 结果 |
|---|---|---|
| 全工作区构建 | `cargo build --workspace --target x86_64-pc-windows-gnullvm`（gnullvm 工具链） | EXIT=0，2 分 07 秒，含 GUI（iced）、russh、ring、aws-lc-sys |
| 主机原生构建 | `cargo build -p whcode-bin`（不传 `--target`，host=gnullvm） | EXIT=0，产物在 `target/debug/`，`duckdb.dll` 随构建脚本就位 |
| 全工作区测试 | `cargo test --workspace`（host=gnullvm） | EXIT=0：1205 通过／0 失败／17 忽略（含 layering／layout 契约；尖峰期唯一失败是本机 `local/` 遗留临时清单，清出后整轮复跑全绿） |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | EXIT=0，零告警 |
| 链接确定性 | release 构建两次（删 exe 重链） | 两次 SHA256 一致：`f64f8ebbb30535a803f3bca7360e3e576a805e003207fcda90d6139c631f0f71` |
| DuckDB 运行 | `whcode.exe doctor --json` | `duckdb_engine_version: v2.0.0`、`duckdb_error: null` |
| 产物运行时依赖 | `llvm-readobj --coff-imports whcode.exe` | 仅系统 DLL＋UCRT API set（`api-ms-win-crt-*`）＋`duckdb.dll`；**无 vcruntime 依赖**（duckdb.dll 自身也只依赖 kernel32／RstrtMgr／WS2_32） |

排除过的歧路（留档防重蹈）：

- rust-lld ＋ Windows SDK 导入库手动 `-Lnative`：链接可过，但 `ring` 的 C 编译因缺
  mingw 头（`assert.h`）失败；且 host 构建脚本链接吃不到 `[target.*]` rustflags；
- 把导入库复制进工具链 self-contained 目录：能过链接但污染工具链、不可复现，弃用；
- `ring`/C 依赖缺少的是**头文件与驱动**，不是库——任何“只补库”的方案都不可行。

## 4. 正负和解

**正面**：

- Windows 全链路脱离 MSVC：rustc（gnullvm 目标）＋clang（C 依赖）＋lld（链接）均为 LLVM 系列；
- 产物不依赖 VC 运行库（无 vcruntime 导入），分发包的运行前提从“装 VC Redist”降为“Windows 11 自带 UCRT”；
- lld 默认确定性，发布可复现性少一个 `/Brepro` 特判。

**负面与承担者**：

- 新增 llvm-mingw 依赖（约 190 MB 下载、约 800 MB 落盘）：开发机与 CI 各承担一次安装，
  CI 用固定版本＋SHA256 校验的下载步骤承担可复现责任；
- 工具链组合多一个外部组件（上游 mstorsjo/llvm-mingw），版本漂移风险由平台矩阵负责人的
  “复审触发”承担（见 §9）；
- `api-ms-win-crt-private-l1-1-0.dll` 属 UCRT API set（加载经 apisetschema 解析到
  ucrtbase.dll），极旧系统无该 API set——验收目标是 Windows 11，风险可接受；
- DuckDB 官方载荷仍为 MSVC 构建：C ABI 兼容性依赖“官方不发布 mingw 载荷”这一外部事实，
  失效时按 §8 处理。

**和解理由**：用户指令明确“不用 GNU 那套”且验收目标是 Windows 11 amd64；gnullvm 是
唯一同时满足“LLVM 系列、非 GNU、Windows 11 amd64、无需浏览器引擎”的目标三元组，
且尖峰已证明全链路可用。代价集中在一次性的安装与文档改造，长期收益是运行前提收窄与
链接器统一。

## 5. 三重时间

- **过去**：D-01（2026-10-21）定下“Windows 只用 msvc、禁 GNU”；ADR-069（2026-11-14）
  把工具链升到 1.100.0。本决策修订 D-01 的工具链落点：禁的是 GNU 系，不再绑定 MSVC 系；
- **现在**：本文件 §3 的实测证据（构建、测试、clippy、确定性、运行）；
- **将来失效条件与复审触发**：见 §8（绑定命令＋判定规则）；复审日期不设固定值，
  命令失效即触发。

## 6. 迁移面（文件级）

| 文件 | 改动 |
|---|---|
| `.cargo/config.toml` | 新增 `[target.x86_64-pc-windows-gnullvm]`（显式 linker）；DUCKDB_LIB_DIR 注释说明载荷映射 |
| `rust-toolchain.toml` | 注释更新：Windows 机器级 override 改 `1.100.0-x86_64-pc-windows-gnullvm`；channel 仍只锁版本（BSD 不变） |
| `.github/scripts/fetch-duckdb.py` | gnullvm 目标→msvc 载荷映射；host 推断加 gnullvm 分支 |
| `.github/scripts/package-windows.py` | SUPPORTED_TARGETS 增 gnullvm→amd64；host 三元组改由 `rustc -vV` 实测 |
| `.github/scripts/platform-matrix/record_deps.py` | 目标三元组记录改实测（gnullvm） |
| `.github/workflows/platform-verification.yml` | windows 通道：安装 llvm-mingw（固定版本＋SHA256）＋ RUSTUP_TOOLCHAIN；windows-arm64 通道不变 |
| `.github/workflows/build-release.yml` | 同上，发布通道切 gnullvm |
| `docs/adr/ADR-013`、ADR-047、ADR-069 | 追加修订记录（指向本 ADR） |
| `docs/matrices/platform-matrix.md`、`compatibility.md`、`decisions.md`（D-01） | 三元组与工具链口径更新 |
| `specs/001-whcode-harness/plan.md`（D-01）＋ `spec.md` | 硬边界表述更新 |
| `AGENTS.md`、`CONTRIBUTING.md`、`README*.md`（十三语言） | 环境准备与三元组表述更新 |
| `docs/guides/repository-layout.md`、CHANGELOG、tasks/checklist 台账 | 同步 |

## 7. 回退方案

回退＝机器级 override 改回 `1.100.0-x86_64-pc-windows-msvc`（或 CI 撤掉 gnullvm 环境步骤），
`.cargo/config.toml` 的 `[target.x86_64-pc-windows-gnullvm]` 节删除即可；msvc 分支与
`/Brepro` 全程保留、未被删除，因此回退不需要恢复任何已删代码。回退后按 ADR-047 重跑
同提交双构建哈希比对确认确定性仍成立。

## 8. 已知限制与失效条件（复审触发，绑定命令＋判定）

- **llvm-mingw 下载不可得**（CI 或本地）：`curl -fsSL <固定 URL> | sha256sum` 与
  `552191b5…40f` 不一致或 404——判定：通道红，禁止换“随手可得的版本”冒充；处理：
  升级到新版本需新修订并重跑 §3 全部证据；
- **C 依赖新增**（新增带 C 构建的 crate）：`cargo build --workspace` 红且错误含
  `file not found`／链接器缺库——判定：该 crate 需要评估头文件／导入库可得性后再引入，
  不得以“msvc 下能编译”为由直接合入；
- **DuckDB 官方载荷形态变化**（如只发静态库或 mingw 载荷）：`fetch-duckdb.py --verify`
  失败或 payload 缺 `duckdb.lib`——判定：先在 gnullvm 下实测新载荷链接与运行，再改映射；
- **上游弃用 gnullvm 目标**（`rustup target list` 不再含之，或 rust-toolchain channel
  安装失败）：判定：触发回退方案（§7），并新开 ADR 重定方向；
- **ARM64 后续切换**：另立修订，不在本 ADR 承诺范围内（显式缺口）。

## 9. 文献依据

- rustc 平台支持页：`x86_64-pc-windows-gnullvm` 为 Tier 2 with host tools（rustup 可作
  主机工具链分发）；gnullvm＝GCC 兼容 ABI 但使用 LLVM lld 与 LLVM 运行库（无 GCC/binutils）；
- llvm-mingw 项目（mstorsjo/llvm-mingw）：LLVM/Clang/LLD＋mingw-w64 头与 CRT（UCRT），
  驱动命名 `x86_64-w64-mingw32-clang`；发行说明与自述明确其无 GCC 依赖；
- lld 文档（COFF 后端）：默认确定性输出（时间戳为零），`/Brepro` 仅在需要 MSVC link.exe
  语义时使用；
- Microsoft UCRT 文档：Windows 10 起 UCRT 为操作系统组件，`api-ms-win-crt-*` 为
  API set（apisetschema 解析）。

## 10. 形式审查

- **真实矛盾**：D-01 禁 GNU 与“工具链要全面 LLVM 化”的用户指令之间，msvc 三元组成为
  最后一个非 LLVM 环节；gnullvm 是唯一同时满足两者且尖峰可用的解；
- **正负和解**：见 §4，代价（安装与维护）与承担者（平台矩阵负责人）显式；
- **0 信任校验**：本文件全部结论来自本机可复现命令（§3），无 AI 总结替代证据；
  `api-ms-win-crt-private` 的存在性以运行期实测（doctor 成功）为准，未引用二手文档断言；
- **知难行易**：方向＝工具链 LLVM 化（用户指令），目标＝验收目标三元组更换（用户确认
  “仓库目标也换”），手段＝llvm-mingw 是尖峰排除歧路后剩下的唯一合规解，选型晚于认知。
