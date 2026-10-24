//! # WebSocket 事件广播（Check 199/209、FR-603 事件推送、T094 收口）
//!
//! 工作区级事件流的**纯逻辑层**：订阅者表、有界出站队列与增量拉取封装。连接管理
//! （socket 读写、握手、帧泵）属装配根 `whcode-bin::serve`；本模块只回答
//! “哪个订阅者还欠哪些帧”，不持有 socket——与 [`crate::ws`] 的字节变换边界一致。
//!
//! ## 广播语义（v1）
//! 工作区级全量广播，不分会话订阅（`subscribe(session)` 属未来任务，本注释即声明）。
//! 帧载荷为单条事件的 JSON（字段与 `/api/events` 导出一致）；脱敏级别取
//! [`TraceLevel::default`]（`redacted`，FR-205 安全优先默认）。显式边界（非静默）：
//! `session_id` 为 NULL 的事件不经会话枚举导出，v1 不广播——正式写路径均携带会话
//! 归属，此类事件当前不存在。
//!
//! ## 并发设计档案（ADR-015 口径）
//! - **选型**：单线程所有权。`BroadcastState` 只在 serve 单循环线程内访问，无内部
//!   锁、不要求 `Sync`、零新增线程（NFR-211：不得为每连接新增常驻线程）。
//! - **正**：无锁竞争与调度抖动；每客户端出站 ≤ [`MAX_QUEUED_FRAMES`] 帧、订阅者
//!   ≤ [`MAX_WS_CLIENTS`]，内存上界可静态推导（有界容器，NFR-215）。
//! - **负（承担者：广播尾延迟与单核 CPU）**：一次广播成本 O(订阅者数) × O(帧数)，
//!   worst-case ≈ 32 × 64 = 2 048 次入队/批；广播延迟下界 = 事件轮询周期（200 ms）；
//!   吞吐上限 = [`POLL_BATCH_FRAMES`] × 轮询频率 = 320 事件/秒。
//! - **失效条件**：事件产生速率持续超过 320 事件/秒（尾延迟线性增长）、订阅者接近
//!   上限、或单帧接近 1 MiB 时单核成为瓶颈；实测出现该瓶颈时不再适配本设计。
//! - **回退路径**：本模块为纯逻辑，另立 ADR 评估线程池/async 运行时后，可在不动
//!   协议字节变换（[`crate::ws`]）与安全判定（[`crate::security`]）的前提下替换
//!   并发模型——当前无测量证据支持其内存与调度成本，不做原型先行（NFR-216/217）。

use std::collections::{HashMap, VecDeque};

use duckdb::Connection;
use whcode_app::{Actor, Policy};
use whcode_domain::{TraceLevel, WhError};

use crate::security;
use crate::ws;

/// 每订阅者出站队列容量（帧数；有界容器，NFR-215）。
pub const MAX_QUEUED_FRAMES: usize = 64;

/// 订阅者上限：达到时新连接在握手后回 `1013 Try Again Later` 并断开（显式拒绝，不静默）。
pub const MAX_WS_CLIENTS: usize = 32;

/// 单次轮询对每个会话拉取的事件数上限（`events_export` 上限 1000 内取半，留批处理余量）。
pub const POLL_PAGE_LIMIT: usize = 500;

/// 单次轮询产出的广播帧数上限（= 每订阅者出站队列容量 [`MAX_QUEUED_FRAMES`]）。
///
/// 取等号是慢客户端判定的**公平性基础**：一批帧即使全部入队也不溢出任何队列——
/// 于是队列溢出当且仅当客户端在整个轮询周期（200 ms）内一帧未消费，即真慢。
/// 吞吐上限 = 批大小 × 轮询频率（64 × 5 = 320 事件/秒），更高速率时尾延迟线性
/// 增长（失效条件，见模块文档）。
pub const POLL_BATCH_FRAMES: usize = MAX_QUEUED_FRAMES;

/// 订阅者标识（服务端分配，进程内单调递增）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(u64);

/// 出站入队结果：队列已满时为 [`PushResult::Overflowed`]，调用方据此断开该客户端
/// （慢客户端策略：断开慢者，不停别人——Check 199）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushResult {
    /// 已入队。
    Queued,
    /// 队列已满，本帧被拒：该客户端消费速度跟不上广播速率。
    Overflowed,
}

/// 有界出站队列（每订阅者一个；容量 [`MAX_QUEUED_FRAMES`]）。
#[derive(Debug, Default)]
pub struct ClientQueue {
    frames: VecDeque<Vec<u8>>,
}

impl ClientQueue {
    fn new() -> Self {
        Self::default()
    }

    /// 入队一帧；满时返回 [`PushResult::Overflowed`]（本帧不入队，也不挤掉旧帧——
    /// 部分丢帧会让订阅者错过事件且不自知，断开重连是唯一诚实语义）。
    pub fn push(&mut self, frame: Vec<u8>) -> PushResult {
        if self.frames.len() >= MAX_QUEUED_FRAMES {
            return PushResult::Overflowed;
        }
        self.frames.push_back(frame);
        PushResult::Queued
    }

    /// 出队一帧（写 socket 成功后调用方再取下一帧；PartialWrite 残余由调用方暂存）。
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        self.frames.pop_front()
    }

    /// 当前积压帧数（诊断用）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// 是否无积压。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// 订阅者表与广播状态（单线程所有权，见模块文档并发设计档案）。
#[derive(Debug, Default)]
pub struct BroadcastState {
    next_id: u64,
    subscribers: HashMap<ClientId, ClientQueue>,
}

impl BroadcastState {
    /// 空状态（无订阅者）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册订阅者，分配新 id；达到 [`MAX_WS_CLIENTS`] 时显式拒绝
    /// （调用方回 1013 后断开，不得静默丢弃连接）。
    ///
    /// # Errors
    ///
    /// 订阅者已达 [`MAX_WS_CLIENTS`] 时返回 `invalid_state`。
    pub fn register(&mut self) -> Result<ClientId, WhError> {
        if self.subscribers.len() >= MAX_WS_CLIENTS {
            return Err(WhError::invalid_state(format!(
                "WebSocket 订阅者已达上限 {MAX_WS_CLIENTS}（回 1013 Try Again Later，RFC 6455 §7.4.1）"
            )));
        }
        let id = ClientId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.subscribers.insert(id, ClientQueue::new());
        Ok(id)
    }

    /// 注销订阅者（幂等：不存在的 id 返回 `false`，不报错——重复断开安全）。
    pub fn unregister(&mut self, id: ClientId) -> bool {
        self.subscribers.remove(&id).is_some()
    }

    /// 取订阅者出站队列（服务循环写 socket 时逐帧弹出）。
    pub fn queue_mut(&mut self, id: ClientId) -> Option<&mut ClientQueue> {
        self.subscribers.get_mut(&id)
    }

    /// 当前订阅者数。
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    /// 慢客户端报告：出站积压达到队列容量一半即计入（诊断用；断开决策由
    /// [`BroadcastState::broadcast_frame`] 返回的 Overflowed 驱动）。
    #[must_use]
    pub fn slow_clients(&self) -> Vec<ClientId> {
        let threshold = MAX_QUEUED_FRAMES / 2;
        let mut slow: Vec<ClientId> = self
            .subscribers
            .iter()
            .filter(|(_, queue)| queue.len() >= threshold)
            .map(|(id, _)| *id)
            .collect();
        slow.sort_unstable();
        slow
    }

    /// 广播一条文本：超 [`security::MAX_MESSAGE_BYTES`] 的单条消息拒绝（不拆帧、
    /// 不截断）；返回因队列满而溢出的订阅者（调用方断开——Check 209）。
    ///
    /// # Errors
    ///
    /// 载荷超过 [`security::MAX_MESSAGE_BYTES`] 时返回 `invalid_state`。
    pub fn broadcast_text(&mut self, payload: &str) -> Result<Vec<ClientId>, WhError> {
        security::check_message_size(payload.len())?;
        let frame = ws::encode_text_frame(payload.as_bytes());
        Ok(self.broadcast_frame(frame))
    }

    /// 广播一帧已编码数据（服务循环用：[`poll_events`] 返回的帧直接分发）；
    /// 返回溢出的订阅者 id（升序，确定性便于测试）。
    pub fn broadcast_frame(&mut self, frame: Vec<u8>) -> Vec<ClientId> {
        let mut overflowed = Vec::new();
        for (id, queue) in self.subscribers.iter_mut() {
            if queue.push(frame.clone()) == PushResult::Overflowed {
                overflowed.push(*id);
            }
        }
        overflowed.sort_unstable();
        overflowed
    }
}

/// 一次增量轮询结果。
#[derive(Debug)]
pub struct PollBatch {
    /// 已编码的服务端文本帧（按 `seq` 升序；空 = 无新事件，不广播）。
    pub frames: Vec<Vec<u8>>,
    /// 新游标（已消费到的最大 `seq`；调用方持久，下一轮作为 `after` 传入）。
    pub cursor: i64,
}

/// 增量拉取封装：工作区全部会话在 `after` 之后的事件 → 广播帧（转调
/// `whcode_app::events_export`，前端不触库，ADR-003/018；借用签名与之一致）。
///
/// 实现：先经 `build_snapshot().event_cursor`（工作区最大 `seq`，同一底层
/// `query::latest_seq`）做空闲短路——无新事件时不逐会话查询（空闲成本 = 一次会话
/// 列表 + 一次快照，而非 200 次导出）；有新事件时逐会话增量导出后按 `seq` 归并
/// （`seq` 工作区唯一且单调递增）。单帧超限（序列化后 > [`security::MAX_MESSAGE_BYTES`]）
/// **跳过并推进游标**（不拆帧、不截断；跳过经 stderr 显式留痕），游标仍前进避免
/// 死循环。单次轮询至多产出 [`POLL_BATCH_FRAMES`] 帧（= 出站队列容量，多余事件
/// 由游标下一轮继续），保证一批帧不会挤爆任何订阅者队列。
///
/// # Errors
///
/// 会话列表或事件导出失败时返回 [`WhError`]（游标未推进，调用方下一轮重试，不丢事件）。
pub fn poll_events(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    after: i64,
) -> Result<PollBatch, WhError> {
    let sessions =
        whcode_app::list_sessions(conn, policy, actor, true, whcode_app::LIST_SESSIONS_LIMIT)?;
    if sessions.is_empty() {
        return Ok(PollBatch {
            frames: Vec::new(),
            cursor: after,
        });
    }
    // 空闲短路；快照失败（如首个会话恰被删除）按“可能有新事件”处理——退化为全量
    // 逐会话查询。宁可多查不可漏：游标未推进前漏掉即永久丢失。
    let latest = match whcode_app::build_snapshot(conn, policy, actor, &sessions[0].id) {
        Ok(snapshot) => snapshot.event_cursor,
        Err(_) => i64::MAX,
    };
    if latest <= after {
        return Ok(PollBatch {
            frames: Vec::new(),
            cursor: after,
        });
    }
    let mut rows = Vec::new();
    for session in &sessions {
        let export = whcode_app::events_export(
            conn,
            policy,
            actor,
            &session.id,
            after,
            POLL_PAGE_LIMIT,
            TraceLevel::default(),
        )?;
        rows.extend(export.events);
    }
    rows.sort_by_key(|row| row.seq);
    rows.dedup_by(|a, b| a.seq == b.seq);
    rows.truncate(POLL_BATCH_FRAMES);
    let mut frames = Vec::with_capacity(rows.len());
    let mut cursor = after;
    for row in &rows {
        cursor = row.seq;
        let payload = serde_json::to_string(row)
            .map_err(|err| WhError::parse_error(format!("事件 {} 序列化失败：{err}", row.id)))?;
        match security::check_message_size(payload.len()) {
            Ok(()) => frames.push(ws::encode_text_frame(payload.as_bytes())),
            Err(_) => eprintln!(
                "广播跳过超限事件 {}（{} 字节 > {}，不拆帧；游标仍推进）",
                row.id,
                payload.len(),
                security::MAX_MESSAGE_BYTES
            ),
        }
    }
    Ok(PollBatch { frames, cursor })
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_app::{DefaultPolicy, actor_with_all_capabilities};
    use whcode_domain::ErrorCode;

    /// 与本 crate 既有夹具一致（api.rs tests）：直接 include 迁移 SQL——
    /// web 层不得依赖 whcode-store（tests/contract/layering 门禁）。
    const TEST_MIGRATIONS: &str = concat!(
        include_str!("../../whcode-store/migrations/0001_initial.sql"),
        include_str!("../../whcode-store/migrations/0002_event_trace_indexes.sql"),
        include_str!("../../whcode-store/migrations/0003_query_indexes.sql"),
        include_str!("../../whcode-store/migrations/0004_session_id_sequence.sql"),
        include_str!("../../whcode-store/migrations/0005_session_stage.sql"),
        include_str!("../../whcode-store/migrations/0006_events_variant.sql"),
    );

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(TEST_MIGRATIONS).expect("迁移");
        conn
    }

    fn seed_session(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status)
             VALUES (?, '广播测试', current_timestamp, current_timestamp, NULL, FALSE, 'active')",
            [id],
        )
        .expect("创建会话");
    }

    fn seed_event(conn: &Connection, session_id: &str, seq: i64, payload: &str) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', CAST(? AS JSON)::VARIANT, current_timestamp, 'redacted')",
            duckdb::params![seq, format!("E-{seq}"), session_id, payload],
        )
        .expect("插入事件");
    }

    /// 服务端帧不掩码：剥掉帧头取载荷（测试断言用）。
    fn frame_payload(frame: &[u8]) -> &[u8] {
        let header_len = match (frame[1] & 0x7F) as usize {
            126 => 4,
            127 => 10,
            _ => 2,
        };
        &frame[header_len..]
    }

    #[test]
    fn register_and_unregister_are_idempotent_and_bounded() {
        let mut state = BroadcastState::new();
        let first = state.register().expect("注册");
        let second = state.register().expect("注册");
        assert_ne!(first, second, "id 单调不重复");
        assert!(state.unregister(first), "注销已注册 id");
        assert!(!state.unregister(first), "重复注销幂等（不报错）");
        assert_eq!(state.subscriber_count(), 1);
        // 注册至上限后，第 33 个必须显式拒绝（不静默）。
        while state.subscriber_count() < MAX_WS_CLIENTS {
            state.register().expect("上限内注册");
        }
        let err = state.register().expect_err("超上限必须显式拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.to_string().contains("1013"),
            "错误须指出 1013 语义：{err}"
        );
        assert_eq!(state.subscriber_count(), MAX_WS_CLIENTS);
    }

    /// 慢客户端策略：断开慢者，不停别人（Check 199 的逻辑层证据）。
    #[test]
    fn overflowing_queue_disconnects_only_the_slow_client() {
        let mut state = BroadcastState::new();
        let fast = state.register().expect("注册");
        let slow = state.register().expect("注册");
        // 模拟服务循环：fast 每帧后即被写空（正常读），slow 从不消费。
        let mut overflowed_total = Vec::new();
        for seq in 0..(MAX_QUEUED_FRAMES + 1) {
            overflowed_total.extend(
                state
                    .broadcast_text(&format!(r#"{{"seq":{seq}}}"#))
                    .expect("载荷合法"),
            );
            let queue = state.queue_mut(fast).expect("fast 在册");
            while queue.pop().is_some() {}
        }
        assert_eq!(overflowed_total, vec![slow], "只有从不消费的 slow 溢出");
        assert!(state.queue_mut(fast).expect("fast 在册").is_empty());
        assert_eq!(
            state.queue_mut(slow).expect("slow 在册").len(),
            MAX_QUEUED_FRAMES,
            "slow 队列停在容量上限（第 65 帧被拒）"
        );
    }

    #[test]
    fn oversized_broadcast_text_is_rejected_without_queueing() {
        let mut state = BroadcastState::new();
        let id = state.register().expect("注册");
        let payload = "x".repeat(security::MAX_MESSAGE_BYTES + 1);
        let err = state.broadcast_text(&payload).expect_err("超限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            state.queue_mut(id).expect("在册").is_empty(),
            "拒绝时不得入队（不拆帧、不截断）"
        );
        // 恰好等于上限：允许（与 check_message_size 语义一致）。
        let exact = "y".repeat(security::MAX_MESSAGE_BYTES);
        assert!(state.broadcast_text(&exact).expect("上限内").is_empty());
        assert_eq!(state.queue_mut(id).expect("在册").len(), 1);
    }

    #[test]
    fn slow_clients_report_at_half_capacity() {
        let mut state = BroadcastState::new();
        let id = state.register().expect("注册");
        for seq in 0..(MAX_QUEUED_FRAMES / 2 - 1) {
            state
                .broadcast_text(&format!(r#"{{"seq":{seq}}}"#))
                .expect("载荷合法");
        }
        assert!(state.slow_clients().is_empty(), "未达容量一半不算慢");
        state.broadcast_text(r#"{"seq":99}"#).expect("载荷合法");
        assert_eq!(state.slow_clients(), vec![id], "达到容量一半即报告");
        let queue = state.queue_mut(id).expect("在册");
        while queue.pop().is_some() {}
        assert!(state.slow_clients().is_empty(), "清空后不再报告");
    }

    #[test]
    fn broadcast_with_no_subscribers_is_a_noop() {
        let mut state = BroadcastState::new();
        assert!(
            state
                .broadcast_text(r#"{"seq":1}"#)
                .expect("载荷合法")
                .is_empty()
        );
        let id = state.register().expect("注册");
        assert!(state.unregister(id));
        assert!(
            state
                .broadcast_text(r#"{"seq":2}"#)
                .expect("载荷合法")
                .is_empty()
        );
    }

    #[test]
    fn poll_events_empty_batch_does_not_broadcast() {
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        // 空工作区：空批，游标不动。
        let batch = poll_events(&conn, &policy, &actor, 7).expect("轮询");
        assert!(batch.frames.is_empty());
        assert_eq!(batch.cursor, 7);
        // 有会话无事件：同样空批。
        seed_session(&conn, "S-1");
        let batch = poll_events(&conn, &policy, &actor, 0).expect("轮询");
        assert!(batch.frames.is_empty());
        assert_eq!(batch.cursor, 0);
        // 游标已到工作区最大 seq：不重复广播旧事件。
        seed_event(&conn, "S-1", 1, r#"{"n":1}"#);
        seed_event(&conn, "S-1", 2, r#"{"n":2}"#);
        let batch = poll_events(&conn, &policy, &actor, 2).expect("轮询");
        assert!(batch.frames.is_empty(), "游标之后无事件不得广播");
        assert_eq!(batch.cursor, 2);
    }

    #[test]
    fn poll_events_orders_frames_by_seq_and_advances_cursor() {
        let conn = migrated();
        seed_session(&conn, "S-1");
        seed_session(&conn, "S-2");
        // 跨会话插入：seq 是工作区唯一序号，归并后必须升序不重复。
        seed_event(&conn, "S-1", 1, r#"{"n":1}"#);
        seed_event(&conn, "S-2", 2, r#"{"n":2}"#);
        seed_event(&conn, "S-1", 3, r#"{"n":3}"#);
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let batch = poll_events(&conn, &policy, &actor, 0).expect("轮询");
        assert_eq!(batch.frames.len(), 3);
        let seqs: Vec<i64> = batch
            .frames
            .iter()
            .map(|frame| {
                let event: serde_json::Value =
                    serde_json::from_slice(frame_payload(frame)).expect("帧载荷为 JSON");
                event["seq"].as_i64().expect("seq 字段")
            })
            .collect();
        assert_eq!(seqs, vec![1, 2, 3], "跨会话按工作区 seq 升序");
        assert_eq!(batch.cursor, 3);
        // 续传：游标之后只有新事件，不重复。
        seed_event(&conn, "S-1", 4, r#"{"n":4}"#);
        let batch = poll_events(&conn, &policy, &actor, batch.cursor).expect("续传");
        assert_eq!(batch.frames.len(), 1);
        assert_eq!(batch.cursor, 4);
    }

    #[test]
    fn poll_events_skips_oversized_event_and_advances_cursor() {
        let conn = migrated();
        seed_session(&conn, "S-1");
        let big = format!(r#"{{"pad":"{}"}}"#, "x".repeat(security::MAX_MESSAGE_BYTES));
        seed_event(&conn, "S-1", 1, &big);
        seed_event(&conn, "S-1", 2, r#"{"n":2}"#);
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let batch = poll_events(&conn, &policy, &actor, 0).expect("轮询");
        assert_eq!(batch.frames.len(), 1, "超限帧被跳过（不拆帧）");
        assert_eq!(batch.cursor, 2, "游标仍越过超限事件，避免死循环");
    }
}
