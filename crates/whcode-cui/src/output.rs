//! # CUI `--json` 输出信封（T085、FR-602、§8.1、contracts/cui-cli.md §3）
//!
//! 全部 CUI JSON 输出统一信封：`schema_version` + 错误对象四字段
//! （`code`/`message`/`retryable`/`details`）+ 稳定退出码。信封是唯一输出形态，
//! 命令树（T084）与各视图函数经 [`success_envelope`] / [`error_envelope`] 统一包装。
//!
//! 稳定退出码（contracts/cui-cli.md）：成功 0；错误按 code 映射（见 [`exit_code`]）。

use serde::Serialize;
use whcode_domain::{ErrorCode, SCHEMA_VERSION, WhError};

/// 成功退出码。
pub const EXIT_SUCCESS: i32 = 0;

/// 用法/参数错误退出码（clap 解析失败；contracts/cui-cli.md §3 稳定退出码）。
pub const EXIT_USAGE: i32 = 2;

/// 成功信封（数据 + schema_version）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Envelope<T: Serialize> {
    /// 事件 schema 版本（contracts/cui-cli.md §3 必备）。
    pub schema_version: u32,
    /// 命令数据。
    pub data: T,
}

/// 错误信封（schema_version + 错误对象四字段，FR-602）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorEnvelope {
    /// 事件 schema 版本。
    pub schema_version: u32,
    /// 错误码（稳定字符串）。
    pub code: String,
    /// 面向使用者的说明（不得含秘密）。
    pub message: String,
    /// 是否可重试。
    pub retryable: bool,
    /// 附加细节（恒有四字段，无细节时为 null）。
    pub details: serde_json::Value,
}

/// 稳定退出码映射（contracts/cui-cli.md）。
#[must_use]
pub const fn exit_code(code: &ErrorCode) -> i32 {
    match code {
        ErrorCode::PermissionDenied => 10,
        ErrorCode::GateFailed => 20,
        ErrorCode::RuntimeUnavailable => 30,
        ErrorCode::Backpressure => 40,
        ErrorCode::InvalidState => 50,
        // 已登记但该端不提供（预留/已登记排除；FR-620、ADR-032）。
        ErrorCode::NotSupported => 55,
        ErrorCode::NotFound => 60,
        ErrorCode::IoError => 70,
        ErrorCode::ParseError => 80,
        ErrorCode::Timeout => 90,
        ErrorCode::Internal => 99,
    }
}

/// 成功信封（T085）。
#[must_use]
pub fn success_envelope<T: Serialize>(data: T) -> Envelope<T> {
    Envelope {
        schema_version: SCHEMA_VERSION,
        data,
    }
}

/// 错误信封（错误对象恒四字段，FR-602）。
#[must_use]
pub fn error_envelope(error: &WhError) -> ErrorEnvelope {
    ErrorEnvelope {
        schema_version: SCHEMA_VERSION,
        code: error.code.as_str().to_string(),
        message: error.message.clone(),
        retryable: error.retryable,
        details: error.details.clone().unwrap_or(serde_json::Value::Null),
    }
}

/// 错误对应的稳定退出码。
#[must_use]
pub const fn error_exit_code(error: &WhError) -> i32 {
    exit_code(&error.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn success_envelope_carries_schema_version() {
        let envelope = success_envelope(json!({ "stage": "plan" }));
        let value = serde_json::to_value(&envelope).expect("序列化");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["data"]["stage"], "plan");
    }

    #[test]
    fn error_envelope_has_four_fields_and_stable_exit_codes() {
        let error = WhError::permission_denied("无权限");
        let envelope = error_envelope(&error);
        let value = serde_json::to_value(&envelope).expect("序列化");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["code"], "permission_denied");
        assert_eq!(value["retryable"], false);
        // 四字段恒有：无细节时 details 为 null，不省略键。
        assert!(value.get("details").is_some());
        assert!(value.get("message").is_some());
        assert_eq!(error_exit_code(&error), 10, "稳定退出码");

        let with_details = WhError {
            details: Some(json!({"unresolved": ["CLAR-001"]})),
            ..WhError::gate_failed("门禁未通过")
        };
        let envelope = error_envelope(&with_details);
        let value = serde_json::to_value(&envelope).expect("序列化");
        assert_eq!(value["details"]["unresolved"][0], "CLAR-001");
        assert_eq!(error_exit_code(&with_details), 20);
    }

    #[test]
    fn exit_codes_are_stable_across_error_codes() {
        let seen: Vec<i32> = [
            ErrorCode::PermissionDenied,
            ErrorCode::GateFailed,
            ErrorCode::RuntimeUnavailable,
            ErrorCode::Backpressure,
            ErrorCode::InvalidState,
            ErrorCode::NotSupported,
            ErrorCode::NotFound,
            ErrorCode::IoError,
            ErrorCode::ParseError,
            ErrorCode::Timeout,
            ErrorCode::Internal,
        ]
        .iter()
        .map(exit_code)
        .collect();
        let mut unique = seen.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), seen.len(), "退出码必须互异且稳定");
        assert_eq!(seen[0], 10);
        assert_eq!(seen[10], 99);
        // 55（not_supported）介于 50（invalid_state）与 60（not_found）之间，
        // contracts/cui-cli.md §2：语义介于“状态异常”与“不存在”之间。
        assert_eq!(exit_code(&ErrorCode::NotSupported), 55);
        assert_eq!(seen[4], 50);
        assert_eq!(seen[6], 60);
        assert_eq!(error_exit_code(&WhError::not_supported("未提供")), 55);
    }
}
