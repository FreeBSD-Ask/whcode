# ADR-058: turn 执行移出 serve 循环线程——WS 逐块增量转发（FR-742）

- **状态**：**已批准（附条件）**——2026-11-06 用户「批准开发」，同日评审并收口；条件＝
  评审 M1—M11（§3.7）。**批准不豁免 M2—M7**：认证绕过、并发计数、busy 语义、通道语义、
  期限取舍、客户端断开六项未闭环前，不得据此勾选 Check 347（Web 对话页），也不得对外
  宣称 FR-742 已达成。
- **日期**：2026-11-06（初稿 2026-11-05；批准与实现收口 2026-11-06）
- **关联**：ADR-057（批次 B1 的 T251）、ADR-015（并发设计档案）、ADR-044（turn 循环）、
  ADR-022（Web 安全默认）、ADR-023（局域网认证必填）、ADR-037（CI 唯一验收）、
  ADR-046（复审触发＝绑定命令＋判定）、ADR-056（连接共享先例）。
- **落点**：`crates/whcode-bin/src/serve.rs`（循环、拦截、降级、帧组装）、
  `crates/whcode-web/src/api.rs`（`llm.run` 路由、`TurnRequest` 单一构造器、错误码映射）、
  `crates/whcode-web/frontend/chat.js`（delta 消费端）。

> **行号口径**：实现已随本 ADR 定稿同步修改，本文一律写“函数名＋常量名”；行号会漂移，
> 判定以函数与常量为准。

---

## 1. 事实与证据（第 1 层：源码）

- `fn serve_loop` 为**单线程非阻塞多路复用**：accept → 逐 client 泵 → 每
  `EVENT_POLL_INTERVAL` 轮询 `poll_events` 广播。HTTP（含 `POST /api/llm/run`）原先在
  `fn respond` 内同步执行（`api.rs` 的 `llm.run` 路由直调 `whcode_app::turn::run_turn`）。
- 同步 turn 期间循环停摆：WS 泵不推进、事件不轮询、其他客户端 HTTP 不被接受；turn 结束
  后积压帧一次性涌出。
- **StreamingChunk 事件 payload 不含正文**（`turn.rs` 的 `emit!`；FR-722 是登记契约）。
- 增量正文通道已使能（`f9c5cf6`）：`TurnProgress = &dyn Fn(u64, &str)`＝（累计块数, 增量正文）。
- **`TurnProgress` 的既有契约是“回调必须非阻塞”**（`whcode-app/src/turn.rs` 类型文档）：
  明写“必须非阻塞、不得触库……`try_send`，满即丢弃”。这直接否决了“在回调里阻塞发送”。
- **引擎可计算上界**：`MAX_TURN_ITERATIONS: u32 = 16`（FR-719；请求可调低不可调高）；
  单次模型请求默认总超时 `default_total_timeout_ms() = 300_000`。纯模型 step 上界 =
  16 × 300 s = 4800 s；工具执行与子 turn（FR-733 递归）不在该上界内 `[待验证]`。
- 连接共享先例：ADR-056 Phase 2 已把工作区连接改为 `Arc<Mutex<Connection>>`（GUI/TUI
  执行器跨线程），`gui_executors_are_send_sync` 编译期证明在库。
- **预实现登记（事实，不是许可）**：`fc3a0c0` 先于批准改动 `serve.rs`（+386/−65）与
  `api.rs`（+8），并导致 CI 三平台红（run `42143683382`）。门禁修复见 `942dd14`。
- 现有错误信封（`api::error_response`）为顶层五字段
  `{schema_version, code, message, retryable, details}`；`Backpressure` 与 `Timeout` 在
  `ErrorCode::is_retryable` 中为真。
- 前端：`chat.js` 已消费 `/ws` 的 `llm.chunk`／`llm.loss`（按 `session` 归属、逐块追加、
  完成重拉尾部）；无 JS 时服务端直出历史（`chat.html`）。

## 2. 决策（并发设计档案）

### 2.1 方案比选

| 方案 | 语义影响 | 并发影响 | 判定 |
|---|---|---|---|
| A. `llm.run` 改流式响应（SSE/chunked） | **破坏登记契约**（三端同步等待完成） | 服务器仍单线程 | 否决（契约变更无需求方） |
| B. 正文入 StreamingChunk payload | 破坏 FR-722＋脱敏口径 | 无 | 否决 |
| C. 维持现状（burst 后补） | 无 | 无 | 否决（FR-742 未满足） |
| **D. turn 移出循环线程＋等待期联合泵** | **无**（HTTP 仍在 turn 完成后返回） | 每活跃 turn 一个临时线程（≤2、有界、RAII join） | **采纳** |

### 2.2 结构

1. **连接升级为 `Arc<Mutex<Connection>>`**（与 GUI/TUI 执行器同构）：“单连接＝唯一写路径”
   （Check 98）由互斥保证。
2. **鉴权与守卫先于拦截**：`POST /api/llm/run` 的拦截在静态路由之后、
   `handle_request_with_llm` 之前；进入拦截前执行与 `api.rs` 同源的
   `security::check_auth`／`security::check_origin`（`fn check_request_guards`），失败返回
   同一错误信封且**不 spawn**。〔M2〕
3. **工作线程**：spawn `turn-worker`；`run_turn_with_progress` 执行，增量经
   `sync_channel(TURN_PROGRESS_CAPACITY = 64)` **`try_send`** 回传（满即丢并计数）；完成经
   `mpsc::channel` 送达序列化结果。
4. **相位 `Phase::TurnPending`**：携带 `session`/`request_id`/`progress`/`done`/`worker`/
   `dropped`。循环每 tick 由 `fn drain_turn_pending` 排水 ≤ `TURN_DELTA_DRAIN_PER_TICK`（=64，
   ＝出站队列容量）帧，经 `fn turn_stream_frames` 组装后由 `broadcast_and_flush` 广播；
   `done_rx` 就绪 → 组装与同步路径**同字段同值**的 JSON 信封 → 写回该客户端。
5. **并发上限**：`MAX_ACTIVE_TURNS = 2`；`AtomicUsize::try_update` 完成“读-判-加”后才
   spawn；worker 退出（含 panic）由 `SlotGuard` 的 `Drop` 递减。超限即 503，不排队。〔M3〕
6. **busy 语义**：turn 期间触库路径返回 **503 ＋ 标准 JSON 信封**，`code = backpressure`、
   `retryable = true`，并附 `Retry-After: 1`；`/api/llm/cancel` 与 `/ws` 握手**在锁判定
   之前**处理（两者不触库），保证 turn 期间“取消可达、订阅可达”。〔M4〕
7. **安全**：delta 帧含模型正文，仅送已握手的 WS 订阅者；事件 payload 仍不含正文；
   广播范围、跨会话可见与代价见 §3.3。
8. **取消**：标记目录跨线程机制不变（chunk 边界生效）；成功/取消/失败/通道断开四态经通道
   回循环，相位照常收敛。

### 2.3 错误语义与确定性降级清单（M4 定稿）

| 路径 | 行为 | 依据 |
|---|---|---|
| `POST /api/llm/run`（容量未满） | 拦截器接管，完成后 200 + 同步信封 | §2.2.4 |
| `POST /api/llm/run`（容量已满） | 503 + `code=backpressure` + `retryable:true` + `Retry-After: 1` | §2.2.5 |
| `POST /api/llm/cancel` | **可达**（不触库） | FR-723 |
| `GET /ws`（握手升级） | **可达**（不触库） | 订阅不因 turn 被拒 |
| `GET /`（首页） | **200 未注入业务数据的外壳**：命中静态模板（`assets::static_asset`），注入标记原样保留，不触库 | §3.1“页面壳与静态资源仍可取”（2026-11-09 按实现收口） |
| `GET /chat`（对话页） | **200 未注入历史的外壳**（同上） | 同上 |
| 其他 `/api/*` | 503 + 同一 JSON 信封（判据：`/api/sessions`） | 触库 |
| `GET /assets/*`（静态资产） | 200，照常 | 不触库 |

**错误码决定**：新增 `ErrorCode::Backpressure => 503` 的映射（`api::error_response`），
**不复用 `invalid_state`**——同一错误码不能既映射 400 又映射 503，按码分派的客户端会歧义
（评审异议 1 的采纳）。

### 2.4 并发上限与计数（M3）

- `AtomicUsize` + `MAX_ACTIVE_TURNS = 2`；**判定与占用同一步**：
  `try_update(|n| (n < 2).then_some(n + 1))`，`Err` 即 503；`Ok` 后才 spawn。
- **递减点唯一**：worker 闭包入口的 `SlotGuard`（`Drop` 中 `fetch_sub`），覆盖正常结束、
  错误返回与 panic 展开；spawn 失败路径显式回滚一次。计数不因任何路径泄漏。
- 该修正关闭原缺陷：判定原在 `serve_loop` 每 tick 求值一次，同 tick 多请求共用陈旧值，
  真实上界退化为 `MAX_CLIENTS`（96 个线程阻塞在同一 `Mutex`）。

### 2.5 增量通道与帧格式（M5 / M9）

- 通道 `sync_channel(64)`，发送端一律 `try_send`；`Full` 即丢弃并递增 `dropped`。
- **丢弃口径**：只影响 WS 呈现连续性，**不改变**完成信封、不改变 `messages`/`events` 落库、
  不改变 `event_missing == 0`；单 turn 丢帧上限＝该 turn 产出块数（构造上有界）。
- **帧格式**：
  `{"type":"llm.chunk","session":"<id>","request_id":"<id>","index":<u64>,"delta":"<text>"}`
  丢帧汇总：`{"type":"llm.loss","session":"<id>","request_id":"<id>","dropped":<u64>}`。
  `session`/`request_id` 为必需字段——两个并发 turn 在同一 WS 流上靠它们归属。
- 帧与事件**共用**每订阅者 64 帧出站队列与同一慢客户端策略（Check 199）：队列满即断开该
  订阅者（可观测、可重连、可经 `/api/events` 补拉）。

### 2.6 turn 时长上界与客户端断开（M6 取舍）

- **不引入杀线程**（Rust 无可安全中止线程的机制；强杀破坏 DuckDB 连接不变量）。
- **不引入独立期限常量与 orphan 表**：期限表需要第二张有界表与第二条收敛路径，而其收益
  只是把“HTTP 等待”截短到常量；turn 本身仍持锁跑完，读面仍 503——截短等待并不缩短降级
  窗口，收益不抵复杂度（评审“先试简化版”建议的采纳）。
- **上界改为显式可计算值**：纯模型 step ≤ `MAX_TURN_ITERATIONS(16) × total_timeout(300 s)`
  = 4800 s；工具与子 turn 的上界未取得 `[待验证]`。取消在 turn 期间可达（§2.3），用户不必
  等待上界。
- **失效条件**：若 provider 的 `total_timeout` 被调高、或工具/子 turn 出现不可计算的长尾，
  本条失效，必须补期限并重写 §2.8。
- **客户端提前断开不中止 turn**：`TurnPending` 不读 socket，断开只能在下一次写时发现；
  turn 继续执行、结果丢弃（已落库不回滚）。代价：最多一个 turn 的计算浪费；收益：不引入
  “写半途被中断”的不一致面。此取舍为显式决定。

### 2.7 构造器收敛（M11）

`TurnRequest` 的构造收敛为 `whcode_web::api::turn_request_from_body`，`api.rs` 的 `llm.run`
路由与 `serve.rs` 拦截器共用。理由：两份同形构造会在 `model`/`max_tokens`/`request_id`/
未来字段上漂移，而漂移不会被任何测试发现（两条路径互不覆盖）。

### 2.8 ADR-015 十一字段

#### 2.8.1 问题与边界

- 并发点：一条 DuckDB `Connection` 被两类线程使用——循环线程（每 tick `try_lock`）与
  turn 工作线程（持锁执行整个 turn）。
- 共享：连接（互斥）；客户端表与 `BroadcastState`（**仅循环线程**）；活跃计数（原子）。
- 不共享：`ServeLlm` 上下文按请求求值；出站队列只在循环线程读写；`TurnOutcome` 由通道
  转移所有权。

#### 2.8.2 不变量

- **I1 单写者**：任一时刻至多一条线程持有连接；获取只有两处（循环 `try_lock`、worker
  `lock`），不嵌套第二把锁。
- **I2 相位有界**：任一 `TurnPending` 必在有限 tick 内离开——正常完成或通道断开；上界由
  §2.6 的 4800 s 与取消可达性共同承担。
- **I3 帧内容**：delta 帧只含本 turn 的模型正文增量，来源唯一（`TurnProgress` 实参）；
  不含提示词、工具结果、凭据或事件 payload；事件 payload 继续不含正文（FR-722）。
- **I4 计数守恒**：活跃计数任何时刻 ≤ 2；每次成功 `try_update` 恰由一次 `SlotGuard::drop`
  配对递减；panic、失败、spawn 失败均覆盖。
- **I5 信封同一**：拦截路径 200 与同步路径 200 同字段同值；增值只在 WS 帧。
- **I6 游标不跳**：turn 期间 `poll_events` 跳过时 `cursor` 不推进；结束后按原 cursor 续拉。
- **I7 队列同策**：delta 与事件共用出站队列与慢客户端策略；不为容纳 delta 丢弃事件，
  也不静默丢 delta 而保持连接——队列满即断开。

#### 2.8.3 线性化点

| 操作 | 线性化点 |
|---|---|
| 接纳 turn | `try_update` 返回 `Ok(prev)` 那一刻 |
| 拒绝 turn（503） | `try_update` 返回 `Err` 且未 spawn |
| delta 送达某订阅者 | 该订阅者出站队列 push 成功那一刻（各自独立，无全局序） |
| turn 完成 | 循环 `done_rx.try_recv()` 返回 `Ok` 的那一 tick |
| turn 失败（通道断开） | `try_recv()` 返回 `Disconnected` 的那一 tick |

#### 2.8.4 内存序

- **计数**：跨线程可见性只经“worker 结束 → `done` 通道 → 循环”这一条 happens-before 边，
  由 `mpsc` 内部同步建立；正确性上 `Relaxed` 已足够。选 `AcqRel/Acquire` 是为将来把拦截器
  移入工作线程留安全余量，并避免 `SeqCst` 的全局屏障（指南 §2.3）。
- **连接数据**：由 `Mutex` 的 Acquire/Release 承担；worker 内读写都在同一 guard 生命周期内。
- **通道数据**：`mpsc` 内部同步，不需额外屏障。
- **毒锁**：`Poisoned` 分支终止 serve 进程（一致性优先），不 `into_inner` 继续使用可能被
  panic 打断的连接。

#### 2.8.5 进展保证

- 循环线程每 tick wait-free：`try_lock`、`try_recv`、`try_send`、非阻塞 socket 读写均不阻塞。
- worker 阻塞在 `Mutex::lock`，但持锁期间**不等待任何通道**（`try_send`），不会级联停摆。
- 计数 lock-free（单次 `try_update`）。std 不保证 `Mutex` 公平，故**不承诺公平**，只承诺
  有界并发（≤2）与有界等待窗口（≤ §2.6 上界）。
- 最坏等待：第二个 turn 在锁上等待 ≤ 第一个 turn 的 4800 s 上界；第三个请求确定 503。

#### 2.8.6 数据所有权

| 对象 | 创建 | 跨线程传递 | 释放 |
|---|---|---|---|
| `Arc<Mutex<Connection>>` | 装配根 | 克隆到 worker | 最后一个引用释放 |
| `JoinHandle` | 拦截器 spawn 处 | 随 `TurnPending` 留在循环 | 收敛 `join` |
| delta `String` | worker | `try_send` → 循环 | 组帧后队列 → 写出后丢弃 |
| 完成结果 | worker（序列化） | `done` 通道 → 循环 | 组信封后丢弃 |
| `Arc<ServeLlm>` | 装配根 | 克隆到 worker | worker 结束 |
| `dropped: Arc<AtomicU64>` | 拦截器 | 克隆到 worker（写）/循环（读清） | 随 turn 丢弃 |

#### 2.8.7 失败模式

| 模式 | 表现 | 处置 |
|---|---|---|
| 数据竞争 | 无（单锁＋通道＋客户端表仅循环线程） | — |
| 毒锁（worker panic） | 下个 tick `try_lock` 返回 `Poisoned` | 终止 serve，`IoError` 可操作消息 |
| 活锁 | 无（无自旋） | — |
| 饥饿/不公平 | 第二个 turn 在 `Mutex` 排队 | 已知代价；上界见 §2.6 |
| ABA/伪共享 | 不适用（无槽位复用） | — |
| 栈溢出 | 默认 2 MiB；子 turn 递归受 FR-733 约束 | `[待验证]`：深层子 turn 未测 |
| 通道满 | `try_send` 返回 `Full` | 丢帧＋计数＋`llm.loss` 帧 |
| 客户端断开 | 写时失败 | turn 继续、结果丢弃（§2.6） |
| 计数泄漏 | 递减覆盖全部路径 | 由 I4 与用例 2 判定 |

#### 2.8.8 验证计划（可执行）

| # | 用例 | 状态 |
|---|---|---|
| 1 | `turn_slot_limit_rejects_with_backpressure`：槽位满 → 503 + `backpressure` + `Retry-After`，不 spawn | 已落地 |
| 2 | `turn_slot_released_when_worker_exits`：worker 退出归还槽位 | 已落地 |
| 3 | `turn_pending_aborted_channel_converges_to_failure`：完成通道断开 → 500 结构化信封 | 已落地 |
| 4 | `turn_stream_frames_carry_identity_and_loss`：帧含 session/request_id，丢帧以 `llm.loss` 公开 | 已落地 |
| 5 | `busy_degrade_keeps_cancel_and_ws_reachable`：turn 期间取消可达、`/ws` 101、其余 503 | 已落地 |
| 6 | `llm_run_requires_auth_before_turn_intercept`：无 Bearer/坏 Origin → 403 且不 spawn；合法 Bearer 仍走拦截 | 已落地 |
| 7 | `turn_intercept_envelope_matches_sync_path_field_by_field`：拦截路径 200 与同步路径逐字段一致（信封／`messages`／`events`），并验证逐块 relay 帧拼回的正文与信封 `text` 相同 | 已落地（2026-11-09） |
| 8 | `turn_ws_relay_first_delta_busy_codes_and_event_missing`：三指标端到端＋归档 `artifacts/turn-ws-relay-2026-11-09/turn-ws-relay-e2e.json` | 已落地（2026-11-09 实测：首帧 delta 1006 ms；触库路径 503 `backpressure`＋`Retry-After: 1`；`/api/llm/cancel` 200；`/ws` 101；`event_missing` 0） |
| 9 | `slow_consumer_loss_is_published_and_events_survive`：通道满丢帧以 `llm.loss` 公开、计数守恒（送达＋丢弃＝产出）、事件表不缺（`StreamingChunk` 条数＝产出块数，事件与消息共用序号池合并后连续） | 已落地（2026-11-09） |
| 10 | `turn_resources_return_to_baseline`（`#[ignore]` 资源实验）：turn 期间线程增量 1、结束后回基线、句柄不增长；归档 `artifacts/turn-ws-relay-2026-11-09/turn-resources.json` | 已落地（2026-11-09 Windows 实测：基线 81 线程／234 句柄 → turn 中 82／236 → 结束后 81／234） |
| 11 | 差分（回退验证）：revert 前后 `messages`/`events`/信封一致 | 缺口登记（§3.6）：本机未取得，确切命令与阻塞原因已写明，未伪造 |

**用例 7—9 发现并修复的两处缺陷（2026-11-09）**：

1. **delta 帧未按 WebSocket 编码**：`turn_stream_frames` 产出 JSON 载荷，而广播队列
   （`BroadcastState::broadcast_frame`）收的是**已编码**帧（事件侧由 `poll_events` 调
   `ws::encode_text_frame`）。原实现把裸 JSON 直接写进 WS 流，客户端按 RFC 6455 解析必然
   帧错位（正文会随帧边界撕裂）。修复：`drain_turn_pending` 在广播前统一
   `ws::encode_text_frame`；用例 7、9 的 socket／队列级断言作回归。
2. **`GET /`、`/chat` 与 §2.3 清单不一致**：两者命中静态模板（`assets::static_asset`），
   turn 期间返回**未注入业务数据的外壳 200**，不是 503。外壳不触库，且 §3.1 已写明
   “页面壳与静态资源仍可取”，故按实现收口 §2.3 两行，并把“触库路径 503”的判据固定在
   `/api/sessions`（用例 8 断言）。

**不上的验证及理由**：本设计不含自定义无锁结构，同步语义全部由 std `Mutex`/`mpsc` 承担并
有文档化保证，故不上 Loom/Shuttle；一旦引入自建队列或手写 CAS，本理由失效，必须补模型
检查。不得以“跑了几小时没崩”作为结题证据。

### 2.9 一手文献（逐篇：结论／适用条件／差异／反例）

1. **Rust std `std::sync::mpsc::SyncSender`**。原文：“`send` will block if there is no space
   in the internal buffer.”；`try_send`“differs from `send` by returning immediately if the
   channel's buffer is full or no receiver is waiting”。
   - 适用：有界缓冲、生产者不得被消费者拖住。
   - 差异：初稿把“有损”写成目标，实现却用阻塞 `send`——正是本条的反面。
   - 反例：消费侧长期不排水时，`send` 把 worker 连同连接锁挂住，局部 busy 放大为全局冻结。
   - 结论：改 `try_send`（M5）。
2. **Rust std `Receiver::try_recv`**。原文：“Attempts to return a pending value on this
   receiver without blocking.”“never block the caller in order to wait for data”。
   - 适用：循环线程每 tick 排水。
   - 差异：本设计把 `Disconnected` 复用为“worker 提前终止”的判定信号。
   - 反例：把 `Disconnected` 与 `Empty` 合并处理 → 连接永久停在 `TurnPending`。
3. **Rust std `std::sync::Mutex`（`try_lock`／中毒）**。原文：“This function does not block.”
   “Once a mutex is poisoned, all other threads are unable to access the data by default”。
   - 适用：非阻塞获取＋跨线程共享可变连接。
   - 差异：std 建议 `unwrap()` 传播 panic；本项目选择终止整个服务。
   - 反例：`into_inner` 可绕开毒锁继续用连接，但连接可能已被 panic 打断在语句中间，继续
     使用等于放弃 I1 的一致性前提。
4. **仓库一手证据**：`docs/adr/ADR-002` 记录 DuckDB Rust `Connection` 为 `Send` 非 `Sync`，
   解释了“必须 `Mutex` 包一层”。失效条件：升级 crate 或改走原生 C API 时重核。
5. **DuckDB 官方并发文档**：**未取得**（`/docs/stable/connect/concurrency` 返回重定向页）。
   因此不引用其结论；本文“单连接串行化”的依据是 ADR-002 ＋仓库结构 ＋I1。`[待验证]`

## 3. 副作用

### 3.1 确定性降级清单

见 §2.3。要点：**turn 期间取消可达、订阅可达；触库路径不可达**。用户刷新会看到**未注入
业务数据的外壳**（首页与对话页命中静态模板，注入标记原样保留）与静态资源；`/api/*` 等
触库请求返回 503。这是已知代价；若要求“刷新仍能看对话页历史”，改为“锁忙时直出有界尾部
快照 + 客户端补拉”，不改变并发结构（2026-11-09 按实现收口：外壳已可取，历史仍需 503 或
补拉）。

### 3.2 正负和解

- **正面**：FR-742 的服务端与消费端两侧均落地（tick 粒度增量＋前端渲染）；`llm.run` 同步
  契约零变化；curl/脚本行为不变；静态资产、WS 订阅、取消在 turn 期间不受影响。
- **负面**：①turn 期间触库路径确定性 503（`/api/*`；首页与 `/chat` 返回未注入数据的外壳，
  见 §2.3 与 §3.1）；②正文原文进入
  广播面（跨会话可见）；③多一把锁引入毒锁关停面；④每 turn 一个临时线程（≤2）与最长
  4800 s 的占用窗口；⑤客户端断开不中止 turn（计算可能浪费）；⑥有损通道丢帧（以
  `llm.loss` 公开）。
- **承担者**：serve 维护者（状态机与回归）；turn 期间的其他 Web 用户（读面降级、需重试）；
  LAN 部署者（delta 正文跨会话可见；认证部署下浏览器 WS 不可用，见 §3.3）。
- **和解理由**：A/B/C 分别要求破坏同步契约、破坏 FR-722、放弃 FR-742；D 的代价全部落在
  “turn 期间的可用性与暴露面”，可观测（503 清单确定）、可重试（`retryable:true`）、
  可回退（一次 revert），不触碰数据格式与 schema。

### 3.3 安全威胁模型（delta 广播）

- **资产**：模型输出正文（可能含提示词回显、仓库内容摘要、工具结果摘录）。
- **广播范围**：全部已握手的 WS 订阅者，**跨会话可见**；不按 `session` 过滤。前端
  `chat.js` 只在 `frame.session` 与本页一致时渲染——这是**呈现侧过滤，不是授权边界**，
  任一订阅者仍可在网络上收到其他会话的正文。
- **认证边界**：沿用既有 WS 准入（ADR-022/023）。**浏览器无法给 WS 握手带
  `Authorization`，且 token 禁止走 URL**（`whcode-web::security` 文档明令），因此认证部署下
  浏览器端 WS 会被拒绝——`chat.js` 静默回落为“发送后重拉历史”，不伪造实时。WS 增量实际
  只在未认证回环部署可用。
- **威胁**：①同服务的其他会话/脚本可读到不属于自己的正文；②正文含提示词回显，等价于把
  提示词暴露给订阅者；③事件仍 `redacted` 且不含正文，但 WS 帧是明文，两者并存。
- **代价与失效条件**：接受 ①②为“单机单用户工具”的默认和解；一旦出现多用户部署或
  `session` 级权限隔离需求，本条失效，必须改为按 `session` 过滤（帧内已有字段，改造点集中
  在广播选择处）。
- **不做的事**：不把正文写进事件 payload（FR-722 不动）；不在日志打印帧内容；丢帧只记数量。

### 3.4 三重时间

- **过去来源**：ADR-056 Phase 2（`Arc<Mutex<Connection>>`＋有界通道＋join，已实测）；
  `serve.rs` 档案一；FR-722/723/719；domain-M2 §① 双通道对话流。**差异已补**：ADR-056 是
  单进程单用户 GUI（无 HTTP 多客户端、无慢客户端、无 96 连接上限、无毒锁关停），GUI 的
  “有损进度”不能原样搬成 serve 的“有损正文”——serve 的丢弃必须配计数与公开信号。
- **现在证据**（2026-11-09）：M2—M5、M9—M11 已实现；§2.8.8 用例 1—10 有可执行测试并本机
  全部通过（`cargo test -p whcode-bin`：103 passed／0 failed／2 ignored；两个忽略项分别为
  Check 199 慢客户端对照与 §2.8.8 用例 10 资源实验，均以显式命令单独运行）；
  `cargo clippy -p whcode-bin --all-targets -- -D warnings` 与
  `cargo fmt -p whcode-bin -- --check` 通过；三指标归档见
  `artifacts/turn-ws-relay-2026-11-09/`（首帧 delta 实测 967—1006 ms、触库路径 503
  `backpressure`＋`Retry-After: 1`、`/api/llm/cancel` 200、`/ws` 101、`event_missing` 0；
  资源实验：turn 期间线程 +1、结束后回基线、句柄不增长）。**CI 尚无本批绿灯 run**
  （ADR-037：本地结果只作复现与排障，不作验收结论）；用例 11 见 §3.6 缺口登记。因此现在
  证据支持“§2.8.8 用例 1—10 已落地”，不支持据本地结果直接勾选 Check 347。
- **将来失效条件**：①事件速率持续超过 320 事件/秒；②订阅者接近 `MAX_WS_CLIENTS` 且帧
  增大；③`MAX_CLIENTS`/`MAX_ACTIVE_TURNS` 调整；④provider 改 SSE 契约；⑤多用户或
  `session` 级隔离需求出现；⑥provider 总超时被调高或工具/子 turn 出现不可计算长尾。

### 3.5 契约影响与 Check 347 的可闭条件

- HTTP 契约：不变。`feature-parity.md` 的 `llm.run`（同步等待）与 `view.chat` 行无需改。
- WS 契约：新增 `llm.chunk` / `llm.loss` 帧（§2.5）；`chat.js` 已消费并按 `session` 过滤。
- **Check 347 可闭四条件（同时满足）**：
  1. 前端消费 delta 并渲染（已落地：`chat.js` 订阅 `/ws`、按 session 过滤、逐块追加、
     完成重拉尾部、丢帧提示）；
  2. 无 JS 降级仍成立（服务端直出有界尾部快照，HTML 转义、超限写明截断）——已落地；
  3. `check-frontend-budget.py` 达标——本批实测 JS gzip chat 2213 B、CSS 2346 B、
     DOM chat 20；
  4. 三指标归档 + CI 绿（§2.8.8 第 8 项）：端到端归档已取得——
     `artifacts/turn-ws-relay-2026-11-09/turn-ws-relay-e2e.json`（2026-11-09 实测：首帧
     delta 1006 ms；触库路径 503 `backpressure`＋`Retry-After: 1`；`/api/llm/cancel` 200；
     `/ws` 101；`event_missing` 0）；资源实验归档同名目录下 `turn-resources.json`（turn 期间
     线程 +1、结束后回到基线、句柄不增长）。再生成命令见该目录 `README.md`。CI 绿灯仍以
     GitHub Actions 日志与 artifact 为准（ADR-037），本地结果只作复现与排障。
- 因此 `tasks.md` 的 T251 与 `checklist.md` 的 Check 347：条件 1—4 的**本地证据**已齐备；
  勾选仍以 CI 绿灯为准。用例 11 的缺口（§3.6）不影响条件 4 的三指标口径，但作为回退证据
  仍属未取得，需在 Check 取证文字里如实写明。

### 3.6 回退方案与差分验证

- 回退＝`git revert` 本批次（无数据迁移、无 schema、无配置项）。
- 差分验证（用例 11）**本机未取得**，按缺口登记处理（不伪造）：
  - 目标：回退前（`8eccad7` 之前的同步路径）与回退后的 `messages`／`events`／信封逐字段一致。
  - 确切命令：
    ```text
    git worktree add local/t251-revert 8eccad7
    cd local/t251-revert
    cargo test -p whcode-bin turn_intercept_envelope_matches_sync_path_field_by_field -- --nocapture
    ```
  - 阻塞原因（事实）：回退树**没有工厂注入缝隙**——`8eccad7` 及以前 `ServeLlm.factory` 是具体
    类型 `LocalProviderFactory`，无法注入 `StubProvider` 剧本；该缝隙正是本批改动新增。因此
    在回退树上运行对应用例无法离线驱动确定性剧本（只能触发真实 provider 或“会话不存在”的
    快速失败），逐字段比对的被测行为在回退树上不存在。另：净树需重建 DuckDB 链接层与整套
    crate，超出本批时限。
  - 已取得的替代证据：本树同步路径与拦截路径逐字段一致（用例 7
    `turn_intercept_envelope_matches_sync_path_field_by_field`）；同步路径代码
    （`whcode-web/src/api.rs` 的 `llm.run` 路由 → `run_turn`）在本批次零改动——回退只删除
    拦截路径，不改变已落库内容与信封的生成逻辑。

### 3.7 预实现补救与条件闭环

- **事实**：实现（`fc3a0c0`）先于批准，且三平台红。**承担者：主代理**。
- **补救**：条件闭环＋分离提交：①`fix(serve)` 复绿（M1，`942dd14`）；②`fix(serve)`
  关闭 M2—M5/M9/M11；③`test(serve)` 关闭 M7 的一部分；④`docs(adr-058)` 关闭 M8/M10 与
  本文同步。四类不混批，也不与 Check 勾选混批。
- 条件清单与状态：
  - **M1 复绿**：`942dd14` 修 E0252 ＋ 4 项 clippy ＋ fmt；待新 run 复核。
  - **M2 鉴权前置**：已落地，用例 6。
  - **M3 上限原子化**：已落地，用例 1、2。
  - **M4 busy 语义**：已落地（503 `backpressure`＋`Retry-After`；取消与 WS 可达），用例 5。
  - **M5 有损通道**：已落地（`try_send`＋`llm.loss`），用例 4。
  - **M6 期限取舍**：以 §2.6 的显式上界取代期限常量（评审允许的另一路径），已成文。
  - **M7 测试**：用例 1—10 已落地（7—10 于 2026-11-09 补齐，含两处缺陷修复：delta 帧 WS
    编码、`GET /`／`/chat` 降级清单收口）；用例 11 按 §3.6 缺口登记（回退树无工厂注入缝隙，
    净树重建超时限）。
  - **M8 档案**：本文 §2.8/§2.9 完成。
  - **M9 帧字段与威胁模型**：已落地（§2.5、§3.3），用例 4。
  - **M10 文档一致性**：`serve.rs` 档案一改为“档案一零新增线程；档案二临时线程 ≤2”；
    ADR 日期统一；`module-design.md` 登记 api 公开面扩张。
  - **M11 构造器收敛**：`api::turn_request_from_body` 落地，两处共用。

## 4. 复审触发（绑定命令＋判定）

| 命令 | 判定 |
|---|---|
| `cargo test --locked -p whcode-bin -- turn` | 退出码非 0 → 复审实现 |
| `cargo test --locked -p whcode-bin -- busy` | 退出码非 0 → 复审降级清单 |
| `python .github/scripts/benchmarks/measure-resources.py` | turn 结束线程计数不归零或随会话线性增长 → 复审 |
| `python .github/scripts/benchmarks/check-frontend-budget.py` | 失败或 JS > 50 KB gzip → 复审（Check 347 不可闭） |
| `python .github/scripts/traceability.py`、`check_counts.py` | 不一致 → 当批不得结题 |
| `artifacts/turn-ws-relay-*` 缺失 | 不勾 Check 347 |
| 部署形态 | 出现多用户或 session 级隔离需求 → 复审 §3.3（改为按 session 过滤） |
| provider 超时 | `total_timeout_ms` 被调高或工具/子 turn 长尾不可计算 → 复审 §2.6 |
| 错误码分派 | 出现“同码多状态”需求 → 复议新错误码 |

## 5. 修订记录

- 2026-11-05：初稿（提议，待用户批准），写明“批准前不动 `serve.rs`”。
- 2026-11-06：**用户「批准开发」→ 状态转已批准**；实现落地（`fc3a0c0` 主体，
  `942dd14` 修复编译与 clippy 门禁）。
- 2026-11-06：评审（memo：`local/adr-058-review.md`），结论“有条件批准”，列出 M1—M11；
  确证 `fc3a0c0` 先于批准实现，run `42143683382` 三平台红。同日按条件收口：
  **与初稿的差异**：①busy 用 `backpressure`＋503，不用 `invalid_state`（评审异议 1 采纳）；
  ②不设期限常量与 orphan 表，改用 §2.6 显式上界（评审异议 2 采纳）；③新增 `llm.loss`
  帧；④`session`/`request_id` 进帧；⑤构造器收敛；⑥补 ADR-015 十一字段与 3 篇可核文献。
- 2026-11-06（并发协作冲突记录）：本 ADR 实现期间另一并行代理同时在本文件对应的
  `serve.rs` 上作业（新增 `llm.loss` 丢帧帧协议等未提交改动），两方补丁交错导致
  工作树一度不可编译；已停止对该文件的编辑并以 `942dd14` 为基线继续（教训入
  `whcode-verification-and-gotchas` 规则 26：同文件并发作业须先认领）。
- 2026-11-06：实现批次（M2—M5、M9—M11）与 6 条测试落地；M7 用例 7—11 与三指标归档
  仍未取得，T251/Check 347 保持未勾。
- 2026-11-09：M7 用例 7—10 落地并归档（`artifacts/turn-ws-relay-2026-11-09/`：
  `turn-ws-relay-e2e.json` 与 `turn-resources.json`）。**发现并修复两处缺陷**：①delta 帧未按
  WebSocket 编码（裸 JSON 写进 WS 流，浏览器会帧错位）；②`GET /`、`/chat` turn 期间返回
  未注入外壳 200，与 §2.3 清单不一致（按 §3.1“页面壳可取”收口，触库判据固定为
  `/api/sessions`）。§3.5 条件 4 补齐实测证据；用例 11 按 §3.6 缺口登记（回退树无工厂注入
  缝隙）。资源实验口径修正：基线须在装配与预热之后采样，否则一次性装配成本被算进 turn 增量
  （旧读数差 79—80 线程已作废，保留在归档的 `measurement_note` 中）。
- 2026-11-10：CI run `42680588379`（commit `239eeb6`）**全平台 success**（Windows／
  wins-arm64／FreeBSD／NetBSD／OpenBSD／DragonFly＋measurements），§3.5 条件 4 满足；
  T251 与 Check 347 同批转勾。此前两次红是同一测试时序假设（第二个 turn 因并发槽位被
  503 拒绝后仍等持锁），已由 `post_turn_until_accepted` 修正，不是产品缺陷。
