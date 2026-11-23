# ADR-097：准入工具清单按步重算（MCP 失效即时摘除）

- **状态**：已批准（2026-11-23，用户批准“其他建议全部批准”；对应 `research/dsh-comparison`
  的“准入工具集按步失效”）
- **关联**：ADR-044（turn 循环）、ADR-048（工具系统）、ADR-054（MCP per-turn 连接）、
  FR-736/737；`research/dsh-comparison/actionable-gaps.md` 的 2026-11-23 跨书交叉
- **上游依据**：DSH `preStep()` 每步 `tools.resolve()`（`agent.ts:289`—`292`，本步
  `finally` dispose）；ZCode `prompt-snapshot.md` 明确“工具列表不记录，每请求从当前
  registry 准备”（MCP 中途连接会改变工具集）

## 任务认知卡

- **方向**：让 turn 内每一步的 `tools` 清单反映当步真实可用的工具，而不是 turn 开始时的快照。
- **目标**：MCP 会话失效（`is_broken()`）后，其工具不再出现在后续步的 `tools` 中；内置工具清单不受影响。
- **用户意图**：用户批准比较书建议，要求按参照实现补齐“准入工具集按步失效”。
- **非目标／边界**：不新增主动存活探测（不轮询子进程、不发 ping）；不做披露分级
  （`none/name/summary/full`，属 DSH proposed，未实现，不在本 ADR）；不改内置工具注册表；
  不改审批语义（deny 仍按调用检查，不据此摘除工具）。
- **现有证据**：`turn.rs:516` 装配一次 `tool_manifest`，循环内 `:959` 克隆使用；
  `McpSession::is_broken()`（`session.rs:529`）已存在，`broken` 在传输失败时置位
  （`session.rs:851`／`:886`）；`McpManager::tool_manifest(&self)` 当前不检查 broken。
- **手段（后置）**：`tool_manifest` 过滤 broken 服务器 → turn 循环每步重算 → 失效时追加
  system 告示 → 测试。

## 问题

turn 开始时装配一次 `tools`，循环内每步复用同一快照。若某 MCP 服务器在步 2 的调用中失败
（传输错误置 `broken`），步 3 的 `tools` 仍包含该服务器的全部工具定义——模型会继续调用
必然失败的工具，浪费一次以上模型往返与 token，且失败原因对模型不可见。

## 决策

1. `McpManager::tool_manifest(&self)` 跳过 `server.session.is_broken()` 的服务器；
   被跳过者不产生清单条目，也不计入截断预算。
2. `turn.rs` 在每步组装 `LlmRequest` 前重算清单：内置部分由注册表清单克隆（无查询成本），
   MCP 部分调用 `mcp_manager.tool_manifest()`（一次 BTreeMap 遍历＋序列化）。
3. 若某步的清单相对上一步减少了服务器，追加一条 system 消息告示（与 ADR-054 的连接失败
   告示同构，不静默）：`- 服务器 <name> 会话已失效，其工具自本步起不再提供`。
4. 内置工具清单仍按 turn 开始时的注册表快照（注册表在 turn 内不变化，无重解析必要）。

## 理由与约束

- 为何不主动探测：`Child::try_wait` 需 `&mut` 且对 HTTP transport 无意义；`broken` 已由
  传输层在真实失败时置位，属“事件驱动”而非“轮询”，符合资源铁律（空闲零占用）。
- 为何每步重算而非仅检查变化：清单是 `LlmRequest.tools` 的直接输入，重算才能保证一致；
  成本为一次序列化（内置工具数十条，量级与既有每步消息组装相当）。
- 与 DSH 的差异：DSH 的准入集还承载“授权/可达性”，WHCode 的授权在调用层检查（ADR-064），
  故本 ADR 只做“可用性”维度，不引入授权快照。
- 承担者：app／mcp 维护者；模型侧收益是少调用必然失败的工具。

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | 失效 MCP 工具即时退出清单，避免模型反复调用失败工具；与两个参照实现的行为对齐；失效对模型可见（system 告示） |
| 负面 | 每步一次清单重算与序列化；清单在步间可能变化，provider prompt cache 前缀随之失效（工具确实变了，属正确行为） |
| 承担者 | app 维护者承担重算开销；用户承担极小的每步 CPU 成本 |
| 和解理由 | 以一次 BTreeMap 遍历换掉“模型调用必然失败的工具”，净收益为正；且不引入轮询与新增线程 |

## 三重时间

- **过去来源**：ADR-044 的 turn/step 结构；ADR-054 的 per-turn MCP 连接与告示机制；
  DSH／ZCode 的每步／每请求解析证据。
- **现在证据**：`turn.rs:516` 快照装配；`session.rs:529` 的 `is_broken()` 已存在且未被
  清单使用。
- **已知限制与失效条件**：①`broken` 只在一次真实调用失败后置位——服务器崩溃但未被调用时
  仍留在清单，属“无主动探测”的有意取舍；②HTTP transport 的 `broken` 由请求失败置位，
  同样不主动探测；③若未来引入工具披露分级，本 ADR 的“全量清单”假设需重审。
- **复审触发**：①`cargo test -p whcode-app turn::` 或 `-p whcode-mcp` 失败→回退；
  ②引入工具披露分级或授权快照→重审第 2 条；③每步重算被测量为显著开销（>1% turn 时延）
  →改为“仅在 broken 集合变化时重建”。

## 影响

- 正面：MCP 失效即时反映到模型可见的工具清单；失败原因显式告示。
- 负面：每步重算；清单变化导致 prompt cache 前缀失效。
- 不做什么：不主动探测、不做披露分级、不改审批语义、不改内置注册表。
