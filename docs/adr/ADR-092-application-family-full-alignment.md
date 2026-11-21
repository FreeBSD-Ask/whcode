# ADR-092：应用族剩余差异全量对齐——12 面放宽与形态登记修订

- **状态**：已批准（2026-11-21，用户在选项中明确选择“放宽全部剩余面”，并连续指令“请继续执行任务”）
- **编号认领**：ADR-092（090／091 之后无占用）；关联 FR-619—621、FR-711／714、FR-797／799；承接 ADR-032／056／089／091 与判定报告 `artifacts/gui-web-parity-assessment-2026-11-21.md`
- **关联**：ADR-026（原生铁律不变）、ADR-027（资源预算不变）、ADR-040（证书决策修订）、ADR-089（非目标修订）

## 任务认知卡

- **方向**：把 Web 已实现而 GUI 仍标“部分”的 12 面按风险分级收敛或正名，使应用族（Web／GUI）差异只剩真实形态差异。
- **目标**：12 面中可无损收敛者翻转为已实现；受 FR-714 等硬约束者改为如实登记为合法 GUI 形态；每面有真实锚点、正向与失败路径测试、全链门禁。
- **用户意图**：用户在“放宽全部剩余面”选项中确认逐项修订 ADR-026／040／089 与 FR-711／714；连续“继续”视为授权自主推进，带回结果而非再次确认。
- **非目标／边界**：不触碰 ADR-026 原生铁律（禁止浏览器套壳）；不突破 ADR-027 资源预算（RSS ≤96 MB／线程 ≤24／句柄 ≤256／CPU ≤0.2%）；不引入周期定时器（FR-714 保持）；不伪造无真实锚点的“已实现”。
- **现有证据**：判定报告 16 面逐项表；ADR-091 已收敛 4 面；`commands.rs` 已有 `SpecNext`／`SpecForceAdvance` 但无 runtime／rules／export 变体；`ViewSource` 无 runtime 接口；`view.changes` 的 `r` 键刷新已实现却被登记为排除。
- **手段（后置）**：分三批——登记正名与现成门面接线 → runtime 族读接口与视图 → 证书与其余写面门面；每批独立取证与提交。

## 关键判定：ADR-026 铁律与本次放宽不冲突

ADR-026 的铁律是**原生渲染、禁止浏览器套壳**，其“只读 v1”口径已由 ADR-056 修订为 v1.1 交互对等。因此放宽 GUI 写面不触碰原生死线。真正需要修订的是 ADR-089 的非目标（`six_view_scope` 等不改）、ADR-040 决策 4（证书 Web-only）与 FR-711 的只读措辞。FR-714 禁止周期定时器保持有效——新增读面一律走按需查询与显式刷新。

## 12 面逐项改动表

| 面 | 当前理由码 | 效果 | 批次 | 改动 |
|---|---|---|---|---|
| `view.changes` | `gui_manual_refresh` | Read | 一 | **正名**：`r` 键显式刷新已是真实锚点，翻转为 `Implemented`（锚点 `GuiAction::Refresh`）；理由码语义改为“显式刷新为合法 GUI 形态”，FR-714 不改（仍禁定时器） |
| `spec.next` | `gui_read_only_v1` | Write | 一 | 门面 `SpecNext` 已存在；GUI 加操作面板（阶段＋workspace），复用预览/审计同源 |
| `spec.force-advance` | `gui_read_only_v1` | Write | 一 | 门面 `SpecForceAdvance` 已存在；reason 必填输入，审计同源 |
| `runtime.list` | `six_view_scope` | Read | 二 | `ViewSource` 增 runtime 读接口＋装配根实现；GUI 第 11 视图按需查询 |
| `runtime.status` | `six_view_scope` | Read | 二 | 同上，视图内按运行时查询状态 |
| `runtime.enable` | `six_view_scope` | Write | 二 | `commands.rs` 增 `FrontendCommand::RuntimeEnable`（`RuntimeManage` 能力＋审计）；GUI 视图按钮接线 |
| `runtime.disable` | `six_view_scope` | Write | 二 | 同上 |
| `runtime.install` | `six_view_scope` | Write | 二 | 同上；只接受本地 payload，哈希比对，不联网（ADR-089 限制保留） |
| `settings.rules.write` | `six_view_scope` | Write | 三 | `commands.rs` 增 `FrontendCommand::ApprovalRuleWrite`；GUI 高级面板接线 |
| `events.export` | `command_output_only` | Write | 三 | `commands.rs` 增导出命令；GUI 采用**有界文本呈现＋手动保存**（修订记录 2026-11-21：原写“原生保存对话框”，但 `whcode-gui` 与 `Cargo.lock` 均无对话框依赖，新增依赖须走 ADR 修订并复核 ADR-027 预算；故以 `CommandOutcome::EventsExported` 返回有界数据、GUI 只显摘要不回显正文，不新增依赖、不触碰 ADR-026／027） |
| `settings.cert.read` | `six_view_scope` | Read | 三 | 重审 ADR-040 决策 4；GUI 证书设置面按需读取状态（不含私钥） |
| `settings.cert.write` | `gui_read_only_v1` | Destructive | 三 | 修订 ADR-040；GUI 证书设置面经门面写，降档须确认、写审计 |

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | 应用族功能对等；`view.changes` 正名消除“已实现却被标部分”的登记不实；GUI 获运行时、证书与门禁写能力 |
| 负面 | 新增第 11 视图与 5 个 `FrontendCommand` 变体；`ViewSource` 接口扩展触及全部实现方；GUI 证书面扩大攻击面 |
| 承担者 | GUI 维护者、app 门面维护者、装配根维护者；用户承担新键位与设置面 |
| 和解理由 | 全部改动经既有命令门面与 `Capability` 判定，不新增绕过路径；资源预算与原生铁律不放松；分批可独立回退 |

## 三重时间

- **过去来源**：判定报告 16 面表；ADR-032 理由码全表；ADR-056 GUI 交互化；ADR-089 四端清零；T393／T394 的 GUI 操作面板与门禁命令先例。
- **现在证据**：`commands.rs` 变体清单；`ViewSource` 无 runtime 接口；`view.changes` 的 `r` 键已实现；FR-711／714 原文。
- **已知限制与失效条件**：①无 Anthropic／Gemini 真凭据，不涉及本批；②GUI 实机资源未测，每批后须跑 CI 探针，超 ADR-027 预算即回滚该批；③证书面写入涉及真实文件，必须保留降档确认与审计，否则回退；④运行时安装不做网络下载，保持 ADR-089 限制。
- **复审触发（绑定命令＋判定）**：
  1. `cargo test -p whcode-tests --test frontend_parity` 任一面对账失败即回滚该批登记表翻转；
  2. `python .github/scripts/benchmarks/check-frontend-budget.py` 或 CI GUI 探针超 ADR-027 预算即复审视图与字段；
  3. `cargo test -p whcode-app -p whcode-gui --lib` 失败即定位到具体面并回退；
  4. ADR-026／027 正文修订即重审本 ADR 边界。

## 回退路径

每批独立提交。回退单位为批次：登记表翻转、门面变体、GUI 视图与 i18n 在同一提交内，可用 `git revert <commit>` 整体回退，不留半翻转状态。批次一回退不影响批次二、三；批次二回退需连带撤销第 11 视图与 `ViewSource` 接口扩展。

## 分批计划

1. **批次一**：`view.changes` 正名 + `spec.next`／`spec.force-advance` 接线（现成门面，风险最低）。
2. **批次二**：runtime 六面（`ViewSource` 接口 + 第 11 视图 + 门面三变体）。
3. **批次三**：`settings.rules.write`、`events.export`、证书两面（ADR-040 重审）。

## 修订记录

### 2026-11-21：批次三前半落地与两处偏离登记（0 信任，以磁盘事实为准）

- **完成**：批次三前半（`settings.rules.write`、`events.export`）经 `FrontendCommand::
  ApprovalRuleWrite`／`EventsExport` 入 GUI；登记表与矩阵翻转、写白名单扩容、13 套 i18n
  与门面正反向测试全绿。
- **偏离 1（events.export 呈现形态）**：原表写“GUI 走原生保存对话框”，实测 `whcode-gui`
  依赖表与 `Cargo.lock` 均无对话框 crate（“原生保存对话框”作为系统集成能力见 ADR-026
  第 51 行，但本仓库未引入）。按 ADR-027 资源余量很小、新增依赖须走 ADR 修订，改为
  `CommandOutcome::EventsExported` 返回有界数据、GUI 显有界摘要并提示用户手动保存；
  不回显正文、不新增依赖、不触碰两条硬约束。上表与本记录同步修正该措辞。
- **证书两面（用户批准后完成）**：`settings.cert.read`／`settings.cert.write` 已入 GUI
  高级面板。用户明确批准 ADR-040 决策 4 修订后，装配根 `TuiCommandExecutor::
  execute_cert_write` 以 `CertPaths::for_workspace`＋`StoreAudit` 注入（不改 55 处
  `execute_command` 调用），读面经 `ViewSource::tls_status()`（`TlsStatus` 不含私钥）。
  **私钥上传 `WriteServer` 保持 Web-only**（避免凭据进入 GUI 输入控件／截图），为已知的
  部分覆盖，不伪造全能力对等；ADR-040 决策 4 正文与修订记录已同步。
