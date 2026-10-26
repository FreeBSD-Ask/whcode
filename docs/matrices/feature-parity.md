# 四端对等基线（受校验产物，FR-619—621、ADR-032）

> 对等判据为业务能力、权限检查、状态迁移、错误语义与可审计性五维；视觉、交互形态、
> 视图数量、按键与布局**不**作为对等判据（ADR-009、ADR-032）。新能力必须在同一 PR 内
> 定义 API、CLI 行为、权限与测试（FR-610）。

**单一事实源**：`whcode-app::surface::SURFACES` 是能力面的唯一定义；本表是它的**受校验
产物**——行序与登记顺序一致，任一行手工改动（状态符号、理由码、里程碑）都会使
`tests/contract/test_frontend_parity.rs` 失败（FR-619—621、D-17）。

**符号**：✅ 已实现（前端锚点在登记表中）；◐ 已排除（理由码见“依据／理由”列）；
⏳ 预留（里程碑见“依据／理由”列）；— 不适用（判定依据见登记表条目注释）。

**最后更新**：2026-10-26（T167 收口：`llm.run`／`llm.cancel` 的 TUI 翻转为已实现
——`L`／`C` 键位，剖面名→提示词两步输入，经 `llm_provider::TuiLlmExecutor` 真实编排；
三端 CUI／TUI／Web 均 `Implemented`，GUI 保持只读排除；T190：新增证书库设置两行）。
依据：`specs/001-whcode-harness/contracts/app-service.md` §2、`crates/whcode-bin/src/cui_dispatch.rs`、
`crates/whcode-app/src/surface.rs`、`crates/whcode-bin/src/llm_provider.rs`。

| SurfaceId | 类别 | 效果 | CUI | TUI | GUI | Web | 依据／理由 |
|---|---|---|---|---|---|---|---|
| `session.new` | Command | Write | ✅ | ✅ | ◐ | ✅ | FR-101；◐ gui_read_only_v1 |
| `session.list` | Command | Read | ✅ | ✅ | ✅ | ✅ | FR-101 |
| `session.show` | Command | Read | ✅ | ✅ | ✅ | ✅ | FR-101 |
| `session.rename` | Command | Write | ✅ | ✅ | ◐ | ✅ | FR-101；◐ gui_read_only_v1 |
| `session.archive` | Command | Write | ✅ | ✅ | ◐ | ✅ | FR-101；◐ gui_read_only_v1 |
| `session.delete` | Command | Destructive | ✅ | ✅ | ◐ | ✅ | FR-101／509；◐ gui_read_only_v1 |
| `session.switch` | Command | Read | ⏳ | ⏳ | ⏳ | ⏳ | FR-102；⏳ 未排期 |
| `task.list` | Command | Read | ✅ | ✅ | ✅ | ✅ | FR-103／104 |
| `task.overview` | Command | Read | ✅ | ✅ | ✅ | ✅ | FR-103／104 |
| `task.show` | Command | Read | ✅ | ◐ | ◐ | ◐ | FR-103／104；◐ view_summary_only |
| `task.retry` | Command | Write | ✅ | ✅ | ◐ | ✅ | FR-401／408；◐ gui_read_only_v1 |
| `task.abort` | Command | Write | ✅ | ✅ | ◐ | ✅ | FR-401／408；◐ gui_read_only_v1 |
| `context.budget` | Command | Read | ✅ | ✅ | ✅ | ✅ | FR-105／108 |
| `events.list` | Command | Read | ✅ | ✅ | ✅ | ✅ | FR-202 |
| `events.export` | Command | Write | ✅ | ◐ | ◐ | ✅ | FR-205；◐ command_output_only |
| `events.show` | Command | Read | ⏳ | ⏳ | ◐ | ⏳ | FR-202；⏳ 未排期；◐ gui_read_only_v1 |
| `spec.status` | Command | Read | ✅ | ✅ | ✅ | ✅ | FR-301—314 |
| `spec.next` | Command | Write | ⏳ | ⏳ | ◐ | ⏳ | FR-301—314；⏳ 未排期；◐ gui_read_only_v1 |
| `spec.validate` | Command | Read | ⏳ | ⏳ | ◐ | ⏳ | FR-301—314；⏳ 未排期；◐ gui_read_only_v1 |
| `spec.analyze` | Command | Read | ⏳ | ⏳ | ◐ | ⏳ | FR-301—314；⏳ 未排期；◐ gui_read_only_v1 |
| `spec.converge` | Command | Write | ⏳ | ⏳ | ◐ | ⏳ | FR-301—314；⏳ 未排期；◐ gui_read_only_v1 |
| `spec.force-advance` | Command | Write | ⏳ | ⏳ | ◐ | ⏳ | FR-301—314；⏳ 未排期；◐ gui_read_only_v1 |
| `db.purge` | Command | Destructive | ✅ | ✅ | ◐ | ✅ | FR-507—510；◐ gui_read_only_v1 |
| `db.stats` | Command | Read | ✅ | ◐ | ◐ | ◐ | FR-507—510；◐ cli_local_admin |
| `db.check` | Command | Read | ✅ | ◐ | ◐ | ◐ | FR-507—510；◐ cli_local_admin |
| `db.compact` | Command | Destructive | ✅ | ◐ | ◐ | ◐ | FR-507—510；◐ cli_local_admin |
| `db.archive` | Command | Write | ✅ | ◐ | ◐ | ◐ | FR-507—510；◐ cli_local_admin |
| `db.restore` | Command | Destructive | ✅ | ◐ | ◐ | ◐ | FR-507—510；◐ cli_local_admin |
| `db.checkpoint` | Command | Write | ✅ | ◐ | ◐ | ◐ | FR-507—510；◐ cli_local_admin |
| `db.explain` | Command | Read | ✅ | ◐ | ◐ | ◐ | FR-507—510；◐ cli_local_admin |
| `runtime.list` | Command | Read | ✅ | ◐ | ◐ | ✅ | FR-701—710；◐ six_view_scope |
| `runtime.status` | Command | Read | ⏳ | ⏳ | ◐ | ⏳ | FR-701—710；⏳ M8；◐ six_view_scope |
| `runtime.enable` | Command | Write | ⏳ | ⏳ | ◐ | ⏳ | FR-701—710；⏳ M8；◐ six_view_scope |
| `runtime.disable` | Command | Write | ⏳ | ⏳ | ◐ | ⏳ | FR-701—710；⏳ M8；◐ six_view_scope |
| `runtime.install` | Command | Write | ⏳ | ⏳ | ◐ | ⏳ | FR-701—710；⏳ M8；◐ six_view_scope |
| `llm.run` | Command | Write | ✅ | ✅ | ◐ | ✅ | FR-201—208；T167 接线（CUI／Web／TUI 三端，TUI 剖面→提示词两步输入）；◐ gui_read_only_v1 |
| `llm.cancel` | Command | Write | ✅ | ✅ | ◐ | ✅ | FR-201—208；T167 接线（CUI／Web／TUI 三端）；◐ gui_read_only_v1 |
| `theme.switch` | Command | Read | ◐ | ✅ | ✅ | ✅ | FR-612／713／625；◐ cui_no_theme |
| `locale` | Command | Read | ✅ | ✅ | ✅ | ✅ | FR-615／625 |
| `settings.cert.read` | View | Read | ◐ | ◐ | ◐ | ✅ | FR-626／627、ADR-040 决策 4；◐ web_settings_only、six_view_scope |
| `settings.cert.write` | Command | Destructive | ◐ | ◐ | ◐ | ✅ | FR-626／627、ADR-040 决策 4；◐ web_settings_only、gui_read_only_v1 |
| `font.install` | Command | Write | ✅ | ◐ | ◐ | ◐ | FR-717；◐ cli_local_admin |
| `font.status` | Command | Read | ✅ | ◐ | ◐ | ◐ | FR-717；◐ cli_local_admin |
| `font.uninstall` | Command | Destructive | ✅ | ◐ | ◐ | ◐ | FR-717；◐ cli_local_admin |
| `serve` | Command | Write | ✅ | ◐ | ◐ | ◐ | FR-603／607；◐ cli_local_admin |
| `doctor` | Command | Read | ✅ | ◐ | ◐ | ◐ | §10；◐ cli_local_admin |
| `bench` | Command | Read | ✅ | ◐ | ◐ | ◐ | NFR-210；◐ cli_local_admin |
| `app.quit` | Command | Read | — | ✅ | ✅ | — | FR-601；— 非命令面形态 |
| `ssh.transport` | Command | Read | — | — | — | — | FR-607／608；— transport_specific |
| `view.sessions` | View | Read | ◐ | ✅ | ✅ | ✅ | FR-601／622；◐ cui_command_surface |
| `view.tasks` | View | Read | ◐ | ✅ | ✅ | ✅ | FR-601／622；◐ cui_command_surface |
| `view.context` | View | Read | ◐ | ✅ | ✅ | ✅ | FR-601／622；◐ cui_command_surface |
| `view.events` | View | Read | ◐ | ✅ | ✅ | ✅ | FR-601／622；◐ cui_command_surface |
| `view.spec` | View | Read | ◐ | ✅ | ✅ | ✅ | FR-601／622；◐ cui_command_surface |
| `view.diff` | View | Read | ◐ | ✅ | ✅ | ✅ | FR-601／622；◐ cui_command_surface |
| `view.changes` | View | Read | ◐ | ✅ | ◐ | ✅ | FR-605／624；◐ cui_request_only、gui_manual_refresh |

**权限**：全部命令经 `whcode-app::authz` 统一判定（拒绝 `permission_denied` 不可重试，
FR-604）；四端不得自行推断权限。

**统一不可用语义**：登记为预留或排除的能力在该端被调用时返回结构化 `not_supported`
（`retryable = false`，`details` 含 `surface_id`、`state` 与 `milestone` 或 `reason_code`）；
CUI 退出码 **55**、Web **HTTP 501**；未登记路径仍为 404（ADR-032 决策 4）。

**实现现状摘要（2026-10-25）**：Web 读命令为会话列表、任务总览与活跃任务、事件列表、
状态快照、上下文、diff、运行时清单、i18n 与证书库状态／CA 下载／连通性校验；写命令经
`POST /api/sessions/*`、`/api/tasks/*`、`/api/purge`（预览默认 + 确认执行）与
`POST /api/certs/*`（档位、根证书、服务端证书；降档须确认、成功写 `tls_config_change` 审计）；
四套主题经 `data-theme` 运行时切换；
无 JS 时首页服务端直出只读会话快照（HTML 转义、超限写明截断；仅本机回环未认证模式）。
`/settings/certs` 设置页已交付（服务端直出 + 原生 ESM `settings.js`，纳入前端预算）：
档位、根证书与服务端证书编辑、下载、重新生成与连通性校验均走 `/api/certs/*`；
无 JS 时页面显示只读状态摘要与证书库文件路径（HTML 转义）。
TUI 补写操作（`n`／`m`／`d`／`A`、`R`／`X`、`p`，破坏性命令先预览后确认）、`t` 四主题循环
与 ≥1000 ms 低频游标检查（推进才重绘）。GUI 只读六视图，写命令登记 `GUI_READ_ONLY_V1`。
`llm.run`／`llm.cancel` 的 CUI（`cui_dispatch::llm_command`）、Web（`POST /api/llm/*`）
与 TUI（`L`／`C` 键位 → `llm_provider::TuiLlmExecutor`，剖面名→提示词两步输入，
会话取当前选中）已接线：权限、会话、剖面、凭据与取消在 `whcode_app::llm` 统一编排；
三端均为同步等待完成（同语义）；真实 HTTP 传输由
`whcode-llm::http::client` 承担（T163—T166），装配根按工作区出站信任装配，未启用
`http` feature 时返回结构化 `not_supported`，不伪造调用成功。
部署边界：仅回环或局域网（RFC1918，认证必填，ADR-023）。

**测试映射**：`tests/contract/test_frontend_parity.rs`（三向一致 + 矩阵逐行比对 + 负向篡改
自测 + 行为等价夹具）；CUI 视图测试（信封 schema_version + 脱敏注明）；
`test_json_envelope.rs`（T089）、`test_tui_cui_parity.rs`（T088）、
`test_web_security_defaults.rs`（T095）。

**历史说明**：本表此前为手工维护的“域／命令行”状态快照，曾出现同一文件头部称 Web 写命令
已接入而表格行仍标 ⛔ 的自相矛盾（存续数周未被评审发现）。该事实是 ADR-032 把矩阵改为
受校验产物的直接依据；手工矩阵不再保留，任何状态变更只改登记表并同 PR 重新生成本表。
