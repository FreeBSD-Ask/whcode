//! # 传输层进度事件（T132、FR-616）
//!
//! 连接建立/断开、重连尝试、按连接字节收发计数、往返时延与断连原因码，全部进入
//! 统一事件体系（`ProgressChanged` 载荷 `transport` 字段），经 CUI JSON 可导出。
//!
//! 指标按**连接**聚合（不是按会话线性叠加，NFR-614/资源纪律）；RTT 采样有界
//! （[`MAX_RTT_SAMPLES`]），计数为 `u64` 饱和累加。
//!
//! 边界：本模块只维护计数与产出事件记录；真实网络连接属后续里程碑。

use std::collections::VecDeque;

use whcode_domain::{EventKind, EventRecord, PersistenceLevel};

/// RTT 采样上限（有界）。
pub const MAX_RTT_SAMPLES: usize = 64;

/// 传输状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    /// 已连接。
    Connected,
    /// 已断开。
    Disconnected,
    /// 重连尝试中。
    Reconnecting,
}

impl TransportState {
    /// 稳定标识（入事件载荷）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Reconnecting => "reconnecting",
        }
    }
}

/// 传输层只读快照（FR-616：供 `Provider::transport_snapshot` 返回）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TransportSnapshot {
    /// 连接 ID。
    pub connection_id: String,
    /// 状态。
    pub state: String,
    /// 累计发送字节。
    pub bytes_sent: u64,
    /// 累计接收字节。
    pub bytes_received: u64,
    /// RTT 采样（毫秒）。
    pub rtt_samples: Vec<u64>,
    /// 重连尝试次数。
    pub reconnect_attempts: u32,
}

/// 按连接聚合的传输追踪器。
#[derive(Debug)]
pub struct TransportTracker {
    /// 连接 ID。
    pub connection_id: String,
    state: TransportState,
    bytes_sent: u64,
    bytes_received: u64,
    rtt_samples: VecDeque<u64>,
    reconnect_attempts: u32,
    disconnect_reason: Option<String>,
}

impl TransportTracker {
    /// 以连接 ID 构造（初始 `disconnected`）。
    #[must_use]
    pub fn new(connection_id: impl Into<String>) -> Self {
        Self {
            connection_id: connection_id.into(),
            state: TransportState::Disconnected,
            bytes_sent: 0,
            bytes_received: 0,
            rtt_samples: VecDeque::new(),
            reconnect_attempts: 0,
            disconnect_reason: None,
        }
    }

    /// 记录发送字节数（按连接计数，饱和累加）。
    pub fn record_sent(&mut self, bytes: u64) {
        self.bytes_sent = self.bytes_sent.saturating_add(bytes);
    }

    /// 记录接收字节数。
    pub fn record_received(&mut self, bytes: u64) {
        self.bytes_received = self.bytes_received.saturating_add(bytes);
    }

    /// 记录一次往返时延采样（有界 [`MAX_RTT_SAMPLES`]）。
    pub fn record_rtt(&mut self, rtt_ms: u64) {
        if self.rtt_samples.len() >= MAX_RTT_SAMPLES {
            self.rtt_samples.pop_front();
        }
        self.rtt_samples.push_back(rtt_ms);
    }

    /// 当前状态。
    #[must_use]
    pub const fn state(&self) -> TransportState {
        self.state
    }

    /// 重连尝试次数。
    #[must_use]
    pub const fn reconnect_attempts(&self) -> u32 {
        self.reconnect_attempts
    }

    /// RTT 采样（毫秒，最多 [`MAX_RTT_SAMPLES`] 个）。
    #[must_use]
    pub fn rtt_samples(&self) -> Vec<u64> {
        self.rtt_samples.iter().copied().collect()
    }

    /// 只读快照（FR-206／FR-208：供 Provider 暴露传输摘要）。
    #[must_use]
    pub fn snapshot(&self) -> TransportSnapshot {
        TransportSnapshot {
            connection_id: self.connection_id.clone(),
            state: self.state.as_str().to_string(),
            bytes_sent: self.bytes_sent,
            bytes_received: self.bytes_received,
            rtt_samples: self.rtt_samples(),
            reconnect_attempts: self.reconnect_attempts,
        }
    }

    fn payload(&self) -> serde_json::Value {
        serde_json::json!({
            "transport": {
                "connection_id": self.connection_id,
                "state": self.state.as_str(),
                "bytes_sent": self.bytes_sent,
                "bytes_received": self.bytes_received,
                "rtt_ms_samples": self.rtt_samples(),
                "reconnect_attempts": self.reconnect_attempts,
                "disconnect_reason": self.disconnect_reason,
            }
        })
    }

    fn event(&self, now_ms: u64) -> EventRecord {
        EventRecord {
            schema_version: whcode_domain::SCHEMA_VERSION,
            seq: 0,
            id: format!("E-transport-{}-{now_ms}", self.connection_id),
            kind: EventKind::ProgressChanged,
            session_id: None,
            task_id: None,
            request_id: None,
            tool_call_id: None,
            payload: self.payload(),
            ts_ms: now_ms,
            persistence_level: PersistenceLevel::Metadata,
        }
    }

    /// 记录连接建立，返回事件记录（入事件体系，FR-616）。
    pub fn record_connected(&mut self, now_ms: u64) -> EventRecord {
        self.state = TransportState::Connected;
        self.disconnect_reason = None;
        self.event(now_ms)
    }

    /// 记录断开（含原因码），返回事件记录。
    pub fn record_disconnect(&mut self, reason: &str, now_ms: u64) -> EventRecord {
        self.state = TransportState::Disconnected;
        self.disconnect_reason = Some(reason.to_string());
        self.event(now_ms)
    }

    /// 记录一次重连尝试，返回事件记录。
    pub fn record_reconnect_attempt(&mut self, now_ms: u64) -> EventRecord {
        self.state = TransportState::Reconnecting;
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        self.event(now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_events_carry_bytes_rtt_attempts_and_reason() {
        let mut tracker = TransportTracker::new("conn-1");
        tracker.record_sent(120);
        tracker.record_received(2_048);
        tracker.record_rtt(30);
        tracker.record_rtt(45);

        let connected = tracker.record_connected(1_000);
        assert_eq!(connected.kind, EventKind::ProgressChanged);
        assert_eq!(connected.payload["transport"]["state"], "connected");
        assert_eq!(connected.payload["transport"]["bytes_sent"], 120);
        assert_eq!(connected.payload["transport"]["bytes_received"], 2_048);

        tracker.record_disconnect("timeout", 2_000);
        let attempt = tracker.record_reconnect_attempt(3_000);
        assert_eq!(tracker.state(), TransportState::Reconnecting);
        assert_eq!(tracker.reconnect_attempts(), 1);
        assert_eq!(attempt.payload["transport"]["reconnect_attempts"], 1);

        let disconnected = tracker.record_disconnect("timeout", 4_000);
        assert_eq!(
            disconnected.payload["transport"]["disconnect_reason"], "timeout",
            "断连原因码入事件（FR-616）"
        );
        assert_eq!(disconnected.payload["transport"]["state"], "disconnected");
    }

    #[test]
    fn rtt_samples_are_bounded() {
        let mut tracker = TransportTracker::new("conn-2");
        for ms in 0..(MAX_RTT_SAMPLES as u64 + 10) {
            tracker.record_rtt(ms);
        }
        assert_eq!(tracker.rtt_samples().len(), MAX_RTT_SAMPLES, "采样必须有界");
        assert_eq!(tracker.rtt_samples()[0], 10, "超限后丢弃最旧采样");
    }
}
