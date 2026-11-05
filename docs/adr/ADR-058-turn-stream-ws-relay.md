# ADR-058: turn 执行移出 serve 循环线程——WS 逐块增量转发（FR-742 剩余项）

- **状态**：**提议（待用户批准）**——2026-11-06 起草；批准前不动 `serve.rs`。
- **日期**：2026-11-06
- **关联**：ADR-057（批次 B1 的 T251 剩余项）、ADR-015（设计先行并发纪律：本 ADR 即
  并发设计档案）、ADR-044（turn 循环）、ADR-022（Web 安全默认）、ADR-037（CI 唯一验收）。
- **落点**：`crates/whcode-bin/src/serve.rs`（单线程多路复用循环＋HTTP 处理）、
  `crates/whcode-web/src/api.rs`（llm.run 路由）。

## 1. 事实与证据（第 1 层：源码）

- `serve_loop` 为**单线程非阻塞多路复用**（serve.rs:604 起模块档案）：accept → 逐 client
  泵 → 每 `EVENT_POLL_INTERVAL` 轮询 `poll_events` 广播。HTTP（含 POST /api/llm/run）
  在 `respond()` 内**同步执行**（api.rs llm 路由直调 `whcode_app::turn::run_turn`）。
- 同步 turn 期间（数秒至数十秒）：循环停摆——WS 泵不推进、事件不轮询、其他客户端
  HTTP 不被接受。turn 结束后积压帧一次性涌出。
- **StreamingChunk 事件已落库但 payload 不含正文**（turn.rs `emit!(EventKind::StreamingChunk,
  "iteration"/"request_id"/"index"/"chars")`；FR-722“payload 不含正文”是登记契约）——
  WS 泵广播的只有计数骨架，不是逐块正文。
- 增量正文通道已使能（`f9c5cf6`）：`TurnProgress` 回调＝`(累计块数, 增量正文)`；
  GUI 经有损有界通道消费（ADR-056 Phase 2 模式）。
- ZCode 参照（`research/zcode-comparison/domain-M2-ui-experience.md` §①）：对话流为
  虚拟滚动历史区＋独立实时尾双通道；停止入口在输入框。
- 连接共享先例：ADR-056 Phase 2 已把工作区连接改为 `Arc<Mutex<Connection>>`（GUI/TUI
  执行器跨线程），`gui_executors_are_send_sync` 编译期证明在库——**同构路径已验证**。

## 2. 决策（并发设计档案）

### 2.1 方案比选

| 方案 | 语义影响 | 并发影响 | 判定 |
|---|---|---|---|
| A. `llm.run` 改流式响应（SSE/chunked） | **破坏登记契约**（三端同步等待完成，feature-parity 登记） | 服务器仍单线程 | 否决（契约变更无需求方） |
| B. 正文入 StreamingChunk payload | 破坏 FR-722“payload 不含正文”＋脱敏口径 | 无 | 否决 |
| C. 维持现状（burst 后补） | 无 | 无 | 否决（FR-742 未满足） |
| **D. turn 移出循环线程＋等待期联合泵（本 ADR）** | **无**（HTTP 响应仍在 turn 完成后返回，同步契约保持） | 每活跃 turn 一个临时工作线程（有界、RAII join） | **采纳** |

### 2.2 采纳方案的结构

1. **工作区连接升级为 `Arc<Mutex<Connection>>`**（与 GUI/TUI 执行器同构，ADR-056
   Phase 2 先例）：serve_loop 与 turn 工作线程共享；互斥保持“单连接＝唯一写路径”
   （Check 98）。
2. **turn 工作线程**：`POST /api/llm/run` 路由改为——校验后 spawn `std::thread`，线程内
   经既有执行器封装调 `run_with_progress`（`TurnProgress` 回调把 `(块数, 增量正文)`
   经**有界有损通道**送达循环线程，满即丢弃——与 GUI 同口径；通道有效上界＝turn
   输出预算 1 MiB/次＋块数上限 FR-719，构造性有界，不依赖无界容器）；完成经
   oneshot 保证送达（同 GUI 双通道模式）。
3. **新客户端相位 `Phase::TurnPending`**：发起 turn 的 HTTP 客户端进入该相位，
   **保留响应待写权**。循环每 tick：`try_recv` 增量 → 组装 delta JSON 帧 →
   `BroadcastState::broadcast_frame` 广播给全部 WS 订阅者＋`flush_queues`（复用既有
   慢客户端策略：队列满即断，不停他人）；`done_rx` 就绪 → 组装与现行**完全相同**的
   JSON 信封 → 写回该客户端。RAII：JoinHandle 在相位结束时 join，无常驻线程。
4. **并发上限**：同时活跃 turn 工作线程 ≤ 2（超出即 `invalid_state` 显式拒绝，不排队）；
   FR-723“单会话至多一个活跃 turn”语义不变（经 store `.live` 标记照旧生效）。
5. **turn 运行期的循环纪律**（与 GUI 冻结同构）：`poll_events` **跳过**（连接锁被
   工作线程持有；增量经直接通道承载实时流量，turn 结束后事件批次自然补齐）；
   其他需要连接锁的请求路径在锁忙时返回 `invalid_state`（busy，显式可见，不静默
   阻塞——单线程循环内阻塞等待＝冻结全体，禁用）。
6. **安全**：delta 帧含模型正文——仅送已通过握手的 WS 订阅者（认证模式沿用既有
   WS 准入门；回环未认证模式与现行服务端直出页面同信任级）。脱敏口径不变
   （正文经 turn 输出管线，事件 payload 仍不含正文）。
7. **取消**：标记目录跨线程机制不变（turn 内 chunk 边界生效）；取消/失败/成功三态
   都经 oneshot 回循环，相位照常收敛。

### 2.3 一手文献依据（ADR-015 口径）

- Rust 标准库 `std::sync::mpsc` 文档（`try_recv` 非阻塞语义；通道作为线程间消息
  传递的一手定义）——增量通道的消费侧永不阻塞循环。
- Rust 标准库 `std::sync::Mutex` 文档（`try_lock` 语义）——锁忙请求路径的显式失败
  依据；以及 RAII（`MutexGuard` 释放）作为无遗忘解锁的结构保证。
- 本仓一手先例：ADR-056 Phase 2（GUI turn 后台执行，`Arc<Mutex<Connection>>`＋
  有界有损通道＋oneshot 完成保证，已实测）＋serve.rs 既有并发档案（单线程多路复用
  的正负与失效条件）——本 ADR 是该两份档案的**组合应用**，不引入新的并发原语。

### 2.4 正负和解

- **正面**：FR-742 的 WS 逐块推送达成（~tick 粒度）；llm.run 同步契约零变化；
  curl/脚本行为不变；GUI/Web/TUI 三端同语义维持。
- **负面**：turn 期间事件轮询暂停（结束后批次补齐——观察口径不变，实时性由
  delta 帧承担）；`Arc<Mutex<Connection>>` 使 serve 路径从直接借用改为加锁
  （tick 内锁竞争失败→busy 响应，极端场景其他会话 HTTP 读面在 turn 期间可能
  遇到 busy）；每活跃 turn 一个临时线程（≤2，RAII join）；实现面涉及循环相位
  状态机改动（回归风险集中在本文件）。
- **承担者**：serve 维护者（实现与回归）、turn 期间的其他 Web 用户（读面可能
  遇 busy——显式错误可重试）。
- **和解理由**：收益（FR-742 完整达成）与代价（turn 期间局部 busy、可回退）
  不对称小；替代方案全部要求破坏已登记契约或脱敏口径。

### 2.5 回退验证

- 回退＝git revert 本批次（无数据迁移、无 schema 变化、无配置项）。
- 同夹具前后测量：WS 客户端订阅＋`curl POST /api/llm/run`（StubProvider 定速剧本）
  ——记录（a）首帧 delta 相对 turn 开始的延迟（前＝turn 结束后一次性涌出，后＝
  tick 粒度）；（b）turn 期间 HTTP /api/status 的响应（前＝阻塞至 turn 结束，后＝
  立即 200 或显式 busy）；（c）turn 结束后事件批次完整性（`event_missing` 恒 0）。
  归档 `artifacts/turn-ws-relay-<date>/`，回退复测同夹具对照。

## 3. 三重时间

- **过去来源**：ADR-056 Phase 2（连接共享＋有损通道已实测）；serve.rs 并发档案
  （单线程多路复用的既定结构）；domain-M2 §①（ZCode 双通道对话流参照）；
  FR-722/723/719 既有契约。
- **现在证据**：serve_loop 单线程结构（源码）；GUI 后台执行路径已验证同构；
  T251 页面与 tail API 已交付（页面已具备消费增量的前端挂点，chat.js 预留）。
- **将来失效条件与复审触发**（绑定命令＋判定）：
  - `cargo test --locked -p whcode-bin` serve 测试失败即复审本 ADR 实现；
  - WS 逐块验收（回退验证节同夹具）任一指标不达即复审；
  - `python .github/scripts/benchmarks/measure-resources.py` 资源铁律失守
    （临时线程泄漏会在线程计数显现）即复审；
  - 若未来引入 `llm.run` 流式契约需求（SSE），本 ADR 由新 ADR 取代（D 方案
    是流式契约的兼容前置，不互斥）。

## 4. 复审触发（绑定命令＋判定）

- `cargo test --locked -p whcode-bin` 失败 → 复审（判定：退出码非 0）。
- 回退验证三项指标任一不达 → 复审（判定：见 §2.5 归档报告字段）。
- `traceability.py`/`check_counts.py` 不一致 → 当批不得结题（既有绑定）。

## 5. 修订记录

- 2026-11-05：初稿（提议，待用户批准）。批准后：①T251 剩余片实施（Phase::TurnPending
  ＋工作线程＋delta 广播）；②Check 347 取证；③serve.rs 模块档案增补第 2 节。
