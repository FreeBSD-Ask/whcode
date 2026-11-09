//! # 工具级审批规则的本地存储（T256／T257、FR-745）
//!
//! 只存 `allow`／`deny`：`ask` 是“每次都问”的缺省行为，不需要持久化；一条工具一行，
//! 规则变更 UPSERT，撤销即删除。上限 [`MAX_APPROVAL_RULES`]（与 app 侧规则表上限一致），
//! 超限拒绝写入而不是静默淘汰（用户显式设置的规则不得被自动删除）。
//!
//! 边界：本模块只做 SQL（ADR-018）；权限判定与审计在 `whcode-app`。

use duckdb::{Connection, params};

use whcode_domain::approval::{ApprovalRule, MAX_APPROVAL_ID_LEN};
use whcode_domain::{ErrorCode, WhError};

/// 本地审批规则条数上限（与 `whcode_app::approval::MAX_APPROVAL_TOOL_RULES` 同值）。
pub const MAX_APPROVAL_RULES: usize = 256;

/// 规则表的一行（工具名＋规则＋更新时间）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRuleRow {
    /// 工具稳定名。
    pub tool: String,
    /// 规则（`allow`／`deny`；表级 CHECK 保证）。
    pub rule: ApprovalRule,
    /// 最后一次更新时间（毫秒）。
    pub updated_at_ms: u64,
}

fn store_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("审批规则{context}失败：{err}"))
}

fn validate_tool(tool: &str) -> Result<&str, WhError> {
    let trimmed = tool.trim();
    if trimmed.is_empty() {
        return Err(WhError::invalid_state("审批规则的工具名不得为空"));
    }
    if trimmed.chars().count() > MAX_APPROVAL_ID_LEN {
        return Err(WhError::invalid_state(format!(
            "审批规则的工具名超过长度上限 {MAX_APPROVAL_ID_LEN}"
        )));
    }
    Ok(trimmed)
}

/// 读取全部已存规则（按工具名升序，结果确定）。
///
/// # Errors
///
/// 查询失败返回 `io_error`。
pub fn list_rules(conn: &Connection) -> Result<Vec<ApprovalRuleRow>, WhError> {
    let mut statement = conn
        .prepare("SELECT tool, rule, updated_at_ms FROM approval_rules ORDER BY tool")
        .map_err(|err| store_err("准备查询", err))?;
    let rows = statement
        .query_map([], |row| {
            let tool: String = row.get(0)?;
            let raw: String = row.get(1)?;
            let updated_at_ms: i64 = row.get(2)?;
            Ok((tool, raw, updated_at_ms))
        })
        .map_err(|err| store_err("查询", err))?;
    let mut out = Vec::new();
    for row in rows {
        let (tool, raw, updated_at_ms) = row.map_err(|err| store_err("读取", err))?;
        let rule = ApprovalRule::parse(&raw)
            .ok_or_else(|| WhError::invalid_state(format!("审批规则 {tool} 的值非法：{raw}")))?;
        if rule == ApprovalRule::Ask {
            return Err(WhError::invalid_state(format!(
                "审批规则 {tool} 不得持久化为 ask（ask 是缺省询问行为）"
            )));
        }
        out.push(ApprovalRuleRow {
            tool,
            rule,
            updated_at_ms: u64::try_from(updated_at_ms).unwrap_or(0),
        });
    }
    Ok(out)
}

/// 写入（或覆盖）一条工具级规则。
///
/// # Errors
///
/// 工具名为空／超长、规则为 `ask`、条数超 [`MAX_APPROVAL_RULES`]，或 SQL 失败。
pub fn upsert_rule(
    conn: &Connection,
    tool: &str,
    rule: ApprovalRule,
    now_ms: u64,
) -> Result<(), WhError> {
    let tool = validate_tool(tool)?;
    if rule == ApprovalRule::Ask {
        return Err(WhError::invalid_state(
            "审批规则不得持久化为 ask（ask 是缺省询问行为）",
        ));
    }
    let existing: Option<String> = conn
        .query_row(
            "SELECT rule FROM approval_rules WHERE tool = ?",
            [tool],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|err| match err {
            duckdb::Error::QueryReturnedNoRows => Ok(None),
            other => Err(store_err("检查既有规则", other)),
        })?;
    if existing.is_none() {
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM approval_rules", [], |row| row.get(0))
            .map_err(|err| store_err("统计条数", err))?;
        if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_APPROVAL_RULES {
            return Err(WhError::invalid_state(format!(
                "本地审批规则超过上限 {MAX_APPROVAL_RULES}；请先在设置页撤销不再需要的规则"
            )));
        }
    }
    let now = i64::try_from(now_ms).unwrap_or(i64::MAX);
    if existing.is_some() {
        conn.execute(
            "UPDATE approval_rules SET rule = ?, updated_at_ms = ? WHERE tool = ?",
            params![rule.as_str(), now, tool],
        )
        .map_err(|err| store_err("更新", err))?;
    } else {
        conn.execute(
            "INSERT INTO approval_rules (tool, rule, created_at_ms, updated_at_ms)
             VALUES (?, ?, ?, ?)",
            params![tool, rule.as_str(), now, now],
        )
        .map_err(|err| store_err("写入", err))?;
    }
    Ok(())
}

/// 撤销一条规则；返回是否确有删除。
///
/// # Errors
///
/// 工具名非法或 SQL 失败。
pub fn delete_rule(conn: &Connection, tool: &str) -> Result<bool, WhError> {
    let tool = validate_tool(tool)?;
    let removed = conn
        .execute("DELETE FROM approval_rules WHERE tool = ?", [tool])
        .map_err(|err| store_err("删除", err))?;
    Ok(removed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    #[test]
    fn upsert_lists_and_deletes_in_stable_order() {
        let conn = migrated();
        assert!(list_rules(&conn).expect("空表").is_empty());
        upsert_rule(&conn, "shell", ApprovalRule::Deny, 10).expect("写入");
        upsert_rule(&conn, "edit_file", ApprovalRule::Allow, 11).expect("写入");
        let rows = list_rules(&conn).expect("列表");
        assert_eq!(
            rows.iter().map(|row| row.tool.as_str()).collect::<Vec<_>>(),
            vec!["edit_file", "shell"],
            "按工具名升序"
        );
        assert_eq!(rows[1].rule, ApprovalRule::Deny);

        // 覆盖：同一工具不新增行，规则与时间更新。
        upsert_rule(&conn, "shell", ApprovalRule::Allow, 20).expect("覆盖");
        let rows = list_rules(&conn).expect("列表");
        assert_eq!(rows.len(), 2);
        let shell = rows
            .iter()
            .find(|row| row.tool == "shell")
            .expect("shell 行");
        assert_eq!(shell.rule, ApprovalRule::Allow);
        assert_eq!(shell.updated_at_ms, 20);

        assert!(delete_rule(&conn, "shell").expect("删除"));
        assert!(!delete_rule(&conn, "shell").expect("重复删除"));
        assert_eq!(list_rules(&conn).expect("列表").len(), 1);
    }

    #[test]
    fn invalid_rules_are_rejected() {
        let conn = migrated();
        assert!(upsert_rule(&conn, "", ApprovalRule::Allow, 1).is_err());
        assert!(
            upsert_rule(
                &conn,
                &"x".repeat(MAX_APPROVAL_ID_LEN + 1),
                ApprovalRule::Allow,
                1
            )
            .is_err()
        );
        assert!(
            upsert_rule(&conn, "shell", ApprovalRule::Ask, 1).is_err(),
            "ask 不得持久化"
        );
        assert!(delete_rule(&conn, "  ").is_err());
    }

    #[test]
    fn rule_count_is_bounded() {
        let conn = migrated();
        for index in 0..MAX_APPROVAL_RULES {
            upsert_rule(&conn, &format!("tool-{index}"), ApprovalRule::Allow, 1)
                .expect("写入上限内");
        }
        let err =
            upsert_rule(&conn, "tool-over", ApprovalRule::Allow, 1).expect_err("超限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        // 覆盖既有行不受上限影响。
        upsert_rule(&conn, "tool-0", ApprovalRule::Deny, 2).expect("覆盖既有");
        assert_eq!(list_rules(&conn).expect("列表").len(), MAX_APPROVAL_RULES);
    }
}
