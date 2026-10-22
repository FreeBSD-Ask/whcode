//! # 链路关联 ID（T059、FR-204）
//!
//! [`Correlation`] 串联 `request_id / session_id / task_id / tool_call_id`：
//! 每条事件携带这四个 ID（相关事件必须带 `request_id`），查询侧按 `request_id`
//! 分组即可串联整条链路。
//!
//! 断链可检出（FR-204、Check 59）：[`chain_breaks`] 返回缺少 `request_id` 的事件
//! 下标——任务/工具相关事件缺失请求 ID 即为断链，不静默跳过。
//!
//! 边界：本模块只做关联结构的构造、校验与分组，不做持久化与查询（查询属
//! `whcode-store::query`）。

use std::collections::BTreeMap;

use whcode_domain::{EventRecord, WhError};

/// 一次调用的关联 ID 集合（构造事件时统一携带，FR-204）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Correlation {
    /// 请求 ID（链路根；必填）。
    pub request_id: String,
    /// 所属会话。
    pub session_id: Option<String>,
    /// 所属任务。
    pub task_id: Option<String>,
    /// 所属工具调用。
    pub tool_call_id: Option<String>,
}

impl Correlation {
    /// 以必填的请求 ID 构造。
    #[must_use]
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            session_id: None,
            task_id: None,
            tool_call_id: None,
        }
    }

    /// 附加会话 ID。
    #[must_use]
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// 附加任务 ID。
    #[must_use]
    pub fn with_task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    /// 附加工具调用 ID。
    #[must_use]
    pub fn with_tool_call(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
        self
    }

    /// 校验关联完整：请求 ID 非空。
    ///
    /// # Errors
    ///
    /// 请求 ID 为空时返回 `invalid_state`（空 ID 无法串联链路，V-106 同源语义）。
    pub fn validate(&self) -> Result<(), WhError> {
        if self.request_id.trim().is_empty() {
            return Err(WhError::invalid_state(
                "request_id 不得为空：空请求 ID 无法串联链路（FR-204）",
            ));
        }
        Ok(())
    }

    /// 把关联 ID 写入事件记录的对应字段。
    pub fn apply(&self, record: &mut EventRecord) {
        record.request_id = Some(self.request_id.clone());
        record.session_id = self.session_id.clone();
        record.task_id = self.task_id.clone();
        record.tool_call_id = self.tool_call_id.clone();
    }
}

/// 按 `request_id` 分组串联事件（FR-204）。
#[must_use]
pub fn link(records: &[EventRecord]) -> BTreeMap<String, Vec<&EventRecord>> {
    let mut linked: BTreeMap<String, Vec<&EventRecord>> = BTreeMap::new();
    for record in records {
        if let Some(request_id) = &record.request_id {
            linked.entry(request_id.clone()).or_default().push(record);
        }
    }
    linked
}

/// 检出断链：缺少 `request_id` 的事件下标（FR-204：断链可检出，不静默）。
#[must_use]
pub fn chain_breaks(records: &[EventRecord]) -> Vec<usize> {
    records
        .iter()
        .enumerate()
        .filter(|(_, record)| record.request_id.as_deref().unwrap_or("").trim().is_empty())
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use whcode_domain::{ErrorCode, EventKind, PersistenceLevel, SCHEMA_VERSION};

    fn record(request_id: Option<&str>, kind: EventKind) -> EventRecord {
        EventRecord {
            schema_version: SCHEMA_VERSION,
            seq: 0,
            id: format!("E-{kind:?}"),
            kind,
            session_id: Some("S-1".to_string()),
            task_id: Some("T-1".to_string()),
            request_id: request_id.map(str::to_string),
            tool_call_id: None,
            payload: json!({}),
            ts_ms: 1_000,
            persistence_level: PersistenceLevel::Redacted,
        }
    }

    #[test]
    fn correlation_applies_all_four_ids_and_validates() {
        let correlation = Correlation::new("R-889")
            .with_session("S-001")
            .with_task("T-017")
            .with_tool_call("TC-1");
        correlation.validate().expect("关联完整");

        let mut record = record(None, EventKind::RequestStarted);
        correlation.apply(&mut record);
        assert_eq!(record.request_id.as_deref(), Some("R-889"));
        assert_eq!(record.session_id.as_deref(), Some("S-001"));
        assert_eq!(record.task_id.as_deref(), Some("T-017"));
        assert_eq!(record.tool_call_id.as_deref(), Some("TC-1"));

        let broken = Correlation::new("  ");
        let err = broken.validate().expect_err("空请求 ID 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn link_groups_by_request_id_and_chain_breaks_are_detectable() {
        let records = vec![
            record(Some("R-1"), EventKind::RequestStarted),
            record(Some("R-1"), EventKind::StreamingChunk),
            record(Some("R-2"), EventKind::RequestCompleted),
            record(None, EventKind::StreamingChunk),
        ];

        let linked = link(&records);
        assert_eq!(linked.len(), 2, "两个请求分组");
        assert_eq!(linked["R-1"].len(), 2);
        assert_eq!(linked["R-1"][0].kind, EventKind::RequestStarted);
        assert_eq!(linked["R-1"][1].kind, EventKind::StreamingChunk);
        assert_eq!(linked["R-2"].len(), 1);

        let breaks = chain_breaks(&records);
        assert_eq!(
            breaks,
            vec![3],
            "缺少 request_id 的事件必须被检出（FR-204）"
        );

        let complete = vec![record(Some("R-1"), EventKind::RequestStarted)];
        assert!(chain_breaks(&complete).is_empty(), "完整链路无断链");
    }
}
