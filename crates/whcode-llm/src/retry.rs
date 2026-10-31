//! # 流式生成重试（FR-730、ADR-050）
//!
//! [`with_stream_retry`] 把任意 [`Provider`] 包装为**带流式重试语义**的 Provider：
//! 失败发生在**首个 chunk 之前**且错误可重试（[`WhError::retryable`]）时，按退避策略
//! 重新发起请求；重试的总时长与次数有界（[`StreamRetryPolicy`]），退避睡眠**取消感知**
//! （每个 50 ms 切片检查取消标记），429 的 `retry_after_ms`（错误 details）优先于固定退避。
//!
//! **safe_to_retry 判据**（BH 契约语义与 [`crate::provider::WhError::retryable`] 的合并，
//! ADR-050 §2.2）：只有当调用方尚未消费任何成功 chunk 时才允许重放——此时重放不会
//! 产生重复事件/重复正文（幂等充分条件）；一旦有 chunk 已发射，任何失败都原样上抛
//! （调用方已持有部分输出，重放必然重复计费与事件）。
//!
//! 可观测：实际尝试次数并入 [`TransportSnapshot::reconnect_attempts`]（信封 `transport`
//! 已有字段，四端可见，FR-616 口径）。
//!
//! 默认策略是**交互优先的当前和解**（ADR-050 §2.3）：5 次、基准 1 s、×2、上限 30 s、
//! 总预算 60 s——对照 ZCode（10 次、基准 2 s、上限 60 s）：交互场景中长时间阻塞调用
//! 线程的代价高于边际成功率；复审触发＝真实会话首 chunk 失败率统计（§5）。

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use whcode_domain::WhError;

use crate::provider::{ChunkStream, LlmRequest, Provider};
use crate::{CancelToken, TransportSnapshot};

/// 退避睡眠的取消检查切片。
const SLEEP_SLICE_MS: u64 = 50;

/// 流式重试策略（有界；与探测路径的 [`crate::http::client::RetryPolicy`] 语义分离）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamRetryPolicy {
    /// 最大重试次数（不含首次尝试）。
    pub max_retries: u32,
    /// 首次重试前的等待（后续 ×2，封顶 [`StreamRetryPolicy::max_delay`]）。
    pub base_delay: Duration,
    /// 单次等待上限。
    pub max_delay: Duration,
    /// 重试总时长预算（含全部等待；超出即上抛最后一次错误）。
    pub total_budget: Duration,
}

impl StreamRetryPolicy {
    /// 默认策略（交互优先的当前和解，ADR-050 §2.3）。
    #[must_use]
    pub const fn standard() -> Self {
        Self {
            max_retries: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            total_budget: Duration::from_secs(60),
        }
    }

    /// 不重试（测试与显式关闭用）。
    #[must_use]
    pub const fn none() -> Self {
        Self {
            max_retries: 0,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            total_budget: Duration::ZERO,
        }
    }
}

/// 退避睡眠（取消感知切片；返回 false 表示已取消或超总预算）。
fn sleep_backoff(cancel: &CancelToken, delay: Duration, deadline: Instant) -> bool {
    let until = Instant::now() + delay;
    while Instant::now() < until {
        if cancel.is_cancelled() {
            return false;
        }
        let remain = u64::try_from((until - Instant::now()).as_millis()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(SLEEP_SLICE_MS.min(remain)));
    }
    Instant::now() < deadline
}

/// 429 等 backpressure 错误 details 里的 `retry_after_ms`（供应商口径优先于固定退避）。
fn retry_after_ms(err: &WhError) -> Option<Duration> {
    err.details
        .as_ref()?
        .get("retry_after_ms")
        .and_then(serde_json::Value::as_u64)
        .map(Duration::from_millis)
}

/// 把 Provider 包装为带流式重试语义的 Provider（FR-730）。
#[must_use]
pub fn with_stream_retry(inner: Box<dyn Provider>, policy: StreamRetryPolicy) -> Box<dyn Provider> {
    Box::new(RetryingProvider {
        inner: Arc::from(inner),
        policy,
        attempts: Arc::new(AtomicU64::new(0)),
    })
}

struct RetryingProvider {
    inner: Arc<dyn Provider>,
    policy: StreamRetryPolicy,
    attempts: Arc<AtomicU64>,
}

impl Provider for RetryingProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn capabilities(&self) -> crate::provider::ProviderCapabilities {
        self.inner.capabilities()
    }

    fn is_available(&self) -> bool {
        self.inner.is_available()
    }

    fn transport_snapshot(&self) -> Option<TransportSnapshot> {
        let mut snapshot = self.inner.transport_snapshot()?;
        snapshot.reconnect_attempts = snapshot.reconnect_attempts.saturating_add(
            u32::try_from(self.attempts.load(Ordering::Relaxed)).unwrap_or(u32::MAX),
        );
        Some(snapshot)
    }

    fn execute(&self, request: &LlmRequest, cancel: &CancelToken) -> Result<ChunkStream, WhError> {
        let mut retries_left = self.policy.max_retries;
        let mut next_delay = self.policy.base_delay;
        let deadline = Instant::now() + self.policy.total_budget;
        // 连接期失败（发送请求即错）在此重试；流建立后的失败交给 RetryStream。
        loop {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            match self.inner.execute(request, cancel) {
                Ok(stream) => {
                    return Ok(ChunkStream {
                        inner: Box::new(RetryStream {
                            inner: Some(stream),
                            request: request.clone(),
                            cancel: cancel.clone(),
                            provider: Arc::clone(&self.inner),
                            policy: self.policy,
                            attempts: Arc::clone(&self.attempts),
                            retries_left,
                            next_delay,
                            deadline,
                            emitted: false,
                            last_error: None,
                        }),
                    });
                }
                Err(err) => {
                    if !err.retryable
                        || retries_left == 0
                        || !sleep_backoff(
                            cancel,
                            retry_after_ms(&err).unwrap_or(next_delay.min(self.policy.max_delay)),
                            deadline,
                        )
                    {
                        return Err(err);
                    }
                    if cancel.is_cancelled() {
                        return Err(err);
                    }
                    retries_left = retries_left.saturating_sub(1);
                    next_delay = next_delay
                        .checked_mul(2)
                        .unwrap_or(self.policy.max_delay)
                        .min(self.policy.max_delay);
                }
            }
        }
    }
}

/// 带重试语义的流迭代器（`emitted` 是 safe_to_retry 的判定状态；全 owned，'static）。
struct RetryStream {
    inner: Option<ChunkStream>,
    request: LlmRequest,
    cancel: CancelToken,
    provider: Arc<dyn Provider>,
    policy: StreamRetryPolicy,
    attempts: Arc<AtomicU64>,
    retries_left: u32,
    next_delay: Duration,
    deadline: Instant,
    emitted: bool,
    last_error: Option<WhError>,
}

impl Iterator for RetryStream {
    type Item = Result<crate::provider::LlmChunk, WhError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let stream = self.inner.as_mut()?;
            match stream.next() {
                Some(Ok(chunk)) => {
                    self.emitted = true;
                    return Some(Ok(chunk));
                }
                Some(Err(err)) => {
                    // safe_to_retry 判据：零 chunk 已发射 + 错误可重试 + 次数/预算有余。
                    if self.emitted || !err.retryable || self.retries_left == 0 {
                        return Some(Err(err));
                    }
                    self.inner = None;
                    self.last_error = Some(err);
                    // 有界重试环：等待 → 重启 → 再判定；失败不静默结束流（返回 None
                    // 会被调用方当作“正常完成”，属于伪造成功——必须显式上抛）。
                    loop {
                        let last = self.last_error.as_ref().expect("已存错误");
                        let delay = retry_after_ms(last)
                            .unwrap_or(self.next_delay.min(self.policy.max_delay));
                        if !sleep_backoff(&self.cancel, delay, self.deadline)
                            || self.cancel.is_cancelled()
                        {
                            return self.last_error.take().map(Err);
                        }
                        if self.retries_left == 0 {
                            return Some(Err(self.last_error.take().expect("已存错误")));
                        }
                        self.attempts.fetch_add(1, Ordering::Relaxed);
                        self.retries_left = self.retries_left.saturating_sub(1);
                        self.next_delay = self
                            .next_delay
                            .checked_mul(2)
                            .unwrap_or(self.policy.max_delay)
                            .min(self.policy.max_delay);
                        match self.provider.execute(&self.request, &self.cancel) {
                            Ok(stream) => {
                                self.inner = Some(stream);
                                break;
                            }
                            Err(restart_err) => {
                                self.last_error = Some(restart_err);
                            }
                        }
                    }
                }
                None => return None,
            }
        }
    }
}

/// 重试策略非零时的构造校验（ADR-050：策略错误在装配期暴露，不静默降级）。
///
/// # Errors
///
/// 策略字段非法（max_delay < base_delay 且 max_retries > 0）时返回
/// [`ErrorCode::InvalidState`]。
pub fn validate_policy(policy: &StreamRetryPolicy) -> Result<(), WhError> {
    if policy.max_retries > 0 && policy.max_delay < policy.base_delay {
        return Err(WhError::invalid_state(
            "流式重试策略非法：max_delay 不得小于 base_delay（max_retries>0 时）",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{LlmChunk, ProviderCapabilities};
    use serde_json::json;
    use std::sync::atomic::AtomicU32;
    use whcode_domain::ErrorCode;

    /// 夹具：前 `failures` 次 execute 返回错误（可配可重试性/中途失败）。
    struct FlakyProvider {
        failures: AtomicU32,
        non_retryable: bool,
        mid_stream: bool,
    }

    impl FlakyProvider {
        fn new(failures: u32) -> Self {
            Self {
                failures: AtomicU32::new(failures),
                non_retryable: false,
                mid_stream: false,
            }
        }

        fn non_retryable(failures: u32) -> Self {
            Self {
                failures: AtomicU32::new(failures),
                non_retryable: true,
                mid_stream: false,
            }
        }

        fn mid_stream() -> Self {
            Self {
                failures: AtomicU32::new(1),
                non_retryable: false,
                mid_stream: true,
            }
        }
    }

    fn flaky_err() -> WhError {
        WhError::new(ErrorCode::IoError, "连接被拒（夹具）".to_string())
            .with_details(json!({ "retry_after_ms": 1 }))
    }

    impl Provider for FlakyProvider {
        fn name(&self) -> &str {
            "flaky"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                cancel: true,
                usage_reporting: false,
            }
        }

        fn is_available(&self) -> bool {
            true
        }

        fn transport_snapshot(&self) -> Option<TransportSnapshot> {
            Some(TransportSnapshot {
                connection_id: "flaky".to_string(),
                state: "idle".to_string(),
                bytes_sent: 0,
                bytes_received: 0,
                rtt_samples: Vec::new(),
                reconnect_attempts: 0,
            })
        }

        fn execute(
            &self,
            _request: &LlmRequest,
            _cancel: &CancelToken,
        ) -> Result<ChunkStream, WhError> {
            let remaining = self.failures.fetch_sub(1, Ordering::SeqCst);
            if remaining > 1 {
                let mut err = flaky_err();
                if self.non_retryable {
                    err.retryable = false;
                }
                return Err(err);
            }
            let chunks: Vec<Result<LlmChunk, WhError>> = if self.mid_stream {
                vec![
                    Ok(LlmChunk {
                        index: 0,
                        text: "部分".to_string(),
                        final_chunk: false,
                        usage: None,
                        finish_reason: None,
                        tool_calls: Vec::new(),
                    }),
                    Err(flaky_err()),
                ]
            } else {
                vec![Ok(LlmChunk {
                    index: 0,
                    text: "ok".to_string(),
                    final_chunk: true,
                    usage: None,
                    finish_reason: Some("stop".to_string()),
                    tool_calls: Vec::new(),
                })]
            };
            Ok(ChunkStream {
                inner: Box::new(chunks.into_iter()),
            })
        }
    }

    fn request() -> LlmRequest {
        LlmRequest {
            request_id: "R".to_string(),
            session_id: "S".to_string(),
            task_id: None,
            model: "m".to_string(),
            messages: vec![crate::provider::LlmMessage::user("hi")],
            max_tokens: 16,
            timeout: None,
            tools: None,
        }
    }

    fn fast_policy() -> StreamRetryPolicy {
        StreamRetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(4),
            total_budget: Duration::from_secs(1),
        }
    }

    #[test]
    fn pre_stream_failure_is_retried_and_succeeds() {
        let provider = with_stream_retry(Box::new(FlakyProvider::new(2)), fast_policy());
        let chunks: Vec<_> = provider
            .execute(&request(), &CancelToken::default())
            .expect("重试后成功")
            .collect::<Result<_, _>>()
            .expect("流完整");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "ok");
        // 可观测：尝试次数并入传输快照 reconnect_attempts（3 次尝试 = 2 次重试）。
        let snapshot = provider.transport_snapshot().expect("快照");
        assert_eq!(snapshot.reconnect_attempts, 2);
    }

    #[test]
    fn mid_stream_failure_after_chunk_is_not_retried() {
        let provider = with_stream_retry(Box::new(FlakyProvider::mid_stream()), fast_policy());
        let result: Result<Vec<_>, _> = provider
            .execute(&request(), &CancelToken::default())
            .expect("流建立")
            .collect();
        let err = result.expect_err("chunk 已发射后的失败必须上抛（safe_to_retry 判据）");
        assert!(err.retryable, "错误本身可重试，但重放不安全");
    }

    #[test]
    fn non_retryable_failure_is_not_retried() {
        let provider = with_stream_retry(Box::new(FlakyProvider::non_retryable(2)), fast_policy());
        let err = provider
            .execute(&request(), &CancelToken::default())
            .expect_err("不可重试错误必须立即上抛");
        assert!(!err.retryable);
        assert_eq!(
            provider
                .transport_snapshot()
                .expect("快照")
                .reconnect_attempts,
            1
        );
    }

    #[test]
    fn retries_are_bounded_by_max_retries() {
        let provider = with_stream_retry(Box::new(FlakyProvider::new(u32::MAX)), fast_policy());
        assert!(
            provider
                .execute(&request(), &CancelToken::default())
                .is_err()
        );
        // 4 次尝试（1 + 3 重试），全部记入快照。
        assert_eq!(
            provider
                .transport_snapshot()
                .expect("快照")
                .reconnect_attempts,
            4
        );
    }

    #[test]
    fn restart_failure_is_surfaced_not_silently_dropped() {
        // 回归（T221）：execute 首次成功、重启请求失败——失败必须上抛，
        // 不得以“流结束”伪造成功（修复前 continue 回到 None 分支）。
        struct RestartFailThenStream {
            calls: AtomicU32,
        }
        impl Provider for RestartFailThenStream {
            fn name(&self) -> &str {
                "restart-fail"
            }
            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities {
                    streaming: true,
                    cancel: true,
                    usage_reporting: false,
                }
            }
            fn is_available(&self) -> bool {
                true
            }
            fn execute(
                &self,
                _request: &LlmRequest,
                _cancel: &CancelToken,
            ) -> Result<ChunkStream, WhError> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    // 流建立后、任何 chunk 之前失败（触发流内重试）。
                    return Ok(ChunkStream {
                        inner: Box::new(std::iter::once(Err(flaky_err()))),
                    });
                }
                Err(flaky_err())
            }
        }
        let provider = with_stream_retry(
            Box::new(RestartFailThenStream {
                calls: AtomicU32::new(0),
            }),
            fast_policy(),
        );
        let result: Result<Vec<_>, _> = provider
            .execute(&request(), &CancelToken::default())
            .expect("流建立（首次 execute 成功）")
            .collect();
        let err = result.expect_err("重启失败必须显式上抛，不得静默结束");
        assert!(err.retryable);
    }

    #[test]
    fn policy_validation_rejects_inverted_delays() {
        let policy = StreamRetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(1),
            total_budget: Duration::from_secs(10),
        };
        let err = validate_policy(&policy).expect_err("倒挂策略必须拒绝");
        assert!(err.message.contains("max_delay"), "{err}");
        assert!(validate_policy(&StreamRetryPolicy::none()).is_ok());
    }
}
