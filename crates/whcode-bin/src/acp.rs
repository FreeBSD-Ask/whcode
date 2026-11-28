//! # ACP automation-only 服务端（P6.3／FR-789／ADR-081）
//!
//! 编辑器与自动化客户端经 Agent Client Protocol v1（JSON-RPC 2.0 over stdio）驱动本仓。
//! 本模块只做 **Agent 侧**，且只有一种形态：**automation-only**——没有人工应答面，工具
//! 审批由“配置规则表＋`approval_rules` 本地已存规则”显式决策，无规则命中的调用以
//! `approval_unavailable` 结构化失败（fail-closed，审计在案）。`session/request_permission`
//! 永不发出：决策不出进程（ADR-081 决策 5）。
//!
//! 纪律（沿 SDK 通道 ADR-071）：帧 ≤64 KiB、单活跃 turn、单工作区（装配期绑定）、
//! `clientInfo` 不用于安全决策、凭据不进 `_meta`。

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::Connection;
use serde_json::{Value, json};
use whcode_app::approval::ApprovalGate;
use whcode_app::turn::{TurnObserver, TurnRequest, TurnSignal, run_turn_with_approval};
use whcode_domain::{ErrorCode, WhError};

use crate::config::Config;
use crate::llm_provider::ServeLlm;
use crate::sdk::{FrameSink, FrameSource, MAX_LINE_BYTES, OUT_CHANNEL_CAPACITY};

/// ACP v1 协议版本（整数＝主版本；只升有破坏性变更）。
pub const PROTOCOL_VERSION: i64 = 1;

// JSON-RPC 2.0 标准错误码。
pub const RPC_PARSE: i64 = -32700;
pub const RPC_INVALID_REQUEST: i64 = -32600;
pub const RPC_METHOD_NOT_FOUND: i64 = -32601;
pub const RPC_INVALID_PARAMS: i64 = -32602;
pub const RPC_INTERNAL: i64 = -32603;
/// 本仓服务端状态错误（JSON-RPC 服务器错误区；避开 ACP 已占用的 -32000）。
pub const RPC_WHCODE_STATE: i64 = -32001;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

// ── 协议层：帧解析与编码 ────────────────────────────────────────────────

/// 一帧入站消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpIncoming {
    /// 请求（`id` 保原形：数字或字符串）。
    Request {
        /// 请求 ID。
        id: Value,
        /// 方法名。
        method: String,
        /// 参数对象。
        params: Value,
    },
    /// 通知（无 id）。
    Notification {
        /// 方法名。
        method: String,
        /// 参数对象。
        params: Value,
    },
    /// 坏帧（尽量保留 id 以便回错误）。
    Malformed {
        /// 可提取的请求 ID。
        id: Option<Value>,
        /// 原因。
        message: String,
    },
}

/// 解析一帧 JSON-RPC 2.0（`jsonrpc` 必须为 `"2.0"`；`id` 接受数字或字符串）。
#[must_use]
pub fn parse_frame(line: &str) -> AcpIncoming {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(err) => {
            return AcpIncoming::Malformed {
                id: None,
                message: format!("JSON 解析失败：{err}"),
            };
        }
    };
    let id = value
        .get("id")
        .cloned()
        .filter(|id| id.is_i64() || id.is_u64() || id.is_string());
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return AcpIncoming::Malformed {
            id,
            message: "jsonrpc 字段必须为 \"2.0\"".to_string(),
        };
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return AcpIncoming::Malformed {
            id,
            message: "缺少 method 字段".to_string(),
        };
    };
    let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return AcpIncoming::Malformed {
            id,
            message: "params 必须为对象".to_string(),
        };
    }
    match id {
        Some(id) => AcpIncoming::Request {
            id,
            method: method.to_string(),
            params,
        },
        None => AcpIncoming::Notification {
            method: method.to_string(),
            params,
        },
    }
}

/// 成功响应帧。
#[must_use]
pub fn encode_result(id: &Value, result: &Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// 错误响应帧。
#[must_use]
pub fn encode_error(id: Option<&Value>, code: i64, message: &str) -> String {
    let id = id.cloned().unwrap_or(Value::Null);
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// 通知帧。
#[must_use]
pub fn encode_notification(method: &str, params: &Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// `initialize` 响应（能力最小集＋automation-only 显式声明）。
#[must_use]
pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": false,
            "promptCapabilities": {
                "image": false,
                "audio": false,
                "embeddedContext": false,
            },
            "_meta": {"whcode": {"automation_only": true}},
        },
        "agentInfo": {
            "name": "whcode",
            "title": "旺皇大模型执行框架",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "authMethods": [],
    })
}

/// 从 `session/prompt` 参数提取正文：只接受 text 内容块（能力位已广告为 false 的类型
/// 显式拒绝），按序以空行连接。空数组与全空白都是 `invalid params`。
///
/// # Errors
///
/// `sessionId` 缺失、`prompt` 缺失或非数组、出现非 text 块、正文为空时返回
/// （错误码, 消息）。
pub fn prompt_text(params: &Value) -> Result<(String, String), (i64, String)> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            (
                RPC_INVALID_PARAMS,
                "session/prompt 缺少 sessionId".to_string(),
            )
        })?
        .to_string();
    let blocks = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            (
                RPC_INVALID_PARAMS,
                "session/prompt 缺少 prompt 数组".to_string(),
            )
        })?;
    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        let kind = block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind != "text" {
            return Err((
                RPC_INVALID_PARAMS,
                format!(
                    "内容块类型 `{kind}` 未在本端广告（promptCapabilities 全 false）：只接受 text"
                ),
            ));
        }
        let text = block
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        parts.push(text.to_string());
    }
    let joined = parts.join("\n\n");
    if joined.trim().is_empty() {
        return Err((RPC_INVALID_PARAMS, "prompt 正文为空".to_string()));
    }
    Ok((session_id, joined))
}

/// 终止原因 → ACP `stopReason`：只有 `cancelled` 直传，其余一律 `end_turn`；精确值
/// 经 `result._meta` 透传（ADR-081 决策 4）。
#[must_use]
pub fn stop_reason(termination_reason: &str) -> &'static str {
    if termination_reason == "cancelled" {
        "cancelled"
    } else {
        "end_turn"
    }
}

/// 工具名 → ACP `ToolKind`（近似映射；未知一律 `other`）。
#[must_use]
pub fn tool_kind(tool: &str) -> &'static str {
    match tool {
        "read_file" => "read",
        "write_file" | "edit_file" => "edit",
        "shell" => "execute",
        "search_messages" | "lsp_definition" | "lsp_references" | "lsp_hover"
        | "lsp_diagnostics" => "search",
        _ => "other",
    }
}

/// 工具执行状态 → ACP `ToolCallStatus`（v1 枚举经 schema 源码与 docs.rs v1.4.0 双源
/// 确认：pending／in_progress／completed／failed）：`ok`→`completed`，其余→`failed`。
#[must_use]
pub fn tool_status(state: &str) -> &'static str {
    if state == "ok" { "completed" } else { "failed" }
}

/// 自动化审批策略（ADR-081 决策 5）：非交互口径 ＋ shell 默认拒绝种子（配置显式写
/// `tools.shell` 时不覆盖用户选择）＋ `approval_rules` 表（最后叠加，优先生效）。
///
/// # Errors
///
/// 配置非法或存储失败透传（运行期不得静默忽略配置错误）。
pub fn automation_policy(
    conn: &Connection,
    approval: &crate::config::ApprovalConfig,
) -> Result<whcode_app::approval::ApprovalPolicy, WhError> {
    let mut policy = approval.to_policy()?;
    if !approval.tools.contains_key("shell") {
        policy = policy.with_tool_rule("shell", whcode_domain::approval::ApprovalRule::Deny)?;
    }
    policy.with_stored_rules(
        whcode_store::approval_rules::list_rules(conn)?
            .into_iter()
            .map(|row| (row.tool, row.rule)),
    )
}

// ── 网关：turn 执行（自动化审批口径）────────────────────────────────────

/// 一次 turn 的结果（终止原因保留稳定名，供 `stop_reason` 与 `_meta` 使用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpTurnOutcome {
    /// 请求 ID。
    pub request_id: String,
    /// 终止原因稳定名。
    pub termination_reason: String,
    /// 末轮拼接正文。
    pub text: String,
}

/// ACP 网关（生产实现 [`AppAcpGateway`]；测试用桩）。
pub trait AcpGateway: Send + Sync {
    /// 创建会话，返回会话 ID（即 ACP sessionId）。
    ///
    /// # Errors
    ///
    /// 权限或存储失败透传。
    fn create_session(&self, title: Option<&str>) -> Result<String, WhError>;

    /// 阻塞执行一个 turn（自动化审批口径：规则表决策，无规则 fail-closed）。
    ///
    /// # Errors
    ///
    /// 编排、审批配置或存储失败透传。
    fn run_turn(
        &self,
        session: &str,
        prompt: &str,
        provider: Option<&str>,
        request_id: &str,
        observer: Option<TurnObserver<'_>>,
    ) -> Result<AcpTurnOutcome, WhError>;

    /// 取消活跃 turn（写取消标记）；`false`＝无匹配活跃请求。
    ///
    /// # Errors
    ///
    /// 标记写入失败透传。
    fn cancel_turn(&self, request_id: &str) -> Result<bool, WhError>;

    /// 绑定的工作区根（`session/new` 的 `cwd` 一致性校验用）。
    fn workspace_root(&self) -> &Path;
}

/// 生产网关：自动化审批口径的 turn 执行。
pub struct AppAcpGateway {
    /// 装配根持有的工作区连接（单写者；ACP 不自开库）。
    pub conn: Arc<Mutex<Connection>>,
    /// LLM 装配（剖面注册表＋工厂＋取消目录）。
    pub llm: Arc<ServeLlm>,
    /// 顶层配置（审批规则表的装配口径来自这里）。
    pub config: Config,
    /// 绑定的工作区根。
    pub workspace: std::path::PathBuf,
}

impl AcpGateway for AppAcpGateway {
    fn create_session(&self, title: Option<&str>) -> Result<String, WhError> {
        let mut guard = self.conn.lock().expect("工作区连接锁");
        let outcome = whcode_app::execute_command(
            &mut guard,
            &whcode_app::DefaultPolicy,
            &whcode_app::actor_with_all_capabilities("acp-local"),
            &whcode_app::FrontendCommand::SessionNew {
                title: title.unwrap_or_default().to_string(),
                model: None,
                now_ms: now_ms(),
            },
        )?;
        let whcode_app::CommandOutcome::Session(row) = outcome else {
            return Err(WhError::internal("门面返回形状不符：应为 Session"));
        };
        Ok(row.id.clone())
    }

    fn run_turn(
        &self,
        session: &str,
        prompt: &str,
        provider: Option<&str>,
        request_id: &str,
        observer: Option<TurnObserver<'_>>,
    ) -> Result<AcpTurnOutcome, WhError> {
        let mut guard = self.conn.lock().expect("工作区连接锁");
        let context = self.llm.context();
        // provider 缺省：注册表恰有一个剖面时取它，否则显式要求声明（与 SDK 同口径，不猜测）。
        let provider = match provider.map(str::to_string) {
            Some(name) => name,
            None => {
                let profiles = context
                    .registry
                    .ok_or_else(|| {
                        WhError::invalid_state(
                            "剖面注册表未接线：acp serve 必须以 --provider 显式声明".to_string(),
                        )
                    })?
                    .profiles();
                match profiles {
                    [single] => single.name.clone(),
                    _ => {
                        return Err(WhError::invalid_state(
                            "acp serve 必须以 --provider 显式声明剖面（注册表剖面数非唯一）"
                                .to_string(),
                        ));
                    }
                }
            }
        };
        // 自动化审批口径（ADR-081 决策 5）：非交互策略＋shell 默认拒绝种子＋本地已存规则；
        // 应答器恒为 None——规则为 ask 的调用 approval_unavailable，绝不静默放行。
        let policy = automation_policy(&guard, &self.config.approval)?;
        let mut gate =
            ApprovalGate::for_store("acp-local", policy, None, self.config.approval.wait_ms);
        let outcome = run_turn_with_approval(
            &mut guard,
            &whcode_app::DefaultPolicy,
            &whcode_app::actor_with_all_capabilities("acp-local"),
            &context,
            &TurnRequest {
                session_id: session.to_string(),
                provider,
                prompt: prompt.to_string(),
                model: None,
                max_tokens: None,
                request_id: Some(request_id.to_string()),
                iterations_limit: None,
                budget_tokens: None,
                workspace_root: Some(self.workspace.clone()),
                depth: 0,
                steer: None,
            },
            None,
            observer,
            &mut gate,
            now_ms(),
        )?;
        Ok(AcpTurnOutcome {
            request_id: outcome.request_id,
            termination_reason: outcome.termination_reason,
            text: outcome.text,
        })
    }

    fn cancel_turn(&self, request_id: &str) -> Result<bool, WhError> {
        let context = self.llm.context();
        match whcode_app::llm::cancel(
            &whcode_app::DefaultPolicy,
            &whcode_app::actor_with_all_capabilities("acp-local"),
            &context,
            request_id,
        ) {
            Ok(_) => Ok(true),
            Err(err) if err.code == ErrorCode::NotFound => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn workspace_root(&self) -> &Path {
        &self.workspace
    }
}

// ── 服务循环 ────────────────────────────────────────────────────────────

/// 服务运行期状态。
struct AcpState {
    initialized: bool,
    /// 活跃 turn（请求 ID；`session/cancel` 与重复 `session/prompt` 的判定依据）。
    active: Option<String>,
}

/// 请求 ID：`acp-<now>-<hash16>`（与 SDK 同构的取消标记安全命名）。
fn acp_request_id(session: &str, prompt: &str) -> String {
    use std::hash::{Hash, Hasher};
    let now = now_ms();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session.hash(&mut hasher);
    prompt.hash(&mut hasher);
    now.hash(&mut hasher);
    format!("acp-{now}-{:016x}", hasher.finish())
}

/// 运行 ACP 服务（阻塞至 stdin EOF；ACP 客户端以子进程方式启动本进程）。
///
/// # Errors
///
/// 写线程创建失败或写帧失败时返回 [`WhError`]（读侧 EOF 是正常结束）。
pub fn run_acp_server(
    gateway: Arc<dyn AcpGateway>,
    provider: Option<String>,
    source: &mut dyn FrameSource,
    sink: Box<dyn FrameSink>,
) -> Result<(), WhError> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<String>(OUT_CHANNEL_CAPACITY);
    let state = Arc::new(Mutex::new(AcpState {
        initialized: false,
        active: None,
    }));
    // stdout 写线程（唯一写者；与 SDK 同构）。
    let writer = std::thread::Builder::new()
        .name("acp-writer".to_string())
        .spawn(move || {
            let mut sink = sink;
            while let Ok(line) = rx.recv() {
                if !sink.write_frame(&line) {
                    break;
                }
            }
        })
        .map_err(|err| WhError::internal(format!("ACP 写线程创建失败：{err}")))?;

    let send = |tx: &std::sync::mpsc::SyncSender<String>, frame: String| {
        let _ = tx.send(frame);
    };

    while let Some(line) = source.next_line() {
        if line.len() > MAX_LINE_BYTES {
            send(
                &tx,
                encode_error(None, RPC_INVALID_REQUEST, "帧超过 64 KiB 上限"),
            );
            continue;
        }
        match parse_frame(&line) {
            AcpIncoming::Malformed { id, message } => {
                send(&tx, encode_error(id.as_ref(), RPC_PARSE, &message));
            }
            AcpIncoming::Notification { method, params } => {
                handle_notification(&gateway, &state, &tx, &method, &params);
            }
            AcpIncoming::Request { id, method, params } => {
                handle_request(
                    &gateway,
                    &state,
                    &tx,
                    &id,
                    &method,
                    &params,
                    provider.as_deref(),
                );
            }
        }
    }
    drop(tx);
    let _ = writer.join();
    Ok(())
}

fn handle_notification(
    gateway: &Arc<dyn AcpGateway>,
    state: &Arc<Mutex<AcpState>>,
    tx: &std::sync::mpsc::SyncSender<String>,
    method: &str,
    params: &Value,
) {
    match method {
        // 取消（ACP 专用通知与 JSON-RPC 通用通知都接受；无活跃 turn 时忽略）。
        "session/cancel" | "$/cancel_request" => {
            let request_id = state.lock().expect("ACP 状态锁").active.clone();
            if let Some(request_id) = request_id {
                let _ = gateway.cancel_turn(&request_id);
            }
        }
        _ => {
            let _ = tx;
            let _ = params;
        }
    }
}

fn handle_request(
    gateway: &Arc<dyn AcpGateway>,
    state: &Arc<Mutex<AcpState>>,
    tx: &std::sync::mpsc::SyncSender<String>,
    id: &Value,
    method: &str,
    params: &Value,
    provider: Option<&str>,
) {
    match method {
        "initialize" => {
            state.lock().expect("ACP 状态锁").initialized = true;
            let _ = tx.send(encode_result(id, &initialize_result()));
        }
        _ if !state.lock().expect("ACP 状态锁").initialized => {
            let _ = tx.send(encode_error(
                Some(id),
                RPC_INVALID_REQUEST,
                "必须先 initialize（ACP initialization MUST）",
            ));
        }
        "session/new" => {
            session_new(gateway, state, tx, id, params);
        }
        "session/prompt" => {
            session_prompt(gateway, state, tx, id, params, provider);
        }
        // 客户端方法（fs/*、terminal/*、elicitation、session/load、
        // session/request_permission）由 Agent 调用而非实现；客户端发来即未知方法。
        _ => {
            let _ = tx.send(encode_error(
                Some(id),
                RPC_METHOD_NOT_FOUND,
                &format!("未知方法 `{method}`（本端能力见 initialize 响应）"),
            ));
        }
    }
}

fn session_new(
    gateway: &Arc<dyn AcpGateway>,
    state: &Arc<Mutex<AcpState>>,
    tx: &std::sync::mpsc::SyncSender<String>,
    id: &Value,
    params: &Value,
) {
    // cwd 一致性（ADR-081 决策 3）：工作区在装配期绑定，客户端给的 cwd 必须一致。
    if let Some(cwd) = params.get("cwd").and_then(Value::as_str) {
        let bound = gateway.workspace_root();
        let same = std::fs::canonicalize(cwd)
            .ok()
            .zip(std::fs::canonicalize(bound).ok())
            .is_some_and(|(left, right)| left == right);
        if !same {
            let _ = tx.send(encode_error(
                Some(id),
                RPC_INVALID_PARAMS,
                &format!(
                    "session/new 的 cwd 与绑定工作区不一致：绑定 {}，请求 {cwd}",
                    bound.display()
                ),
            ));
            return;
        }
    }
    if params
        .get("mcpServers")
        .and_then(Value::as_array)
        .is_some_and(|servers| !servers.is_empty())
    {
        let _ = tx.send(encode_error(
            Some(id),
            RPC_INVALID_PARAMS,
            "不接受客户端注入的 mcpServers（提示注入面；用工作区 .whcode/mcp.json）",
        ));
        return;
    }
    match gateway.create_session(Some("ACP 会话")) {
        Ok(session_id) => {
            let _ = tx.send(encode_result(id, &json!({"sessionId": session_id})));
        }
        Err(err) => {
            let _ = tx.send(encode_error(Some(id), RPC_INTERNAL, &err.message));
        }
    }
    let _ = state;
}

fn session_prompt(
    gateway: &Arc<dyn AcpGateway>,
    state: &Arc<Mutex<AcpState>>,
    tx: &std::sync::mpsc::SyncSender<String>,
    id: &Value,
    params: &Value,
    provider: Option<&str>,
) {
    let (session_id, prompt) = match prompt_text(params) {
        Ok(pair) => pair,
        Err((code, message)) => {
            let _ = tx.send(encode_error(Some(id), code, &message));
            return;
        }
    };
    // 单活跃 turn：第二个 prompt 显式拒绝（busy），不排队、不静默合并。
    {
        let guard = state.lock().expect("ACP 状态锁");
        if guard.active.is_some() {
            let _ = tx.send(encode_error(
                Some(id),
                RPC_WHCODE_STATE,
                "已有活跃 turn：等待其终态或先 session/cancel",
            ));
            return;
        }
    }
    let request_id = acp_request_id(&session_id, &prompt);
    state.lock().expect("ACP 状态锁").active = Some(request_id.clone());
    let id_worker = id.clone();
    let tx_worker = tx.clone();
    let state_worker = Arc::clone(state);
    let gateway_worker = Arc::clone(gateway);
    let session_observer = session_id.clone();
    let session_worker = session_id;
    let provider_worker = provider.map(str::to_string);
    let request_for_id = request_id.clone();
    // 观察者：工具信号 → session/update 通知（实时的 tool_call/tool_call_update）。
    let tx_observer = tx.clone();
    let observer = move |signal: TurnSignal| {
        let (variant, update) = match signal {
            TurnSignal::ToolStarted {
                tool_call_id,
                tool_name,
                ..
            } => (
                "tool_call",
                json!({
                    "toolCallId": tool_call_id,
                    "title": tool_name,
                    "name": tool_name,
                    "kind": tool_kind(&tool_name),
                    "status": "in_progress",
                }),
            ),
            TurnSignal::ToolFinished {
                tool_call_id,
                state,
                ..
            } => (
                "tool_call_update",
                json!({
                    "toolCallId": tool_call_id,
                    "status": tool_status(&state),
                }),
            ),
        };
        let mut update = update;
        update["sessionUpdate"] = json!(variant);
        let frame = encode_notification(
            "session/update",
            &json!({"sessionId": session_observer, "update": update}),
        );
        let _ = tx_observer.try_send(frame);
    };
    let join = std::thread::Builder::new()
        .name("acp-turn".to_string())
        .spawn(move || {
            let outcome = gateway_worker.run_turn(
                &session_worker,
                &prompt,
                provider_worker.as_deref(),
                &request_for_id,
                Some(&observer),
            );
            state_worker.lock().expect("ACP 状态锁").active = None;
            match outcome {
                Ok(outcome) => {
                    // 正文送达（ACP：输出经 session/update 的 agent_message_chunk 流出；
                    // v1 无逐块流式，终态后一次送达——ADR-081 已知限制）。
                    if !outcome.text.is_empty() {
                        let frame = encode_notification(
                            "session/update",
                            &json!({
                                "sessionId": session_worker,
                                "update": {
                                    "sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": outcome.text},
                                },
                            }),
                        );
                        let _ = tx_worker.send(frame);
                    }
                    let _ = tx_worker.send(encode_result(
                        &id_worker,
                        &json!({
                            "stopReason": stop_reason(&outcome.termination_reason),
                            "_meta": {"whcode": {
                                "termination_reason": outcome.termination_reason,
                                "request_id": outcome.request_id,
                            }},
                        }),
                    ));
                }
                Err(err) => {
                    let _ =
                        tx_worker.send(encode_error(Some(&id_worker), RPC_INTERNAL, &err.message));
                }
            }
        });
    if let Err(err) = join {
        state.lock().expect("ACP 状态锁").active = None;
        let _ = tx.send(encode_error(
            Some(id),
            RPC_INTERNAL,
            &format!("turn 线程创建失败：{err}"),
        ));
    }
    let _ = state;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::{FrameSink, FrameSource};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    // ── 测试替身 ────────────────────────────────────────────────────────

    struct VecSource {
        lines: VecDeque<String>,
    }

    impl FrameSource for VecSource {
        fn next_line(&mut self) -> Option<String> {
            self.lines.pop_front()
        }
    }

    /// 抄送 sink：帧同时进入共享缓冲，供服务返回后断言。
    struct TeeSink {
        inner: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl FrameSink for TeeSink {
        fn write_frame(&mut self, frame: &str) -> bool {
            self.inner.lock().expect("帧锁").push(frame.to_string());
            true
        }
    }

    /// 桩网关：会话固定 `S-1`；turn 阻塞到取消标记（≤2 s）后以 `cancelled` 终态返回，
    /// 否则以 `model_finished` 返回固定正文。
    struct StubGateway {
        workspace: std::path::PathBuf,
        cancel_flag: Arc<AtomicBool>,
    }

    impl AcpGateway for StubGateway {
        fn create_session(&self, _title: Option<&str>) -> Result<String, WhError> {
            Ok("S-1".to_string())
        }

        fn run_turn(
            &self,
            _session: &str,
            _prompt: &str,
            _provider: Option<&str>,
            request_id: &str,
            _observer: Option<TurnObserver<'_>>,
        ) -> Result<AcpTurnOutcome, WhError> {
            for _ in 0..40 {
                if self.cancel_flag.load(Ordering::SeqCst) {
                    return Ok(AcpTurnOutcome {
                        request_id: request_id.to_string(),
                        termination_reason: "cancelled".to_string(),
                        text: String::new(),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(AcpTurnOutcome {
                request_id: request_id.to_string(),
                termination_reason: "model_finished".to_string(),
                text: "模型正文".to_string(),
            })
        }

        fn cancel_turn(&self, _request_id: &str) -> Result<bool, WhError> {
            self.cancel_flag.store(true, Ordering::SeqCst);
            Ok(true)
        }

        fn workspace_root(&self) -> &Path {
            &self.workspace
        }
    }

    fn request(id: u64, method: &str, params: Value) -> String {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
    }

    /// 运行一串入站帧，返回出站帧的 JSON 值列表（服务返回后帧已全部冲刷）。
    fn run_frames(gateway: Arc<dyn AcpGateway>, lines: Vec<String>) -> Vec<Value> {
        let mut source = VecSource {
            lines: VecDeque::from(lines),
        };
        let collected: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let boxed: Box<dyn FrameSink> = Box::new(TeeSink {
            inner: Arc::clone(&collected),
        });
        run_acp_server(gateway, None, &mut source, boxed).expect("服务运行");
        let frames = collected.lock().expect("帧锁").clone();
        frames
            .iter()
            .map(|frame| serde_json::from_str(frame).expect("出站帧 JSON"))
            .collect()
    }

    // ── 协议层 ──────────────────────────────────────────────────────────

    /// 帧解析：请求／通知／坏帧三分；`jsonrpc` 与 `params` 形状必须正确。
    #[test]
    fn parse_frames_distinguish_request_notification_and_malformed() {
        match parse_frame(r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{}}"#) {
            AcpIncoming::Request { id, method, .. } => {
                assert_eq!(id, json!(7));
                assert_eq!(method, "initialize");
            }
            other => panic!("应为请求：{other:?}"),
        }
        match parse_frame(
            r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"S-1"}}"#,
        ) {
            AcpIncoming::Notification { method, .. } => assert_eq!(method, "session/cancel"),
            other => panic!("应为通知：{other:?}"),
        }
        match parse_frame(r#"{"jsonrpc":"2.0","id":"req-1","method":"x","params":{}}"#) {
            AcpIncoming::Request { id, .. } => assert_eq!(id, json!("req-1")),
            other => panic!("字符串 id 应为请求：{other:?}"),
        }
        for (line, why) in [
            (r#"{"id":1,"method":"x"}"#, "缺 jsonrpc"),
            (r#"{"jsonrpc":"1.0","id":1,"method":"x"}"#, "错误版本"),
            (r#"{"jsonrpc":"2.0","id":1,"params":{}}"#, "缺 method"),
            (
                r#"{"jsonrpc":"2.0","id":1,"method":"x","params":[]}"#,
                "params 非对象",
            ),
            ("not json", "非 JSON"),
        ] {
            match parse_frame(line) {
                AcpIncoming::Malformed { .. } => {}
                other => panic!("{why} 应为坏帧：{other:?}"),
            }
        }
    }

    /// initialize 响应：版本 1、能力最小集、automation_only 经 _meta 显式声明。
    #[test]
    fn initialize_result_advertises_minimal_capabilities() {
        let result = initialize_result();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["agentCapabilities"]["loadSession"], false);
        for key in ["image", "audio", "embeddedContext"] {
            assert_eq!(
                result["agentCapabilities"]["promptCapabilities"][key], false,
                "{key} 能力位必须为 false"
            );
        }
        assert_eq!(result["authMethods"], json!([]));
        assert_eq!(
            result["agentCapabilities"]["_meta"]["whcode"]["automation_only"], true,
            "automation-only 必须显式声明（ADR-081 决策 2）"
        );
    }

    /// prompt 提取：多 text 块按序合并；非 text 块与空正文显式拒绝。
    #[test]
    fn prompt_text_accepts_only_text_blocks() {
        let params = json!({
            "sessionId": "S-1",
            "prompt": [
                {"type": "text", "text": "第一段"},
                {"type": "text", "text": "第二段"},
            ],
        });
        let (session, text) = prompt_text(&params).expect("提取");
        assert_eq!(session, "S-1");
        assert_eq!(text, "第一段\n\n第二段");

        for (params, why) in [
            (
                json!({"sessionId": "S-1", "prompt": [{"type": "image", "mimeType": "image/png", "data": "AA"}]}),
                "非 text 块",
            ),
            (json!({"sessionId": "S-1", "prompt": []}), "空数组"),
            (
                json!({"sessionId": "S-1", "prompt": [{"type": "text", "text": "  "}]}),
                "全空白",
            ),
            (
                json!({"prompt": [{"type": "text", "text": "hi"}]}),
                "缺 sessionId",
            ),
            (json!({"sessionId": "S-1"}), "缺 prompt"),
        ] {
            let err = prompt_text(&params).expect_err(why);
            assert_eq!(err.0, RPC_INVALID_PARAMS, "{why}");
        }
    }

    /// 映射：stopReason、ToolCallStatus 与 ToolKind。
    #[test]
    fn stop_reason_and_tool_status_mappings() {
        assert_eq!(stop_reason("cancelled"), "cancelled");
        for reason in [
            "model_finished",
            "iteration_limit",
            "budget_exceeded",
            "error",
        ] {
            assert_eq!(stop_reason(reason), "end_turn", "{reason} → end_turn");
        }
        assert_eq!(tool_status("ok"), "completed");
        for state in ["failed", "denied", "approval_unavailable"] {
            assert_eq!(tool_status(state), "failed", "{state} → failed");
        }
        assert_eq!(tool_kind("read_file"), "read");
        assert_eq!(tool_kind("write_file"), "edit");
        assert_eq!(tool_kind("shell"), "execute");
        assert_eq!(tool_kind("lsp_definition"), "search");
        assert_eq!(tool_kind("mystery_tool"), "other");
    }

    // ── 端到端（桩网关）─────────────────────────────────────────────────

    /// 端到端：未初始化拒绝；initialize 版本协商；session/new；prompt 正文流出＋
    /// end_turn 与 _meta 终止原因；未知方法 -32601。
    #[test]
    fn server_e2e_happy_path_and_rejections() {
        let workspace = std::env::temp_dir().join(format!("whcode-acp-e2e-{}", std::process::id()));
        // Code Scanning 收口：路径字符串不含 ".." 才使用（防临时目录遍历注入）。
        let workspace_checked = workspace.display().to_string();
        if workspace_checked.contains("..") {
            panic!("临时目录路径不得包含 ..");
        }
        let workspace = std::path::PathBuf::from(workspace_checked);
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&workspace).expect("临时工作区");
        let gateway: Arc<dyn AcpGateway> = Arc::new(StubGateway {
            workspace: workspace.clone(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        });
        let cwd = std::fs::canonicalize(&workspace).expect("规范化");
        let frames = run_frames(
            gateway,
            vec![
                // 未初始化先调用：显式拒绝。
                request(1, "session/new", json!({"cwd": cwd.to_string_lossy()})),
                request(2, "initialize", json!({"protocolVersion": 1})),
                request(
                    3,
                    "session/new",
                    json!({"cwd": cwd.to_string_lossy(), "mcpServers": []}),
                ),
                // 未知方法（放在 prompt 之前：turn 工作线程异步执行，其后的同步请求
                // 会先于 turn 的通知/结果被处理——客户端侧同样如此）。
                request(5, "mystery", json!({})),
                request(
                    4,
                    "session/prompt",
                    json!({"sessionId": "S-1", "prompt": [{"type": "text", "text": "你好"}]}),
                ),
            ],
        );

        assert_eq!(
            frames[0]["error"]["code"], RPC_INVALID_REQUEST,
            "帧序 {frames:?}"
        );
        assert_eq!(frames[1]["result"]["protocolVersion"], 1);
        assert_eq!(frames[2]["result"]["sessionId"], "S-1");
        // 未知方法（异步语义：turn 工作线程执行期间，同步请求先被处理）。
        assert_eq!(frames[3]["error"]["code"], RPC_METHOD_NOT_FOUND);
        // prompt：先正文通知，后 end_turn 结果与 _meta 终止原因。
        assert_eq!(frames[4]["method"], "session/update");
        assert_eq!(
            frames[4]["params"]["update"]["sessionUpdate"],
            "agent_message_chunk"
        );
        assert_eq!(frames[4]["params"]["update"]["content"]["text"], "模型正文");
        assert_eq!(frames[5]["result"]["stopReason"], "end_turn");
        assert_eq!(
            frames[5]["result"]["_meta"]["whcode"]["termination_reason"],
            "model_finished"
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// 端到端：cwd 不一致与 mcpServers 注入显式拒绝；非 text 内容块拒绝；
    /// 单活跃 turn（第二个 prompt busy）＋取消流（stopReason=cancelled）。
    #[test]
    fn server_e2e_rejects_injection_and_supports_cancellation() {
        let workspace =
            std::env::temp_dir().join(format!("whcode-acp-e2e2-{}", std::process::id()));
        // Code Scanning 收口：路径字符串不含 ".." 才使用（防临时目录遍历注入）。
        let workspace_checked = workspace.display().to_string();
        if workspace_checked.contains("..") {
            panic!("临时目录路径不得包含 ..");
        }
        let workspace = std::path::PathBuf::from(workspace_checked);
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&workspace).expect("临时工作区");
        let gateway: Arc<dyn AcpGateway> = Arc::new(StubGateway {
            workspace: workspace.clone(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        });
        let cwd = std::fs::canonicalize(&workspace).expect("规范化");
        let frames = run_frames(
            gateway,
            vec![
                request(1, "initialize", json!({"protocolVersion": 1})),
                // cwd 不一致：显式拒绝。
                request(2, "session/new", json!({"cwd": "C:/definitely-not-bound"})),
                // mcpServers 注入：显式拒绝。
                request(
                    3,
                    "session/new",
                    json!({"cwd": cwd.to_string_lossy(), "mcpServers": [{"name": "x"}]}),
                ),
                request(
                    4,
                    "session/new",
                    json!({"cwd": cwd.to_string_lossy(), "mcpServers": []}),
                ),
                // 非 text 内容块：拒绝。
                request(
                    5,
                    "session/prompt",
                    json!({"sessionId": "S-1", "prompt": [{"type": "image", "data": "AA"}]}),
                ),
                // 活跃 turn 占位（桩网关阻塞至取消标记）。
                request(
                    6,
                    "session/prompt",
                    json!({"sessionId": "S-1", "prompt": [{"type": "text", "text": "长任务"}]}),
                ),
                // 第二个 prompt：busy。
                request(
                    7,
                    "session/prompt",
                    json!({"sessionId": "S-1", "prompt": [{"type": "text", "text": "并发"}]}),
                ),
                // 取消 → 第一个 prompt 以 cancelled 应答。
                json!({
                    "jsonrpc": "2.0",
                    "method": "session/cancel",
                    "params": {"sessionId": "S-1"}
                })
                .to_string(),
            ],
        );

        assert_eq!(frames[1]["error"]["code"], RPC_INVALID_PARAMS, "cwd 不一致");
        assert_eq!(
            frames[2]["error"]["code"], RPC_INVALID_PARAMS,
            "mcpServers 注入"
        );
        assert_eq!(frames[3]["result"]["sessionId"], "S-1");
        assert_eq!(frames[4]["error"]["code"], RPC_INVALID_PARAMS, "非 text 块");
        assert_eq!(
            frames[5]["error"]["code"], RPC_WHCODE_STATE,
            "第二个 prompt busy"
        );
        assert_eq!(
            frames[6]["result"]["stopReason"], "cancelled",
            "帧序 {frames:?}"
        );
        assert_eq!(
            frames[6]["result"]["_meta"]["whcode"]["termination_reason"],
            "cancelled"
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    // ── 自动化审批口径（ADR-081 决策 5）────────────────────────────────

    /// 策略构造：shell 默认拒绝；配置显式规则优先于种子；`approval_rules` 表最后叠加、
    /// 优先生效。
    #[test]
    fn automation_policy_seeds_deny_and_honors_configured_and_stored_rules() {
        use whcode_app::approval::SideEffectScope;
        use whcode_domain::approval::ApprovalRule;

        let conn = Connection::open_in_memory().expect("内存库");
        whcode_store::apply_migrations(&conn).expect("迁移");

        // 默认配置：shell 被种子拒绝，只读工具沿用读侧默认。
        let policy =
            automation_policy(&conn, &crate::config::ApprovalConfig::default()).expect("默认策略");
        assert_eq!(
            policy.rule_for("shell", SideEffectScope::ProcessSpawn),
            ApprovalRule::Deny,
            "无人值守的 shell 默认拒绝（ADR-081 决策 5）"
        );
        assert_eq!(
            policy.rule_for("read_file", SideEffectScope::ReadOnly),
            ApprovalRule::Allow
        );

        // 配置显式写 tools.shell = ask：用户选择优先于种子（ask → 无应答面即 unavailable）。
        let mut config = crate::config::ApprovalConfig::default();
        config.tools.insert("shell".to_string(), ApprovalRule::Ask);
        let policy = automation_policy(&conn, &config).expect("配置策略");
        assert_eq!(
            policy.rule_for("shell", SideEffectScope::ProcessSpawn),
            ApprovalRule::Ask
        );

        // 已存规则最后叠加：shell 显式 allow 覆盖种子。
        whcode_store::approval_rules::upsert_rule(&conn, "shell", ApprovalRule::Allow, 1_000)
            .expect("登记规则");
        let policy = automation_policy(&conn, &config).expect("叠加策略");
        assert_eq!(
            policy.rule_for("shell", SideEffectScope::ProcessSpawn),
            ApprovalRule::Allow,
            "已存规则优先生效"
        );
    }
}
