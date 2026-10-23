//! 维护命令的应用层入口（FR-204/508/509）。
//!
//! 清理是**破坏性**命令：判定（判据/保护/审计）留在 `whcode-store`，权限与确认语义
//! 在本层统一（`permission_denied` 与 CUI 同源，FR-604）；前端只负责传输。
//!
//! 权限映射：清理按 TTL/时间点删除会话及其关联行，与按 ID 删除同属 `session.delete`
//! 能力——不新增大类，避免同一后果存在两套授权口径。

use duckdb::Connection;
use whcode_domain::WhError;

use crate::authz::{Actor, Capability, Policy, require};

pub use whcode_store::{PurgeCounts, PurgeCriteria, PurgePreview, PurgeProtection, PurgeResult};

/// 清理预览（dry-run）：不写库，返回冻结清单与各行数。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`WhError`]。
pub fn purge_preview(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    criteria: &PurgeCriteria,
    protection: &PurgeProtection,
    now_ms: u64,
) -> Result<PurgePreview, WhError> {
    require(policy, actor, Capability::SessionDelete)?;
    whcode_store::preview_purge(conn, criteria, protection, now_ms)
}

/// 执行清理：按预览冻结的清单删除，同一事务写审计；`confirm = false` 时由 store 拒绝。
///
/// # Errors
///
/// 权限拒绝、确认缺失或存储失败时返回 [`WhError`]。
pub fn purge_execute(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    preview: &PurgePreview,
    confirm: bool,
    protection: &PurgeProtection,
    now_ms: u64,
) -> Result<PurgeResult, WhError> {
    require(policy, actor, Capability::SessionDelete)?;
    whcode_store::execute_purge(conn, preview, confirm, &actor.id, protection, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};

    const MIGRATIONS: &str = include_str!("../../whcode-store/migrations/0001_initial.sql");

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(MIGRATIONS).expect("迁移");
        conn
    }

    /// 预览不写库；执行必须显式 `confirm`；缺能力一律 `permission_denied`。
    #[test]
    fn purge_requires_capability_and_explicit_confirm() {
        let mut conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-OLD', '旧', timestamp '2020-01-01 00:00:00',
                     timestamp '2020-01-01 00:00:00', 'active')",
            [],
        )
        .expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let protection = PurgeProtection::default();
        // 判据（2020-09）晚于会话活动时间（2020-01），当前时间（2023-11）远超活跃窗口。
        let cutoff = 1_600_000_000_000_i64;
        let now = 1_700_000_000_000_u64;
        let criteria = PurgeCriteria::before(cutoff);
        let preview =
            purge_preview(&conn, &policy, &actor, &criteria, &protection, now).expect("预览");
        assert_eq!(preview.selected_sessions, vec!["S-OLD".to_string()]);
        let remaining = |conn: &Connection| -> i64 {
            conn.query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
                .expect("计数")
        };
        assert_eq!(remaining(&conn), 1, "预览不得写库");

        // 未确认：store 拒绝，数据不动。
        let err = purge_execute(
            &mut conn,
            &policy,
            &actor,
            &preview,
            false,
            &protection,
            now,
        )
        .expect_err("未确认必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        assert_eq!(remaining(&conn), 1, "拒绝后不得删除");

        // 无能力：拒绝且不写库。
        let weak = Actor::default();
        let err = purge_preview(&conn, &policy, &weak, &criteria, &protection, now)
            .expect_err("缺能力必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::PermissionDenied);

        // 确认后执行。
        purge_execute(&mut conn, &policy, &actor, &preview, true, &protection, now).expect("执行");
        assert_eq!(remaining(&conn), 0);
    }
}
