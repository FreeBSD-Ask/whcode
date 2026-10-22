//! # 上下文预算视图（T062、FR-108）
//!
//! 暴露预算视图：当前预算、装入来源、截断原因。本模块是 CUI 对应用服务的**展示
//! 包装**（前端禁止触库，ADR-003/005）；clap 命令树属 M5（T084），此处交付 JSON
//! 视图函数（`schema_version` 信封，T085 接线）。

use duckdb::Connection;
use serde_json::json;
use whcode_app::{Actor, Policy};
use whcode_domain::SCHEMA_VERSION;

/// 预算视图（FR-108：当前预算、装入来源、截断原因）。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`whcode_domain::WhError`]。
pub fn budget_view(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    budget_tokens: usize,
) -> Result<serde_json::Value, whcode_domain::WhError> {
    let window = whcode_app::load_context(conn, policy, actor, session_id, budget_tokens)?;
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "session_id": session_id,
        "budget_tokens": budget_tokens,
        "consumed_budget": window.consumed_budget,
        "items": window.items.len(),
        "sources": window.sources,
        "truncated_reason": window.truncated_reason,
        "next_cursor": window.next_cursor.as_ref().map(|cursor| {
            json!({ "ts_us": cursor.ts_us, "id": cursor.id })
        }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TEST_MIGRATIONS;
    use duckdb::{Connection, params};
    use whcode_app::{DefaultPolicy, actor_with_all_capabilities};

    #[test]
    fn budget_view_reports_budget_sources_and_truncation_reason() {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(TEST_MIGRATIONS).expect("迁移");
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', '预算视图', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("创建");
        for index in 0..10 {
            conn.execute(
                "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
                 VALUES (?, ?, 'user', ?, 100, current_timestamp, NULL)",
                params![format!("M-{index}"), "S-1", format!("ref-{index}")],
            )
            .expect("插入消息");
        }
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let view = budget_view(&conn, &policy, &actor, "S-1", 350).expect("预算视图");
        assert_eq!(view["schema_version"], SCHEMA_VERSION);
        assert_eq!(view["budget_tokens"], 350);
        assert_eq!(view["consumed_budget"], 300);
        assert_eq!(view["items"], 3);
        assert_eq!(view["sources"], serde_json::json!(["messages"]));
        assert_eq!(
            view["truncated_reason"], "budget_exhausted",
            "截断原因必须可查（FR-108）"
        );
        assert_eq!(view["next_cursor"]["id"], "M-2");
    }
}
