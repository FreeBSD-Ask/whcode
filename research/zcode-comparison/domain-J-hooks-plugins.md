# 域 J：Hooks 与插件

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| Hooks | `packages/contracts/src/hooks/index.ts:7-15` 事件枚举确切定义：`SessionStart / UserPromptSubmit / PreToolUse / PermissionRequest / PostToolUse / PostToolUseFailure / Stop`（`:17` 导出类型）；各输入载荷接口 `:78-139`，合并 `HookInput`（`:141-148`）；默认运行时配置 `DefaultHooksRuntimeConfig`（`:425-430`）：`enabled:false`、`maxOutputBytes:32768`、`timeoutMs:60000` | 无 hooks。但事件流 + 审计已备，可作钩子源 | 空白；**实现成本不低**（见修正） |
| runner | `core/src/hooks/`：**matcher** `output.ts:98-112 matchesHookMatcher`（`*`/空通配全匹配；纯 `[a-zA-Z0-9_\|]+` 字面量按 `\|` 拆分精确匹配；否则 `new RegExp(matcher).test`，正则失败 false）；聚合匹配 `runner-helpers.ts:36-48`（matchValues+matchValue 并集 any）；**command/process 执行** `configured-runner-callback.ts`（command 走 `mode:"shell"` `:33-34`，process 走 `mode:"argv"` `:76-77`，均经 `executionPort.run` 带 cwd/env/stdin/outputLimit 双限）；**异步** `configured-runner.ts:143`（async: command && async===true），`runner.ts:125-154 runBackgroundHook` 独立生命周期；**超时** `runner.ts:334-379 runCallbackWithTimeout`（AbortController+setTimeout，超时→`createHookTimeoutError`，父 signal 联动）；**JSON 输出可改权限/输入/阻断** `output.ts processHookOutput`（`continue:false`→blockRequested `:20-29`；`decision:"approve"\|"block"` `:34-47`；`hookSpecificOutput.PreToolUse.permissionDecision` allow/ask/deny 与 `updatedInput` `:120-128`；`PermissionRequest.decision` `:130-134`；`Stop.stopShouldContinue` `:30-33`；schema 校验 `contracts/index.ts:208-292`） | 无 | 空白 |
| 信任审批 | `contracts/src/hooks/workspace-hook-trust.ts:31-41` 状态机 `NotApplicable/PendingTrust/TrustedPersistent/BlockedUntrusted/BlockedPolicy/Revoked/StaleDigest`；状态→准入映射 `:87-130`；评审超时 10min（`:4`）；协调器 `core/.../workspace-hook-trust-coordinator.ts`（policy provider、persistent records、revoked keys、security revision `coordinatorEpoch+counter` `:138-153`）；运行时准入 `workspace-hook-runtime-admission.ts`（`evaluateDispatch` 逐条校验 digest/快照/激活/策略 `:116-191`，**fail-closed**；`pendingCount` 软门禁 `:239-255`）；评审流 `workspace-hook-review-flow.ts`（pending/resolved/superseded/timed_out/cancelled `:275-312`，snapshot 不匹配即拒绝） | 无 | 空白 |
| 插件 | **路径修正**：`apps/zcode-cli/packages/services/src/plugins/` **不存在**。实际三处——RPC 服务侧 `packages/services/src/plugins/`（根仓库 `packages/services`，非 CLI）：`pluginManagementService.ts`、`installedPluginRoots.ts`（`installed_plugins.json` 读取 `:10`，结构 `{plugins:[{id,marketplace,installPath,...}]}` `:45`，缺文件/坏 JSON 返回 `[]` fail-open）；CLI 加载侧 `packages/bootstrap/src/plugins.ts` + `packages/adapters/src/plugins/marketplace.ts`（`INSTALLED_PLUGINS_FILE="installed_plugins.json"` `:49`；`InstalledPluginRecord` `:137-150`）；UI 插件 = MCP Apps `adapters/src/plugins/ui-surfaces.ts`（manifest `ui.surfaces[]`→`PluginUiSurfaceDefinition`，`server` 须为 `mcpServers` 声明键，运行时名 `plugin:${name}:${server}` `:57`；`resourceUri` 必须 `ui://` 前缀 `:88-94`）；`contracts/index.ts:168-183` 定义 | 无 | 空白；体系重，暂缓 |

## 2. 与 WHCode 差异修正（相对原差异书）

- 原判定“”**基本成立**。
- **需修正“”**：ZCode hooks 不是简单 shell 触发——含 matcher 正则、
  command/process 双模式、async/超时、JSON 权限改写、workspace 信任审批流
  （digest+policy+revocation+review registry）、telemetry，移植需配套安全边界，
  非低价值低成本的薄层。
- **插件路径需修正**（原书路径 `services/src/plugins/` 有误，实际在根 `packages/services`）。

## 3. 存疑项

- `services/src/plugins/` 无独立 RPC 服务文件——`pluginManagementService.ts` 是接口注入，
  具体 RPC 频道注册未定位到 `[待验证]`。
- `installed_plugins.json` 的精确写入路径（atomic-directory、跨进程锁在
  `bootstrap/src/lib/plugin-storage-lock.ts` 与 `adapters/plugins/atomic-directory.ts`），
  未逐一核验原子写实现 `[待验证]`。
- WHCode 侧“”仅凭 AGENTS.md 文档证据，未读 WHCode 源码 `[待验证]`。
