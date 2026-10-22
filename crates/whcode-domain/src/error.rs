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

    /// 解析失败。
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ParseError, message)
    }

    /// 未分类内部错误。
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
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

    #[test]
    fn retryable_defaults_follow_contract() {
        assert!(!WhError::permission_denied("x").retryable);
        assert!(WhError::backpressure("x").retryable);
        assert!(!WhError::runtime_unavailable("x").retryable);
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
