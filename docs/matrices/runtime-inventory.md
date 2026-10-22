# 运行时能力清单（T120、§12.2）

**版本**：`0.1.0`（M9 首版） | **更新**：2026-10-22 | 依据：FR-701—710、D-07—D-09、R-10

## 1. 运行时能力（M8/T100—T114 范围；当前状态）

| 运行时 | capability | 状态 | 说明 |
|---|---|---|---|
| `js`（受限 ECMAScript 引擎） | Restricted | **未实现**（T110，M8） | D-08：不宣称完整 Node.js；未覆盖能力返回结构化错误（`runtime_unavailable`） |
| `python`（子进程 worker） | Restricted | **未实现**（T111，M8） | D-09：先验证子进程协议与平台解释器再决定是否捆绑 |
| `gitbash`（PortableGit payload） | ShellOnly | **未实现**（T112，M8） | V-304：Windows PortableGit 校验与 Bash/coreutils 验证 |
| `bsdshell` | ShellOnly | **未实现**（T113，M8） | 与 `gitbash` 是不同能力，不共用名称伪装等价（§4.7.1） |

**能力等级**（FR-702）：`Full` / `Restricted` / `ShellOnly`——当前全部运行时
**未安装即不可用**（`enabled=false` 不被工具注册表加载，FR-704）。

## 2. payload 校验（FR-703、D-07）

- payload **独立分发**（来源/版本/哈希/平台校验，T102）；
- 运行时不随主二进制默认携带——按需安装、支持离线镜像（D-07）；
- 安装记录入 `runtime_state` 表（version、payload_path、installed_at）。

## 3. 许可证清单

| 组件 | 许可证 |
|---|---|
| WHCode | BSD-2-Clause（FreeBSD 中文社区） |
| DuckDB 1.5.6（动态库） | MIT（上游预编译包，`ci/fetch-duckdb.py` 校验 SHA256） |
| Rust 1.99.1 工具链 | Apache-2.0/MIT |
| clap / serde / thiserror / duckdb-rs / ratatui / crossterm | Apache-2.0/MIT（Cargo.lock 版本锁定） |

**未捆绑**：任何运行时 payload（js/python/gitbash）——安装时须核对其上游许可证
并登记到本清单（T102/T120 收口）。
