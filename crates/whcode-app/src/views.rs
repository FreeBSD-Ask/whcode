//! 三端共用的数据形状（FR-604、FR-610）。
//!
//! 同一业务结果在 CUI 的 `--json`、Web 的 `/api/*` 与未来的 SSH 入口必须是**同一组字段名**
//! （`contracts/cui-cli.md` §3：`schema_version` + 数据对象；数据字段名不翻译）。
//! 本模块是这些形状的唯一实现：适配器只负责传输与文案，不得各自拼字段。
//!
//! 只做序列化，不读库、不判定权限——分层不变（ADR-018）。

use serde_json::{Value, json};
use whcode_store::PurgeCounts;
use whcode_store::SessionRow;
use whcode_store::tasks::TaskRow;

/// 会话元数据（`session` 对象）：`session new/rename/archive/list` 共用。
#[must_use]
pub fn session_value(row: &SessionRow) -> Value {
    json!({
        "id": row.id,
        "title": row.title,
        "stage": row.stage,
        "status": row.status,
        "archived": row.archived,
        "created_at_ms": row.created_at_ms,
    })
}

/// 任务元数据（`task` 对象）：`task retry/abort` 共用。
#[must_use]
pub fn task_value(row: &TaskRow) -> Value {
    json!({
        "id": row.id,
        "title": row.title,
        "status": row.status.as_str(),
        "stage": row.stage,
        "retry_count": row.retry_count,
    })
}

/// 清理/删除的影响行数（`counts` 对象）：预览与执行共用同一组键。
///
/// 覆盖 `PurgeCounts` 的全部九个字段——少报表名等于隐瞒将被删除的数据（FR-204）。
#[must_use]
pub fn purge_counts_value(counts: &PurgeCounts) -> Value {
    json!({
        "sessions": counts.sessions,
        "tasks": counts.tasks,
        "checkpoints": counts.checkpoints,
        "runtime_executions": counts.runtime_executions,
        "stage_artifacts": counts.stage_artifacts,
        "events": counts.events,
        "messages": counts.messages,
        "context_windows": counts.context_windows,
        "session_overview": counts.session_overview,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 字段集合固定：新增/删除字段必须同步改契约与三端（FR-610）。
    #[test]
    fn shared_shapes_keep_stable_key_sets() {
        let counts = PurgeCounts {
            sessions: 1,
            tasks: 2,
            checkpoints: 3,
            runtime_executions: 4,
            stage_artifacts: 5,
            events: 6,
            messages: 7,
            context_windows: 8,
            session_overview: 9,
        };
        assert_eq!(
            purge_counts_value(&counts),
            json!({
                "sessions": 1,
                "tasks": 2,
                "checkpoints": 3,
                "runtime_executions": 4,
                "stage_artifacts": 5,
                "events": 6,
                "messages": 7,
                "context_windows": 8,
                "session_overview": 9,
            })
        );
    }
}
