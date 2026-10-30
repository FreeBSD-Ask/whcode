//! # whcode-llm
//!
//! LLM Provider 抽象、流式响应、重试与模型计量（§4.2、§8.4）。
//!
//! 已交付（M3，T056—T064、T132）：
//! - [`provider`]：Provider 语义（能力探测 / 取消 / 流式 / 计量来源 / 超时 / 错误
//!   归一化）与桩实现（T056、FR-201、§8.4）；
//! - [`stream`]：双队列事件路由（关键不丢弃 / 展示可合并并留可查询标记，T057、
//!   FR-206、ADR-006）与 chunk 流编排（进度事件不含正文，R-05）；
//! - [`trace`]：四级追踪与双层脱敏（字段名 + 值模式）及完整性标记（T058/T064、
//!   FR-203/208、D-05）；
//! - [`correlation`]：`request_id/session_id/task_id/tool_call_id` 串联与断链检出
//!   （T059、FR-204）；
//! - [`metering`]：计量来源两级（供应商返回 / 本地估算，估算标记 `cost_is_estimate`，
//!   T063、FR-207）；
//! - [`transport_events`]：传输层进度事件（连接 / 断开 / 重连 / 字节计数 / RTT /
//!   断连原因码，T132、FR-616）。
//!
//! 边界：本 crate 通过事件接口报告状态，**不**直接修改 UI 状态（§6.2）；不做持久化
//! （事件由调用方经存储写者落库）；异步执行必须有取消语义，桩 Provider 的取消在
//! 下一个 chunk 之前生效（§8.4）。

pub mod correlation;
pub mod http;
pub mod metering;
pub mod provider;
pub mod retry;
pub mod stream;
pub mod trace;
pub mod transport_events;

pub use correlation::{Correlation, chain_breaks, link};
pub use metering::{Usage, estimate_tokens};
pub use provider::{
    CancelToken, ChunkStream, LlmChunk, LlmRequest, Provider, ProviderCapabilities, StubProvider,
};
pub use stream::{
    CRITICAL_CAPACITY, DISPLAY_CAPACITY, EventRouter, PumpConfig, PumpOutcome, RouteOutcome,
    StreamPriority, pump_stream,
};
pub use trace::{IntegrityFlags, SUSPICIOUS_FIELDS, SanitizedTrace, sanitize, with_integrity};
pub use transport_events::{MAX_RTT_SAMPLES, TransportSnapshot, TransportState, TransportTracker};
