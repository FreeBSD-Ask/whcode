# ADR-038：会话归档的确认与审计口径

- **状态**：已批准（2026-10-25，用户指令“补遗 `session.archive` 确认与审计口径冲突”；
  裁决方向为**不放宽既有契约**，以实现补齐登记要求）
- **来源**：`CHANGELOG.md` 已知未完成第 3 项（2026-10-24 登记）给出的两种出口——“补确认与
  审计，或修订契约与登记口径”；本 ADR 选择前者
- **责任人**：四端写命令维护者、应用服务与存储维护者
- **退出条件**：归档若出现批量自动化场景（一次归档 ≥ 100 个会话）导致确认成本不可接受，
  或审计行占用成为可测量的存储压力，须重新评估本 ADR 并给出代价证据

## 背景

2026-10-24 四端对等收口复核时发现三处表述互不一致：

| 来源 | 表述 |
|---|---|
| `contracts/app-service.md` §2-1 | 破坏性命令（delete／archive／purge／compact／force-advance）必须支持 dry-run 或确认机制，**并写审计** |
| `whcode-app::surface` 登记表 | `session.archive`：效果 `Write`，确认 `PreviewConfirm`，`audited = true` |
| 实现（当时） | 四端均无预览／确认；`whcode-store::set_archived` 明确“可逆元数据标记，不删数据，不写 `db_purge` 审计” |

三者不能同时为真。登记表与契约把 archive 纳入破坏族并要求审计，实现两者皆无；
`CHANGELOG.md` 第 3 项记录了该冲突与退出条件，未擅自改动已验收行为。

## 决策

1. **契约不降级**：`session.archive` 维持契约 §2-1 的破坏族地位，确认与审计两项都不省略。
   理由：归档会改变会话在默认列表中的可见性（`sessions.archived = TRUE` 不参与活跃尾部），
   这是用户可观察的状态迁移；契约把“可逆”与“无需留痕”区分开，本 ADR 不把可逆当作豁免理由。
2. **确认口径四端对齐**（`Confirmation::PreviewConfirm` 落实为“预览后确认”）：
   - CUI：`session archive` 默认 dry-run，输出 `dry_run=true`、`current_archived`、`archived`、
     `changed` 与目标说明；`--yes` 才执行；`--dry-run` 与 `--yes` 互斥（沿用 `confirm_flags`）。
   - TUI：`A` 先执行预览段（`confirm=false`），状态行显示“当前 → 目标”，按 `y` 才执行。
   - Web：`POST /api/sessions/archive` 缺 `confirm` 或为 `false` 时只返回预览 JSON；
     `confirm: true` 才更新。前端改为两段按钮（归档预览 → 确认归档），
     确认按钮在预览成功后才启用，并记住预览目标，避免误归档其他会话。
   - GUI：维持 `GUI_READ_ONLY_V1` 只读排除（FR-711），不新增写面。
3. **审计口径**：新增 `whcode_domain::AuditAction::SessionArchive`（稳定名 `session_archive`），
   由 `whcode-store::set_archived` 在**同一事务**内写入，审计失败则整体回滚：
   - `target`：`sessions by id (n) [sessions] (archive|unarchive)`，方向显式；
   - `detail_json`：`archived`、`updated`、`sessions`（复用既有 `AUDIT_SESSION_LIST_CAP = 100`
     截断口径，超出标 `truncated` 与 `total`）；
   - 每次成功更新写一条——重复请求同样留痕，不静默吞掉用户动作。
   不写 `db_purge`：归档不删除任何行，审计动作必须按真实语义命名，混用会让清理审计失真。
4. **反向操作**：CUI `--unarchive`、Web `archived: false` 均走同一门面与同一审计路径
   （方向记为 `unarchive`）。TUI 暂未提供反归档键——登记为已知不对称（见“未完成”），
   不得因此把 TUI 归档登记为“可逆完成”。
5. **CLI 行为变更**：`session archive` 由“直接执行”改为“默认预览、`--yes` 执行”，与
   `session delete` 同构。脚本若依赖旧行为必须显式加 `--yes`；CHANGELOG 标 BREAKING。

## 证据

实现位置：

- `crates/whcode-domain/src/audit.rs`：`AuditAction::SessionArchive`（`session_archive`）。
- `crates/whcode-store/src/purge.rs`：`set_archived(conn, ids, archived, actor)` 同一事务写审计；
  `session_list_json` 复用 100 条上限与截断标记。
- `crates/whcode-app/src/session.rs`：`set_session_archived` 透传 `actor.id`。
- `crates/whcode-app/src/commands.rs`：`FrontendCommand::SessionArchive { confirm }` 与
  `CommandOutcome::SessionArchivePreview { session, target_archived }`。
- `crates/whcode-bin/src/cui_dispatch.rs`、`crates/whcode-tui/src/lib.rs`、
  `crates/whcode-web/src/api.rs` 与 `frontend/{index.html,app.js}`：四端预览／确认接线。
- `crates/whcode-app/src/surface.rs`：`session.archive` 条目注释更新为已交付口径
  （确认 `PreviewConfirm`、`audited = true` 与实现一致）。

实测命令与结果（2026-10-25，Windows 11 amd64）：

| 命令 | 结果 |
|---|---|
| `cargo test -p whcode-domain -p whcode-store -p whcode-app` | 全绿；新增 `set_archived_flips_flag_audits_and_missing_ids_rejected`、`archive_preview_then_confirm_writes_audit` |
| `cargo test -p whcode-bin` | 50 passed；`session_lifecycle_commands_execute` 覆盖归档 dry-run → `--yes` |
| `cargo test -p whcode-web` | 16 passed；`session_write_commands_default_to_preview` 覆盖缺 `confirm` 只预览 |
| `cargo test -p whcode-tui` | 61 passed；新增 `archive_requires_preview_then_confirm`（`A` → 预览 → `y` 执行） |
| `python .github/scripts/benchmarks/check-frontend-budget.py` | 通过（JS／CSS gzip 预算内，Web 新增一个按钮与四个文案键） |

负向覆盖：缺失会话仍 `not_found` 且不写审计；预览不写库不写审计；权限拒绝不受确认流程影响
（门面前置 `require`）；Web `archived` 非布尔仍 400。

## 正负和解

- **正面**：登记表与契约、实现三者对齐，`audited = true` 从声称变为可验证事实；归档与取消
  归档都有不可否认的 actor 与时间戳；CUI／TUI／Web 的破坏性写命令语义统一，FR-623 的
  “相同 dry-run／确认语义”不再有例外。
- **负面**：CUI 属**行为变更**（旧脚本不带 `--yes` 会得到预览而非执行）；TUI 与 Web 各多一步
  交互；每次归档新增一行审计，长期归档操作频繁时审计表会增长；`set_archived` 增加 `actor`
  参数，调用方需同步。
- **承担者**：CUI 脚本使用者（迁移成本）、四端交互维护者（多一步确认）、存储维护者
  （审计保留策略）。
- **和解理由**：归档改变用户可见状态，且契约与登记表已把审计列为强制；以一次性迁移成本
  换取“四端同语义 + 可审计”，比修改契约降低要求更符合项目“安全优先、不静默”的既有和解。

## 三重时间

- **过去来源**：`contracts/app-service.md` §2-1（2026-10-20 起草，2026-10-24 增补四端登记）、
  `spec.md` FR-623（写入面一致，含“破坏性命令不得无预览无确认”）、FR-509 的
  “预览—确认—审计”范式（清理链已在 `execute_purge` 实现）。
- **现在证据**：上文实测表与代码位置；`CHANGELOG.md` 的记录与取消归档路径
  （`whcode session archive --unarchive`、`POST /api/sessions/archive {archived:false}`）。
- **将来失效条件**：出现批量归档自动化（确认成本超过收益）、审计行增长成为可测量存储压力、
  或 TUI 补上反归档动作后“可逆不对称”消失但仍不提供确认——任一发生时本 ADR 须复审。

## 0 信任校验

- **查证（一手）**：契约文本、登记表条目、store 实现注释与 `Execute_purge` 的审计写法均从
  仓库直接读取，未依赖二手总结。
- **交叉验证**：同一语义由四个独立证据互证——契约条款、登记字段、三端测试断言、审计表
  实际行（测试内 `SELECT action FROM audit`）。
- **判定**：冲突为真（登记与实现不一致），已按“补实现”关闭；残留缺口只有 TUI 无反归档键，
  显式保留而非默认通过。

## 未完成（显式登记）

1. **TUI 反归档键**：TUI 只有 `A`（归档），没有反向动作；用户可经 CUI `--unarchive` 或 Web
   `archived: false` 还原。退出条件：TUI 增加反归档键并接入同一门面与审计（同 PR 更新
   `docs/guides/tui.md` 与登记表锚点）。
2. **批量归档的确认形态**：当前门面一次一条会话；若将来支持批量，须先给出确认与审计
   的批量语义（单条审计含清单上限 100 与截断标记已预留）。

## 复审触发

- 归档操作频率与审计行增长达到可测量规模；
- 任一端把归档移出破坏族或去掉确认（必须先修订本 ADR 与契约）；
- TUI 反归档键交付；
- 审计保留策略变更（`data-model.md` §audit）影响归档审计的可查询性。
