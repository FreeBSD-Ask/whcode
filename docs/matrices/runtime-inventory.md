# 运行时能力清单（T120、§12.2）

**版本**：`0.1.0`（M9 首版） | **更新**：2026-10-24 | 依据：FR-701—710、D-07—D-09、R-10

## 1. 运行时能力（M8/T100—T114 范围；当前状态）

| 运行时 | capability | 状态 | 说明 |
|---|---|---|---|
| `js`（受限 ECMAScript 引擎） | Restricted | **已实现**（T110，2026-10-24；feature `runtime-js` 门控，默认构建不引入） | ADR-029（修订 ADR-024 的 D-08 条款）：Boa 0.21.1，Node API 六名字显式“能力不足”拒绝桩；声明不宣称完整 Node.js；未覆盖能力返回结构化错误 |
| `python`（子进程 worker） | Restricted | **已实现**（T111，2026-10-24） | D-09：stdin 单行 JSON worker 协议（请求 ≤4 MiB、输出有界缓冲）；`PythonEnv` 记录解释器绝对路径/版本/实现名/ABI/C 扩展探测；受限档位五项不支持项（承诺边界不是沙箱） |
| `gitbash`（PortableGit payload） | ShellOnly | **已实现**（T112，2026-10-24） | V-304：payload 四重校验（来源/版本/平台/哈希）+ Bash 五类能力探测（管道/重定向/含空格路径/超时终止/目标命令）真实取证 |
| `bsdshell` | ShellOnly | **未实现**（T113 注册条目已交付；实现属 M7，ADR-013 延后） | 与 `gitbash` 是不同能力，不共用名称伪装等价（§4.7.1） |

**能力等级**（FR-702）：`Full` / `Restricted` / `ShellOnly`——当前全部运行时
**未安装即不可用**（`enabled=false` 不被工具注册表加载，FR-704）。

**默认启用策略**（P5.6／FR-784／ADR-024 修订 2026-11-15）：四项全部**默认关闭**，
`doctor`／CUI `runtime list`／Web `/api/runtimes` 输出 `default_enabled` 与
`enable_requirement`（为什么关、怎么打开）：js＝引擎不交付（ADR-024 决策 1）；
python＝装解释器并 opt-in；gitbash＝payload 四重校验；bsdshell＝M7 延后。
改默认值必须修订 ADR-024 并同批更新守卫测试。

## 2. payload 校验（FR-703、D-07）

- payload **独立分发**（来源/版本/哈希/平台校验，T102）；
- 运行时不随主二进制默认携带——按需安装、支持离线镜像（D-07）；
- 安装记录入 `runtime_state` 表（version、payload_path、installed_at）。

## 3. 许可证清单

| 组件 | 许可证 |
|---|---|
| WHCode | BSD-2-Clause（FreeBSD 中文社区） |
| DuckDB 2.0.0（动态库） | MIT（上游预编译包，`.github/scripts/fetch-duckdb.py` 校验 SHA256） |
| Rust 1.100.0 工具链 | Apache-2.0/MIT |
| clap / serde / thiserror / ratatui / crossterm | Apache-2.0/MIT（Cargo.lock 版本锁定） |
| duckdb 1.20000.0 / libduckdb-sys 1.20000.0 | MIT（`cargo metadata` 核实） |
| arrow 60.0.0 | Apache-2.0（`duckdb` 传递依赖，静态链入 `whcode.exe`） |

**未捆绑**：任何运行时 payload（js/python/gitbash）——安装时须核对其上游许可证
并登记到本清单（T102/T120 收口）。

## 4. 沙箱强度（P5.2，ADR-077）

上报面：`whcode doctor`（文本）与 `doctor --json` 的 `sandbox` 块，两者同源（同一份
`SandboxReport`）。强度按**实际生效的面**判定，不按配置意图判定。

| 强度 | 判据 | 当前 |
|---|---|---|
| `none` | 模型没有任意命令执行面（`shell` 工具未注册） | 默认构建即此档 |
| `partial` | 有执行面，仅工具级控制；OS 层不隔离文件与网络 | 启用 `shell` 工具后（`.whcode/shell.json`＋bash 探测通过） |
| `full` | OS 层强制隔离生效且残余缺口清零 | **不可达**（两平台均为 `Unavailable`，见下） |

**工具级控制（六条，已生效）**：`cwd_anchor`、`minimal_env`、`credential_strip`、
`output_truncation`、`timeout_tree_kill`、`concurrency_gate`。

**OS 层缺口（五条，未收敛）**：`filesystem_write`（ADR-070 T6）、`filesystem_read`（T3）、
`network`、`disk_quota`（T5）、`secondary_command_path`（T4）。

**为何 `full` 不可达**：Windows 侧需 `CreateAppContainerProfile`、
`UpdateProcThreadAttribute(SECURITY_CAPABILITIES)`、`CreateProcess(EXTENDED_STARTUPINFO_PRESENT)`
与工作区 DACL 授权；本仓 `unsafe_code = "forbid"`（NFR-215）且依赖树无 Win32 绑定，
`std::process::Command` 也不暴露 `lpAttributeList`。Unix 家族本批未启用 `chroot`／`jail`。
强制隔离须另立 ADR（安全包装依赖或 ADR-015 式隔离 crate）。
