//! # 索引注册表与完整性检查（T036、§7.2）
//!
//! 索引 DDL 只在 `migrations/` 中执行；本模块是可执行清单：列出每个必需索引、所属表、
//! 列与用途，并提供缺失检查，避免出现“文档声称有索引、库内实际没有”的状态。
//!
//! 不负责创建索引（schema 迁移是唯一入口），也不规定查询必须使用某个索引：
//! 查询计划由真实基准判定，小表或完整聚合的顺序扫描可能是正确计划（§7.2、TBD-03）。

use std::collections::HashSet;

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

/// 必需索引的静态描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredIndex {
    /// 索引名（与迁移脚本一致）。
    pub name: &'static str,
    /// 所属表。
    pub table: &'static str,
    /// 索引列（按迁移中的顺序）。
    pub columns: &'static str,
    /// 用途与需求依据。
    pub purpose: &'static str,
}

/// 全部必需索引；与 `0001_initial.sql`—`0003_query_indexes.sql` 的 DDL 一一对应。
pub const REQUIRED_INDEXES: &[RequiredIndex] = &[
    RequiredIndex {
        name: "idx_events_session_seq",
        table: "events",
        columns: "session_id, seq",
        purpose: "会话历史游标分页（FR-504）",
    },
    RequiredIndex {
        name: "idx_events_task",
        table: "events",
        columns: "task_id",
        purpose: "按任务回溯事件（FR-104/204）",
    },
    RequiredIndex {
        name: "idx_events_request",
        table: "events",
        columns: "request_id",
        purpose: "按模型请求串联事件（FR-204，v2）",
    },
    RequiredIndex {
        name: "idx_events_tool_call",
        table: "events",
        columns: "tool_call_id",
        purpose: "按工具调用串联事件（FR-204，v2）",
    },
    RequiredIndex {
        name: "idx_messages_session_ts_id",
        table: "messages",
        columns: "session_id, ts, id",
        purpose: "消息游标分页与同一时间戳破平局（FR-504，v3）",
    },
    RequiredIndex {
        name: "idx_tasks_session_status",
        table: "tasks",
        columns: "session_id, status",
        purpose: "任务列表与状态过滤（FR-103）",
    },
    RequiredIndex {
        name: "idx_tasks_session_stage",
        table: "tasks",
        columns: "session_id, stage",
        purpose: "阶段分布与概览聚合（FR-103/506，v3）",
    },
    RequiredIndex {
        name: "idx_checkpoints_task",
        table: "checkpoints",
        columns: "task_id",
        purpose: "检查点按任务恢复（FR-402/403）",
    },
    RequiredIndex {
        name: "idx_tool_executions_call",
        table: "tool_executions",
        columns: "tool_call_id",
        purpose: "工具执行恢复与未知状态核对（FR-410）",
    },
    RequiredIndex {
        name: "idx_audit_ts",
        table: "audit",
        columns: "ts",
        purpose: "审计按时间检索（§10.3）",
    },
];

fn query_err(err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("读取索引目录失败：{err}"))
}

/// 当前数据库中已安装的索引名（仅 `main` schema）。
///
/// # Errors
///
/// 索引目录不可读时返回 `io_error`（可重试）。
pub fn installed_indexes(conn: &Connection) -> Result<HashSet<String>, WhError> {
    let mut stmt = conn
        .prepare("SELECT index_name FROM duckdb_indexes() WHERE schema_name = 'main'")
        .map_err(query_err)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(query_err)?;
    let mut names = HashSet::new();
    for row in rows {
        names.insert(row.map_err(query_err)?);
    }
    Ok(names)
}

/// 缺失的必需索引名（按 [`REQUIRED_INDEXES`] 顺序）；空表示齐全。
///
/// # Errors
///
/// 索引目录不可读时返回 `io_error`（可重试）。
pub fn missing_indexes(conn: &Connection) -> Result<Vec<&'static str>, WhError> {
    let installed = installed_indexes(conn)?;
    Ok(REQUIRED_INDEXES
        .iter()
        .filter(|index| !installed.contains(index.name))
        .map(|index| index.name)
        .collect())
}

/// 校验必需索引齐全。
///
/// # Errors
///
/// 缺失时返回 `invalid_state`（不可重试：需完成迁移或修复工作区），消息列出缺失项。
pub fn verify_indexes(conn: &Connection) -> Result<(), WhError> {
    let missing = missing_indexes(conn)?;
    if missing.is_empty() {
        Ok(())
    } else {
        Err(WhError::invalid_state(format!(
            "缺少 {} 个必需索引：{}；schema 迁移未完成或被外部改动（§7.2）",
            missing.len(),
            missing.join(", ")
        )))
    }
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
    fn required_indexes_exist_after_migration() {
        let conn = migrated();
        assert_eq!(
            missing_indexes(&conn).expect("缺失检查"),
            Vec::<&str>::new()
        );
        verify_indexes(&conn).expect("索引齐全");
    }

    #[test]
    fn fresh_database_reports_all_required_indexes_missing() {
        let conn = Connection::open_in_memory().expect("内存数据库");
        let missing = missing_indexes(&conn).expect("缺失检查");
        assert_eq!(
            missing.len(),
            REQUIRED_INDEXES.len(),
            "未迁移的库应缺少全部必需索引"
        );

        let err = verify_indexes(&conn).expect_err("未迁移必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.message.contains(REQUIRED_INDEXES[0].name),
            "错误消息应列出缺失索引名"
        );
    }

    #[test]
    fn registry_entries_are_unique_and_described() {
        let mut names = HashSet::new();
        for index in REQUIRED_INDEXES {
            assert!(names.insert(index.name), "索引名重复：{}", index.name);
            assert!(!index.table.is_empty(), "索引必须声明所属表");
            assert!(!index.columns.is_empty(), "索引必须声明列");
            assert!(!index.purpose.is_empty(), "索引必须声明用途");
        }
    }

    #[test]
    fn legacy_messages_index_is_replaced_by_three_column_index() {
        let conn = migrated();
        let installed = installed_indexes(&conn).expect("索引目录");
        assert!(
            installed.contains("idx_messages_session_ts_id"),
            "v3 应安装三列消息索引"
        );
        assert!(
            !installed.contains("idx_messages_session_ts"),
            "v3 应删除被三列索引覆盖的旧两列索引"
        );
    }
}
