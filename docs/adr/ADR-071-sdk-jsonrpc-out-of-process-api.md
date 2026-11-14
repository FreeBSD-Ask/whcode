# ADR-071：出进程 SDK（JSON-RPC over stdio）与流多路复用并发档案（T293／FR-778）

**状态**：已批准（2026-11-14，proposal-v3 P6.1／D14 批准“最小协议＋主版本升级策略”）
**日期**：2026-11-14 | **依据**：ADR-015（设计先行）、ADR-017（首个并发档案范本）、
ADR-018/030（前端只透传注入连接）、ADR-044/060（turn 编排与失败尝试）、ADR-068（世代语义）、
D14（SDK 版本承诺，不可逆决策）

本文件同时承载两件事：**SDK v1 协议决策**（版本承诺，不可逆）与 **ADR-015 要求的并发设计
档案**（流多路复用，先于代码存在）。T294 的实现必须逐项兑现 §3—§8，否则不得合并。

## 1. 问题与边界（并发档案：问题与边界）

**真实矛盾**（形式主义）：WHCode 的对外面只有四端命令＋CUI JSON 信封——外部程序想驱动
会话只能拼 CLI 文本；DSH 域 I 判定“结构性缺失（WHCode）”：DSH 有出进程 JSON-RPC
（TypeScript/Python 客户端），WHCode 不可编程驱动。

- **目标**：`whcode sdk` 子命令以 JSON-RPC 暴露最小协议——`initialize`（版本协商）、
  `session.create`／`session.open`、`turn.run`／`turn.cancel`、`events.follow`
  （游标＋世代）——错误复用 CUI JSON 信封。
- **共享什么**：工作区单连接（`Arc<Mutex<Connection>>`，serve 同款）；事件游标；活跃 turn 槽。
- **不共享**：stdout 写者（单线程独占）；各请求的解析状态。
- **约束**：不引入 async 运行时；零 `unsafe`；全部通道有界；**活跃 turn 上限 1/连接**
  （不新增常驻线程池）；Windows 11 amd64 唯一验收。

**非目标**：多语言客户端 SDK（TS/Python wheel——以契约文档代替，客户端由使用方按契约实现）；
MCP resource/instructions（P6.2）；ACP 机器审批（P6.3）；loopback TCP 传输（演进路径，不承诺）；
多 workspace 单连接；并发多 turn。

## 2. 设计决策

### 2.1 协议面（版本承诺，D14 不可逆）

- 传输：**line-delimited JSON-RPC 2.0 over stdio**（`whcode sdk`，单连接；与 MCP stdio
  同框架口径，编解码语义与 `whcode-mcp::protocol` 同形）。
- 版本：`protocol_version = "1.0"`；同主版本兼容（minor 只增不改）；破坏性变更升主版本；
  `initialize` 协商——客户端声明其主.次版本，服务端主版本不同即拒绝（错误携带服务端版本）。
- 方法面（提交即承诺，正文见 `specs/001-whcode-harness/contracts/sdk-jsonrpc.md`）：
  `initialize`／`session.create`／`session.open`／`turn.run`／`turn.cancel`／`events.follow`；
  通知：`event`（seq＋generation＋事件帧）、`generation`（世代推进提示）、`closed`（连接关闭原因）。
- 错误：JSON-RPC `error.data` 承载 CUI 信封四字段（`schema_version`／`code`／`message`／
  `retryable`／`details`）——错误口径零新增，复用既有稳定码表。
- 审批口径：v1 `turn.run` 与 CUI `llm run` 同装配口径（无交互审批闸门；无人值守的
  “换一个可审计决策者”属 P6.3 ACP，显式登记不做静默绕过）。

### 2.2 并发模型（流多路复用）

```text
stdin 读线程（主循环：逐行解析、分发）
   ├─ turn.run ──→ 活跃槽（AtomicUsize，上限 1，RAII 释放）→ turn 工作线程（跑 run_turn）
   ├─ turn.cancel ─→ 直接置 CancelToken（既有机制）
   └─ events.follow ─→ 事件泵线程（有界轮询：100 ms 增量 seq 查询）
全部出站（响应＋通知）──→ 有界通道（sync_channel，容量 256）──→ stdout 写线程（唯一写者）
```

- 会话焦点：单连接绑定**一个工作区**（启动时给定）；`session.create/open` 在其内选择会话。
- 世代（generation）＝该会话 `SessionRewound` 事件累计条数（单调不减）：原始事件按 seq
  事实流（事件只增不改，ADR-068），世代只提示“派生口径已变、派生视图需重算”——不重放、
  不改写历史。
- 背压：出站通道满 → 先写一帧 `closed`（原因＝slow_consumer）再关闭连接；客户端带
  游标重连重放（游标＋世代使重放可判定）。不丢帧、不静默丢弃、不阻塞读线程。

## 3. 不变量（逐条可判定）

1. 单会话内通知 `seq` 严格递增、无重复、无跳过（游标连续，serve 广播同口径）。
2. `generation` 单调不减；任何 `SessionRewound` 入流后，后续通知的 generation 严格 +1。
3. 至多一个活跃 turn 工作线程；槽满时 `turn.run` 立即返回 busy 错误（不排队、不静默）。
4. stdout 写者唯一；任何帧完整写出后才写下一帧（无交错损坏）。
5. 出站通道长度 ≤ 256；满即触发关闭序列（先 `closed` 帧）。
6. 每个请求恰好一个响应或一个错误响应，且 `id` 对应；通知无 `id`。
7. 全部线程在连接关闭时 join（无常驻泄漏）；通道全部有界。

## 4. 线性化点与内存序

- 事件通知线性化点：增量查询返回批次 → 逐帧入出站通道（`sync_channel` FIFO 保序）。
- turn 终态线性化点：`record_result` 成功返回 → 该 turn 的响应入通道（响应不得先于
  终态登记出现）。
- `CancelToken` 沿用既有实现（`AtomicBool` + `SeqCst`，与 turn 既有口径一致）；其余共享
  状态经通道消息传递；活跃槽用 `AtomicUsize` `fetch_add`/`fetch_sub`（`SeqCst`，评审 M3
  同款——判定与递增不可分割）。
- 不引入无锁队列与自旋（同 ADR-017 口径；无锁结构需单独档案与模型检验）。

## 5. 进展保证

- 读线程阻塞于 stdin 行读（预期 blocking；EOF 触发优雅关闭）。
- 事件泵：有界轮询，100 ms 间隔（serve 广播泵同口径）；无事件时空转推进游标不产出帧。
- turn 工作线程：完成／取消／失败三终态必达（`run_turn` 既有语义）；槽在线程退出时 RAII 释放。
- 慢消费者不影响读线程与泵线程（通道满即关闭序列，非阻塞策略）。
- 无 lock-free/无饥饿承诺：全部经有界阻塞通道，最坏情况为通道容量背压。

## 6. 数据所有权

- `Connection` 由装配根注入（`Arc<Mutex<Connection>>`，web/ssh 同款；SDK 不自开库，
  ADR-018/030）；turn 工作线程与泵线程经 `try_lock` 竞争，冲突方返回 busy/跳过本轮
  （serve 同款，不阻塞持锁者）。
- stdout 由写线程独占 `Write`；其余线程只入队（`Sender`）。
- `CancelToken`／活跃槽以 `Arc` 共享；生命周期 = 连接生命周期，关闭时 join 全部线程。

## 7. 失败模式

| 失败 | 行为 |
|---|---|
| 出站通道满（慢消费者） | `closed` 帧（slow_consumer）→ 关闭连接；客户端游标重放 |
| turn 工作线程 panic | fail-fast：进程以非零码退出（缺陷显式暴露，不伪装存活）；客户端按连接关闭处理 |
| stdin EOF / 写 stdout 失败 | 优雅关闭：停泵、停读、排空出站（有界排空时限）、join 全部线程 |
| 活跃槽泄漏 | 槽守卫 RAII（worker 退出即释放，serve 评审 M3 同款） |
| 重复 `events.follow` | 返回 `invalid_state`（单消费者 v1；多消费者属演进，不在承诺面） |
| 世代乱序请求 | 客户端带过期世代重入 `events.follow` → 正常接受（世代由服务端维护，客户端只读） |

## 8. 验证计划

- 单测：帧编解码往返（请求／通知／错误）、`initialize` 版本协商矩阵（同主/异主/缺字段）、
  帧超限拒绝（64 KiB 行上限）。
- 集成（内存双工，`stdin`/`stdout` 以 trait 注入）：create→open→run→事件帧序列断言
  （seq 连续＋generation 正确）；游标重放（断连重连补齐不重不漏）；世代推进（注入
  `SessionRewound` 后 generation +1）；取消竞态（run 中 cancel → 终态 cancelled 且响应
  到达）；背压（容量压满 → `closed` 帧＋关闭）；槽互斥（第二次 run → busy）。
- **Loom/Shuttle 豁免理由**：线程数 ≤ 3/连接，全部共享经有界通道与既有守卫原语
  （无自旋、无锁、无手动内存序设计点），Loom 模型检验的成本高于收益；以集成竞态测试
  （取消竞态、槽互斥、背压）＋ serve 同款原语先例为验证载体。
- 测量：新增能力无既有基线，同夹具前后测量豁免；资源铁律走既有空闲基线
  （`measure-resources`／`measure-nfr-budgets` 通道，SDK 空闲态 = 读线程阻塞 + 泵空转，
  无新增固定占用承诺按 NFR-201—204 既有预算复核）。

## 9. 回退方案

- 多路复用缺陷 → **单工回退**：协议不变（客户端本可串行）——服务端模式收敛为
  “follow 期间拒绝 run、run 期间拒绝 follow”，契约无需变更。
- stdio 传输缺陷 → 既有 Web（HTTP+WS）与 CUI JSON 通道兜底（外部程序降级走 `--json` CLI）。
- 审批面缺口暴露 → 加速 P6.3（ACP 机器决策者显式化），SDK 协议面不变。

## 10. 文献依据

- JSON-RPC 2.0 规范（官方；id／notification／error 结构与语义）。
- MCP stdio 帧实践（line-delimited；ADR-054 实测在案）。
- DSH 域 I（对照取其“协议细节集中、传输交给框架”思路；舍其多语言 SDK 面——WHCode 以
  契约文档为承诺载体，客户端按契约实现）。
- serve_loop 单线程多路复用与原子 turn 槽先例（in-repo，ADR-058 §2.4、评审 M3）。
- ADR-017（首个并发档案：有界通道 + 单写者 + RAII 释放的同款纪律）。

## 已知限制与失效条件

- **已知限制**：审批面与 CUI `llm run` 同装配（无交互闸门，无人值守决策面属 P6.3）；
  `events.follow` 单消费者；活跃 turn 上限 1/连接；单连接单工作区；多语言客户端不交付
  （契约文档为承诺载体）；用户取消不打断已派生的子进程副作用。
- **失效条件与复审触发（绑定命令＋判定规则）**：
  - `cargo test -p whcode-bin sdk` 出现竞态/乱序相关失败 → 复审 §2.2 线程模型与 §3 不变量。
  - 自动化客户端频繁收到 `closed`（slow_consumer）→ 复审背压策略与通道容量。
  - 方法面新增 → 契约 minor 升级并在 CHANGELOG 登记；语义破坏 → major 升级（D14）。
  - `measure-resources`／`measure-nfr-budgets` 空闲基线超既有预算 → 复审线程/句柄/通道常量。

## 12. 形式审查与正负和解

- **形式审查**：真实矛盾＝对外面不可编程驱动（域 I“结构性缺失”）；最小协议是 D14 批准的
  范围，方法面不越界。
- **正面**：可编程互操作补齐；错误复用 CUI 信封零新口径；游标＋世代复用 ADR-068 事实流；
  执行骨干全复用（turn 编排/取消/游标广播既有）。
- **负面**：新协议面＝版本承诺不可逆；并发面新增（档案收口＋验证计划）；单消费者与单活跃
  turn 的能力让步；多语言客户端不交付（使用方自实现）。
- **承担者**：协议兼容由维护者按 D14 承担；并发风险由 §8 验证计划承担；审批面缺口由
  P6.3 收口。
- **和解理由**：负面均有界、显式、有回退；正面补齐结构性缺失。
