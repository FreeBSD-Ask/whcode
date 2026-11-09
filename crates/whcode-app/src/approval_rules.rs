//! # 本地审批规则的应用门面（T256／T257、FR-745／FR-751）
//!
//! 设置页展示与撤销“记住规则”写下的本地规则；写入与撤销都要求
//! [`Capability::ApprovalRuleWrite`]，读取要求 [`Capability::ApprovalRuleRead`]。
//! 变更写 `PermissionChange` 审计（只带工具名与规则，不带工具参数）。
//!
//! 边界：SQL 在 `whcode-store`（ADR-018）；本模块只做权限、审计与错误语义。

use duckdb::Connection;

use whcode_domain::approval::ApprovalRule;
use whcode_domain::{AuditAction, AuditRecord, WhError};

use crate::authz::{Actor, Capability, Policy, require};

/// 设置页展示的一行规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRuleView {
    /// 工具稳定名。
    pub tool: String,
    /// 生效规则（`allow`／`deny`）。
    pub rule: ApprovalRule,
}

/// 列出本地规则（按工具名升序）。
///
/// # Errors
///
/// 缺 [`Capability::ApprovalRuleRead`] 时 `permission_denied`；存储读取失败原样返回。
pub fn list_rules(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
) -> Result<Vec<ApprovalRuleView>, WhError> {
    require(policy, actor, Capability::ApprovalRuleRead)?;
    let rows = whcode_store::approval_rules::list_rules(conn)?;
    Ok(rows
        .into_iter()
        .map(|row| ApprovalRuleView {
            tool: row.tool,
            rule: row.rule,
        })
        .collect())
}

/// 写入（或覆盖）一条规则并写审计。
///
/// # Errors
///
/// 缺能力、工具名非法、规则为 `ask`、超上限或 SQL 失败；审计失败时变更不生效
/// （先审计后写入，沿用“审计失败阻断”口径）。
pub fn remember_rule(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    tool: &str,
    rule: ApprovalRule,
    now_ms: u64,
) -> Result<(), WhError> {
    require(policy, actor, Capability::ApprovalRuleWrite)?;
    let tool = tool.trim();
    audit_change(conn, actor, tool, rule, "remember", now_ms)?;
    whcode_store::approval_rules::upsert_rule(conn, tool, rule, now_ms)
}

/// 撤销一条规则并写审计；返回是否确有删除。
///
/// # Errors
///
/// 缺能力、工具名非法、审计失败或 SQL 失败。
pub fn revoke_rule(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    tool: &str,
    now_ms: u64,
) -> Result<bool, WhError> {
    require(policy, actor, Capability::ApprovalRuleWrite)?;
    let tool = tool.trim();
    // 撤销没有“新规则”；detail 用 `rule: null` 表达删除。
    audit_change(conn, actor, tool, ApprovalRule::Allow, "revoke", now_ms)?;
    whcode_store::approval_rules::delete_rule(conn, tool)
}

fn audit_change(
    conn: &Connection,
    actor: &Actor,
    tool: &str,
    rule: ApprovalRule,
    change: &str,
    now_ms: u64,
) -> Result<(), WhError> {
    let record = AuditRecord {
        id: format!("A-approval-rule-{change}-{tool}-{now_ms}"),
        actor: actor.id.clone(),
        action: AuditAction::PermissionChange,
        target: format!("approval_rule:{tool}"),
        detail: serde_json::json!({
            "change": change,
            "tool": tool,
            "rule": if change == "revoke" { serde_json::Value::Null } else { serde_json::json!(rule.as_str()) },
        }),
        ts_ms: now_ms,
    };
    whcode_store::audit::insert_record(conn, &record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{Actor, Capability, DefaultPolicy};
    use whcode_store::apply_migrations;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn actor(caps: &[Capability]) -> Actor {
        Actor::new("settings-test", caps.iter().copied())
    }

    #[test]
    fn remember_list_and_revoke_require_capabilities_and_audit() {
        let conn = migrated();
        let policy = DefaultPolicy;
        let full = actor(&[Capability::ApprovalRuleRead, Capability::ApprovalRuleWrite]);
        remember_rule(&conn, &policy, &full, "shell", ApprovalRule::Deny, 10).expect("写入");
        remember_rule(&conn, &policy, &full, "edit_file", ApprovalRule::Allow, 11).expect("写入");
        let rows = list_rules(&conn, &policy, &full).expect("列表");
        assert_eq!(
            rows.iter().map(|row| row.tool.as_str()).collect::<Vec<_>>(),
            vec!["edit_file", "shell"]
        );

        // 审计：每次变更一条 PermissionChange，且不含工具参数（这里只有工具名）。
        let audit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audit WHERE action = 'permission_change'",
                [],
                |row| row.get(0),
            )
            .expect("审计计数");
        assert_eq!(audit, 2, "写入只有两条（落库前各一条）");

        assert!(revoke_rule(&conn, &policy, &full, "shell", 12).expect("撤销"));
        assert!(!revoke_rule(&conn, &policy, &full, "shell", 13).expect("重复撤销"));
        assert_eq!(list_rules(&conn, &policy, &full).expect("列表").len(), 1);

        // 缺能力：读与写都被拒绝（统一 permission_denied）。
        let reader_only = actor(&[Capability::ApprovalRuleRead]);
        assert_eq!(
            remember_rule(
                &conn,
                &policy,
                &reader_only,
                "shell",
                ApprovalRule::Allow,
                14
            )
            .expect_err("无写能力")
            .code,
            whcode_domain::ErrorCode::PermissionDenied
        );
        let no_caps = actor(&[]);
        assert_eq!(
            list_rules(&conn, &policy, &no_caps)
                .expect_err("无读能力")
                .code,
            whcode_domain::ErrorCode::PermissionDenied
        );
    }
}
