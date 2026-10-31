//! # HTTP API 与 WebSocket（T090、FR-603、ADR-004）
//!
//! 复用应用服务命令/快照/事件（分层不变：Web 是前端适配器，**只**经 `whcode-app`
//! 访问状态，禁止触库——ADR-003/005、C-03；本模块的 `&Connection` 由装配根透传）。
//!
//! 路由（M6 口径 + 写命令）：
//! - `GET /api/status?session=S` → 状态快照（FR-314）；
//! - `GET /api/sessions` → 会话列表（FR-101）；
//! - `GET /api/events?session=S&after=N` → 事件增量（FR-202/605）；
//! - `GET /api/tasks?session=S` → 任务总览（FR-103）；
//! - `GET /api/runtimes` → 运行时能力声明（FR-702）；
//! - `GET /api/i18n?locale=zh-CN` → 文本层（FR-615）；
//! - `GET /ws` → WebSocket 升级（服务端推送事件文本帧，RFC 6455）；
//! - `POST /api/sessions/new|rename|archive|delete` → 会话写命令（FR-609）；
//! - `POST /api/tasks/retry|abort` → 任务写命令（FR-609）；
//! - `POST /api/purge` → 清理预览/执行（FR-204/508/509）。
//!
//! 写命令与 CUI 共用同一应用服务函数与同一 JSON 形状（`whcode_app::views`），
//! 破坏性命令一律**默认预览**（`confirm=false`），显式 `confirm=true` 才执行——
//! 与 CUI 的 `--dry-run`/`--yes` 同语义（FR-604）。
//!
//! 每个请求经 [`crate::security`] 认证 + Origin 检查（§10.2）；请求体有界
//! （[`MAX_MESSAGE_BYTES`]）。`whcode` 是**原生 Web UI**，不是 TUI 的 PTY 镜像
//! （ADR-004）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::Connection;
use whcode_app::cert::{self, CaKind, CertPaths, CertProbe};
use whcode_app::surface::{FrontendId, Support};
use whcode_app::{Actor, Capability, DefaultPolicy, build_snapshot, require};
use whcode_domain::{
    CERT_JSON_MAX_BYTES, ErrorCode, InboundMode, TlsSettings, TlsStatus, TrustMode, WhError,
};

use crate::security;
use crate::ws;

/// HTTP 方法（M6 支持子集）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET。
    Get,
    /// POST。
    Post,
}

/// 最小 HTTP 请求（M6 口径：请求行 + 头 + 体；分块传输不支持——收到即 411）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// 方法。
    pub method: Method,
    /// 路径（含 query）。
    pub path: String,
    /// 头（小写键 → 值）。
    pub headers: Vec<(String, String)>,
    /// 体（有界）。
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// 读取指定头（大小写不敏感）。
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// 解析 query 参数。
    #[must_use]
    pub fn query(&self, name: &str) -> Option<&str> {
        let (_, query) = self.path.split_once('?')?;
        query.split('&').find_map(|pair| {
            let (key, value) = pair
                .split_once('=')
                .map_or((pair, ""), |(key, value)| (key, value));
            (key == name).then_some(value)
        })
    }
}

/// HTTP 响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// 状态码。
    pub status: u16,
    /// 头。
    pub headers: Vec<(String, String)>,
    /// 体。
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// 读取指定头（大小写不敏感）。
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// JSON 响应。
    #[must_use]
    pub fn json(status: u16, body: &serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: serde_json::to_vec(body).unwrap_or_default(),
        }
    }

    /// 文本响应。
    #[must_use]
    pub fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
            body: body.as_bytes().to_vec(),
        }
    }

    /// WebSocket 升级响应（RFC 6455 §1.3）。
    #[must_use]
    pub fn websocket_upgrade(accept_key: &str) -> Self {
        Self {
            status: 101,
            headers: vec![
                ("upgrade".to_string(), "websocket".to_string()),
                ("connection".to_string(), "Upgrade".to_string()),
                ("sec-websocket-accept".to_string(), accept_key.to_string()),
            ],
            body: Vec::new(),
        }
    }
}

/// 从字节流解析最小 HTTP 请求（M6 口径）。
///
/// # Errors
///
/// 请求行/头损坏、体超过 [`security::MAX_MESSAGE_BYTES`] 时返回 [`WhError`]。
pub fn parse_request(bytes: &[u8]) -> Result<HttpRequest, WhError> {
    let header_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| WhError::parse_error("HTTP 请求缺头部终止符"))?;
    let head = std::str::from_utf8(&bytes[..header_end])
        .map_err(|err| WhError::parse_error(format!("请求头非 UTF-8：{err}")))?;
    let mut lines = head.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| WhError::parse_error("HTTP 请求行缺失"))?;
    let mut parts = request_line.split_whitespace();
    let method = match parts.next().unwrap_or("") {
        "GET" => Method::Get,
        "POST" => Method::Post,
        other => {
            return Err(WhError::parse_error(format!("不支持的方法：{other}")));
        }
    };
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let body = bytes[header_end + 4..].to_vec();
    security::check_message_size(body.len())?;
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

/// 从 `Accept-Language` 提取首选语言标签（取第一段，忽略 q 权重——完整协商属后续）。
fn preferred_locale(request: &HttpRequest) -> Option<String> {
    let accept = request.header("accept-language")?;
    let first = accept.split(',').next()?.split(';').next()?.trim();
    (!first.is_empty()).then(|| first.to_string())
}

fn error_response(error: &WhError) -> HttpResponse {
    let status = match error.code {
        ErrorCode::PermissionDenied => 403,
        ErrorCode::InvalidState => 400,
        // 已登记但该端不提供（FR-620）；当前统一守卫直接返回 501，此处兜底同码。
        ErrorCode::NotSupported => 501,
        ErrorCode::NotFound => 404,
        ErrorCode::ParseError => 400,
        _ => 500,
    };
    HttpResponse::json(
        status,
        &serde_json::json!({
            "schema_version": whcode_domain::SCHEMA_VERSION,
            "code": error.code.as_str(),
            "message": error.message,
            "retryable": error.retryable,
            "details": error.details,
        }),
    )
}

/// 当前时间（毫秒）；时钟异常返回内部错误而不是静默用 0。
fn now_ms() -> Result<u64, WhError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| WhError::internal(format!("系统时钟异常：{err}")))?;
    Ok(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// 证书 API 上下文（T190、ADR-040 决策 4）：由装配根注入，Web 层不自行打开证书库。
///
/// - `paths`：工作区证书库路径；
/// - `probe`：出站连通性校验（实现属装配根——`whcode-web` 保持零网络依赖，ADR-031）；
/// - `restart_required`：本次 `serve --web` 会话的“入站变更需重启”标记；写操作成功且
///   入站档位／服务端证书变化时置位，读操作只读取。
pub struct CertApiContext<'a> {
    /// 工作区证书库路径。
    pub paths: &'a CertPaths,
    /// 出站连通性校验实现。
    pub probe: &'a dyn CertProbe,
    /// 入站变更需重启标记（`Relaxed` 足够：单线程服务循环内读写，无跨线程顺序依赖）。
    pub restart_required: &'a AtomicBool,
    /// 本进程实际监听地址（host 字面量 + 端口）；用于拒绝“自探本进程”的必然超时。
    ///
    /// `serve --web` 为单线程服务：处理校验请求期间无法为另一条连接推进 TLS 握手，
    /// 指向自身的校验必然耗尽预算（2026-10-25 实跑实测）。拒绝并说明原因，比返回
    /// 误导性的 timeout 更可操作（NFR-222）。
    pub listen: Option<(String, u16)>,
}

/// 证书设置面未接线时的统一错误（不以 404 掩盖已登记能力，ADR-032）。
fn cert_not_wired() -> WhError {
    WhError::invalid_state(
        "证书设置面未接线：本次请求未注入证书库上下文（ADR-040 决策 4）；\
         请以 serve --web 启动并确认工作区可写",
    )
}

/// 取证书上下文；未接线返回 [`cert_not_wired`]。
fn require_cert<'ctx, 'a>(
    cert: Option<&'ctx CertApiContext<'a>>,
) -> Result<&'ctx CertApiContext<'a>, WhError> {
    cert.ok_or_else(cert_not_wired)
}

/// 用会话原子标记覆盖状态里的 `restart_required`（读操作不置位，只反映已置位的变更）。
fn apply_restart(mut status: TlsStatus, cert: &CertApiContext<'_>) -> TlsStatus {
    status.restart_required = cert.restart_required.load(Ordering::Relaxed);
    status
}

/// 入站变更（档位或服务端证书）成功后置位重启标记；根证书变更不置位。
fn mark_restart(status: &TlsStatus, cert: &CertApiContext<'_>) {
    if status.restart_required {
        cert.restart_required.store(true, Ordering::Relaxed);
    }
}

/// 状态响应的统一形状（`schema_version` + 完整 `TlsStatus` 字段）。
fn tls_status_response(status: &TlsStatus) -> HttpResponse {
    match serde_json::to_value(status) {
        Ok(value) => HttpResponse::json(200, &value),
        Err(err) => error_response(&WhError::internal(format!(
            "证书状态序列化失败：{err}；请检查状态字段是否可序列化"
        ))),
    }
}

/// 证书写操作的请求体：解析前按 [`CERT_JSON_MAX_BYTES`] 拒绝（ADR-040 决策 1／4）。
///
/// 空体（全为空白）按空对象处理：`POST /api/certs/server/self-signed` 等只有可选
/// `confirm` 的路由允许不带体，仍由各路由校验必需字段。
fn cert_json_body(request: &HttpRequest) -> Result<serde_json::Value, WhError> {
    if request.body.len() > CERT_JSON_MAX_BYTES {
        return Err(WhError::parse_error(format!(
            "证书 API 请求体 {} 字节超过上限 {CERT_JSON_MAX_BYTES} 字节（解析前拒绝）；\
             请裁剪后重试（ADR-040 决策 1 的拒绝阈值）",
            request.body.len()
        )));
    }
    if request.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::json!({}));
    }
    json_body(request)
}

/// 可选 `confirm`：缺失视为 `false`；出现但不是布尔时显式拒绝（不把字符串当布尔）。
fn optional_confirm(body: &serde_json::Value) -> Result<bool, WhError> {
    match body.get("confirm") {
        None => Ok(false),
        Some(_) => body_bool(body, "confirm"),
    }
}

/// `kind` 稳定字符串 → [`CaKind`]；未知取值返回 `invalid_state`（不回落默认）。
fn parse_ca_kind(value: &str) -> Result<CaKind, WhError> {
    match value {
        "builtin" => Ok(CaKind::Builtin),
        "extra" => Ok(CaKind::Extra),
        other => Err(WhError::invalid_state(format!(
            "未知根证书种类“{other}”；合法取值为 builtin 或 extra"
        ))),
    }
}

/// 请求体字段 `kind`（缺参数可操作报错）。
fn body_ca_kind(body: &serde_json::Value) -> Result<CaKind, WhError> {
    let value = body
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| WhError::invalid_state("缺 kind 参数（builtin／extra）"))?;
    parse_ca_kind(value)
}

/// 探测目标主机校验：非空、无 scheme／userinfo／路径、长度 ≤ 253（端口另用 `port`）。
fn probe_host(raw: &str) -> Result<String, WhError> {
    let host = raw.trim();
    if host.is_empty() {
        return Err(WhError::invalid_state("缺 host 参数（主机名或 IP 字面量）"));
    }
    if host.len() > 253 {
        return Err(WhError::invalid_state(format!(
            "host 长度 {} 超过 DNS 名称上限 253；请改用合法主机名",
            host.len()
        )));
    }
    if host.contains("://") || host.contains('@') || host.contains('/') {
        return Err(WhError::invalid_state(format!(
            "host“{host}”不得包含 ://、@ 或 /；请只给主机名或 IP 字面量，端口用 port 参数"
        )));
    }
    Ok(host.to_string())
}

/// 可选 `port`：缺失默认 443；必须是 1..=65535 的整数。
fn probe_port(body: &serde_json::Value) -> Result<u16, WhError> {
    match body.get("port") {
        None => Ok(443),
        Some(_) => {
            let value = body_i64(body, "port")?;
            u16::try_from(value)
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| {
                    WhError::invalid_state(format!("port 必须在 1..=65535（当前 {value}）"))
                })
        }
    }
}

/// 目标是否就是本进程监听地址（端口相等且主机为上送监听 host 或回环别名）。
///
/// 回环别名只认 `127.0.0.1`、`localhost`、`::1`（含方括号形式）——它们是同一台机器的
/// 同一回环接口；其他主机名不做猜测（可能经 DNS 指向本机，但无法在 Web 层可靠判定）。
fn is_self_probe(host: &str, port: u16, listen: Option<&(String, u16)>) -> bool {
    let Some((listen_host, listen_port)) = listen else {
        return false;
    };
    if port != *listen_port {
        return false;
    }
    let normalize = |value: &str| {
        value
            .trim()
            .trim_matches(|ch| ch == '[' || ch == ']')
            .to_ascii_lowercase()
    };
    let host = normalize(host);
    let listen_host = normalize(listen_host);
    let loopback = |value: &str| matches!(value, "127.0.0.1" | "localhost" | "::1");
    if host == listen_host {
        return true;
    }
    loopback(&host) && loopback(&listen_host)
}

/// 501 `not_supported` 响应（FR-620、ADR-032）：已登记但 Web 端预留/排除的能力。
///
/// details 由登记表驱动（`surface_id` + Web 端状态码 + 里程碑/理由码）；message 只
/// 说明“能力未在该端提供”与里程碑/理由码，**不回显内部路径细节**（路径可经登记表
/// 公开查询，不作为错误正文）。
fn not_supported_response(surface_id: &str, support: &Support) -> HttpResponse {
    let mut details = serde_json::json!({
        "surface_id": surface_id,
        "state": support.state_code(),
    });
    let message = match support {
        Support::Reserved { milestone } => {
            details["milestone"] = serde_json::json!(milestone);
            format!("该能力未在 Web 端提供（预留，里程碑 {milestone}）")
        }
        Support::Excluded { reason } => {
            details["reason_code"] = serde_json::json!(reason.as_str());
            format!(
                "该能力未在 Web 端提供（已登记排除，理由码 {}）",
                reason.as_str()
            )
        }
        _ => format!("该能力未在 Web 端提供（{}）", support.state_code()),
    };
    HttpResponse::json(
        501,
        &serde_json::json!({
            "schema_version": whcode_domain::SCHEMA_VERSION,
            "code": "not_supported",
            "message": message,
            "retryable": false,
            "details": details,
        }),
    )
}

/// 写命令的请求体：必须是 JSON 对象（§8.1：写操作只走 POST + JSON）。
fn json_body(request: &HttpRequest) -> Result<serde_json::Value, WhError> {
    let value: serde_json::Value = serde_json::from_slice(&request.body)
        .map_err(|err| WhError::parse_error(format!("请求体不是合法 JSON：{err}")))?;
    if !value.is_object() {
        return Err(WhError::parse_error("请求体必须是 JSON 对象"));
    }
    Ok(value)
}

/// 取非空字符串字段；缺失/空白按“缺参数”处理（错误可操作）。
fn body_str<'a>(body: &'a serde_json::Value, field: &str) -> Result<&'a str, WhError> {
    body.get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| WhError::invalid_state(format!("缺 {field} 参数（非空字符串）")))
}

/// 取布尔字段（不把字符串 "true" 当布尔——显式拒绝比猜测更安全）。
fn body_bool(body: &serde_json::Value, field: &str) -> Result<bool, WhError> {
    body.get(field)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| WhError::parse_error(format!("{field} 必须是布尔值 true/false")))
}

/// 取整数字段。
fn body_i64(body: &serde_json::Value, field: &str) -> Result<i64, WhError> {
    body.get(field)
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| WhError::parse_error(format!("{field} 必须是整数")))
}

/// 处理单个 HTTP 请求（T090）：认证 + Origin 检查 → 路由 → 应用服务。
///
/// # Errors
///
/// 本函数把错误编码为 HTTP 响应（错误信封与 CUI 同源，FR-602），不返回 `Err`；
/// 签名保留 `Result` 以便装配根统一处理。
pub fn handle_request(
    conn: &mut Connection,
    auth_enabled: bool,
    token: Option<&str>,
    allowed_origins: &[&str],
    request: &HttpRequest,
    actor: &Actor,
) -> HttpResponse {
    handle_request_with_runtimes(
        conn,
        auth_enabled,
        token,
        allowed_origins,
        request,
        actor,
        None,
        "zh-CN",
    )
}

/// 同 [`handle_request`]，并注入运行时清单（FR-702、Check 101）。
///
/// `runtimes` 为装配根预先序列化的声明数组（`whcode-bin::runtimes::inventory`）；
/// `None` 表示未注入——响应仍为 200，但 `wired=false`，便于把“没接线”与“没有运行时”区分开。
/// `default_locale` 为装配根解析的语言默认值（FR-625 链：`ui.locale` → `WHCODE_UI_LOCALE`
/// → LANG/LC_ALL → zh-CN）；`?locale=` 与 Accept-Language 在其上显式覆盖。
#[allow(clippy::too_many_arguments)] // 8 个参数是正交的请求上下文；拆分公共结构会波及所有前端调用点
pub fn handle_request_with_runtimes(
    conn: &mut Connection,
    auth_enabled: bool,
    token: Option<&str>,
    allowed_origins: &[&str],
    request: &HttpRequest,
    actor: &Actor,
    runtimes: Option<&serde_json::Value>,
    default_locale: &str,
) -> HttpResponse {
    handle_request_with_certs(
        conn,
        auth_enabled,
        token,
        allowed_origins,
        request,
        actor,
        runtimes,
        default_locale,
        None,
    )
}

/// 同 [`handle_request_with_runtimes`]，并注入证书库上下文（`/api/certs/*`，T190）。
///
/// `cert` 为 `None` 表示装配根**未接线**证书设置面：`/api/certs/*` 一律返回
/// `invalid_state`（说明未接线），绝不伪装成功，也不以 404 掩盖已登记能力。
#[allow(clippy::too_many_arguments)] // 9 个参数同上一函数 + 证书上下文；拆分结构会波及全部调用点
pub fn handle_request_with_certs(
    conn: &mut Connection,
    auth_enabled: bool,
    token: Option<&str>,
    allowed_origins: &[&str],
    request: &HttpRequest,
    actor: &Actor,
    runtimes: Option<&serde_json::Value>,
    default_locale: &str,
    cert: Option<&CertApiContext<'_>>,
) -> HttpResponse {
    handle_request_with_llm(
        conn,
        auth_enabled,
        token,
        allowed_origins,
        request,
        actor,
        runtimes,
        default_locale,
        cert,
        None,
        None,
    )
}

/// 同 [`handle_request_with_certs`]，并注入 LLM 调用上下文（`/api/llm/*`，T167）。
///
/// `llm` 为 `None` 表示装配根**未接线** LLM 面：`/api/llm/*` 一律返回 `invalid_state`
/// （说明未接线与配置入口），绝不伪装成功；剖面注册表为空时由应用层给出可操作错误。
#[allow(clippy::too_many_arguments)] // 10 个参数同上一函数 + LLM 上下文；拆分结构会波及全部调用点
pub fn handle_request_with_llm(
    conn: &mut Connection,
    auth_enabled: bool,
    token: Option<&str>,
    allowed_origins: &[&str],
    request: &HttpRequest,
    actor: &Actor,
    runtimes: Option<&serde_json::Value>,
    default_locale: &str,
    cert: Option<&CertApiContext<'_>>,
    llm: Option<&whcode_app::llm::LlmContext<'_>>,
    workspace_root: Option<&std::path::Path>,
) -> HttpResponse {
    // 认证 + Origin（§10.2、V-110）。
    if let Err(error) = security::check_auth(auth_enabled, token, request.header("authorization")) {
        return error_response(&error);
    }
    // Origin 允许集默认回环来源；对外部署须显式扩展（V-110 门禁在 bind 层）。
    if let Err(error) = security::check_origin(allowed_origins, request.header("origin")) {
        return error_response(&error);
    }

    // ── 统一不可用语义守卫（FR-620、ADR-032） ─────────────────────────────
    // 已登记但 Web 端预留/排除的路径 → 501 `not_supported` 信封，不伪装成 404；
    // 未登记路径不入守卫（保持末尾 404 兜底，`not_found` 留给真正不存在的路由）；
    // 已实现路径继续走正常路由。守卫在认证/Origin **之后**：不放宽安全默认值
    // （docs/guides/web-deployment-security.md §4.1 口径）。
    let method_str = match request.method {
        Method::Get => "GET",
        Method::Post => "POST",
    };
    if let Some(surface_id) = crate::surface_map::surface_of(method_str, &request.path)
        && let Some(entry) = whcode_app::surface::find(surface_id)
    {
        let support = entry.support(FrontendId::Web);
        if matches!(support, Support::Reserved { .. } | Support::Excluded { .. }) {
            return not_supported_response(surface_id, support);
        }
    }

    let policy = DefaultPolicy;
    let (route, query) = match request.path.split_once('?') {
        Some((route, query)) => (route.to_string(), query.to_string()),
        None => (request.path.clone(), String::new()),
    };
    let query_value = |name: &str| -> Option<&str> {
        query.split('&').find_map(|pair| {
            let (key, value) = pair
                .split_once('=')
                .map_or((pair, ""), |(key, value)| (key, value));
            (key == name).then_some(value)
        })
    };

    match (request.method, route.as_str()) {
        (Method::Get, "/api/status") => {
            let Some(session) = query_value("session") else {
                return error_response(&WhError::invalid_state("缺 session 参数"));
            };
            match build_snapshot(conn, &policy, actor, session) {
                Ok(snapshot) => {
                    HttpResponse::json(200, &serde_json::to_value(&snapshot).unwrap_or_default())
                }
                Err(error) => error_response(&error),
            }
        }
        (Method::Get, "/api/sessions") => {
            match whcode_app::list_sessions(conn, &policy, actor, false, 200) {
                Ok(sessions) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "sessions": sessions,
                    }),
                ),
                Err(error) => error_response(&error),
            }
        }
        (Method::Get, "/api/events") => {
            let Some(session) = query_value("session") else {
                return error_response(&WhError::invalid_state("缺 session 参数"));
            };
            let after = query_value("after")
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0);
            match whcode_app::events_export(
                conn,
                &policy,
                actor,
                session,
                after,
                1000,
                TraceLevel::default(),
            ) {
                Ok(export) => {
                    HttpResponse::json(200, &serde_json::to_value(&export).unwrap_or_default())
                }
                Err(error) => error_response(&error),
            }
        }
        (Method::Get, "/api/tasks") => {
            // 任务视图（FR-103）：session 可选——缺省为工作区级总览。
            let session = query_value("session");
            match whcode_app::task_overview(conn, &policy, actor, session) {
                Ok(overview) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "session_id": overview.session_id,
                        "total": overview.total,
                        "done": overview.done,
                        "blocked": overview.blocked,
                        "by_stage": overview.by_stage,
                        "active": overview.active.iter().map(|row| serde_json::json!({
                            "id": row.id,
                            "title": row.title,
                            "status": row.status.as_str(),
                            "retry_count": row.retry_count,
                        })).collect::<Vec<_>>(),
                    }),
                ),
                Err(error) => error_response(&error),
            }
        }
        (Method::Get, "/api/context") => {
            // 上下文读取面（FR-622／174.1）：与 TUI／GUI 同一 `load_context`，字段同源。
            let Some(session) = query_value("session") else {
                return error_response(&WhError::invalid_state("缺 session 参数"));
            };
            let budget = match query_value("budget") {
                Some(raw) => match raw.parse::<usize>() {
                    Ok(parsed) if parsed > 0 => parsed,
                    Ok(_) => {
                        return error_response(&WhError::invalid_state("budget 不得为 0"));
                    }
                    Err(_) => {
                        return error_response(&WhError::parse_error("budget 取值不是非负整数"));
                    }
                },
                None => whcode_app::DEFAULT_CONTEXT_BUDGET_TOKENS,
            };
            match whcode_app::load_context(conn, &policy, actor, session, budget) {
                Ok(window) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "session_id": session,
                        "budget_tokens": budget,
                        "consumed_budget": window.consumed_budget,
                        "items": window.items.iter().map(|item| serde_json::json!({
                            "id": item.id,
                            "role": item.role,
                            "content_ref": item.content_ref,
                            "token_count": item.token_count,
                            "ts_us": item.ts_us,
                        })).collect::<Vec<_>>(),
                        "sources": window.sources,
                        "truncated_reason": window.truncated_reason,
                        "next_cursor": window.next_cursor.map(|cursor| serde_json::json!({
                            "ts_us": cursor.ts_us,
                            "id": cursor.id,
                        })),
                    }),
                ),
                Err(error) => error_response(&error),
            }
        }
        (Method::Get, "/api/diff") => {
            // diff 预览（FR-622／174.2）：与 TUI／GUI 同一 converge_differences 路径，
            // 有界 ≤200 行 + truncated 标记；未选会话与数据源失败按错误语义区分。
            let Some(session) = query_value("session") else {
                return error_response(&WhError::invalid_state("缺 session 参数"));
            };
            match whcode_app::overview::converge_differences(conn, &policy, actor, session) {
                Ok(view) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "session_id": view.session_id,
                        "task_count": view.task_count,
                        "pass_count": view.pass_count,
                        "fail_count": view.fail_count,
                        "not_tested_count": view.not_tested_count,
                        "truncated": view.truncated,
                        "differences": view.differences.iter().map(|line| serde_json::json!({
                            "marker": line.marker,
                            "text": line.text,
                        })).collect::<Vec<_>>(),
                    }),
                ),
                Err(error) => error_response(&error),
            }
        }
        (Method::Get, "/api/events/export") => {
            // 事件导出（FR-205／174.3）：复用 `events_export` 与 CUI 同一口径；
            // JSONL 首行注明追踪级别与脱敏策略（不引 CUI crate，本地渲染同形输出）。
            let Some(session) = query_value("session") else {
                return error_response(&WhError::invalid_state("缺 session 参数"));
            };
            let after = query_value("after")
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0);
            let format = query_value("format").unwrap_or("json");
            if format != "json" && format != "jsonl" {
                return error_response(&WhError::parse_error("format 仅支持 json 与 jsonl"));
            }
            let limit = match query_value("limit") {
                Some(raw) => match raw.parse::<usize>() {
                    Ok(parsed) if (1..=1000).contains(&parsed) => parsed,
                    Ok(_) => {
                        return error_response(&WhError::invalid_state(
                            "limit 须在 1..=1000（有界导出）",
                        ));
                    }
                    Err(_) => {
                        return error_response(&WhError::parse_error("limit 取值不是非负整数"));
                    }
                },
                None => 1000,
            };
            match whcode_app::events_export(
                conn,
                &policy,
                actor,
                session,
                after,
                limit,
                TraceLevel::default(),
            ) {
                Ok(export) => {
                    if format == "jsonl" {
                        let header = serde_json::json!({
                            "schema_version": whcode_domain::SCHEMA_VERSION,
                            "session_id": export.session_id,
                            "trace_level": export.trace_level,
                            "redacted": export.redacted,
                            "integrity": export.integrity.to_json(),
                            "count": export.events.len(),
                        });
                        let mut lines = vec![header.to_string()];
                        for event in &export.events {
                            match serde_json::to_string(event) {
                                Ok(line) => lines.push(line),
                                Err(err) => {
                                    return error_response(&WhError::parse_error(format!(
                                        "事件序列化失败：{err}"
                                    )));
                                }
                            }
                        }
                        HttpResponse {
                            status: 200,
                            headers: vec![(
                                "content-type".to_string(),
                                "application/x-ndjson".to_string(),
                            )],
                            body: lines.join("\n").into_bytes(),
                        }
                    } else {
                        HttpResponse::json(200, &serde_json::to_value(&export).unwrap_or_default())
                    }
                }
                Err(error) => error_response(&error),
            }
        }
        (Method::Get, "/api/runtimes") => {
            // 三档能力等级对 UI 可见（FR-702）：数据字段不翻译，能力名用稳定值。
            let list = runtimes
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
            HttpResponse::json(
                200,
                &serde_json::json!({
                    "schema_version": whcode_domain::SCHEMA_VERSION,
                    "wired": runtimes.is_some(),
                    "runtimes": list,
                }),
            )
        }
        (Method::Get, "/api/i18n") => {
            // 文本层下发（FR-615，批次 3）：locale 参数优先，其次 Accept-Language。
            let locale = query_value("locale")
                .map(str::trim)
                .filter(|locale| !locale.is_empty())
                .map(str::to_string)
                .or_else(|| preferred_locale(request))
                .unwrap_or_else(|| default_locale.to_string());
            let texts = whcode_domain::i18n::I18n::load(
                &locale,
                whcode_domain::i18n::LocaleSource::Explicit,
            );
            HttpResponse::json(
                200,
                &serde_json::json!({
                    "schema_version": whcode_domain::SCHEMA_VERSION,
                    "locale": texts.locale,
                    "rtl": whcode_domain::i18n::is_rtl_locale(&texts.locale),
                    "entries": texts.entries(),
                }),
            )
        }
        (Method::Get, "/ws") => {
            // WebSocket 升级：Origin 校验（安全层已做）+ Sec-WebSocket-Key。
            let Some(key) = request.header("sec-websocket-key") else {
                return error_response(&WhError::invalid_state(
                    "WebSocket 升级缺 Sec-WebSocket-Key（RFC 6455）",
                ));
            };
            HttpResponse::websocket_upgrade(&ws::accept_key(key))
        }
        (Method::Post, "/api/sessions/new") => {
            // 会话创建（FR-101/609）：与 CUI `session new` 同一应用服务。
            let body = match json_body(request) {
                Ok(body) => body,
                Err(error) => return error_response(&error),
            };
            let title = match body_str(&body, "title") {
                Ok(title) => title,
                Err(error) => return error_response(&error),
            };
            let model = body
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let now = match now_ms() {
                Ok(now) => now,
                Err(error) => return error_response(&error),
            };
            match whcode_app::execute_command(
                conn,
                &policy,
                actor,
                &whcode_app::FrontendCommand::SessionNew {
                    title: title.to_string(),
                    model,
                    now_ms: now,
                },
            ) {
                Ok(whcode_app::CommandOutcome::Session(row)) => HttpResponse::json(
                    201,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "session": whcode_app::session_value(&row),
                    }),
                ),
                Ok(_) => error_response(&WhError::internal("门面返回形状不符：应为 Session")),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/sessions/rename") => {
            let body = match json_body(request) {
                Ok(body) => body,
                Err(error) => return error_response(&error),
            };
            let (Ok(session), Ok(title)) = (body_str(&body, "session"), body_str(&body, "title"))
            else {
                return error_response(&WhError::invalid_state("缺 session 或 title 参数"));
            };
            match whcode_app::execute_command(
                conn,
                &policy,
                actor,
                &whcode_app::FrontendCommand::SessionRename {
                    session_id: session.to_string(),
                    title: title.to_string(),
                    now_ms: now_ms().unwrap_or(0),
                },
            ) {
                Ok(whcode_app::CommandOutcome::Session(row)) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "session": whcode_app::session_value(&row),
                    }),
                ),
                Ok(_) => error_response(&WhError::internal("门面返回形状不符：应为 Session")),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/sessions/archive") => {
            let body = match json_body(request) {
                Ok(body) => body,
                Err(error) => return error_response(&error),
            };
            let (Ok(session), Ok(archived)) =
                (body_str(&body, "session"), body_bool(&body, "archived"))
            else {
                return error_response(&WhError::invalid_state(
                    "缺 session 参数或 archived 不是布尔值",
                ));
            };
            // 契约 §2-1 破坏族：默认只预览（confirm 缺失/false），显式 true 才执行（ADR-038）。
            let confirm = body
                .get("confirm")
                .map(|_| body_bool(&body, "confirm"))
                .transpose();
            let confirm = match confirm {
                Ok(confirm) => confirm.unwrap_or(false),
                Err(error) => return error_response(&error),
            };
            let now = match now_ms() {
                Ok(now) => now,
                Err(error) => return error_response(&error),
            };
            if !confirm {
                return match whcode_app::execute_command(
                    conn,
                    &policy,
                    actor,
                    &whcode_app::FrontendCommand::SessionArchive {
                        session_id: session.to_string(),
                        archived,
                        confirm: false,
                        now_ms: now,
                    },
                ) {
                    Ok(whcode_app::CommandOutcome::SessionArchivePreview {
                        session: row,
                        target_archived,
                    }) => HttpResponse::json(
                        200,
                        &serde_json::json!({
                            "schema_version": whcode_domain::SCHEMA_VERSION,
                            "dry_run": true,
                            "session": session,
                            "current_archived": row.archived,
                            "archived": target_archived,
                            "changed": row.archived != target_archived,
                        }),
                    ),
                    Ok(_) => error_response(&WhError::internal("门面返回形状不符：应为归档预览")),
                    Err(error) => error_response(&error),
                };
            }
            match whcode_app::execute_command(
                conn,
                &policy,
                actor,
                &whcode_app::FrontendCommand::SessionArchive {
                    session_id: session.to_string(),
                    archived,
                    confirm: true,
                    now_ms: now,
                },
            ) {
                Ok(whcode_app::CommandOutcome::Session(row)) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "session": whcode_app::session_value(&row),
                    }),
                ),
                Ok(_) => error_response(&WhError::internal("门面返回形状不符：应为 Session")),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/sessions/delete") => {
            // 破坏性命令：默认只预览（confirm 缺失/false），显式 true 才执行（§8.1）。
            let body = match json_body(request) {
                Ok(body) => body,
                Err(error) => return error_response(&error),
            };
            let Ok(session) = body_str(&body, "session") else {
                return error_response(&WhError::invalid_state("缺 session 参数"));
            };
            let confirm = body
                .get("confirm")
                .map(|_| body_bool(&body, "confirm"))
                .transpose();
            let confirm = match confirm {
                Ok(confirm) => confirm.unwrap_or(false),
                Err(error) => return error_response(&error),
            };
            let now = match now_ms() {
                Ok(now) => now,
                Err(error) => return error_response(&error),
            };
            // 用户点名删除 + confirm：不套用 TTL 活跃窗口（口径在门面统一，FR-623）。
            if !confirm {
                return match whcode_app::execute_command(
                    conn,
                    &policy,
                    actor,
                    &whcode_app::FrontendCommand::SessionDelete {
                        session_id: session.to_string(),
                        confirm: false,
                        now_ms: now,
                    },
                ) {
                    Ok(whcode_app::CommandOutcome::SessionDeletePreview(preview)) => {
                        HttpResponse::json(
                            200,
                            &serde_json::json!({
                                "schema_version": whcode_domain::SCHEMA_VERSION,
                                "dry_run": true,
                                "session": session,
                                "selected_sessions": preview.selected_sessions,
                                "skipped_protected": preview.skipped_protected,
                                "counts": whcode_app::purge_counts_value(&preview.counts),
                            }),
                        )
                    }
                    Ok(_) => error_response(&WhError::internal("门面返回形状不符：应为删除预览")),
                    Err(error) => error_response(&error),
                };
            }
            match whcode_app::execute_command(
                conn,
                &policy,
                actor,
                &whcode_app::FrontendCommand::SessionDelete {
                    session_id: session.to_string(),
                    confirm: true,
                    now_ms: now,
                },
            ) {
                Ok(whcode_app::CommandOutcome::SessionDeleted(result)) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "dry_run": false,
                        "deleted_sessions": result.deleted_sessions,
                        "skipped_protected": result.skipped_protected,
                        "deleted": whcode_app::purge_counts_value(&result.deleted),
                    }),
                ),
                Ok(_) => error_response(&WhError::internal("门面返回形状不符：应为删除结果")),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/tasks/retry") | (Method::Post, "/api/tasks/abort") => {
            let body = match json_body(request) {
                Ok(body) => body,
                Err(error) => return error_response(&error),
            };
            let Ok(task) = body_str(&body, "task") else {
                return error_response(&WhError::invalid_state("缺 task 参数"));
            };
            let now = match now_ms() {
                Ok(now) => now,
                Err(error) => return error_response(&error),
            };
            let command = if route == "/api/tasks/retry" {
                whcode_app::FrontendCommand::TaskRetry {
                    task_id: task.to_string(),
                    now_ms: now,
                }
            } else {
                whcode_app::FrontendCommand::TaskAbort {
                    task_id: task.to_string(),
                    now_ms: now,
                }
            };
            match whcode_app::execute_command(conn, &policy, actor, &command) {
                Ok(whcode_app::CommandOutcome::Task(row)) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "task": whcode_app::task_value(&row),
                    }),
                ),
                Ok(_) => error_response(&WhError::internal("门面返回形状不符：应为 Task")),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/purge") => {
            // 清理（FR-204/508/509）：默认预览，`confirm=true` 才执行；审计与保护在 store。
            let body = match json_body(request) {
                Ok(body) => body,
                Err(error) => return error_response(&error),
            };
            let before_ms = match body_i64(&body, "before_ms") {
                Ok(before_ms) => before_ms,
                Err(error) => return error_response(&error),
            };
            let confirm = body
                .get("confirm")
                .map(|_| body_bool(&body, "confirm"))
                .transpose();
            let confirm = match confirm {
                Ok(confirm) => confirm.unwrap_or(false),
                Err(error) => return error_response(&error),
            };
            let now = match now_ms() {
                Ok(now) => now,
                Err(error) => return error_response(&error),
            };
            let preview = match whcode_app::execute_command(
                conn,
                &policy,
                actor,
                &whcode_app::FrontendCommand::PurgePreview {
                    before_ms,
                    now_ms: now,
                },
            ) {
                Ok(whcode_app::CommandOutcome::PurgePreview(preview)) => preview,
                Ok(_) => {
                    return error_response(&WhError::internal("门面返回形状不符：应为清理预览"));
                }
                Err(error) => return error_response(&error),
            };
            if !confirm {
                return HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "dry_run": true,
                        "cutoff_ms": preview.cutoff_ms,
                        "selected_sessions": preview.selected_sessions,
                        "skipped_protected": preview.skipped_protected,
                        "counts": whcode_app::purge_counts_value(&preview.counts),
                    }),
                );
            }
            match whcode_app::execute_command(
                conn,
                &policy,
                actor,
                &whcode_app::FrontendCommand::PurgeExecute {
                    before_ms,
                    confirm: true,
                    now_ms: now,
                },
            ) {
                Ok(whcode_app::CommandOutcome::PurgeExecuted(result)) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "dry_run": false,
                        "deleted_sessions": result.deleted_sessions,
                        "skipped_protected": result.skipped_protected,
                        "deleted": whcode_app::purge_counts_value(&result.deleted),
                    }),
                ),
                Ok(_) => error_response(&WhError::internal("门面返回形状不符：应为清理结果")),
                Err(error) => error_response(&error),
            }
        }
        // ── 证书库设置（FR-626／627、ADR-040 决策 4；T190） ────────────────
        // 读面判 `settings.cert.read`、写面判 `settings.cert.write`；私钥只进不出
        // （任何响应都不含私钥内容）。写操作的请求体在解析前按 `CERT_JSON_MAX_BYTES` 拒绝。
        (Method::Get, "/api/certs") => {
            let result = (|| -> Result<TlsStatus, WhError> {
                let cert = require_cert(cert)?;
                require(&policy, actor, Capability::SettingsCertRead)?;
                let now = now_ms()?;
                let status = cert::status(cert.paths, now)?;
                Ok(apply_restart(status, cert))
            })();
            match result {
                Ok(status) => tls_status_response(&status),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/certs/modes") => {
            let result = (|| -> Result<TlsStatus, WhError> {
                let cert = require_cert(cert)?;
                require(&policy, actor, Capability::SettingsCertWrite)?;
                let body = cert_json_body(request)?;
                if body.get("outbound").is_none() && body.get("inbound").is_none() {
                    return Err(WhError::invalid_state(
                        "至少需要 outbound 或 inbound 之一（builtin／system；\
                         builtin／external／off）",
                    ));
                }
                let current = cert::load_settings(cert.paths)?;
                let outbound = match body.get("outbound") {
                    Some(value) => TrustMode::parse(value.as_str().ok_or_else(|| {
                        WhError::parse_error("outbound 必须是字符串 builtin 或 system")
                    })?)?,
                    None => current.outbound,
                };
                let inbound = match body.get("inbound") {
                    Some(value) => InboundMode::parse(value.as_str().ok_or_else(|| {
                        WhError::parse_error("inbound 必须是字符串 builtin、external 或 off")
                    })?)?,
                    None => current.inbound,
                };
                let confirm = optional_confirm(&body)?;
                let now = now_ms()?;
                let next = cert::update_settings(
                    conn,
                    cert.paths,
                    &policy,
                    actor,
                    &cert::StoreAudit,
                    TlsSettings {
                        outbound,
                        inbound,
                        ..current
                    },
                    confirm,
                    now,
                )?;
                mark_restart(&next, cert);
                Ok(apply_restart(next, cert))
            })();
            match result {
                Ok(status) => tls_status_response(&status),
                Err(error) => error_response(&error),
            }
        }
        (Method::Get, "/api/certs/ca") => {
            let result = (|| -> Result<serde_json::Value, WhError> {
                let cert = require_cert(cert)?;
                require(&policy, actor, Capability::SettingsCertRead)?;
                let Some(kind) = request.query("kind") else {
                    return Err(WhError::invalid_state("缺 kind 参数（builtin／extra）"));
                };
                let kind = parse_ca_kind(kind.trim())?;
                let report = cert::describe_ca(cert.paths, kind)?;
                Ok(serde_json::json!({
                    "schema_version": whcode_domain::SCHEMA_VERSION,
                    "kind": kind.as_str(),
                    "pem": report.pem,
                    "cert_count": report.cert_count,
                    "sha256": report.sha256,
                    "modified": report.modified,
                }))
            })();
            match result {
                Ok(value) => HttpResponse::json(200, &value),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/certs/ca") => {
            let result = (|| -> Result<TlsStatus, WhError> {
                let cert = require_cert(cert)?;
                require(&policy, actor, Capability::SettingsCertWrite)?;
                let body = cert_json_body(request)?;
                let kind = body_ca_kind(&body)?;
                let pem = body
                    .get("pem")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        WhError::invalid_state("缺 pem 参数（字符串；extra 可为空串）")
                    })?;
                let confirm = optional_confirm(&body)?;
                let now = now_ms()?;
                let status = cert::write_ca(
                    conn,
                    cert.paths,
                    &policy,
                    actor,
                    &cert::StoreAudit,
                    kind,
                    pem,
                    confirm,
                    now,
                )?;
                mark_restart(&status, cert);
                Ok(apply_restart(status, cert))
            })();
            match result {
                Ok(status) => tls_status_response(&status),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/certs/ca/reset") => {
            let result = (|| -> Result<TlsStatus, WhError> {
                let cert = require_cert(cert)?;
                require(&policy, actor, Capability::SettingsCertWrite)?;
                let body = cert_json_body(request)?;
                let kind = match body.get("kind") {
                    Some(_) => body_ca_kind(&body)?,
                    None => CaKind::Builtin,
                };
                // `confirm` 接受但不强制：恢复内置资产／清空追加证书不是降档动作
                // （ADR-040 决策 4 只要求入站 off、空内置根与外部私钥确认）。
                let _confirm = optional_confirm(&body)?;
                let now = now_ms()?;
                let status = cert::reset_ca(
                    conn,
                    cert.paths,
                    &policy,
                    actor,
                    &cert::StoreAudit,
                    kind,
                    now,
                )?;
                mark_restart(&status, cert);
                Ok(apply_restart(status, cert))
            })();
            match result {
                Ok(status) => tls_status_response(&status),
                Err(error) => error_response(&error),
            }
        }
        (Method::Get, "/api/certs/server") => {
            let result = (|| -> Result<serde_json::Value, WhError> {
                let cert = require_cert(cert)?;
                require(&policy, actor, Capability::SettingsCertRead)?;
                let now = now_ms()?;
                let status = cert::status(cert.paths, now)?;
                // 无证书材料不是错误：`cert_pem = null`，页面据此提示上传。
                let cert_pem = match cert::read_server_material(cert.paths) {
                    Ok(material) => Some(material.cert_pem),
                    Err(error) => {
                        let present =
                            cert.paths.server_cert().is_file() || cert.paths.server_key().is_file();
                        if present {
                            return Err(error);
                        }
                        None
                    }
                };
                Ok(serde_json::json!({
                    "schema_version": whcode_domain::SCHEMA_VERSION,
                    "inbound": status.inbound,
                    "cert_pem": cert_pem,
                }))
            })();
            match result {
                Ok(value) => HttpResponse::json(200, &value),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/certs/server") => {
            let result = (|| -> Result<TlsStatus, WhError> {
                let cert = require_cert(cert)?;
                require(&policy, actor, Capability::SettingsCertWrite)?;
                let body = cert_json_body(request)?;
                let cert_pem = body
                    .get("cert_pem")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| WhError::invalid_state("缺 cert_pem 参数（PEM 证书链）"))?;
                let key_pem = body
                    .get("key_pem")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| WhError::invalid_state("缺 key_pem 参数（未加密 PEM 私钥）"))?;
                let confirm = optional_confirm(&body)?;
                // 先校验再做任何写入：不匹配的证书与私钥不留下半完成状态。
                cert::validate_server_pair(cert_pem, key_pem)?;
                let now = now_ms()?;
                let next = cert::write_server_cert(
                    conn,
                    cert.paths,
                    &policy,
                    actor,
                    &cert::StoreAudit,
                    cert_pem,
                    key_pem,
                    confirm,
                    now,
                )?;
                mark_restart(&next, cert);
                let current = cert::load_settings(cert.paths)?;
                let status = if current.inbound == InboundMode::External {
                    next
                } else {
                    cert::update_settings(
                        conn,
                        cert.paths,
                        &policy,
                        actor,
                        &cert::StoreAudit,
                        TlsSettings {
                            inbound: InboundMode::External,
                            ..current
                        },
                        confirm,
                        now,
                    )
                    .map_err(|err| {
                        WhError::new(
                            err.code,
                            format!(
                                "{}；服务端证书材料已保存，但入站档位未切换到 external：\
                                 请重试把 inbound 设为 external（材料无需重新上传）",
                                err.message
                            ),
                        )
                    })?
                };
                mark_restart(&status, cert);
                Ok(apply_restart(status, cert))
            })();
            match result {
                Ok(status) => tls_status_response(&status),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/certs/server/self-signed") => {
            let result = (|| -> Result<TlsStatus, WhError> {
                let cert = require_cert(cert)?;
                require(&policy, actor, Capability::SettingsCertWrite)?;
                let body = cert_json_body(request)?;
                let confirm = optional_confirm(&body)?;
                let now = now_ms()?;
                // 保留现有 SAN 并合并默认 SAN（`localhost`／`127.0.0.1`／`::1`／本机名）。
                let mut required_sans = cert::status(cert.paths, now)?.inbound.sans;
                for san in cert::default_sans() {
                    if !required_sans.contains(&san) {
                        required_sans.push(san);
                    }
                }
                let next = cert::regenerate_server_cert(
                    conn,
                    cert.paths,
                    &policy,
                    actor,
                    &cert::StoreAudit,
                    &required_sans,
                    confirm,
                    now,
                )?;
                mark_restart(&next, cert);
                let current = cert::load_settings(cert.paths)?;
                let status = if current.inbound == InboundMode::Builtin {
                    next
                } else {
                    cert::update_settings(
                        conn,
                        cert.paths,
                        &policy,
                        actor,
                        &cert::StoreAudit,
                        TlsSettings {
                            inbound: InboundMode::Builtin,
                            ..current
                        },
                        confirm,
                        now,
                    )
                    .map_err(|err| {
                        WhError::new(
                            err.code,
                            format!(
                                "{}；自签证书已重新生成，但入站档位未切换到 builtin：\
                                 请重试把 inbound 设为 builtin",
                                err.message
                            ),
                        )
                    })?
                };
                mark_restart(&status, cert);
                Ok(apply_restart(status, cert))
            })();
            match result {
                Ok(status) => tls_status_response(&status),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/certs/verify") => {
            let result = (|| -> Result<serde_json::Value, WhError> {
                let cert = require_cert(cert)?;
                require(&policy, actor, Capability::SettingsCertRead)?;
                let body = cert_json_body(request)?;
                let host = probe_host(body_str(&body, "host")?)?;
                let port = probe_port(&body)?;
                if is_self_probe(&host, port, cert.listen.as_ref()) {
                    return Err(WhError::invalid_state(format!(
                        "不能校验本进程自身的监听地址 {host}:{port}：serve --web 为单线程服务，\
                         校验请求处理期间无法为另一条连接推进 TLS 握手（必然超时）；\
                         请改用另一台主机或另一个服务实例作为校验目标"
                    )));
                }
                // 每次探测重新读取当前信任材料：出站 CA 变更立即生效（无需重启）。
                let material = cert::trust_material(cert.paths)?;
                let report = cert.probe.probe(&host, port, &material)?;
                Ok(serde_json::json!({
                    "schema_version": whcode_domain::SCHEMA_VERSION,
                    "ok": true,
                    "host": host,
                    "port": port,
                    "subject": report.subject,
                    "sans": report.sans,
                    "not_after": report.not_after,
                    "sha256": report.sha256,
                    "tls_version": report.tls_version,
                }))
            })();
            match result {
                Ok(value) => HttpResponse::json(200, &value),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/llm/run") => {
            // 真实调用（T167）：权限、会话、剖面、凭据与流式收集都在应用层；
            // Web 只做请求体解析与信封输出。剖面注册表来自装配根（ADR-031 决策 1）。
            let result = (|| -> Result<serde_json::Value, WhError> {
                let Some(llm) = llm else {
                    return Err(llm_not_wired());
                };
                let body = json_body(request)?;
                let session = body_str(&body, "session")?;
                let provider = body_str(&body, "provider")?;
                let prompt = body_str(&body, "prompt")?;
                let model = optional_body_str(&body, "model");
                let request_id = optional_body_str(&body, "request_id");
                let max_tokens = body.get("max_tokens").and_then(serde_json::Value::as_u64);
                let outcome = whcode_app::turn::run_turn(
                    conn,
                    &policy,
                    actor,
                    llm,
                    &whcode_app::turn::TurnRequest {
                        session_id: session.to_string(),
                        provider: provider.to_string(),
                        prompt: prompt.to_string(),
                        model: model.map(str::to_string),
                        max_tokens,
                        request_id: request_id.map(str::to_string),
                        iterations_limit: None,
                        budget_tokens: None,
                        workspace_root: workspace_root.map(std::path::Path::to_path_buf),
                        depth: 0,
                    },
                    now_ms()?,
                )?;
                serde_json::to_value(&outcome)
                    .map_err(|err| WhError::internal(format!("LLM 结果序列化失败：{err}")))
            })();
            match result {
                Ok(value) => HttpResponse::json(200, &value),
                Err(error) => error_response(&error),
            }
        }
        (Method::Post, "/api/llm/cancel") => {
            // 取消（T167）：标记文件对同机跨进程生效；Web 单线程服务无法在同请求内互相取消。
            let result = (|| -> Result<serde_json::Value, WhError> {
                let Some(llm) = llm else {
                    return Err(llm_not_wired());
                };
                let body = json_body(request)?;
                let request_id = body_str(&body, "request_id")?;
                let outcome = whcode_app::llm::cancel(&policy, actor, llm, request_id)?;
                serde_json::to_value(&outcome)
                    .map_err(|err| WhError::internal(format!("取消失败结果序列化失败：{err}")))
            })();
            match result {
                Ok(value) => HttpResponse::json(200, &value),
                Err(error) => error_response(&error),
            }
        }
        _ => HttpResponse::text(404, "not found"),
    }
}

/// 取可选的非空字符串字段（缺失／空白返回 `None`，不把空串当有效覆盖）。
fn optional_body_str<'a>(body: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    body.get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// 装配根未注入 LLM 上下文时的结构化错误（不伪装成功，也不以 404 掩盖已登记能力）。
fn llm_not_wired() -> WhError {
    WhError::invalid_state(
        "LLM 面未接线：装配根未注入 LLM 上下文；请设置 WHCODE_LLM_PROFILES（仓库外剖面 JSON）\
         后重启 serve --web（ADR-031：无剖面即无请求，默认离线）",
    )
}

use whcode_domain::TraceLevel;

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_app::actor_with_all_capabilities;

    const TEST_MIGRATIONS: &str = concat!(
        include_str!("../../whcode-store/migrations/0001_initial.sql"),
        include_str!("../../whcode-store/migrations/0002_event_trace_indexes.sql"),
        include_str!("../../whcode-store/migrations/0003_query_indexes.sql"),
        include_str!("../../whcode-store/migrations/0004_session_id_sequence.sql"),
        include_str!("../../whcode-store/migrations/0005_session_stage.sql"),
        include_str!("../../whcode-store/migrations/0006_events_variant.sql"),
    );

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(TEST_MIGRATIONS).expect("迁移");
        conn
    }

    fn request(method: Method, path: &str, auth: bool) -> HttpRequest {
        let mut headers = vec![("origin".to_string(), "http://127.0.0.1".to_string())];
        if auth {
            headers.push(("authorization".to_string(), "Bearer tok".to_string()));
        }
        HttpRequest {
            method,
            path: path.to_string(),
            headers,
            body: Vec::new(),
        }
    }

    /// 任务视图路由（批次 1）：会话范围与工作区级总览均可用（FR-103）。
    /// 文本层路由（FR-615）：locale 参数与 Accept-Language；未知 locale 回退 en；RTL 判定。
    #[test]
    fn i18n_route_serves_entries_and_rtl_flag() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("tester");
        let req = request(Method::Get, "/api/i18n?locale=en", false);
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &req,
            &actor,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["locale"], "en");
        assert_eq!(body["rtl"], false);
        assert_eq!(body["entries"]["web.tab.tasks"], "Tasks");
        assert_eq!(body["entries"]["view.sessions"], "Sessions");

        // 阿拉伯语：rtl=true（文本回退 en——未提供阿语目录，不伪造翻译）。
        let req = request(Method::Get, "/api/i18n?locale=ar", false);
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &req,
            &actor,
        );
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["rtl"], true);

        // Accept-Language 回退（无 locale 参数）。
        let mut req = request(Method::Get, "/api/i18n", false);
        req.headers
            .push(("accept-language".into(), "en-US,en;q=0.9".into()));
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &req,
            &actor,
        );
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["entries"]["web.conn.connected"], "Connected");
    }

    /// Check 101：`/api/runtimes` 暴露三档能力等级；未注入时 `wired=false`，不伪造数据。
    #[test]
    fn runtimes_route_exposes_capability_levels() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("tester");
        let inventory = serde_json::json!([
            {
                "name": "js",
                "capability": "restricted",
                "available": null,
                "unsupported": ["Node.js 内置模块"]
            },
            {
                "name": "bsdshell",
                "capability": "shell_only",
                "available": null,
                "unsupported": []
            }
        ]);
        let req = request(Method::Get, "/api/runtimes", false);
        let response = handle_request_with_runtimes(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &req,
            &actor,
            Some(&inventory),
            "zh-CN",
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["wired"], true);
        assert_eq!(body["runtimes"][0]["capability"], "restricted");
        assert_eq!(body["runtimes"][1]["capability"], "shell_only");

        // 未注入：200 + wired=false + 空数组（把“没接线”与“没有运行时”区分开）。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &req,
            &actor,
        );
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["wired"], false);
        assert_eq!(body["runtimes"].as_array().expect("数组").len(), 0);
    }

    #[test]
    fn tasks_route_reports_overview() {
        let mut conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-T', '任务视图', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("创建");
        let actor = actor_with_all_capabilities("tester");
        let mut req = request(Method::Get, "/api/tasks?session=S-T", false);
        req.headers
            .push(("origin".into(), "http://127.0.0.1".into()));
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &req,
            &actor,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["session_id"], "S-T");
        assert_eq!(body["total"], 0, "正确的空：会话存在但无任务");
        assert_eq!(body["active"], serde_json::json!([]));

        // 工作区级（缺 session）：session_id 为 null。
        let req = request(Method::Get, "/api/tasks", false);
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &req,
            &actor,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert!(body["session_id"].is_null());
    }

    #[test]
    fn api_routes_reuse_app_services() {
        let mut conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', 'API', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("创建");
        let actor = actor_with_all_capabilities("tester");

        // /api/status：快照（FR-314）。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/status?session=S-1", false),
            &actor,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["schema_version"], whcode_domain::SCHEMA_VERSION);
        assert_eq!(body["session"]["id"], "S-1");

        // /api/context（FR-622）：预算/来源/截断原因/游标四元组同源（T174）。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/context?session=S-1&budget=1024", false),
            &actor,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["schema_version"], whcode_domain::SCHEMA_VERSION);
        assert_eq!(body["budget_tokens"], 1024);
        assert!(body["items"].is_array());
        assert!(body["sources"].is_array());

        // budget=0 与非整数均拒绝（参数校验不静默）。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/context?session=S-1&budget=0", false),
            &actor,
        );
        assert_eq!(response.status, 400);
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/context?session=S-1&budget=abc", false),
            &actor,
        );
        assert_eq!(response.status, 400);

        // /api/diff（FR-622）：converge 差异同源；缺 session → invalid_state。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/diff?session=S-1", false),
            &actor,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["schema_version"], whcode_domain::SCHEMA_VERSION);
        assert_eq!(body["session_id"], "S-1");
        assert!(body["differences"].is_array());
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/diff", false),
            &actor,
        );
        assert_eq!(response.status, 400);

        // /api/events/export（FR-205）：JSON 对象与 JSONL 双格式；JSONL 首行为脱敏头。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(
                Method::Get,
                "/api/events/export?session=S-1&format=jsonl",
                false,
            ),
            &actor,
        );
        assert_eq!(response.status, 200);
        let jsonl = String::from_utf8(response.body.clone()).expect("文本");
        let header: serde_json::Value =
            serde_json::from_str(jsonl.lines().next().expect("首行")).expect("JSONL 头");
        assert_eq!(header["trace_level"], "redacted");
        assert_eq!(header["redacted"], true);
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(
                Method::Get,
                "/api/events/export?session=S-1&format=jsonl&limit=0",
                false,
            ),
            &actor,
        );
        assert_eq!(response.status, 400, "limit 越界拒绝");
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(
                Method::Get,
                "/api/events/export?session=S-1&format=csv",
                false,
            ),
            &actor,
        );
        assert_eq!(response.status, 400, "未知 format 拒绝");

        // /api/sessions：会话列表。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/sessions", false),
            &actor,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["sessions"].as_array().expect("清单").len(), 1);

        // /api/events：事件增量。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/events?session=S-1&after=0", false),
            &actor,
        );
        assert_eq!(response.status, 200);
    }

    #[test]
    fn auth_and_origin_are_enforced_on_every_request() {
        let mut conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', '安全', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("创建");
        let actor = actor_with_all_capabilities("tester");

        // 认证启用：缺 Authorization → 403（§10.2）。
        let response = handle_request(
            &mut conn,
            true,
            Some("tok"),
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/sessions", false),
            &actor,
        );
        assert_eq!(response.status, 403);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(
            body["code"], "permission_denied",
            "错误信封与 CUI 同源（FR-602）"
        );

        // 带 token：放行。
        let response = handle_request(
            &mut conn,
            true,
            Some("tok"),
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/sessions", true),
            &actor,
        );
        assert_eq!(response.status, 200);

        // Origin 非法：403。
        let mut evil = request(Method::Get, "/api/sessions", true);
        evil.headers[0] = ("origin".to_string(), "http://evil.example".to_string());
        let response = handle_request(
            &mut conn,
            true,
            Some("tok"),
            &security::ALLOWED_ORIGINS,
            &evil,
            &actor,
        );
        assert_eq!(response.status, 403);
    }

    #[test]
    fn websocket_upgrade_and_unknown_routes() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("tester");

        // /ws 升级：缺 key → 400；带 key → 101 + Sec-WebSocket-Accept（RFC 6455）。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/ws", false),
            &actor,
        );
        assert_eq!(response.status, 400);
        let mut upgrade = request(Method::Get, "/ws", false);
        upgrade.headers.push((
            "sec-websocket-key".to_string(),
            "dGhlIHNhbXBsZSBub25jZQ==".to_string(),
        ));
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &upgrade,
            &actor,
        );
        assert_eq!(response.status, 101);
        assert_eq!(
            response.header("sec-websocket-accept").expect("Accept"),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
            "RFC 6455 §1.3 规范示例值"
        );

        // 未知路由：404。
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/nope", false),
            &actor,
        );
        assert_eq!(response.status, 404);
    }

    #[test]
    fn oversized_body_is_rejected_and_requests_parse() {
        let oversized = vec![0u8; security::MAX_MESSAGE_BYTES + 1];
        assert!(
            parse_request(&oversized).is_err(),
            "体超限必须拒绝（§10.2）"
        );
        let parsed =
            parse_request(b"GET /api/sessions HTTP/1.1\r\norigin: http://127.0.0.1\r\n\r\n")
                .expect("解析");
        assert_eq!(parsed.method, Method::Get);
        assert_eq!(parsed.path, "/api/sessions");
        assert_eq!(parsed.header("Origin"), Some("http://127.0.0.1"));
    }

    /// POST 请求：写命令一律 JSON 体（§8.1）。
    fn post(path: &str, body: &serde_json::Value) -> HttpRequest {
        let mut req = request(Method::Post, path, false);
        req.body = serde_json::to_vec(body).expect("序列化");
        req
    }

    fn json_of(response: &HttpResponse) -> serde_json::Value {
        serde_json::from_slice(&response.body).expect("JSON")
    }

    /// 会话写命令（FR-609）：创建 → 重命名 → 归档 → 删除预览 → 确认删除。
    ///
    /// 破坏性命令默认只预览（`confirm` 缺失），确认后才写库——与 CUI `--dry-run`/`--yes`
    /// 同语义（FR-604）；预览阶段数据不变。
    #[test]
    fn session_write_commands_default_to_preview() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("tester");
        let call = |conn: &mut Connection, req: &HttpRequest| {
            handle_request(conn, false, None, &security::ALLOWED_ORIGINS, req, &actor)
        };

        let created = call(
            &mut conn,
            &post(
                "/api/sessions/new",
                &serde_json::json!({"title": "Web 写命令"}),
            ),
        );
        assert_eq!(created.status, 201, "创建成功返回 201");
        let body = json_of(&created);
        assert_eq!(body["schema_version"], whcode_domain::SCHEMA_VERSION);
        assert_eq!(body["session"]["title"], "Web 写命令");
        let id = body["session"]["id"].as_str().expect("会话 ID").to_string();

        let renamed = call(
            &mut conn,
            &post(
                "/api/sessions/rename",
                &serde_json::json!({"session": id, "title": "改名"}),
            ),
        );
        assert_eq!(renamed.status, 200);
        assert_eq!(json_of(&renamed)["session"]["title"], "改名");

        // 归档预览：dry_run=true、目标状态可见、数据不动。
        let archive_preview = call(
            &mut conn,
            &post(
                "/api/sessions/archive",
                &serde_json::json!({"session": id, "archived": true}),
            ),
        );
        assert_eq!(archive_preview.status, 200);
        let body = json_of(&archive_preview);
        assert_eq!(body["dry_run"], true, "缺 confirm 必须只预览");
        assert_eq!(body["current_archived"], false);
        assert_eq!(body["archived"], true);
        assert_eq!(body["changed"], true);

        let archived = call(
            &mut conn,
            &post(
                "/api/sessions/archive",
                &serde_json::json!({"session": id, "archived": true, "confirm": true}),
            ),
        );
        assert_eq!(archived.status, 200);
        assert_eq!(json_of(&archived)["session"]["archived"], true);

        // 删除预览：dry_run=true 且数据不动。
        let preview = call(
            &mut conn,
            &post("/api/sessions/delete", &serde_json::json!({"session": id})),
        );
        assert_eq!(preview.status, 200);
        let body = json_of(&preview);
        assert_eq!(body["dry_run"], true);
        assert_eq!(body["counts"]["sessions"], 1);
        // 预览不写库：会话仍可读取（归档会话不在默认列表中，故用状态接口核对存在性）。
        let still_there = call(
            &mut conn,
            &request(Method::Get, &format!("/api/status?session={id}"), false),
        );
        assert_eq!(still_there.status, 200, "预览不得写库");

        // 确认删除：dry_run=false，会话被删除。
        let deleted = call(
            &mut conn,
            &post(
                "/api/sessions/delete",
                &serde_json::json!({"session": id, "confirm": true}),
            ),
        );
        assert_eq!(deleted.status, 200);
        let body = json_of(&deleted);
        assert_eq!(body["dry_run"], false);
        assert_eq!(body["deleted_sessions"], serde_json::json!([id]));
        let gone = call(
            &mut conn,
            &request(Method::Get, &format!("/api/status?session={id}"), false),
        );
        assert_eq!(gone.status, 404, "确认后会话必须已删除");
    }

    /// 任务写命令（FR-609）：失败任务可重试（retry+1 → ready），也可中止（→ aborted）。
    #[test]
    fn task_write_commands_retry_and_abort() {
        let mut conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', '任务写命令', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("创建会话");
        conn.execute(
            "INSERT INTO tasks (id, session_id, title, stage, status, parent_id,
                                acceptance_json, retry_count, updated_at)
             VALUES ('T-1', 'S-1', '失败任务', 'implement', 'failed', NULL,
                     CAST('[]' AS JSON)::VARIANT, 0, current_timestamp)",
            [],
        )
        .expect("创建任务");
        let actor = actor_with_all_capabilities("tester");
        let call = |conn: &mut Connection, req: &HttpRequest| {
            handle_request(conn, false, None, &security::ALLOWED_ORIGINS, req, &actor)
        };

        let retried = call(
            &mut conn,
            &post("/api/tasks/retry", &serde_json::json!({"task": "T-1"})),
        );
        assert_eq!(retried.status, 200);
        let body = json_of(&retried);
        assert_eq!(body["task"]["status"], "ready");
        assert_eq!(body["task"]["retry_count"], 1);

        let aborted = call(
            &mut conn,
            &post("/api/tasks/abort", &serde_json::json!({"task": "T-1"})),
        );
        assert_eq!(aborted.status, 200);
        assert_eq!(json_of(&aborted)["task"]["status"], "aborted");

        // 未知任务：404（与 CUI 同一错误码语义）。
        let missing = call(
            &mut conn,
            &post("/api/tasks/abort", &serde_json::json!({"task": "T-404"})),
        );
        assert_eq!(missing.status, 404);
    }

    /// 清理（FR-204/508/509）：预览不写库；确认后按冻结清单删除；活跃会话受保护。
    #[test]
    fn purge_preview_then_confirm() {
        let mut conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-OLD', '旧会话', timestamp '2020-01-01 00:00:00',
                     timestamp '2020-01-01 00:00:00', 'active')",
            [],
        )
        .expect("创建");
        let actor = actor_with_all_capabilities("tester");
        let call = |conn: &mut Connection, req: &HttpRequest| {
            handle_request(conn, false, None, &security::ALLOWED_ORIGINS, req, &actor)
        };
        let cutoff = 1_600_000_000_000_i64; // 2020-09：晚于会话活动时间

        let preview = call(
            &mut conn,
            &post("/api/purge", &serde_json::json!({"before_ms": cutoff})),
        );
        assert_eq!(preview.status, 200);
        let body = json_of(&preview);
        assert_eq!(body["dry_run"], true);
        assert_eq!(body["selected_sessions"], serde_json::json!(["S-OLD"]));
        assert_eq!(body["counts"]["sessions"], 1);
        assert_eq!(
            body["counts"].as_object().expect("对象").len(),
            9,
            "九个表名全报，少报即隐瞒（FR-204）"
        );

        let executed = call(
            &mut conn,
            &post(
                "/api/purge",
                &serde_json::json!({"before_ms": cutoff, "confirm": true}),
            ),
        );
        assert_eq!(executed.status, 200);
        let body = json_of(&executed);
        assert_eq!(body["dry_run"], false);
        assert_eq!(body["deleted"]["sessions"], 1);
        let listed = call(
            &mut conn,
            &request(Method::Get, "/api/sessions?all=1", false),
        );
        assert_eq!(
            json_of(&listed)["sessions"].as_array().expect("清单").len(),
            0
        );
    }

    /// 写命令的拒绝语义：缺能力 403；体非 JSON/非对象/字段类型错 400。
    #[test]
    fn write_routes_reject_bad_bodies_and_missing_capability() {
        let mut conn = migrated();
        let weak = whcode_app::Actor::default();
        let call = |conn: &mut Connection, req: &HttpRequest, actor: &Actor| {
            handle_request(conn, false, None, &security::ALLOWED_ORIGINS, req, actor)
        };

        // 缺能力：permission_denied（不可重试），不写库。
        let denied = call(
            &mut conn,
            &post("/api/sessions/new", &serde_json::json!({"title": "越权"})),
            &weak,
        );
        assert_eq!(denied.status, 403);
        let body = json_of(&denied);
        assert_eq!(body["code"], "permission_denied");
        assert_eq!(body["retryable"], false);

        let actor = actor_with_all_capabilities("tester");
        // 体不是 JSON。
        let mut bad = request(Method::Post, "/api/sessions/new", false);
        bad.body = b"not-json".to_vec();
        assert_eq!(call(&mut conn, &bad, &actor).status, 400);
        // 体是数组而非对象。
        let mut array = request(Method::Post, "/api/sessions/new", false);
        array.body = b"[]".to_vec();
        assert_eq!(call(&mut conn, &array, &actor).status, 400);
        // 缺 title。
        assert_eq!(
            call(
                &mut conn,
                &post("/api/sessions/new", &serde_json::json!({})),
                &actor
            )
            .status,
            400
        );
        // archived 用字符串冒充布尔：显式拒绝，不做猜测。
        assert_eq!(
            call(
                &mut conn,
                &post(
                    "/api/sessions/archive",
                    &serde_json::json!({"session": "S-1", "archived": "true"}),
                ),
                &actor
            )
            .status,
            400
        );
        // purge 缺 before_ms。
        assert_eq!(
            call(
                &mut conn,
                &post("/api/purge", &serde_json::json!({})),
                &actor
            )
            .status,
            400
        );
    }

    // ── 证书 API（T190、ADR-040 决策 4） ──────────────────────────────────

    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 证书库初始化时间（固定值：证书生成可复现；状态读取用真实时钟只影响剩余天数）。
    const CERT_NOW_MS: u64 = 1_793_000_000_000;

    /// 临时工作区（只用于证书库文件；Drop 时递归删除）。
    struct CertWorkspace {
        root: PathBuf,
    }

    impl CertWorkspace {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let root = std::env::temp_dir().join(format!(
                "whcode-web-cert-{}-{tag}-{stamp}-{seq}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("创建测试工作区");
            Self { root }
        }

        fn paths(&self) -> whcode_app::cert::CertPaths {
            whcode_app::cert::CertPaths::for_workspace(&self.root)
        }
    }

    impl Drop for CertWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// 假探针：不触网；`fail` 时返回注入的超时错误（覆盖失败信封）。
    struct FakeProbe {
        fail: bool,
    }

    impl whcode_app::cert::CertProbe for FakeProbe {
        fn probe(
            &self,
            host: &str,
            port: u16,
            material: &whcode_app::cert::TrustMaterial,
        ) -> Result<whcode_app::cert::ProbeReport, WhError> {
            assert!(
                !material.builtin_pem.is_empty(),
                "探针必须收到当前信任材料（builtin）"
            );
            if self.fail {
                return Err(WhError::new(ErrorCode::Timeout, "注入的握手超时（测试）"));
            }
            Ok(whcode_app::cert::ProbeReport {
                host: host.to_string(),
                port,
                subject: Some("CN=probe.local".to_string()),
                sans: vec!["probe.local".to_string()],
                not_after: Some("2035-01-01T00:00:00Z".to_string()),
                sha256: Some("a".repeat(64)),
                tls_version: Some("TLSv1_3".to_string()),
            })
        }
    }

    /// 已初始化的证书库 + 内存库 + 探针；`send` 统一走 `handle_request_with_certs`。
    struct CertFixture {
        workspace: CertWorkspace,
        conn: Connection,
        restart_required: AtomicBool,
        probe: FakeProbe,
    }

    impl CertFixture {
        fn new(tag: &str, fail_probe: bool) -> Self {
            let workspace = CertWorkspace::new(tag);
            let sans = vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "::1".to_string(),
            ];
            whcode_app::cert::ensure_initialized(&workspace.paths(), &sans, CERT_NOW_MS)
                .expect("初始化证书库");
            Self {
                workspace,
                conn: migrated(),
                restart_required: AtomicBool::new(false),
                probe: FakeProbe { fail: fail_probe },
            }
        }

        fn paths(&self) -> whcode_app::cert::CertPaths {
            self.workspace.paths()
        }

        fn send(&mut self, request: &HttpRequest, actor: &Actor) -> HttpResponse {
            let paths = self.workspace.paths();
            let context = CertApiContext {
                paths: &paths,
                probe: &self.probe,
                restart_required: &self.restart_required,
                // 测试夹具不模拟监听地址：连通性校验照常走 FakeProbe。
                listen: None,
            };
            handle_request_with_certs(
                &mut self.conn,
                false,
                None,
                &security::ALLOWED_ORIGINS,
                request,
                actor,
                None,
                "zh-CN",
                Some(&context),
            )
        }

        /// 带自定义 Origin 与允许集的请求（Origin 语义用例）。
        fn send_with_origins(
            &mut self,
            method: Method,
            path: &str,
            body: Option<&serde_json::Value>,
            origin: &str,
            allowed: &[&str],
            actor: &Actor,
        ) -> HttpResponse {
            let request = HttpRequest {
                method,
                path: path.to_string(),
                headers: vec![("origin".to_string(), origin.to_string())],
                body: body
                    .map(|value| serde_json::to_vec(value).expect("序列化"))
                    .unwrap_or_default(),
            };
            let paths = self.workspace.paths();
            let context = CertApiContext {
                paths: &paths,
                probe: &self.probe,
                restart_required: &self.restart_required,
                listen: None,
            };
            handle_request_with_certs(
                &mut self.conn,
                false,
                None,
                allowed,
                &request,
                actor,
                None,
                "zh-CN",
                Some(&context),
            )
        }

        fn get(&mut self, path: &str, actor: &Actor) -> HttpResponse {
            self.send(&request(Method::Get, path, false), actor)
        }

        fn post(&mut self, path: &str, body: &serde_json::Value, actor: &Actor) -> HttpResponse {
            self.send(&post(path, body), actor)
        }
    }

    fn assert_no_private_key(response: &HttpResponse) {
        let text = String::from_utf8_lossy(&response.body);
        assert!(
            !text.contains("PRIVATE KEY"),
            "任何响应都不得含私钥（ADR-040 决策 1／4）：{text}"
        );
    }

    /// `GET /api/certs`：状态字段齐全、`restart_required=false`、不含私钥。
    #[test]
    fn cert_status_reads_without_private_material() {
        let mut fixture = CertFixture::new("status", false);
        let actor = actor_with_all_capabilities("cert-admin");
        let response = fixture.get("/api/certs", &actor);
        assert_eq!(
            response.status,
            200,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        assert_no_private_key(&response);
        let body = json_of(&response);
        assert_eq!(body["schema_version"], whcode_domain::SCHEMA_VERSION);
        assert_eq!(body["outbound"]["mode"], "builtin");
        assert_eq!(body["outbound"]["source"], "builtin");
        assert!(
            body["outbound"]["builtin"]["cert_count"]
                .as_u64()
                .is_some_and(|count| count > 0),
            "内置副本份数必须可见：{}",
            body["outbound"]["builtin"]["cert_count"]
        );
        assert_eq!(body["outbound"]["builtin"]["modified"], false);
        assert_eq!(body["inbound"]["mode"], "builtin");
        assert_eq!(body["inbound"]["cert_present"], true);
        assert_eq!(body["inbound"]["key_present"], true, "只报告存在性");
        assert_eq!(body["restart_required"], false);
        assert!(
            body["inbound"]["sha256"]
                .as_str()
                .is_some_and(|sha| sha.len() == 64)
        );
    }

    /// 未接线（`cert = None`）：明确 `invalid_state`，不伪装成功、不以 404 掩盖。
    #[test]
    fn cert_routes_without_context_report_not_wired() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("tester");
        let response = handle_request(
            &mut conn,
            false,
            None,
            &security::ALLOWED_ORIGINS,
            &request(Method::Get, "/api/certs", false),
            &actor,
        );
        assert_eq!(response.status, 400);
        let body = json_of(&response);
        assert_eq!(body["code"], "invalid_state");
        assert!(
            body["message"].as_str().unwrap_or("").contains("未接线"),
            "{body}"
        );
    }

    /// 能力判定：读能力不得写；写面要么拒绝、要么不落文件。
    #[test]
    fn cert_writes_require_write_capability() {
        let mut fixture = CertFixture::new("perm", false);
        let reader = Actor::new("cert-reader", [Capability::SettingsCertRead]);
        let response = fixture.post(
            "/api/certs/modes",
            &serde_json::json!({"inbound": "off", "confirm": true}),
            &reader,
        );
        assert_eq!(response.status, 403);
        let body = json_of(&response);
        assert_eq!(body["code"], "permission_denied");
        assert_eq!(body["retryable"], false);
        assert_eq!(
            whcode_app::cert::load_settings(&fixture.paths())
                .expect("读取设置")
                .inbound,
            InboundMode::Builtin,
            "权限拒绝不得改文件"
        );
        assert_eq!(fixture.get("/api/certs", &reader).status, 200, "读面允许");
    }

    /// TLS 回环：非空 `https` 允许集生效（认证关闭），明文与跨站均拒绝。
    #[test]
    fn tls_origin_allowlist_is_enforced_without_auth() {
        let mut fixture = CertFixture::new("origin", false);
        let actor = actor_with_all_capabilities("cert-admin");
        let https_origins = ["https://127.0.0.1:8443"];
        let allowed = fixture.send_with_origins(
            Method::Get,
            "/api/certs",
            None,
            "https://127.0.0.1:8443",
            &https_origins,
            &actor,
        );
        assert_eq!(
            allowed.status, 200,
            "TLS 模式 https 回环来源必须放行（ADR-022 2026-10-25 修订）"
        );
        let plain = fixture.send_with_origins(
            Method::Get,
            "/api/certs",
            None,
            "http://127.0.0.1:8443",
            &https_origins,
            &actor,
        );
        assert_eq!(plain.status, 403, "TLS 模式下明文 scheme 必须拒绝");
        let cross_site = fixture.send_with_origins(
            Method::Post,
            "/api/certs/modes",
            Some(&serde_json::json!({"inbound": "off", "confirm": true})),
            "https://evil.example",
            &https_origins,
            &actor,
        );
        assert_eq!(cross_site.status, 403, "跨站写请求必须拒绝");
        assert_eq!(
            json_of(&fixture.get("/api/certs", &actor))["inbound"]["mode"],
            "builtin",
            "跨站请求不得改文件"
        );
    }

    /// 档位切换、CA 读取／写入／重置与未知 kind／档位的明确错误。
    #[test]
    fn cert_modes_and_ca_routes_are_bounded_and_explicit() {
        let mut fixture = CertFixture::new("modes-ca", false);
        let actor = actor_with_all_capabilities("cert-admin");

        // 入站 off 未确认 → 400（说明 confirm 语义）；未知档位 → parse_error。
        let denied = fixture.post(
            "/api/certs/modes",
            &serde_json::json!({"inbound": "off"}),
            &actor,
        );
        assert_eq!(denied.status, 400);
        assert!(
            json_of(&denied)["message"]
                .as_str()
                .unwrap_or("")
                .contains("confirm")
        );
        let unknown = fixture.post(
            "/api/certs/modes",
            &serde_json::json!({"inbound": "maybe"}),
            &actor,
        );
        assert_eq!(unknown.status, 400);
        assert_eq!(json_of(&unknown)["code"], "parse_error");
        // 空档位请求 → invalid_state。
        let empty = fixture.post("/api/certs/modes", &serde_json::json!({}), &actor);
        assert_eq!(empty.status, 400);
        assert_eq!(json_of(&empty)["code"], "invalid_state");

        // 确认后执行：入站 off + restart_required = true。
        let off = fixture.post(
            "/api/certs/modes",
            &serde_json::json!({"inbound": "off", "confirm": true}),
            &actor,
        );
        assert_eq!(off.status, 200, "{}", String::from_utf8_lossy(&off.body));
        assert_eq!(json_of(&off)["inbound"]["mode"], "off");
        assert_eq!(json_of(&off)["restart_required"], true);
        fixture.post(
            "/api/certs/modes",
            &serde_json::json!({"inbound": "builtin"}),
            &actor,
        );

        // CA：追加为空 → 写入一份证书 → 重置回 0；未知 kind 明确 invalid_state。
        let initial = fixture.get("/api/certs/ca?kind=extra", &actor);
        assert_eq!(initial.status, 200);
        assert_eq!(json_of(&initial)["cert_count"], 0);
        assert_no_private_key(&initial);
        let server_pem = whcode_app::cert::read_server_material(&fixture.paths())
            .expect("读取受管材料")
            .cert_pem;
        let written = fixture.post(
            "/api/certs/ca",
            &serde_json::json!({"kind": "extra", "pem": server_pem}),
            &actor,
        );
        assert_eq!(
            written.status,
            200,
            "{}",
            String::from_utf8_lossy(&written.body)
        );
        assert_no_private_key(&written);
        let extra = json_of(&fixture.get("/api/certs/ca?kind=extra", &actor));
        assert_eq!(extra["kind"], "extra");
        assert_eq!(extra["cert_count"], 1);
        assert_eq!(extra["modified"], false, "追加文件没有“已修改”判定");
        assert!(extra["sha256"].as_str().is_some_and(|sha| sha.len() == 64));
        assert!(
            extra["pem"]
                .as_str()
                .is_some_and(|pem| pem.contains("BEGIN CERTIFICATE")),
            "读面返回公开证书 PEM"
        );
        let reset = fixture.post(
            "/api/certs/ca/reset",
            &serde_json::json!({"kind": "extra"}),
            &actor,
        );
        assert_eq!(reset.status, 200);
        assert_eq!(
            json_of(&fixture.get("/api/certs/ca?kind=extra", &actor))["cert_count"],
            0
        );
        let bad_kind = fixture.get("/api/certs/ca?kind=bogus", &actor);
        assert_eq!(bad_kind.status, 400);
        assert_eq!(json_of(&bad_kind)["code"], "invalid_state");
        let missing_kind = fixture.get("/api/certs/ca", &actor);
        assert_eq!(missing_kind.status, 400);
    }

    /// 服务端材料：公开读面、匹配上传（切 external + 重启标记）、不匹配与未确认拒绝。
    #[test]
    fn cert_server_routes_publish_public_material_only() {
        let mut fixture = CertFixture::new("server", false);
        let actor = actor_with_all_capabilities("cert-admin");

        let initial = fixture.get("/api/certs/server", &actor);
        assert_eq!(initial.status, 200);
        let body = json_of(&initial);
        assert!(
            body["cert_pem"]
                .as_str()
                .is_some_and(|pem| pem.contains("BEGIN CERTIFICATE")),
            "读面返回公开证书 PEM"
        );
        assert_eq!(body["inbound"]["mode"], "builtin");
        assert_no_private_key(&initial);

        let material =
            whcode_app::cert::read_server_material(&fixture.paths()).expect("读取受管材料");

        // 未确认 → 400 invalid_state（上传外部私钥属降档）。
        let unconfirmed = fixture.post(
            "/api/certs/server",
            &serde_json::json!({
                "cert_pem": material.cert_pem,
                "key_pem": material.key_pem,
            }),
            &actor,
        );
        assert_eq!(unconfirmed.status, 400);
        assert!(
            json_of(&unconfirmed)["message"]
                .as_str()
                .unwrap_or("")
                .contains("confirm")
        );

        // 匹配材料（本工作区自身）+ 确认 → 200，档位切到 external 并标记重启。
        let uploaded = fixture.post(
            "/api/certs/server",
            &serde_json::json!({
                "cert_pem": material.cert_pem,
                "key_pem": material.key_pem,
                "confirm": true,
            }),
            &actor,
        );
        assert_eq!(
            uploaded.status,
            200,
            "{}",
            String::from_utf8_lossy(&uploaded.body)
        );
        assert_no_private_key(&uploaded);
        let body = json_of(&uploaded);
        assert_eq!(body["inbound"]["mode"], "external");
        assert_eq!(body["restart_required"], true);
        assert!(
            whcode_app::cert::load_settings(&fixture.paths())
                .expect("读取设置")
                .inbound
                == InboundMode::External,
            "上传成功后档位必须切换（不静默留在旧档位）"
        );

        // 不匹配：用另一工作区的证书 + 本工作区私钥 → parse_error，材料不变。
        let other = CertFixture::new("server-other", false);
        let other_material =
            whcode_app::cert::read_server_material(&other.paths()).expect("另一工作区材料");
        let before_cert = std::fs::read(fixture.paths().server_cert()).expect("读取当前证书");
        let mismatch = fixture.post(
            "/api/certs/server",
            &serde_json::json!({
                "cert_pem": other_material.cert_pem,
                "key_pem": material.key_pem,
                "confirm": true,
            }),
            &actor,
        );
        assert_eq!(mismatch.status, 400);
        assert!(
            json_of(&mismatch)["message"]
                .as_str()
                .unwrap_or("")
                .contains("不匹配")
        );
        assert_eq!(
            std::fs::read(fixture.paths().server_cert()).expect("读取"),
            before_cert,
            "不匹配不得改变材料"
        );

        // 重新生成自签：指纹变化、档位回 builtin、SAN 覆盖默认集合。
        let before_sha = json_of(&fixture.get("/api/certs", &actor))["inbound"]["sha256"].clone();
        let regenerated = fixture.post(
            "/api/certs/server/self-signed",
            &serde_json::json!({}),
            &actor,
        );
        assert_eq!(
            regenerated.status,
            200,
            "{}",
            String::from_utf8_lossy(&regenerated.body)
        );
        assert_no_private_key(&regenerated);
        let body = json_of(&regenerated);
        assert_ne!(
            body["inbound"]["sha256"], before_sha,
            "重新生成后指纹必须变化"
        );
        assert_eq!(body["inbound"]["mode"], "builtin");
        assert_eq!(body["restart_required"], true);
        let sans = body["inbound"]["sans"].as_array().expect("SAN 数组");
        for expected in ["localhost", "127.0.0.1", "::1"] {
            assert!(
                sans.iter().any(|value| value == expected),
                "SAN 必须覆盖 {expected}：{sans:?}"
            );
        }

        // 无材料：`cert_pem = null`（不是错误）。
        std::fs::remove_file(fixture.paths().server_cert()).expect("删除证书");
        std::fs::remove_file(fixture.paths().server_key()).expect("删除私钥");
        let missing = fixture.get("/api/certs/server", &actor);
        assert_eq!(missing.status, 200, "无证书不是错误");
        let body = json_of(&missing);
        assert!(body["cert_pem"].is_null());
        assert_eq!(body["inbound"]["cert_present"], false);
        assert_no_private_key(&missing);
    }

    /// 连通性校验：成功载荷 + 主机／端口校验 + 超时结构化错误。
    #[test]
    fn cert_verify_reports_probe_result_and_structured_errors() {
        let mut fixture = CertFixture::new("verify", false);
        let actor = actor_with_all_capabilities("cert-admin");

        let ok = fixture.post(
            "/api/certs/verify",
            &serde_json::json!({"host": "probe.local", "port": 8443}),
            &actor,
        );
        assert_eq!(ok.status, 200, "{}", String::from_utf8_lossy(&ok.body));
        let body = json_of(&ok);
        assert_eq!(body["schema_version"], whcode_domain::SCHEMA_VERSION);
        assert_eq!(body["ok"], true);
        assert_eq!(body["host"], "probe.local");
        assert_eq!(body["port"], 8443);
        assert_eq!(body["subject"], "CN=probe.local");
        assert_eq!(body["tls_version"], "TLSv1_3");
        assert_no_private_key(&ok);

        // 缺省端口 443。
        let default_port = fixture.post(
            "/api/certs/verify",
            &serde_json::json!({"host": "probe.local"}),
            &actor,
        );
        assert_eq!(json_of(&default_port)["port"], 443);

        for (payload, expected) in [
            (
                serde_json::json!({"host": "https://probe.local"}),
                "不得包含",
            ),
            (serde_json::json!({"host": "user@probe.local"}), "不得包含"),
            (serde_json::json!({"host": "probe.local/path"}), "不得包含"),
            (
                serde_json::json!({"host": "probe.local", "port": 0}),
                "1..=65535",
            ),
            (
                serde_json::json!({"host": "probe.local", "port": 70000}),
                "1..=65535",
            ),
        ] {
            let response = fixture.post("/api/certs/verify", &payload, &actor);
            assert_eq!(response.status, 400, "{payload}");
            assert!(
                json_of(&response)["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains(expected),
                "{payload} → 响应消息须包含预期原因"
            );
        }

        // 探针失败：结构化错误信封（不返回 ok:true）。
        let mut failing = CertFixture::new("verify-timeout", true);
        let response = failing.post(
            "/api/certs/verify",
            &serde_json::json!({"host": "probe.local"}),
            &actor,
        );
        assert_eq!(response.status, 500);
        let body = json_of(&response);
        assert_eq!(body["code"], "timeout");
        assert_eq!(body["retryable"], true);
        assert!(body["ok"].is_null(), "错误信封不得携带 ok:true");
    }

    /// 重启标记：入站变更置位、根证书与读操作不置位，且置位后读面保持可见。
    #[test]
    fn restart_flag_tracks_inbound_changes_only() {
        let mut fixture = CertFixture::new("restart", false);
        let actor = actor_with_all_capabilities("cert-admin");
        assert_eq!(
            json_of(&fixture.get("/api/certs", &actor))["restart_required"],
            false
        );

        let server_pem = whcode_app::cert::read_server_material(&fixture.paths())
            .expect("读取受管材料")
            .cert_pem;
        let written = fixture.post(
            "/api/certs/ca",
            &serde_json::json!({"kind": "extra", "pem": server_pem}),
            &actor,
        );
        assert_eq!(written.status, 200);
        assert_eq!(
            json_of(&written)["restart_required"],
            false,
            "根证书变更不需要重启"
        );
        assert!(
            !fixture.restart_required.load(Ordering::Relaxed),
            "读操作与根证书写入不置位"
        );

        let off = fixture.post(
            "/api/certs/modes",
            &serde_json::json!({"inbound": "off", "confirm": true}),
            &actor,
        );
        assert_eq!(json_of(&off)["restart_required"], true);
        assert!(fixture.restart_required.load(Ordering::Relaxed));
        assert_eq!(
            json_of(&fixture.get("/api/certs", &actor))["restart_required"],
            true,
            "置位后读面必须持续可见"
        );
    }

    /// 请求体上限：超出 `CERT_JSON_MAX_BYTES` 在解析前拒绝（不明文解析超限载荷）。
    #[test]
    fn cert_request_body_is_rejected_before_parsing() {
        let mut fixture = CertFixture::new("limit", false);
        let actor = actor_with_all_capabilities("cert-admin");
        let oversized = HttpRequest {
            method: Method::Post,
            path: "/api/certs/modes".to_string(),
            headers: vec![("origin".to_string(), "http://127.0.0.1".to_string())],
            body: vec![b'x'; whcode_domain::CERT_JSON_MAX_BYTES + 1],
        };
        let response = fixture.send(&oversized, &actor);
        assert_eq!(response.status, 400);
        let body = json_of(&response);
        assert_eq!(body["code"], "parse_error");
        assert!(
            body["message"].as_str().unwrap_or("").contains("上限"),
            "{body}"
        );
        // 上限内的空体按空对象处理（self-signed 只需可选 confirm）。
        let empty = HttpRequest {
            method: Method::Post,
            path: "/api/certs/server/self-signed".to_string(),
            headers: vec![("origin".to_string(), "http://127.0.0.1".to_string())],
            body: Vec::new(),
        };
        assert_eq!(fixture.send(&empty, &actor).status, 200);
    }

    /// 写审计：成功的证书变更写 `tls_config_change`，detail 不含 PEM 正文。
    #[test]
    fn cert_writes_are_audited_without_pem_bodies() {
        let mut fixture = CertFixture::new("audit", false);
        let actor = actor_with_all_capabilities("cert-admin");
        let off = fixture.post(
            "/api/certs/modes",
            &serde_json::json!({"inbound": "off", "confirm": true}),
            &actor,
        );
        assert_eq!(off.status, 200);
        let settings_rows: i64 = fixture
            .conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'tls_config_change' \
                 AND target = 'tls:settings'",
                [],
                |row| row.get(0),
            )
            .expect("统计审计");
        assert_eq!(
            settings_rows, 1,
            "档位变更必须写一条 tls_config_change 审计"
        );
        let (detail,): (String,) = fixture
            .conn
            .query_row(
                "SELECT CAST(detail_json AS VARCHAR) FROM audit WHERE target = 'tls:settings'",
                [],
                |row| Ok((row.get(0)?,)),
            )
            .expect("读取审计 detail");
        assert!(
            !detail.contains("PRIVATE KEY"),
            "审计正文不得含私钥：{detail}"
        );
        assert!(!detail.contains("BEGIN CERTIFICATE"), "审计不得含 PEM 正文");
        assert!(
            detail.contains("inbound") && detail.contains("off"),
            "审计须含档位变化：{detail}"
        );
    }
}
