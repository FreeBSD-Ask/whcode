//! # 模型计量（T063、FR-207、contracts/event-schema.md §4）
//!
//! 计量来源两级：**供应商返回值**与**本地估算**。估算（启发式 token、估算成本）
//! 必须标记 `cost_is_estimate = true`；供应商返回的计量值另行保存，不得与估算混写
//! （FR-207）。估算启发式：约 4 字符 / token（对 CJK 偏高，仅作预算与展示用途，
//! 不作计量承诺——偏差记录在模块文档）。
//!
//! `RequestCompleted` 事件的载荷按 event-schema §1 示例字段输出
//! （`prompt_tokens` / `completion_tokens` / `latency_ms` / `cost_estimate` /
//! `cost_is_estimate`）。

use serde::Serialize;

/// 一次调用的计量（供应商返回或本地估算）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Usage {
    /// 提示 token 数。
    pub prompt_tokens: u64,
    /// 生成 token 数。
    pub completion_tokens: u64,
    /// 往返耗时（毫秒）。
    pub latency_ms: u64,
    /// 成本（供应商返回或估算）。
    pub cost: Option<f64>,
    /// 成本与 token 是否为估算（FR-207：估算必须标记）。
    pub cost_is_estimate: bool,
}

impl Usage {
    /// 本地估算：约 4 字符 / token，成本按估算（`cost_is_estimate = true`）。
    ///
    /// 估算用于预算与展示，不作计量承诺；供应商返回值必须另行保存（FR-207）。
    #[must_use]
    pub fn estimate(prompt: &str, completion: &str) -> Self {
        Self {
            prompt_tokens: estimate_tokens(prompt),
            completion_tokens: estimate_tokens(completion),
            latency_ms: 0,
            cost: None,
            cost_is_estimate: true,
        }
    }

    /// 供应商返回的计量；`cost_is_estimate` 由 Provider 对该值的置信声明决定。
    #[must_use]
    pub fn provider_reported(
        prompt_tokens: u64,
        completion_tokens: u64,
        latency_ms: u64,
        cost: Option<f64>,
        cost_is_estimate: bool,
    ) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            latency_ms,
            cost,
            cost_is_estimate,
        }
    }

    /// 附加耗时（估算路径无计时，由调用方填充）。
    #[must_use]
    pub fn with_latency(mut self, latency_ms: u64) -> Self {
        self.latency_ms = latency_ms;
        self
    }

    /// `RequestCompleted` 事件载荷（event-schema §1 示例字段）。
    #[must_use]
    pub fn completion_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "latency_ms": self.latency_ms,
            "cost_estimate": self.cost,
            "cost_is_estimate": self.cost_is_estimate,
        })
    }
}

/// 启发式 token 估算：字符数除以 4 向上取整（CJK 偏高，仅预算/展示用途）。
#[must_use]
pub fn estimate_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    chars.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_marks_cost_as_estimate_and_uses_heuristic() {
        let usage = Usage::estimate("abcd", "x");
        assert!(usage.cost_is_estimate, "估算必须标记（FR-207）");
        assert_eq!(usage.prompt_tokens, 1);
        assert_eq!(usage.completion_tokens, 1);

        let cjk = Usage::estimate("你好世界", "");
        assert_eq!(
            cjk.prompt_tokens, 1,
            "4 CJK 字符 → 1 token（偏高仅预算用途）"
        );

        let empty = estimate_tokens("");
        assert_eq!(empty, 0);
    }

    #[test]
    fn provider_reported_keeps_claimed_estimate_flag() {
        let reported = Usage::provider_reported(2_100, 540, 7_321, Some(0.004_2), false);
        assert!(!reported.cost_is_estimate, "供应商计量不标估算");
        let payload = reported.completion_payload();
        assert_eq!(payload["prompt_tokens"], 2_100);
        assert_eq!(payload["cost_estimate"], serde_json::json!(0.004_2));
        assert_eq!(payload["cost_is_estimate"], false);

        let unsure = Usage::provider_reported(1, 1, 1, None, true);
        assert!(unsure.cost_is_estimate);
    }
}
