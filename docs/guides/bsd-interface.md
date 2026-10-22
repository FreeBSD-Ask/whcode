# BSD 接口预留指南（FreeBSD / NetBSD / OpenBSD / DragonFly）

**Feature**: [spec.md](../../specs/001-whcode-harness/spec.md) | **ADR**: [ADR-011](../adr/ADR-011-platform-commitment-revision.md) | **DB**: [ADR-DB-BUILD](../adr/ADR-DB-BUILD.md) | **日期**： 2026-10-21

本文档固化“BSD 落地时**必须复用**、**必须实现**、**禁止绕过**的接口”，保证 Windows 侧先行交付
的代码不为 BSD 返工（M0 决策：Windows 已 Go；FreeBSD/NetBSD 原生目标待环境；OpenBSD/DragonFly
经内置 SSH 客户端控制，不再原生安装）。

## 1. 必须复用（已就绪，BSD 落地零改动）

| 接口 | 位置 | 说明 |
|---|---|---|
| 状态/事件/错误类型 | `whcode-domain` | 四组状态枚举、事件类型（§8.3 命名）、`WhError` 四字段、审计记录——**唯一出处**，BSD 与 Windows 共享，禁止平台各自定义 |
| 有界队列语义 | `whcode-app::BoundedQueue` | 关键条目背压、快照只淘汰快照——平台无关 |
| 写者服务边界 | `whcode-store::EventWriter` | ADR-002 单写者；BSD 仅需提供可链接的 DuckDB |
| SSH 客户端 | `whcode-ssh::proto` | `SshTarget` / `ExecReport` / `ExecRegistry`，按 ADR-007 登记；OpenBSD/DragonFly 的**全部**控制走此接口（FR-611） |
| 依赖记录 | `ci/platform-matrix/record-deps.sh` | POSIX sh，FreeBSD/NetBSD 直接可执行 |

## 2. 必须实现（接口已预留，BSD 侧补实现）

### 2.1 平台行为（NFR-101：全部收敛于 `whcode-platform`）

| 接口 | 当前状态 | BSD 侧工作 |
|---|---|---|
| `process::termination_policy_for()` | 已声明 BSD 分支（进程组 `setsid` + `killpg` 语义） | M8/T109 实现并实测进程树清理 |
| `paths::*`（`workspace_temp_dir` / `workspace_config_dir`） | 平台无关（`<workspace>/tmp`、`<workspace>/.whcode`） | 无需改动；确认默认权限位（0700 目录）符合 §10.2 最小暴露 |
| `resources::available_physical_cores()` | `std::thread::available_parallelism` | 在四大 BSD 各实测一次取值，偏差写入依赖记录 |
| `resources::memory_limit_defaults()` | 常量基线 | 无需改动 |
| `term::terminal_size()` | crossterm `terminal::size()` | 在 BSD 终端（含串口/SSH pty）实测；`None` 语义不变 |
| 内存总量探测 | **未预留**（`record_deps.py` 仅 Windows ctypes） | 在 `whcode-platform` 新增 `total_memory_bytes()`，BSD 走 `sysctl hw.physmem`（cfg 分支只许在此 crate） |

**纪律**：以上任何 `cfg(target_os = "freebsd")` 等分支**只允许**出现在 `whcode-platform`；
`tests/contract/test_layering.rs` 以 10 类模式（含 `cfg(all(`、`CARGO_CFG_*`、`build.rs`）强制执行。

### 2.2 DuckDB 装载（环境变量契约，零代码切换）

`.cargo/config.toml` 的 `[env]` 表只服务 Windows vendor 路径；外部 shell 环境变量**优先于**该表
（未设 `force`），因此 BSD 侧只需导出：

```sh
# FreeBSD（pkg）：头文件与库在 /usr/local
export DUCKDB_LIB_DIR=/usr/local/lib
export DUCKDB_INCLUDE_DIR=/usr/local/include

# NetBSD（pkgsrc）：前缀 /usr/pkg
export DUCKDB_LIB_DIR=/usr/pkg/lib
export DUCKDB_INCLUDE_DIR=/usr/pkg/include
```

- 头文件位置要求：`$DUCKDB_LIB_DIR/duckdb.h` 或 `$DUCKDB_INCLUDE_DIR/duckdb.h`（`libduckdb-sys` 契约）；
- `crates/whcode-store/build.rs` 只在 `DUCKDB_LIB_DIR/duckdb.dll` **存在**时复制——BSD 无该文件，
  自然跳过，无需平台分支；
- 版本核验：pkg/ports 的 DuckDB 版本须与 crate 绑定的 1.5.6 对账，不一致时改 `bundled-cmake`
  源码构建，并把结论写进 ADR-DB-BUILD（禁止静默换后端）。

### 2.3 Go/No-Go 门禁（CI 已按此预留）

`ci/platform-matrix/{freebsd,netbsd}.yml` 的绿路径（`cargo check` + 无 libduckdb 链接的包级测试）
必须过；完整构建+测试作为显式门禁步骤，失败即 No-Go 证据（R-01）。

## 3. 禁止绕过

- **禁止**在前端 crate（tui/cui/web/ssh）直接触库或平台分支（ADR-003、NFR-101）；
- **禁止**把 OpenBSD/DragonFly 当原生目标重新引入——只有新的批准 ADR 才能修订 ADR-011；
- **禁止**以交叉编译产物冒充 BSD 运行验证（G-01、ADR-010）；
- SSH 控制必须走 `ExecRegistry` 登记（ADR-007），不确定副作用标 `unknown`。

## 4. 落地路线

| 阶段 | 平台 | 工作项 | 依赖 |
|---|---|---|---|
| M1 起 | FreeBSD/NetBSD | DuckDB 版本对账 + `record-deps.sh` 产出 `deps-*.txt` | 环境（B-01/B-02） |
| M2—M5 | 全部 | 应用服务/TUI 天然跨平台，无需平台工作 | 绿路径 CI |
| M7 | FreeBSD/NetBSD | SSH **服务端**验收（V-004 修订判据） | TBD-05 选型 |
| M7 | OpenBSD/DragonFly | SSH **客户端**连通性 + 受限命令执行证据 | `whcode-ssh::proto` 已原型 |
| M8 | 全部 BSD | 进程组终止实测（T109）、运行时 payload 校验 | `termination_policy_for()` 已声明 |
