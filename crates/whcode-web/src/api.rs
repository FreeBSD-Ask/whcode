//! # HTTP API 与 WebSocket（T090、FR-603、ADR-004）
//!
//! 复用应用服务命令/快照/事件（分层不变：Web 是前端适配器，**只**经 `whcode-app`
//! 访问状态，禁止触库——ADR-003/005、C-03；本模块的 `&Connection` 由装配根透传）。
//!
//! 路由（M6 口径）：
//! - `GET /api/status?session=S` → 状态快照（FR-314）；
//! - `GET /api/sessions` → 会话列表（FR-101）；
//! - `GET /api/events?session=S&after=N` → 事件增量（FR-202/605）；
//! - `GET /ws` → WebSocket 升级（服务端推送事件文本帧，RFC 6455）。
//!
//! 每个请求经 [`crate::security`] 认证 + Origin 检查（§10.2）；请求体有界
//! （[`MAX_MESSAGE_BYTES`]）。`whcode` 是**原生 Web UI**，不是 TUI 的 PTY 镜像
//! （ADR-004）。

use duckdb::Connection;
use whcode_app::{Actor, DefaultPolicy, build_snapshot};
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

/// 处理单个 HTTP 请求（T090）：认证 + Origin 检查 → 路由 → 应用服务。
///
/// # Errors
///
/// 本函数把错误编码为 HTTP 响应（错误信封与 CUI 同源，FR-602），不返回 `Err`；
/// 签名保留 `Result` 以便装配根统一处理。
pub fn handle_request(
    conn: &Connection,
    auth_enabled: bool,
    token: Option<&str>,
    request: &HttpRequest,
    actor: &Actor,
) -> HttpResponse {
    // 认证 + Origin（§10.2、V-110）。
    if let Err(error) = security::check_auth(auth_enabled, token, request.header("authorization")) {
        return error_response(&error);
    }
    // Origin 允许集默认回环来源；对外部署须显式扩展（V-110 门禁在 bind 层）。
    if let Err(error) = security::check_origin(
        auth_enabled,
        &security::ALLOWED_ORIGINS,
        request.header("origin"),
    ) {
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
        (Method::Get, "/ws") => {
            // WebSocket 升级：Origin 校验（安全层已做）+ Sec-WebSocket-Key。
            let Some(key) = request.header("sec-websocket-key") else {
                return error_response(&WhError::invalid_state(
                    "WebSocket 升级缺 Sec-WebSocket-Key（RFC 6455）",
                ));
            };
            HttpResponse::websocket_upgrade(&ws::accept_key(key))
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
    #[test]
    fn tasks_route_reports_overview() {
        let conn = migrated();
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
        let response = handle_request(&conn, false, None, &req, &actor);
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["session_id"], "S-T");
        assert_eq!(body["total"], 0, "正确的空：会话存在但无任务");
        assert_eq!(body["active"], serde_json::json!([]));

        // 工作区级（缺 session）：session_id 为 null。
        let req = request(Method::Get, "/api/tasks", false);
        let response = handle_request(&conn, false, None, &req, &actor);
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert!(body["session_id"].is_null());
    }

    #[test]
    fn api_routes_reuse_app_services() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', 'API', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("创建");
        let actor = actor_with_all_capabilities("tester");

        // /api/status：快照（FR-314）。
        let response = handle_request(
            &conn,
            false,
            None,
            &request(Method::Get, "/api/status?session=S-1", false),
            &actor,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["schema_version"], whcode_domain::SCHEMA_VERSION);
        assert_eq!(body["session"]["id"], "S-1");

        // /api/sessions：会话列表。
        let response = handle_request(
            &conn,
            false,
            None,
            &request(Method::Get, "/api/sessions", false),
            &actor,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
        assert_eq!(body["sessions"].as_array().expect("清单").len(), 1);

        // /api/events：事件增量。
        let response = handle_request(
            &conn,
            false,
            None,
            &request(Method::Get, "/api/events?session=S-1&after=0", false),
            &actor,
        );
        assert_eq!(response.status, 200);
    }

    #[test]
    fn auth_and_origin_are_enforced_on_every_request() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', '安全', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("创建");
        let actor = actor_with_all_capabilities("tester");

        // 认证启用：缺 Authorization → 403（§10.2）。
        let response = handle_request(
            &conn,
            true,
            Some("tok"),
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
            &conn,
            true,
            Some("tok"),
            &request(Method::Get, "/api/sessions", true),
            &actor,
        );
        assert_eq!(response.status, 200);

        // Origin 非法：403。
        let mut evil = request(Method::Get, "/api/sessions", true);
        evil.headers[0] = ("origin".to_string(), "http://evil.example".to_string());
        let response = handle_request(&conn, true, Some("tok"), &evil, &actor);
        assert_eq!(response.status, 403);
    }

    #[test]
    fn websocket_upgrade_and_unknown_routes() {
        let conn = migrated();
        let actor = actor_with_all_capabilities("tester");

        // /ws 升级：缺 key → 400；带 key → 101 + Sec-WebSocket-Accept（RFC 6455）。
        let response = handle_request(
            &conn,
            false,
            None,
            &request(Method::Get, "/ws", false),
            &actor,
        );
        assert_eq!(response.status, 400);
        let mut upgrade = request(Method::Get, "/ws", false);
        upgrade.headers.push((
            "sec-websocket-key".to_string(),
            "dGhlIHNhbXBsZSBub25jZQ==".to_string(),
        ));
        let response = handle_request(&conn, false, None, &upgrade, &actor);
        assert_eq!(response.status, 101);
        assert_eq!(
            response.header("sec-websocket-accept").expect("Accept"),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
            "RFC 6455 §1.3 规范示例值"
        );

        // 未知路由：404。
        let response = handle_request(
            &conn,
            false,
            None,
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
}
