# 三端功能能力矩阵（T087、FR-610）

> 新能力必须在**同一 PR 内**定义 API、CLI 行为、权限与测试（FR-610）。
> 三端对等判据是业务能力、权限检查、状态迁移、错误语义与可审计性一致，不要求
> 视觉/交互一致（ADR-009）。

**状态**：M5 部分交付（T087 首版）。✅ = 已交付并有测试；🚧 = 路由存在、视图/入口未交付；
⛔ = 里程碑未到。

**Web 面（2026-10-22 更新）**：`whcode serve --web` 已装配（T090，真实浏览器实测）；
Web 当前为**只读**：会话列表 / 事件列表（脱敏注明）/ 状态快照——写命令与任务视图未接入。

**最后更新**：2026-10-22 | 依据：`specs/001-whcode-harness/contracts/app-service.md` §2 命令面

| 域 | 命令 | app 层（API） | CUI | TUI | Web | 依据 |
|---|---|---|---|---|---|---|
| session | new/list/show/rename/archive | ✅（M2/T046） | 🚧（T084） | 🚧 列表只读 + 选择 | 🚧 列表只读（可点击选择） | FR-101 |
| session | delete（预览+确认+审计） | ✅（M2/T046） | 🚧 | 🚧 | ⛔ | FR-101、FR-509 |
| session | switch（有界加载） | ✅（M2/T047） | 🚧 | 🚧 | ⛔ | FR-102、V-101 |
| task | list/show（详情五类） | ✅（M2/T048/T049） | 🚧 | ✅ 总览 + 活跃（只读） | ✅ 总览 + 活跃（只读） | FR-103/104 |
| task | retry/abort | ✅（M2/T052） | 🚧 | 🚧 | ⛔ | FR-401/408 |
| spec | 状态机/门禁/force-advance | ✅（M4/T074/T078） | ✅ 视图（spec_cmd） | ✅ 状态摘要只读 | 🚧 状态快照只读 | FR-307/312 |
| context | budget/compact（指针） | ✅（M3/T060/T061） | ✅ 视图（context_cmd） | ✅ 预算摘要只读 | ⛔ | FR-105/107/108 |
| llm | run/cancel（Provider） | ✅（M3/T056 桩） | ⛔ | ⛔ | ⛔ | FR-201 |
| events | list/export（脱敏注明） | ✅（M3/T065/T066） | ✅ 视图（events_cmd） | ✅ 尾部只读 | 🚧 列表只读（无导出） | FR-202/205 |
| db | stats/check/purge/checkpoint/compact/archive | ✅（M1/T038—T042） | 🚧 | 🚧 | ⛔ | FR-507—510 |
| doctor | 环境自检 | ✅（M0） | ✅（whcode-bin） | — | — | §10 |

**权限**：全部命令经 `whcode-app::authz` 统一判定（拒绝 `permission_denied` 不可重试，
FR-604）；四端不得自行推断权限。

**测试映射**：app 层内联测试（每命令正向 + 失败路径）；CUI 视图测试（信封 schema_version +
脱敏注明）；契约测试 `test_json_envelope.rs`（T089）、`test_tui_cui_parity.rs`（T088）、
`test_web_security_defaults.rs`（T095）。
