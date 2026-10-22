//! # 运行时结构化错误（T107、FR-708/FR-710、V-306）
//!
//! 能力不足/缺失/不兼容一律返回结构化错误（`runtime_unavailable`），并附
//! 安装/替代建议（FR-708）；**接入 Analyze 阻止执行**（FR-710）——引用未启用
//! 运行时的任务在执行前被阻止（V-306，由调用方以 [`requires_unavailable_runtime`]
//! 判定）。
//!
//! 边界：错误构造为纯函数；"阻止执行"由 app 门禁/任务执行路径调用本模块判定。

use whcode_domain::{ErrorCode, WhError};

/// 失败原因（FR-708 三类 + V-306 未启用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableReason {
    /// 能力不足（引擎存在但能力未覆盖）。
    CapabilityInsufficient,
    /// 运行时缺失（未安装/未启用）。
    Missing,
    /// 不兼容（版本/平台不满足）。
    Incompatible,
    /// 未启用（FR-704：enabled=false 不得执行）。
    NotEnabled,
}

impl std::fmt::Display for UnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl UnavailableReason {
    /// 稳定标识（入 details）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CapabilityInsufficient => "capability_insufficient",
            Self::Missing => "missing",
            Self::Incompatible => "incompatible",
            Self::NotEnabled => "not_enabled",
        }
    }
}

/// 结构化错误：`runtime_unavailable` + 安装/替代建议（FR-708）。
#[must_use]
pub fn runtime_unavailable(name: &str, reason: UnavailableReason, suggestion: &str) -> WhError {
    WhError {
        code: ErrorCode::RuntimeUnavailable,
        message: format!("运行时 {name} 不可用（{reason}）：{suggestion}"),
        retryable: false,
        details: Some(serde_json::json!({
            "runtime": name,
            "reason": reason.as_str(),
            "suggestion": suggestion,
        })),
    }
}

/// 任务是否引用未启用运行时（V-306 执行前判定；FR-710 接入 Analyze 阻止）。
///
/// 从任务声明的运行时清单中检出未启用者——存在即阻止执行。
#[must_use]
pub fn requires_unavailable_runtime(required: &[String], enabled: &[String]) -> Vec<String> {
    required
        .iter()
        .filter(|runtime| !enabled.iter().any(|candidate| candidate == *runtime))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_unavailable_carries_reason_and_suggestion() {
        let error = runtime_unavailable(
            "python",
            UnavailableReason::Missing,
            "whcode runtime install python",
        );
        assert_eq!(error.code, ErrorCode::RuntimeUnavailable);
        assert!(!error.retryable, "不可用不可重试（FR-708）");
        assert_eq!(
            error.details.as_ref().expect("details")["reason"],
            "missing"
        );
        assert!(error.to_string().contains("whcode runtime install python"));
    }

    #[test]
    fn unavailable_runtimes_block_execution_before_start() {
        // V-306：引用未启用运行时的任务在执行前被阻止。
        let required = vec!["python".to_string(), "js".to_string()];
        let enabled = vec!["python".to_string()];
        let missing = requires_unavailable_runtime(&required, &enabled);
        assert_eq!(missing, vec!["js"], "未启用运行时必须检出");
        assert!(
            requires_unavailable_runtime(&enabled, &enabled).is_empty(),
            "全部启用则无阻止"
        );
    }
}
