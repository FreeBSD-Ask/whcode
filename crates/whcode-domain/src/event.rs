//! 事件类型、持久化等级与载荷上限（任务书 §4.2、§7.1、§8.3）。
//!
//! 事件是跨端唯一的状态变更通道。可靠性分级（关键／内容／展示快照）决定队列饱和时的
//! 语义：关键事件不得静默丢弃，展示快照可合并或丢弃。**不得**把"实时广播 + 数据库写入"
//! 描述为自动原子双写（ADR-006）。

use serde::{Deserialize, Serialize};

/// 单条事件载荷上限（字节，1 MiB）。
///
/// 超出上限的内容必须走内容表/引用（R-05 对策）；**不得**静默截断原始数据，截断必须留下
/// 可查询标记（FR-208）。
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

/// 统一事件类型（FR-201）。
///
/// 序列化名与任务书 §8.3 的事件示例一致（PascalCase，例如 `"kind":"RequestStarted"`），
/// 同时也是数据库 `kind` 列与 CUI JSONL 导出的取值——三处不得各用一套命名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    /// 请求开始（含 queued → started 状态转换）。
    RequestStarted,
    /// 流式增量。
    StreamingChunk,
    /// 请求正常完成（含 token 计量）。
    RequestCompleted,
    /// 请求失败（供应商错误已归一化）。
    RequestFailed,
    /// 上下文状态（预算、装入来源、截断原因）。
    ContextState,
    /// 进度变化（可重建的展示信息）。
    ProgressChanged,
    /// 工具调用开始（执行意图已提交）。
    ToolCallStarted,
    /// 工具调用结束（结果或副作用凭证已登记）。
    ToolCallFinished,
    /// 运行时终止（超时、取消、信号）。
    RuntimeTerminated,
    /// 持久化异常。
    PersistenceError,
}

impl EventKind {
    /// 稳定序列化名；与任务书 §8.3、CUI JSONL 导出、Web 协议共用。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RequestStarted => "RequestStarted",
            Self::StreamingChunk => "StreamingChunk",
            Self::RequestCompleted => "RequestCompleted",
            Self::RequestFailed => "RequestFailed",
            Self::ContextState => "ContextState",
            Self::ProgressChanged => "ProgressChanged",
            Self::ToolCallStarted => "ToolCallStarted",
            Self::ToolCallFinished => "ToolCallFinished",
            Self::RuntimeTerminated => "RuntimeTerminated",
            Self::PersistenceError => "PersistenceError",
        }
    }

    /// 可靠性分级（§4.2）。分级决定队列饱和时是背压还是丢弃。
    pub const fn criticality(self) -> EventCriticality {
        match self {
            Self::RequestStarted
            | Self::RequestCompleted
            | Self::RequestFailed
            | Self::ToolCallStarted
            | Self::ToolCallFinished
            | Self::RuntimeTerminated
            | Self::PersistenceError => EventCriticality::Critical,
            Self::StreamingChunk | Self::ContextState => EventCriticality::Content,
            Self::ProgressChanged => EventCriticality::DisplaySnapshot,
        }
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 事件可靠性三级（§4.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCriticality {
    /// 任务状态转换、请求最终结果、工具执行意图与结果、门禁结果、人工放行、审计事件。
    /// 队列饱和时**必须**背压、失败返回或触发安全停止。
    Critical,
    /// 请求/响应正文与流式增量：是否持久化由追踪级别控制；省略或采样必须留标记。
    Content,
    /// 帧率、重复进度通知等可重建信息：允许合并或丢弃。
    DisplaySnapshot,
}

/// 事件持久化等级（§7.1 `persistence_level`、FR-203）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistenceLevel {
    /// 完整内容已持久化。
    Full,
    /// 仅元数据（时长、token 数、状态），无正文。
    Metadata,
    /// 内容已持久化但疑似秘密字段已脱敏（默认级别，D-05）。
    Redacted,
    /// 按策略省略内容（例如 `streaming_chunk` 在 `metadata` 级别下）。
    OmittedByPolicy,
}

impl PersistenceLevel {
    /// 稳定序列化名；与数据库列取值、CUI JSONL 导出共用。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Metadata => "metadata",
            Self::Redacted => "redacted",
            Self::OmittedByPolicy => "omitted_by_policy",
        }
    }
}

impl std::fmt::Display for PersistenceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 事件记录（`events` 表逻辑字段；data-model.md §1）。
///
/// 对外 JSON 形态遵循任务书 §8.3：`schema_version` 必备，事件标识为 `event_id`，
/// 时间戳为 ISO-8601 UTC 字符串 `ts`。数据库侧仍以整数毫秒（`ts_ms`）存取，比较与排序
/// 不受字符串格式影响。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    /// 事件 schema 版本（任务书 §8.3 要求必备）。
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// 工作区内唯一且单调递增的事件序号。
    pub seq: u64,
    /// 事件 ID（例如 `E-100`）。
    #[serde(rename = "event_id")]
    pub id: String,
    /// 事件类型。
    pub kind: EventKind,
    /// 所属会话。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 所属任务。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// 所属 LLM 请求。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// 所属工具调用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 载荷。大正文必须改为引用（R-05）。
    pub payload: serde_json::Value,
    /// Unix epoch 毫秒（UTC）。对外序列化为 ISO-8601 UTC 字符串 `ts`（任务书 §8.3）。
    #[serde(
        rename = "ts",
        serialize_with = "serialize_ts_iso",
        deserialize_with = "deserialize_ts_iso"
    )]
    pub ts_ms: u64,
    /// 持久化等级。
    pub persistence_level: PersistenceLevel,
}

/// `EventRecord::schema_version` 的默认值（缺省即当前版本）。
const fn default_schema_version() -> u32 {
    crate::error::SCHEMA_VERSION
}

/// 把 epoch 毫秒格式化为 ISO-8601 UTC（`YYYY-MM-DDTHH:MM:SSZ`）。
///
/// 不引入时间库：日历换算采用 Howard Hinnant 的 civil-from-days 算法，只处理 UTC，
/// 秒以下精度按毫秒截断（事件排序依据是 `seq`，不是时间戳）。
#[must_use]
pub fn epoch_ms_to_iso8601(ms: u64) -> String {
    let secs = ms / 1_000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// 解析 [`epoch_ms_to_iso8601`] 生成的字符串（也接受带毫秒或 `+00:00` 后缀的形态）。
///
/// 解析失败返回 `None`——调用方必须显式处理，禁止把失败当成 0（那会把事件时间静默改成
/// 1970 年）。
#[must_use]
pub fn iso8601_to_epoch_ms(text: &str) -> Option<u64> {
    let text = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))?;
    let (date, time) = text.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut time_parts = time.split(':');
    let hour: u64 = time_parts.next()?.parse().ok()?;
    let minute: u64 = time_parts.next()?.parse().ok()?;
    let second_part = time_parts.next()?;
    if time_parts.next().is_some() {
        return None;
    }
    let (second, millis) = match second_part.split_once('.') {
        Some((sec, frac)) => {
            let digits: String = frac.chars().take(3).collect();
            let padded = format!("{digits:0<3}");
            (sec.parse::<u64>().ok()?, padded.parse::<u64>().ok()?)
        }
        None => (second_part.parse::<u64>().ok()?, 0),
    };
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    let secs = days as u64 * 86_400 + hour * 3_600 + minute * 60 + second;
    Some(secs * 1_000 + millis)
}

/// civil-from-days（Howard Hinnant）；`days` 为 1970-01-01 起的天数。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// days-from-civil（Howard Hinnant）。
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = month as u64;
    let d = day as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn serialize_ts_iso<S: serde::Serializer>(ms: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&epoch_ms_to_iso8601(*ms))
}

fn deserialize_ts_iso<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let text = String::deserialize(deserializer)?;
    iso8601_to_epoch_ms(&text)
        .ok_or_else(|| serde::de::Error::custom(format!("无法解析事件时间戳：{text}")))
}

impl EventRecord {
    /// 载荷的紧凑 JSON 字节长度。
    pub fn payload_bytes(&self) -> usize {
        serde_json::to_vec(&self.payload)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// 载荷是否在上限内。无界正文会被 R-05 风险登记与 Check 198 追踪。
    pub fn payload_within_limit(&self) -> bool {
        self.payload_bytes() <= MAX_PAYLOAD_BYTES
    }

    /// 可靠性分级快捷访问。
    pub const fn criticality(&self) -> EventCriticality {
        self.kind.criticality()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_kind_set_covers_fr201() {
        let all = [
            EventKind::RequestStarted,
            EventKind::StreamingChunk,
            EventKind::RequestCompleted,
            EventKind::RequestFailed,
            EventKind::ContextState,
            EventKind::ProgressChanged,
            EventKind::ToolCallStarted,
            EventKind::ToolCallFinished,
            EventKind::RuntimeTerminated,
            EventKind::PersistenceError,
        ];
        assert_eq!(all.len(), 10);
        assert_eq!(all[0].as_str(), "RequestStarted");
        assert_eq!(all[9].as_str(), "PersistenceError");
    }

    #[test]
    fn event_kind_serializes_with_task_book_pascal_case() {
        // 任务书 §8.3 示例：{"kind":"RequestStarted",...}；数据库与导出共用同一取值。
        let json = serde_json::to_string(&EventKind::StreamingChunk).expect("serialize");
        assert_eq!(json, "\"StreamingChunk\"");
    }

    #[test]
    fn critical_events_are_the_state_and_audit_class() {
        assert_eq!(
            EventKind::ToolCallStarted.criticality(),
            EventCriticality::Critical
        );
        assert_eq!(
            EventKind::RequestFailed.criticality(),
            EventCriticality::Critical
        );
        assert_eq!(
            EventKind::StreamingChunk.criticality(),
            EventCriticality::Content
        );
        assert_eq!(
            EventKind::ProgressChanged.criticality(),
            EventCriticality::DisplaySnapshot
        );
    }

    #[test]
    fn oversized_payload_is_detected() {
        let record = EventRecord {
            schema_version: default_schema_version(),
            seq: 1,
            id: "E-1".to_string(),
            kind: EventKind::StreamingChunk,
            session_id: Some("S-001".to_string()),
            task_id: None,
            request_id: Some("R-889".to_string()),
            tool_call_id: None,
            payload: json!("x".repeat(MAX_PAYLOAD_BYTES + 1)),
            ts_ms: 0,
            persistence_level: PersistenceLevel::Redacted,
        };
        assert!(!record.payload_within_limit());
    }

    #[test]
    fn small_payload_passes_and_optional_ids_are_omitted() {
        let record = EventRecord {
            schema_version: default_schema_version(),
            seq: 7,
            id: "E-7".to_string(),
            kind: EventKind::RequestStarted,
            session_id: None,
            task_id: None,
            request_id: None,
            tool_call_id: None,
            payload: json!({"model": "stub"}),
            ts_ms: 1_700_000_000_000,
            persistence_level: PersistenceLevel::Metadata,
        };
        assert!(record.payload_within_limit());
        assert_eq!(record.criticality(), EventCriticality::Critical);
        let serialized = serde_json::to_string(&record).expect("serialize");
        assert!(!serialized.contains("session_id"));
        assert!(serialized.contains("\"persistence_level\":\"metadata\""));
    }

    #[test]
    fn event_json_has_contract_fields() {
        // 任务书 §8.3 + contracts/event-schema.md §2：schema_version、event_id、ts 必备。
        let record = EventRecord {
            schema_version: default_schema_version(),
            seq: 12,
            id: "E-100".to_string(),
            kind: EventKind::RequestStarted,
            session_id: Some("S-001".to_string()),
            task_id: None,
            request_id: Some("R-889".to_string()),
            tool_call_id: None,
            payload: json!({}),
            ts_ms: 1_760_932_800_000,
            persistence_level: PersistenceLevel::Redacted,
        };
        let value = serde_json::to_value(&record).expect("serialize");
        assert_eq!(value["schema_version"], json!(1));
        assert_eq!(value["event_id"], json!("E-100"));
        assert_eq!(value["kind"], json!("RequestStarted"));
        assert_eq!(value["ts"], json!("2025-10-20T04:00:00Z"));
        assert!(value.get("id").is_none(), "不得同时输出 id 与 event_id");
        assert!(value.get("ts_ms").is_none(), "对外时间戳字段名为 ts");
    }

    #[test]
    fn iso8601_round_trips_epoch_millis() {
        for ms in [0_u64, 1_000, 1_700_000_000_000, 1_760_932_800_000] {
            let text = epoch_ms_to_iso8601(ms);
            assert_eq!(iso8601_to_epoch_ms(&text), Some(ms), "往返失败：{text}");
        }
    }

    #[test]
    fn invalid_timestamp_is_rejected_not_coerced_to_zero() {
        assert!(
            iso8601_to_epoch_ms("2025-10-20").is_none(),
            "缺时间部分必须拒绝"
        );
        assert!(
            iso8601_to_epoch_ms("2025-13-01T00:00:00Z").is_none(),
            "非法月份必须拒绝"
        );
        assert!(
            iso8601_to_epoch_ms("not-a-timestamp").is_none(),
            "非时间文本必须拒绝"
        );
    }
}
