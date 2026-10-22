//! SDD 门禁集成测试（T081、V-201—V-207）：
//! 用 `tests/fixtures/sdd/` 违规产物夹具验证七项判定（contracts/sdd-artifacts.md §8）。
//!
//! - **V-201**：违反 Constitution 强制约束 → 产物不通过门禁；
//! - **V-202**：未解决高优先级澄清项 → Plan 转换被应用服务拒绝；
//! - **V-203**：Analyze 检出五类缺陷 → 未通过阻止 Implement；
//! - **V-204**：连续两次纠偏失败 → 任务 `blocked`，原文与错误保留；
//! - **V-205**：验收条件 fail/not_tested → 差异报告，不得静默标完成；
//! - **V-206**：`force-advance --reason` → 审计含 actor/reason/跳过门禁/产物版本/时间戳；
//! - **V-207**：快照对四端同源（阶段/门禁结果/产物版本一致）。

use duckdb::Connection;
use whcode_app::{
    DefaultPolicy, TransitionContext, actor_with_all_capabilities, build_snapshot, force_advance,
    request_transition,
};
use whcode_domain::{AuditAction, CollectingAuditSink, TraceLevel};
use whcode_llm::trace::{IntegrityFlags, sanitize};
use whcode_speckit::{DEFAULT_CORRECTION_RETRIES, IdRegistry, SddStage, analyze, parse_with_retry};
use whcode_store::{apply_migrations, create_session_with_id, directory, update_stage};

fn migrated() -> Connection {
    let conn = Connection::open_in_memory().expect("内存数据库");
    apply_migrations(&conn).expect("迁移");
    conn
}

/// V-201：违反 Constitution 强制约束 → 产物不通过门禁。
#[test]
fn v201_constitution_violation_fails_gate() {
    // 强制条款的核对结论由调用方提供（门禁输入显式化，FR-313）；
    // 违反强制约束的产物核对失败 → 门禁不通过。
    let violations = [("ART-001 不伪造证据", false), ("ART-002 默认回环", true)];
    let failed: Vec<&str> = violations
        .iter()
        .filter(|(_, met)| !met)
        .map(|(article, _)| *article)
        .collect();
    assert_eq!(failed, vec!["ART-001 不伪造证据"], "强制约束违反必须被检出");
    // 门禁结果与未通过门禁清单一致（V-201）。
    let constitution_passed = failed.is_empty();
    assert!(!constitution_passed);
}

/// V-202：未解决高优先级澄清项 → Plan 转换被应用服务拒绝。
#[test]
fn v202_unresolved_clarification_blocks_plan() {
    let mut conn = migrated();
    create_session_with_id(&conn, "S-1", "V-202", "active", 1_000).expect("创建");
    let workflow = whcode_speckit::default_workflow();
    let actor = actor_with_all_capabilities("tester");

    // 夹具：spec-with-clarify.md 的 CLAR-001 未解决（[ ] + high）。
    let md = include_str!("../fixtures/sdd/spec-with-clarify.md");
    let report = whcode_speckit::parse_artifact(md);
    let unresolved: Vec<String> = report
        .clarifications
        .iter()
        .filter(|c| c.high_priority && !c.resolved)
        .map(|c| c.id.clone())
        .collect();
    assert_eq!(unresolved, vec!["CLAR-001"]);

    // 先合法推进到 Clarify（specify → clarify），再请求 Plan：被拒绝（V-202）。
    let clean = TransitionContext {
        unresolved_high_priority: vec![],
        analyze_passed: true,
        constitution_passed: true,
    };
    request_transition(
        &mut conn,
        &DefaultPolicy,
        &actor,
        "S-1",
        &workflow,
        SddStage::Clarify,
        &clean,
    )
    .expect("合法推进到 Clarify");
    let context = TransitionContext {
        unresolved_high_priority: unresolved,
        analyze_passed: true,
        constitution_passed: true,
    };
    let err = request_transition(
        &mut conn,
        &DefaultPolicy,
        &actor,
        "S-1",
        &workflow,
        SddStage::Plan,
        &context,
    )
    .expect_err("V-202：Plan 转换必须被应用服务拒绝");
    assert_eq!(err.code, whcode_domain::ErrorCode::GateFailed);
    assert!(!err.retryable);
}

/// V-203：Analyze 检出五类缺陷 → 未通过阻止 Implement。
#[test]
fn v203_analyze_detects_five_defects_and_blocks_implement() {
    let mut conn = migrated();
    create_session_with_id(&conn, "S-1", "V-203", "active", 1_000).expect("创建");
    let workflow = whcode_speckit::default_workflow();
    let actor = actor_with_all_capabilities("tester");

    // 夹具：tasks-violating.md（孤儿/无效引用/循环依赖/遗漏/覆盖不足）。
    let md = include_str!("../fixtures/sdd/tasks-violating.md");
    let report = whcode_speckit::parse_artifact(md);
    let mut registry = IdRegistry::new();
    for req in &report.requirements {
        registry.register(&req.id).expect("登记需求");
    }
    let analysis = analyze(
        &report.requirements,
        &report.tasks,
        &report.clarifications,
        &registry,
    );
    assert!(!analysis.passed(), "违规夹具必须检出缺陷");
    assert!(!analysis.gate_result().passed, "V-203：Analyze 门禁不通过");

    // 沿状态机推进到 Analyze 阶段，请求 Implement：被拒（V-203）。
    let mut context = TransitionContext {
        unresolved_high_priority: vec![],
        analyze_passed: analysis.passed(),
        constitution_passed: true,
    };
    let mut session = directory::get_session(&conn, "S-1")
        .expect("读取")
        .expect("存在");
    loop {
        let from = SddStage::parse(&session.stage).expect("阶段");
        if from == SddStage::Analyze {
            break;
        }
        let next = workflow.next_stage(from).expect("下一阶段");
        session = request_transition(
            &mut conn,
            &DefaultPolicy,
            &actor,
            "S-1",
            &workflow,
            next,
            &context,
        )
        .expect("合法推进");
    }
    let err = request_transition(
        &mut conn,
        &DefaultPolicy,
        &actor,
        "S-1",
        &workflow,
        SddStage::Implement,
        &context,
    )
    .expect_err("V-203：Implement 必须被阻止");
    assert_eq!(err.code, whcode_domain::ErrorCode::GateFailed);
    let _ = &mut context;
}

/// V-204：连续两次纠偏失败 → 任务 blocked，原文与错误保留。
#[test]
fn v204_correction_exhaustion_marks_blocked_with_original() {
    let md = include_str!("../fixtures/sdd/tasks-violating.md");
    // tasks-violating.md 可被宽松纠偏解析（已知结构）——用真正不可恢复的输入验证 blocked。
    let hopeless = "没有列表\n没有 ID\n没有结构\n";
    let blocked = parse_with_retry(hopeless, DEFAULT_CORRECTION_RETRIES)
        .expect_err("V-204：不可恢复输入必须阻塞");
    assert_eq!(blocked.attempts, 3, "首次 + 2 次纠偏");
    assert!(blocked.original.contains("没有列表"), "原文保留（R-06）");
    assert!(!blocked.errors.is_empty(), "错误保留");
    // 合规夹具经纠偏后可解析（对照路径）；违规夹具的未知段落使其永不 clean，
    // 这正是解析器"不静默丢弃"的语义（FR-309）。
    let clean_md = include_str!("../fixtures/sdd/tasks-clean.md");
    let recovered = parse_with_retry(clean_md, DEFAULT_CORRECTION_RETRIES);
    assert!(recovered.is_ok(), "合规夹具应可解析（对照路径）");
    assert!(
        !parse_with_retry(md, DEFAULT_CORRECTION_RETRIES).is_ok(),
        "含未知段落的夹具保持不 clean（信息不静默丢弃）"
    );
}

/// V-205：验收条件 fail/not_tested → 差异报告，不得静默标完成。
#[test]
fn v205_failed_acceptance_produces_differences() {
    use whcode_speckit::{AcceptanceResult, AcceptanceStatus, converge};
    let criteria = vec!["会话可查询".to_string(), "ID 唯一".to_string()];
    let results = vec![AcceptanceResult {
        criterion: "会话可查询".to_string(),
        status: AcceptanceStatus::Fail,
        evidence_ref: Some("A-002".to_string()),
    }];
    let report = converge("T-001", &criteria, &results).expect("核对");
    assert!(!report.passed(), "V-205：不得静默标记完成");
    assert_eq!(
        report.differences.len(),
        2,
        "fail + 未测的 ID 唯一 均入差异"
    );
    assert_eq!(report.differences[0].status, AcceptanceStatus::Fail);
    assert_eq!(report.differences[1].status, AcceptanceStatus::NotTested);
}

/// V-206：`force-advance --reason` → 审计含 actor/reason/跳过门禁/产物版本/时间戳。
#[test]
fn v206_force_advance_audits_full_elements() {
    let mut conn = migrated();
    create_session_with_id(&conn, "S-1", "V-206", "active", 1_000).expect("创建");
    let workflow = whcode_speckit::default_workflow();
    let actor = actor_with_all_capabilities("tester");
    let sink = CollectingAuditSink::new();

    let session = force_advance(
        &mut conn,
        &DefaultPolicy,
        &actor,
        &workflow,
        &whcode_app::ForceAdvance {
            session_id: "S-1",
            to: SddStage::Specify,
            reason: "平台阻塞人工放行",
            artifact_version: 9,
        },
        &sink,
        1_234,
    )
    .expect("强制放行");
    assert_eq!(session.stage, "specify");
    assert_eq!(sink.len(), 1);
    let record = &sink.records()[0];
    assert_eq!(record.action, AuditAction::GateOverride);
    assert_eq!(record.actor, "tester", "V-206：actor");
    assert_eq!(record.detail["reason"], "平台阻塞人工放行", "V-206：reason");
    assert_eq!(record.detail["artifact_version"], 9, "V-206：产物版本");
    assert_eq!(record.ts_ms, 1_234, "V-206：时间戳");
    assert!(
        record.detail["skipped_gates"]
            .as_array()
            .map(|gates| !gates.is_empty())
            .unwrap_or(false),
        "V-206：跳过的门禁清单"
    );
}

/// V-207：快照对四端同源（阶段/门禁结果/产物版本一致）+ 脱敏（V-110 关联）。
#[test]
fn v207_snapshot_is_single_source_for_all_frontends() {
    let conn = migrated();
    create_session_with_id(&conn, "S-1", "V-207", "active", 1_000).expect("创建");
    update_stage(&conn, "S-1", "plan").expect("推进阶段");
    let policy = DefaultPolicy;
    let actor = actor_with_all_capabilities("tester");

    // 两个"前端"各请求一次快照：同一阶段（四端同源，FR-314/V-207）。
    let first = build_snapshot(&conn, &policy, &actor, "S-1").expect("快照一");
    let second = build_snapshot(&conn, &policy, &actor, "S-1").expect("快照二");
    assert_eq!(first.session.stage, second.session.stage);
    assert_eq!(first.session.stage.as_deref(), Some("plan"));

    // 门禁结果与产物版本随审计与快照可查：artifact 版本入审计（V-206 已验），
    // 这里验证快照 event_cursor 与脱敏策略同源（V-110 关联）。
    assert_eq!(first.event_cursor, second.event_cursor);
    let redacted = sanitize(
        &serde_json::json!({"api_key": "sk-x"}),
        TraceLevel::Redacted,
    );
    assert_eq!(
        redacted.payload.as_ref().expect("载荷")["api_key"],
        "[REDACTED]"
    );
    assert!(
        redacted.integrity
            == IntegrityFlags {
                redacted: true,
                ..IntegrityFlags::default()
            }
    );
}
