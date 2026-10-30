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

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use whcode_domain::{ErrorCode, WhError};

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
        inner,
        policy,
        attempts: AtomicU64::new(0),
    })
}

struct RetryingProvider {
    inner: Box<dyn Provider>,
    policy: StreamRetryPolicy,
    attempts: AtomicU64,
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
        snapshot.reconnect_attempts = snapshot
            .reconnect_attempts
            .saturating_add(u32::try_from(self.attempts.load(Ordering::Relaxed)).unwrap_or(u32::MAX));
        Some(snapshot)
    }

    fn execute(&self, request: &LlmRequest, cancel: &CancelToken) -> Result<ChunkStream, WhError> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        let stream = self.inner.execute(request, cancel)?;
        Ok(ChunkStream {
            inner: Box::new(RetryStream {
                inner: Some(stream),
                request: request.clone(),
                cancel: cancel.clone(),
                provider: self,
                retries_left: self.policy.max_retries,
                next_delay: self.policy.base_delay,
                deadline: Instant::now() + self.policy.total_budget,
                emitted: false,
            }),
        })
        // 首次 execute 的即时失败也交给 RetryStream 统一判定：该路径同样满足
        // “未发射任何 chunk”，由迭代器完成重试或上抛。
    }
}

/// 带重试语义的流迭代器（`emitted` 是 safe_to_retry 的判定状态）。
struct RetryStream<'p> {
    inner: Option<ChunkStream>,
    request: LlmRequest,
    cancel: CancelToken,
    provider: &'p RetryingProvider,
    retries_left: u32,
    next_delay: Duration,
    deadline: Instant,
    emitted: bool,
}

impl RetryStream<'_> {
    /// 退避睡眠（取消感知切片；返回 false 表示已取消或超预算，调用方上抛 last_error）。
    fn backoff_sleep(&mut self, last_error: &WhError) -> bool {
        let delay =
            retry_after_ms(last_error).unwrap_or(self.next_delay.min(self.provider.policy.max_delay));
        let deadline = Instant::now() + delay;
        while Instant::now() < deadline {
            if self.cancel.is_cancelled() {
                return false;
            }
            let remain = u64::try_from((deadline - Instant::now()).as_millis()).unwrap_or(0);
            std::thread::sleep(Duration::from_millis(SLEEP_SLICE_MS.min(remain)));
        }
        // 总预算：等待累计超预算则不再重试。
        Instant::now() < self.deadline
    }

    fn restart(&mut self) {
        self.provider.attempts.fetch_add(1, Ordering::Relaxed);
        self.retries_left = self.retries_left.saturating_sub(1);
        self.next_delay = self
            .next_delay
            .checked_mul(2)
            .unwrap_or(self.provider.policy.max_delay)
            .min(self.provider.policy.max_delay);
        self.inner = self
            .provider
            .inner
            .execute(&self.request, &self.cancel)
            .ok();
    }
}

impl Iterator for RetryStream<'_> {
    type Item = Result<crate::provider::LlmChunk, WhError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let Some(stream) = self.inner.as_mut() else {
                return None;
            };
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
                    if !self.backoff_sleep(&err) {
                        return Some(Err(err));
                    }
                    if self.cancel.is_cancelled() {
                        return Some(Err(err));
                    }
                    self.restart();
                    if self.inner.is_none() {
                        // 重启请求即时失败：进入下一轮判定（仍可能继续重试）。
                        continue;
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
