# ADR-DB-BUILD：DuckDB 原生依赖的构建与分发方式

- **状态**：**已批准（M0 实测结项）**——取代“未定”，但平台覆盖为**部分结论**，见下
- **来源**：tasks.md T022/T023；《WHCode 技术任务书 v3.0》ADR-001（唯一默认持久化后端）、ADR-010（跨平台可构建与 DuckDB 原生构建分开验收）、R-01/R-02、research.md TBD-01
- **责任人**：M0 执行人（本机实测：Windows x86_64）
- **退出条件**：五平台各自取得“DuckDB 原生依赖可构建并运行”的实测结论，或形成明确 Go/No-Go 与阻塞记录

## 决策

DuckDB 以**官方预编译动态库**（而非源码构建）引入，理由有三：

1. 不再需要为构建 DuckDB 引入 C++ 工具链与 CMake 流程，减少平台构建面（R-01 风险收敛）；
2. 官方预编译包为 MIT 许可，可直接校验哈希（ADR-010 要求记录来源与补丁状态）；
3. 与用户决策一致：不使用任何 GNU 工具链，改用预编译产物（见 `docs/matrices/decisions.md` D-01）。

具体接法：`duckdb` crate（`default-features = false`）→ `libduckdb-sys` 的
`DUCKDB_LIB_DIR` 分支——该分支只做链接与头文件定位，不做源码构建。仓库侧：

| 组件 | 作用 |
|---|---|
| `ci/fetch-duckdb.py` | 按固定 URL + SHA256 拉取官方预编译包并解包到 `vendor/duckdb/<version>/` |
| `.cargo/config.toml` `[env] DUCKDB_LIB_DIR` | 指向解包目录（相对仓库根；未拉取时构建**显式失败**） |
| `crates/whcode-store/build.rs` | 存在 `duckdb.dll` 的平台上把该文件复制到 `<profile>/` 与 `<profile>/deps/`，使二进制与测试进程能加载（按载荷存在性分流，不做平台判定，NFR-101） |
| `.gitignore` `/vendor` | 预编译二进制不入库；`PROVENANCE.txt` 留证 |

**禁止**在没有新 ADR 的情况下切换默认持久化后端，或以某平台构建困难为由静默降级（任务书封面约束、§9.1）。

## 实测证据（Windows x86_64）

| 项 | 值 |
|---|---|
| `duckdb` crate | `1.10506.1`（MIT，`rust-version = 1.88`） |
| crate → 引擎版本映射 | `1.10506.1` → DuckDB `1.5.6`（映射规则见 `libduckdb-sys` 的 `duckdb_version_from_pkg_version`） |
| 下载来源 | `https://github.com/duckdb/duckdb/releases/download/v1.5.6/libduckdb-windows-amd64.zip` |
| 归档 SHA256 | `44cf59583f9951d2cb09b1bf115a63ecb2d8901e363903029d86c7d8683fe96a`（13 408 787 字节） |
| 载荷 SHA256（`duckdb.dll`） | `7e90bdef028d57b45490d6f53530e3c012a5d9abdc4c6723449200663dc84ed0`（36 697 896 字节） |
| 归档成员 | `duckdb.dll`、`duckdb.lib`、`duckdb.h`、`duckdb.hpp`、`duckdb_extension.h` |
| 运行期引擎版本 | `v1.5.6`（`whcode doctor --json` 的 `duckdb_engine_version`，由 `SELECT version()` 实测） |
| 构建与冒烟 | `cargo test -p whcode-store` 通过（创建 / 事务写读 / 关闭重启 / 迁移四步） |
| 工具链 | rustc 1.99.1（`x86_64-pc-windows-msvc`）、MSVC 14.44.35207、Windows SDK 10.0.26100.0 |

**上游补丁**：无。使用官方发布物原样，未做任何本地补丁。

## 各平台结论（V-001/V-002 的平台覆盖）

| 平台 | 目标三元组 | M0 结论 | 说明 |
|---|---|---|---|
| Windows x86_64 | `x86_64-pc-windows-msvc` | **通过（本机实测）** | 构建、冒烟四步、运行期引擎版本均已取得证据 |
| FreeBSD | `x86_64-unknown-freebsd` | **阻塞** | 官方**无**预编译包；须由 `pkg`/ports 提供，或以 `bundled-cmake` 源码构建。本机无 FreeBSD 环境，未取得运行证据 |
| NetBSD | `x86_64-unknown-netbsd` | **阻塞** | 同上 |
| OpenBSD | `x86_64-unknown-openbsd` | **不适用（已移出原生安装目标）** | 平台承诺修订为“经 WHCode 内置 SSH 客户端控制”，见 `decisions.md` D-13 |
| DragonFly BSD | `x86_64-unknown-dragonfly` | **不适用（同上）** | 同上 |

**交叉检查的边界（重要）**：`cargo check --target x86_64-unknown-{freebsd,netbsd}` 只做类型检查，
**不链接**，因此它证明的是“代码级兼容”，**不能**证明 DuckDB 原生库在该平台可构建或可运行
（ADR-010、G-01 明确禁止以交叉编译代替运行验证）。BSD 侧的 V-002 结论在取得真实平台环境前保持**阻塞**。

## 替代方案

- **`bundled` / `bundled-cmake` 源码构建**：需要目标平台的 C/C++ 编译器与 CMake；BSD 侧仍需
  `cc`/`cmake` 且构建耗时长。保留为 BSD 平台的候选路径（当 `pkg` 提供的 DuckDB 版本与 crate
  要求的 `1.5.6` 不一致时，源码构建是唯一可控选择）。
- **系统包管理器（`pkg install duckdb`）**：省去下载与校验，但版本由 ports 树决定，可能与 crate
  绑定版本不匹配；采用前必须先核对 `duckdb.h` 的 ABI 版本。
- **改用其他嵌入式数据库**：被任务书封面约束与 §9.1 否决（须先有批准 ADR 与 Go/No-Go 决策）。

## 影响

- 约束 crate：`whcode-store`（唯一静态链接点）、`whcode-bin`（`doctor` 报告运行期引擎版本）。
- 构建前置：任何构建/测试前必须先执行 `python ci/fetch-duckdb.py`，否则 cargo 会因
  `DUCKDB_LIB_DIR` 指向不存在的目录而显式失败（这是刻意的失败模式，避免静默改用其他后端）。
- CI：`ci/platform-matrix/*.yml` 需在构建前加入拉取步骤（Windows 用 `record_deps.py` 同源逻辑；
  BSD runner 需改为 `pkg` 或源码构建，属后续真实平台验证项）。
- 遗留项：BSD 侧 V-002 证据缺失；`docs/matrices/platform-matrix.md` 与 `artifacts/m0-report.md`
  必须如实登记为阻塞，不得标记为“已交付”。
