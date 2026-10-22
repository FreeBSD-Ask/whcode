# docs/matrices

平台、三端能力与运行时矩阵（tasks.md T007、T028、T087、T118、T120）。

矩阵文档状态：

| 文件 | 内容 | 交付任务 | 状态 |
|---|---|---|---|
| `platform-matrix.md` | 各平台精确版本、架构、角色、构建与运行验证状态；性能预算冻结状态 | T028（M0） | 已产出（性能预算**未冻结**） |
| `decisions.md` | D-01—D-13 决策点、结果、证据引用与责任人 | T030（M0 起持续更新） | 已产出 |
| `feature-parity.md` | 三端（TUI/CUI/Web）+ SSH 的业务能力矩阵与权限一致性 | T087（M5） | 未产出 |
| `runtime-inventory.md` | 运行时能力等级（Full/Restricted/ShellOnly）、payload 哈希与许可证清单 | T120（M9 前） | 未产出 |

相关 ADR：`docs/adr/ADR-DB-BUILD.md`（DuckDB 预编译库）、`docs/adr/ADR-011-platform-commitment-revision.md`
（OpenBSD/DragonFly 改为经内置 SSH 客户端控制）。

纪律：

- 平台承诺必须写明精确 OS 版本与 CPU 架构，不以"Windows""BSD"作无限范围承诺（§9.1）；
- 未经实测的能力不得出现在矩阵中标记为"已交付"（宪法原则 VII）；
- 平台/运行时限制必须可查询（NFR-107），并随依赖升级更新（§15 PR 清单）。
