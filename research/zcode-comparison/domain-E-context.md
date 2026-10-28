# 域 E：上下文管理

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 组装 | `context/builder.ts:83-233` `ContextBuilder.build()`：①CLI 前缀（`:104-106`）②身份段（`:109-123`：customSystemPrompt / 工作流子代理互斥 / 默认 identity 含安全 IMPORTANT 行 + `# Harness` 块）③动态 system 段（`:131-183`：desktop→dynamic_behavior→session_guidance→**memory**（injectionTarget: system）→**env_info**→output_style→context_management→git_system_context）④**skills**（`:189-197`，injectionTarget: meta_user，budget 20_000 字符、描述截 250）⑤meta_user（request_user_context → current_date） | 无 prompt 组装（无 agent 循环所致） | 空白，补齐时可直接照此分层 |
| 预算/截断 | `compact/policy.ts`：`DEFAULT_COMPACT_CONTEXT_WINDOW=200_000`（`:6`）；`getAutoCompactThreshold = max(0, (window - min(maxOutput??32K, 21K)) - 13K)`（`:67-88`）默认阈值 200K−21K−13K=**166K**；触发链 disabled→not_enough_messages→circuit_breaker（连续失败≥3）→below_threshold→above_threshold（`:124-155`）；**compact 生成新 summary user 消息**（`compact-active.ts:578-599`，以 `role:"user"` + `metadata.compactBoundary` 回填 `messageHistory.replaceMessages` `:672-680`）——非单纯截断；`microcompact.ts` 仅清旧工具结果（保留最近 5 组、idle 60min、最小节省 256 token），**不产生摘要消息** | `context.rs` `load_context`：预算默认 128_000 token、页 1000 有界迭代，返回四元组（items/next_cursor/consumed_budget/truncated_reason）；`compress_context` 只记 context_windows 摘要指针，**摘要生成未接线** | WHCode 预算加载与截断原因可观测性更好；缺压缩执行 |
| 持久化 | sqlite session/message/part（sequence）+ CLI 会话库 `~/.zcode/cli/db/db.sqlite` + CLI JSONL transcript | messages/context_windows 表 + 事件流 | 相当 |

## 2. cacheHint 修正

- 每个 section 带 `stable`/`dynamic` 标记（`types.ts:54`），**仅用于排序与遥测**
  （`orderSectionsForInjection:319-334`）。
- 实际发给 provider 的 system message 中，**stable body 与 dynamic system 都统一
  `cacheControl: { type: "ephemeral" }`**（`builder.ts:267,281`）——cacheHint 并未映射为
  不同 provider 缓存档。
- **原差异书“”需细化**：cacheHint 是 section 元数据（排序/遥测），非 provider
  缓存档区分。

## 3. token 限制（`model-token-limits.ts`）

- `resolveNormalRequestMaxOutputTokens` = 模型上限 ?? 32_000（`:7-15`）。
- `resolveModelStepMaxOutputTokens` = `min(baseline, floor(window − usage − 1000))`，≤0 回退
  baseline（`:17-41`）。
- compact summary cap = `min(normal, 20_000)`（`compact-active.ts:774-783`）。

## 4. 与 WHCode 差异修正（相对原差异书）

- 原判定**基本成立**，细化：ZCode 有 WHCode 没有的三点——
  ①cacheHint/`cacheControl: ephemeral` 标记机制（但仅排序遥测）；
  ②完整压缩执行（手动/自动/响应式 + microcompact + 摘要回填）；
  ③`context-usage-log-compact.ts` 专用快照压缩（categoryBreakdown 每类只留 top-5 contributor）。
- WHCode 预算 128_000/页 1000 有界、`compress_context` 仅记指针未接线、缺压缩执行——维持。
- WHCode 侧描述本次未能读源码，保留 `[待验证]`（基于 AGENTS.md 与 ADR-044 证据维持）。

## 5. 存疑项

- `metaUserAttachments` 在 builder 层**不包裹** `<system-reminder>`（返回裸体，包裹在 runtime
  侧进行，`builder.ts:288-316`）——包裹点未在本轮文件内核实，`[待验证]`。
