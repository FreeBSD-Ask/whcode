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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

/// 消息角色（FR-720：system/user/assistant/tool 四角色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// 系统指令。
    System,
    /// 用户输入。
    User,
    /// 模型输出。
    Assistant,
    /// 工具结果（P0-2 接线后出现在多轮请求中；FR-720）。
    Tool,
}

impl Role {
    /// 协议无关的稳定名（五协议映射与事件命名共用）。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// 单条消息（FR-720）。`content` 为纯文本；工具调用结构化字段由 P0-2 扩展。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmMessage {
    /// 角色。
    pub role: Role,
    /// 正文。
    pub content: String,
}

impl LlmMessage {
    /// 便捷构造：user 消息。
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
}

/// 模型声明的工具调用（FR-718/FR-724；P0-1 由 StubProvider 剧本产生，真实协议解析属 P0-2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmToolCall {
    /// 调用 ID（回传 `role: tool` 结果时关联）。
    pub id: String,
    /// 工具名。
    pub name: String,
    /// 参数（JSON 字符串，与供应商线格式一致）。
    pub arguments: String,
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
    /// 消息数组（FR-720：system/user/assistant/tool；非空校验上移到调用方）。
    pub messages: Vec<LlmMessage>,
    /// 最大生成 token 数（估算来源）。
    pub max_tokens: u64,
    /// 超时；`None` 为不限（桩 Provider 不受影响）。
    pub timeout: Option<Duration>,
}

impl LlmRequest {
    /// 全部消息正文的拼接（计量估算与调试用；以换行连接，FR-207 口径）。
    #[must_use]
    pub fn joined_content(&self) -> String {
        self.messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 最后一条 user 消息正文（桩 Provider 回显口径；无 user 消息时回退全拼接）。
    #[must_use]
    pub fn last_user_content(&self) -> String {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_else(|| self.joined_content())
    }
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
    /// 模型声明的工具调用（仅终结 chunk 且终止原因为工具调用时非空；FR-718/FR-724。
    /// 桩 Provider 由剧本产生；真实协议的 tool_calls 帧解析属 P0-2 接线范围）。
    pub tool_calls: Vec<LlmToolCall>,
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
    /// crate 内可见：真实 Provider（如 `http::client::HttpClient`）在 provider 模块之外
    /// 构造流时需要写入自己的迭代器；不新增公开构造函数，公共接口保持不变。
    pub(crate) inner: Box<dyn Iterator<Item = Result<LlmChunk, WhError>> + Send>,
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

/// 桩剧本的一步（T206/FR-718：可编程步序，供 turn 循环测试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubStep {
    /// 本步按序发射的 chunk 文本序列（逐块流式）。
    pub chunks: Vec<String>,
    /// 本步终止原因（`stop`/`tool_call`/`length` 等，随终结 chunk 发射）。
    pub finish_reason: String,
    /// 终止原因为工具调用时的调用声明（仅终结 chunk 携带）。
    pub tool_calls: Vec<LlmToolCall>,
}

impl StubStep {
    /// 便捷构造：单 chunk、给定终止原因、无工具调用。
    #[must_use]
    pub fn simple(text: impl Into<String>, finish_reason: impl Into<String>) -> Self {
        Self {
            chunks: vec![text.into()],
            finish_reason: finish_reason.into(),
            tool_calls: Vec::new(),
        }
    }
}

/// 桩 Provider（quickstart §5：固定速率与 chunk 大小）。
///
/// 行为：无剧本时把**最后一条 user 消息**的正文按 [`StubProvider::CHUNK_SIZE`] 字符切块
/// （单条 user 消息与既有单发行为等价，FR-720）；每个 chunk 附带 `chunk_index`；最后一块
/// 为终结并携带**估算**计量（`cost_is_estimate = true`，FR-207）。取消在下个 chunk 之前
/// 生效；超时按调用起点计算，超限后返回 `timeout` 错误。
///
/// 剧本模式（T206）：`script` 非空时，每次 [`Provider::execute`] 依次消费一步
/// （步内 chunk 序列 + 终止原因 + 可选工具调用声明），步序耗尽后重复最后一步；
/// 步计数跨 `Clone` 共享（`Arc`），供 turn 循环多轮测试编排。
#[derive(Debug, Clone)]
pub struct StubProvider {
    /// 能力声明（可见可改的默认值）。
    pub capabilities: ProviderCapabilities,
    /// 多步剧本（`None` = 默认回显行为，既有测试不破）。
    pub script: Option<Vec<StubStep>>,
    /// 剧本步游标（跨 Clone 共享）。
    step_cursor: Arc<AtomicUsize>,
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
            script: None,
            step_cursor: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 带多步剧本的桩（turn 循环测试用；步序按 `execute` 次数推进）。
    #[must_use]
    pub fn with_script(steps: Vec<StubStep>) -> Self {
        let mut provider = Self::new();
        provider.script = Some(steps);
        provider
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
        if request.messages.is_empty() {
            return Err(WhError::invalid_state("请求消息数组不得为空（FR-720）"));
        }
        if request.model.trim().is_empty() {
            return Err(WhError::invalid_state("模型标识不得为空"));
        }
        if let Some(script) = &self.script {
            if script.is_empty() {
                return Err(WhError::invalid_state("桩剧本不得为空"));
            }
            let index = self
                .step_cursor
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n: usize| {
                    Some((n + 1).min(script.len() - 1))
                })
                .unwrap_or_default();
            let step = script[index].clone();
            return Ok(self.stream_step(&step, &request.joined_content(), request.timeout, cancel));
        }
        let prompt = request.last_user_content();
        let step = StubStep {
            chunks: prompt
                .chars()
                .collect::<Vec<_>>()
                .chunks(Self::CHUNK_SIZE)
                .map(|chunk| chunk.iter().collect())
                .collect(),
            finish_reason: "stop".to_string(),
            tool_calls: Vec::new(),
        };
        Ok(self.stream_step(&step, &prompt, request.timeout, cancel))
    }

    /// 传输层只读快照（FR-616：字节计数与 RTT 可查询）。
    ///
    /// 默认 `None`（桩 Provider 无网络传输）；真实 Provider 返回累计快照。
    fn transport_snapshot(&self) -> Option<TransportSnapshot> {
        None
    }
}

impl StubProvider {
    /// 把剧本一步展开为 chunk 流（取消/超时语义与默认行为一致；超时按调用起点计）。
    fn stream_step(
        &self,
        step: &StubStep,
        estimate_source: &str,
        timeout: Option<Duration>,
        cancel: &CancelToken,
    ) -> ChunkStream {
        let started = Instant::now();
        let total = step.chunks.len() as u64;
        let cancel = cancel.clone();
        let chunks = step.chunks.clone();
        let finish_reason = step.finish_reason.clone();
        let tool_calls = step.tool_calls.clone();
        let estimate_source = estimate_source.to_string();
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
            let step_tool_calls = if final_chunk {
                tool_calls.clone()
            } else {
                Vec::new()
            };
            Ok(LlmChunk {
                index,
                text: chunks[index as usize].clone(),
                final_chunk,
                usage: final_chunk.then(|| Usage::estimate(&estimate_source, &estimate_source)),
                finish_reason: final_chunk.then(|| finish_reason.clone()),
                tool_calls: step_tool_calls,
            })
        });
        ChunkStream {
            inner: Box::new(iter),
        }
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
            messages: vec![LlmMessage::user(prompt)],
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
        assert!(chunks[3].tool_calls.is_empty(), "默认行为无工具调用");
    }

    #[test]
    fn empty_messages_are_rejected() {
        let mut req = request("x");
        req.messages.clear();
        let err = StubProvider::new()
            .execute(&req, &CancelToken::default())
            .expect_err("空消息数组必须拒绝（FR-720）");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn script_mode_plays_steps_in_order_and_holds_last() {
        let provider = StubProvider::with_script(vec![
            StubStep {
                chunks: vec!["调用".to_string(), "工具".to_string()],
                finish_reason: "tool_call".to_string(),
                tool_calls: vec![LlmToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"src/main.rs"}"#.to_string(),
                }],
            },
            StubStep::simple("完成", "stop"),
        ]);
        // 第一轮：剧本第一步，终结 chunk 携带 tool_call 与终止原因。
        let first: Vec<LlmChunk> = provider
            .execute(&request("读文件"), &CancelToken::default())
            .expect("执行")
            .collect::<Result<_, _>>()
            .expect("流式");
        assert_eq!(first.len(), 2);
        assert_eq!(first[1].finish_reason.as_deref(), Some("tool_call"));
        assert_eq!(first[1].tool_calls.len(), 1, "工具调用仅随终结 chunk");
        assert_eq!(first[1].tool_calls[0].name, "read_file");
        assert!(!first[0].final_chunk && first[0].tool_calls.is_empty());
        // 第二轮：剧本第二步（无工具调用）。
        let second: Vec<LlmChunk> = provider
            .execute(&request("读文件"), &CancelToken::default())
            .expect("执行")
            .collect::<Result<_, _>>()
            .expect("流式");
        assert_eq!(second[0].finish_reason.as_deref(), Some("stop"));
        // 第三轮：步序耗尽后重复最后一步（turn 循环超上限测试的确定性基础）。
        let third: Vec<LlmChunk> = provider
            .execute(&request("读文件"), &CancelToken::default())
            .expect("执行")
            .collect::<Result<_, _>>()
            .expect("流式");
        assert_eq!(third[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn script_mode_empty_script_is_rejected() {
        let provider = StubProvider::with_script(Vec::new());
        let err = provider
            .execute(&request("x"), &CancelToken::default())
            .expect_err("空剧本必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
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
    fn empty_model_with_messages_is_rejected() {
        let provider = StubProvider::new();
        let mut req = request("x");
        req.model = String::new();
        let err = provider
            .execute(&req, &CancelToken::default())
            .expect_err("空模型标识必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }
}
