//! # 解码结果归一为 chunk 流（T164、FR-206／FR-207／FR-208）
//!
//! 协议层给出的是**帧语义**（[`DecodeOutcome`]：增量、终止、用量、未知、协议错误），
//! 领域层要的是**chunk 流**（[`LlmChunk`]：序号连续、终止唯一、终止原因与用量随终结
//! chunk 一起给出）。本模块是这两者之间唯一的转换点，承担三条不变量：
//!
//! 1. **序号连续**：每个产出的 chunk 序号从 0 开始逐一递增，不跳号、不重号（FR-208）；
//! 2. **终止唯一**：`final_chunk = true` 只出现一次；终止之后的帧不再产出 chunk
//!    （供应商常见的“用量帧跟在终止帧之后”由待定终止机制吸收，不丢用量）；
//! 3. **不静默**：未知事件计数（FR-206／FR-208）、协议错误事件以 [`PushOutcome::Error`]
//!    交给调用方转结构化错误；流在供应商未显式终止时以 [`EOF_FINISH_REASON`] 收尾，
//!    与正常 `stop` 区分，便于下游发现异常截断。
//!
//! **待定终止**（pending finish）：OpenAI 兼容族把 `finish_reason` 与 `usage` 放在不同帧
//! （含 `include_usage` 时用量帧在最后且没有 `choices`，协议层只能给它合成原因 `stop`）。
//! 因此“无用量但带终止原因”的帧先记为待定，等后续用量帧或流结束再产出终结 chunk：
//! 有真实原因时以先到的真实原因为准，不用合成原因覆盖（FR-206：不丢关键转换）。

use crate::http::protocols::DecodeOutcome;
use crate::metering::Usage;
use crate::provider::LlmChunk;

/// 供应商未显式终止时使用的兜底终止原因（与 `stop` 区分，便于发现截断，FR-208）。
pub const EOF_FINISH_REASON: &str = "eof";

/// 归一结果（未识别帧与协议错误都显式返回，不 panic、不静默丢弃）。
#[derive(Debug, Clone, PartialEq)]
pub enum PushOutcome {
    /// 已产出一个 chunk（可能是终结 chunk）。
    Chunk(LlmChunk),
    /// 空帧、已终止后的多余帧，或已记为待定终止（暂不产出）。
    Idle,
    /// 未识别事件：已计入 [`ChunkNormalizer::unknown_frames`]。
    Unknown,
    /// 协议错误事件（供应商在流中途报错）：调用方必须转为结构化错误，不得当作正常终止。
    Error(String),
}

#[derive(Debug)]
struct PendingFinish {
    text: String,
    reason: String,
}

/// 把 [`DecodeOutcome`] 归一为序号连续的 [`LlmChunk`]。
#[derive(Debug, Default)]
pub struct ChunkNormalizer {
    next_index: u64,
    finished: bool,
    pending: Option<PendingFinish>,
    unknown_frames: u64,
}

impl ChunkNormalizer {
    /// 空归一器（序号从 0 开始）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 下一个将产出的 chunk 序号（诊断与序号连续性断言用）。
    #[must_use]
    pub const fn next_index(&self) -> u64 {
        self.next_index
    }

    /// 已累计的未识别帧数（FR-206／FR-208：必须留痕）。
    #[must_use]
    pub const fn unknown_frames(&self) -> u64 {
        self.unknown_frames
    }

    /// 是否已产出终结 chunk。
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// 归一一个解码结果。
    pub fn push(&mut self, outcome: DecodeOutcome) -> PushOutcome {
        if self.finished {
            // 终止之后不再产出：重复的终止帧或尾随正文都不改变已产出的 chunk 序号。
            return PushOutcome::Idle;
        }
        match outcome {
            DecodeOutcome::Empty => PushOutcome::Idle,
            DecodeOutcome::Unknown => {
                self.unknown_frames = self.unknown_frames.saturating_add(1);
                PushOutcome::Unknown
            }
            DecodeOutcome::Error { message } => PushOutcome::Error(message),
            DecodeOutcome::Delta { text } => {
                if text.is_empty() {
                    return PushOutcome::Idle;
                }
                match self.pending.as_mut() {
                    // 终止之后的增量并入待定正文，保证不丢文本且不违反“终止唯一”。
                    Some(pending) => {
                        pending.text.push_str(&text);
                        PushOutcome::Idle
                    }
                    None => PushOutcome::Chunk(self.emit(text, false, None, None)),
                }
            }
            DecodeOutcome::Finish { reason, usage } => match self.pending.take() {
                Some(pending) => {
                    PushOutcome::Chunk(self.emit(pending.text, true, usage, Some(pending.reason)))
                }
                None => PushOutcome::Chunk(self.emit(String::new(), true, usage, Some(reason))),
            },
            DecodeOutcome::DeltaAndFinish {
                text,
                reason,
                usage,
            } => {
                if usage.is_some() {
                    let (text, reason) = match self.pending.take() {
                        Some(pending) => {
                            let mut merged = pending.text;
                            merged.push_str(&text);
                            (merged, pending.reason)
                        }
                        None => (text, reason),
                    };
                    return PushOutcome::Chunk(self.emit(text, true, usage, Some(reason)));
                }
                match self.pending.as_mut() {
                    Some(pending) => {
                        pending.text.push_str(&text);
                        PushOutcome::Idle
                    }
                    None => {
                        self.pending = Some(PendingFinish { text, reason });
                        PushOutcome::Idle
                    }
                }
            }
        }
    }

    /// 流结束（EOF）时的收尾：把待定终止或无终止的流补成唯一终结 chunk。
    ///
    /// 供应商已给出原因时保留该原因；完全没有终止帧时使用 [`EOF_FINISH_REASON`]。
    /// 已终止则返回 `None`。
    pub fn finish(&mut self, fallback_reason: &str) -> Option<LlmChunk> {
        if self.finished {
            return None;
        }
        let (text, reason) = match self.pending.take() {
            Some(pending) => (pending.text, pending.reason),
            None => (String::new(), fallback_reason.to_string()),
        };
        Some(self.emit(text, true, None, Some(reason)))
    }

    fn emit(
        &mut self,
        text: String,
        final_chunk: bool,
        usage: Option<Usage>,
        finish_reason: Option<String>,
    ) -> LlmChunk {
        let index = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        if final_chunk {
            self.finished = true;
        }
        LlmChunk {
            index,
            text,
            final_chunk,
            usage,
            finish_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(text: &str) -> DecodeOutcome {
        DecodeOutcome::Delta {
            text: text.to_string(),
        }
    }

    #[test]
    fn indices_are_contiguous_and_only_last_chunk_is_final() {
        let mut normalizer = ChunkNormalizer::new();
        let mut chunks = Vec::new();
        for text in ["a", "b", "c"] {
            match normalizer.push(delta(text)) {
                PushOutcome::Chunk(chunk) => chunks.push(chunk),
                other => panic!("增量必须产出 chunk：{other:?}"),
            }
        }
        match normalizer.push(DecodeOutcome::Finish {
            reason: "stop".to_string(),
            usage: None,
        }) {
            PushOutcome::Chunk(chunk) => chunks.push(chunk),
            other => panic!("终止必须产出终结 chunk：{other:?}"),
        }

        let indices: Vec<u64> = chunks.iter().map(|chunk| chunk.index).collect();
        assert_eq!(indices, vec![0, 1, 2, 3], "序号必须连续（FR-208）");
        assert_eq!(
            chunks.iter().filter(|chunk| chunk.final_chunk).count(),
            1,
            "终结 chunk 必须唯一"
        );
        let last = chunks.last().expect("终结 chunk");
        assert!(last.final_chunk);
        assert_eq!(last.finish_reason.as_deref(), Some("stop"));
        assert!(normalizer.is_finished());
    }

    #[test]
    fn usage_only_frame_after_finish_keeps_provider_usage_and_real_reason() {
        let mut normalizer = ChunkNormalizer::new();
        let _ = normalizer.push(delta("你好"));
        // OpenAI 兼容：finish_reason 帧不带 usage，只有帧内无 choices 时才带 usage。
        assert_eq!(
            normalizer.push(DecodeOutcome::DeltaAndFinish {
                text: String::new(),
                reason: "length".to_string(),
                usage: None,
            }),
            PushOutcome::Idle,
            "无用量时先记为待定终止"
        );
        let usage = Usage::provider_reported(10, 5, 0, None, false);
        let final_chunk = match normalizer.push(DecodeOutcome::Finish {
            reason: "stop".to_string(),
            usage: Some(usage),
        }) {
            PushOutcome::Chunk(chunk) => chunk,
            other => panic!("用量帧必须产出终结 chunk：{other:?}"),
        };
        assert!(final_chunk.final_chunk);
        assert_eq!(
            final_chunk.finish_reason.as_deref(),
            Some("length"),
            "已有真实原因时不得被合成原因 stop 覆盖"
        );
        let reported = final_chunk.usage.expect("供应商用量必须保留");
        assert_eq!(
            (reported.prompt_tokens, reported.completion_tokens),
            (10, 5)
        );
        assert!(!reported.cost_is_estimate, "供应商计量与本地估算可区分");
        assert_eq!(final_chunk.index, 1);
    }

    #[test]
    fn pending_finish_is_flushed_at_eof_with_supplier_reason() {
        let mut normalizer = ChunkNormalizer::new();
        let _ = normalizer.push(delta("尾部"));
        let _ = normalizer.push(DecodeOutcome::DeltaAndFinish {
            text: "末帧".to_string(),
            reason: "end_turn".to_string(),
            usage: None,
        });
        let final_chunk = normalizer.finish(EOF_FINISH_REASON).expect("EOF 必须收尾");
        assert!(final_chunk.final_chunk);
        assert_eq!(final_chunk.index, 1);
        assert_eq!(final_chunk.text, "末帧", "待定正文必须随终结 chunk 给出");
        assert_eq!(
            final_chunk.finish_reason.as_deref(),
            Some("end_turn"),
            "待定终止保留供应商原因"
        );
        assert!(final_chunk.usage.is_none(), "无供应商用量时走本地估算");
        assert!(normalizer.finish(EOF_FINISH_REASON).is_none());
    }

    #[test]
    fn unknown_frames_are_counted_and_errors_surface() {
        let mut normalizer = ChunkNormalizer::new();
        assert_eq!(
            normalizer.push(DecodeOutcome::Unknown),
            PushOutcome::Unknown
        );
        assert_eq!(normalizer.push(DecodeOutcome::Empty), PushOutcome::Idle);
        assert_eq!(normalizer.unknown_frames(), 1, "未知帧必须计数留痕");
        assert_eq!(
            normalizer.push(DecodeOutcome::Error {
                message: "配额超限".to_string()
            }),
            PushOutcome::Error("配额超限".to_string()),
            "协议错误不得静默"
        );
        assert!(!normalizer.is_finished(), "错误不产出终结 chunk");
    }

    #[test]
    fn frames_after_finish_are_ignored_without_breaking_indices() {
        let mut normalizer = ChunkNormalizer::new();
        let first = match normalizer.push(DecodeOutcome::Finish {
            reason: "stop".to_string(),
            usage: None,
        }) {
            PushOutcome::Chunk(chunk) => chunk,
            other => panic!("{other:?}"),
        };
        assert_eq!(first.index, 0);
        assert_eq!(normalizer.push(delta("多余正文")), PushOutcome::Idle);
        assert_eq!(normalizer.push(DecodeOutcome::Unknown), PushOutcome::Idle);
        assert_eq!(normalizer.unknown_frames(), 0, "终止后的帧不再计数为未知");
    }
}
