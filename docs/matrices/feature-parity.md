# 三端功能能力矩阵（T087、FR-610）

> 新能力必须在**同一 PR 内**定义 API、CLI 行为、权限与测试（FR-610）。
> 三端对等判据是业务能力、权限检查、状态迁移、错误语义与可审计性一致，不要求
> 视觉/交互一致（ADR-009）。

**状态**：M5 部分交付（T087 首版；CUI 列于 2026-10-23 按 `cui_dispatch` 实测复核并更新）。
✅ = 已交付并有测试；🚧 = 路由存在、视图/入口未交付；⛔ = 里程碑未到。

**Web 面（2026-10-24 更新）**：`whcode serve --web` 已装配（T090，真实浏览器实测）；
读命令：会话列表 / 任务总览 + 活跃任务 / 事件列表（脱敏注明）/ 状态快照 / 文本层；
**写命令已接入（T110—T114 批次，FR-609）**：`POST /api/sessions/new|rename|archive|delete`、
`/api/tasks/retry|abort`、`/api/purge`（预览默认 + 确认执行，破坏性命令防误触）——
与 CUI 同源 `whcode-app` 判权（FR-604），33 项 API 测试含写命令预览/确认/权限拒绝
（`crates/whcode-web/src/api.rs` 内联测试）。部署边界：仅回环或局域网（RFC1918，认证必填，
ADR-023）。**无 JS 只读兜底（2026-10-24）**：首页服务端直出 `<noscript>` 会话快照（HTML 转义、
超限写明截断；仅未认证的本机回环模式，认证模式不注入数据），见 Check 229。

**最后更新**：2026-10-24 | 依据：`specs/001-whcode-harness/contracts/app-service.md` §2 命令面；
CUI 列以 `crates/whcode-bin/src/cui_dispatch.rs` 的分支与“未接线”清单为准（`run`、`events show`、
`spec next/validate/analyze/converge/force-advance`、`runtime *`、`db checkpoint` 明确未接线）

| 域 | 命令 | app 层（API） | CUI | TUI | Web | 依据 |
|---|---|---|---|---|---|---|
| session | new/list/show/rename/archive | ✅（M2/T046） | ✅（T084/T085 调度） | 🚧 列表只读 + 选择 | 🚧 列表只读（可点击选择） | FR-101 |
| session | delete（预览+确认+审计） | ✅（M2/T046） | ✅（dry-run 默认 + `--yes`） | 🚧 | ⛔ | FR-101、FR-509 |
| session | switch（有界加载） | ✅（M2/T047） | 🚧（无独立子命令，随 `--session` 生效） | 🚧 | ⛔ | FR-102、V-101 |
| task | list/show（详情五类） | ✅（M2/T048/T049） | ✅ | ✅ 总览 + 活跃（只读） | ✅ 总览 + 活跃（只读） | FR-103/104 |
| task | retry/abort | ✅（M2/T052） | ✅ | 🚧 | ⛔ | FR-401/408 |
| spec | 状态机/门禁/force-advance | ✅（M4/T074/T078） | ✅ `status`；🚧 门禁检查与 force-advance 未接线 | ✅ 状态摘要只读 | 🚧 状态快照只读 | FR-307/312 |
| context | budget/compact（指针） | ✅（M3/T060/T061） | ✅ 视图（context_cmd） | ✅ 预算摘要只读 | ⛔ | FR-105/107/108 |
| llm | run/cancel（Provider） | ✅（M3/T056 桩） | ⛔（`run` 未接线） | ⛔ | ⛔ | FR-201 |
| events | list/export（脱敏注明） | ✅（M3/T065/T066） | ✅ list/export；🚧 `show` 未接线 | ✅ 尾部只读 | 🚧 列表只读（无导出） | FR-202/205 |
| db | stats/check/purge/checkpoint/compact/archive | ✅（M1/T038—T042） | ✅ stats/check/purge/compact/archive/explain；⛔ `checkpoint` 非公开命令 | 🚧 | ⛔ | FR-507—510 |
| theme | 主题选择（配置面） | — | —（随端内建） | ✅ 四套（`ui.theme`／`WHCODE_UI_THEME`） | 🚧 仅基础主题（无切换） | FR-612/713 |
| font | 本机字体声明（install/status/uninstall） | ✅（FR-717 编排） | ✅（T155） | —（终端字形由模拟器控制） | —（非浏览器能力） | FR-717、ADR-028 |
| ssh | 服务端受限入口（`sessions`／`session`／`tasks`／`task`；默认无 Shell） | ✅（M7/T097 判权） | —（经内置 SSH 客户端） | — | — | FR-607/608、ADR-030 |
| doctor | 环境自检 | ✅（M0） | ✅（whcode-bin） | — | — | §10 |

**权限**：全部命令经 `whcode-app::authz` 统一判定（拒绝 `permission_denied` 不可重试，
FR-604）；四端不得自行推断权限。

**测试映射**：app 层内联测试（每命令正向 + 失败路径）；CUI 视图测试（信封 schema_version +
脱敏注明）；契约测试 `test_json_envelope.rs`（T089）、`test_tui_cui_parity.rs`（T088）、
`test_web_security_defaults.rs`（T095）。
