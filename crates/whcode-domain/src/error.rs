//! 结构化错误与 JSON 输出信封（任务书 §8.1、contracts/cui-cli.md §3）。
//!
//! 契约：所有 JSON 输出必须携带 `schema_version`；错误对象必须含 `code`、`message`、
//! `retryable`、`details` 四字段。四端（TUI/CUI/Web/SSH）共用本类型，不得各自定义。

use serde::{Deserialize, Serialize};

/// 对外 JSON 信封与错误对象的 schema 版本。
pub const SCHEMA_VERSION: u32 = 1;

/// 稳定错误码（contracts/app-service.md §4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// 权限不足。
    PermissionDenied,
    /// SDD 阶段门禁未通过。
    GateFailed,
    /// 运行时能力不足、缺失或不兼容。
    RuntimeUnavailable,
    /// 有界队列饱和（关键命令），调用方可重试。
    Backpressure,
    /// 当前状态不允许该操作。
    InvalidState,
    /// 已登记能力在该端不提供（预留或已登记排除；FR-620、ADR-032）。
    ///
    /// 语义介于 `invalid_state`（状态异常）与 `not_found`（不存在）之间：能力已登记、
    /// 路由/命令存在，但该前端按登记表不提供。未知路由仍用 `not_found`，不得混用。
    NotSupported,
    /// 目标不存在。
    NotFound,
    /// I/O 失败。
    IoError,
    /// 产物或输入解析失败。
    ParseError,
    /// 超时。
    Timeout,
    /// 未分类内部错误。
    Internal,
}

impl ErrorCode {
    /// 契约中的字符串形式。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PermissionDenied => "permission_denied",
            Self::GateFailed => "gate_failed",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::Backpressure => "backpressure",
            Self::InvalidState => "invalid_state",
            Self::NotSupported => "not_supported",
            Self::NotFound => "not_found",
            Self::IoError => "io_error",
            Self::ParseError => "parse_error",
            Self::Timeout => "timeout",
            Self::Internal => "internal",
        }
    }

    /// 该错误码的默认可重试语义。
    ///
    /// 只对"重试不会造成副作用重复"的类别置 `true`：背压、超时与 I/O 失败可重试；
    /// 权限、门禁、状态与运行时能力问题必须由调用方改变输入或环境（FR-604、§10.2）。
    pub const fn default_retryable(self) -> bool {
        matches!(self, Self::Backpressure | Self::Timeout | Self::IoError)
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 结构化错误。四字段与 `retryable` 语义由契约固定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct WhError {
    /// 稳定错误码。
    pub code: ErrorCode,
    /// 面向使用者的说明（不得包含秘密或未脱敏载荷）。
    pub message: String,
    /// 调用方是否可安全重试。
    pub retryable: bool,
    /// 附加细节（未通过的门禁清单、澄清项 ID 等）。
    ///
    /// 契约（contracts/cui-cli.md §3）要求错误对象**恒有四个字段**，因此本字段在无细节时
    /// 仍序列化为 `null`，不得依赖 `skip_serializing_if` 省略整键。
    pub details: Option<serde_json::Value>,
}

impl WhError {
    /// 使用错误码的默认可重试语义构造。
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: code.default_retryable(),
            details: None,
        }
    }

    /// 附加细节。
    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// 权限不足（不可重试）。
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PermissionDenied, message)
    }

    /// 门禁未通过。
    pub fn gate_failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::GateFailed, message)
    }

    /// 运行时不可用。
    pub fn runtime_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::RuntimeUnavailable, message)
    }

    /// 队列饱和（可重试）。
    pub fn backpressure(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Backpressure, message)
    }

    /// 状态不允许（不可重试）。
    pub fn invalid_state(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidState, message)
    }

    /// 该端不提供已登记能力（不可重试；FR-620、ADR-032）。
    pub fn not_supported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotSupported, message)
    }

    /// 解析失败。
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ParseError, message)
    }

    /// 未分类内部错误。
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    /// 从 I/O 错误构造（统一分类，§11.5 稳定性故障处置）。
    ///
    /// 磁盘空间不足（`ErrorKind::StorageFull`，以及 Windows 的 `ERROR_DISK_FULL` =
    /// 112 这类被归入 `Other` 的原始码）单独说明：它是环境问题、修好空间后可重试，
    /// 不得与“权限/参数错误”混为一谈；其余 I/O 失败按原文报告。
    #[must_use]
    pub fn io(context: &str, err: &std::io::Error) -> Self {
        let disk_full =
            err.kind() == std::io::ErrorKind::StorageFull || err.raw_os_error() == Some(112);
        if disk_full {
            Self::new(
                ErrorCode::IoError,
                format!("{context}：磁盘空间不足（{err}）；清理空间后可重试"),
            )
        } else {
            Self::new(ErrorCode::IoError, format!("{context}：{err}"))
        }
    }
}

/// 成功信封：`data` 必须序列化为 map（扁平化后与 `schema_version` 同级）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OkEnvelope<T> {
    /// 信封 schema 版本。
    pub schema_version: u32,
    /// 业务载荷。
    #[serde(flatten)]
    pub data: T,
}

impl<T> OkEnvelope<T> {
    /// 以当前 `SCHEMA_VERSION` 包装业务载荷。
    pub fn new(data: T) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            data,
        }
    }
}

/// 错误信封：与成功信封共用 `schema_version` 约定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrEnvelope {
    /// 信封 schema 版本。
    pub schema_version: u32,
    /// 错误对象（四字段）。
    #[serde(flatten)]
    pub error: WhError,
}

impl ErrEnvelope {
    /// 包装错误对象。
    pub fn new(error: WhError) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            error,
        }
    }
}

impl From<WhError> for ErrEnvelope {
    fn from(error: WhError) -> Self {
        Self::new(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Serialize, Deserialize)]
    struct SamplePayload {
        session_id: String,
        stage: String,
    }

    #[test]
    fn error_code_strings_match_contract() {
        assert_eq!(ErrorCode::PermissionDenied.as_str(), "permission_denied");
        assert_eq!(ErrorCode::GateFailed.as_str(), "gate_failed");
        assert_eq!(ErrorCode::Backpressure.as_str(), "backpressure");
        assert_eq!(
            ErrorCode::RuntimeUnavailable.as_str(),
            "runtime_unavailable"
        );
    }

    /// `not_supported` 序列化稳定（FR-620、contracts/cui-cli.md §2 退出码表 55）：
    /// 字符串与既有码互异，`retryable` 默认 `false`。
    #[test]
    fn not_supported_serializes_stably_and_differs_from_existing_codes() {
        assert_eq!(ErrorCode::NotSupported.as_str(), "not_supported");
        let serialized = serde_json::to_value(ErrorCode::NotSupported).expect("序列化");
        assert_eq!(serialized, json!("not_supported"));
        let others: Vec<&str> = [
            ErrorCode::PermissionDenied,
            ErrorCode::GateFailed,
            ErrorCode::RuntimeUnavailable,
            ErrorCode::Backpressure,
            ErrorCode::InvalidState,
            ErrorCode::NotFound,
            ErrorCode::IoError,
            ErrorCode::ParseError,
            ErrorCode::Timeout,
            ErrorCode::Internal,
        ]
        .iter()
        .map(|code| code.as_str())
        .collect();
        assert!(
            !others.contains(&"not_supported"),
            "not_supported 必须与既有错误码互异：{others:?}"
        );
        let error = WhError::not_supported("该端未提供");
        assert_eq!(error.code, ErrorCode::NotSupported);
        assert!(!error.retryable, "端不提供语义默认不可重试（FR-620）");

        // 反序列化往返稳定。
        let round_trip: ErrorCode =
            serde_json::from_value(json!("not_supported")).expect("反序列化");
        assert_eq!(round_trip, ErrorCode::NotSupported);
    }

    /// `not_supported` 错误信封：schema_version + 四字段，details 恒有键。
    #[test]
    fn not_supported_envelope_carries_four_fields() {
        let envelope = ErrEnvelope::new(WhError::not_supported("该能力未在此端提供").with_details(
            json!({
                "surface_id": "runtime.status",
                "state": "reserved",
                "milestone": "M8",
            }),
        ));
        let value = serde_json::to_value(&envelope).expect("序列化");
        assert_eq!(value["schema_version"], json!(SCHEMA_VERSION));
        assert_eq!(value["code"], json!("not_supported"));
        assert_eq!(value["retryable"], json!(false));
        for field in ["code", "message", "retryable", "details"] {
            assert!(value.get(field).is_some(), "错误对象必须含 {field}");
        }
        assert_eq!(value["details"]["surface_id"], json!("runtime.status"));
        assert_eq!(value["details"]["state"], json!("reserved"));
        assert_eq!(value["details"]["milestone"], json!("M8"));
    }

    #[test]
    fn retryable_defaults_follow_contract() {
        assert!(!WhError::permission_denied("x").retryable);
        assert!(WhError::backpressure("x").retryable);
        assert!(!WhError::runtime_unavailable("x").retryable);
    }

    /// Check 206“磁盘不足”：满盘单独分类（可重试，消息可操作）；其他 I/O 失败按原文报告。
    #[test]
    fn io_error_classifies_disk_full_and_keeps_other_errors_verbatim() {
        let full = std::io::Error::new(std::io::ErrorKind::StorageFull, "no space left");
        let err = WhError::io("写出导出文件", &full);
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(err.retryable, "满盘属环境问题，清理空间后可重试");
        assert!(err.message.contains("磁盘空间不足"), "{}", err.message);

        // Windows 的 ERROR_DISK_FULL=112 会被归入 Other：仍须按满盘处置。
        let raw = std::io::Error::from_raw_os_error(112);
        let err = WhError::io("写出导出文件", &raw);
        assert!(err.message.contains("磁盘空间不足"), "{}", err.message);

        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = WhError::io("写出导出文件", &denied);
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(!err.message.contains("磁盘空间不足"), "{}", err.message);
        assert!(err.message.contains("写出导出文件"), "{}", err.message);
    }

    #[test]
    fn error_envelope_has_schema_version_and_four_fields() {
        let envelope = ErrEnvelope::new(
            WhError::gate_failed("Plan 转换被拒绝").with_details(json!({"gates": ["clarify"]})),
        );
        let value = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(value["schema_version"], json!(SCHEMA_VERSION));
        assert_eq!(value["code"], json!("gate_failed"));
        assert_eq!(value["message"], json!("Plan 转换被拒绝"));
        assert_eq!(value["retryable"], json!(false));
        assert_eq!(value["details"]["gates"][0], json!("clarify"));
    }

    #[test]
    fn ok_envelope_flattens_payload_next_to_schema_version() {
        let envelope = OkEnvelope::new(SamplePayload {
            session_id: "S-001".to_string(),
            stage: "Implement".to_string(),
        });
        let value = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(value["schema_version"], json!(1));
        assert_eq!(value["session_id"], json!("S-001"));
        assert_eq!(value["stage"], json!("Implement"));
    }
}
