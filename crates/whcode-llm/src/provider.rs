//! # LLM Provider 语义（T056、FR-201、§8.4）
//!
//! Provider 是模型调用的抽象：能力探测、流式结果、取消、计量来源、超时与错误归一化。
//! 接口为**同步迭代器**流式（tokio 延后引入，见 tasks.md T009 实现记录）；取消经
//! [`CancelToken`] 共享标志——对桩 Provider，取消在**下一个 chunk 之前**生效
//! （§8.4：底层无法立即取消时须说明取消请求何时生效）。
//!
//! 错误归一化：供应商错误一律映射为 [`whcode_domain::WhError`]（`timeout` /
//! `internal`），原文保留在 `details`，不作为控制指令（§10.1）。
//!
//! 边界：本模块不做持久化与 UI 状态（事件由 [`crate::stream`] 发射）；真实 HTTP
//! Provider 属后续里程碑，当前交付语义抽象与桩实现。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use whcode_domain::{ErrorCode, WhError};

use crate::metering::Usage;
use crate::transport_events::TransportSnapshot;

/// Provider 能力声明（探测结果，FR-201）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// 是否支持流式增量。
    pub streaming: bool,
    /// 是否支持外部取消。
    pub cancel: bool,
    /// 是否提供供应商计量值（否则计量为本地估算，FR-207）。
    pub usage_reporting: bool,
}

/// 模型调用请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmRequest {
    /// 请求 ID（链路串联 FR-204）。
    pub request_id: String,
    /// 所属会话。
    pub session_id: String,
    /// 所属任务（可空）。
    pub task_id: Option<String>,
    /// 模型标识。
    pub model: String,
    /// 提示词正文（脱敏策略由追踪层决定，本层不复制进事件）。
    pub prompt: String,
    /// 最大生成 token 数（估算来源）。
    pub max_tokens: u64,
    /// 超时；`None` 为不限（桩 Provider 不受影响）。
    pub timeout: Option<Duration>,
}

/// 流式 chunk（内容 + 序号 + 是否终结）。
#[derive(Debug, Clone, PartialEq)]
pub struct LlmChunk {
    /// chunk 序号（从 0 连续）。
    pub index: u64,
    /// 增量正文。
    pub text: String,
    /// 是否为终结 chunk；终结 chunk 可携带供应商计量。
    pub final_chunk: bool,
    /// 供应商计量（仅终结 chunk；`None` 表示供应商不报告，走本地估算，FR-207）。
    pub usage: Option<Usage>,
    /// 归一化终止原因（仅终结 chunk；`stop`/`length`/`tool_call` 等，FR-206／FR-208）。
    /// 非终结 chunk 恒为 `None`。
    pub finish_reason: Option<String>,
}

/// 取消令牌：共享标志，跨流共享（`Arc`）。
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// 发出取消请求。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// 是否已请求取消。
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 流式结果：chunk 迭代器 + 计量（Metering source，FR-207）。
pub struct ChunkStream {
    inner: Box<dyn Iterator<Item = Result<LlmChunk, WhError>> + Send>,
}

impl std::fmt::Debug for ChunkStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChunkStream")
    }
}

impl Iterator for ChunkStream {
    type Item = Result<LlmChunk, WhError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

/// LLM Provider 抽象（能力探测、流式、取消、超时、错误归一化）。
pub trait Provider: Send + Sync {
    /// Provider 名（如 `stub`、`sensenova`）。
    fn name(&self) -> &str;

    /// 能力探测（FR-201）。
    fn capabilities(&self) -> ProviderCapabilities;

    /// 是否可用（配置/凭据就绪）。
    fn is_available(&self) -> bool;

    /// 发起流式调用；返回 chunk 流。
    ///
    /// # Errors
    ///
    /// Provider 不可用或请求非法时返回归一化错误。
    fn execute(&self, request: &LlmRequest, cancel: &CancelToken) -> Result<ChunkStream, WhError>;

    /// 传输层只读快照（FR-616：字节计数与 RTT 可查询）。
    ///
    /// 默认 `None`（桩 Provider 无网络传输）；真实 Provider 返回累计快照。
    fn transport_snapshot(&self) -> Option<TransportSnapshot> {
        None
    }
}

/// 桩 Provider（quickstart §5：固定速率与 chunk 大小）。
///
/// 行为：把 `prompt` 按 [`StubProvider::CHUNK_SIZE`] 字符切块；每个 chunk 附带
/// `chunk_index`；最后一块为终结并携带**估算**计量（`cost_is_estimate = true`，
/// FR-207）。取消在下个 chunk 之前生效；超时按调用起点计算，超限后返回
/// `timeout` 错误。
#[derive(Debug, Clone)]
pub struct StubProvider {
    /// 能力声明（可见可改的默认值）。
    pub capabilities: ProviderCapabilities,
}

impl StubProvider {
    /// 桩的 chunk 切块大小（字符）。
    pub const CHUNK_SIZE: usize = 32;

    /// 桩：支持流式与取消，不报告供应商计量（走本地估算）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            capabilities: ProviderCapabilities {
                streaming: true,
                cancel: true,
                usage_reporting: false,
            },
        }
    }
}

impl Default for StubProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for StubProvider {
    fn name(&self) -> &str {
        "stub"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
    }

    fn is_available(&self) -> bool {
        true
    }

    fn execute(&self, request: &LlmRequest, cancel: &CancelToken) -> Result<ChunkStream, WhError> {
        if !request.prompt.is_empty() && request.model.trim().is_empty() {
            return Err(WhError::invalid_state("模型标识不得为空"));
        }
        let started = Instant::now();
        let chunks: Vec<String> = request
            .prompt
            .chars()
            .collect::<Vec<_>>()
            .chunks(Self::CHUNK_SIZE)
            .map(|chunk| chunk.iter().collect())
            .collect();
        let total = chunks.len() as u64;
        let timeout = request.timeout;
        let cancel = cancel.clone();
        let prompt = request.prompt.clone();

        let iter = (0..total).map(move |index| {
            // 取消在下一个 chunk 之前生效（§8.4 语义，本 Provider 的既定说明）。
            if cancel.is_cancelled() {
                return Err(WhError::new(
                    ErrorCode::Internal,
                    "调用已被取消：Provider 在下一个 chunk 前停止（取消语义见文档）".to_string(),
                ));
            }
            if let Some(timeout) = timeout
                && started.elapsed() > timeout
            {
                return Err(WhError {
                    code: ErrorCode::Timeout,
                    message: format!(
                        "Provider 超时：{}（已归一化，原文见 details）",
                        timeout.as_millis()
                    ),
                    retryable: true,
                    details: Some(serde_json::json!({
                        "provider": "stub",
                        "timeout_ms": timeout.as_millis(),
                    })),
                });
            }
            let final_chunk = index + 1 == total;
            Ok(LlmChunk {
                index,
                text: chunks[index as usize].clone(),
                final_chunk,
                usage: final_chunk.then(|| Usage::estimate(&prompt, &prompt)),
                finish_reason: final_chunk.then(|| "stop".to_string()),
            })
        });
        Ok(ChunkStream {
            inner: Box::new(iter),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(prompt: &str) -> LlmRequest {
        LlmRequest {
            request_id: "R-1".to_string(),
            session_id: "S-1".to_string(),
            task_id: None,
            model: "stub-model".to_string(),
            prompt: prompt.to_string(),
            max_tokens: 128,
            timeout: None,
        }
    }

    #[test]
    fn stub_streams_chunks_and_reports_estimated_usage_on_final() {
        let provider = StubProvider::new();
        assert_eq!(provider.name(), "stub");
        assert!(provider.capabilities().streaming);
        assert!(provider.is_available());

        let prompt = "a".repeat(100);
        let stream = provider
            .execute(&request(&prompt), &CancelToken::default())
            .expect("执行");
        let chunks: Vec<LlmChunk> = stream.collect::<Result<_, _>>().expect("流式");
        assert_eq!(chunks.len(), 4, "100 字符按 32 切块应为 4 块");
        assert!(!chunks[0].final_chunk);
        assert!(chunks[3].final_chunk);
        let usage = chunks[3].usage.expect("终结块携带计量");
        assert!(usage.cost_is_estimate, "桩计量必须标记为估算（FR-207）");
        assert_eq!(usage.prompt_tokens, 25, "启发式 4 字符/token");
    }

    #[test]
    fn cancel_takes_effect_before_next_chunk() {
        let provider = StubProvider::new();
        let cancel = CancelToken::default();
        let mut stream = provider
            .execute(&request(&"b".repeat(200)), &cancel)
            .expect("执行");
        stream.next().expect("首块").expect("首块成功");
        cancel.cancel();
        let err = stream
            .next()
            .expect("取消后仍有一次迭代机会")
            .expect_err("取消必须生效");
        assert!(
            err.to_string().contains("取消"),
            "取消语义须在错误中说明：{err}"
        );
    }

    #[test]
    fn timeout_is_normalized_with_provider_details() {
        let provider = StubProvider::new();
        let mut req = request(&"c".repeat(100));
        req.timeout = Some(Duration::from_millis(0));
        let mut stream = provider
            .execute(&req, &CancelToken::default())
            .expect("执行");
        // 首块在超时判定前仍可能成功（桩按调用起点计时，0ms 超时立即触发）。
        loop {
            match stream.next() {
                Some(Ok(_)) => continue,
                Some(Err(err)) => {
                    assert_eq!(err.code, ErrorCode::Timeout);
                    assert!(err.retryable, "超时可重试");
                    assert_eq!(err.details.as_ref().expect("details")["provider"], "stub");
                    break;
                }
                None => panic!("超时必须以错误形式报告"),
            }
        }
    }

    #[test]
    fn empty_model_with_prompt_is_rejected() {
        let provider = StubProvider::new();
        let mut req = request("x");
        req.model = String::new();
        let err = provider
            .execute(&req, &CancelToken::default())
            .expect_err("空模型标识必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }
}
