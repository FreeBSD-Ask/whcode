# ADR-098：上下文组成分解（快照 context.breakdown）

- **状态**：已批准（2026-11-23，用户批准比较书全部建议）
- **关联**：ADR-094（快照三字段）；FR-802；`contracts/state-snapshot.md` §2；
  `research/zcode-comparison` 的 Context Composition（v3.18）
- **上游依据**：ZCode `apps/zcode-cli/docs/design/v2/model/context-composition.md`——
  11 个细类别投影到 7 个粗类别，只给类别与大小不给正文，支持发送前预演与修订号通知

## 任务认知卡

- **方向**：让用户看见上下文 token 被什么占满，而不是只知道总量。
- **目标**：快照 `context` 增 `breakdown`，按消息侧类别给出 token 估算，且各类之和与
  `estimated_tokens` 严格相等；工具定义单列并以“默认注册表估算”标注。
- **用户意图**：用户批准比较书建议，要求补齐“上下文组成分解”。
- **非目标／边界**：不改 `estimated_tokens` 的既有口径（消息侧）；不做发送前预演
  （ZCode 的 draft-preview，属请求路径，不在快照）；不做修订号通知（快照本身是拉取式）；
  不引入 provider 实际计数（仍是 `estimate_tokens` 估算）。
- **现有证据**：`snapshot.rs` 的 `derive_context` 只给 `estimated_tokens`／`budget`／
  `truncated`；`context::estimate_tokens` 可用；`whcode_tools::registry_with_optional_tools
  (None, None, None)` 等价默认九件注册表（零成本、无副作用）；摘要消息以
  `[上下文摘要｜模型生成]` 开头（`compaction::messages_with_summary`）。
- **手段（后置）**：`derive_context` 内按 role 与前缀分类累加 → 工具定义单列 → 单测断言
  各类之和等于总量。

## 问题

ADR-094 让快照给出了 `estimated_tokens` 与 `truncated`，但用户无法判断“为什么这么大”。
典型场景：技能描述注入、MCP 告示、压缩摘要、长对话，各自占用比例未知，无法决定该清理
哪一侧。ZCode 的 Context Composition 正是为此提供按类别分解。

## 决策

1. `context.breakdown` 增三个数值字段（单位：估算 token）：
   - `compact_summary`：内容以 `[上下文摘要` 开头的消息（压缩摘要）；
   - `conversation`：其余消息（user／assistant／tool）；
   - `tool_schemas`：**默认九件注册表**清单序列化后的估算（近似值，见第 3 条）。
2. **可判定约束**：`compact_summary + conversation == estimated_tokens`
   （同源同函数，严格相等，单测断言）。
3. `tool_schemas` 与前两者**不构成等式**：快照是会话级投影，不知道当前 turn 实际装配的
   shell／LSP／技能／MCP 工具，故只给默认注册表估算，并在 `breakdown.note` 显式标注
   “默认九件注册表估算；turn 实际清单可能含 shell／LSP／技能／MCP，未计入”。
4. **system 提示不在快照口径内**（重要限制）：`TurnPersister::flush` 明确过滤
   `Role::System`（`turn.rs:214`），技能描述、MCP 告示、计划引导等 system 消息
   **不落 messages 表**，因此快照无法分解它们。`note` 必须同时写明这一点，
   不得让读者误以为“breakdown 之和 = 全部上下文”。
5. `breakdown` 为**新增字段**，不改变既有字段语义；`SNAPSHOT_SCHEMA_VERSION` 不递增
   （新增可选字段向后兼容，旧读者忽略之）。

## 理由与约束

- 为何不递增 schema 版本：`breakdown` 是 `context` 内部的附加键，旧读者按 `Value` 读取
  不受影响；递增会触发兼容审查与四端同步，收益不足。
- 为何不把工具定义并入等式：会迫使快照重算 turn 装配（含 MCP 连接），既昂贵又越权
  （快照只读、不触网）。
- 为何不伪造 system 分解：system 提示不落库，快照既无原文也无组装参数（workspace、
  技能清单、MCP 状态）；伪造一个估算值会违反“不伪造”条款。如实标注缺口是正确做法。
- 承担者：app 快照维护者；用户获得“哪一侧占满”的可判定视图，并明确知道 system 侧未覆盖。

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | 用户可见各类占比；技能/摘要/对话的膨胀可定位；工具定义有参考值且标注近似 |
| 负面 | 每快照一次默认注册表构造与序列化（九件，量级小）；`tool_schemas` 是近似值，可能低估实际（含 MCP 时） |
| 承担者 | app 维护者承担构造成本；用户承担近似值的不确定性（已显式标注） |
| 和解理由 | 以一次小规模构造换取可定位的构成视图；等式约束保证消息侧数字可信 |

## 三重时间

- **过去来源**：ADR-094 的 `context` 字段；ZCode Context Composition 的分类动机。
- **现在证据**：`derive_context` 当前只有三个键；`estimate_tokens` 与默认注册表均可用。
- **已知限制与失效条件**：①估算非供应商实际计数，与真实 prompt_tokens 有偏差；
  ②`tool_schemas` 只覆盖默认九件，含 MCP 时低估；③若未来引入发送前预演，本 ADR 的
  “快照内静态分解”需扩展为请求路径。
- **复审触发**：①`cargo test -p whcode-app snapshot::` 失败→回退；②契约 §2 增列
  breakdown 语义→同步文档；③若实测偏差 >20%（与供应商 usage 对比）→改口径或标注更醒目。

## 影响

- 正面：上下文构成可判定；与 ZCode 的分类思路对齐。
- 负面：每快照一次小构造；工具侧为近似。
- 不做什么：不改总量口径、不做预演、不做修订号通知、不递增 schema 版本。
