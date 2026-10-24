//! TUI/CUI 一致性契约测试（T088、FR-604、V-109 部分）。
//!
//! TUI 命令面（T082，六视图）尚未交付——本测试在**已交付面**上验证一致性契约：
//! 1. **命令路由同源**：CUI 命令树（T084）解析出的每个命令都映射到
//!    `whcode-app` 的同一批命令函数——TUI 的会话面（T024 基线）与 CUI 均只经
//!    应用服务访问状态（ADR-003/005、C-03），前端不触库；
//! 2. **权限语义一致**（FR-604）：同一调用者能力集合下，TUI 与 CUI 走同一
//!    `authz` 判定——拒绝均为 `permission_denied`（不可重试）；
//! 3. **错误码一致**：两前端的错误信封同源（`output::error_envelope`）。
//!
//! 同命令最终状态一致（2026-10-22 补齐，T082/T084/T090 交付后）：同一夹具库上，
//! CUI 命令路径（`whcode_cui` 视图函数 + `whcode-app` 只读命令）与 Web API
//! （`handle_request`）返回同一份数据（事件序列/会话清单/任务总览）；写命令的跨端
//! 一致性仍阻塞于 Web 写 API（未交付）。不得删除既有断言。

use whcode_app::{Actor, Capability, DefaultPolicy, actor_with_all_capabilities};
use whcode_cui::cli::{Cli, Command, parse};
use whcode_domain::ErrorCode;

/// CUI 命令树覆盖的 app 能力集合（路由同源的结构断言）。
fn cui_capabilities(command: &Command) -> Vec<Capability> {
    match command {
        Command::Session { .. } => vec![
            Capability::SessionCreate,
            Capability::SessionRead,
            Capability::SessionRename,
            Capability::SessionArchive,
            Capability::SessionDelete,
        ],
        Command::Task { .. } => vec![
            Capability::TaskRead,
            Capability::TaskRetry,
            Capability::TaskAbort,
        ],
        Command::Spec { .. } => vec![Capability::SpecAdvance],
        Command::Status { .. } | Command::Events { .. } => vec![Capability::SessionRead],
        _ => vec![],
    }
}

#[test]
fn cui_command_tree_routes_to_app_capabilities() {
    // 每个 CUI 子命令都映射到 whcode-app 能力（同源路由，不触库）。
    let commands: Vec<Command> = vec![
        serde_parse(["whcode", "session", "new", "--title", "t"]).command,
        serde_parse(["whcode", "task", "list", "--session", "S-1"]).command,
        serde_parse(["whcode", "spec", "status", "--session", "S-1"]).command,
        serde_parse(["whcode", "events", "list", "--session", "S-1"]).command,
        serde_parse(["whcode", "status", "--session", "S-1"]).command,
    ];
    for command in &commands {
        let capabilities = cui_capabilities(command);
        assert!(
            !capabilities.is_empty(),
            "CUI 命令必须映射到 app 能力：{command:?}"
        );
    }
}

fn serde_parse<const N: usize>(args: [&str; N]) -> Cli {
    parse(args).expect("解析")
}

#[test]
fn permission_semantics_identical_for_both_frontends() {
    // FR-604：同一能力集合 → 同一判定。TUI 与 CUI 均经 authz（单点判定），
    // 这里验证判定结果对调用者能力集合是确定性的（前端无关）。
    let reader = Actor::new("reader", [Capability::SessionRead]);
    let full = actor_with_all_capabilities("admin");
    let policy = DefaultPolicy;

    for capability in [
        Capability::SessionCreate,
        Capability::SessionDelete,
        Capability::TaskRetry,
        Capability::SpecAdvance,
    ] {
        let denied_for_reader = whcode_app::require(&policy, &reader, capability).is_err();
        let allowed_for_admin = whcode_app::require(&policy, &full, capability).is_ok();
        assert!(
            denied_for_reader || capability == Capability::SessionRead,
            "无能力调用必须拒绝（FR-604）：{capability:?}"
        );
        assert!(allowed_for_admin, "有能力调用必须放行：{capability:?}");
    }
}

/// 同命令最终状态一致（V-109 追加用例，2026-10-22）：
/// 同一夹具库、同一应用服务——CUI 视图/只读命令与 Web API 的数据必须一致。
#[test]
fn same_read_commands_yield_identical_state_across_cui_and_web() {
    use duckdb::Connection;

    use whcode_app::{DefaultPolicy, actor_with_all_capabilities};
    use whcode_web::api::Method;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(concat!(
            include_str!("../../crates/whcode-store/migrations/0001_initial.sql"),
            include_str!("../../crates/whcode-store/migrations/0002_event_trace_indexes.sql"),
            include_str!("../../crates/whcode-store/migrations/0003_query_indexes.sql"),
            include_str!("../../crates/whcode-store/migrations/0004_session_id_sequence.sql"),
            include_str!("../../crates/whcode-store/migrations/0005_session_stage.sql"),
            include_str!("../../crates/whcode-store/migrations/0006_events_variant.sql"),
        ))
        .expect("迁移");
        conn
    }

    let mut conn = migrated();
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, last_active_at, status, stage)
         VALUES ('S-P', '一致性', epoch_ms(1000), epoch_ms(1000), 'active', 'implement')",
        [],
    )
    .expect("会话");
    for (seq, kind) in [(1_i64, "RequestStarted"), (2, "StreamingChunk")] {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, 'S-P', NULL, NULL, NULL, ?, CAST('{}' AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            duckdb::params![seq, format!("E-{seq}"), kind],
        )
        .expect("事件");
    }
    let policy = DefaultPolicy;
    let actor = actor_with_all_capabilities("parity");

    let request = |method: Method, path: &str| whcode_web::api::HttpRequest {
        method,
        path: path.to_string(),
        headers: vec![("origin".to_string(), "http://127.0.0.1".to_string())],
        body: Vec::new(),
    };

    // 1) 事件列表：CUI 视图函数 vs Web /api/events。
    let cui_events = whcode_cui::events_cmd::events_json(
        &conn,
        &policy,
        &actor,
        "S-P",
        0,
        100,
        whcode_domain::TraceLevel::default(),
    )
    .expect("CUI 事件");
    let web_events = whcode_web::api::handle_request(
        &mut conn,
        false,
        None,
        &whcode_web::security::ALLOWED_ORIGINS,
        &request(Method::Get, "/api/events?session=S-P&after=0"),
        &actor,
    );
    assert_eq!(web_events.status, 200);
    let web_events: serde_json::Value = serde_json::from_slice(&web_events.body).expect("Web JSON");
    let cui_seq: Vec<i64> = cui_events["events"]
        .as_array()
        .expect("数组")
        .iter()
        .map(|event| event["seq"].as_i64().expect("seq"))
        .collect();
    let web_seq: Vec<i64> = web_events["events"]
        .as_array()
        .expect("数组")
        .iter()
        .map(|event| event["seq"].as_i64().expect("seq"))
        .collect();
    assert_eq!(cui_seq, web_seq, "同命令事件序列必须一致");
    assert_eq!(
        cui_events["trace_level"], web_events["trace_level"],
        "脱敏策略注明一致"
    );

    // 2) 会话清单：CUI 命令路径（app::list_sessions）vs Web /api/sessions。
    let cui_sessions =
        whcode_app::list_sessions(&conn, &policy, &actor, false, 200).expect("CUI 会话");
    let web_sessions = whcode_web::api::handle_request(
        &mut conn,
        false,
        None,
        &whcode_web::security::ALLOWED_ORIGINS,
        &request(Method::Get, "/api/sessions"),
        &actor,
    );
    let web_sessions: serde_json::Value =
        serde_json::from_slice(&web_sessions.body).expect("Web JSON");
    let cui_ids: Vec<&str> = cui_sessions.iter().map(|row| row.id.as_str()).collect();
    let web_ids: Vec<&str> = web_sessions["sessions"]
        .as_array()
        .expect("数组")
        .iter()
        .filter_map(|row| row["id"].as_str())
        .collect();
    assert_eq!(cui_ids, web_ids, "同命令会话清单必须一致");

    // 3) 任务总览：CUI 命令路径（app::task_overview）vs Web /api/tasks。
    let cui_tasks =
        whcode_app::task_overview(&conn, &policy, &actor, Some("S-P")).expect("CUI 任务总览");
    let web_tasks = whcode_web::api::handle_request(
        &mut conn,
        false,
        None,
        &whcode_web::security::ALLOWED_ORIGINS,
        &request(Method::Get, "/api/tasks?session=S-P"),
        &actor,
    );
    let web_tasks: serde_json::Value = serde_json::from_slice(&web_tasks.body).expect("Web JSON");
    assert_eq!(cui_tasks.total, web_tasks["total"], "任务总数一致");
    assert_eq!(cui_tasks.done, web_tasks["done"], "完成任务数一致");
    assert_eq!(cui_tasks.blocked, web_tasks["blocked"], "阻塞任务数一致");

    // 4) 写命令（FR-609/604）：同一会话创建，CUI 路径（app::new_session）与
    //    Web `POST /api/sessions/new` 返回同一形状的 `session` 对象。
    let cui_created = whcode_app::new_session(
        &conn,
        &policy,
        &actor,
        whcode_app::NewSession {
            title: "写命令一致性".to_string(),
            model: None,
        },
        2_000,
    )
    .expect("CUI 创建");
    let mut post = request(Method::Post, "/api/sessions/new");
    post.body = serde_json::to_vec(&serde_json::json!({"title": "写命令一致性"})).expect("体");
    let web_created = whcode_web::api::handle_request(
        &mut conn,
        false,
        None,
        &whcode_web::security::ALLOWED_ORIGINS,
        &post,
        &actor,
    );
    assert_eq!(web_created.status, 201, "写命令成功语义一致");
    let web_created: serde_json::Value =
        serde_json::from_slice(&web_created.body).expect("Web JSON");
    let cui_shape = whcode_app::session_value(&cui_created);
    let mut web_keys: Vec<&str> = web_created["session"]
        .as_object()
        .expect("对象")
        .keys()
        .map(String::as_str)
        .collect();
    let mut cui_keys: Vec<&str> = cui_shape
        .as_object()
        .expect("对象")
        .keys()
        .map(String::as_str)
        .collect();
    web_keys.sort_unstable();
    cui_keys.sort_unstable();
    assert_eq!(cui_keys, web_keys, "写命令返回的 session 字段集合必须一致");
    assert_eq!(
        web_created["session"]["title"], "写命令一致性",
        "同一标题写入结果一致"
    );
}

#[test]
fn error_codes_are_frontend_agnostic() {
    // V-109 部分：错误码一致——错误由 app 产生，前端只包装（output.rs 信封）。
    let reader = Actor::new("受限", []);
    let policy = DefaultPolicy;
    let err = whcode_app::require(&policy, &reader, Capability::SessionDelete).expect_err("拒绝");
    assert_eq!(err.code, ErrorCode::PermissionDenied, "TUI/CUI 同一错误码");
    assert!(!err.retryable);

    // 前端错误信封同源：同一错误 → 同一信封内容。
    let envelope = whcode_cui::output::error_envelope(&err);
    assert_eq!(envelope.code, "permission_denied");
    assert!(!envelope.retryable);
}
