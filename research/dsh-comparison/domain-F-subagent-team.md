# 域 F：子代理与 Team

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 抽象 | `SubagentProvider` 注册表：同一接口既可创建本地子 Agent，也可启动**外部执行**（别的产品） | 无 provider 抽象；`spawn_subagent` 工具内联处理，同步嵌套 `run_turn`（`crates/whcode-app/src/turn.rs:891`—`897`） | **DSH 领先**：接口统一本地/外部 |
| 深度上限 | `maxDepth` **默认 `1`**（`0` 禁用委派、`1` 只允许直接子级；工具/调用方显式值优先）；**深度取自持久会话头且单调**，运行时只能加深不能降低（`packages/subagent/subagent/README.md` Delegation settings 节；`packages/subagent/subagent/src/depth.ts:18`—`31`） | `request.depth >= 1` 即拒绝，子代理不得再委派（`turn.rs:911`—`920`，硬编码 1 层） | **同构**：默认都是 1 层；DSH 可配置且有“单调防重置”规则 |
| 数量上限 | `maxActiveSubagents` **默认 `8`**（每棵子代理树的活跃＋物化中子代理，根不占槽；创建/冷恢复先预留，dispose 后归还）（`packages/subagent/subagent/README.md` Activation capacity 节） | `MAX_SUBAGENTS_PER_TURN = 2`（`turn.rs:49`、`:926`） | **同构**：都有硬上限；WHCode 更保守 |
| 身份 | 子 Session 有独立 id；挂载名在兄弟间唯一（1—32 小写字母/数字/连字符，保留 `lead`），缺省 `agent-N`（`docs/subsystems/subagent.md`） | 子 turn 用独立 `request_id`/`turn_id`，同一会话落表（`turn.rs:891`—`893`） | **DSH 领先**：有模型可见命名与寻址 |
| 通信 | 子代理有 inbox；`delivery: 'parent'` 通知模型，`'caller'` 只回调用方；Team 用普通 subagent 消息（`subagent.md`；`docs/subsystems/agent-team.md`） | 只回填父循环：终止原因＋末轮正文作为 `role:tool` 消息，上限 64 KiB（`turn.rs:821`、`:840`） | **DSH 领先**：双向消息 |
| 生命周期 | `SubagentActivation`：`result` 在 teardown 后结算，`dispose()` 只停这一 activation（`subagent.md`） | 同步嵌套调用，父 turn 阻塞至子 turn 结束；无独立取消 | **DSH 领先** |
| 失败语义 | 失败 reject 捕获结果；teardown 失败经 `dispose()` 上报但不替换结果（`subagent.md`） | 任何失败结构化回填，不中断父 turn、不伪造成功（`turn.rs:891`—`893`、`:840`） | **同构**：都不污染父循环 |
| Team | 共享任务板（`TeamTaskId = task-<n>`）、按需发现 eligible 属主、任务变更**不**唤醒 agent（`agent-team.md`） | 无 Team / 任务板；`tasks` 表是 SDD 任务账本，不供子代理领取 | **结构性缺失（WHCode）** |

## 2. DSH 实现解剖

### 2.1 一个接口，两类后端

`startActivation` 是唯一入口：能力标志（`agentOptions`/`agentPreset`/`outputSchema`/`depthLimit`）
在启动前校验；`prepareContinuable` 走本地 Agent 创建，`start` 走外部执行（`docs/subsystems/subagent.md`）。
“外部执行”意味着同一个 `spawn` 工具可以把活派给另一个产品，本地/远端由 provider 决定。

### 2.2 深度是持久事实，不是运行期参数

`delegationDepthOf()` 取 `max(session.header.delegationDepth, options.subagentDepth)`：
持久头是权威且单调的，运行期值只能加深（`depth.ts:11`—`31`）。这条规则防的是
“恢复的子会话带着全新 options，从 0 重新计数，然后像顶层一样继续委派”。

### 2.3 交付策略决定“谁被通知”

`delivery: 'parent'` 把结果作为父 inbox 消息（模型可见）；`'caller'` 只把结果返回给调用方
（workflow/code mode 用），不额外注入父消息（`subagent.md`）。同一份结果因此可以
“给模型看”或“只给编排器用”。

### 2.4 Team 是任务板，不是消息总线

Team 把 `TeamId` 绑定到根 Session，任务 id 为 `task-<n>`；成员用普通 subagent 消息通信，
**任务变更不结束 inbox 等待、不追加消息、不唤醒空闲 agent**（`agent-team.md`）。
这是刻意的“控制面与数据面分离”：任务板负责协调，唤醒必须显式发消息。

## 3. WHCode 现状核对

- 已实现：`spawn_subagent` 工具、两层硬上限（深度 1、每 turn 2）、结构化失败回填、
  独立 request/turn 标识（`turn.rs:49`、`:891`—`983`）。
- 已实现：父 turn 不被中断；子代理结果截断到 64 KiB（`turn.rs:821`）。
- 缺失：provider 抽象（只有本地同步嵌套）、子代理独立取消、双向消息、命名寻址、Team/任务板
  （检索 `subagent_provider`、`team`、`task_board` 于 `crates/` 无实现）。
- 差异：WHCode 子代理**同步阻塞父 turn**，因为 `run_turn` 是同步函数；DSH 子代理是并发
  activation，父子可交错。这是域 A 线程模型的直接后果。

## 4. 差异判定与可借鉴点

1. **深度单调来自持久头**：可采纳。WHCode 若将来允许子代理再委派，`depth` 必须落会话元数据，
   不能只靠 `TurnRequest.depth` 传递；否则恢复路径会重置深度。代价：加一列/一字段。
2. **`delivery` 两档**：可采纳为工具参数（`notify_parent: bool`）。当前 WHCode 只有
   “回填父消息”一档，workflow 类消费者会多一次模型可见噪声。
3. **命名与寻址**：可采纳（轻量实现）。`agent-N` 名称让父模型能指定“问哪个子代理”，
   比“按顺序读结果”更可控。代价：需要在会话内维护名字表。
4. **activation 生命周期**：不采纳（当前）。WHCode 的同步嵌套更容易证明“父返回时子已结束”；
   改成并发 activation 需要线程池与配额（NFR-211），收益不足以抵消复杂度。
5. **Team 任务板**：不采纳。WHCode 已有 SDD 任务账本，再造一个会话级任务板会产生两套
   “任务”语义；若需要，应先把 SDD 任务暴露为工具，而不是新建板。
6. **不采纳** DSH 的“上限可配置”：DSH 默认 `maxDepth=1`／`maxActiveSubagents=8`，都可被
   组合与用户设置改写；`MAX_SUBAGENTS_PER_TURN = 2`／深度 1 这类编译期常量是 WHCode
   资源铁律的正面样例，应保持不可被配置放宽。

## 5. 待验证项

1. `maxActiveSubagents` 在“每 turn 新起”与“整树存活”两种口径下的实际拒绝点：README 说
   “每棵树”，WHCode 是“每 turn”；两者计数窗口不同，不能直接换算。取证：读
   `packages/subagent/subagent/src/activation.ts` 的槽位预留与释放路径。
2. `provider-managed` 深度的语义：README 说显式工具深度（含 `provider-managed`）优先于
   `maxDepth`，未说明外部 provider 如何自报深度。取证：检索 `provider-managed` 于
   `packages/subagent/**` 与对应 provider 实现。
3. WHCode 子代理是否共享父的取消标记：`handle_spawn_subagent` 调 `run_turn` 时未传 cancel
   参数（`turn.rs:895`—`910` 签名内无 `cancel`），且 `TurnRequest` 无 cancel 字段（`:1000`
   一带的构造），因此子 turn 在父取消后仍会跑完。取证：读 `run_turn` 的 marker 创建点
   （`turn.rs:345`）确认子 turn 自建标记。

## 6. 证据路径清单

- `docs/subsystems/subagent.md`（§Capabilities and activation requests：`startActivation` 统一入口）
- `docs/subsystems/subagent.md`（§同上：命名规则与 `delivery` 两档）
- `docs/subsystems/subagent.md`（§同上：`SubagentActivation` 生命周期）
- `packages/subagent/subagent/README.md`（Delegation settings：`maxDepth` 默认 1）
- `packages/subagent/subagent/README.md`（Activation capacity：`maxActiveSubagents` 默认 8）
- `packages/subagent/subagent/src/depth.ts:18`（深度单调）
- `docs/subsystems/agent-team.md`（§Identity and historical roster：TeamId/TeamTaskId）
- `docs/subsystems/agent-team.md`（§Messages and historical mailbox：任务变更不唤醒）
- `crates/whcode-app/src/turn.rs:49`（每 turn 上限 2）
- `crates/whcode-app/src/turn.rs:891`（spawn_subagent 执行器）
- `crates/whcode-app/src/turn.rs:911`（深度上限）
- `crates/whcode-app/src/turn.rs:926`（数量上限）
