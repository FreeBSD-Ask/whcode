# ADR-100：steer 最小有界命令通道（turn 中途投喂）

- **状态**：已批准（2026-11-23，用户批准比较书全部建议）
- **关联**：ADR-058（turn-worker 线程与有界通道）；ADR-044（turn/step 结构）；
  `research/dsh-comparison/domain-A-agent-core-loop.md` §4.4
- **上游依据**：DSH inbox 有 `send/followup/steer/inject` 四档；其 §4.4 判定“完整四档与
  pre-step 钩子不复制，若只为 steer 可用有界命令通道实现”

## 任务认知卡

- **方向**：让用户在 turn 执行中追加一条消息，在下一步生效。
- **目标**：有界队列、满则拒绝（不阻塞、不丢弃已有）、审计可查、不引入新线程。
- **用户意图**：用户批准比较书建议；本项按域 A §4.4 的**最小**路径实现，不复制四档与钩子。
- **非目标／边界**：不做 followup/inject 档位；不做 pre-step 拦截/重写；不改 provider
  协议；不新增线程（复用 turn-worker 调用方线程）；不做跨 turn 队列（队列随 TurnRequest 生命周期）。
- **现有证据**：`TurnRequest`（`turn.rs:89`）无投喂入口；turn 主循环 `turn.rs:730`；
  `EventKind`（`event.rs:20`）无 steer 事件；DSH 的 `inbox.claim` 在每步前取输入。
- **手段（后置）**：`SteerHandle` 有界队列 → `TurnRequest.steer` → 循环每步 drain →
  `SteerAccepted` 事件 → 测试。

## 问题

turn 同步阻塞执行（最多 16 步），调用方一次给 `LlmRequest.messages`，中途无法纠偏。
用户发现方向错误只能等 turn 结束再发一条，浪费已执行的步骤与 token。

## 决策

1. 新增 `SteerHandle`：`Arc<Mutex<VecDeque<String>>>`，容量常量 `STEER_QUEUE_CAPACITY = 4`；
   `send(text)` 在满时返回 `InvalidState`（拒绝、不阻塞、不丢弃已有消息），空文本拒绝。
2. `TurnRequest` 增 `steer: Option<SteerHandle>`；`None` 时行为与改动前完全一致
   （16 个构造点补 `steer: None`）。
3. turn 主循环每步开始前 `drain_one()`：取一条（至多一条/步），前缀
   `[用户在执行中追加]` 后 push 到 `messages`（`Role::User`）——成为模型历史，由
   `TurnPersister` 在下次 flush 时落表；不单独写 messages。
4. 新增 `EventKind::SteerAccepted`（log-only，载荷 `step` 与 `text_chars`，**不含正文**），
   经既有 `emit!` 落事件，可审计且不泄露追加内容到日志。
5. `SteerHandle` 随 `TurnRequest` 借用（`&SteerHandle` 在主线程 send，turn 线程 drain），
   `Mutex` 只保护队列，不跨 awaiting，符合零 unsafe 与有界纪律。

## 理由与约束

- 为何容量 4：足以容纳少量纠偏，又不让内存随交互频次增长；满则拒绝是明确信号，
  优于静默丢弃或无限排队。
- 为何不复制 DSH 四档：followup/inject 涉及“插到哪一步”的语义分叉，pre-step 钩子需要
  动态派发，与静态分层冲突（域 A §4.4 判定）。
- 为何事件不含正文：日志/事件可能以 redacted 级别导出，追加内容属用户输入，不落独立事件。
- 承担者：app 维护者；调用方决定是否暴露 send 入口。

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | turn 中可纠偏；有界、满则拒绝；审计有事件；零新增线程 |
| 负面 | `TurnRequest` 增字段（16 处补 `None`）；每步一次 `try_lock`；追加消息出现在模型历史 |
| 承担者 | app 维护者承担字段与锁；用户承担 queue 满时需重试 |
| 和解理由 | 以极小侵入换取纠偏能力； locks 只在步边界、不跨 IO，成本低 |

## 三重时间

- **过去来源**：DSH inbox 四档（域 A）；ADR-058 的有界通道纪律；ADR-044 的 turn/step。
- **现在证据**：`TurnRequest` 无入口、循环在 `turn.rs:730`、事件枚举可扩展。
- **已知限制与失效条件**：①每条消息只在下一步生效，不能插入当前正在进行的模型请求；
  ②队列随 turn 结束丢弃，未被消费的追加消息不自动转成下一 turn 的 prompt（调用方需处理）；
  ③`Mutex` 争用下 `send` 是短临界区，不做条件变量等待（无阻塞语义）。
- **复审触发**：①`cargo test -p whcode-app turn::` 失败→回退；②若需要 followup/inject 档位
  →另立 ADR（本 ADR 明确不复制）；③若实测每步 try_lock 造成可测开销→改为原子标志＋单槽。

## 影响

- 正面：turn 中纠偏；有界、可审计。
- 负面：字段扩散 16 处；每步一次短锁。
- 不做什么：不做四档、不做 pre-step 钩子、不新增线程、不跨 turn 保留队列。
