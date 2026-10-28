# 域 C：权限与确认

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 规则模型 | `apps/zcode-cli/packages/core/src/permission/service.ts`：`PermissionBehavior = "allow"\|"ask"\|"deny"`（`:63`）；`PermissionRuleValue { toolName; ruleContent? }`、`PermissionRuleset`（`contracts/src/interfaces/permission.port.ts:19-33`）；匹配键 toolName+input pattern（`matchesRuleToolName` `:299-302` 特例——Write 命中 Edit 规则；`ruleSubjects` `:318-333` 抽 command/url/file_path/path/pattern/patch_text，WebFetch 抽 `domain:<host>`；`matchesRuleContent` `:342-355` 精确/`:*` 前缀/`*` 通配） | `crates/whcode-app/src/authz.rs` Capability 11 项（session.create/read/…、spec.advance、settings.cert.read/write），`require()` 统一入口，拒绝恒 permission_denied 不可重试 | 粒度不同：ZCode 是工具输入级交互确认；WHCode 是命令级形式判权 |
| 模式策略 | `plan-mode-policy.ts:20-43`（EnterPlanMode 直接 allow；ExitPlanMode 非 plan 时 deny）；`checkPlanMode`（`service.ts:447-489`）只读+非破坏→allow、MCP 非破坏→allow、`allowedInPlanMode && sideEffectScope==="session" && !destructive && !needsApproval`→allow、否则 deny（`mode.plan.nonReadOnly`）；`checkBuildMode`（`:495-559`）readOnly→allow、critical→ask、high→ask、session+low→allow、`needsApproval||destructive||sideEffect!=="none"`→ask、否则 allow | 无工具级规则 | WHCode 缺工具级规则 + broker |
| 交互 broker | **存在**：`permission/broker.ts`。`ManualPermissionBroker`（`:41-167`）：Map pending、requestId 去重、timeoutMs/AbortSignal、resolvePermission/rejectPermission/listPendingRequests；`DenyPermissionBroker`（`:24-39`）guarded 抛错否则 deny。**四态**：`PermissionDecision = "allow"\|"deny"\|"escalate"\|"modify"`（`contracts/src/events/session.events.ts:1141`）；应答解释 `tool/executor/permission-response.ts:29-90`（deny→错误、escalate→PermissionEscalation、modify/guarded modifiedInput→**输入改写**、allow→完成） | 无交互 broker（TUI 两段确认是“”命令门面，属 UI 层不是策略层） | WHCode 缺工具级规则 + broker |
| 对等性 | 无跨端契约登记 | `surface.rs` 56 条能力登记（SurfaceEntry：effect/confirmation/audited/frontends[Support;4]），四端同源校验 + 对等契约门禁测试 | **WHCode 先手**：跨端一致性形式化程度更高 |
| 审计 | 无显式审计表（依赖事件流） | `audit` 表 11 类动作，不随 purge 删除，`AuditSink` 失败即操作不得视为成功 | **WHCode 先手** |

## 2. 兜底（修正原判定）

- `alwaysAsk` **不是全局默认**，而是**工具级声明** `alwaysAsk?: true`
  （`contracts/src/tools/contract.ts:105`），解析于 `service.ts:632`，默认 `false`。
  `checkAlwaysAsk` 返回 `tool.alwaysAsk` ask（`:421-426`），但可被 auto/disallowedTools/project
  deny 压过、可被会话 allow 放行。
- 真正的**末位兜底是 `checkBuildMode`**（`service.ts:265`）——build 模式下 lowRisk→allow、
  有副作用→ask。
- **原差异书“”应修正为“”。**

## 3. ZCode 有而 WHCode 未见的机制

- **escalate**（`PermissionEscalation`）、**modify + modifiedInput 输入改写**、
  **inputAdjustments 确认窗改参**（`permission-input-adjustments.ts:48-105`，仅工具声明
  `applyInputAdjustments` 时生效——CreateWorkflow/AmendWorkflow）。
- **会话级 “”**（仅内存，`permission.port.ts:85-87`）。
- **WebFetch 域名预批准**（`webfetch-preapproved.ts:1-115`）。
- **workflow 草稿免确认**（`workflow-draft-path.ts:60-90`）、**guarded 模式 `user-once`**。

## 4. 与 WHCode 差异修正（相对原差异书）

- 原判定“”**成立**：ZCode 有工具契约级 `permission`
  （`contract.ts:182`）+ `permissionCapabilityGroup: official_cua`（`permission.port.ts:12-17`）
  + 完整 broker port。
- 原书“”**保留 `[待验证]`**——本次子代理仅读 ZCode，
  未复核 WHCode 源码；但基于 AGENTS.md 与 ADR 证据，此先手结论维持（待 WHCode 侧复核确认）。

## 5. 存疑项

- `permission-rules.ts` 不在 `permission/` 下，实际为 `tool/executor/permission-rules.ts`
  （规则合并去重）——原书路径名需更正。
- WHCode 侧证据未重读，`[待验证]`。
