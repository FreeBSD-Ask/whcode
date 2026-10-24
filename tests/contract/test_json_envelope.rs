//! CUI JSON 信封契约测试（T089、FR-602、§8.1、contracts/cui-cli.md §3）：
//! 全部 CUI JSON 输出与错误对象契约——`schema_version` 必备 + 错误对象四字段
//! （`code`/`message`/`retryable`/`details`）+ 稳定退出码。
//!
//! 覆盖全部已交付 CUI 视图函数（context_cmd/events_cmd/spec_cmd）与
//! output.rs 的统一信封：每类输出经 `serde_json` 序列化后断言信封字段。

use duckdb::Connection;
use whcode_app::{DefaultPolicy, actor_with_all_capabilities};
use whcode_cui::confirm::Confirm;
use whcode_cui::context_cmd::budget_view;
use whcode_cui::events_cmd::{events_json, events_jsonl};
use whcode_cui::output::{error_envelope, exit_code, success_envelope};
use whcode_cui::spec_cmd::force_advance;
use whcode_domain::{AuditAction, AuditRecord, AuditSink, ErrorCode, SCHEMA_VERSION, WhError};
use whcode_speckit::SddStage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AuditAction2;

impl AuditSink for AuditAction2 {
    fn record(&self, _record: &AuditRecord) -> Result<(), WhError> {
        Ok(())
    }
}

/// CUI 夹具：迁移 + 会话 + 事件（store 依赖不可用——经 include_str 迁移 SQL，
/// 与 context_cmd/events_cmd 同口径；此处仅验证无 store 依赖的视图）。
fn migrated() -> Connection {
    let conn = Connection::open_in_memory().expect("内存库");
    conn.execute_batch(include_str!(
        "../../crates/whcode-store/migrations/0001_initial.sql"
    ))
    .and_then(|_| {
        conn.execute_batch(include_str!(
            "../../crates/whcode-store/migrations/0002_event_trace_indexes.sql"
        ))
    })
    .and_then(|_| {
        conn.execute_batch(include_str!(
            "../../crates/whcode-store/migrations/0003_query_indexes.sql"
        ))
    })
    .and_then(|_| {
        conn.execute_batch(include_str!(
            "../../crates/whcode-store/migrations/0004_session_id_sequence.sql"
        ))
    })
    .and_then(|_| {
        conn.execute_batch(include_str!(
            "../../crates/whcode-store/migrations/0005_session_stage.sql"
        ))
    })
    .and_then(|_| {
        conn.execute_batch(include_str!(
            "../../crates/whcode-store/migrations/0006_events_variant.sql"
        ))
    })
    .expect("迁移");
    conn
}

#[test]
fn all_cui_views_emit_schema_version_envelope() {
    let conn = migrated();
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, last_active_at, status)
         VALUES ('S-1', '信封', current_timestamp, current_timestamp, 'active')",
        [],
    )
    .expect("创建会话");
    for seq in 0..3 {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, 'S-1', NULL, NULL, NULL, 'StreamingChunk', CAST('{}' AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            duckdb::params![seq, format!("E-{seq}")],
        )
        .expect("插入事件");
    }
    let mut conn = conn;
    let policy = DefaultPolicy;
    let actor = actor_with_all_capabilities("tester");
    let sink = AuditAction2;

    // 预算视图：schema_version 必备。
    let budget = budget_view(&conn, &policy, &actor, "S-1", 1_000).expect("预算视图");
    assert_eq!(budget["schema_version"], SCHEMA_VERSION);

    // 事件 JSON/JSONL：schema_version 必备。
    let events = events_json(&conn, &policy, &actor, "S-1", 0, 100, TraceLevel::default())
        .expect("事件 JSON");
    assert_eq!(events["schema_version"], SCHEMA_VERSION);
    let jsonl = events_jsonl(&conn, &policy, &actor, "S-1", 0, 100, TraceLevel::default())
        .expect("事件 JSONL");
    let header: serde_json::Value =
        serde_json::from_str(jsonl.lines().next().expect("头部")).expect("头部 JSON");
    assert_eq!(header["schema_version"], SCHEMA_VERSION);

    // spec force-advance 视图：schema_version 必备。
    let workflow = whcode_speckit::default_workflow();
    let view = force_advance(
        &mut conn,
        &policy,
        &actor,
        &workflow,
        &whcode_app::ForceAdvance {
            session_id: "S-1",
            to: SddStage::Specify,
            reason: "契约测试",
            artifact_version: 1,
        },
        &sink,
        1_000,
    )
    .expect("强制放行");
    assert_eq!(view["schema_version"], SCHEMA_VERSION);
}

#[test]
fn error_envelope_contract_four_fields_and_exit_codes() {
    // 错误对象恒四字段（contracts/cui-cli.md §3）：无细节时 details 为 null 不省略。
    let simple = WhError::new(ErrorCode::NotFound, "不存在");
    let envelope = error_envelope(&simple);
    let value = serde_json::to_value(&envelope).expect("序列化");
    assert_eq!(value["schema_version"], SCHEMA_VERSION);
    for field in ["code", "message", "retryable", "details"] {
        assert!(
            value.get(field).is_some(),
            "错误对象必须含 {field}（FR-602 四字段）"
        );
    }
    assert_eq!(
        value["details"],
        serde_json::Value::Null,
        "无细节时 details 为 null"
    );

    // retryable 语义一致。
    let retryable = WhError::backpressure("队列满");
    assert!(error_envelope(&retryable).retryable);
    let permanent = WhError::permission_denied("无权限");
    assert!(!error_envelope(&permanent).retryable);

    // 稳定退出码（contracts/cui-cli.md）。
    assert_eq!(exit_code(&ErrorCode::PermissionDenied), 10);
    assert_eq!(exit_code(&ErrorCode::GateFailed), 20);
    assert_eq!(exit_code(&ErrorCode::Internal), 99);
}

#[test]
fn success_envelope_and_confirm_defaults_are_contractual() {
    // 成功信封：data + schema_version。
    let envelope = success_envelope(serde_json::json!({"n": 1}));
    let value = serde_json::to_value(&envelope).expect("序列化");
    assert_eq!(value["schema_version"], SCHEMA_VERSION);
    assert_eq!(value["data"]["n"], 1);

    // 破坏性命令默认 dry-run（§8.1：无隐式执行）。
    assert!(!Confirm::from_flags(false, false).allows_execution());
}

use whcode_domain::TraceLevel;

#[test]
fn audit_action_alias_compiles() {
    // AuditSink 实现占位类型的健全性检查。
    let sink = AuditAction2;
    let record = AuditRecord {
        id: "A-1".to_string(),
        actor: "tester".to_string(),
        action: AuditAction::GateOverride,
        target: "session:S-1".to_string(),
        detail: serde_json::json!({"reason": "契约测试"}),
        ts_ms: 1,
    };
    sink.record(&record).expect("审计写入");
}
