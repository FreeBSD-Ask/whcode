//! # 阻塞式 HTTP 客户端（T163—T166、FR-201／FR-206／FR-207／FR-616、ADR-031）
//!
//! 本模块把 `ureq::Agent`（仅 `http` feature）变成一次可观察的真实模型调用：
//!
//! - **构造**（T163）：三元组时限（连接／读取／总时限）与信任根注入点
//!   （[`build_agent`]、[`tls_config_from_roots`]；后者与 T188 的证书库衔接）；
//! - **鉴权**（T162）：剖面 [`AuthScheme`](crate::http::profile::AuthScheme) 求值后注入头
//!   或查询参数，错误与日志不含密钥；
//! - **发送与读取**（T163／T166）：状态归一用 [`errors::normalize_status`]，正文逐帧
//!   **有界**读取（[`frames`](crate::http::frames) 的 1 MiB 帧／64 MiB 响应上限），
//!   网络错误映射到 [`WhError`]；
//! - **归一**（T164）：[`ChunkNormalizer`] 把帧语义变成序号连续的 [`LlmChunk`] 流；
//! - **传输可观察**（T165）：[`TransportTracker`] 接入连接生命周期（连接／断开／字节／
//!   RTT／原因码），快照经 [`Provider::transport_snapshot`] 暴露；事件记录进入**有界**
//!   环形缓冲（[`MAX_TRANSPORT_EVENTS`]），由调用方经 [`HttpClient::take_transport_events`]
//!   取出写入事件体系（FR-616）；
//! - **取消与时限**（T166）：取消在**下一个帧边界**生效（阻塞读取无法中途打断，最长再等
//!   读取时限），该时机写入错误 `message` 与 `details`；总时限由代理层与读循环双重执行；
//! - **重试**（T166）：只有幂等的探测请求（[`HttpClient::probe_get`]）允许有界重试
//!   （最多 2 次、1 s／2 s 退避），**流式生成请求从不自动重放**（ADR-031 决策 8）。
//!
//! ## 已知限制（不伪造）
//!
//! 1. **按连接聚合退化为按客户端聚合**：`ureq` 不暴露底层连接标识，连接池复用也不可见；
//!    因此 `connection_id` 为剖面名，字节与 RTT 在同一个客户端实例内累计。若将来需要精确
//!    分连接，必须自建连接层，而不是在此猜测。
//! 2. **RTT 是“请求发出到响应头到达”的耗时**，不是 ICMP 或 TCP 层往返时延（FR-616 只要求
//!    可查询的采样值，本实现如实标注测点）。
//! 3. **`0` 表示不限**：三元时限任一为 0 时不向传输层下发该档位超时（剖面默认值见
//!    [`ProviderProfile`]；ADR-031 决策 7）。
//! 4. **不跟随重定向**：避免凭据被转发到第三主机；3xx 按非 2xx 归一为失败。
//! 5. **同步客户端**：一次调用占用发起线程（ADR-031 决策 3），不新增常驻线程。

use std::collections::VecDeque;
use std::io::{BufReader, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use whcode_domain::{ErrorCode, EventRecord, WhError};

use crate::http::chunks::{ChunkNormalizer, EOF_FINISH_REASON, PushOutcome};
use crate::http::errors;
use crate::http::frames::{Budget, next_frame};
use crate::http::profile::{self, AuthInjection, FrameFormat, ProviderProfile};
use crate::http::protocols::{self, WireProtocol};
use crate::provider::{
    CancelToken, ChunkStream, LlmChunk, LlmRequest, Provider, ProviderCapabilities,
};
use crate::transport_events::{TransportSnapshot, TransportTracker};

/// 传输事件环形缓冲上限（有界；超出时丢弃最旧并累计 [`TransportEvents::dropped`]）。
pub const MAX_TRANSPORT_EVENTS: usize = 64;

/// 探测请求允许的最大重试次数（不含首次尝试；ADR-031 决策 8 的硬上限）。
pub const MAX_PROBE_RETRIES: u32 = 2;

/// 探测重试的默认退避（1 s、2 s）。
pub const PROBE_BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];

/// 探测响应正文上限（1 MiB；模型／能力清单足够，超出即报错，不静默截断）。
pub const MAX_PROBE_BODY_BYTES: usize = 1 << 20;

/// 探测重试策略（有界；生效重试次数不会超过 [`MAX_PROBE_RETRIES`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// 最大重试次数（不含首次尝试）。
    pub max_retries: u32,
    /// 每次重试前的等待（索引 0 对应第 1 次重试）。
    pub backoff: [Duration; MAX_PROBE_RETRIES as usize],
}

impl RetryPolicy {
    /// ADR-031 决策 8 的默认策略：最多 2 次，退避 1 s、2 s。
    #[must_use]
    pub const fn standard() -> Self {
        Self {
            max_retries: MAX_PROBE_RETRIES,
            backoff: PROBE_BACKOFF,
        }
    }

    /// 不重试（流式生成路径的语义；也用于测试）。
    #[must_use]
    pub const fn none() -> Self {
        Self {
            max_retries: 0,
            backoff: [Duration::ZERO, Duration::ZERO],
        }
    }

    /// 本次实际尝试次数（1 + 生效重试次数，后者有界）。
    fn attempts(&self) -> u32 {
        self.max_retries.min(MAX_PROBE_RETRIES).saturating_add(1)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::standard()
    }
}

/// 一次取出的传输事件批次（有界）。
#[derive(Debug, Clone)]
pub struct TransportEvents {
    /// 事件记录（`ProgressChanged`，载荷 `transport`）。
    pub events: Vec<EventRecord>,
    /// 因环形缓冲上限被丢弃的最旧事件数（可观测，不静默）。
    pub dropped: u64,
}

/// 传输失败分类（错误消息只给分类，不回显请求 URI——URI 可能含查询参数密钥）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailure {
    /// 超时（连接／读取／总时限）。
    Timeout,
    /// 连接失败（DNS、拒绝、网络不可达）。
    Connect,
    /// TLS 握手或证书校验失败。
    Tls,
    /// 响应不符合 HTTP 语义，或请求无法编码。
    Protocol,
    /// 其他读写失败。
    Io,
}

impl TransportFailure {
    /// 稳定标识（进错误 `details.failure` 与断连原因码）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::Tls => "tls",
            Self::Protocol => "protocol",
            Self::Io => "io",
        }
    }
}

struct TransportState {
    tracker: TransportTracker,
    events: VecDeque<EventRecord>,
    dropped_events: u64,
}

impl TransportState {
    fn new(connection_id: String) -> Self {
        Self {
            tracker: TransportTracker::new(connection_id),
            events: VecDeque::new(),
            dropped_events: 0,
        }
    }

    fn push_event(&mut self, event: EventRecord) {
        if self.events.len() >= MAX_TRANSPORT_EVENTS {
            self.events.pop_front();
            self.dropped_events = self.dropped_events.saturating_add(1);
        }
        self.events.push_back(event);
    }
}

fn lock_transport(state: &Arc<Mutex<TransportState>>) -> MutexGuard<'_, TransportState> {
    // 锁中毒只可能来自同进程 panic；传输计数不应因此不可用，直接取回内部状态。
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn with_transport<T>(
    state: &Arc<Mutex<TransportState>>,
    action: impl FnOnce(&mut TransportState) -> T,
) -> T {
    let mut guard = lock_transport(state);
    action(&mut guard)
}

fn record_connected(state: &Arc<Mutex<TransportState>>) {
    let now = now_ms();
    with_transport(state, |guard| {
        let event = guard.tracker.record_connected(now);
        guard.push_event(event);
    });
}

fn record_disconnect(state: &Arc<Mutex<TransportState>>, reason: &str) {
    let now = now_ms();
    with_transport(state, |guard| {
        let event = guard.tracker.record_disconnect(reason, now);
        guard.push_event(event);
    });
}

fn record_reconnect_attempt(state: &Arc<Mutex<TransportState>>) {
    let now = now_ms();
    with_transport(state, |guard| {
        let event = guard.tracker.record_reconnect_attempt(now);
        guard.push_event(event);
    });
}

fn record_sent(state: &Arc<Mutex<TransportState>>, bytes: u64) {
    with_transport(state, |guard| guard.tracker.record_sent(bytes));
}

fn record_received(state: &Arc<Mutex<TransportState>>, bytes: u64) {
    with_transport(state, |guard| guard.tracker.record_received(bytes));
}

fn record_rtt(state: &Arc<Mutex<TransportState>>, rtt: Duration) {
    with_transport(state, |guard| {
        guard
            .tracker
            .record_rtt(u64::try_from(rtt.as_millis()).unwrap_or(u64::MAX));
    });
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// 由信任根 DER 构造 `ureq` 的 TLS 配置（信任根注入点）。
///
/// T188 的 `http::tls::load_roots` 产出根集合后经此接入，再经 [`build_agent`] 的 `tls`
/// 参数进入客户端；本函数不关闭任何校验，也不存在“信任所有”档位。
///
/// # Errors
///
/// 信任根为 0 份时返回 [`ErrorCode::InvalidState`]（ADR-040 决策 2）。
pub fn tls_config_from_roots(roots: &[Vec<u8>]) -> Result<ureq::tls::TlsConfig, WhError> {
    if roots.is_empty() {
        return Err(WhError::invalid_state(
            "生效信任根为 0 份：不得以空信任集构造出站客户端（ADR-040 决策 2）；请先装配证书库",
        ));
    }
    let certs: Vec<ureq::tls::Certificate<'static>> = roots
        .iter()
        .map(|der| ureq::tls::Certificate::from_der(der).to_owned())
        .collect();
    Ok(ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::from(certs))
        .build())
}

/// 按剖面的三元时限构造 `ureq::Agent`（T163）。
///
/// 恒提供的行为：非 2xx 不作为传输错误（由本模块归一）、不跟随重定向（避免凭据被转发到
/// 第三主机）、三元时限显式下发。`tls` 是信任根注入点，`None` 表示 `ureq` 默认的 rustls
/// 内置根集。
#[must_use]
pub fn build_agent(profile: &ProviderProfile, tls: Option<ureq::tls::TlsConfig>) -> ureq::Agent {
    let (connect, read, total) = profile.timeouts();
    let mut builder = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(true)
        .timeout_connect(optional_timeout(connect))
        .timeout_per_read(optional_timeout(read))
        .timeout_global(optional_timeout(total))
        .user_agent(concat!("whcode/", env!("CARGO_PKG_VERSION")));
    if let Some(tls) = tls {
        builder = builder.tls_config(tls);
    }
    ureq::Agent::new_with_config(builder.build())
}

/// `0` 表示不限（不下发该档位超时）。
fn optional_timeout(value: Duration) -> Option<Duration> {
    (!value.is_zero()).then_some(value)
}

/// 真实供应商客户端（同步；一次调用占用发起线程）。
///
/// 构造即求值凭据与校验端点：**只在用户显式选择剖面时构造**，不得用于启动、`doctor`、
/// `serve` 或枚举路径（ADR-031 决策 4：无剖面即无请求）。`Debug` 只打印剖面名、协议与
/// 注入项**数量**，鉴权头／查询参数值（可能含密钥）恒不输出。
pub struct HttpClient {
    profile: ProviderProfile,
    protocol: &'static dyn WireProtocol,
    agent: ureq::Agent,
    auth: Arc<AuthInjection>,
    transport: Arc<Mutex<TransportState>>,
    retry: RetryPolicy,
    unknown_frames: Arc<AtomicU64>,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只打印剖面名、协议与注入项数量；鉴权值（可能含密钥）恒不输出。
        f.debug_struct("HttpClient")
            .field("provider", &self.profile.name)
            .field("protocol", &self.protocol.name())
            .field("auth_headers", &self.auth.headers().len())
            .field("auth_query", &self.auth.query().len())
            .finish()
    }
}

/// 一次请求的静态部分（构造后不再变化；`body` 与头值可能含密钥，不打印）。
struct PreparedRequest {
    url: String,
    headers: Vec<(String, String)>,
    query: Vec<(String, String)>,
    body: String,
}

impl HttpClient {
    /// 以剖面构造，使用 `ureq` 默认 TLS 信任根。
    ///
    /// # Errors
    ///
    /// 端点非法、模型为空或含空白／控制字符、凭据缺失或为空、附加头／查询参数非法、
    /// 与鉴权项重复时返回结构化错误；错误不含密钥值。
    pub fn new(profile: ProviderProfile) -> Result<Self, WhError> {
        Self::with_tls(profile, None)
    }

    /// 以剖面与注入的 TLS 配置构造（信任根注入点；T188 证书库经
    /// [`tls_config_from_roots`] 接入）。
    ///
    /// # Errors
    ///
    /// 同 [`HttpClient::new`]。
    pub fn with_tls(
        profile: ProviderProfile,
        tls: Option<ureq::tls::TlsConfig>,
    ) -> Result<Self, WhError> {
        profile.validate_endpoint()?;
        if profile.model.trim().is_empty() {
            return Err(WhError::invalid_state(
                "非法剖面：model 不得为空（请求体与请求路径都使用它）",
            ));
        }
        if profile
            .model
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(WhError::invalid_state(
                "非法剖面：model 含空白或控制字符；模型标识不得跨行或带空格",
            ));
        }
        let auth = profile.auth.inject()?;
        let protocol = protocols::registry(profile.protocol);
        validate_profile_transport(&profile, &auth, protocol)?;

        let connection_id = profile.name.clone();
        let agent = build_agent(&profile, tls);
        Ok(Self {
            profile,
            protocol,
            agent,
            auth: Arc::new(auth),
            transport: Arc::new(Mutex::new(TransportState::new(connection_id))),
            retry: RetryPolicy::default(),
            unknown_frames: Arc::new(AtomicU64::new(0)),
        })
    }

    /// 剖面（只读）。
    #[must_use]
    pub fn profile(&self) -> &ProviderProfile {
        &self.profile
    }

    /// 线协议适配器（只读）。
    #[must_use]
    pub fn protocol(&self) -> &'static dyn WireProtocol {
        self.protocol
    }

    /// 设置探测重试策略（流式生成始终不重试）。
    #[must_use]
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// 传输层只读快照（FR-616；同 [`Provider::transport_snapshot`]）。
    #[must_use]
    pub fn snapshot(&self) -> TransportSnapshot {
        with_transport(&self.transport, |guard| guard.tracker.snapshot())
    }

    /// 取出累计的传输事件（有界环形缓冲；调用方负责写入事件体系）。
    #[must_use]
    pub fn take_transport_events(&self) -> TransportEvents {
        with_transport(&self.transport, |guard| TransportEvents {
            events: guard.events.drain(..).collect(),
            dropped: guard.dropped_events,
        })
    }

    /// 累计未识别帧数（FR-206／FR-208：可查询，不静默丢弃）。
    #[must_use]
    pub fn unknown_frames(&self) -> u64 {
        self.unknown_frames.load(Ordering::Relaxed)
    }

    /// 幂等探测请求（GET；最多重试 [`MAX_PROBE_RETRIES`] 次，ADR-031 决策 8）。
    ///
    /// 只对连接失败（含连接／读取超时）与 5xx 重试；4xx、429（由调用方按 `retry_after_ms`
    /// 决定）与协议错误不重试。每次重试都写入传输事件（重连尝试），探测从不读取流式正文，
    /// 因此不存在“已产生增量仍重放”的重复计费风险。
    ///
    /// # Errors
    ///
    /// 端点不可用、状态非 2xx、正文超过 [`MAX_PROBE_BODY_BYTES`] 或传输失败时返回归一化错误。
    pub fn probe_get(&self, request_id: &str, path: &str) -> Result<String, WhError> {
        let attempts = self.retry.attempts();
        let mut last_error = None;
        for attempt in 0..attempts {
            if attempt > 0 {
                let delay = self.retry.backoff[(attempt - 1) as usize];
                record_reconnect_attempt(&self.transport);
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
            }
            match self.send_probe_once(request_id, path) {
                Ok(body) => return Ok(body),
                Err(err) => {
                    if !probe_failure_is_retryable(&err) || attempt + 1 >= attempts {
                        return Err(err);
                    }
                    last_error = Some(err);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            WhError::new(
                ErrorCode::Internal,
                "探测请求未执行：重试策略为 0 次尝试（调用方不应到达此处）",
            )
        }))
    }

    fn send_probe_once(&self, request_id: &str, path: &str) -> Result<String, WhError> {
        let url = endpoint_url(&self.profile.base_url, path);
        let mut query = self.profile.query.clone();
        query.extend(self.auth.query().iter().cloned());
        let headers = self.auth.headers();
        let sent_at = Instant::now();
        let response = match decorate(self.agent.get(&url), headers, &query).call() {
            Ok(response) => response,
            Err(err) => {
                let failure = classify_ureq_error(&err);
                record_disconnect(&self.transport, failure.as_str());
                return Err(transport_error(
                    failure,
                    &self.profile.name,
                    self.protocol.name(),
                    request_id,
                    "connect",
                ));
            }
        };
        record_connected(&self.transport);
        record_rtt(&self.transport, sent_at.elapsed());
        record_sent(&self.transport, header_wire_bytes(headers));

        let status = response.status().as_u16();
        let request_id = header_value(response.headers(), "x-request-id")
            .unwrap_or_else(|| request_id.to_string());
        let retry_after =
            errors::retry_after_ms(header_value(response.headers(), "retry-after").as_deref());
        let body = read_bounded_body(response.into_body(), MAX_PROBE_BODY_BYTES)?;
        record_received(&self.transport, body.len() as u64);
        if !(200..=299).contains(&status) {
            record_disconnect(&self.transport, &format!("status_{status}"));
            let message = self.auth.redact(&self.protocol.error_message(&body));
            return Err(errors::normalize_status(
                status,
                &message,
                &self.profile.name,
                self.protocol.name(),
                &request_id,
                retry_after,
            ));
        }
        record_disconnect(&self.transport, "eof");
        Ok(body)
    }

    /// 发送流式请求并返回 chunk 流（DNS、连接、发送与响应头都在本次调用内完成）。
    fn open_stream(
        &self,
        request: &LlmRequest,
        cancel: &CancelToken,
    ) -> Result<ChunkStream, WhError> {
        if cancel.is_cancelled() {
            return Err(cancelled_error(
                &self.profile.name,
                self.protocol.name(),
                &request.request_id,
                "请求发送前",
                self.profile.read_timeout_ms,
            ));
        }
        let prepared = self.prepare(request)?;
        let sent_at = Instant::now();
        let response = match decorate(
            self.agent.post(&prepared.url),
            &prepared.headers,
            &prepared.query,
        )
        .send(prepared.body.as_str())
        {
            Ok(response) => response,
            Err(err) => {
                let failure = classify_ureq_error(&err);
                record_disconnect(&self.transport, failure.as_str());
                return Err(transport_error(
                    failure,
                    &self.profile.name,
                    self.protocol.name(),
                    &request.request_id,
                    "connect",
                ));
            }
        };
        record_connected(&self.transport);
        record_rtt(&self.transport, sent_at.elapsed());
        record_sent(
            &self.transport,
            request_wire_bytes(&prepared.headers, prepared.body.len()),
        );

        let request_id = header_value(response.headers(), "x-request-id")
            .unwrap_or_else(|| request.request_id.clone());
        let status = response.status().as_u16();
        if !(200..=299).contains(&status) {
            let retry_after =
                errors::retry_after_ms(header_value(response.headers(), "retry-after").as_deref());
            let body = read_bounded_body(response.into_body(), errors::MAX_ERROR_BODY_BYTES)
                .unwrap_or_default();
            record_disconnect(&self.transport, &format!("status_{status}"));
            let message = self.auth.redact(&self.protocol.error_message(&body));
            return Err(errors::normalize_status(
                status,
                &message,
                &self.profile.name,
                self.protocol.name(),
                &request_id,
                retry_after,
            ));
        }

        let total_timeout = Duration::from_millis(self.profile.total_timeout_ms);
        let deadline = optional_timeout(total_timeout).map(|total| sent_at + total);
        let reader = CountingReader::new(
            Box::new(response.into_body().into_reader()),
            Arc::clone(&self.transport),
        );
        Ok(ChunkStream {
            inner: Box::new(HttpChunkIter {
                reader: BufReader::new(reader),
                format: self.protocol.frame_format(),
                budget: Budget::default(),
                protocol: self.protocol,
                normalizer: ChunkNormalizer::new(),
                cancel: cancel.clone(),
                deadline,
                read_timeout: Duration::from_millis(self.profile.read_timeout_ms),
                total_timeout,
                provider: self.profile.name.clone(),
                protocol_name: self.protocol.name(),
                request_id,
                transport: Arc::clone(&self.transport),
                unknown_frames: Arc::clone(&self.unknown_frames),
                done: false,
            }),
        })
    }

    fn prepare(&self, request: &LlmRequest) -> Result<PreparedRequest, WhError> {
        let path = self.protocol.request_path(&self.profile);
        let url = endpoint_url(&self.profile.base_url, &path);
        let body = self.protocol.request_body(request, &self.profile)?;
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        headers.push((
            "accept".to_string(),
            accept_header(self.protocol.frame_format()).to_string(),
        ));
        headers.extend(self.protocol.extra_headers());
        headers.extend(self.profile.extra_headers.iter().cloned());
        headers.extend(self.auth.headers().iter().cloned());
        let mut query = self.profile.query.clone();
        query.extend(self.auth.query().iter().cloned());
        Ok(PreparedRequest {
            url,
            headers,
            query,
            body,
        })
    }
}

impl Provider for HttpClient {
    fn name(&self) -> &str {
        &self.profile.name
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            cancel: true,
            // 协议层在供应商给出用量时原样上送；缺失时由 pump 走本地估算（FR-207）。
            usage_reporting: true,
        }
    }

    fn is_available(&self) -> bool {
        // 构造即校验端点与凭据：能构造出来的实例即为可用。
        true
    }

    fn execute(&self, request: &LlmRequest, cancel: &CancelToken) -> Result<ChunkStream, WhError> {
        self.open_stream(request, cancel)
    }

    fn transport_snapshot(&self) -> Option<TransportSnapshot> {
        Some(self.snapshot())
    }
}

/// 校验剖面里所有进入请求的头与查询参数；与鉴权项重复会被拒绝（避免鉴权值被覆盖）。
fn validate_profile_transport(
    profile: &ProviderProfile,
    auth: &AuthInjection,
    protocol: &'static dyn WireProtocol,
) -> Result<(), WhError> {
    for (name, value) in &profile.extra_headers {
        profile::validate_header_name(name)?;
        profile::validate_header_value(name, value)?;
        if auth
            .headers()
            .iter()
            .any(|(auth_name, _)| auth_name.eq_ignore_ascii_case(name))
        {
            return Err(WhError::invalid_state(format!(
                "非法剖面：extra_headers 重复设置了鉴权头 {name}；请只保留 auth 一处\
                 （重复头会让鉴权值被覆盖或被供应商拒绝）"
            )));
        }
    }
    for (name, value) in &profile.query {
        profile::validate_query_name(name)?;
        profile::validate_query_value(name, value)?;
        if auth.query().iter().any(|(auth_name, _)| auth_name == name) {
            return Err(WhError::invalid_state(format!(
                "非法剖面：query 重复设置了鉴权参数 {name}；请只保留 auth 一处"
            )));
        }
    }
    for (name, value) in protocol.extra_headers() {
        profile::validate_header_name(&name)?;
        profile::validate_header_value(&name, &value)?;
    }
    Ok(())
}

/// 逐帧读取器：把读到的字节数记入传输追踪器，并保留最后一次读失败的分类。
///
/// 保留分类是必要的：`frames` 模块把 IO 错误包装成 [`WhError`] 时会丢掉原始
/// `std::io::Error`，而“读取超时”与“连接断开”的归一化结果不同（FR-616）。
struct CountingReader {
    inner: Box<dyn Read + Send>,
    transport: Arc<Mutex<TransportState>>,
    last_failure: Option<TransportFailure>,
}

impl CountingReader {
    fn new(inner: Box<dyn Read + Send>, transport: Arc<Mutex<TransportState>>) -> Self {
        Self {
            inner,
            transport,
            last_failure: None,
        }
    }

    fn take_failure(&mut self) -> Option<TransportFailure> {
        self.last_failure.take()
    }
}

impl Read for CountingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.inner.read(buf) {
            Ok(read) => {
                if read > 0 {
                    record_received(&self.transport, read as u64);
                }
                Ok(read)
            }
            Err(err) => {
                self.last_failure = Some(classify_io_error(&err));
                Err(err)
            }
        }
    }
}

/// 流式读取迭代器：帧读取 → 归一 → chunk；取消与总时限在**每个帧边界**检查。
struct HttpChunkIter {
    reader: BufReader<CountingReader>,
    format: FrameFormat,
    budget: Budget,
    protocol: &'static dyn WireProtocol,
    normalizer: ChunkNormalizer,
    cancel: CancelToken,
    deadline: Option<Instant>,
    read_timeout: Duration,
    total_timeout: Duration,
    provider: String,
    protocol_name: &'static str,
    request_id: String,
    transport: Arc<Mutex<TransportState>>,
    unknown_frames: Arc<AtomicU64>,
    done: bool,
}

impl HttpChunkIter {
    fn stop(&mut self, reason: &str) {
        if self.done {
            return;
        }
        self.done = true;
        self.unknown_frames.fetch_add(
            self.normalizer.unknown_frames(),
            std::sync::atomic::Ordering::Relaxed,
        );
        record_disconnect(&self.transport, reason);
    }
}

impl Iterator for HttpChunkIter {
    type Item = Result<LlmChunk, WhError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.done {
                return None;
            }
            if self.cancel.is_cancelled() {
                let err = cancelled_error(
                    &self.provider,
                    self.protocol_name,
                    &self.request_id,
                    "读取循环",
                    u64::try_from(self.read_timeout.as_millis()).unwrap_or(u64::MAX),
                );
                self.stop("cancelled");
                return Some(Err(err));
            }
            if let Some(deadline) = self.deadline
                && Instant::now() >= deadline
            {
                let err = total_timeout_error(
                    &self.provider,
                    self.protocol_name,
                    &self.request_id,
                    self.total_timeout,
                    self.read_timeout,
                );
                self.stop("timeout");
                return Some(Err(err));
            }
            let protocol = self.protocol;
            match next_frame(&mut self.reader, self.format, &mut self.budget) {
                Ok(Some(frame)) => match self.normalizer.push(protocol.decode(&frame)) {
                    PushOutcome::Chunk(chunk) => return Some(Ok(chunk)),
                    PushOutcome::Idle | PushOutcome::Unknown => {}
                    PushOutcome::Error(message) => {
                        let err = protocol_stream_error(
                            &self.provider,
                            self.protocol_name,
                            &self.request_id,
                            message,
                        );
                        self.stop("protocol_error");
                        return Some(Err(err));
                    }
                },
                Ok(None) => {
                    let final_chunk = self.normalizer.finish(EOF_FINISH_REASON);
                    self.stop("eof");
                    return final_chunk.map(Ok);
                }
                Err(err) => {
                    let failure = self.reader.get_mut().take_failure();
                    let err = match failure {
                        Some(TransportFailure::Timeout) => read_timeout_error(
                            &self.provider,
                            self.protocol_name,
                            &self.request_id,
                            self.read_timeout,
                        ),
                        other => attach_read_details(
                            err,
                            &self.provider,
                            self.protocol_name,
                            &self.request_id,
                            other,
                        ),
                    };
                    let reason = match failure {
                        Some(TransportFailure::Timeout) => "timeout",
                        Some(TransportFailure::Connect) => "connect",
                        Some(TransportFailure::Tls) => "tls",
                        Some(TransportFailure::Protocol) => "protocol_error",
                        Some(TransportFailure::Io) | None => "io_error",
                    };
                    self.stop(reason);
                    return Some(Err(err));
                }
            }
        }
    }
}

fn endpoint_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if path.is_empty() {
        base.to_string()
    } else if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

fn accept_header(format: FrameFormat) -> &'static str {
    match format {
        FrameFormat::Sse => "text/event-stream",
        FrameFormat::Ndjson => "application/x-ndjson",
        FrameFormat::JsonArray => "application/json",
    }
}

/// 逐项写入头与查询参数（`ureq` 对重复名照写多行，HTTP 语义允许；配置层已拦截鉴权重复）。
fn decorate<B>(
    request: ureq::RequestBuilder<B>,
    headers: &[(String, String)],
    query: &[(String, String)],
) -> ureq::RequestBuilder<B> {
    let mut request = request;
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    for (name, value) in query {
        request = request.query(name.as_str(), value.as_str());
    }
    request
}

fn header_value(headers: &ureq::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// 请求头字节数（不含请求行与协议帧开销；FR-616 的可查询近似值）。
fn header_wire_bytes(headers: &[(String, String)]) -> u64 {
    headers
        .iter()
        .map(|(name, value)| (name.len() + value.len() + 4) as u64)
        .sum()
}

fn request_wire_bytes(headers: &[(String, String)], body_len: usize) -> u64 {
    header_wire_bytes(headers).saturating_add(body_len as u64)
}

/// 有界读取响应正文：超过 `limit` 立即返回 `parse_error`（不静默截断，FR-208）。
fn read_bounded_body(body: ureq::Body, limit: usize) -> Result<String, WhError> {
    let mut reader = body.into_reader();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = reader.read(&mut chunk).map_err(|_err| {
            WhError::new(
                ErrorCode::IoError,
                "响应正文读取失败：连接可能已中断（可重试，是否重试由调用方决定）",
            )
        })?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.len() > limit {
            return Err(WhError::parse_error(format!(
                "响应正文超过上限 {limit} 字节：不静默截断（FR-208）"
            )));
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// 传输失败分类：只看分类标签，**不**把 `ureq` 的错误文本拼进消息（`BadUri`／`Http`
/// 的文本可能含完整 URI 或头值，即可能含查询参数里的密钥）。
fn classify_ureq_error(err: &ureq::Error) -> TransportFailure {
    match err {
        ureq::Error::Timeout(_) => TransportFailure::Timeout,
        ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => TransportFailure::Connect,
        ureq::Error::Io(io) => classify_io_error(io),
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => TransportFailure::Tls,
        ureq::Error::Protocol(_)
        | ureq::Error::BodyExceedsLimit(_)
        | ureq::Error::BadUri(_)
        | ureq::Error::Http(_) => TransportFailure::Protocol,
        _ => TransportFailure::Io,
    }
}

/// IO 错误分类：先看 `kind`，再沿 `source` 链找 `ureq::Error::Timeout`（`ureq` 把超时
/// 折成 `io::Error`，只看 `kind` 会误判为普通 IO 失败）。
fn classify_io_error(err: &std::io::Error) -> TransportFailure {
    if matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        return TransportFailure::Timeout;
    }
    let mut current: Option<&(dyn std::error::Error + 'static)> = err
        .get_ref()
        .map(|inner| inner as &(dyn std::error::Error + 'static));
    // 嵌套深度由依赖决定且很小；上限 4 层防止任何意外环。
    for _ in 0..4 {
        let Some(source) = current else {
            break;
        };
        if let Some(ureq_error) = source.downcast_ref::<ureq::Error>() {
            return match ureq_error {
                ureq::Error::Timeout(_) => TransportFailure::Timeout,
                ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
                    TransportFailure::Connect
                }
                ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => {
                    TransportFailure::Tls
                }
                _ => TransportFailure::Io,
            };
        }
        if let Some(io) = source.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            )
        {
            return TransportFailure::Timeout;
        }
        current = source.source();
    }
    TransportFailure::Io
}

fn transport_error(
    failure: TransportFailure,
    provider: &str,
    protocol: &str,
    request_id: &str,
    phase: &str,
) -> WhError {
    let (code, retryable, message) = match failure {
        TransportFailure::Timeout => (
            ErrorCode::Timeout,
            true,
            "供应商请求超时：连接／读取／总时限（ADR-031 决策 7）内未完成；可重试，是否重试由调用方决定",
        ),
        TransportFailure::Connect => (
            ErrorCode::IoError,
            true,
            "供应商连接失败（DNS／拒绝／网络不可达）：请确认端点可达且未被代理拦截；不自动重放生成请求",
        ),
        TransportFailure::Tls => (
            ErrorCode::InvalidState,
            false,
            "供应商 TLS 握手失败：请检查证书与出站信任档位（ADR-040）；任何路径都不得关闭证书链与主机名校验",
        ),
        TransportFailure::Protocol => (
            ErrorCode::ParseError,
            false,
            "供应商响应不符合 HTTP 语义，或请求无法编码：请核对端点形态与剖面额外头／体；请求 URI 与凭据不回显",
        ),
        TransportFailure::Io => (
            ErrorCode::IoError,
            true,
            "供应商传输读写失败：连接可能已中断；可重试，是否重试由调用方决定",
        ),
    };
    WhError {
        code,
        message: message.to_string(),
        retryable,
        details: Some(serde_json::json!({
            "provider": provider,
            "protocol": protocol,
            "request_id": request_id,
            "failure": failure.as_str(),
            "phase": phase,
        })),
    }
}

fn read_timeout_error(
    provider: &str,
    protocol: &str,
    request_id: &str,
    read_timeout: Duration,
) -> WhError {
    WhError {
        code: ErrorCode::Timeout,
        message: format!(
            "供应商读取超时：单次读取在 {} ms 内没有返回数据（传输层已中断该次读取）；可重试，\
             是否重试由调用方决定",
            read_timeout.as_millis()
        ),
        retryable: true,
        details: Some(serde_json::json!({
            "provider": provider,
            "protocol": protocol,
            "request_id": request_id,
            "failure": "timeout",
            "phase": "read_loop",
            "read_timeout_ms": read_timeout.as_millis(),
        })),
    }
}

fn total_timeout_error(
    provider: &str,
    protocol: &str,
    request_id: &str,
    total_timeout: Duration,
    read_timeout: Duration,
) -> WhError {
    WhError {
        code: ErrorCode::Timeout,
        message: format!(
            "供应商读取超过总时限 {} ms：在帧边界停止读取（总时限覆盖连接、发送与读取；单次读取\
             另有 {} ms 上限）；可重试",
            total_timeout.as_millis(),
            read_timeout.as_millis()
        ),
        retryable: true,
        details: Some(serde_json::json!({
            "provider": provider,
            "protocol": protocol,
            "request_id": request_id,
            "failure": "timeout",
            "phase": "read_loop",
            "total_timeout_ms": total_timeout.as_millis(),
            "read_timeout_ms": read_timeout.as_millis(),
        })),
    }
}

fn cancelled_error(
    provider: &str,
    protocol: &str,
    request_id: &str,
    phase: &str,
    read_timeout_ms: u64,
) -> WhError {
    WhError {
        code: ErrorCode::Internal,
        message: format!(
            "调用已被取消：{phase}在下一个帧边界停止（取消请求于帧边界生效；阻塞读取期间无法\
             中断，最长再等读取时限 {read_timeout_ms} ms，届时以 timeout 报告）；已收到的 chunk 保留"
        ),
        retryable: false,
        details: Some(serde_json::json!({
            "provider": provider,
            "protocol": protocol,
            "request_id": request_id,
            "cancelled": true,
            "cancel_effective_at": "next_frame_boundary",
            "read_timeout_ms": read_timeout_ms,
        })),
    }
}

fn protocol_stream_error(
    provider: &str,
    protocol: &str,
    request_id: &str,
    message: String,
) -> WhError {
    WhError {
        code: ErrorCode::Internal,
        message: format!("供应商在流中途返回错误事件：{message}（协议错误不得当作正常终止）"),
        retryable: false,
        details: Some(serde_json::json!({
            "provider": provider,
            "protocol": protocol,
            "request_id": request_id,
            "failure": "protocol_event",
            "phase": "read_loop",
        })),
    }
}

fn attach_read_details(
    mut err: WhError,
    provider: &str,
    protocol: &str,
    request_id: &str,
    failure: Option<TransportFailure>,
) -> WhError {
    err.details = Some(serde_json::json!({
        "provider": provider,
        "protocol": protocol,
        "request_id": request_id,
        "failure": failure.map_or("io", TransportFailure::as_str),
        "phase": "read_loop",
    }));
    err
}

/// 只有连接失败与 5xx 可重试（ADR-031 决策 8）；429 的退避由调用方按 `retry_after_ms` 决定。
fn probe_failure_is_retryable(err: &WhError) -> bool {
    match err.code {
        ErrorCode::IoError | ErrorCode::Timeout => true,
        ErrorCode::Internal => err
            .details
            .as_ref()
            .and_then(|details| details.get("http_status"))
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|status| (500..=599).contains(&status)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{ErrorKind, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::thread::JoinHandle;

    use crate::http::profile::{AuthScheme, CredentialRef, Protocol};
    use whcode_domain::EventKind;

    /// 桩服务器的一段回包（分段写出，段前可等待）。
    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        segments: Vec<(Duration, Vec<u8>)>,
    }

    impl Reply {
        fn sse(body: &str) -> Self {
            Self {
                status: 200,
                headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                segments: vec![(Duration::ZERO, body.as_bytes().to_vec())],
            }
        }

        fn status(status: u16, headers: &[(&str, &str)], body: &str) -> Self {
            Self {
                status,
                headers: headers
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect(),
                segments: vec![(Duration::ZERO, body.as_bytes().to_vec())],
            }
        }

        fn delayed(mut self, delay: Duration) -> Self {
            if let Some(first) = self.segments.first_mut() {
                first.0 = delay;
            }
            self
        }
    }

    /// 回环桩服务器（不访问公网）：按顺序回包，记录收到的原始请求。
    struct StubServer {
        port: u16,
        stop: Arc<std::sync::atomic::AtomicBool>,
        handle: Option<JoinHandle<()>>,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl StubServer {
        fn start(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定回环端口");
            listener.set_nonblocking(true).expect("非阻塞监听");
            let port = listener.local_addr().expect("本地地址").port();
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&requests);
            let handle = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut index = 0usize;
                while !flag.load(Ordering::Relaxed) && Instant::now() < deadline {
                    match listener.accept() {
                        Ok((mut stream, _peer)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                            if let Some(raw) = read_request(&mut stream) {
                                seen.lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .push(raw);
                            }
                            let reply = replies.get(index).or_else(|| replies.last());
                            index = index.saturating_add(1);
                            if let Some(reply) = reply {
                                let _ = write_reply(&mut stream, reply);
                            }
                        }
                        Err(err) if err.kind() == ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                port,
                stop,
                handle: Some(handle),
                requests,
            }
        }

        fn base_url(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        fn requests(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl Drop for StubServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn read_request(stream: &mut TcpStream) -> Option<String> {
        let mut raw: Vec<u8> = Vec::new();
        let mut buf = [0u8; 1024];
        let header_end = loop {
            let read = stream.read(&mut buf).ok()?;
            if read == 0 {
                return None;
            }
            raw.extend_from_slice(&buf[..read]);
            if let Some(position) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
            if raw.len() > 1 << 20 {
                return None;
            }
        };
        let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
        let length = head
            .to_ascii_lowercase()
            .find("content-length:")
            .and_then(|index| {
                head[index + "content-length:".len()..]
                    .split("\r\n")
                    .next()
                    .map(str::trim)
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(0);
        let mut body = raw[header_end..].to_vec();
        while body.len() < length {
            let read = stream.read(&mut buf).ok()?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read]);
        }
        Some(format!(
            "{head}---BODY---\n{}",
            String::from_utf8_lossy(&body)
        ))
    }

    fn reason_phrase(status: u16) -> &'static str {
        match status {
            200 => "OK",
            302 => "Found",
            400 => "Bad Request",
            401 => "Unauthorized",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "Unknown",
        }
    }

    fn write_reply(stream: &mut TcpStream, reply: &Reply) -> std::io::Result<()> {
        let total: usize = reply.segments.iter().map(|(_, bytes)| bytes.len()).sum();
        let mut head = format!(
            "HTTP/1.1 {} {}\r\n",
            reply.status,
            reason_phrase(reply.status)
        );
        for (name, value) in &reply.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!(
            "content-length: {total}\r\nconnection: close\r\n\r\n"
        ));
        stream.write_all(head.as_bytes())?;
        stream.flush()?;
        for (delay, bytes) in &reply.segments {
            if !delay.is_zero() {
                std::thread::sleep(*delay);
            }
            stream.write_all(bytes)?;
            stream.flush()?;
        }
        Ok(())
    }

    fn profile(base_url: &str) -> ProviderProfile {
        ProviderProfile {
            name: "stub".to_string(),
            protocol: Protocol::OpenAi,
            base_url: base_url.to_string(),
            path: "/v1".to_string(),
            query: Vec::new(),
            model: "m".to_string(),
            auth: AuthScheme::None,
            extra_headers: Vec::new(),
            extra_body: serde_json::Value::Null,
            connect_timeout_ms: 2_000,
            read_timeout_ms: 2_000,
            total_timeout_ms: 5_000,
            max_tokens: 16,
        }
    }

    fn request() -> LlmRequest {
        LlmRequest {
            request_id: "R-1".to_string(),
            session_id: "S-1".to_string(),
            task_id: None,
            model: "m".to_string(),
            prompt: "hi".to_string(),
            max_tokens: 16,
            timeout: None,
        }
    }

    fn chunks(stream: ChunkStream) -> Vec<LlmChunk> {
        stream.collect::<Result<Vec<_>, _>>().expect("流式收集")
    }

    fn unique_temp_path(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        std::env::temp_dir().join(format!(
            "whcode-llm-client-{}-{stamp}-{name}",
            std::process::id()
        ))
    }

    const OPENAI_SSE: &str = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"好\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n"
    );

    #[test]
    fn streams_sse_into_contiguous_chunks_with_provider_usage() {
        let server = StubServer::start(vec![Reply::sse(OPENAI_SSE)]);
        let client = HttpClient::new(profile(&server.base_url())).expect("构造客户端");
        let stream = client
            .execute(&request(), &CancelToken::default())
            .expect("发起请求");
        let chunks = chunks(stream);

        let indices: Vec<u64> = chunks.iter().map(|chunk| chunk.index).collect();
        assert_eq!(indices, vec![0, 1, 2], "chunk 序号必须连续（FR-208）");
        assert_eq!(
            chunks.iter().filter(|chunk| chunk.final_chunk).count(),
            1,
            "终结 chunk 必须唯一"
        );
        let final_chunk = chunks.last().expect("终结 chunk");
        assert_eq!(final_chunk.finish_reason.as_deref(), Some("length"));
        let usage = final_chunk.usage.expect("供应商用量必须保留");
        assert_eq!((usage.prompt_tokens, usage.completion_tokens), (10, 5));
        assert!(!usage.cost_is_estimate, "供应商计量与本地估算可区分");
        assert_eq!(
            client.snapshot().state,
            "disconnected",
            "流读完必须记录断开"
        );
    }

    #[test]
    fn auth_header_reaches_server_but_never_leaks_into_errors() {
        let path = unique_temp_path("secret.txt");
        std::fs::write(&path, "fake-secret-value\n").expect("写测试密钥文件");
        let server = StubServer::start(vec![Reply::status(
            401,
            &[],
            r#"{"error":{"message":"bad key"}}"#,
        )]);
        let mut profile = profile(&server.base_url());
        profile.auth = AuthScheme::Header {
            name: "x-api-key".to_string(),
            prefix: String::new(),
            credential: CredentialRef::File(path.clone()),
        };
        let client = HttpClient::new(profile).expect("构造客户端");
        let err = client
            .execute(&request(), &CancelToken::default())
            .expect_err("401 必须失败");

        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.message.contains("fake-secret-value"), "{err}");
        let details = serde_json::to_string(&err.details).expect("details JSON");
        assert!(
            !details.contains("fake-secret-value"),
            "details 不得含密钥：{details}"
        );
        let seen = server.requests();
        assert_eq!(seen.len(), 1);
        assert!(
            seen[0]
                .to_ascii_lowercase()
                .contains("x-api-key: fake-secret-value"),
            "鉴权头必须真正发出：{}",
            seen[0]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn query_auth_is_encoded_into_request_target() {
        let path = unique_temp_path("query.txt");
        std::fs::write(&path, "fake-secret-value").expect("写测试密钥文件");
        let server = StubServer::start(vec![Reply::sse("data: [DONE]\n\n")]);
        let mut profile = profile(&server.base_url());
        profile.auth = AuthScheme::Query {
            name: "key".to_string(),
            credential: CredentialRef::File(path.clone()),
        };
        let client = HttpClient::new(profile).expect("构造客户端");
        let chunks = chunks(
            client
                .execute(&request(), &CancelToken::default())
                .expect("发起请求"),
        );
        assert_eq!(chunks.len(), 1);

        let seen = server.requests();
        assert!(
            seen[0].contains("key=fake-secret-value"),
            "查询参数必须进入请求目标：{}",
            seen[0]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn non_2xx_maps_to_normalized_status_with_retry_after() {
        let server = StubServer::start(vec![Reply::status(
            429,
            &[("retry-after", "2")],
            r#"{"error":{"message":"slow down"}}"#,
        )]);
        let client = HttpClient::new(profile(&server.base_url())).expect("构造客户端");
        let err = client
            .execute(&request(), &CancelToken::default())
            .expect_err("429 必须失败");
        assert_eq!(err.code, ErrorCode::Backpressure);
        assert!(err.retryable);
        assert_eq!(
            err.details.as_ref().expect("details")["retry_after_ms"],
            2_000
        );
        assert!(err.message.contains("slow down"), "{err}");
    }

    #[test]
    fn oversized_error_body_is_bounded_and_never_silently_truncated() {
        let body = "x".repeat(200 * 1024);
        let server = StubServer::start(vec![Reply::status(400, &[], &body)]);
        let client = HttpClient::new(profile(&server.base_url())).expect("构造客户端");
        let err = client
            .execute(&request(), &CancelToken::default())
            .expect_err("400 必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.message.chars().count() < 512,
            "错误消息必须有界（正文上限 64 KiB，只提取消息）：{}",
            err.message.chars().count()
        );
    }

    #[test]
    fn unreachable_endpoint_fails_within_the_connect_budget_and_is_retryable() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定临时端口");
        let address = listener.local_addr().expect("本地地址");
        drop(listener);

        let mut profile = profile(&format!("http://{address}"));
        profile.connect_timeout_ms = 200;
        let client = HttpClient::new(profile).expect("构造客户端");
        let started = Instant::now();
        let err = client
            .execute(&request(), &CancelToken::default())
            .expect_err("不可达端点必须失败");
        // 0 信任口径（Windows 11 amd64 实测，2026-10-25）：对已关闭的回环端口，
        // `std::net::TcpStream::connect_timeout` 在整段预算用尽后报 `TimedOut`（没有
        // RST／拒绝语义，`raw_os_error` 为 `None`），因此此处归一为 `timeout`；
        // 平台确实报连接错误时才是 `io_error`。两者都可重试，由调用方决定。
        assert!(
            matches!(err.code, ErrorCode::Timeout | ErrorCode::IoError),
            "{err}"
        );
        assert!(err.retryable, "连接类失败可重试（由调用方决定）");
        let failure = err.details.as_ref().expect("details")["failure"]
            .as_str()
            .expect("failure 字符串")
            .to_string();
        assert!(
            failure == "timeout" || failure == "connect",
            "failure 分类：{failure}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "连接失败必须有界（实际 {:?}）",
            started.elapsed()
        );
        let events = client.take_transport_events();
        assert!(
            events.events.iter().all(|event| {
                event.payload["transport"]["state"] != "reconnecting"
                    && event.payload["transport"]["reconnect_attempts"] == 0
            }),
            "流式生成路径不得自动重试"
        );
        assert_eq!(
            events.events.last().expect("断开事件").payload["transport"]["disconnect_reason"],
            failure,
            "断连原因码必须与失败分类一致（FR-616）"
        );
    }

    #[test]
    fn read_timeout_is_reported_as_timeout() {
        let server = StubServer::start(vec![
            Reply::sse("data: {\"choices\":[{\"delta\":{\"content\":\"慢\"}}]}\n\n")
                .delayed(Duration::from_millis(600)),
        ]);
        let mut profile = profile(&server.base_url());
        profile.read_timeout_ms = 150;
        profile.connect_timeout_ms = 1_000;
        profile.total_timeout_ms = 2_000;
        let client = HttpClient::new(profile).expect("构造客户端");
        let started = Instant::now();
        let mut stream = client
            .execute(&request(), &CancelToken::default())
            .expect("发起请求");
        let err = stream
            .next()
            .expect("必须有一次迭代机会")
            .expect_err("读取超时必须失败");
        assert_eq!(err.code, ErrorCode::Timeout, "{err}");
        assert!(err.retryable, "超时可重试");
        assert!(err.message.contains("超时"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "读取超时必须有界"
        );
    }

    #[test]
    fn cancel_takes_effect_at_next_frame_boundary() {
        let server = StubServer::start(vec![Reply {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
            segments: vec![
                (
                    Duration::ZERO,
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n".to_vec(),
                ),
                (
                    Duration::from_millis(400),
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n".to_vec(),
                ),
            ],
        }]);
        let client = HttpClient::new(profile(&server.base_url())).expect("构造客户端");
        let cancel = CancelToken::default();
        let mut stream = client.execute(&request(), &cancel).expect("发起请求");
        let first = stream.next().expect("首块").expect("首块成功");
        assert_eq!(first.index, 0);
        cancel.cancel();
        let err = stream
            .next()
            .expect("取消后仍有一次迭代机会")
            .expect_err("取消必须生效");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(!err.retryable);
        assert!(err.message.contains("取消"), "{err}");
        assert!(
            err.message.contains("帧边界"),
            "取消生效时机必须写入错误说明：{err}"
        );
        assert_eq!(
            err.details.as_ref().expect("details")["cancel_effective_at"],
            "next_frame_boundary"
        );

        let events = client.take_transport_events();
        assert_eq!(
            events.events.last().expect("断开事件").payload["transport"]["disconnect_reason"],
            "cancelled"
        );
        assert!(stream.next().is_none(), "取消后流必须结束");
    }

    #[test]
    fn eof_without_finish_is_reported_as_eof_not_stop() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"甲\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"乙\"}}]}\n\n"
        );
        let server = StubServer::start(vec![Reply::sse(body)]);
        let client = HttpClient::new(profile(&server.base_url())).expect("构造客户端");
        let chunks = chunks(
            client
                .execute(&request(), &CancelToken::default())
                .expect("发起请求"),
        );
        let last = chunks.last().expect("终结 chunk");
        assert_eq!(last.index, 2);
        assert!(last.final_chunk);
        assert_eq!(
            last.finish_reason.as_deref(),
            Some(EOF_FINISH_REASON),
            "无显式终止时必须与 stop 区分（FR-208）"
        );
        assert!(last.usage.is_none(), "无供应商用量时走本地估算");
    }

    #[test]
    fn transport_snapshot_reports_bounded_bytes_and_rtt() {
        let server = StubServer::start(vec![Reply::sse(OPENAI_SSE)]);
        let client = HttpClient::new(profile(&server.base_url())).expect("构造客户端");
        let _ = chunks(
            client
                .execute(&request(), &CancelToken::default())
                .expect("发起请求"),
        );
        let snapshot = client.snapshot();
        assert_eq!(snapshot.connection_id, "stub");
        assert_eq!(snapshot.state, "disconnected");
        assert!(snapshot.bytes_sent > 0, "发送字节必须被记录");
        assert!(
            snapshot.bytes_received >= OPENAI_SSE.len() as u64,
            "接收字节必须覆盖正文：{}",
            snapshot.bytes_received
        );
        assert_eq!(snapshot.rtt_samples.len(), 1, "每次连接一次 RTT 采样");
        assert!(snapshot.rtt_samples.len() <= crate::transport_events::MAX_RTT_SAMPLES);
        assert_eq!(snapshot.reconnect_attempts, 0);
        assert!(
            client.transport_snapshot().is_some(),
            "Provider 钩子必须返回快照"
        );
    }

    #[test]
    fn unknown_frames_are_counted_without_breaking_the_stream() {
        let body = concat!(
            "data: not-json\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let server = StubServer::start(vec![Reply::sse(body)]);
        let client = HttpClient::new(profile(&server.base_url())).expect("构造客户端");
        let chunks = chunks(
            client
                .execute(&request(), &CancelToken::default())
                .expect("发起请求"),
        );
        assert_eq!(chunks.len(), 2, "未知帧不产出 chunk，但不得中断流");
        assert_eq!(client.unknown_frames(), 1, "未知帧必须计数留痕");
    }

    #[test]
    fn protocol_headers_and_body_reach_the_server() {
        let server = StubServer::start(vec![Reply::sse(concat!(
            "event: content_block_delta\ndata: {\"delta\":{\"text\":\"段\"}}\n\n",
            "event: message_stop\ndata: {}\n\n"
        ))]);
        let mut profile = profile(&server.base_url());
        profile.protocol = Protocol::Anthropic;
        profile.path = String::new();
        let client = HttpClient::new(profile).expect("构造客户端");
        let chunks = chunks(
            client
                .execute(&request(), &CancelToken::default())
                .expect("发起请求"),
        );
        assert_eq!(chunks.len(), 2);
        assert!(chunks[1].final_chunk);

        let seen = &server.requests()[0];
        let lower = seen.to_ascii_lowercase();
        assert!(lower.contains("anthropic-version: 2023-06-01"), "{seen}");
        assert!(lower.contains("content-type: application/json"), "{seen}");
        assert!(seen.contains("POST /v1/messages "), "{seen}");
        assert!(
            seen.contains("\"stream\":true"),
            "请求体必须请求流式：{seen}"
        );
    }

    #[test]
    fn probe_retries_are_bounded_and_recorded() {
        let server = StubServer::start(vec![
            Reply::status(500, &[], r#"{"error":{"message":"boom"}}"#),
            Reply::status(500, &[], r#"{"error":{"message":"boom"}}"#),
            Reply::status(200, &[], r#"{"data":[]}"#),
        ]);
        let client = HttpClient::new(profile(&server.base_url()))
            .expect("构造客户端")
            .with_retry_policy(RetryPolicy {
                max_retries: 2,
                backoff: [Duration::ZERO, Duration::ZERO],
            });
        let body = client
            .probe_get("R-probe", "/v1/models")
            .expect("第三次必须成功");
        assert_eq!(body, r#"{"data":[]}"#);
        assert_eq!(server.requests().len(), 3, "首发 + 2 次重试");
        assert_eq!(client.snapshot().reconnect_attempts, 2);
        assert!(
            client
                .take_transport_events()
                .events
                .iter()
                .any(|event| event.kind == EventKind::ProgressChanged
                    && event.payload["transport"]["state"] == "reconnecting"),
            "每次重试都必须写传输事件（FR-616）"
        );
    }

    #[test]
    fn probe_retries_never_exceed_the_hard_cap_and_skip_4xx() {
        let server = StubServer::start(vec![Reply::status(
            500,
            &[],
            r#"{"error":{"message":"boom"}}"#,
        )]);
        let client = HttpClient::new(profile(&server.base_url()))
            .expect("构造客户端")
            .with_retry_policy(RetryPolicy {
                max_retries: 99,
                backoff: [Duration::ZERO, Duration::ZERO],
            });
        let err = client
            .probe_get("R-probe", "/v1/models")
            .expect_err("持续 5xx 必须失败");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(err.retryable);
        assert_eq!(
            server.requests().len(),
            (MAX_PROBE_RETRIES + 1) as usize,
            "重试次数有硬上限"
        );
        drop(server);

        let server = StubServer::start(vec![Reply::status(
            400,
            &[],
            r#"{"error":{"message":"bad"}}"#,
        )]);
        let client = HttpClient::new(profile(&server.base_url())).expect("构造客户端");
        let err = client
            .probe_get("R-probe", "/v1/models")
            .expect_err("400 必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert_eq!(server.requests().len(), 1, "4xx 不重试");
    }

    #[test]
    fn redirects_are_not_followed_to_protect_credentials() {
        let server = StubServer::start(vec![Reply::status(
            302,
            &[("location", "http://127.0.0.1:1/elsewhere")],
            "",
        )]);
        let client = HttpClient::new(profile(&server.base_url())).expect("构造客户端");
        let err = client
            .execute(&request(), &CancelToken::default())
            .expect_err("3xx 不得被跟随");
        assert_eq!(err.code, ErrorCode::Internal, "{err}");
        assert_eq!(server.requests().len(), 1, "不得发出第二跳请求");
    }

    #[test]
    fn profile_with_duplicate_auth_header_is_rejected() {
        let path = unique_temp_path("dup.txt");
        std::fs::write(&path, "fake-secret-value").expect("写测试密钥文件");
        let mut profile = profile("http://127.0.0.1:1");
        profile.auth = AuthScheme::Bearer {
            credential: CredentialRef::File(path.clone()),
        };
        profile.extra_headers = vec![("Authorization".to_string(), "other".to_string())];
        let err = HttpClient::new(profile).expect_err("重复鉴权头必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("Authorization"), "{err}");
        assert!(!err.message.contains("fake-secret-value"), "{err}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn tls_config_from_roots_rejects_empty_trust_set() {
        let err = tls_config_from_roots(&[]).expect_err("空信任集必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("ADR-040"), "{err}");
    }
}
