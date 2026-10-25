//! # 审计写入（T187、ADR-040 决策 4；ADR-018 的 SQL 边界）
//!
//! 证书库变更的审计落库入口。SQL 只在本 crate 出现（ADR-018）：`whcode-app` 通过
//! [`insert_record`] 写入，不自行拼接语句；`detail_json` 按 ADR-036 的唯一口径
//! `CAST(? AS JSON)::VARIANT` 写入，时间用数据库 `current_timestamp`。
//!
//! 语义约束（domain `AuditSink` 契约）：写入失败必须返回错误，调用方不得把操作
//! 视为成功；本模块不做事务包装——单条审计是独立语句，回滚责任在编排层
//! （证书文件的字节回滚由 `whcode-app::cert` 承担）。

use duckdb::{Connection, params};
use whcode_domain::{AuditRecord, ErrorCode, WhError};

/// 写入一条审计记录。
///
/// `record.ts_ms` 不参与本语句：时间由数据库 `current_timestamp` 统一，避免调用方
/// 时钟漂移；`record.detail` 序列化为 JSON 文本后按 `VARIANT` 写入。
///
/// # Errors
///
/// - `detail` 序列化失败：`parse_error`（`serde_json::Value` 不应失败，此处仍显式处理）；
/// - `audit` 表不存在或连接不可用：`io_error`，消息含目标与动作，便于排障；
/// - 主键冲突（同一 `id` 重复写入）：`invalid_state`，审计不可覆盖。
pub fn insert_record(conn: &Connection, record: &AuditRecord) -> Result<(), WhError> {
    let detail = serde_json::to_string(&record.detail).map_err(|err| {
        WhError::parse_error(format!(
            "审计记录 {} 的 detail_json 序列化失败：{err}；审计未写入，操作不得视为成功",
            record.id
        ))
    })?;
    conn.execute(
        "INSERT INTO audit (id, actor, action, target, detail_json, ts)
         VALUES (?, ?, ?, ?, CAST(? AS JSON)::VARIANT, current_timestamp)",
        params![
            record.id,
            record.actor,
            record.action.as_str(),
            record.target,
            detail,
        ],
    )
    .map_err(|err| {
        let text = err.to_string();
        let lower = text.to_lowercase();
        if lower.contains("constraint") || lower.contains("duplicate") {
            WhError::new(
                ErrorCode::InvalidState,
                format!(
                    "审计记录 id “{}” 已存在（目标 {}，动作 {}），审计不可覆盖；\
                     请更换记录 id 后重试",
                    record.id,
                    record.target,
                    record.action.as_str()
                ),
            )
        } else {
            WhError::new(
                ErrorCode::IoError,
                format!(
                    "写入审计记录 {} 失败（目标 {}，动作 {}）：{text}；\
                     确认 audit 表已迁移且数据库连接可用，审计失败时操作不得视为成功",
                    record.id,
                    record.target,
                    record.action.as_str()
                ),
            )
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use whcode_domain::AuditAction;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        crate::schema::apply_migrations(&conn).expect("迁移");
        conn
    }

    fn record(id: &str) -> AuditRecord {
        AuditRecord {
            id: id.to_string(),
            actor: "tester".to_string(),
            action: AuditAction::TlsConfigChange,
            target: "tls:settings".to_string(),
            detail: json!({
                "action": "update-settings",
                "inbound": "off",
                "cert_count": 0,
            }),
            ts_ms: 1_700_000_000_000,
        }
    }

    /// 写入后按读取口径读回：`CAST(CAST(detail_json AS JSON) AS VARCHAR)`。
    #[test]
    fn insert_record_round_trips_fields_and_variant_detail() {
        let conn = migrated();
        let record = record("tls-settings-1");
        insert_record(&conn, &record).expect("写入审计");

        let (actor, action, target, detail): (String, String, String, String) = conn
            .query_row(
                "SELECT actor, action, target, CAST(CAST(detail_json AS JSON) AS VARCHAR)
                 FROM audit WHERE id = ?",
                params![record.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("读回审计");
        assert_eq!(actor, "tester");
        assert_eq!(action, "tls_config_change");
        assert_eq!(target, "tls:settings");
        let detail: serde_json::Value = serde_json::from_str(&detail).expect("detail 可解析");
        assert_eq!(detail["action"], json!("update-settings"));
        assert_eq!(detail["inbound"], json!("off"));
        assert_eq!(detail["cert_count"], json!(0));
    }

    /// 重复主键：`invalid_state` + 可操作消息，已有行不被覆盖。
    #[test]
    fn duplicate_primary_key_is_rejected_and_row_is_preserved() {
        let conn = migrated();
        insert_record(&conn, &record("tls-dup")).expect("首次写入");
        let mut changed = record("tls-dup");
        changed.target = "tls:outbound-ca:builtin".to_string();
        let err = insert_record(&conn, &changed).expect_err("重复 id 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(!err.retryable, "审计覆盖不可重试，须更换记录 id");
        assert!(err.message.contains("tls-dup"), "{}", err.message);
        assert!(err.message.contains("不可覆盖"), "{}", err.message);

        let target: String = conn
            .query_row("SELECT target FROM audit WHERE id = 'tls-dup'", [], |row| {
                row.get(0)
            })
            .expect("读回原行");
        assert_eq!(target, "tls:settings", "失败路径不得覆盖既有审计");
    }

    /// 表不存在（未迁移）：`io_error` + 可操作消息，不 panic。
    #[test]
    fn missing_table_is_io_error_with_actionable_message() {
        let conn = Connection::open_in_memory().expect("内存数据库");
        let err = insert_record(&conn, &record("tls-missing-table")).expect_err("缺表必须报错");
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(err.message.contains("tls-missing-table"), "{}", err.message);
        assert!(err.message.contains("audit"), "{}", err.message);
        assert!(
            err.message.contains("迁移") || err.message.contains("连接"),
            "消息须给出补救方向：{}",
            err.message
        );
    }
}
