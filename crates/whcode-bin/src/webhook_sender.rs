//! # 出站 webhook 发送器（装配根，P6.4／FR-788／ADR-079 决策 6）
//!
//! 网络栈只在 `whcode-llm`（`http` feature），app 只定义 [`WebhookSender`] 口。
//! 本模块是把两者接起来的**唯一**位置：按次求值凭据、按次构造请求，不缓存、不复用。
//!
//! 凭据纪律（ADR-079 决策 1）：配置里只有环境变量名；取值在本函数内求值并只用于本次
//! 请求头。变量缺失**不发无鉴权请求**，按可重试失败登记（原因只说变量名，不含取值）。

use std::time::Duration;

use whcode_app::webhooks::{WebhookRequest, WebhookSendOutcome, WebhookSender};

/// 出站 webhook 发送器：把 app 的请求映射到 `whcode_llm::http::webhook`。
pub struct LlmWebhookSender;

impl WebhookSender for LlmWebhookSender {
    fn send(&self, request: &WebhookRequest) -> WebhookSendOutcome {
        // 凭据按次求值：不驻留实例，不进入错误文本。
        let token = match request.token_env.as_deref() {
            None => None,
            Some(name) => match std::env::var(name) {
                Ok(value) if !value.trim().is_empty() => Some(value),
                Ok(_) => {
                    return WebhookSendOutcome::Retryable {
                        status: None,
                        reason: format!(
                            "环境变量 {name} 为空：为避免无鉴权请求，本次不投递（ADR-079 决策 1）"
                        ),
                    };
                }
                Err(_) => {
                    return WebhookSendOutcome::Retryable {
                        status: None,
                        reason: format!(
                            "环境变量 {name} 未设置或不可读：为避免无鉴权请求，本次不投递"
                        ),
                    };
                }
            },
        };
        let header = token.as_deref().map(|value| format!("Bearer {value}"));
        let outcome = whcode_llm::http::webhook::post(&whcode_llm::http::webhook::WebhookRequest {
            url: &request.url,
            body: &request.body,
            idempotency_key: &request.idempotency_key,
            authorization: header.as_deref(),
            timeout: Duration::from_millis(request.timeout_ms),
        });
        drop(header);
        match outcome {
            whcode_llm::http::webhook::WebhookOutcome::Delivered { status } => {
                WebhookSendOutcome::Delivered { status }
            }
            whcode_llm::http::webhook::WebhookOutcome::Retryable { status, reason } => {
                WebhookSendOutcome::Retryable { status, reason }
            }
            whcode_llm::http::webhook::WebhookOutcome::Permanent { status, reason } => {
                WebhookSendOutcome::Permanent { status, reason }
            }
        }
    }
}
