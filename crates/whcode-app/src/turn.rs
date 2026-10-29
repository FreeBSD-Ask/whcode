//! # Agent turn 循环编排器（T207—T209、FR-718—723、ADR-044）
//!
//! 把 `llm.rs` 的单次模型调用升级为**多轮 turn 循环**：中止/预算检查 → 消息组装 →
//! 模型 step → 终止判定 →（工具执行回填为 P0-2 接线点，本模块按 FR-724 记录
//! `unknown` 并显式终止，T216 接线后替换）。
//!
//! 边界与取舍（承接 `llm.rs` 既有口径）：
//! - **同步、无线程**：turn 占用调用线程，不新增线程/异步运行时（NFR-216）；
//! - **有界**：迭代上限 [`MAX_TURN_ITERATIONS`]（FR-719，请求只可调低）、每步
//!   chunk/正文上限沿用 `llm.rs` 常量、事件与消息落表走 [`whcode_store::append_turn`]
//!   （同事务、超限拒绝，不静默丢）；
//! - **事实源**：turn 关键事件与 user/assistant/tool 消息轮次落 `events`/`messages`
//!   表（FR-722，payload 只带标识与计数）；信封 `events` 与落表事件**同名同 payload**
//!   （`EventKind` 稳定名，命名收敛）；每个事件批次恰好落表一次，不重放；
//! - **取消跨进程**：每轮迭代前与每个 chunk 前检查 `.cancel` 标记（FR-723，"下一
//!   chunk 边界生效"语义不变）；单会话至多一个活跃 turn（`turn-<session>.live` RAII）；
//! - **不伪造**：五种终止（model_finished/iteration_limit/cancelled/budget_exceeded/
//!   tool_not_wired）全部以 [`EventKind::RuntimeTerminated`] 事件显式落表（FR-718）；
//!   截断/流中断按 `error` 终止并保留 `truncated`/`integrity_gaps` 证据（FR-208）。

use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};
use whcode_domain::{ErrorCode, EventKind, WhError};
use whcode_llm::provider::{LlmMessage, LlmToolCall, Role};
use whcode_llm::{CancelToken, LlmRequest, Usage};
use whcode_store::{TurnEventDraft, TurnMessageDraft, append_turn};

use crate::authz::{Actor, Capability, Policy, require};
use crate::context::load_context;
use crate::llm::{
    CancelMarker, LlmChunkView, LlmContext, MAX_OUTPUT_BYTES, MAX_STREAM_CHUNKS,
    generate_request_id, validate_request_id,
};
use crate::session::show_session;

use std::path::Path;

/// turn 迭代硬上限（FR-719；请求可用 `iterations_limit` 调低，不可调高）。
pub const MAX_TURN_ITERATIONS: u32 = 16;
/// 默认上下文预算（token；FR-105 契约口径）。
pub const DEFAULT_TURN_BUDGET_TOKENS: usize = 128_000;
/// 活跃 turn 标记后缀（`turn-<session>.live`；FR-723）。
const TURN_LIVE_SUFFIX: &str = ".live";

/// 终止原因稳定名（FR-718／plan-p0-1 §2.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationReason {
    /// 模型返回无工具调用（正常完成）。
    ModelFinished,
    /// 达到迭代上限。
    IterationLimit,
    /// 用户取消。
    Cancelled,
    /// 上下文预算超限。
    BudgetExceeded,
    /// 工具执行未接线（P0-1 过渡行为，FR-724；T216 接线后替换）。
    ToolNotWired,
    /// 供应商错误、截断或流完整性缺口（FR-208 证据随事件保留）。
    Error,
}

impl TerminationReason {
    /// 稳定名（事件 payload 与信封共用）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TerminationReason::ModelFinished => "model_finished",
            TerminationReason::IterationLimit => "iteration_limit",
            TerminationReason::Cancelled => "cancelled",
            TerminationReason::BudgetExceeded => "budget_exceeded",
            TerminationReason::ToolNotWired => "tool_not_wired",
            TerminationReason::Error => "error",
        }
    }
}

/// `llm turn` 的请求（`llm run` 字段 + turn 专属可调项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRequest {
    /// 会话 ID（必须先存在）。
    pub session_id: String,
    /// 剖面名（用户显式选择）。
    pub provider: String,
    /// 提示词正文（本轮 user 消息；不进入事件）。
    pub prompt: String,
    /// 模型覆盖（`None` 用剖面值）。
    pub model: Option<String>,
    /// `max_tokens` 覆盖（`None` 用剖面值）。
    pub max_tokens: Option<u64>,
    /// 预置请求 ID（贯穿 turn 内全部模型 step；`None` 自动生成）。
    pub request_id: Option<String>,
    /// 迭代上限（只可调低 [`MAX_TURN_ITERATIONS`]）。
    pub iterations_limit: Option<u32>,
    /// 上下文预算（token；`None` 用 [`DEFAULT_TURN_BUDGET_TOKENS`]）。
    pub budget_tokens: Option<usize>,
}

/// turn 的结果（信封；`LlmRunOutcome` 的 turn 扩展，四端适配属 T210）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnOutcome {
    /// 信封 schema 版本。
    pub schema_version: u32,
    /// turn ID（会话内单调，从库内事件状态派生、不依赖时钟）。
    pub turn_id: String,
    /// 请求 ID（贯穿 turn 内全部模型 step）。
    pub request_id: String,
    /// 剖面名。
    pub provider: String,
    /// 线协议稳定标识。
    pub protocol: String,
    /// 实际使用的模型。
    pub model: String,
    /// 实际执行的模型 step 数。
    pub iterations: u32,
    /// 终止原因稳定名。
    pub termination_reason: String,
    /// 末轮 chunk 视图（只带计数）。
    pub chunks: Vec<LlmChunkView>,
    /// 末轮拼接正文。
    pub text: String,
    /// 末轮供应商终止原因。
    pub finish_reason: Option<String>,
    /// 末轮是否读到终结 chunk。
    pub completed: bool,
    /// 末轮是否因上限截断。
    pub truncated: bool,
    /// 末轮 chunk 序号不连续次数。
    pub integrity_gaps: u64,
    /// 末轮计量。
    pub usage: Option<Usage>,
    /// 有界事件序列（稳定名；与落表事件同名同 payload）。
    pub events: Vec<Value>,
    /// 传输只读快照（真实 Provider 提供）。
    pub transport: Option<whcode_llm::TransportSnapshot>,
}

/// 活跃 turn 标记（RAII）：`turn-<session>.live` 存在即拒绝第二个 turn（FR-723）。
struct TurnMarker {
    dir: std::path::PathBuf,
    session: String,
}

impl TurnMarker {
    fn begin(dir: &Path, session: &str) -> Result<Self, WhError> {
        std::fs::create_dir_all(dir).map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!("turn 标记目录不可用（{}）：{err}", dir.display()),
            )
        })?;
        let live = dir.join(format!("turn-{session}{TURN_LIVE_SUFFIX}"));
        if live.exists() {
            return Err(WhError::invalid_state(format!(
                "会话 {session} 已有活跃 turn：单会话同进程至多一个活跃 turn（FR-723）；\
                 残留标记请先 llm cancel 或清理 {}",
                live.display()
            )));
        }
        std::fs::write(&live, b"turn").map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!("写入活跃 turn 标记失败（{}）：{err}", live.display()),
            )
        })?;
        Ok(Self {
            dir: dir.to_path_buf(),
            session: session.to_string(),
        })
    }
}

impl Drop for TurnMarker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(
            self.dir
                .join(format!("turn-{}{TURN_LIVE_SUFFIX}", self.session)),
        );
    }
}

/// turn 落表器：把待写事件与尚未落表的消息后缀在单事务内追加（增量、不重放）。
struct TurnPersister {
    session: String,
    /// messages 中已落表的前缀长度。
    persisted: usize,
}

impl TurnPersister {
    fn flush(
        &mut self,
        conn: &mut duckdb::Connection,
        pending_events: &mut Vec<TurnEventDraft>,
        messages: &[LlmMessage],
    ) -> Result<(), WhError> {
        let message_drafts: Vec<TurnMessageDraft> = messages[self.persisted..]
            .iter()
            .map(|m| TurnMessageDraft {
                role: match m.role {
                    Role::Tool => "tool",
                    Role::Assistant => "assistant",
                    Role::User => "user",
                    Role::System => "system",
                },
                content: m.content.clone(),
                token_count: None,
                parent_id: None,
            })
            .collect();
        append_turn(conn, &self.session, pending_events, &message_drafts)?;
        self.persisted = messages.len();
        pending_events.clear();
        Ok(())
    }
}

/// 执行一轮 agent turn（同步；权限、会话、剖面、预算、循环与终止判定全在本函数内）。
///
/// # Errors
///
/// - 权限不足 → `permission_denied`；会话不存在 → `not_found`；
/// - 空提示词／未知剖面／剖面不可用／已有活跃 turn → `invalid_state`；
/// - 供应商错误原样返回（`RequestFailed` + `RuntimeTerminated(error)` 事件已落表）；
/// - 取消与五种终止中其余四种为**正常终止**：返回 `Ok(TurnOutcome)`，
///   `termination_reason` 指明原因，全部有可查询事件（FR-718）。
pub fn run_turn(
    conn: &mut duckdb::Connection,
    policy: &dyn Policy,
    actor: &Actor,
    context: &LlmContext<'_>,
    request: &TurnRequest,
    now_ms: u64,
) -> Result<TurnOutcome, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    let session = request.session_id.trim();
    if session.is_empty() {
        return Err(WhError::invalid_state("llm turn 需要非空 session ID"));
    }
    show_session(conn, policy, actor, session)?;
    let prompt = request.prompt.trim();
    if prompt.is_empty() {
        return Err(WhError::invalid_state("llm turn 需要非空提示词"));
    }
    let registry = context
        .registry
        .ok_or_else(|| match context.registry_error {
            Some(message) => WhError::invalid_state(format!("LLM 剖面不可用：{message}")),
            None => WhError::invalid_state(
                "LLM 剖面不可用：装配根未注入剖面（ADR-031：无剖面即无请求，默认离线）",
            ),
        })?;
    let provider_name = request.provider.trim();
    let profile = registry.find(provider_name).ok_or_else(|| {
        WhError::invalid_state(format!(
            "未注册剖面 {provider_name}；已注册：{}（ADR-031：未注册名不静默回落）",
            registry.names().join(", ")
        ))
    })?;
    profile.validate_endpoint()?;
    let model = request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(profile.model.as_str())
        .to_string();
    if model.is_empty() {
        return Err(WhError::invalid_state("模型标识为空"));
    }
    let max_tokens = request.max_tokens.unwrap_or(profile.max_tokens);
    let request_id = request
        .request_id
        .as_deref()
        .map(validate_request_id)
        .transpose()?
        .map(str::to_string)
        .unwrap_or_else(|| generate_request_id(session, provider_name, prompt, now_ms));
    let iterations_limit = request
        .iterations_limit
        .unwrap_or(MAX_TURN_ITERATIONS)
        .min(MAX_TURN_ITERATIONS);
    if iterations_limit == 0 {
        return Err(WhError::invalid_state(
            "iterations_limit 不得为 0（至少执行一个模型 step）",
        ));
    }
    let budget_tokens = request.budget_tokens.unwrap_or(DEFAULT_TURN_BUDGET_TOKENS);

    // turn ID：从库内状态派生（本会话当前最大事件 seq + 1）——会话内单调、可复现、不依赖时钟。
    let base_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?",
            [session],
            |row| row.get(0),
        )
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("读取会话事件序号失败：{err}")))?;
    let turn_id = format!("turn-{session}-{}", base_seq + 1);

    let (_, _, total) = profile.timeouts();
    let turn_marker = TurnMarker::begin(context.cancel_dir, session)?;
    let marker = CancelMarker::begin(context.cancel_dir, &request_id)?;
    let cancel = CancelToken::default();
    let provider = context.factory.create(profile)?;
    if !provider.is_available() {
        return Err(WhError::invalid_state(format!(
            "Provider {} 不可用：检查剖面与凭据来源后重试",
            provider.name()
        )));
    }

    let mut messages: Vec<LlmMessage> = vec![LlmMessage {
        role: Role::User,
        content: prompt.to_string(),
    }];
    let mut pending_events: Vec<TurnEventDraft> = Vec::new();
    let mut persister = TurnPersister {
        session: session.to_string(),
        persisted: 0,
    };
    let mut envelope: Vec<Value> = Vec::new();
    let mut iterations: u32 = 0;
    let mut chunks: Vec<LlmChunkView> = Vec::new();
    let mut text = String::new();
    let mut finish_reason = None;
    let mut completed = false;
    let mut truncated = false;
    let mut integrity_gaps = 0u64;
    let mut usage = None;
    let mut transport = None;

    /// 信封条目 + 落表草稿成对产出（同名同 payload，FR-722 命名收敛）。
    macro_rules! emit {
        ($kind:expr, $($json:tt)+) => {{
            let entry = json!({ "kind": $kind.as_str(), "turn_id": &turn_id, $($json)+ });
            pending_events.push(TurnEventDraft {
                kind: $kind,
                request_id: Some(request_id.clone()),
                tool_call_id: entry
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                payload: entry.clone(),
            });
            envelope.push(entry);
        }};
    }

    // user 提示词先行落表（FR-722：turn 内 user 轮次落表；预算检查据此可见本轮输入）。
    persister.flush(conn, &mut pending_events, &messages)?;

    // P0-1 口径：工具未接线时每个 step 必然终止（model_finished/tool_not_wired/error/
    // cancelled/budget_exceeded），循环体当前只执行一次——clippy never_loop 如实报告。
    // T216 接线真实工具执行后循环生效，届时移除本 expect（ADR-044 复审触发 1）。
    #[expect(
        clippy::never_loop,
        reason = "P0-1 每步必停；T216 工具接线后循环生效并移除本标注"
    )]
    let termination: TerminationReason = loop {
        // ── 取消检查（每轮迭代前，FR-723）────────────────────────────────
        if marker.is_cancelled() {
            cancel.cancel();
        }
        if cancel.is_cancelled() {
            break TerminationReason::Cancelled;
        }
        // ── 预算检查（T209/FR-721：历史超预算即显式终止，不静默压缩）──────
        let window = load_context(conn, policy, actor, session, budget_tokens)?;
        if let Some(reason) = window.truncated_reason {
            emit!(
                EventKind::ContextState,
                "iteration": iterations,
                "consumed_budget": window.consumed_budget,
                "budget_tokens": budget_tokens,
                "truncated_reason": reason,
            );
            break TerminationReason::BudgetExceeded;
        }
        if iterations >= iterations_limit {
            break TerminationReason::IterationLimit;
        }
        let iteration = iterations;
        iterations += 1;

        // ── 模型 step（chunk/正文上限沿用 llm.rs 常量）───────────────────
        let started = Instant::now();
        let llm_request = LlmRequest {
            request_id: request_id.clone(),
            session_id: session.to_string(),
            task_id: None,
            model: model.clone(),
            messages: messages.clone(),
            max_tokens,
            timeout: Some(total),
        };
        emit!(
            EventKind::RequestStarted,
            "iteration": iteration,
            "request_id": &request_id,
            "provider": profile.name,
            "protocol": profile.protocol.as_str(),
            "model": &model,
            "max_tokens": max_tokens,
        );
        let stream = match provider.execute(&llm_request, &cancel) {
            Ok(stream) => stream,
            Err(err) => {
                emit!(
                    EventKind::RequestFailed,
                    "iteration": iteration,
                    "request_id": request_id,
                    "error_code": err.code.as_str(),
                );
                persister.flush(conn, &mut pending_events, &messages)?;
                return Err(err);
            }
        };

        let mut step_chunks: Vec<LlmChunkView> = Vec::new();
        let mut step_text = String::new();
        let mut step_bytes = 0usize;
        let mut expected_index = 0u64;
        let mut step_truncated = false;
        let mut step_finish: Option<String> = None;
        let mut step_usage: Option<Usage> = None;
        let mut step_tool_calls: Vec<LlmToolCall> = Vec::new();

        for item in stream {
            if marker.is_cancelled() {
                cancel.cancel();
            }
            if cancel.is_cancelled() {
                break;
            }
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(err) => {
                    if cancel.is_cancelled() {
                        break;
                    }
                    emit!(
                        EventKind::RequestFailed,
                        "iteration": iteration,
                        "request_id": request_id,
                        "error_code": err.code.as_str(),
                    );
                    persister.flush(conn, &mut pending_events, &messages)?;
                    return Err(err);
                }
            };
            if chunk.index != expected_index {
                integrity_gaps = integrity_gaps.saturating_add(1);
            }
            expected_index = chunk.index.saturating_add(1);
            let chars = chunk.text.chars().count();
            if step_chunks.len() >= MAX_STREAM_CHUNKS
                || step_bytes.saturating_add(chunk.text.len()) > MAX_OUTPUT_BYTES
            {
                step_truncated = true;
                cancel.cancel();
                break;
            }
            step_bytes = step_bytes.saturating_add(chunk.text.len());
            step_text.push_str(&chunk.text);
            emit!(
                EventKind::StreamingChunk,
                "iteration": iteration,
                "request_id": request_id,
                "index": chunk.index,
                "chars": chars,
            );
            let final_chunk = chunk.final_chunk;
            if final_chunk {
                step_finish.clone_from(&chunk.finish_reason);
                step_usage = chunk.usage;
                step_tool_calls = chunk.tool_calls;
            }
            step_chunks.push(LlmChunkView {
                index: chunk.index,
                chars,
                final_chunk,
            });
            if final_chunk {
                break;
            }
        }

        // ── 末轮取消：显式终止，不伪造完成 ───────────────────────────────
        if cancel.is_cancelled() {
            persister.flush(conn, &mut pending_events, &messages)?;
            break TerminationReason::Cancelled;
        }
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let step_usage_value = match step_usage {
            Some(u) if u.latency_ms > 0 => u,
            Some(u) => u.with_latency(latency_ms),
            None => Usage::estimate(&step_text, &step_text).with_latency(latency_ms),
        };

        // ── 截断/流中断：error 终止并保留完整性证据（FR-208）──────────────
        if step_truncated || step_finish.is_none() {
            emit!(
                EventKind::RequestFailed,
                "iteration": iteration,
                "request_id": request_id,
                "truncated": step_truncated,
                "integrity_gaps": integrity_gaps,
            );
            if !step_text.is_empty() {
                messages.push(LlmMessage {
                    role: Role::Assistant,
                    content: step_text.clone(),
                });
            }
            persister.flush(conn, &mut pending_events, &messages)?;
            chunks = step_chunks;
            truncated = step_truncated;
            break TerminationReason::Error;
        }

        // ── step 完成：RequestCompleted + assistant 消息轮次 ─────────────
        completed = true;
        emit!(
            EventKind::RequestCompleted,
            "iteration": iteration,
            "request_id": request_id,
            "chunks": step_chunks.len(),
            "completed": true,
            "truncated": false,
            "integrity_gaps": integrity_gaps,
            "finish_reason": step_finish,
            "usage": step_usage_value.completion_payload(),
        );
        messages.push(LlmMessage {
            role: Role::Assistant,
            content: step_text.clone(),
        });
        persister.flush(conn, &mut pending_events, &messages)?;
        chunks = step_chunks;
        text = step_text;
        finish_reason = step_finish;
        truncated = step_truncated;
        usage = Some(step_usage_value);
        transport = provider.transport_snapshot();

        // ── 终止判定：无工具调用即完成（对齐 ZCode turn-model-step）────────
        if step_tool_calls.is_empty() {
            break TerminationReason::ModelFinished;
        }

        // ── FR-724：工具执行未接线（P0-1 过渡）——记录 unknown 并终止 ─────
        for tool_call in &step_tool_calls {
            emit!(
                EventKind::ToolCallStarted,
                "iteration": iteration,
                "request_id": request_id,
                "tool_call_id": tool_call.id,
                "tool_name": tool_call.name,
                "state": "unknown",
                "unwired": true,
            );
            emit!(
                EventKind::ToolCallFinished,
                "iteration": iteration,
                "request_id": request_id,
                "tool_call_id": tool_call.id,
                "state": "unknown",
                "unwired": true,
            );
        }
        messages.push(LlmMessage {
            role: Role::Tool,
            content: format!(
                "工具执行未接线（P0-2 前过渡）：{} 个工具调用保持 unknown，未产生任何副作用",
                step_tool_calls.len()
            ),
        });
        persister.flush(conn, &mut pending_events, &messages)?;
        break TerminationReason::ToolNotWired;
    };

    // ── turn 终止事件（五种终止全部显式落表，FR-718）────────────────────
    emit!(
        EventKind::RuntimeTerminated,
        "request_id": request_id,
        "termination_reason": termination.as_str(),
        "iterations": iterations,
    );
    persister.flush(conn, &mut pending_events, &messages)?;
    drop(marker);
    drop(turn_marker);

    Ok(TurnOutcome {
        schema_version: whcode_domain::SCHEMA_VERSION,
        turn_id,
        request_id,
        provider: profile.name.clone(),
        protocol: profile.protocol.as_str().to_string(),
        model,
        iterations,
        termination_reason: termination.as_str().to_string(),
        chunks,
        text,
        finish_reason,
        completed,
        truncated,
        integrity_gaps,
        usage,
        events: envelope,
        transport,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ProviderFactory;
    use crate::{DefaultPolicy, NewSession, actor_with_all_capabilities, new_session};
    use whcode_llm::StubProvider;
    use whcode_llm::http::profile::ProviderProfile;
    use whcode_llm::provider::StubStep;
    use whcode_store::apply_migrations;

    struct StubFactory(StubProvider);

    impl ProviderFactory for StubFactory {
        fn create(
            &self,
            _profile: &ProviderProfile,
        ) -> Result<Box<dyn whcode_llm::Provider>, WhError> {
            Ok(Box::new(self.0.clone()))
        }
    }

    fn migrated() -> duckdb::Connection {
        let conn = duckdb::Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn profile() -> ProviderProfile {
        serde_json::from_str(
            r#"{"name":"p","protocol":"openai","base_url":"https://api.example.com",
                "model":"test-model","auth":{"type":"none"}}"#,
        )
        .expect("剖面夹具")
    }

    fn registry() -> crate::llm::ProviderRegistry {
        crate::llm::ProviderRegistry::from_profiles(vec![profile()]).expect("注册表")
    }

    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "whcode-turn-test-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录");
        dir
    }

    fn session(conn: &mut duckdb::Connection) -> String {
        new_session(
            conn,
            &DefaultPolicy,
            &actor_with_all_capabilities("turn-test"),
            NewSession {
                title: "turn 测试会话".to_string(),
                model: None,
            },
            1_700_000_000_000,
        )
        .expect("新建会话")
        .id
    }

    fn request(session_id: &str, prompt: &str) -> TurnRequest {
        TurnRequest {
            session_id: session_id.to_string(),
            provider: "p".to_string(),
            prompt: prompt.to_string(),
            model: None,
            max_tokens: None,
            request_id: None,
            iterations_limit: None,
            budget_tokens: None,
        }
    }

    fn context<'a>(
        registry: &'a crate::llm::ProviderRegistry,
        factory: &'a StubFactory,
        dir: &'a std::path::Path,
    ) -> LlmContext<'a> {
        LlmContext {
            registry: Some(registry),
            registry_error: None,
            factory,
            cancel_dir: dir,
        }
    }

    fn tool_call_step() -> StubStep {
        StubStep {
            chunks: vec!["调用工具".to_string()],
            finish_reason: "tool_call".to_string(),
            tool_calls: vec![whcode_llm::provider::LlmToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"src/lib.rs"}"#.to_string(),
            }],
        }
    }

    #[test]
    fn single_round_without_tools_finishes_with_model_finished() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("turn-test");
        let policy = DefaultPolicy;
        let registry = registry();
        let factory = StubFactory(StubProvider::new());
        let dir = temp_dir("finished");
        let ctx = context(&registry, &factory, &dir);
        let id = session(&mut conn);
        let outcome = run_turn(&mut conn, &policy, &actor, &ctx, &request(&id, "你好"), 42)
            .expect("单轮完成");
        assert_eq!(outcome.termination_reason, "model_finished");
        assert_eq!(outcome.iterations, 1);
        assert_eq!(outcome.text, "你好", "桩回显");
        assert!(outcome.completed);
        assert!(outcome.turn_id.starts_with(&format!("turn-{id}-")));
        // FR-722：事件与消息轮次落表（user + assistant + 终止事件）。
        let (events, msgs): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM events WHERE session_id=?),
                        (SELECT COUNT(*) FROM messages WHERE session_id=?)",
                [&id, &id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("计数");
        assert!(
            events >= 4,
            "至少 started+chunk+completed+terminated：{events}"
        );
        assert!(msgs >= 2, "user+assistant 落表：{msgs}");
        let reason: String = conn
            .query_row(
                "SELECT CAST(CAST(payload_json AS JSON)->>'$.termination_reason' AS VARCHAR)
                 FROM events WHERE session_id=? AND kind='RuntimeTerminated'",
                [&id],
                |row| row.get(0),
            )
            .expect("终止事件");
        assert_eq!(reason, "model_finished");
        // FR-203：事件载荷不含提示词正文。
        let payload_all: String = conn
            .query_row(
                "SELECT string_agg(CAST(CAST(payload_json AS JSON) AS VARCHAR), '')
                 FROM events WHERE session_id=?",
                [&id],
                |row| row.get(0),
            )
            .expect("载荷聚合");
        assert!(!payload_all.contains("你好"), "事件不得复制提示词正文");
        assert!(
            !dir.join(format!("turn-{id}.live")).exists(),
            "活跃 turn 标记必须随 turn 结束清理（RAII）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tool_call_without_wiring_records_unknown_and_terminates() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("turn-test");
        let policy = DefaultPolicy;
        let registry = registry();
        let factory = StubFactory(StubProvider::with_script(vec![
            tool_call_step(),
            StubStep::simple("不应到达", "stop"),
        ]));
        let dir = temp_dir("unwired");
        let ctx = context(&registry, &factory, &dir);
        let id = session(&mut conn);
        let outcome = run_turn(&mut conn, &policy, &actor, &ctx, &request(&id, "读文件"), 7)
            .expect("tool_not_wired 是正常终止");
        assert_eq!(outcome.termination_reason, "tool_not_wired");
        assert_eq!(outcome.iterations, 1, "未接线即终止，不进入第二轮");
        let started: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=? AND kind='ToolCallStarted'
                 AND CAST(CAST(payload_json AS JSON)->>'$.state' AS VARCHAR)='unknown'",
                [&id],
                |row| row.get(0),
            )
            .expect("工具事件");
        assert_eq!(started, 1, "FR-724：unknown 状态的 ToolCallStarted 落表");
        let tool_msg: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id=? AND role='tool'",
                [&id],
                |row| row.get(0),
            )
            .expect("tool 消息");
        assert_eq!(tool_msg, 1, "未接线说明以 role:tool 轮次落表");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn iteration_limit_zero_is_rejected_and_clamped_to_cap() {
        // 诚实口径：P0-1 下工具未接线即终止，循环每步必停（model_finished 或
        // tool_not_wired），iteration_limit 分支是 T216 接线后的纵深防御——
        // 其端到端测试在 T216 落地（Check 303 复审注记登记）。此处只测入口约束。
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("turn-test");
        let policy = DefaultPolicy;
        let registry = registry();
        let factory = StubFactory(StubProvider::new());
        let dir = temp_dir("limit");
        let ctx = context(&registry, &factory, &dir);
        let id = session(&mut conn);
        let mut req = request(&id, "上限");
        req.iterations_limit = Some(0);
        let err =
            run_turn(&mut conn, &policy, &actor, &ctx, &req, 1).expect_err("limit=0 必须拒绝");
        assert!(err.message.contains("不得为 0"), "{err}");
        // clamp：超过 MAX_TURN_ITERATIONS 的请求被压回上限（行为不受超大值影响）。
        let mut req = request(&id, "上限");
        req.iterations_limit = Some(u32::MAX);
        let outcome = run_turn(&mut conn, &policy, &actor, &ctx, &req, 2).expect("完成");
        assert_eq!(outcome.termination_reason, "model_finished");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancellation_takes_effect_before_first_chunk() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("turn-test");
        let policy = DefaultPolicy;
        let registry = registry();
        let factory = StubFactory(StubProvider::with_script(vec![
            StubStep::simple("第一轮", "stop"),
            StubStep::simple("第二轮", "stop"),
        ]));
        let dir = temp_dir("cancelled");
        let ctx = context(&registry, &factory, &dir);
        let id = session(&mut conn);
        // 预置请求 ID + 预先写入 .cancel：CancelMarker::begin 只写 .live 不清 .cancel，
        // 首 chunk 前的取消检查读到它（FR-723：下一 chunk 边界生效）。
        std::fs::write(dir.join("req-cancel-me.cancel"), b"cancel").expect("取消标记");
        let mut req = request(&id, "取消我");
        req.request_id = Some("req-cancel-me".to_string());
        let outcome = run_turn(&mut conn, &policy, &actor, &ctx, &req, 3).expect("取消终止");
        assert_eq!(outcome.termination_reason, "cancelled");
        assert_eq!(outcome.iterations, 0, "取消在首个 chunk 之前生效");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn budget_exceeded_terminates_with_reason_event() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("turn-test");
        let policy = DefaultPolicy;
        let registry = registry();
        let factory = StubFactory(StubProvider::new());
        let dir = temp_dir("budget");
        let ctx = context(&registry, &factory, &dir);
        let id = session(&mut conn);
        // 预置一条大计量消息：load_context 即截断 → budget_exceeded（T209）。
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES ('M-seed', ?, 'user', 'seed', 999999, current_timestamp, NULL)",
            [&id],
        )
        .expect("预置消息");
        let mut req = request(&id, "预算测试");
        req.budget_tokens = Some(1_000);
        let outcome = run_turn(&mut conn, &policy, &actor, &ctx, &req, 5).expect("预算终止");
        assert_eq!(outcome.termination_reason, "budget_exceeded");
        assert_eq!(outcome.iterations, 0, "预算检查在模型 step 之前");
        let state: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=? AND kind='ContextState'
                 AND CAST(CAST(payload_json AS JSON)->>'$.truncated_reason' AS VARCHAR) IS NOT NULL",
                [&id],
                |row| row.get(0),
            )
            .expect("ContextState 事件");
        assert!(state >= 1, "预算终止必须有 truncated_reason 事件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn active_turn_conflict_and_validation_errors() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("turn-test");
        let policy = DefaultPolicy;
        let registry = registry();
        let factory = StubFactory(StubProvider::new());
        let dir = temp_dir("conflict");
        let ctx = context(&registry, &factory, &dir);
        let id = session(&mut conn);

        let err = run_turn(
            &mut conn,
            &policy,
            &actor,
            &ctx,
            &request("S-不存在", "hi"),
            1,
        )
        .expect_err("未知会话必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);

        let err = run_turn(&mut conn, &policy, &actor, &ctx, &request(&id, "  "), 1)
            .expect_err("空提示词必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        // FR-723：残留活跃标记 → 第二个 turn 被拒绝。
        std::fs::write(dir.join(format!("turn-{id}.live")), b"turn").expect("残留标记");
        let err = run_turn(&mut conn, &policy, &actor, &ctx, &request(&id, "hi"), 1)
            .expect_err("活跃冲突必须拒绝");
        assert!(err.message.contains("FR-723"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn envelope_events_do_not_contain_prompt_text() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("turn-test");
        let policy = DefaultPolicy;
        let registry = registry();
        let factory = StubFactory(StubProvider::new());
        let dir = temp_dir("privacy");
        let ctx = context(&registry, &factory, &dir);
        let id = session(&mut conn);
        let prompt = "机密提示词正文-不得进事件";
        let outcome =
            run_turn(&mut conn, &policy, &actor, &ctx, &request(&id, prompt), 1).expect("完成");
        let serialized = serde_json::to_string(&outcome.events).expect("序列化");
        assert!(
            !serialized.contains(prompt),
            "FR-203：事件不得复制提示词正文"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
