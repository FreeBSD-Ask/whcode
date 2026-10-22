//! # 工具执行“意图—执行—登记”与不确定恢复（T114、FR-409/410、ADR-007）
//!
//! 外部副作用（子进程、SSH 命令、运行时执行）一律走三步，SQL **只**在本模块：
//!
//! 1. [`record_intent`]：登记 `prepared` 意图（`input_hash` 先固定，执行尚未开始）；
//! 2. [`mark_running`]：副作用**可能**已发生，状态 `running`；
//! 3. [`record_result`]：登记终态（`completed`/`failed`/`timed_out`）与输出/副作用引用。
//!
//! ## 恢复协议（FR-410、V-105/V-307、SC-005）
//!
//! 进程在 1—3 任一步骤之间崩溃后，下一次打开工作区必须调用 [`recover_uncertain`]：把仍是
//! `prepared`/`running` 的记录标为 `unknown` 并返回清单。语义边界（不得越过）：
//!
//! - **不重放副作用**：本模块只读表与改状态，不执行任何外部动作——重放必须由调用方在核对后
//!   显式发起（幂等或人工决策）；
//! - **不宣称未发生**：`prepared` 与 `running` 一律视为“可能已发生”，不得降级为 `failed`；
//!   崩溃可能落在“已执行、未登记”之间（Check 196），因此 `unknown` 是唯一诚实的取值；
//! - **不宣称已发生但未登记不存在**：`unknown` 必须经核对、幂等重试或人工决策才可推进；
//! - 已登记终态（`completed`/`failed`/`timed_out`）不受恢复影响（Check 158 的“登记后”时点）；
//! - 扫描**有界**（[`MAX_RECOVERED`]），超出部分 `truncated=true`，下次启动继续。

use duckdb::{Connection, params, params_from_iter};
use whcode_domain::{ErrorCode, RuntimeExecutionStatus, WhError};

/// 单次恢复扫描上限（有界；避免一次启动长时间占用）。
pub const MAX_RECOVERED: usize = 1000;

/// 恢复后要求调用方采取的处置（写进返回值，避免“标记完就算完”）。
pub const UNCERTAIN_NEXT_ACTION: &str = "核对副作用是否已发生 → 幂等重试或人工决策（FR-410）";

fn exec_err(context: &str, err: impl std::fmt::Display) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}：{err}"))
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

/// `tool_executions` 行（只读投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecRow {
    /// 执行 ID。
    pub id: String,
    /// 工具调用 ID（关联 `tool_call_id`）。
    pub tool_call_id: String,
    /// 执行状态。
    pub state: RuntimeExecutionStatus,
    /// 输入哈希（意图固定，重放时用于幂等判定）。
    pub input_hash: String,
    /// 执行开始（Unix 毫秒；`prepared` 阶段为 `None`）。
    pub started_at_ms: Option<i64>,
    /// 结束（Unix 毫秒；未登记结果为 `None`）。
    pub finished_at_ms: Option<i64>,
    /// 输出引用（不内联大对象，R-05）。
    pub output_ref: Option<String>,
    /// 副作用凭证引用（外部操作的登记依据）。
    pub side_effect_ref: Option<String>,
}

/// 被恢复为 `unknown` 的单条记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredExec {
    /// 执行 ID。
    pub id: String,
    /// 工具调用 ID。
    pub tool_call_id: String,
    /// 恢复前的状态（`prepared` 或 `running`）。
    pub previous_state: RuntimeExecutionStatus,
}

/// 恢复报告。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    /// 本次被标记 `unknown` 的记录（按 ID 升序，最多 [`MAX_RECOVERED`] 条）。
    pub recovered: Vec<RecoveredExec>,
    /// 是否因有界扫描而截断（剩余记录下次启动继续）。
    pub truncated: bool,
    /// 处置建议（固定文本，便于 CUI/日志原样输出）。
    pub next_action: &'static str,
}

impl RecoveryReport {
    /// 本次是否未发现不确定记录。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recovered.is_empty()
    }

    /// 本次恢复的记录数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.recovered.len()
    }
}

fn status_from_str(value: &str) -> Result<RuntimeExecutionStatus, WhError> {
    const ALL: [RuntimeExecutionStatus; 7] = [
        RuntimeExecutionStatus::Prepared,
        RuntimeExecutionStatus::Running,
        RuntimeExecutionStatus::Completed,
        RuntimeExecutionStatus::Failed,
        RuntimeExecutionStatus::TimedOut,
        RuntimeExecutionStatus::Interrupted,
        RuntimeExecutionStatus::Unknown,
    ];
    ALL.into_iter()
        .find(|status| status.as_str() == value)
        .ok_or_else(|| {
            invalid(&format!(
                "未知执行状态：{value}（须为 domain RuntimeExecutionStatus 稳定名）"
            ))
        })
}

/// 登记执行意图（`prepared`）。
///
/// # Errors
///
/// 三个参数任一为空，或写入失败（含 ID 重复）时返回 [`WhError`]。
pub fn record_intent(
    conn: &Connection,
    id: &str,
    tool_call_id: &str,
    input_hash: &str,
) -> Result<(), WhError> {
    if id.trim().is_empty() || tool_call_id.trim().is_empty() || input_hash.trim().is_empty() {
        return Err(invalid("执行 ID、工具调用 ID 与输入哈希不得为空"));
    }
    conn.execute(
        "INSERT INTO tool_executions (id, tool_call_id, state, input_hash,
                                      started_at, finished_at, output_ref, side_effect_ref)
         VALUES (?, ?, 'prepared', ?, NULL, NULL, NULL, NULL)",
        params![id, tool_call_id, input_hash],
    )
    .map_err(|err| exec_err("登记执行意图", err))?;
    Ok(())
}

/// 进入执行阶段（`prepared` → `running`）。
///
/// # Errors
///
/// 记录不存在返回 `not_found`；状态不允许该转换返回 `invalid_state`。
pub fn mark_running(conn: &Connection, id: &str, now_ms: u64) -> Result<(), WhError> {
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let changed = conn
        .execute(
            "UPDATE tool_executions SET state = 'running', started_at = epoch_ms(?)
             WHERE id = ? AND state = 'prepared'",
            params![now, id],
        )
        .map_err(|err| exec_err("进入执行阶段", err))?;
    ensure_transition(conn, id, changed, "仅 prepared 可进入 running")
}

/// 登记终态结果（`completed` / `failed` / `timed_out`）。
///
/// `interrupted` / `unknown` 由恢复流程写入，不经本函数——避免把“不确定”伪装成“确定”。
///
/// # Errors
///
/// 状态不是终态、记录不存在或状态机不允许该转换时返回 [`WhError`]。
pub fn record_result(
    conn: &Connection,
    id: &str,
    status: RuntimeExecutionStatus,
    output_ref: Option<&str>,
    side_effect_ref: Option<&str>,
    now_ms: u64,
) -> Result<(), WhError> {
    let terminal = matches!(
        status,
        RuntimeExecutionStatus::Completed
            | RuntimeExecutionStatus::Failed
            | RuntimeExecutionStatus::TimedOut
    );
    if !terminal {
        return Err(invalid(&format!(
            "record_result 只接受终态（completed/failed/timed_out），收到 {}",
            status.as_str()
        )));
    }
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let changed = conn
        .execute(
            "UPDATE tool_executions
                SET state = ?, finished_at = epoch_ms(?), output_ref = ?, side_effect_ref = ?
              WHERE id = ? AND state IN ('prepared', 'running')",
            params![status.as_str(), now, output_ref, side_effect_ref, id],
        )
        .map_err(|err| exec_err("登记执行结果", err))?;
    ensure_transition(conn, id, changed, "仅 prepared/running 可登记结果")
}

/// 记录不存在则 `not_found`；存在但状态不允许该转换则 `invalid_state`。
fn ensure_transition(
    conn: &Connection,
    id: &str,
    changed: usize,
    expectation: &str,
) -> Result<(), WhError> {
    if changed > 0 {
        return Ok(());
    }
    match load(conn, id)? {
        None => Err(WhError::new(
            ErrorCode::NotFound,
            format!("执行记录不存在：{id}"),
        )),
        Some(row) => Err(invalid(&format!(
            "{expectation}（当前 {}）：{id}",
            row.state.as_str()
        ))),
    }
}

const TOOL_EXEC_COLUMNS: &str = "id, tool_call_id, state, input_hash, epoch_us(started_at), \
                                 epoch_us(finished_at), output_ref, side_effect_ref";

/// 读取单条执行记录（`unknown` 记录同样可查询，供核对使用）。
///
/// # Errors
///
/// 查询失败或状态列为未知取值时返回 [`WhError`]。
pub fn load(conn: &Connection, id: &str) -> Result<Option<ToolExecRow>, WhError> {
    let sql = format!("SELECT {TOOL_EXEC_COLUMNS} FROM tool_executions WHERE id = ?");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| exec_err("查询执行记录", err))?;
    let mut rows = stmt
        .query(params![id])
        .map_err(|err| exec_err("查询执行记录", err))?;
    let Some(row) = rows.next().map_err(|err| exec_err("查询执行记录", err))? else {
        return Ok(None);
    };
    let state_raw: String = row.get(2).map_err(|err| exec_err("读取执行状态", err))?;
    let micros_to_ms = |value: Option<i64>| value.map(|us| us.div_euclid(1000));
    Ok(Some(ToolExecRow {
        id: row.get(0).map_err(|err| exec_err("读取执行 ID", err))?,
        tool_call_id: row.get(1).map_err(|err| exec_err("读取工具调用 ID", err))?,
        state: status_from_str(&state_raw)?,
        input_hash: row.get(3).map_err(|err| exec_err("读取输入哈希", err))?,
        started_at_ms: micros_to_ms(
            row.get::<_, Option<i64>>(4)
                .map_err(|err| exec_err("读取开始时间", err))?,
        ),
        finished_at_ms: micros_to_ms(
            row.get::<_, Option<i64>>(5)
                .map_err(|err| exec_err("读取结束时间", err))?,
        ),
        output_ref: row.get(6).map_err(|err| exec_err("读取输出引用", err))?,
        side_effect_ref: row.get(7).map_err(|err| exec_err("读取副作用引用", err))?,
    }))
}

/// 恢复：把中断遗留的 `prepared`/`running` 记录标记为 `unknown`（**不执行任何外部动作**）。
///
/// 幂等：已 `unknown` 的记录不会被重复返回；已登记终态的记录不受影响。
///
/// # Errors
///
/// 查询或更新失败时返回 [`WhError`]（事务回滚，不留下半更新状态）。
pub fn recover_uncertain(conn: &mut Connection) -> Result<RecoveryReport, WhError> {
    let limit = MAX_RECOVERED + 1;
    let tx = conn
        .transaction()
        .map_err(|err| exec_err("开启恢复事务", err))?;
    let mut recovered: Vec<RecoveredExec> = Vec::new();
    {
        let mut stmt = tx
            .prepare(&format!(
                "SELECT id, tool_call_id, state FROM tool_executions
                  WHERE state IN ('prepared', 'running') ORDER BY id LIMIT {limit}"
            ))
            .map_err(|err| exec_err("扫描不确定执行", err))?;
        let mut rows = stmt
            .query([])
            .map_err(|err| exec_err("扫描不确定执行", err))?;
        while let Some(row) = rows.next().map_err(|err| exec_err("读取不确定执行", err))? {
            let state_raw: String = row.get(2).map_err(|err| exec_err("读取执行状态", err))?;
            recovered.push(RecoveredExec {
                id: row.get(0).map_err(|err| exec_err("读取执行 ID", err))?,
                tool_call_id: row.get(1).map_err(|err| exec_err("读取工具调用 ID", err))?,
                previous_state: status_from_str(&state_raw)?,
            });
        }
    }
    // 有界扫描：多取一条用于判定是否截断。
    let truncated = recovered.len() > MAX_RECOVERED;
    recovered.truncate(MAX_RECOVERED);

    if !recovered.is_empty() {
        let placeholders = vec!["?"; recovered.len()].join(", ");
        tx.execute(
            &format!("UPDATE tool_executions SET state = 'unknown' WHERE id IN ({placeholders})"),
            params_from_iter(recovered.iter().map(|item| item.id.as_str())),
        )
        .map_err(|err| exec_err("标记不确定执行", err))?;
    }
    tx.commit().map_err(|err| exec_err("提交恢复事务", err))?;

    Ok(RecoveryReport {
        recovered,
        truncated,
        next_action: UNCERTAIN_NEXT_ACTION,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;

    const NOW: u64 = 1_700_000_000_000;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    #[test]
    fn intent_then_result_reaches_completed() {
        let conn = migrated();
        record_intent(&conn, "X-1", "C-1", "hash-1").expect("意图");
        assert_eq!(
            load(&conn, "X-1").expect("读").expect("存在").state,
            RuntimeExecutionStatus::Prepared
        );
        mark_running(&conn, "X-1", NOW).expect("执行");
        record_result(
            &conn,
            "X-1",
            RuntimeExecutionStatus::Completed,
            Some("out-1"),
            Some("side-1"),
            NOW + 10,
        )
        .expect("结果");
        let row = load(&conn, "X-1").expect("读").expect("存在");
        assert_eq!(row.state, RuntimeExecutionStatus::Completed);
        assert_eq!(row.started_at_ms, Some(NOW as i64));
        assert_eq!(row.output_ref.as_deref(), Some("out-1"));
        assert_eq!(row.side_effect_ref.as_deref(), Some("side-1"));
    }

    #[test]
    fn recovery_marks_only_uncertain_states() {
        let mut conn = migrated();
        record_intent(&conn, "X-prepared", "C-1", "h").expect("意图");
        record_intent(&conn, "X-running", "C-2", "h").expect("意图");
        mark_running(&conn, "X-running", NOW).expect("执行");
        record_intent(&conn, "X-done", "C-3", "h").expect("意图");
        record_result(
            &conn,
            "X-done",
            RuntimeExecutionStatus::Completed,
            None,
            None,
            NOW,
        )
        .expect("结果");

        let report = recover_uncertain(&mut conn).expect("恢复");
        assert_eq!(report.len(), 2, "只应恢复 prepared/running：{report:?}");
        assert!(!report.truncated);
        assert_eq!(
            report.recovered[0].previous_state,
            RuntimeExecutionStatus::Prepared
        );
        assert_eq!(
            report.recovered[1].previous_state,
            RuntimeExecutionStatus::Running
        );
        assert_eq!(
            load(&conn, "X-done").expect("读").expect("存在").state,
            RuntimeExecutionStatus::Completed,
            "登记后的终态不得被误标 unknown"
        );
        assert_eq!(
            load(&conn, "X-prepared").expect("读").expect("存在").state,
            RuntimeExecutionStatus::Unknown
        );
        // 幂等：第二次恢复为空。
        assert!(recover_uncertain(&mut conn).expect("再恢复").is_empty());
    }

    #[test]
    fn result_rejects_non_terminal_and_wrong_order() {
        let conn = migrated();
        record_intent(&conn, "X-1", "C-1", "h").expect("意图");
        let err = record_result(
            &conn,
            "X-1",
            RuntimeExecutionStatus::Unknown,
            None,
            None,
            NOW,
        )
        .expect_err("unknown 不得经 record_result 写入");
        assert!(err.to_string().contains("只接受终态"), "{err}");

        mark_running(&conn, "X-1", NOW).expect("执行");
        let err = mark_running(&conn, "X-1", NOW).expect_err("重复进入执行必须拒绝");
        assert!(err.to_string().contains("running"), "{err}");
    }

    #[test]
    fn missing_record_reports_not_found() {
        let conn = migrated();
        let err = mark_running(&conn, "X-none", NOW).expect_err("不存在");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(
            record_intent(&conn, "X-1", "C-1", "h").is_ok(),
            "前置：首次登记成功"
        );
        let err = record_intent(&conn, "X-1", "C-2", "h").expect_err("重复 ID");
        assert_eq!(
            err.code,
            ErrorCode::IoError,
            "重复主键按写入错误上报: {err}"
        );
    }
}
