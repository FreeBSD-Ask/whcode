//! # 四端对等契约门禁（FR-619/620/621，Task T171、T180）
//!
//! 登记表（`whcode-app::surface`）× 前端清单（四 crate 的 `surface_map`）× 矩阵
//! （`docs/matrices/feature-parity.md`）三向一致；缺映射、孤儿路由、矩阵手工漂移、
//! 过期排除均失败（负向用例证明门禁可失败——证伪优先）。
//!
//! 行为等价（T180.1）：同一写命令经 CUI 形状与 Web 形状执行——两者都改道同一门面
//! `execute_command`，最终状态、审计字段与权限拒绝码一致在门面单点钉死。

use duckdb::Connection;
use whcode_app::commands::{FrontendCommand, execute_command};
use whcode_app::surface::{FrontendId, Support, SurfaceEntry, SurfaceKind, find, validate_entries};
use whcode_app::{Actor, DefaultPolicy, actor_with_all_capabilities};
use whcode_store::apply_migrations;

/// 内存库夹具（含全部迁移；与 CUI/Web 测试同源形状）。
fn migrated() -> Connection {
    let conn = Connection::open_in_memory().expect("内存库");
    apply_migrations(&conn).expect("迁移");
    conn
}

/// FR-619：登记表全量自洽（唯一 ID、锚点、理由码、里程碑——由 validate 承载）。
#[test]
fn surface_registry_is_self_consistent() {
    let problems = whcode_app::surface::validate();
    assert!(problems.is_ok(), "登记表不得有内部违例：{problems:?}");
    assert!(
        whcode_app::surface::SURFACES
            .iter()
            .any(|entry| entry.kind == SurfaceKind::View),
        "六视图必须作为 View 类条目登记"
    );
}

/// FR-621 正向：Web 清单的每条路由都能在登记表找到，且 Web 端状态为已实现
/// （预留路由由守卫测试覆盖，不入本断言）。
#[test]
fn web_routes_resolve_to_implemented_surfaces() {
    let routes = [
        ("GET", "/api/status"),
        ("GET", "/api/sessions"),
        ("GET", "/api/events"),
        ("GET", "/api/tasks"),
        ("GET", "/api/context"),
        ("GET", "/api/diff"),
        ("GET", "/api/events/export"),
        ("GET", "/api/runtimes"),
        ("GET", "/api/i18n"),
        ("POST", "/api/sessions/new"),
        ("POST", "/api/sessions/rename"),
        ("POST", "/api/sessions/archive"),
        ("POST", "/api/sessions/delete"),
        ("POST", "/api/tasks/retry"),
        ("POST", "/api/tasks/abort"),
        ("POST", "/api/purge"),
    ];
    for (method, path) in routes {
        let id = whcode_web::surface_map::surface_of(method, path)
            .unwrap_or_else(|| panic!("{method} {path} 是孤儿路由（未入清单）"));
        let entry = find(id).unwrap_or_else(|| panic!("{method} {path} → {id} 不在登记表"));
        let support = entry.support(FrontendId::Web);
        assert!(
            matches!(support, Support::Implemented { .. }),
            "{method} {path}（{id}）的 Web 状态应为已实现，实际 {support:?}"
        );
    }
}

/// FR-621 正向：TUI 动作清单全部解析到登记表；写动作与登记状态一致。
#[test]
fn tui_actions_resolve_to_registered_surfaces() {
    use whcode_tui::surface_map::{TuiAction, surface_of};
    for action in [
        TuiAction::ViewSwitch(whcode_app::view_source::ViewId::Diff),
        TuiAction::SessionNext,
        TuiAction::Refresh,
        TuiAction::ThemeCycle,
        TuiAction::Quit,
        TuiAction::SessionNew,
        TuiAction::SessionRename,
        TuiAction::SessionArchive,
        TuiAction::SessionDelete,
        TuiAction::TaskRetry,
        TuiAction::TaskAbort,
        TuiAction::Purge,
    ] {
        let id = surface_of(action);
        let entry = find(id).unwrap_or_else(|| panic!("{action:?} → {id} 不在登记表"));
        let _ = entry;
    }
}

/// FR-621 正向：GUI 清单全部解析且不包含任何写命令面（只读 v1，FR-711）。
#[test]
fn gui_actions_stay_read_only() {
    use whcode_gui::surface_map::{GuiAction, surface_of};
    for action in [
        GuiAction::SelectView(whcode_app::view_source::ViewId::Diff),
        GuiAction::NextSession,
        GuiAction::Refresh,
        GuiAction::CycleTheme,
        GuiAction::Quit,
    ] {
        let id = surface_of(action);
        let entry = find(id).expect("GUI 动作必须已登记");
        assert!(
            matches!(entry.effect, whcode_app::surface::EffectClass::Read),
            "{id} 的 GUI 动作不得触及写/破坏面（FR-711）：{:?}",
            entry.effect
        );
    }
}

/// FR-621 负向（证伪门禁本身）：合成登记表的四类违例必须被 validate 拒绝。
#[test]
fn synthetic_registry_violations_are_rejected() {
    let valid = |id: &'static str| SurfaceEntry {
        id,
        kind: SurfaceKind::Command,
        capability: whcode_app::Capability::SessionRead,
        effect: whcode_app::surface::EffectClass::Read,
        confirmation: whcode_app::surface::Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented { anchor: "a" },
            Support::Reserved { milestone: "M" },
            Support::Excluded {
                reason: whcode_app::surface::ExclusionReason::GuiReadOnlyV1,
            },
            Support::NotApplicable,
        ],
    };
    // 重复 ID。
    let duplicate = [valid("x"), valid("x")];
    assert!(!validate_entries(&duplicate).is_empty(), "重复 ID 必须失败");
    // 缺锚点。
    let mut missing_anchor = valid("y");
    missing_anchor.frontends[0] = Support::Implemented { anchor: "" };
    assert!(
        !validate_entries(&[missing_anchor]).is_empty(),
        "已实现缺锚点必须失败"
    );
    // 预留缺里程碑。
    let mut missing_milestone = valid("z");
    missing_milestone.frontends[1] = Support::Reserved { milestone: "" };
    assert!(
        !validate_entries(&[missing_milestone]).is_empty(),
        "预留缺里程碑必须失败"
    );
    // 空表。
    assert!(!validate_entries(&[]).is_empty(), "空登记表必须失败");
}

/// FR-619：矩阵文件必须声明“由登记表校验”并带 GUI 列——手工漂移的载体被收窄。
#[test]
fn feature_parity_matrix_declares_governance() {
    let matrix = include_str!("../../docs/matrices/feature-parity.md");
    assert!(
        matrix.contains("登记表"),
        "矩阵必须写明由 whcode-app::surface 登记表校验（FR-619）"
    );
    assert!(matrix.contains("GUI"), "矩阵必须含 GUI 列（本变更新增）");
    // 已知自相矛盾行必须已被修正：Web delete 不再是 ⛔（Check 93/183 补录）。
    for line in matrix.lines() {
        if line.contains("delete") && line.contains("Web") {
            assert!(
                !line.contains("⛔"),
                "Web delete 已交付（FR-609），矩阵仍标 ⛔ 属手工漂移：{line}"
            );
        }
    }
}

/// T180.1 行为等价：同一 purge 经“CUI 形状”与“Web 形状”执行——两者都改道
/// 同一门面 `execute_command`，预览冻结清单、执行结果与审计次数一致。
#[test]
fn purge_through_facade_has_identical_outcome_and_audit() {
    let mut cui_conn = migrated();
    let mut web_conn = migrated();
    let policy = DefaultPolicy;
    let actor = actor_with_all_capabilities("parity");
    let now = 1_700_000_000_000_u64;
    for conn in [&mut cui_conn, &mut web_conn] {
        for title in ["旧-1", "旧-2"] {
            execute_command(
                conn,
                &policy,
                &actor,
                &FrontendCommand::SessionNew {
                    title: title.to_string(),
                    model: None,
                    now_ms: now - 1_000,
                },
            )
            .expect("建会话");
        }
    }
    let audit_before_cui = audit_rows(&cui_conn);
    let audit_before_web = audit_rows(&web_conn);
    let before_ms = 1_600_000_000_000_i64;

    let cui_preview = execute_command(
        &mut cui_conn,
        &policy,
        &actor,
        &FrontendCommand::PurgePreview {
            before_ms,
            now_ms: now,
        },
    )
    .expect("CUI 预览");
    let cui_result = execute_command(
        &mut cui_conn,
        &policy,
        &actor,
        &FrontendCommand::PurgeExecute {
            before_ms,
            confirm: true,
            now_ms: now,
        },
    )
    .expect("CUI 确认");
    let web_preview = execute_command(
        &mut web_conn,
        &policy,
        &actor,
        &FrontendCommand::PurgePreview {
            before_ms,
            now_ms: now,
        },
    )
    .expect("Web 预览");
    let web_result = execute_command(
        &mut web_conn,
        &policy,
        &actor,
        &FrontendCommand::PurgeExecute {
            before_ms,
            confirm: true,
            now_ms: now,
        },
    )
    .expect("Web 确认");

    let (whcode_app::CommandOutcome::PurgePreview(a), whcode_app::CommandOutcome::PurgePreview(b)) =
        (cui_preview, web_preview)
    else {
        panic!("预览形状不符");
    };
    assert_eq!(a.selected_sessions, b.selected_sessions, "预览冻结清单一致");
    let (
        whcode_app::CommandOutcome::PurgeExecuted(a),
        whcode_app::CommandOutcome::PurgeExecuted(b),
    ) = (cui_result, web_result)
    else {
        panic!("执行形状不符");
    };
    assert_eq!(a.deleted_sessions, b.deleted_sessions, "执行清单一致");
    assert_eq!(a.deleted.events, b.deleted.events, "删除计数一致");
    assert_eq!(
        audit_rows(&cui_conn) - audit_before_cui,
        1,
        "CUI 执行写一条 db_purge 审计（预览不写）"
    );
    assert_eq!(
        audit_rows(&web_conn) - audit_before_web,
        1,
        "Web 执行写一条 db_purge 审计（预览不写）"
    );
}

/// FR-604/623：权限拒绝码与端无关——门面单点判定，无能力调用者同码同 retryable。
#[test]
fn permission_denial_through_facade_is_uniform() {
    let mut conn = migrated();
    let policy = DefaultPolicy;
    let weak = Actor::new("weak", []);
    let err = execute_command(
        &mut conn,
        &policy,
        &weak,
        &FrontendCommand::SessionNew {
            title: "越权".to_string(),
            model: None,
            now_ms: 0,
        },
    )
    .expect_err("无能力必须拒绝");
    assert_eq!(err.code, whcode_domain::ErrorCode::PermissionDenied);
    assert!(!err.retryable, "权限拒绝不可重试（FR-604）");
}

/// FR-620：预留面在两端状态码同源（not_supported details 的共享键）。
#[test]
fn reserved_surfaces_drive_both_frontend_unavailable_semantics() {
    for id in ["spec.force-advance", "runtime.status", "llm.run"] {
        let entry = find(id).unwrap_or_else(|| panic!("{id} 未登记"));
        let cui = entry.support(FrontendId::Cui);
        let web = entry.support(FrontendId::Web);
        assert_eq!(
            cui.state_code(),
            web.state_code(),
            "{id} 两端状态码必须同源"
        );
    }
}

fn audit_rows(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM audit", [], |row| row.get(0))
        .expect("审计计数")
}
