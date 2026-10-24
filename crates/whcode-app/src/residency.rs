//! # 会话驻留策略（T131、FR-614、FR-615 关联）
//!
//! 空闲会话卸载为"元数据 + 游标"（与 FR-102 一致）、按需唤醒、卸载不丢已确认事件、
//! 卸载与唤醒**可查询**（FR-614）；**禁止**会话级预分配固定大缓冲（资源纪律）。
//!
//! [`ResidencyManager`] 按会话维护驻留状态（`Resident` / `Unloaded`）与**有界的**
//! 事件缓冲：常驻会话只缓存最新若干条事件（条数与字节双上限，超限返回背压而不是
//! 静默丢弃）；卸载立即释放缓冲（内存可观测下降，[`ResidencyManager::accounted_bytes`]），
//! 只保留元数据与游标；唤醒从空缓冲开始，缺的历史由存储按游标补拉（FR-605），
//! 因此不会重复也不会缺失。
//!
//! 边界：内存状态按需创建，不随会话数线性叠加固定开销（FR-614）；卸载与唤醒的行为
//! 可查询（`state` / `unload_record` / `unloaded_sessions` / `event_cursor`）。

use std::collections::{BTreeMap, VecDeque};

use serde::Serialize;
use whcode_domain::{ErrorCode, WhError};
use whcode_store::query::EventRow;

/// 单会话驻留缓冲的事件条数上限（有界，NFR-215）。
pub const MAX_BUFFERED_EVENTS: usize = 200;

/// 单会话驻留缓冲的字节上限（按载荷与字段长度估算，NFR-215）。
pub const MAX_BUFFERED_BYTES: usize = 256 * 1024;

/// 每条事件的固定开销估算（结构体、指针与分配器元数据）。
const EVENT_OVERHEAD_BYTES: usize = 64;

/// 会话驻留状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ResidencyState {
    /// 常驻（可查询）。
    Resident,
    /// 已卸载为"元数据 + 游标"（与 FR-102 一致）。
    Unloaded,
}

impl ResidencyState {
    /// 稳定标识（可查询，FR-614）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resident => "resident",
            Self::Unloaded => "unloaded",
        }
    }
}

/// 卸载的会话保留的最小状态（元数据 + 游标，FR-614）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnloadedSession {
    /// 会话 ID。
    pub session_id: String,
    /// 卸载时的事件游标（补拉基线，FR-202/605）。
    pub event_cursor: i64,
    /// 卸载时刻（Unix 毫秒）。
    pub unloaded_at_ms: i64,
}

/// 常驻会话：有界事件缓冲（条数 + 字节双上限）。
#[derive(Debug, Default)]
struct ResidentSession {
    buffered: VecDeque<EventRow>,
    buffered_bytes: usize,
}

impl ResidentSession {
    /// 已缓存的最新事件游标；空缓冲返回 `None`。
    fn cursor(&self) -> Option<i64> {
        self.buffered.back().map(|event| event.seq)
    }
}

/// 单条事件的估算字节数（载荷 + 标识字段 + 固定开销）。
fn event_cost(event: &EventRow) -> usize {
    let optional = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    event.payload_json.len()
        + event.id.len()
        + event.kind.len()
        + event.persistence_level.len()
        + optional(&event.session_id)
        + optional(&event.task_id)
        + optional(&event.request_id)
        + optional(&event.tool_call_id)
        + EVENT_OVERHEAD_BYTES
}

/// 驻留管理器：按会话维护驻留状态与有界缓冲（按需创建，无固定大缓冲，FR-614）。
#[derive(Debug, Default)]
pub struct ResidencyManager {
    resident: BTreeMap<String, ResidentSession>,
    unloaded: BTreeMap<String, UnloadedSession>,
}

impl ResidencyManager {
    /// 空管理器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 标记会话常驻（按需唤醒/新建；缓冲从空开始，不预分配大缓冲）。
    pub fn wake(&mut self, session_id: &str) {
        self.unloaded.remove(session_id);
        self.resident.entry(session_id.to_string()).or_default();
    }

    /// 卸载空闲会话：丢弃缓冲，只保留元数据与游标（FR-614、FR-102）。
    ///
    /// 不丢已确认事件：游标之后的事件由补拉还原（FR-605）。
    pub fn unload(&mut self, session_id: &str, event_cursor: i64, now_ms: i64) -> UnloadedSession {
        self.resident.remove(session_id);
        let record = UnloadedSession {
            session_id: session_id.to_string(),
            event_cursor,
            unloaded_at_ms: now_ms,
        };
        self.unloaded.insert(session_id.to_string(), record.clone());
        record
    }

    /// 向常驻会话缓冲追加事件（有界；超限返回背压，不静默丢弃）。
    ///
    /// # Errors
    ///
    /// 事件缺少会话、会话已卸载（应先唤醒）、或缓冲达到条数/字节上限时返回错误
    /// （`invalid_state` / `backpressure`）。
    pub fn buffer_event(&mut self, event: EventRow) -> Result<(), WhError> {
        let session_id = event.session_id.clone().ok_or_else(|| {
            WhError::new(
                ErrorCode::InvalidState,
                format!("事件 {} 缺少会话，无法驻留", event.id),
            )
        })?;
        if self.unloaded.contains_key(&session_id) {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!("会话 {session_id} 已卸载：先唤醒再接收事件"),
            ));
        }
        let session = self.resident.entry(session_id.clone()).or_default();
        let cost = event_cost(&event);
        if session.buffered.len() >= MAX_BUFFERED_EVENTS
            || session.buffered_bytes + cost > MAX_BUFFERED_BYTES
        {
            return Err(WhError::new(
                ErrorCode::Backpressure,
                format!(
                    "会话 {session_id} 驻留缓冲已满（{} 条 / {} 字节上限：{} 条 / {} 字节）",
                    session.buffered.len(),
                    session.buffered_bytes,
                    MAX_BUFFERED_EVENTS,
                    MAX_BUFFERED_BYTES
                ),
            ));
        }
        session.buffered.push_back(event);
        session.buffered_bytes += cost;
        Ok(())
    }

    /// 驻留状态（可查询，FR-614）；未知会话按 `Resident`（按需创建）。
    #[must_use]
    pub fn state(&self, session_id: &str) -> ResidencyState {
        if self.unloaded.contains_key(session_id) {
            ResidencyState::Unloaded
        } else {
            ResidencyState::Resident
        }
    }

    /// 卸载记录（可查询，FR-614）。
    #[must_use]
    pub fn unload_record(&self, session_id: &str) -> Option<&UnloadedSession> {
        self.unloaded.get(session_id)
    }

    /// 已卸载的会话清单（可查询，FR-614）。
    #[must_use]
    pub fn unloaded_sessions(&self) -> Vec<&UnloadedSession> {
        self.unloaded.values().collect()
    }

    /// 会话事件游标：常驻取缓冲最新事件，卸载取记录中的游标，未知为 0。
    #[must_use]
    pub fn event_cursor(&self, session_id: &str) -> i64 {
        self.resident
            .get(session_id)
            .and_then(ResidentSession::cursor)
            .or_else(|| {
                self.unloaded
                    .get(session_id)
                    .map(|record| record.event_cursor)
            })
            .unwrap_or(0)
    }

    /// 常驻会话的缓冲事件条数。
    #[must_use]
    pub fn buffered_len(&self, session_id: &str) -> usize {
        self.resident
            .get(session_id)
            .map_or(0, |session| session.buffered.len())
    }

    /// 常驻会话的缓冲字节数（估算）。
    #[must_use]
    pub fn buffered_bytes(&self, session_id: &str) -> usize {
        self.resident
            .get(session_id)
            .map_or(0, |session| session.buffered_bytes)
    }

    /// 全部常驻会话的缓冲字节数合计（内存可观测指标，Check 221）。
    #[must_use]
    pub fn accounted_bytes(&self) -> usize {
        self.resident
            .values()
            .map(|session| session.buffered_bytes)
            .sum()
    }
}

/// 空闲卸载判定（纯函数）：距上次活动超过 `idle_window_ms` 才卸载。
#[must_use]
pub fn idle_unload_due(now_ms: i64, last_activity_ms: i64, idle_window_ms: i64) -> bool {
    now_ms.saturating_sub(last_activity_ms) >= idle_window_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: i64, session_id: &str, payload_bytes: usize) -> EventRow {
        EventRow {
            seq,
            id: format!("E-{seq}"),
            session_id: Some(session_id.to_string()),
            task_id: None,
            request_id: None,
            tool_call_id: None,
            kind: "StreamingChunk".to_string(),
            payload_json: "x".repeat(payload_bytes),
            ts_us: seq * 1_000,
            persistence_level: "redacted".to_string(),
        }
    }

    #[test]
    fn unload_preserves_metadata_and_cursor_and_is_queryable() {
        let mut manager = ResidencyManager::new();
        assert_eq!(manager.state("S-1"), ResidencyState::Resident);

        let record = manager.unload("S-1", 42, 1_000);
        assert_eq!(record.event_cursor, 42, "卸载保留元数据 + 游标（FR-614）");
        assert_eq!(manager.state("S-1"), ResidencyState::Unloaded);
        assert_eq!(
            manager.unload_record("S-1").expect("记录").unloaded_at_ms,
            1_000,
            "卸载行为可查询（FR-614）"
        );
        assert_eq!(manager.unloaded_sessions().len(), 1);

        // 按需唤醒：回到常驻，卸载记录清除。
        manager.wake("S-1");
        assert_eq!(manager.state("S-1"), ResidencyState::Resident);
        assert!(manager.unload_record("S-1").is_none());
    }

    #[test]
    fn idle_decision_is_pure_and_window_boundary_inclusive() {
        assert!(!idle_unload_due(10_000, 9_500, 1_000));
        assert!(idle_unload_due(10_000, 9_000, 1_000), "恰好到窗即视为到期");
        assert!(!idle_unload_due(500, 1_000, 1_000), "时钟回退不得误判");
    }

    /// Check 221：卸载释放缓冲——占用统计归零，游标保留；唤醒从空缓冲开始。
    #[test]
    fn unload_releases_buffer_memory_and_wake_starts_empty() {
        let mut manager = ResidencyManager::new();
        manager.wake("S-1");
        for seq in 1..=3 {
            manager.buffer_event(event(seq, "S-1", 128)).expect("缓冲");
        }
        assert_eq!(manager.buffered_len("S-1"), 3);
        assert!(manager.accounted_bytes() > 0, "驻留占用应可观测");
        assert_eq!(manager.event_cursor("S-1"), 3, "游标取缓冲最新事件");

        let record = manager.unload("S-1", 3, 2_000);
        assert_eq!(record.event_cursor, 3);
        assert_eq!(manager.accounted_bytes(), 0, "卸载后占用必须归零");
        assert_eq!(manager.buffered_len("S-1"), 0);
        assert_eq!(manager.event_cursor("S-1"), 3, "卸载后游标取自记录");

        manager.wake("S-1");
        assert_eq!(manager.accounted_bytes(), 0, "唤醒不预分配缓冲");
        assert_eq!(
            manager.event_cursor("S-1"),
            0,
            "唤醒后缓冲为空，游标由调用方补拉推进"
        );
    }

    /// Check 221：缓冲有界——条数与字节双上限，超限返回背压而不是静默丢弃。
    #[test]
    fn buffering_is_bounded_by_count_and_bytes_with_backpressure() {
        let mut manager = ResidencyManager::new();
        manager.wake("S-1");
        for seq in 1..=MAX_BUFFERED_EVENTS as i64 {
            manager
                .buffer_event(event(seq, "S-1", 8))
                .expect("未到上限");
        }
        let err = manager
            .buffer_event(event(MAX_BUFFERED_EVENTS as i64 + 1, "S-1", 8))
            .expect_err("条数上限必须拒绝");
        assert_eq!(err.code, ErrorCode::Backpressure, "{err}");
        assert!(err.retryable, "背压可重试");
        assert_eq!(
            manager.buffered_len("S-1"),
            MAX_BUFFERED_EVENTS,
            "拒绝时不得改变缓冲"
        );

        // 字节上限：单条超大事件直接拒绝（不截断载荷）。
        let mut manager = ResidencyManager::new();
        manager.wake("S-2");
        let err = manager
            .buffer_event(event(1, "S-2", MAX_BUFFERED_BYTES))
            .expect_err("字节上限必须拒绝");
        assert_eq!(err.code, ErrorCode::Backpressure);
        assert_eq!(manager.buffered_len("S-2"), 0);
        assert_eq!(manager.accounted_bytes(), 0);
    }

    /// Check 221：已卸载会话收到事件必须显式失败（不静默写入丢失的缓冲）。
    #[test]
    fn buffering_into_unloaded_session_is_rejected() {
        let mut manager = ResidencyManager::new();
        manager.unload("S-1", 5, 1_000);
        let err = manager
            .buffer_event(event(6, "S-1", 8))
            .expect_err("已卸载必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = manager
            .buffer_event(EventRow {
                session_id: None,
                ..event(7, "S-1", 8)
            })
            .expect_err("缺少会话必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    /// Check 221：卸载前缓冲 + 唤醒后补拉 = 全集且无重复（FR-605）。
    #[test]
    fn unload_then_wake_replays_without_duplicates_or_gaps() {
        use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
        use crate::replay::{REPLAY_PAGE_LIMIT, replay_since};
        use duckdb::params;
        use whcode_store::{apply_migrations, create_session_with_id};

        let conn = duckdb::Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        create_session_with_id(&conn, "S-1", "驻留", "active", 1_000).expect("创建会话");
        for seq in 1..=10 {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, 'S-1', NULL, NULL, NULL, 'StreamingChunk', CAST('{}' AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                params![seq, format!("E-{seq}")],
            )
            .expect("插入事件");
        }

        let mut manager = ResidencyManager::new();
        manager.wake("S-1");
        for seq in 1..=4 {
            manager.buffer_event(event(seq, "S-1", 8)).expect("缓冲");
        }
        let buffered: Vec<i64> = (1..=4).collect();
        manager.unload("S-1", 4, 2_000);
        manager.wake("S-1");

        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let page = replay_since(&conn, &policy, &actor, "S-1", 4, REPLAY_PAGE_LIMIT).expect("补拉");
        assert!(page.complete, "游标之后应一次补齐");
        let replayed: Vec<i64> = page.events.iter().map(|row| row.seq).collect();
        assert_eq!(replayed, (5..=10).collect::<Vec<_>>(), "补拉严格在游标之后");

        let mut all = buffered;
        all.extend(replayed);
        all.sort_unstable();
        assert_eq!(
            all,
            (1..=10).collect::<Vec<_>>(),
            "缓冲 + 补拉必须是全集且无重复"
        );
    }
}
