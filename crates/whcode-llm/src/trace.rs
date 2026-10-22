//! # 追踪级别与脱敏（T058/T064、FR-203/208、D-05、§10.1、contracts/event-schema.md §4）
//!
//! 四级追踪（[`whcode_domain::TraceLevel`]，默认 `redacted`）：
//!
//! | 级别 | [`sanitize`] 行为 |
//! |---|---|
//! | `off` | 不记录内容（载荷为 `None`） |
//! | `metadata` | 仅元数据：正文类字段全部剥离，结构键与状态保留 |
//! | `redacted`（默认） | 保留内容，但疑似秘密字段与秘密值模式一律替换为 `[REDACTED]`，并标记 `integrity.redacted` |
//! | `full` | 原样保留（须在界面警告潜在敏感信息写入，并提供访问控制与清理/导出策略——UI 警告属前端职责） |
//!
//! ## 完整性标记（FR-208，T064）
//!
//! [`IntegrityFlags`] 四个布尔：`missing_chunks`（缺失 chunk）、`redacted`（脱敏）、
//! `downsampled`（降采样/合并）、`persistence_failed`（持久化失败）——随载荷进入
//! 事件/导出，可被查询识别（V-110、Check 58）。
//!
//! 脱敏规则（双层）：
//! 1. **字段名**：[`SUSPICIOUS_FIELDS`] 命中的字段值整体替换；
//! 2. **值模式**：任意字段值命中 [`SECRET_VALUE_PREFIXES`] 前缀（`sk-`/`Bearer `/
//!    `ghp_` 等）同样替换——防止秘密被写进普通字段。
//!
//! 边界：本模块不持久化、不触库；`full` 级别的访问控制与导出策略属调用方。

use serde::Serialize;
use whcode_domain::TraceLevel;

/// 疑似秘密字段名（脱敏命中即整体替换值；不区分大小写比较）。
pub const SUSPICIOUS_FIELDS: [&str; 10] = [
    "api_key",
    "apikey",
    "authorization",
    "token",
    "secret",
    "password",
    "credential",
    "credentials",
    "private_key",
    "set_cookie",
];

/// 秘密值前缀：任意字段值命中即脱敏（防止秘密写进普通字段）。
pub const SECRET_VALUE_PREFIXES: [&str; 4] = ["sk-", "Bearer ", "ghp_", "xoxb-"];

/// 完整性标记（FR-208）：四类缺失/加工痕迹可被查询识别。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct IntegrityFlags {
    /// 是否存在缺失 chunk。
    pub missing_chunks: bool,
    /// 是否发生脱敏。
    pub redacted: bool,
    /// 是否发生降采样/合并。
    pub downsampled: bool,
    /// 是否发生持久化失败。
    pub persistence_failed: bool,
}

impl IntegrityFlags {
    /// 完整性标记的 JSON 形态（入事件载荷与导出，可查询）。
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "missing_chunks": self.missing_chunks,
            "redacted": self.redacted,
            "downsampled": self.downsampled,
            "persistence_failed": self.persistence_failed,
        })
    }
}

/// 脱敏后的追踪内容。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SanitizedTrace {
    /// 追踪载荷；`off` 级别为 `None`（不记录）。
    pub payload: Option<serde_json::Value>,
    /// 完整性标记（FR-208）。
    pub integrity: IntegrityFlags,
    /// 本次使用的追踪级别。
    pub level: TraceLevel,
}

fn field_is_suspicious(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    SUSPICIOUS_FIELDS
        .iter()
        .any(|field| normalized == *field || normalized.ends_with(field))
}

fn value_is_secret(value: &str) -> bool {
    SECRET_VALUE_PREFIXES
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

fn sanitize_value(value: &serde_json::Value, level: TraceLevel) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sanitized = serde_json::Map::new();
            for (name, inner) in map {
                if field_is_suspicious(name) {
                    sanitized.insert(name.clone(), serde_json::json!("[REDACTED]"));
                } else if let serde_json::Value::String(text) = inner {
                    if value_is_secret(text) {
                        sanitized.insert(name.clone(), serde_json::json!("[REDACTED]"));
                    } else {
                        sanitized.insert(name.clone(), sanitize_value(inner, level));
                    }
                } else {
                    sanitized.insert(name.clone(), sanitize_value(inner, level));
                }
            }
            serde_json::Value::Object(sanitized)
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| sanitize_value(item, level))
                .collect(),
        ),
        serde_json::Value::String(text) => match level {
            TraceLevel::Metadata => serde_json::json!("[stripped:metadata]"),
            TraceLevel::Off => serde_json::json!(null),
            _ => serde_json::Value::String(text.clone()),
        },
        other => match level {
            TraceLevel::Off => serde_json::Value::Null,
            _ => other.clone(),
        },
    }
}

/// 按追踪级别脱敏载荷（FR-203、D-05）。
///
/// # Errors
///
/// 本函数不返回错误（脱敏是纯变换）；签名保持与调用方一致。
#[must_use]
pub fn sanitize(payload: &serde_json::Value, level: TraceLevel) -> SanitizedTrace {
    match level {
        TraceLevel::Off => SanitizedTrace {
            payload: None,
            integrity: IntegrityFlags::default(),
            level,
        },
        TraceLevel::Metadata | TraceLevel::Redacted | TraceLevel::Full => {
            let sanitized = sanitize_value(payload, level);
            let integrity = IntegrityFlags {
                redacted: level == TraceLevel::Redacted && sanitized != *payload,
                missing_chunks: false,
                downsampled: false,
                persistence_failed: false,
            };
            SanitizedTrace {
                payload: Some(sanitized),
                integrity,
                level,
            }
        }
    }
}

/// 为完整性标记构造追踪载荷（FR-208）：把标志并入载荷的 `integrity` 字段。
#[must_use]
pub fn with_integrity(
    payload: &serde_json::Value,
    flags: IntegrityFlags,
    level: TraceLevel,
) -> Option<serde_json::Value> {
    let sanitized = sanitize(payload, level).payload?;
    match sanitized {
        serde_json::Value::Object(mut map) => {
            map.insert("integrity".to_string(), flags.to_json());
            Some(serde_json::Value::Object(map))
        }
        other => Some(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacted_masks_suspicious_fields_and_marks_integrity() {
        let payload = json!({
            "api_key": "sk-abc123",
            "prompt": "写一个排序算法",
            "note": "Bearer eyJhbGciOi",
            "tokens": 42,
        });
        let trace = sanitize(&payload, TraceLevel::Redacted);
        let output = trace.payload.expect("redacted 保留内容");
        assert_eq!(output["api_key"], "[REDACTED]", "字段名命中必须脱敏");
        assert_eq!(output["note"], "[REDACTED]", "值模式命中必须脱敏");
        assert_eq!(output["prompt"], "写一个排序算法", "普通内容保留");
        assert_eq!(output["tokens"], 42);
        assert!(trace.integrity.redacted, "脱敏必须标记（FR-208）");

        // 导出与测试快照中不得出现原始秘密（V-110）。
        let serialized = serde_json::to_string(&output).expect("序列化");
        assert!(!serialized.contains("sk-abc123"));
        assert!(!serialized.contains("eyJhbGciOi"));
    }

    #[test]
    fn off_records_nothing_and_metadata_strips_content() {
        let payload = json!({ "prompt": "正文", "latency_ms": 7_321 });
        let trace = sanitize(&payload, TraceLevel::Off);
        assert!(trace.payload.is_none(), "off 不记录内容");
        assert!(!trace.integrity.redacted);

        let trace = sanitize(&payload, TraceLevel::Metadata);
        let output = trace.payload.expect("metadata 保留结构");
        assert_eq!(output["latency_ms"], 7_321, "元数据保留");
        assert_ne!(output["prompt"], "正文", "正文必须剥离");
        assert!(!trace.integrity.redacted, "metadata 剥离不是脱敏");
    }

    #[test]
    fn full_keeps_content_as_is() {
        let payload = json!({ "prompt": "正文", "tokens": 1 });
        let trace = sanitize(&payload, TraceLevel::Full);
        assert_eq!(trace.payload, Some(payload), "full 原样保留");
        assert!(!trace.integrity.redacted);
    }

    #[test]
    fn integrity_flags_are_queryable_via_json() {
        let flags = IntegrityFlags {
            missing_chunks: true,
            redacted: true,
            downsampled: false,
            persistence_failed: true,
        };
        let payload =
            with_integrity(&json!({ "chunks": 10 }), flags, TraceLevel::Redacted).expect("载荷");
        assert_eq!(payload["integrity"]["missing_chunks"], true);
        assert_eq!(payload["integrity"]["redacted"], true);
        assert_eq!(payload["integrity"]["persistence_failed"], true);
        assert_eq!(payload["integrity"]["downsampled"], false);
    }
}
