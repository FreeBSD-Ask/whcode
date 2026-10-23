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

use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::Connection;
use whcode_app::{
    Actor, DefaultPolicy, NewSession, PurgeCriteria, PurgeProtection, build_snapshot,
};
use whcode_domain::{ErrorCode, WhError};

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
    )
}

/// 同 [`handle_request`]，并注入运行时清单（FR-702、Check 101）。
///
/// `runtimes` 为装配根预先序列化的声明数组（`whcode-bin::runtimes::inventory`）；
/// `None` 表示未注入——响应仍为 200，但 `wired=false`，便于把“没接线”与“没有运行时”区分开。
pub fn handle_request_with_runtimes(
    conn: &mut Connection,
    auth_enabled: bool,
    token: Option<&str>,
    allowed_origins: &[&str],
    request: &HttpRequest,
    actor: &Actor,
    runtimes: Option<&serde_json::Value>,
) -> HttpResponse {
    // 认证 + Origin（§10.2、V-110）。
    if let Err(error) = security::check_auth(auth_enabled, token, request.header("authorization")) {
        return error_response(&error);
    }
    // Origin 允许集默认回环来源；对外部署须显式扩展（V-110 门禁在 bind 层）。
    if let Err(error) =
        security::check_origin(auth_enabled, allowed_origins, request.header("origin"))
    {
        return error_response(&error);
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
                .unwrap_or_else(|| "zh-CN".to_string());
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
            match whcode_app::new_session(
                conn,
                &policy,
                actor,
                NewSession {
                    title: title.to_string(),
                    model,
                },
                now,
            ) {
                Ok(row) => HttpResponse::json(
                    201,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "session": whcode_app::session_value(&row),
                    }),
                ),
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
            match whcode_app::rename_session(conn, &policy, actor, session, title) {
                Ok(row) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "session": whcode_app::session_value(&row),
                    }),
                ),
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
            let ids = [session];
            match whcode_app::set_session_archived(conn, &policy, actor, &ids, archived) {
                Ok(_) => match whcode_app::show_session(conn, &policy, actor, session) {
                    Ok(row) => HttpResponse::json(
                        200,
                        &serde_json::json!({
                            "schema_version": whcode_domain::SCHEMA_VERSION,
                            "session": whcode_app::session_value(&row),
                        }),
                    ),
                    Err(error) => error_response(&error),
                },
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
            // 用户点名删除 + confirm：不套用 TTL 活跃窗口（与 CUI 同口径）。
            let protection = PurgeProtection {
                active_window_ms: 0,
                ..PurgeProtection::default()
            };
            if !confirm {
                return match whcode_app::preview_delete_session(
                    conn,
                    &policy,
                    actor,
                    session,
                    &protection,
                    now,
                ) {
                    Ok(preview) => HttpResponse::json(
                        200,
                        &serde_json::json!({
                            "schema_version": whcode_domain::SCHEMA_VERSION,
                            "dry_run": true,
                            "session": session,
                            "selected_sessions": preview.selected_sessions,
                            "skipped_protected": preview.skipped_protected,
                            "counts": whcode_app::purge_counts_value(&preview.counts),
                        }),
                    ),
                    Err(error) => error_response(&error),
                };
            }
            match whcode_app::delete_session(conn, &policy, actor, session, true, &protection, now)
            {
                Ok(result) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "dry_run": false,
                        "deleted_sessions": result.deleted_sessions,
                        "skipped_protected": result.skipped_protected,
                        "deleted": whcode_app::purge_counts_value(&result.deleted),
                    }),
                ),
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
            let outcome = if route == "/api/tasks/retry" {
                whcode_app::retry_task(conn, &policy, actor, task, now)
            } else {
                whcode_app::abort_task(conn, &policy, actor, task, now)
            };
            match outcome {
                Ok(row) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "task": whcode_app::task_value(&row),
                    }),
                ),
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
            let protection = PurgeProtection::default();
            let criteria = PurgeCriteria::before(before_ms);
            let preview = match whcode_app::purge_preview(
                conn,
                &policy,
                actor,
                &criteria,
                &protection,
                now,
            ) {
                Ok(preview) => preview,
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
            match whcode_app::purge_execute(conn, &policy, actor, &preview, true, &protection, now)
            {
                Ok(result) => HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "schema_version": whcode_domain::SCHEMA_VERSION,
                        "dry_run": false,
                        "deleted_sessions": result.deleted_sessions,
                        "skipped_protected": result.skipped_protected,
                        "deleted": whcode_app::purge_counts_value(&result.deleted),
                    }),
                ),
                Err(error) => error_response(&error),
            }
        }
        _ => HttpResponse::text(404, "not found"),
    }
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

        let archived = call(
            &mut conn,
            &post(
                "/api/sessions/archive",
                &serde_json::json!({"session": id, "archived": true}),
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
             VALUES ('T-1', 'S-1', '失败任务', 'implement', 'failed', NULL, '[]', 0,
                     current_timestamp)",
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
}
