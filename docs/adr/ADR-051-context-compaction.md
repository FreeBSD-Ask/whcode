# ADR-051：turn 预算压力下的请求视图折叠（上下文压缩 v1）

- **状态**：已批准（2026-10-31，建议书 proposal.md D3 排序 + 用户“继续”指令；挂
  ADR-044 决策 2——P1-1 上下文压缩的最小闭环）
- **来源**：`research/zcode-comparison/` 域 E（ZCode 压缩执行 vs 本仓“只记指针”判定）+
  FR-721/FR-107/Check 305（预算护栏既有口径）
- **责任人**：whcode-app/context 维护者
- **退出条件**：真实长会话统计显示折叠引入的上下文损失导致任务失败率上升（折叠摘要
  不足以支撑模型继续任务），或 load_context 表口径与内存估算的偏差造成误判
  （另立 ADR 升级为 LLM 摘要）

## 1. 事实与证据

1. turn 预算护栏（FR-721/Check 305）：预算压力即 `budget_exceeded` 终止——长会话
   必然撞墙，agent 无法跨长对话存活（`research/zcode-comparison/` 域 E：ZCode 有完整
   压缩执行，本仓“只记指针、摘要生成未接线”）。
2. `context.rs::compress_context`（T061/FR-107）已落 `context_windows` 指针
   （窗口边界 + summary_ref），**无任何消费者**；`load_context` 按预算做表侧窗口截断
   （`truncated_reason = budget_exhausted`）。
3. turn 的模型请求消息数组是**内存累积视图**（user 提示 + 逐轮 assistant/tool 回填），
   与表历史分离——折叠作用于此视图不破坏溯源（表历史不删不改，FR-107 口径）。
4. 消息数组的 tool 角色为纯文本（P0-1 无 tool_calls 结构字段）——折叠边界不产生
   供应商协议的 tool_calls 配对约束问题。

## 2. 决策

1. **折叠视图 = 模型请求视图**：压缩作用于 turn 内存的 `messages` 数组（下一轮发给
   模型的内容），**不改写 messages 表历史**（原文仍可查，FR-107）。
2. **确定性折叠**（无 LLM 摘要，v1）：形态为**锚点 + 全量摘要**——保留最早 1 条
   user 消息（任务锚点），其余全部折叠为一条 user 角色摘要（折叠计数 + 被折叠消息
   首行样例 ≤5 行 + 边界说明）；摘要估算上界约 75 token，配合锚点守卫与预算下限
   （256 token）构造上保证折叠后 ≤ 预算。尾部窗口留待 LLM 摘要升级（退出条件）。
3. **触发与终止语义**：预算压力（表口径 `load_context.truncated_reason` 非空）时尝试
   折叠**一次**；折叠成功 → `ContextState` 事件（`compressed=true`）落表并继续循环；
   折叠后仍超（或已折叠过再超）→ `budget_exceeded` 终止（Check 305 的“超限显式终止”
   语义保持——终止仍是显式且有据的，只是先给一次折叠机会）。
4. **指针落档**：折叠时调用既有 `compress_context` 落 `context_windows` 指针
   （summary_ref 指向折叠摘要消息的消息 ID）——既有指针表获得首个消费者。
5. **一次性**：每个 turn 至多折叠一次（`compressed_once` 标记）——防止“折叠→增长→
   再折叠“的循环增长；再撞预算即终止。
6. **LLM 摘要显式排除**（v1）：真摘要需 turn 内嵌套模型调用（成本与复杂度），且
   机械摘要在“保留尾部窗口”语义下信息损失可控；升级路径登记为退出条件。

## 3. 正负和解

- **正面**：长会话可存活（预算压力从“死”变“折叠后继续”）；既有指针表获得消费者；
  确定性可测；零额外模型成本。
- **负面**：折叠丢失中段细节（模型可能遗忘被折叠轮次的内容——摘要只保留首行）；
  每会话至多一次折叠，多次撞墙仍终止。
- **承担者**：长任务用户（上下文损失）；whcode-app 维护者（折叠逻辑）。
- **和解理由**：v1 用最小代价把“必死”变“可活”；损失边界显式（事件可查折叠范围）；
  LLM 摘要等真实矛盾（折叠摘要不足导致任务失败）出现再立项（退出条件）。

## 4. 三重时间

- **过去来源**：ZCode 域 E 压缩执行（summary user 消息回填 + microcompact）；
  FR-107/FR-721/Check 305 既有口径。
- **现在证据**：`compress_context` 零消费者（Grep 实测）；turn 预算终止路径的测试
  （R1 批次 budget_exceeded 用例）。
- **将来失效条件**：见头部退出条件。

## 5. 复审触发

- `cargo test -p whcode-app context` 与 turn 测试随每批提交；
- 真实会话统计：`ContextState(compressed=true)` 事件后仍以 `budget_exceeded` 终止的
  比例 > 30% → 触发 LLM 摘要升级评估（退出条件）。

## 6. 实施摘要

- 本批：FR-731 登记、T224（折叠函数）/T225（turn 接线+事件）/T226（测试）、
  Check 320—322；实现落 `context.rs::fold_messages_for_budget` + `turn.rs` 预算分支。

## 7. 修订记录

- **2026-10-31（首版，已批准）**：D3 排序执行；编号 051（050 已被流式重试占用）。

## 8. 修订（2026-11-08）：LLM 摘要接线（T265／FR-755／ADR-060 项 B）

**决策 6 被取代**：v1 的“LLM 摘要显式排除”是成本和解，不是终态判定；ADR-059 全量采纳
proposal-v3 P1.2、ADR-060 项 B 批准后，摘要升级由退出条件转为正式契约。

1. **先摘要后折叠**：预算压力下**先**做一次模型摘要调用（`whcode-app::compaction::summarize`）：
   - 独立 `request_id`（`<turn request_id>-cmp-<iteration>`）与 `purpose=compaction` 载荷；
   - 显式 `max_tokens`（`COMPACTION_SUMMARY_MAX_TOKENS = 1024`），不借用 turn 的生成预算；
   - 与 turn 共用取消令牌；单次失败即回退，**不重试**（避免与 ADR-050 叠加）。
2. **三事件先开后关**：`CompactionStarted` 在摘要调用前**单独提交**（崩溃即留孤儿锁），
   `CompactionSummary`（正文、provider/model/usage、截断标记）与 `CompactionFinished`
   （实际采用的策略、失败码、跳过原因、耗时）随后落表。孤儿锁由
   `whcode_store::turn::orphan_compactions` 按 `request_id` 配对检测，下一个 turn 以
   `ContextState(orphan_compactions=N)` 显式呈现。
3. **摘要不是构造性预算保证**：模型输出长度不可预知，装配后仍超预算 → 记录
   `summary_skipped=summary_over_budget` 并回退结构折叠；摘要正文仍可查（“可查”与
   “被采用”分开记账）。
4. **有界**：送入摘要的转写（头部 1/3 + 尾部 2/3，中段留标记）与取回的摘要正文各有字符
   上限，超限截断并标 `truncated`；超限只停止累积、不取消共享令牌（摘要超限不得终止 turn）。
5. **尚未实现**：`skill` 工具式按需摘要、跨 turn 摘要复用、多级摘要——均不在本修订范围。

## 9. 已知限制与失效条件

- **限制**：摘要质量由模型决定，本仓不校验摘要是否忠实（只校验非空、长度、上限）——
  摘要失真属“模型输出不可信”的一般风险，回退路径存在但不是自动判定。
- **限制**：每次压缩多一次模型调用（费用与延迟）；`MAX_COMPACTION_INPUT_CHARS` 的子集
  取舍（头部 1/3 + 尾部 2/3）对超长会话可能丢弃关键中段信息。
- **失效条件**（可复现判定）：
  1. `cargo test -p whcode-app compaction` / `turn::tests` 失败 → 本修订的契约已漂移；
  2. 真实会话中摘要调用失败率 > 20%（`CompactionFinished.failure_code` 计数）→ 回退策略
     与模型/剖面选择复审；
  3. `events` 表中 `CompactionSummary` 体积占比 > 事件总量 30% → 摘要上限与保留策略复审；
  4. 摘要被采用后仍以 `budget_exceeded` 终止的比例 > 30% → 摘要预算或装配口径复审。
