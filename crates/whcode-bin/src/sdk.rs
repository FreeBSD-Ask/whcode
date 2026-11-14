//! # SDK JSON-RPC 服务端（T294／FR-778／ADR-071）
//!
//! 出进程 API：`whcode sdk` 以行分帧 JSON-RPC 2.0（stdio，单连接）暴露最小协议。
//! 并发模型与不变量见 `docs/adr/ADR-071-sdk-jsonrpc-out-of-process-api.md`
//! （§2.2 线程模型、§3 不变量、§8 验证计划——本文件的测试逐项兑现）：
//!
//! ```text
//! stdin 读线程（主循环）──→ turn 工作线程（活跃槽上限 1，RAII 释放）
//!   └─ events.follow ──→ 事件泵线程（100 ms 有界轮询）
//! 全部出站 ──→ 有界通道（sync_channel）──→ stdout 写线程（唯一写者）
//! ```
//!
//! - `SdkGateway` 注入：本模块不直接触库（生产实现 [`AppSdkGateway`] 走
//!   `execute_command` 门面与 `run_turn`；测试用桩）。
//! - 背压：出站通道满 → 记录关闭原因 → 各生产者停止 → 写线程补发 `closed` 帧
//!   （关闭序列；客户端带游标重连重放）。不丢帧、不静默丢弃。
//! - 世代（generation）＝会话 `SessionRewound` 事件累计条数：原始事件按 seq 事实流
//!   （只增不改），世代只提示“派生口径已变、派生视图需重算”。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::{Value, json};
use whcode_app::authz::{Actor, DefaultPolicy};
use whcode_app::llm::LlmContext;
use whcode_app::turn::{TurnRequest, run_turn};
use whcode_domain::{ErrorCode, WhError};
use whcode_store::query::{Direction, EventCursor, events_page};

/// 协议版本（D14：主版本兼容、minor 只增、破坏性升主版本）。
pub const PROTOCOL_VERSION: &str = "1.0";
/// 单帧字节上限（超限丢弃并回错误帧后继续）。
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// `turn.run` 提示词字节上限。
pub const MAX_PROMPT_BYTES: usize = 256 * 1024;
/// 事件泵轮询间隔（serve 广播泵同口径）。
pub const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// 事件泵单批条数上限（store `events_page` 的页大小口径）。
pub const EVENT_BATCH_LIMIT: usize = 100;
/// 出站通道容量（慢消费者触发关闭序列的阈值）。
pub const OUT_CHANNEL_CAPACITY: usize = 256;
/// 活跃槽上限（每连接）。
pub const ACTIVE_TURN_LIMIT: usize = 1;

// JSON-RPC 线上码（语义判定以 `data.code` 的 CUI 稳定码名为准，见契约 §5）。
const RPC_INVALID_REQUEST: i64 = -32600;
const RPC_METHOD_NOT_FOUND: i64 = -32601;
const RPC_INVALID_PARAMS: i64 = -32602;
const RPC_SERVER_ERROR: i64 = -32603;
const RPC_BUSY: i64 = -32000;

/// 事件类型名：世代推进的判据（ADR-068 决策 1）。
const REWIND_KIND: &str = "SessionRewound";

// ── 帧编解码 ─────────────────────────────────────────────────────────────

/// 客户端请求帧（v1 客户端不发送通知：无数字 id 的帧按坏帧处理）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkRequest {
    /// JSON-RPC id。
    pub id: u64,
    /// 方法名。
    pub method: String,
    /// 参数对象（缺省空对象）。
    pub params: Value,
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdkIncoming {
    /// 合法请求。
    Request(SdkRequest),
    /// 坏帧（`id` 取得到则回带 id 的错误，否则 id 为 `None`）。
    Malformed { id: Option<u64>, message: String },
}

/// 解析一帧（行已去尾换行）。
#[must_use]
pub fn parse_frame(line: &str) -> SdkIncoming {
    let value = match serde_json::from_str::<Value>(line) {
        Ok(value) => value,
        Err(err) => {
            return SdkIncoming::Malformed {
                id: None,
                message: format!("帧不是合法 JSON：{err}"),
            };
        }
    };
    let id = value.get("id").and_then(Value::as_u64);
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return SdkIncoming::Malformed {
            id,
            message: "jsonrpc 字段必须为 2.0".to_string(),
        };
    }
    let Some(method) = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return SdkIncoming::Malformed {
            id,
            message: "缺少 method 字段".to_string(),
        };
    };
    let Some(id) = id else {
        return SdkIncoming::Malformed {
            id: None,
            message: "v1 客户端只发送带数字 id 的请求（不支持通知）".to_string(),
        };
    };
    let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
    SdkIncoming::Request(SdkRequest { id, method, params })
}

/// 成功响应帧。
#[must_use]
pub fn encode_result(id: u64, result: &Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// 错误响应帧（`data` 承载 CUI 信封四字段，契约 §5）。
#[must_use]
pub fn encode_error(id: Option<u64>, code: i64, message: &str, data: &Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message, "data": data },
    })
    .to_string()
}

/// 通知帧（无 id）。
#[must_use]
pub fn encode_notification(method: &str, params: &Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

/// CUI 信封 `data`（`schemaVersion`／`code`／`message`／`retryable`／`details`）。
fn cui_data(code: &str, message: &str, retryable: bool, details: Value) -> Value {
    json!({
        "schemaVersion": "1.0",
        "code": code,
        "message": message,
        "retryable": retryable,
        "details": details,
    })
}

/// 由 [`WhError`] 构造错误帧（稳定码名/可重试/细节全部透传）。
fn error_frame(id: Option<u64>, rpc_code: i64, err: &WhError) -> String {
    encode_error(
        id,
        rpc_code,
        &err.message,
        &cui_data(
            err.code.as_str(),
            &err.message,
            err.retryable,
            err.details.clone().unwrap_or_else(|| json!({})),
        ),
    )
}

/// 有界出站的唯一发送口：通道满 → 记录关闭原因（slow_consumer，先到者为准）并
/// 返回 `false`；生产者收到 `false` 即停止产出（不阻塞、不丢帧进已满通道）。
fn try_send_frame(
    tx: &mpsc::SyncSender<String>,
    close_reason: &Mutex<Option<String>>,
    frame: String,
) -> bool {
    match tx.try_send(frame) {
        Ok(()) => true,
        Err(mpsc::TrySendError::Full(_)) => {
            let mut guard = close_reason.lock().expect("关闭原因锁");
            if guard.is_none() {
                *guard = Some("slow_consumer".to_string());
            }
            false
        }
        Err(mpsc::TrySendError::Disconnected(_)) => false,
    }
}

// ── 网关注入（本模块不直接触库，ADR-018/030 前端口径）──────────────────

/// 会话快照（世代＋游标起点）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkSessionInfo {
    /// 世代＝`SessionRewound` 累计条数。
    pub generation: u64,
    /// 当前最大事件 seq。
    pub max_seq: u64,
}

/// 单条事件帧（载荷已按追踪级别脱敏——写入时处理）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkEventFrame {
    /// 工作区 seq。
    pub seq: u64,
    /// 事件类型稳定名。
    pub kind: String,
    /// 载荷 JSON。
    pub payload: Value,
}

/// 增量批次（`next_seq` 为续读游标；`rewound_delta` 为本批 `SessionRewound` 条数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkEventBatch {
    /// 本批事件帧（seq 升序）。
    pub events: Vec<SdkEventFrame>,
    /// 续读游标。
    pub next_seq: u64,
    /// 本批世代增量。
    pub rewound_delta: u64,
}

/// turn 执行结果（`status`＝`termination_reason` 稳定名透传）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkTurnResult {
    /// 请求 ID。
    pub request_id: String,
    /// 终止原因稳定名。
    pub status: String,
    /// 末轮拼接正文。
    pub text: String,
    /// 失败信息（无失败为 `None`）。
    pub error: Option<Value>,
}

/// SDK 对应用服务的注入面（生产实现 [`AppSdkGateway`]；测试用桩）。
pub trait SdkGateway: Send + Sync {
    /// 创建会话，返回会话 ID。
    ///
    /// # Errors
    ///
    /// 权限或存储失败透传。
    fn create_session(&self, title: Option<&str>) -> Result<String, WhError>;

    /// 打开会话，返回世代与游标快照。
    ///
    /// # Errors
    ///
    /// 会话不存在 → `not_found`；其余透传。
    fn open_session(&self, session: &str) -> Result<SdkSessionInfo, WhError>;

    /// 增量读取事件（`from_seq` 排他，升序，条数 ≤ `limit`）。
    ///
    /// # Errors
    ///
    /// 存储失败透传。
    fn events_after(
        &self,
        session: &str,
        from_seq: u64,
        limit: usize,
    ) -> Result<SdkEventBatch, WhError>;

    /// 阻塞执行一个 turn（至终态；取消经 [`Self::cancel_turn`] 的标记生效）。
    ///
    /// # Errors
    ///
    /// 编排失败透传。
    fn run_turn(
        &self,
        session: &str,
        prompt: &str,
        max_tokens: Option<u64>,
        provider: Option<&str>,
        request_id: &str,
    ) -> Result<SdkTurnResult, WhError>;

    /// 取消活跃 turn（写取消标记）；`false`＝无匹配活跃请求。
    ///
    /// # Errors
    ///
    /// 标记写入失败透传。
    fn cancel_turn(&self, request_id: &str) -> Result<bool, WhError>;
}

// ── 生产网关（门面 + run_turn + llm::cancel）────────────────────────────

/// 生产网关：门面建会话、`run_turn` 编排、`llm::cancel` 写取消标记。
pub struct AppSdkGateway {
    /// 装配根注入的工作区连接（单写者；SDK 不自开库）。
    pub conn: Arc<Mutex<duckdb::Connection>>,
    /// LLM 装配（剖面注册表＋工厂＋取消目录）。
    pub llm: Arc<crate::llm_provider::ServeLlm>,
    /// 授权策略。
    pub policy: Arc<DefaultPolicy>,
    /// 调用者身份。
    pub actor: Actor,
    /// 工作区根（工具执行绑定）。
    pub workspace: PathBuf,
}

impl SdkGateway for AppSdkGateway {
    fn create_session(&self, title: Option<&str>) -> Result<String, WhError> {
        let mut guard = self.conn.lock().expect("工作区连接锁");
        let outcome = whcode_app::execute_command(
            &mut guard,
            self.policy.as_ref(),
            &self.actor,
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

    fn open_session(&self, session: &str) -> Result<SdkSessionInfo, WhError> {
        let guard = self.conn.lock().expect("工作区连接锁");
        // 世代初值：该会话 SessionRewound 累计条数（回滚稀少，条数有界；
        // 超过页上限时按可得页计数，登记为已知限制）。
        let rewinds =
            whcode_store::query::events_of_kind(&guard, session, REWIND_KIND, usize::MAX)?;
        let max_seq = whcode_store::query::latest_seq(&guard)?;
        Ok(SdkSessionInfo {
            generation: rewinds.len() as u64,
            max_seq: u64::try_from(max_seq).unwrap_or(0),
        })
    }

    fn events_after(
        &self,
        session: &str,
        from_seq: u64,
        limit: usize,
    ) -> Result<SdkEventBatch, WhError> {
        let guard = self.conn.lock().expect("工作区连接锁");
        let page = events_page(
            &guard,
            session,
            Direction::Forward,
            Some(EventCursor {
                seq: i64::try_from(from_seq).unwrap_or(i64::MAX),
            }),
            limit,
        )?;
        let mut rewound_delta = 0u64;
        let mut next_seq = from_seq;
        let events = page
            .items
            .iter()
            .map(|row| {
                next_seq = u64::try_from(row.seq).unwrap_or(next_seq).max(next_seq);
                if row.kind == REWIND_KIND {
                    rewound_delta += 1;
                }
                SdkEventFrame {
                    seq: u64::try_from(row.seq).unwrap_or(0),
                    kind: row.kind.clone(),
                    payload: serde_json::from_str(&row.payload_json)
                        .unwrap_or_else(|_| json!({ "raw": row.payload_json })),
                }
            })
            .collect::<Vec<_>>();
        if let Some(cursor) = page.next {
            next_seq = u64::try_from(cursor.seq).unwrap_or(next_seq);
        }
        Ok(SdkEventBatch {
            events,
            next_seq,
            rewound_delta,
        })
    }

    fn run_turn(
        &self,
        session: &str,
        prompt: &str,
        max_tokens: Option<u64>,
        provider: Option<&str>,
        request_id: &str,
    ) -> Result<SdkTurnResult, WhError> {
        let mut guard = self.conn.lock().expect("工作区连接锁");
        let context: LlmContext<'_> = self.llm.context();
        // provider 缺省：剖面注册表恰有一个剖面时取它，否则显式要求声明（不猜测）。
        let provider = match provider {
            Some(name) => name.to_string(),
            None => {
                let profiles = context
                    .registry
                    .ok_or_else(|| {
                        WhError::invalid_state(
                            "剖面注册表未接线：turn.run 必须显式声明 provider".to_string(),
                        )
                    })?
                    .profiles();
                match profiles {
                    [single] => single.name.clone(),
                    _ => {
                        return Err(WhError::invalid_state(
                            "turn.run 必须显式声明 provider（注册表剖面数非唯一）".to_string(),
                        ));
                    }
                }
            }
        };
        let outcome = run_turn(
            &mut guard,
            self.policy.as_ref(),
            &self.actor,
            &context,
            &TurnRequest {
                session_id: session.to_string(),
                provider,
                prompt: prompt.to_string(),
                model: None,
                max_tokens,
                request_id: Some(request_id.to_string()),
                iterations_limit: None,
                budget_tokens: None,
                workspace_root: Some(self.workspace.clone()),
                depth: 0,
            },
            now_ms(),
        )?;
        Ok(SdkTurnResult {
            request_id: outcome.request_id,
            status: outcome.termination_reason,
            text: outcome.text,
            error: None,
        })
    }

    fn cancel_turn(&self, request_id: &str) -> Result<bool, WhError> {
        let context = self.llm.context();
        match whcode_app::llm::cancel(self.policy.as_ref(), &self.actor, &context, request_id) {
            Ok(_) => Ok(true),
            // 无活跃调用：按契约返回 cancelled=false（非错误）。
            Err(err) if err.code == ErrorCode::NotFound => Ok(false),
            Err(err) => Err(err),
        }
    }
}

/// SDK 请求 ID：`sdk-<now>-<hash>`（字符集与长度符合 app 的取消标记文件名纪律，
/// 与 app 内部生成器同构但独立实现——不为此扩 app 的可见性）。
fn sdk_request_id(session: &str, prompt: &str) -> String {
    use std::hash::{Hash, Hasher};
    let now = now_ms();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session.hash(&mut hasher);
    prompt.hash(&mut hasher);
    now.hash(&mut hasher);
    format!("sdk-{now}-{:016x}", hasher.finish())
}

// ── 生产传输适配器（stdio）──────────────────────────────────────────────

/// 帧源：stdin 行读（EOF＝`None`；行尾换行剥离）。
#[derive(Default)]
pub struct StdioSource {
    buf: String,
}

impl FrameSource for StdioSource {
    fn next_line(&mut self) -> Option<String> {
        use std::io::BufRead;
        self.buf.clear();
        match std::io::stdin().lock().read_line(&mut self.buf) {
            Ok(0) => None,
            Ok(_) => Some(self.buf.trim_end_matches(['\n', '\r']).to_string()),
            Err(_) => None,
        }
    }
}

/// 帧汇：stdout 行写＋立即 flush（帧语义要求客户端即时可见）。
#[derive(Default)]
pub struct StdioSink;

impl FrameSink for StdioSink {
    fn write_frame(&mut self, line: &str) -> bool {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        out.write_all(line.as_bytes())
            .and_then(|_| {
                out.write_all(
                    b"
",
                )
            })
            .and_then(|_| out.flush())
            .is_ok()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── 服务端 ───────────────────────────────────────────────────────────────

/// 入站帧源（测试注入内存通道；生产为 stdio 行读）。
pub trait FrameSource: Send {
    /// 下一行（`None`＝EOF）。
    fn next_line(&mut self) -> Option<String>;
}

/// 出站帧汇（返回 `false`＝写失败，服务端按连接关闭处理）。
pub trait FrameSink: Send {
    fn write_frame(&mut self, line: &str) -> bool;
}

/// 连接内共享状态（短临界区；cursor/generation 与泵线程共享）。
struct SdkState {
    initialized: bool,
    focus: Option<String>,
    generation: u64,
    cursor: u64,
    following: bool,
    active_request: Option<String>,
}

/// 活跃槽 RAII 守卫（评审 M3 同款：退出即释放）。
struct SlotGuard<'a>(&'a AtomicUsize);

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 运行 SDK 服务端（阻塞至 stdin EOF、写失败或慢消费者触发关闭序列）。
///
/// # Errors
///
/// 仅在内部线程 panic 时传播（fail-fast；业务失败以错误帧出站）。
pub fn run_sdk_server(
    gateway: Arc<dyn SdkGateway>,
    capacity: usize,
    source: &mut dyn FrameSource,
    sink: Box<dyn FrameSink>,
) -> Result<(), WhError> {
    let (tx, rx) = mpsc::sync_channel::<String>(capacity);
    let shutdown = Arc::new(AtomicBool::new(false));
    let close_reason: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let state = Arc::new(Mutex::new(SdkState {
        initialized: false,
        focus: None,
        generation: 0,
        cursor: 0,
        following: false,
        active_request: None,
    }));
    let slot = Arc::new(AtomicUsize::new(0));

    // stdout 写线程（唯一写者）：逐帧写出；帧间/超时检查关闭原因（慢消费者等）。
    let writer_close = Arc::clone(&close_reason);
    let writer = std::thread::Builder::new()
        .name("sdk-writer".to_string())
        .spawn(move || {
            let mut sink = sink;
            loop {
                // 退出条件只有三个：写失败、关闭原因、全部生产者停止（Disconnected）。
                // “通道暂时为空”不是退出条件（泵有 100 ms 轮询间隔，帧是稀疏的）。
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(line) => {
                        if !sink.write_frame(&line) {
                            break;
                        }
                        let reason = writer_close.lock().expect("关闭原因锁").clone();
                        if let Some(reason) = reason {
                            let frame = encode_notification("closed", &json!({ "reason": reason }));
                            let _ = sink.write_frame(&frame);
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let reason = writer_close.lock().expect("关闭原因锁").clone();
                        if let Some(reason) = reason {
                            let frame = encode_notification("closed", &json!({ "reason": reason }));
                            let _ = sink.write_frame(&frame);
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // 全部生产者已停止（EOF/关闭）：排空残余帧（排队中的响应
                        // 仍须送达客户端）后退出。
                        while let Ok(line) = rx.try_recv() {
                            if !sink.write_frame(&line) {
                                return;
                            }
                        }
                        let reason = writer_close.lock().expect("关闭原因锁").clone();
                        if let Some(reason) = reason {
                            let frame = encode_notification("closed", &json!({ "reason": reason }));
                            let _ = sink.write_frame(&frame);
                        }
                        break;
                    }
                }
            }
        })
        .expect("写线程创建");

    let mut pump: Option<std::thread::JoinHandle<()>> = None;
    let mut worker: Option<std::thread::JoinHandle<()>> = None;

    loop {
        if close_reason.lock().expect("关闭原因锁").is_some() {
            break;
        }
        let Some(line) = source.next_line() else {
            break;
        };
        if line.len() > MAX_LINE_BYTES {
            try_send_frame(
                &tx,
                &close_reason,
                encode_error(
                    None,
                    RPC_INVALID_REQUEST,
                    "帧超过 64 KiB 上限",
                    &cui_data(
                        "invalid_state",
                        "帧超过 64 KiB 上限，本帧已丢弃",
                        false,
                        json!({}),
                    ),
                ),
            );
            continue;
        }
        match parse_frame(&line) {
            SdkIncoming::Malformed { id, message } => {
                try_send_frame(
                    &tx,
                    &close_reason,
                    encode_error(
                        id,
                        RPC_INVALID_REQUEST,
                        &message,
                        &cui_data("parse_error", &message, false, json!({})),
                    ),
                );
            }
            SdkIncoming::Request(request) => {
                dispatch(
                    request,
                    &gateway,
                    &state,
                    &slot,
                    &tx,
                    &close_reason,
                    &shutdown,
                    &mut pump,
                    &mut worker,
                );
            }
        }
    }

    // 优雅关闭：停泵/停工作线程 → join → 写线程在通道排空后退出。
    shutdown.store(true, Ordering::SeqCst);
    if let Some(handle) = pump.take() {
        let _ = handle.join();
    }
    if let Some(handle) = worker.take() {
        let _ = handle.join();
    }
    drop(tx);
    let _ = writer.join();
    Ok(())
}

/// 请求分发（主线程；turn 与 follow 各自派生线程）。
#[allow(clippy::too_many_arguments)]
fn dispatch(
    request: SdkRequest,
    gateway: &Arc<dyn SdkGateway>,
    state: &Arc<Mutex<SdkState>>,
    slot: &Arc<AtomicUsize>,
    tx: &mpsc::SyncSender<String>,
    close_reason: &Arc<Mutex<Option<String>>>,
    shutdown: &Arc<AtomicBool>,
    pump: &mut Option<std::thread::JoinHandle<()>>,
    worker: &mut Option<std::thread::JoinHandle<()>>,
) {
    let id = request.id;
    let params = request.params;
    let respond = |result: Value| {
        try_send_frame(tx, close_reason, encode_result(id, &result));
    };
    let fail = |rpc_code: i64, err: &WhError| {
        try_send_frame(tx, close_reason, error_frame(Some(id), rpc_code, err));
    };

    match request.method.as_str() {
        "initialize" => {
            let client_version = params.get("protocolVersion").and_then(Value::as_str);
            let Some(client_version) = client_version else {
                fail(
                    RPC_INVALID_PARAMS,
                    &WhError::invalid_state("initialize 缺少 protocolVersion".to_string()),
                );
                return;
            };
            let client_major = client_version.split('.').next().unwrap_or("");
            let server_major = PROTOCOL_VERSION.split('.').next().unwrap_or("");
            if client_major != server_major {
                let err = WhError::invalid_state(format!(
                    "protocolVersion 主版本不兼容：客户端 {client_version}，服务端 {PROTOCOL_VERSION}"
                ));
                fail(RPC_INVALID_PARAMS, &err);
                return;
            }
            state.lock().expect("状态锁").initialized = true;
            respond(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "server": "whcode",
                "serverVersion": env!("CARGO_PKG_VERSION"),
            }));
        }
        method => {
            if !state.lock().expect("状态锁").initialized {
                let err = WhError::invalid_state("必须先调用 initialize（协议协商）".to_string());
                fail(RPC_SERVER_ERROR, &err);
                return;
            }
            match method {
                "session.create" => {
                    let title = params.get("title").and_then(Value::as_str);
                    let session = match gateway.create_session(title) {
                        Ok(session) => session,
                        Err(err) => {
                            fail(RPC_SERVER_ERROR, &err);
                            return;
                        }
                    };
                    let info = match gateway.open_session(&session) {
                        Ok(info) => info,
                        Err(err) => {
                            fail(RPC_SERVER_ERROR, &err);
                            return;
                        }
                    };
                    {
                        let mut guard = state.lock().expect("状态锁");
                        guard.focus = Some(session.clone());
                        guard.generation = info.generation;
                        guard.cursor = info.max_seq;
                    }
                    respond(json!({
                        "session": session,
                        "generation": info.generation,
                        "maxSeq": info.max_seq,
                    }));
                }
                "session.open" => {
                    let Some(session) = params.get("session").and_then(Value::as_str) else {
                        fail(
                            RPC_INVALID_PARAMS,
                            &WhError::invalid_state("session.open 缺少 session".to_string()),
                        );
                        return;
                    };
                    let info = match gateway.open_session(session) {
                        Ok(info) => info,
                        Err(err) => {
                            fail(RPC_SERVER_ERROR, &err);
                            return;
                        }
                    };
                    {
                        let mut guard = state.lock().expect("状态锁");
                        guard.focus = Some(session.to_string());
                        guard.generation = info.generation;
                        guard.cursor = info.max_seq;
                    }
                    respond(json!({
                        "session": session,
                        "generation": info.generation,
                        "maxSeq": info.max_seq,
                    }));
                }
                "turn.run" => {
                    let prompt_owned = params
                        .get("prompt")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let Some(prompt) = prompt_owned else {
                        fail(
                            RPC_INVALID_PARAMS,
                            &WhError::invalid_state("turn.run 缺少 prompt".to_string()),
                        );
                        return;
                    };
                    if prompt.len() > MAX_PROMPT_BYTES {
                        fail(
                            RPC_INVALID_PARAMS,
                            &WhError::invalid_state(format!(
                                "prompt {len} 字节超上限 {MAX_PROMPT_BYTES}",
                                len = prompt.len()
                            )),
                        );
                        return;
                    }
                    let Some(focus) = state.lock().expect("状态锁").focus.clone() else {
                        fail(
                            RPC_SERVER_ERROR,
                            &WhError::invalid_state(
                                "未选择焦点会话：先调用 session.create 或 session.open".to_string(),
                            ),
                        );
                        return;
                    };
                    // 活跃槽：CAS 0→1，失败即 busy（不排队、不静默）。
                    if slot
                        .compare_exchange(0, ACTIVE_TURN_LIMIT, Ordering::SeqCst, Ordering::SeqCst)
                        .is_err()
                    {
                        let err = WhError::invalid_state(
                            "已有活跃 turn（每连接上限 1）：先等待完成或调用 turn.cancel"
                                .to_string(),
                        );
                        fail(RPC_BUSY, &err);
                        return;
                    }
                    let request_id = sdk_request_id(&focus, &prompt);
                    state.lock().expect("状态锁").active_request = Some(request_id.clone());
                    let max_tokens = params.get("maxTokens").and_then(Value::as_u64);
                    let provider = params
                        .get("provider")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let gateway = Arc::clone(gateway);
                    let tx = tx.clone();
                    let state = Arc::clone(state);
                    let close_reason = Arc::clone(close_reason);
                    let slot_ref = Arc::clone(slot);
                    *worker = Some(
                        std::thread::Builder::new()
                            .name("sdk-turn".to_string())
                            .spawn(move || {
                                let guard = SlotGuard(&slot_ref);
                                let result = gateway.run_turn(
                                    &focus,
                                    &prompt,
                                    max_tokens,
                                    provider.as_deref(),
                                    &request_id,
                                );
                                state.lock().expect("状态锁").active_request = None;
                                if close_reason.lock().expect("关闭原因锁").is_some() {
                                    return; // 连接关闭中：终态已在库，帧不再产出
                                }
                                match result {
                                    Ok(turn) => {
                                        try_send_frame(
                                            &tx,
                                            &close_reason,
                                            encode_result(
                                                id,
                                                &json!({
                                                    "requestId": turn.request_id,
                                                    "status": turn.status,
                                                    "text": turn.text,
                                                    "error": turn.error,
                                                }),
                                            ),
                                        );
                                    }
                                    Err(err) => {
                                        try_send_frame(
                                            &tx,
                                            &close_reason,
                                            error_frame(Some(id), RPC_SERVER_ERROR, &err),
                                        );
                                    }
                                }
                                drop(guard);
                            })
                            .expect("turn 工作线程创建"),
                    );
                }
                "turn.cancel" => {
                    let active = state.lock().expect("状态锁").active_request.clone();
                    let Some(active) = active else {
                        respond(json!({ "cancelled": false }));
                        return;
                    };
                    match params.get("requestId").and_then(Value::as_str) {
                        Some(explicit) if explicit != active => {
                            respond(json!({ "cancelled": false }));
                            return;
                        }
                        _ => {}
                    }
                    match gateway.cancel_turn(&active) {
                        Ok(cancelled) => respond(json!({ "cancelled": cancelled })),
                        Err(err) => fail(RPC_SERVER_ERROR, &err),
                    }
                }
                "events.follow" => {
                    let follow_target = {
                        let guard = state.lock().expect("状态锁");
                        if guard.following {
                            None
                        } else {
                            let session = params
                                .get("session")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .or_else(|| guard.focus.clone());
                            let from_seq = params.get("fromSeq").and_then(Value::as_u64);
                            session.map(|s| (s, from_seq.unwrap_or(0)))
                        }
                    };
                    let Some((focus, from_seq)) = follow_target else {
                        let err = if state.lock().expect("状态锁").following {
                            WhError::invalid_state(
                                "本连接已登记 events.follow（单消费者）".to_string(),
                            )
                        } else {
                            WhError::invalid_state(
                                "events.follow 缺少 session 且无焦点会话".to_string(),
                            )
                        };
                        fail(RPC_SERVER_ERROR, &err);
                        return;
                    };
                    let info = match gateway.open_session(&focus) {
                        Ok(info) => info,
                        Err(err) => {
                            fail(RPC_SERVER_ERROR, &err);
                            return;
                        }
                    };
                    {
                        let mut guard = state.lock().expect("状态锁");
                        guard.following = true;
                        guard.focus = Some(focus.clone());
                        guard.generation = info.generation;
                        guard.cursor = from_seq;
                    }
                    let respond_cursor = from_seq;
                    let respond_generation = info.generation;
                    let max_seq = info.max_seq;
                    let gateway = Arc::clone(gateway);
                    let tx = tx.clone();
                    let state = Arc::clone(state);
                    let close_reason = Arc::clone(close_reason);
                    let shutdown = Arc::clone(shutdown);
                    *pump = Some(
                        std::thread::Builder::new()
                            .name("sdk-pump".to_string())
                            .spawn(move || {
                                let mut cursor = respond_cursor;
                                let mut generation = respond_generation;
                                while !shutdown.load(Ordering::SeqCst) {
                                    if close_reason.lock().expect("关闭原因锁").is_some() {
                                        break;
                                    }
                                    let batch = match gateway.events_after(
                                        &focus,
                                        cursor,
                                        EVENT_BATCH_LIMIT,
                                    ) {
                                        Ok(batch) => batch,
                                        Err(err) => {
                                            try_send_frame(
                                                &tx,
                                                &close_reason,
                                                error_frame(None, RPC_SERVER_ERROR, &err),
                                            );
                                            break;
                                        }
                                    };
                                    for frame in &batch.events {
                                        let params = json!({
                                            "session": focus,
                                            "seq": frame.seq,
                                            "generation": generation,
                                            "kind": frame.kind,
                                            "payload": frame.payload,
                                        });
                                        if !try_send_frame(
                                            &tx,
                                            &close_reason,
                                            encode_notification("event", &params),
                                        ) {
                                            return;
                                        }
                                    }
                                    if batch.rewound_delta > 0 {
                                        generation += batch.rewound_delta;
                                        let params = json!({
                                            "session": focus,
                                            "generation": generation,
                                        });
                                        if !try_send_frame(
                                            &tx,
                                            &close_reason,
                                            encode_notification("generation", &params),
                                        ) {
                                            return;
                                        }
                                    }
                                    cursor = batch.next_seq;
                                    if batch.events.is_empty() {
                                        std::thread::sleep(EVENT_POLL_INTERVAL);
                                    }
                                }
                                state.lock().expect("状态锁").cursor = cursor;
                                state.lock().expect("状态锁").generation = generation;
                            })
                            .expect("事件泵线程创建"),
                    );
                    respond(json!({
                        "generation": respond_generation,
                        "nextSeq": respond_cursor,
                        "maxSeq": max_seq,
                    }));
                }
                other => {
                    let err = WhError::invalid_state(format!("未知方法：{other}"));
                    fail(RPC_METHOD_NOT_FOUND, &err);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // ── 帧编解码矩阵 ─────────────────────────────────────────────────

    #[test]
    fn parse_frame_matrix() {
        let ok = parse_frame(r#"{"jsonrpc":"2.0","id":7,"method":"session.create","params":{}}"#);
        match ok {
            SdkIncoming::Request(request) => {
                assert_eq!(request.id, 7);
                assert_eq!(request.method, "session.create");
            }
            other => panic!("应为合法请求：{other:?}"),
        }
        let bad = parse_frame("{ not json }");
        assert!(matches!(bad, SdkIncoming::Malformed { id: None, .. }));
        let bad = parse_frame(r#"{"jsonrpc":"1.0","id":3,"method":"x"}"#);
        assert!(matches!(bad, SdkIncoming::Malformed { id: Some(3), .. }));
        let bad = parse_frame(r#"{"jsonrpc":"2.0","method":"x"}"#);
        assert!(matches!(bad, SdkIncoming::Malformed { id: None, .. }));
        let bad = parse_frame(r#"{"jsonrpc":"2.0","id":1}"#);
        assert!(matches!(bad, SdkIncoming::Malformed { id: Some(1), .. }));
    }

    // ── 桩网关（确定性；取消经标志位）────────────────────────────────

    struct StubGateway {
        inner: Mutex<Stub>,
    }

    struct Stub {
        next_session: u64,
        generation: u64,
        max_seq: u64,
        batches: VecDeque<SdkEventBatch>,
        turn_result: SdkTurnResult,
        block_turn: bool,
        cancelled: Arc<AtomicBool>,
        calls: Vec<(String, u64)>,
    }

    impl StubGateway {
        fn new() -> Self {
            Self {
                inner: Mutex::new(Stub {
                    next_session: 1,
                    generation: 0,
                    max_seq: 0,
                    batches: VecDeque::new(),
                    turn_result: SdkTurnResult {
                        request_id: String::new(),
                        status: "stop".to_string(),
                        text: "好的".to_string(),
                        error: None,
                    },
                    block_turn: false,
                    cancelled: Arc::new(AtomicBool::new(false)),
                    calls: Vec::new(),
                }),
            }
        }

        fn with_batch(self, batch: SdkEventBatch) -> Self {
            self.inner.lock().expect("桩锁").batches.push_back(batch);
            self
        }

        fn with_blocking_turn(self) -> (Self, Arc<AtomicBool>) {
            let flag = Arc::clone(&self.inner.lock().expect("桩锁").cancelled);
            {
                let mut stub = self.inner.lock().expect("桩锁");
                stub.block_turn = true;
                stub.turn_result.status = "cancelled".to_string();
            }
            (self, flag)
        }
    }

    impl SdkGateway for StubGateway {
        fn create_session(&self, _title: Option<&str>) -> Result<String, WhError> {
            let mut stub = self.inner.lock().expect("桩锁");
            let id = format!("S-{}", stub.next_session);
            stub.next_session += 1;
            Ok(id)
        }

        fn open_session(&self, _session: &str) -> Result<SdkSessionInfo, WhError> {
            let stub = self.inner.lock().expect("桩锁");
            Ok(SdkSessionInfo {
                generation: stub.generation,
                max_seq: stub.max_seq,
            })
        }

        fn events_after(
            &self,
            session: &str,
            from_seq: u64,
            _limit: usize,
        ) -> Result<SdkEventBatch, WhError> {
            let mut stub = self.inner.lock().expect("桩锁");
            stub.calls.push((session.to_string(), from_seq));
            Ok(stub.batches.pop_front().unwrap_or(SdkEventBatch {
                events: Vec::new(),
                next_seq: from_seq,
                rewound_delta: 0,
            }))
        }

        fn run_turn(
            &self,
            _session: &str,
            _prompt: &str,
            _max_tokens: Option<u64>,
            _provider: Option<&str>,
            request_id: &str,
        ) -> Result<SdkTurnResult, WhError> {
            let (block, cancelled, mut result) = {
                let mut stub = self.inner.lock().expect("桩锁");
                stub.turn_result.request_id = request_id.to_string();
                (
                    stub.block_turn,
                    Arc::clone(&stub.cancelled),
                    stub.turn_result.clone(),
                )
            };
            if block {
                // 阻塞直至取消标志（取消竞态用例）；上限 5 s 防挂死。
                for _ in 0..500 {
                    if cancelled.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                result.status = if cancelled.load(Ordering::SeqCst) {
                    "cancelled".to_string()
                } else {
                    "stop".to_string()
                };
            }
            Ok(result)
        }

        fn cancel_turn(&self, _request_id: &str) -> Result<bool, WhError> {
            self.inner
                .lock()
                .expect("桩锁")
                .cancelled
                .store(true, Ordering::SeqCst);
            Ok(true)
        }
    }

    // ── 通道双工 ─────────────────────────────────────────────────────

    impl FrameSource for mpsc::Receiver<String> {
        fn next_line(&mut self) -> Option<String> {
            self.recv().ok()
        }
    }

    impl FrameSink for mpsc::Sender<String> {
        fn write_frame(&mut self, line: &str) -> bool {
            self.send(line.to_string()).is_ok()
        }
    }

    fn spawn_server(
        gateway: Arc<dyn SdkGateway>,
        capacity: usize,
    ) -> (
        mpsc::Sender<String>,
        mpsc::Receiver<String>,
        std::thread::JoinHandle<Result<(), WhError>>,
    ) {
        let (in_tx, in_rx) = mpsc::channel::<String>();
        let (out_tx, out_rx) = mpsc::channel::<String>();
        let handle = std::thread::spawn(move || {
            let mut source = in_rx;
            run_sdk_server(gateway, capacity, &mut source, Box::new(out_tx))
        });
        (in_tx, out_rx, handle)
    }

    fn recv_frame(out_rx: &mpsc::Receiver<String>) -> Value {
        let line = out_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("帧在时限内到达");
        serde_json::from_str(&line).expect("帧是合法 JSON")
    }

    fn initialize_frame(id: u64, version: &str) -> String {
        json!({ "jsonrpc": "2.0", "id": id, "method": "initialize",
                "params": { "protocolVersion": version } })
        .to_string()
    }

    // ── 版本协商与初始化门禁（ADR-071 决策 2.1）─────────────────────

    #[test]
    fn initialize_negotiates_and_gates_methods() {
        let (in_tx, out_rx, handle) = spawn_server(Arc::new(StubGateway::new()), 16);
        // 未 initialize 先调用 → invalid_state。
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "session.create", "params": {} })
                    .to_string(),
            )
            .expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["error"]["data"]["code"], "invalid_state");
        // 异主版本 → 错误携带服务端版本。
        in_tx.send(initialize_frame(2, "2.0")).expect("发送");
        let frame = recv_frame(&out_rx);
        assert!(
            frame["error"]["data"]["message"]
                .as_str()
                .unwrap()
                .contains("1.0"),
            "错误须携带服务端版本"
        );
        // 同主版本 → 协商成功。
        in_tx.send(initialize_frame(3, "1.0")).expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["result"]["protocolVersion"], "1.0");
        assert_eq!(frame["result"]["server"], "whcode");
        // 会话创建在协商后可用。
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 4, "method": "session.create", "params": {} })
                    .to_string(),
            )
            .expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["result"]["session"], "S-1");
        drop(in_tx);
        handle.join().expect("服务端优雅退出").expect("无错误");
    }

    #[test]
    fn unknown_method_maps_to_method_not_found() {
        let (in_tx, out_rx, handle) = spawn_server(Arc::new(StubGateway::new()), 16);
        in_tx.send(initialize_frame(1, "1.0")).expect("发送");
        let _ = recv_frame(&out_rx);
        in_tx
            .send(json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" }).to_string())
            .expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["error"]["code"], -32601);
        drop(in_tx);
        handle.join().expect("服务端退出").expect("无错误");
    }

    // ── 会话与 turn 流（焦点会话）────────────────────────────────────

    #[test]
    fn create_open_and_run_flow() {
        let (in_tx, out_rx, handle) = spawn_server(Arc::new(StubGateway::new()), 16);
        in_tx.send(initialize_frame(1, "1.0")).expect("发送");
        let _ = recv_frame(&out_rx);
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 2, "method": "session.create", "params": {} })
                    .to_string(),
            )
            .expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["result"]["session"], "S-1");
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 3, "method": "turn.run",
                        "params": { "prompt": "你好" } })
                .to_string(),
            )
            .expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["result"]["status"], "stop");
        assert_eq!(frame["result"]["text"], "好的");
        assert!(frame["result"]["requestId"].as_str().is_some());
        drop(in_tx);
        handle.join().expect("服务端退出").expect("无错误");
    }

    /// 活跃槽互斥（不变量 3）：第一个 turn 阻塞期间，第二个 turn.run 立即 busy。
    #[test]
    fn second_run_is_busy_while_first_active() {
        let (gateway, _flag) = StubGateway::new().with_blocking_turn();
        let (in_tx, out_rx, handle) = spawn_server(Arc::new(gateway), 16);
        in_tx.send(initialize_frame(1, "1.0")).expect("发送");
        let _ = recv_frame(&out_rx);
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 2, "method": "session.create", "params": {} })
                    .to_string(),
            )
            .expect("发送");
        let _ = recv_frame(&out_rx);
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 3, "method": "turn.run", "params": { "prompt": "a" } })
                    .to_string(),
            )
            .expect("发送");
        // 等第一个 turn 进入阻塞后发起第二个 → busy。
        std::thread::sleep(Duration::from_millis(150));
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 4, "method": "turn.run", "params": { "prompt": "b" } })
                    .to_string(),
            )
            .expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["error"]["code"], -32000, "第二个 turn.run 必须 busy");
        // 取消第一个 → 终态 cancelled 响应到达（取消竞态路径）。
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 5, "method": "turn.cancel", "params": {} })
                    .to_string(),
            )
            .expect("发送");
        let mut seen_cancel_ack = false;
        let mut seen_cancelled_turn = false;
        for _ in 0..2 {
            let frame = recv_frame(&out_rx);
            if frame
                .get("result")
                .is_some_and(|r| r.get("cancelled").is_some())
            {
                assert_eq!(frame["result"]["cancelled"], true);
                seen_cancel_ack = true;
            } else if frame
                .get("result")
                .is_some_and(|r| r.get("status").is_some())
            {
                assert_eq!(frame["result"]["status"], "cancelled");
                seen_cancelled_turn = true;
            }
        }
        assert!(seen_cancel_ack && seen_cancelled_turn);
        drop(in_tx);
        handle.join().expect("服务端退出").expect("无错误");
    }

    /// 取消无活跃请求：cancelled=false（非错误）。
    #[test]
    fn cancel_without_active_turn_is_false() {
        let (in_tx, out_rx, handle) = spawn_server(Arc::new(StubGateway::new()), 16);
        in_tx.send(initialize_frame(1, "1.0")).expect("发送");
        let _ = recv_frame(&out_rx);
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 2, "method": "turn.cancel", "params": {} })
                    .to_string(),
            )
            .expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["result"]["cancelled"], false);
        drop(in_tx);
        handle.join().expect("服务端退出").expect("无错误");
    }

    // ── 事件流：seq 连续、世代推进、游标重放（不变量 1/2）────────────

    #[test]
    fn follow_streams_frames_with_cursor_and_generation() {
        let gateway = Arc::new(
            StubGateway::new()
                .with_batch(SdkEventBatch {
                    events: vec![
                        SdkEventFrame {
                            seq: 1,
                            kind: "TurnFinished".to_string(),
                            payload: json!({}),
                        },
                        SdkEventFrame {
                            seq: 2,
                            kind: REWIND_KIND.to_string(),
                            payload: json!({}),
                        },
                    ],
                    next_seq: 2,
                    rewound_delta: 1,
                })
                .with_batch(SdkEventBatch {
                    events: vec![SdkEventFrame {
                        seq: 3,
                        kind: "TurnStarted".to_string(),
                        payload: json!({}),
                    }],
                    next_seq: 3,
                    rewound_delta: 0,
                }),
        );
        let (in_tx, out_rx, handle) = spawn_server(gateway, 64);
        in_tx.send(initialize_frame(1, "1.0")).expect("发送");
        let _ = recv_frame(&out_rx);
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 2, "method": "session.create", "params": {} })
                    .to_string(),
            )
            .expect("发送");
        let _ = recv_frame(&out_rx);
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 3, "method": "events.follow",
                        "params": { "fromSeq": 0 } })
                .to_string(),
            )
            .expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["result"]["nextSeq"], 0);
        // 事件帧：seq 1、2（gen 0）→ generation 帧 → seq 3（gen 1）——共 4 帧。
        let mut seqs = Vec::new();
        let mut generations = Vec::new();
        let mut saw_generation_frame = false;
        for _ in 0..4 {
            let frame = recv_frame(&out_rx);
            if frame.get("method") == Some(&json!("event")) {
                seqs.push(frame["params"]["seq"].as_u64().expect("seq"));
                generations.push(frame["params"]["generation"].as_u64().expect("generation"));
            } else if frame.get("method") == Some(&json!("generation")) {
                saw_generation_frame = true;
            }
        }
        assert_eq!(seqs, vec![1, 2, 3], "seq 严格递增（不变量 1）");
        assert_eq!(
            generations,
            vec![0, 0, 1],
            "世代随 SessionRewound 推进（不变量 2）"
        );
        assert!(saw_generation_frame, "世代推进有显式通知");
        drop(in_tx);
        handle.join().expect("服务端退出").expect("无错误");
    }

    /// 游标重放：fromSeq 透传给网关（客户端断连重连补齐不重不漏的判据）。
    #[test]
    fn follow_passes_cursor_to_gateway() {
        let gateway = Arc::new(StubGateway::new());
        let (in_tx, out_rx, handle) = spawn_server(gateway, 16);
        in_tx.send(initialize_frame(1, "1.0")).expect("发送");
        let _ = recv_frame(&out_rx);
        in_tx
            .send(
                json!({ "jsonrpc": "2.0", "id": 2, "method": "events.follow",
                        "params": { "session": "S-9", "fromSeq": 5 } })
                .to_string(),
            )
            .expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["result"]["nextSeq"], 5);
        drop(in_tx);
        handle.join().expect("服务端退出").expect("无错误");
    }

    // ── 边界：超长帧／背压关闭 ───────────────────────────────────────

    #[test]
    fn overlong_line_is_rejected_without_crash() {
        let (in_tx, out_rx, handle) = spawn_server(Arc::new(StubGateway::new()), 16);
        let big = "x".repeat(MAX_LINE_BYTES + 1);
        in_tx.send(big).expect("发送");
        let frame = recv_frame(&out_rx);
        assert_eq!(frame["error"]["code"], -32600);
        drop(in_tx);
        handle.join().expect("服务端退出").expect("无错误");
    }

    /// 背压（不变量 5）：容量 1 ＋ 首帧后写失败的 sink → 慢消费者关闭序列触发，
    /// 服务端优雅返回（不挂死）。
    #[test]
    fn slow_consumer_triggers_close() {
        struct FailAfterFirst {
            first: Mutex<bool>,
        }
        impl FrameSink for FailAfterFirst {
            fn write_frame(&mut self, _line: &str) -> bool {
                let mut first = self.first.lock().expect("锁");
                if *first {
                    *first = false;
                    true
                } else {
                    false
                }
            }
        }
        let gateway = Arc::new(
            StubGateway::new().with_batch(SdkEventBatch {
                events: (1..=5)
                    .map(|seq| SdkEventFrame {
                        seq,
                        kind: "Tick".to_string(),
                        payload: json!({}),
                    })
                    .collect(),
                next_seq: 5,
                rewound_delta: 0,
            }),
        );
        // 直接驱动：FailAfterFirst sink 首帧后写失败 → 泵遇通道满 → 关闭序列。
        let lines = Mutex::new(VecDeque::from(vec![
            initialize_frame(1, "1.0"),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "events.follow",
                    "params": { "fromSeq": 0 } })
            .to_string(),
        ]));
        struct ScriptedSource {
            lines: Mutex<VecDeque<String>>,
        }
        impl FrameSource for ScriptedSource {
            fn next_line(&mut self) -> Option<String> {
                self.lines.lock().expect("源锁").pop_front()
            }
        }
        let mut source = ScriptedSource { lines };
        let joined = run_sdk_server(
            gateway,
            1,
            &mut source,
            Box::new(FailAfterFirst {
                first: Mutex::new(true),
            }),
        );
        assert!(joined.is_ok(), "慢消费者关闭路径必须优雅返回");
    }
}
