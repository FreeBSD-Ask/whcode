//! # 流式事件发射与积压合并（T057、FR-206、R-04、ADR-006 双队列分离）
//!
//! [`EventRouter`] 把事件按可靠性分级分离到两条**有界**队列（ADR-006）：
//!
//! - **关键**（任务状态转换、请求最终结果、工具意图/结果、门禁、审计）：**不得
//!   静默丢弃**——队列饱和返回 `backpressure`（`retryable = true`），调用方背压或
//!   安全停止（§4.2）；
//! - **展示**（进度、帧率等可重建信息）：允许合并或丢弃，且必须留下**可查询标记**
//!   ——队列饱和时丢弃最旧并累计 [`EventRouter::merged_total`]，`drain_display`
//!   在末尾追加合并标记事件。
//!
//! [`pump_stream`] 把 Provider 的 chunk 流编排为事件：`RequestStarted`（关键）→
//! 每 [`PumpConfig::progress_every`] 个 chunk 一个 `ProgressChanged`（展示，载荷只含
//! 计数与字节数，**不含正文**——不得把大段响应塞进进度事件，R-05）→ 终结块
//! `RequestCompleted`（关键，携带计量、`usage_source` 与 `finish_reason`）。取消/失败以
//! `RequestFailed`（关键）收尾。
//!
//! 事件何时被确认持久化：本模块只产出记录，持久化由调用方经存储写者完成；
//! "广播已发出、写入失败"的半态由持久化层处理（contracts/event-schema.md §3）。

use std::collections::VecDeque;
use std::time::Instant;

use whcode_domain::{EventKind, EventRecord, PersistenceLevel, WhError};

use crate::provider::{CancelToken, ChunkStream, LlmRequest, Provider};
use crate::trace::IntegrityFlags;

/// 关键队列容量（有界，ADR-006）。
pub const CRITICAL_CAPACITY: usize = 256;

/// 展示队列默认容量（有界）。
pub const DISPLAY_CAPACITY: usize = 64;

/// 事件优先级（可靠性分级，§4.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPriority {
    /// 关键事件：不丢弃，饱和即背压。
    Critical,
    /// 展示事件：可合并/丢弃，留可查询标记。
    Display,
}

/// 路由结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOutcome {
    /// 已入队。
    Queued,
    /// 为接纳本事件丢弃了 `dropped_now` 条展示事件。
    Merged { dropped_now: usize },
}

/// 双队列事件路由器（关键/展示分离，有界，ADR-006）。
#[derive(Debug)]
pub struct EventRouter {
    critical: VecDeque<EventRecord>,
    display: VecDeque<EventRecord>,
    critical_capacity: usize,
    display_capacity: usize,
    merged_total: u64,
}

impl Default for EventRouter {
    fn default() -> Self {
        Self::new(DISPLAY_CAPACITY)
    }
}

impl EventRouter {
    /// 以指定展示容量构造；关键队列容量固定为 [`CRITICAL_CAPACITY`]。
    #[must_use]
    pub fn new(display_capacity: usize) -> Self {
        Self {
            critical: VecDeque::new(),
            display: VecDeque::new(),
            critical_capacity: CRITICAL_CAPACITY,
            display_capacity,
            merged_total: 0,
        }
    }

    /// 路由一条事件。
    ///
    /// # Errors
    ///
    /// 关键队列饱和返回 `backpressure`（可重试）；展示事件饱和时丢弃最旧并返回
    /// [`RouteOutcome::Merged`]，不报错（§4.2 分级语义）。
    pub fn route(
        &mut self,
        record: EventRecord,
        priority: StreamPriority,
    ) -> Result<RouteOutcome, WhError> {
        match priority {
            StreamPriority::Critical => {
                if self.critical.len() >= self.critical_capacity {
                    return Err(WhError::backpressure(format!(
                        "关键事件队列已满（容量 {CRITICAL_CAPACITY}）：关键事件不得静默丢弃（§4.2、ADR-006）"
                    )));
                }
                self.critical.push_back(record);
                Ok(RouteOutcome::Queued)
            }
            StreamPriority::Display => {
                if self.display_capacity == 0 {
                    return Ok(RouteOutcome::Merged { dropped_now: 1 });
                }
                if self.display.len() >= self.display_capacity {
                    self.display.pop_front();
                    self.merged_total += 1;
                    self.display.push_back(record);
                    return Ok(RouteOutcome::Merged { dropped_now: 1 });
                }
                self.display.push_back(record);
                Ok(RouteOutcome::Queued)
            }
        }
    }

    /// 累计被合并（丢弃）的展示事件数（可观测，Check 57）。
    #[must_use]
    pub const fn merged_total(&self) -> u64 {
        self.merged_total
    }

    /// 取出全部关键事件。
    pub fn drain_critical(&mut self) -> Vec<EventRecord> {
        self.critical.drain(..).collect()
    }

    /// 取出全部展示事件；若发生过合并，末尾追加一条合并标记（可查询，FR-206/208）。
    pub fn drain_display(&mut self, now_ms: u64) -> Vec<EventRecord> {
        let mut events: Vec<EventRecord> = self.display.drain(..).collect();
        if self.merged_total > 0 {
            events.push(EventRecord {
                schema_version: whcode_domain::SCHEMA_VERSION,
                seq: 0,
                id: format!("E-merged-{now_ms}"),
                kind: EventKind::ProgressChanged,
                session_id: None,
                task_id: None,
                request_id: None,
                tool_call_id: None,
                payload: serde_json::json!({
                    "merged_dropped": self.merged_total,
                    "integrity": IntegrityFlags { downsampled: true, ..IntegrityFlags::default() },
                }),
                ts_ms: now_ms,
                persistence_level: PersistenceLevel::Metadata,
            });
        }
        events
    }

    /// 两条队列当前长度（诊断）。
    #[must_use]
    pub fn lens(&self) -> (usize, usize) {
        (self.critical.len(), self.display.len())
    }
}

/// 流编排配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpConfig {
    /// 每 N 个 chunk 发射一个 `ProgressChanged`（合并窗口；0 = 只发首尾）。
    pub progress_every: u64,
    /// 展示队列容量。
    pub display_capacity: usize,
}

impl Default for PumpConfig {
    fn default() -> Self {
        Self {
            progress_every: 4,
            display_capacity: DISPLAY_CAPACITY,
        }
    }
}

/// 流编排结果（事件由调用方持久化）。
#[derive(Debug, Clone)]
pub struct PumpOutcome {
    /// 关键事件（不丢失）。
    pub critical: Vec<EventRecord>,
    /// 展示事件（含合并标记）。
    pub display: Vec<EventRecord>,
    /// 被合并的 chunk 进度数。
    pub merged: u64,
    /// 收到的 chunk 数。
    pub chunks: u64,
    /// 是否被取消终止。
    pub interrupted: bool,
}

fn record(
    kind: EventKind,
    request_id: &str,
    session_id: &str,
    payload: serde_json::Value,
    now_ms: u64,
    level: PersistenceLevel,
) -> EventRecord {
    EventRecord {
        schema_version: whcode_domain::SCHEMA_VERSION,
        seq: 0,
        id: format!("E-{request_id}-{kind:?}-{now_ms}"),
        kind,
        session_id: Some(session_id.to_string()),
        task_id: None,
        request_id: Some(request_id.to_string()),
        tool_call_id: None,
        payload,
        ts_ms: now_ms,
        persistence_level: level,
    }
}

/// 把 Provider 的 chunk 流编排为关键/展示事件（FR-206：关键状态转换不丢失）。
///
/// 取消语义：对桩 Provider，取消在下一个 chunk 前生效；取消后以 `RequestFailed`
/// （载荷 `reason = "cancelled"`）收尾，已收到 chunk 的计数保留。
///
/// # Errors
///
/// 关键队列饱和（背压）或 Provider 错误归一化失败时返回 [`WhError`]。
pub fn pump_stream(
    provider: &dyn Provider,
    request: &LlmRequest,
    cancel: &CancelToken,
    config: &PumpConfig,
    now_ms: u64,
) -> Result<PumpOutcome, WhError> {
    let mut router = EventRouter::new(config.display_capacity);
    let started = Instant::now();

    router.route(
        record(
            EventKind::RequestStarted,
            &request.request_id,
            &request.session_id,
            serde_json::json!({ "model": request.model }),
            now_ms,
            PersistenceLevel::Redacted,
        ),
        StreamPriority::Critical,
    )?;

    let stream: ChunkStream = provider.execute(request, cancel)?;
    let mut chunks = 0u64;
    let mut bytes = 0u64;
    let mut since_progress = 0u64;
    let mut outcome = PumpOutcome {
        critical: Vec::new(),
        display: Vec::new(),
        merged: 0,
        chunks: 0,
        interrupted: false,
    };

    for item in stream {
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(err) => {
                let cancelled = cancel.is_cancelled();
                outcome.interrupted = cancelled;
                let reason = if cancelled {
                    "cancelled"
                } else {
                    "provider_error"
                };
                router.route(
                    record(
                        EventKind::RequestFailed,
                        &request.request_id,
                        &request.session_id,
                        serde_json::json!({
                            "reason": reason,
                            "error": err.message,
                            "code": err.code.as_str(),
                            "chunks_seen": chunks,
                        }),
                        now_ms,
                        PersistenceLevel::Redacted,
                    ),
                    StreamPriority::Critical,
                )?;
                break;
            }
        };
        chunks += 1;
        bytes += chunk.text.chars().count() as u64;
        since_progress += 1;
        if since_progress >= config.progress_every && !chunk.final_chunk {
            since_progress = 0;
            router.route(
                record(
                    EventKind::ProgressChanged,
                    &request.request_id,
                    &request.session_id,
                    serde_json::json!({ "chunks": chunks, "bytes": bytes }),
                    now_ms,
                    PersistenceLevel::Metadata,
                ),
                StreamPriority::Display,
            )?;
        }
        if chunk.final_chunk {
            let latency_ms = started.elapsed().as_millis() as u64;
            // 供应商用量与本地估算必须可区分（FR-207）：`usage_source` 标注来源，
            // `cost_is_estimate` 继续标注估算口径；终止原因随终结 chunk 一起入事件
            // （FR-206／FR-208：`eof` 与 `stop` 可区分）。
            let (usage, usage_source) = match chunk.usage {
                Some(usage) => {
                    let source = if usage.cost_is_estimate {
                        "local_estimate"
                    } else {
                        "provider"
                    };
                    (usage.with_latency(latency_ms), source)
                }
                None => (
                    crate::metering::Usage::estimate(&request.prompt, "").with_latency(latency_ms),
                    "local_estimate",
                ),
            };
            let mut payload = usage.completion_payload();
            payload["usage_source"] = serde_json::json!(usage_source);
            if let Some(reason) = &chunk.finish_reason {
                payload["finish_reason"] = serde_json::json!(reason);
            }
            router.route(
                record(
                    EventKind::RequestCompleted,
                    &request.request_id,
                    &request.session_id,
                    payload,
                    now_ms,
                    PersistenceLevel::Redacted,
                ),
                StreamPriority::Critical,
            )?;
        }
    }

    outcome.chunks = chunks;
    outcome.merged = router.merged_total();
    outcome.critical = router.drain_critical();
    outcome.display = router.drain_display(now_ms);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::StubProvider;
    use whcode_domain::ErrorCode;

    fn request(prompt: &str) -> LlmRequest {
        LlmRequest {
            request_id: "R-1".to_string(),
            session_id: "S-1".to_string(),
            task_id: None,
            model: "stub-model".to_string(),
            prompt: prompt.to_string(),
            max_tokens: 64,
            timeout: None,
        }
    }

    fn display_record(n: u64) -> EventRecord {
        EventRecord {
            schema_version: whcode_domain::SCHEMA_VERSION,
            seq: 0,
            id: format!("E-display-{n}"),
            kind: EventKind::ProgressChanged,
            session_id: Some("S-1".to_string()),
            task_id: None,
            request_id: None,
            tool_call_id: None,
            payload: serde_json::json!({ "n": n }),
            ts_ms: n,
            persistence_level: PersistenceLevel::Metadata,
        }
    }

    #[test]
    fn critical_queue_overflow_is_backpressure_not_silent_drop() {
        let mut router = EventRouter::new(4);
        let critical = record(
            EventKind::RequestStarted,
            "R-1",
            "S-1",
            serde_json::json!({}),
            1,
            PersistenceLevel::Redacted,
        );
        // 关键队列容量固定为 CRITICAL_CAPACITY；用小容量语义不可配置——
        // 直接验证饱和路径：填满后第 CRITICAL_CAPACITY + 1 条必须背压。
        for _ in 0..CRITICAL_CAPACITY {
            router
                .route(critical.clone(), StreamPriority::Critical)
                .expect("容量内必须入队");
        }
        let err = router
            .route(critical, StreamPriority::Critical)
            .expect_err("关键队列饱和必须背压");
        assert_eq!(err.code, ErrorCode::Backpressure);
        assert!(err.retryable, "背压可重试（§4.2）");
        assert_eq!(router.lens().0, CRITICAL_CAPACITY, "关键事件零丢失");
    }

    #[test]
    fn display_overflow_merges_and_leaves_queryable_marker() {
        let mut router = EventRouter::new(4);
        for n in 0..10u64 {
            router
                .route(display_record(n), StreamPriority::Display)
                .expect("展示事件不报错");
        }
        assert_eq!(router.lens().1, 4, "展示队列有界");
        assert_eq!(router.merged_total(), 6, "被丢弃的展示事件必须可观测");

        let drained = router.drain_display(1_000);
        assert_eq!(drained.len(), 5, "4 条展示事件 + 1 条合并标记");
        let marker = drained.last().expect("标记事件");
        assert_eq!(
            marker.payload["merged_dropped"], 6,
            "标记必须可查询（FR-206/208）"
        );
        assert!(marker.payload["integrity"]["downsampled"] == true);
    }

    #[test]
    fn pump_coalesces_progress_and_protects_critical_transitions() {
        let provider = StubProvider::new();
        let config = PumpConfig {
            progress_every: 2,
            display_capacity: 64,
        };
        // 200 字符 / 32 = 7 块：进度事件 ≤ 3，关键事件 2（started + completed）。
        let outcome = pump_stream(
            &provider,
            &request(&"x".repeat(200)),
            &CancelToken::default(),
            &config,
            1_000,
        )
        .expect("编排");

        assert_eq!(outcome.chunks, 7);
        assert_eq!(outcome.critical.len(), 2, "关键状态转换零丢失");
        assert_eq!(outcome.critical[0].kind, EventKind::RequestStarted);
        assert_eq!(outcome.critical[1].kind, EventKind::RequestCompleted);
        assert!(outcome.critical[1].payload["cost_is_estimate"] == true);

        for event in &outcome.display {
            assert!(
                event.payload.get("text").is_none(),
                "进度事件不得携带正文（R-05）：{event:?}"
            );
        }
        let mut merged_seen = false;
        for event in &outcome.display {
            if event.payload.get("merged_dropped").is_some() {
                merged_seen = true;
            }
        }
        if outcome.merged > 0 {
            assert!(merged_seen, "合并发生时必须有标记");
        }
        assert_eq!(
            outcome
                .display
                .iter()
                .filter(|event| event.payload.get("chunks").is_some())
                .count(),
            3,
            "7 块、每 2 块一个进度事件 = 3 个"
        );
    }

    #[test]
    fn pump_reports_cancellation_as_failed_with_partial_count() {
        let provider = StubProvider::new();
        let cancel = CancelToken::default();
        let config = PumpConfig {
            progress_every: 1,
            display_capacity: 64,
        };
        let mut req = request(&"y".repeat(200));
        req.request_id = "R-cancel".to_string();
        let mut stream_probe = provider.execute(&req, &cancel).expect("执行");
        // 手动消费 2 块后取消，再让 pump 走完整流（独立流，模拟取消注入）。
        stream_probe.next();
        stream_probe.next();
        cancel.cancel();
        drop(stream_probe);
        let outcome =
            pump_stream(&provider, &req, &cancel, &config, 1_000).expect("编排（取消路径）");
        assert!(outcome.interrupted, "必须标记取消");
        assert_eq!(outcome.chunks, 0, "取消在下一 chunk 前生效");
        let failed = outcome
            .critical
            .iter()
            .find(|event| event.kind == EventKind::RequestFailed)
            .expect("取消必须以 RequestFailed 收尾");
        assert_eq!(failed.payload["reason"], "cancelled");
    }

    /// 供应商报告用量的 Provider（终结 chunk 携带供应商计量与终止原因）。
    struct ProviderUsageStub;

    impl Provider for ProviderUsageStub {
        fn name(&self) -> &str {
            "provider-usage-stub"
        }

        fn capabilities(&self) -> crate::provider::ProviderCapabilities {
            crate::provider::ProviderCapabilities {
                streaming: true,
                cancel: false,
                usage_reporting: true,
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
            let final_chunk = crate::provider::LlmChunk {
                index: 0,
                text: "终".to_string(),
                final_chunk: true,
                usage: Some(crate::metering::Usage::provider_reported(
                    3, 4, 0, None, false,
                )),
                finish_reason: Some("length".to_string()),
            };
            Ok(ChunkStream {
                inner: Box::new(std::iter::once(Ok(final_chunk))),
            })
        }
    }

    #[test]
    fn completed_event_distinguishes_provider_usage_from_local_estimate() {
        let config = PumpConfig {
            progress_every: 1,
            display_capacity: 8,
        };
        let provider = ProviderUsageStub;
        let outcome = pump_stream(
            &provider,
            &request("hi"),
            &CancelToken::default(),
            &config,
            1_000,
        )
        .expect("编排");
        let completed = outcome
            .critical
            .iter()
            .find(|event| event.kind == EventKind::RequestCompleted)
            .expect("必须有完成事件");
        assert_eq!(completed.payload["usage_source"], "provider");
        assert_eq!(completed.payload["finish_reason"], "length");
        assert!(
            completed.payload["cost_is_estimate"] == false,
            "供应商计量不标估算"
        );

        let stub = StubProvider::new();
        let outcome = pump_stream(
            &stub,
            &request("hi"),
            &CancelToken::default(),
            &config,
            1_000,
        )
        .expect("编排");
        let completed = outcome
            .critical
            .iter()
            .find(|event| event.kind == EventKind::RequestCompleted)
            .expect("必须有完成事件");
        assert_eq!(completed.payload["usage_source"], "local_estimate");
        assert!(completed.payload["cost_is_estimate"] == true);
        assert_eq!(completed.payload["finish_reason"], "stop");
    }
}
