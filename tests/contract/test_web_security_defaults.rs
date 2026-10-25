//! Web 安全默认值契约测试（T095、V-110、FR-606）：
//! 默认仅回环绑定；对外监听必须显式开启认证与 Origin 检查，否则拒绝启动；
//! token 不经 URL query；三端（含 Web）权限与错误信封同源（FR-604/602）。

use duckdb::Connection;
use whcode_web::api::{Method, handle_request, parse_request};
use whcode_web::bind::{BindConfig, bind, is_loopback};
use whcode_web::security::{check_auth, check_origin, token_in_query};

fn request(
    method: Method,
    path: &str,
    headers: Vec<(String, String)>,
) -> whcode_web::api::HttpRequest {
    whcode_web::api::HttpRequest {
        method,
        path: path.to_string(),
        headers,
        body: Vec::new(),
    }
}

/// V-110：默认仅回环绑定；对外监听未开认证/Origin → 拒绝启动（FR-606）。
#[test]
fn v110_default_loopback_and_external_requires_auth() {
    assert!(is_loopback(whcode_web::bind::DEFAULT_BIND));
    let listener = bind(&BindConfig::loopback()).expect("默认回环绑定合法");
    assert!(listener.local_addr().expect("地址").ip().is_loopback());

    let external = BindConfig {
        addr: "10.0.0.9:0".to_string(),
        auth_enabled: false,
        origin_check_enabled: false,
    };
    let err = bind(&external).expect_err("V-110：对外监听未开认证必须拒绝启动");
    assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
    assert!(!err.retryable);
    assert!(
        err.to_string().contains("FR-606"),
        "错误须援引 FR-606：{err}"
    );
}

/// token 不经 URL query（§10.2）。
#[test]
fn token_never_passes_through_url_query() {
    assert!(token_in_query("/api/status?token=sekret", "sekret"));
    assert!(!token_in_query("/api/status", "sekret"));
    assert!(!token_in_query("/api/status?session=S-1", "sekret"));
    // 正确方式：Authorization 头（恒定时间比较）。
    assert!(check_auth(true, Some("sekret"), Some("Bearer sekret")).is_ok());
    assert!(check_auth(true, Some("sekret"), Some("Bearer wrong")).is_err());
}

/// Origin 必须命中允许集（§10.2）；回环默认放行。
///
/// 2026-10-22 修订（ADR-022/ADR-023）：命中按 scheme + host（端口不参与——浏览器
/// 携带实际端口）；**缺失 Origin 两种模式均放行**（同源 GET 与 CLI 客户端不带
/// Origin；认证模式由 Bearer token 阻断跨站）。威胁模型不变：非回环 host 一律拒绝。
///
/// 2026-10-25 修订（ADR-022、ADR-040 决策 3）：调用方传入**非空**允许集时始终生效
/// （与认证无关）——TLS 档位注入的 `https://…` 允许集必须被接受。
#[test]
fn origin_checks_apply() {
    // 回环来源：任意端口命中；真实浏览器 Origin 形如 http://127.0.0.1:<port>。
    assert!(check_origin(&[], Some("http://127.0.0.1")).is_ok());
    assert!(check_origin(&[], Some("http://127.0.0.1:8737")).is_ok());
    assert!(check_origin(&[], Some("http://localhost:3000")).is_ok());
    // 非回环来源：一律拒绝（含相似前缀与任意端口）。
    assert!(check_origin(&[], Some("http://evil.example")).is_err());
    assert!(check_origin(&[], Some("http://127.0.0.1.evil.example")).is_err());
    // TLS 档位的 https 允许集：同源 https 放行、明文 http 拒绝（scheme 参与判定）。
    assert!(check_origin(&["https://127.0.0.1:8443"], Some("https://127.0.0.1:8443")).is_ok());
    assert!(check_origin(&["https://127.0.0.1:8443"], Some("http://127.0.0.1:8443")).is_err());
    // 缺失 Origin：均放行（ADR-022）；跨站由 Bearer 阻断。
    assert!(
        check_origin(&[], None).is_ok(),
        "回环模式缺 Origin 放行（ADR-022）"
    );
    assert!(
        check_origin(&whcode_web::security::ALLOWED_ORIGINS, None).is_ok(),
        "认证模式缺 Origin 放行（ADR-023）；跨站由 Bearer 阻断"
    );
}

/// 三端（含 Web）权限与错误信封同源（FR-604/602）：Web API 的错误对象
/// 与 CUI 信封同一结构（schema_version + 四字段）。
#[test]
fn web_error_envelope_matches_cui() {
    let mut conn = Connection::open_in_memory().expect("内存库");
    conn.execute_batch(concat!(
        include_str!("../../crates/whcode-store/migrations/0001_initial.sql"),
        include_str!("../../crates/whcode-store/migrations/0002_event_trace_indexes.sql"),
        include_str!("../../crates/whcode-store/migrations/0003_query_indexes.sql"),
        include_str!("../../crates/whcode-store/migrations/0004_session_id_sequence.sql"),
        include_str!("../../crates/whcode-store/migrations/0005_session_stage.sql"),
        include_str!("../../crates/whcode-store/migrations/0006_events_variant.sql"),
    ))
    .expect("迁移");
    let actor = whcode_app::actor_with_all_capabilities("tester");

    // 认证启用 + 缺 Authorization → 403 + permission_denied（与 CUI 同源错误码）。
    let response = handle_request(
        &mut conn,
        true,
        Some("tok"),
        &whcode_web::security::ALLOWED_ORIGINS,
        &request(Method::Get, "/api/sessions", vec![]),
        &actor,
    );
    assert_eq!(response.status, 403);
    let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
    assert_eq!(body["schema_version"], whcode_domain::SCHEMA_VERSION);
    for field in ["code", "message", "retryable", "details"] {
        assert!(
            body.get(field).is_some(),
            "错误对象必须含 {field}（FR-602）"
        );
    }
    assert_eq!(body["code"], "permission_denied");

    // CUI 错误信封同源对比。
    let cui_error =
        whcode_cui::output::error_envelope(&whcode_domain::WhError::permission_denied("x"));
    let cui_value = serde_json::to_value(&cui_error).expect("序列化");
    assert_eq!(cui_value["code"], body["code"], "三端错误码同源（FR-604）");
}

/// 请求体有界（§10.2）。
#[test]
fn request_bodies_are_bounded() {
    let oversized = vec![0u8; whcode_web::security::MAX_MESSAGE_BYTES + 1];
    assert!(parse_request(&oversized).is_err());
    let bounded = parse_request(b"GET /api/sessions HTTP/1.1\r\n\r\n").expect("有界请求可解析");
    assert_eq!(bounded.method, Method::Get);
}
