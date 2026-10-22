//! # 会话驻留策略（T131、FR-614、FR-615 关联）
//!
//! 空闲会话卸载为"元数据 + 游标"（与 FR-102 一致）、按需唤醒、卸载不丢已确认事件、
//! 卸载与唤醒**可查询**（FR-614）；**禁止**会话级预分配固定大缓冲（资源纪律）。
//!
//! [`ResidencyManager`] 按会话维护驻留状态（`Resident` / `Unloaded`），卸载判定为
//! 纯函数（空闲窗口），卸载动作只保留元数据与游标——有界加载语义与
//! [`crate::session::switch_session`] 一致（FR-102）。
//!
//! 边界：内存状态（按需创建，不随会话数线性叠加固定开销，FR-614）；卸载与唤醒
//! 的行为可查询（`is_unloaded` / `unloaded_sessions`）。

use std::collections::BTreeMap;

use serde::Serialize;

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

/// 驻留管理器：按会话维护驻留状态（按需创建，无固定大缓冲，FR-614）。
#[derive(Debug, Default)]
pub struct ResidencyManager {
    states: BTreeMap<String, ResidencyState>,
    unloaded: BTreeMap<String, UnloadedSession>,
}

impl ResidencyManager {
    /// 空管理器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 标记会话常驻（按需唤醒/新建；不预分配大缓冲）。
    pub fn wake(&mut self, session_id: &str) {
        self.unloaded.remove(session_id);
        self.states
            .insert(session_id.to_string(), ResidencyState::Resident);
    }

    /// 卸载空闲会话（纯函数判定 + 显式游标记录；不丢已确认事件——游标之后的
    /// 事件由补拉还原，FR-605）。
    pub fn unload(&mut self, session_id: &str, event_cursor: i64, now_ms: i64) -> UnloadedSession {
        let record = UnloadedSession {
            session_id: session_id.to_string(),
            event_cursor,
            unloaded_at_ms: now_ms,
        };
        self.states
            .insert(session_id.to_string(), ResidencyState::Unloaded);
        self.unloaded.insert(session_id.to_string(), record.clone());
        record
    }

    /// 驻留状态（可查询，FR-614）。
    #[must_use]
    pub fn state(&self, session_id: &str) -> ResidencyState {
        self.states
            .get(session_id)
            .copied()
            .unwrap_or(ResidencyState::Resident)
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
}

/// 空闲卸载判定（纯函数）：距上次活动超过 `idle_window_ms` 才卸载。
#[must_use]
pub fn idle_unload_due(now_ms: i64, last_activity_ms: i64, idle_window_ms: i64) -> bool {
    now_ms.saturating_sub(last_activity_ms) >= idle_window_ms
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
