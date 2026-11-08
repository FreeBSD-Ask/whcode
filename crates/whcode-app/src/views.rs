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

use crate::llm::ConversationEntry;

/// 对话尾部（`view.chat` 历史，FR-741/742）：`items` 按时间正序；形状与 CUI/Web 同源
/// （三端共用数据形状，本模块唯一实现）。
#[must_use]
pub fn conversation_entries_value(entries: &[ConversationEntry]) -> Value {
    json!({
        "items": entries
            .iter()
            .map(|entry| {
                json!({
                    "id": entry.id,
                    "role": entry.role,
                    "content": entry.content,
                    "ts_us": entry.ts_us,
                })
            })
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::TaskStatus;

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

    /// 会话/任务/对话三种形状的键集合与取值口径固定（FR-604／FR-610／FR-741）。
    #[test]
    fn session_task_and_conversation_shapes_keep_stable_key_sets() {
        let session = SessionRow {
            id: "S-1".to_string(),
            title: "会话标题".to_string(),
            created_at_ms: 1,
            last_active_at_ms: 2,
            model: Some("m".to_string()),
            archived: false,
            status: "active".to_string(),
            stage: "specify".to_string(),
        };
        let session_value = session_value(&session);
        let mut session_keys: Vec<&str> = session_value
            .as_object()
            .expect("session 形状应为对象")
            .keys()
            .map(String::as_str)
            .collect();
        session_keys.sort_unstable();
        assert_eq!(
            session_keys,
            vec![
                "archived",
                "created_at_ms",
                "id",
                "stage",
                "status",
                "title"
            ]
        );
        // `model` 与 `last_active_at_ms` 不进三端会话形状（列表排序另有语义源）。
        assert!(session_value.get("model").is_none());

        let task = TaskRow {
            id: "T-1".to_string(),
            session_id: "S-1".to_string(),
            title: "任务标题".to_string(),
            stage: "plan".to_string(),
            status: TaskStatus::Ready,
            parent_id: None,
            acceptance_json: "[]".to_string(),
            retry_count: 2,
            updated_at_ms: 3,
        };
        let task_value = task_value(&task);
        assert_eq!(task_value["status"], "ready");
        assert_eq!(task_value["retry_count"], 2);
        assert_eq!(task_value["stage"], "plan");

        let entries = vec![
            ConversationEntry {
                id: "MSG-1".to_string(),
                role: "user".to_string(),
                content: "问题".to_string(),
                ts_us: 10,
            },
            ConversationEntry {
                id: "MSG-2".to_string(),
                role: "assistant".to_string(),
                content: "回答".to_string(),
                ts_us: 20,
            },
        ];
        let conversation = conversation_entries_value(&entries);
        let items = conversation["items"]
            .as_array()
            .expect("items 应为数组")
            .clone();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "MSG-1");
        assert_eq!(items[1]["role"], "assistant");
        assert_eq!(items[1]["ts_us"], 20);
    }
}
