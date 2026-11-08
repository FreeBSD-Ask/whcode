//! # 压缩摘要（T265／FR-755／ADR-060 项 B）
//!
//! 上下文超预算时**先**用模型生成一次任务状态摘要，摘要不可用再回退到确定性结构折叠
//! （[`crate::context::fold_messages_for_budget`]）。契约要点：
//!
//! - **独立请求身份**：摘要调用有自己的 `request_id`（`<turn request_id>-cmp-<iteration>`）
//!   与 `purpose=compaction`（[`COMPACTION_PURPOSE`]，随三事件载荷落表）；
//! - **显式预算**：摘要调用固定 [`COMPACTION_SUMMARY_MAX_TOKENS`]，不借用 turn 的生成预算
//!   （单次摘要成本有上界）；
//! - **有界**：送入摘要的转写与取回的摘要正文都有字符上限，超限截断并标 `truncated`；
//!   中途超限只停止累积、**不**取消共享令牌（取消是 turn 级语义，摘要超限不得终止 turn）；
//! - **失败即回退**：只尝试一次，不重试（避免与 ADR-050 重试叠加产生重复成本），
//!   失败不阻塞 turn，由调用方回退结构折叠；
//! - **共用取消**：与 turn 共用 [`CancelToken`]，取消在下一个 chunk 前生效（§8.4 语义）。
//!
//! 边界：本模块只负责“取摘要”与“装配候选消息数组”；事件发射与落表顺序（先开后关）在
//! `turn.rs` 的预算分支，孤儿锁检测在 `whcode_store::turn::orphan_compactions`。

use std::time::{Duration, Instant};

use whcode_domain::{ErrorCode, WhError};
use whcode_llm::provider::{LlmMessage, Role};
use whcode_llm::{CancelToken, LlmRequest, Provider, Usage};

use crate::llm::{MAX_OUTPUT_BYTES, MAX_STREAM_CHUNKS};

/// 摘要调用的用途标识（事件载荷 `purpose` 取值；供应商侧不分支，只作审计与计量口径）。
pub const COMPACTION_PURPOSE: &str = "compaction";

/// 摘要调用的生成上限（token；显式设定，独立于 turn 的 `max_tokens`）。
pub const COMPACTION_SUMMARY_MAX_TOKENS: u64 = 1024;

/// 送入摘要请求的转写字符上限（有界输入：摘要成本随会话增长不得无界）。
pub const MAX_COMPACTION_INPUT_CHARS: usize = 48_000;

/// 摘要正文保留字符上限（事件载荷有界；超限截断并标 `truncated`）。
pub const MAX_COMPACTION_SUMMARY_CHARS: usize = 8_192;

/// 摘要正文下限：低于此值视为失败（过短的摘要不能替代原文，属“错误的空”）。
pub const MIN_COMPACTION_SUMMARY_CHARS: usize = 8;

/// 摘要请求 ID 后缀（与 turn 的 `request_id` 区分，便于按链路回溯）。
pub const COMPACTION_REQUEST_ID_SUFFIX: &str = "-cmp";

/// 摘要指令（system 消息；只要求压缩，不引入新的任务语义）。
pub const SUMMARY_INSTRUCTION: &str = "你是上下文压缩器。把用户消息中的对话转写压缩为任务状态摘要，\
只输出摘要正文，不要寒暄或解释：保留已确认的事实与决定、未完成事项、涉及的文件路径与关键错误；\
不得编造转写中未出现的内容；不超过 400 字。";

/// 超限输入的省略标记（标明中段未送入摘要，原文仍在事件表中）。
const OMITTED_MARKER: &str = "\n…（中段未送入摘要；原文仍在事件与消息历史中可查）\n";

/// 摘要请求 ID（`<turn request_id>-cmp-<iteration>`）：同一 turn 的多次压缩不共享身份。
#[must_use]
pub fn compaction_request_id(turn_request_id: &str, iteration: u32) -> String {
    format!("{turn_request_id}{COMPACTION_REQUEST_ID_SUFFIX}-{iteration}")
}

/// 一次摘要调用的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionSummary {
    /// 摘要请求 ID（与 `CompactionStarted`/`CompactionFinished` 载荷一致）。
    pub request_id: String,
    /// 摘要正文（已按 [`MAX_COMPACTION_SUMMARY_CHARS`] 截断）。
    pub text: String,
    /// 摘要正文是否因上限被截断。
    pub truncated: bool,
    /// 送入摘要的转写是否因上限被截断。
    pub input_truncated: bool,
    /// 计量：供应商上报优先，缺失时本地估算（`cost_is_estimate = true`，FR-207）。
    pub usage: Usage,
}

/// 把消息数组压成有界转写（头部 1/3、尾部 2/3，中段省略并留标记）。
///
/// 返回 `(转写, 是否截断)`。头部保留任务锚点与早期决定，尾部保留最近状态；两侧都按
/// 字符边界截断，不做字节切片（中文不得切出半个字符）。
#[must_use]
pub fn bounded_transcript(messages: &[LlmMessage], max_chars: usize) -> (String, bool) {
    let lines: Vec<String> = messages
        .iter()
        .map(|message| format!("[{}] {}", message.role.as_str(), message.content))
        .collect();
    let total: usize = lines.iter().map(|line| line.chars().count() + 1).sum();
    if total <= max_chars {
        return (lines.join("\n"), false);
    }
    let head_budget = max_chars / 3;
    let tail_budget = max_chars.saturating_sub(head_budget);
    let mut head = String::new();
    let mut used = 0usize;
    for line in &lines {
        let cost = line.chars().count() + 1;
        if used + cost > head_budget {
            break;
        }
        if !head.is_empty() {
            head.push('\n');
        }
        head.push_str(line);
        used += cost;
    }
    let mut tail_lines: Vec<&str> = Vec::new();
    let mut tail_used = 0usize;
    for line in lines.iter().rev() {
        let cost = line.chars().count() + 1;
        if tail_used + cost > tail_budget {
            break;
        }
        tail_lines.push(line.as_str());
        tail_used += cost;
    }
    tail_lines.reverse();
    let tail = tail_lines.join("\n");
    let transcript = format!("{head}{OMITTED_MARKER}{tail}");
    (transcript, true)
}

/// 用模型生成一次压缩摘要。
///
/// # Errors
///
/// - 供应商不可用、请求未发出、流中断、未收到终结块、摘要过短 → 返回归一化错误；
///   调用方据此回退结构折叠（**不得**把失败当成功，也不得让 turn 失败）；
/// - 取消 → 返回 `internal`（消息说明取消在下一个 chunk 前生效）。
pub fn summarize(
    provider: &dyn Provider,
    cancel: &CancelToken,
    request_id: &str,
    session: &str,
    model: &str,
    messages: &[LlmMessage],
    timeout: Option<Duration>,
) -> Result<CompactionSummary, WhError> {
    let (transcript, input_truncated) = bounded_transcript(messages, MAX_COMPACTION_INPUT_CHARS);
    let request = LlmRequest {
        request_id: request_id.to_string(),
        session_id: session.to_string(),
        task_id: None,
        model: model.to_string(),
        messages: vec![
            LlmMessage {
                role: Role::System,
                content: SUMMARY_INSTRUCTION.to_string(),
            },
            LlmMessage::user(transcript.clone()),
        ],
        max_tokens: COMPACTION_SUMMARY_MAX_TOKENS,
        timeout,
        tools: None,
    };
    let started = Instant::now();
    let stream = provider.execute(&request, cancel)?;

    let mut text = String::new();
    let mut bytes = 0usize;
    let mut collected = 0usize;
    let mut completed = false;
    let mut provider_usage = None;
    let mut truncated = false;
    for item in stream {
        if cancel.is_cancelled() {
            return Err(WhError::new(
                ErrorCode::Internal,
                "压缩摘要调用已取消：Provider 在下一个 chunk 前停止（回退结构折叠）".to_string(),
            ));
        }
        let chunk = item?;
        collected += 1;
        // 超限只停止累积，不 cancel：取消令牌是 turn 级共享语义，摘要超限不得终止 turn。
        if collected > MAX_STREAM_CHUNKS
            || bytes.saturating_add(chunk.text.len()) > MAX_OUTPUT_BYTES
        {
            truncated = true;
            break;
        }
        bytes = bytes.saturating_add(chunk.text.len());
        text.push_str(&chunk.text);
        if chunk.final_chunk {
            completed = true;
            provider_usage = chunk.usage;
            break;
        }
    }
    if !completed {
        return Err(WhError::new(
            ErrorCode::Internal,
            format!("压缩摘要流未收到终结块（已收 {collected} 块、{bytes} 字节）：回退结构折叠"),
        ));
    }
    let chars = text.chars().count();
    if chars < MIN_COMPACTION_SUMMARY_CHARS {
        return Err(WhError::invalid_state(format!(
            "压缩摘要过短（{chars} 字符 < 下限 {MIN_COMPACTION_SUMMARY_CHARS}）：回退结构折叠"
        )));
    }
    if chars > MAX_COMPACTION_SUMMARY_CHARS {
        text = text.chars().take(MAX_COMPACTION_SUMMARY_CHARS).collect();
        text.push('…');
        truncated = true;
    }
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let usage = provider_usage.map_or_else(
        || Usage::estimate(&transcript, &text).with_latency(latency_ms),
        |usage| {
            if usage.latency_ms > 0 {
                usage
            } else {
                usage.with_latency(latency_ms)
            }
        },
    );
    Ok(CompactionSummary {
        request_id: request_id.to_string(),
        text,
        truncated,
        input_truncated,
        usage,
    })
}

/// 用摘要装配候选消息数组：保留任务锚点（最早一条 user 消息）及其之前的前缀，中段折叠为
/// 一条 user 角色摘要。
///
/// 返回 `(候选数组, 被折叠消息数)`；无锚点或中段为空时返回 `None`（调用方回退结构折叠——
/// 摘要在无可折叠中段时没有可替换的对象）。
#[must_use]
pub fn messages_with_summary(
    messages: &[LlmMessage],
    summary: &str,
) -> Option<(Vec<LlmMessage>, usize)> {
    let anchor_index = messages
        .iter()
        .position(|message| message.role == Role::User)?;
    let fold_start = anchor_index + 1;
    let folded = messages.len().checked_sub(fold_start)?;
    if folded == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(fold_start + 1);
    out.extend(messages[..fold_start].iter().cloned());
    out.push(LlmMessage {
        role: Role::User,
        content: format!(
            "[上下文摘要｜模型生成] {summary}\n（被折叠 {folded} 条消息的原文仍在事件与消息历史中可查）"
        ),
    });
    Some((out, folded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_llm::provider::StubStep;
    use whcode_llm::{StubProvider, provider::ChunkStream};

    /// 记录最近一次请求的桩：委托 [`StubProvider`] 的剧本行为，只额外留一份请求副本
    /// （`ChunkStream` 不对外构造，故用委托而不是自建流）。
    struct RecordingProvider {
        seen: std::sync::Mutex<Option<LlmRequest>>,
        script: StubProvider,
    }

    impl RecordingProvider {
        fn new(script: StubProvider) -> Self {
            Self {
                seen: std::sync::Mutex::new(None),
                script,
            }
        }

        fn last_request(&self) -> LlmRequest {
            self.seen
                .lock()
                .expect("请求记录锁")
                .clone()
                .expect("应已记录请求")
        }
    }

    impl Provider for RecordingProvider {
        fn name(&self) -> &str {
            "recording"
        }

        fn capabilities(&self) -> whcode_llm::ProviderCapabilities {
            whcode_llm::ProviderCapabilities {
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
            request: &LlmRequest,
            cancel: &CancelToken,
        ) -> Result<ChunkStream, WhError> {
            *self.seen.lock().expect("请求记录锁") = Some(request.clone());
            self.script.execute(request, cancel)
        }
    }

    fn messages() -> Vec<LlmMessage> {
        vec![
            LlmMessage::user("任务锚点：修复构建"),
            LlmMessage {
                role: Role::Assistant,
                content: "先看日志".to_string(),
            },
            LlmMessage {
                role: Role::Tool,
                content: "构建失败：缺少依赖".to_string(),
            },
        ]
    }

    #[test]
    fn summary_request_has_own_identity_purpose_and_explicit_budget() {
        let provider = RecordingProvider::new(StubProvider::new());
        let cancel = CancelToken::default();
        let id = compaction_request_id("R-turn-1", 2);
        let summary = summarize(
            &provider,
            &cancel,
            &id,
            "S-1",
            "test-model",
            &messages(),
            None,
        )
        .expect("摘要成功");
        assert_eq!(summary.request_id, "R-turn-1-cmp-2");
        assert_eq!(COMPACTION_PURPOSE, "compaction");
        let request = provider.last_request();
        assert_eq!(request.request_id, "R-turn-1-cmp-2");
        assert_eq!(request.max_tokens, COMPACTION_SUMMARY_MAX_TOKENS);
        assert!(request.tools.is_none(), "摘要调用不带工具清单");
        assert_eq!(request.messages[0].role, Role::System);
        assert_eq!(request.messages[0].content, SUMMARY_INSTRUCTION);
        // 转写保留角色标记，模型可区分事实来源。
        assert!(request.messages[1].content.starts_with("[user] 任务锚点"));
        assert!(!summary.truncated);
        assert!(!summary.input_truncated);
    }

    #[test]
    fn oversized_summary_text_is_truncated_and_flagged() {
        let long = "字".repeat(MAX_COMPACTION_SUMMARY_CHARS + 500);
        let provider = RecordingProvider::new(StubProvider::with_script(vec![StubStep::simple(
            long, "stop",
        )]));
        let summary = summarize(
            &provider,
            &CancelToken::default(),
            "R-cmp",
            "S-1",
            "m",
            &messages(),
            None,
        )
        .expect("摘要成功");
        assert!(summary.truncated, "超限必须标记");
        assert_eq!(
            summary.text.chars().count(),
            MAX_COMPACTION_SUMMARY_CHARS + 1,
            "保留上限内的字符并追加省略号"
        );
    }

    #[test]
    fn oversized_input_transcript_is_bounded_with_marker() {
        let big = "x".repeat(MAX_COMPACTION_INPUT_CHARS * 2);
        let conversation = vec![
            LlmMessage::user("锚点"),
            LlmMessage {
                role: Role::Assistant,
                content: big,
            },
            LlmMessage::user("最近的追问"),
        ];
        let provider = RecordingProvider::new(StubProvider::new());
        let summary = summarize(
            &provider,
            &CancelToken::default(),
            "R-cmp",
            "S-1",
            "m",
            &conversation,
            None,
        )
        .expect("摘要成功");
        assert!(summary.input_truncated, "输入超限必须标记");
        let transcript = &provider.last_request().messages[1].content;
        assert!(
            transcript.chars().count() <= MAX_COMPACTION_INPUT_CHARS + OMITTED_MARKER.len() + 2,
            "转写必须有界：{}",
            transcript.chars().count()
        );
        assert!(transcript.contains("中段未送入摘要"), "截断必须留可查标记");
        assert!(
            transcript.ends_with("[user] 最近的追问"),
            "尾部应保留最近消息"
        );
    }

    #[test]
    fn empty_or_unfinished_summary_is_a_failure_not_an_empty_success() {
        // 空剧本步：流里没有终结块 → 失败（回退），不得当成“空摘要成功”。
        let provider = RecordingProvider::new(StubProvider::with_script(vec![StubStep {
            chunks: Vec::new(),
            finish_reason: "stop".to_string(),
            tool_calls: Vec::new(),
        }]));
        let err = summarize(
            &provider,
            &CancelToken::default(),
            "R-cmp",
            "S-1",
            "m",
            &messages(),
            None,
        )
        .expect_err("空摘要必须失败");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(err.message.contains("回退结构折叠"));
    }

    #[test]
    fn too_short_summary_is_rejected() {
        let provider = RecordingProvider::new(StubProvider::with_script(vec![StubStep::simple(
            "好", "stop",
        )]));
        let err = summarize(
            &provider,
            &CancelToken::default(),
            "R-cmp",
            "S-1",
            "m",
            &messages(),
            None,
        )
        .expect_err("过短摘要必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn cancelled_summary_reports_cancellation_without_panicking() {
        let provider = RecordingProvider::new(StubProvider::new());
        let cancel = CancelToken::default();
        cancel.cancel();
        let err = summarize(&provider, &cancel, "R-cmp", "S-1", "m", &messages(), None)
            .expect_err("已取消必须失败");
        assert!(err.message.contains("已取消"));
    }

    #[test]
    fn messages_with_summary_keeps_anchor_and_replaces_middle() {
        let (candidate, folded) = messages_with_summary(&messages(), "摘要正文").expect("可折叠");
        assert_eq!(folded, 2);
        assert_eq!(candidate.len(), 2);
        assert_eq!(candidate[0].content, "任务锚点：修复构建");
        assert_eq!(candidate[1].role, Role::User);
        assert!(candidate[1].content.contains("摘要正文"));
        assert!(
            candidate[1]
                .content
                .contains("原文仍在事件与消息历史中可查")
        );
    }

    #[test]
    fn messages_with_summary_returns_none_without_foldable_middle() {
        assert!(messages_with_summary(&[LlmMessage::user("唯一消息")], "摘要").is_none());
        assert!(messages_with_summary(&messages()[..1], "摘要").is_none());
        // 没有任何 user 消息（无锚点）→ 无折叠边界。
        assert!(
            messages_with_summary(
                &[LlmMessage {
                    role: Role::Assistant,
                    content: "只有助手消息".to_string()
                }],
                "摘要"
            )
            .is_none()
        );
    }
}
