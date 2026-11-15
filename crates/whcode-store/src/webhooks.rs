//! # webhook 幂等投递（P6.4／FR-788／ADR-079）
//!
//! 每行是一条“端点 × 事件”的投递事实，主键是幂等键 `<endpoint>:<event_id>`。
//! 认领用 `INSERT ... ON CONFLICT DO NOTHING`：影响 0 行说明这条事件在该端点下已经被
//! 认领过，调用方不得再投递——**重复执行同一命令不会重复触发外部副作用**（ADR-079
//! 决策 2，DSH 反例的直接修正）。
//!
//! 状态封闭三态：`claimed`（已认领、结果未知）／`delivered`（终态）／`failed`
//! （`attempts < max_attempts` 时可重新认领，达到上限即终态）。永久失败（接收方 4xx）
//! 由调用方把 `attempts` 直接提到上限，本模块不再给它重新认领的机会。
//!
//! 有界性：端点名 ≤ [`MAX_ENDPOINT_CHARS`]、事件标识 ≤ [`MAX_EVENT_ID_CHARS`]、错误文本
//! ≤ [`MAX_ERROR_CHARS`]（按字符边界截断）、列表一次 ≤ [`MAX_DELIVERY_ROWS`] 条。
//!
//! 边界：本模块只读写 `webhook_deliveries` 表；配置解析、投递计划、HTTP 与权限在
//! `whcode-app::webhooks`／装配根（ADR-018／ADR-031）。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

/// 端点名字符上限（与配置校验一致）。
pub const MAX_ENDPOINT_CHARS: usize = 64;

/// 事件标识字符上限。
pub const MAX_EVENT_ID_CHARS: usize = 64;

/// 错误文本字符上限（落库前截断）。
pub const MAX_ERROR_CHARS: usize = 512;

/// 单次列表返回条数上限。
pub const MAX_DELIVERY_ROWS: usize = 500;

/// 单个端点可订阅的事件类型上限（与配置校验一致）。
pub const MAX_EVENT_KINDS: usize = 16;

/// 投递状态（库内稳定名与枚举一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStatus {
    /// 已认领，结果未知。
    Claimed,
    /// 已投递（终态）。
    Delivered,
    /// 失败（未达上限可重试）。
    Failed,
}

impl DeliveryStatus {
    /// 库内稳定名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }

    /// 解析稳定名（未知取值返回 `None`，由调用方决定报错口径）。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "claimed" => Some(Self::Claimed),
            "delivered" => Some(Self::Delivered),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// 一次投递事实（库内一行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRecord {
    /// 幂等键 `<endpoint>:<event_id>`。
    pub idempotency_key: String,
    /// 端点名。
    pub endpoint: String,
    /// 事件标识。
    pub event_id: String,
    /// 事件序号（便于按事件定位）。
    pub event_seq: i64,
    /// 状态。
    pub status: DeliveryStatus,
    /// 已尝试次数。
    pub attempts: i64,
    /// 最近一次 HTTP 状态码（传输失败时为 `None`）。
    pub last_status_code: Option<i64>,
    /// 最近一次错误文本（截断后）。
    pub last_error: Option<String>,
}

fn delivery_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("webhook {context} 失败：{err}"))
}

fn invalid(message: impl Into<String>) -> WhError {
    WhError::new(ErrorCode::InvalidState, message.into())
}

/// 按字符边界截断（不切在多字节字符中间）。
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

/// 幂等键：`<endpoint>:<event_id>`。
///
/// 端点改名即视为新端点（ADR-079 §6 已登记）；键的两段各自有长度上限。
///
/// # Errors
///
/// 端点名或事件标识为空、超长，或含控制字符时返回 [`WhError`]。
pub fn idempotency_key(endpoint: &str, event_id: &str) -> Result<String, WhError> {
    for (label, value, max) in [
        ("端点名", endpoint, MAX_ENDPOINT_CHARS),
        ("事件标识", event_id, MAX_EVENT_ID_CHARS),
    ] {
        if value.trim().is_empty() {
            return Err(invalid(format!("webhook {label}不得为空")));
        }
        if value.chars().count() > max {
            return Err(invalid(format!(
                "webhook {label}超过 {max} 字符上限（ADR-079 决策 1）"
            )));
        }
        if value.chars().any(char::is_control) {
            return Err(invalid(format!("webhook {label}不得含控制字符")));
        }
    }
    Ok(format!("{endpoint}:{event_id}"))
}

const DELIVERY_COLUMNS: &str = "idempotency_key, endpoint, event_id, event_seq, status, attempts, \
                                last_status_code, last_error";

fn row_to_record(row: &duckdb::Row<'_>) -> Result<DeliveryRecord, WhError> {
    let status_text: String = row.get(4).map_err(|err| delivery_err("读取状态", err))?;
    let status = DeliveryStatus::parse(&status_text).ok_or_else(|| {
        invalid(format!(
            "webhook 投递状态 `{status_text}` 未知：允许 claimed／delivered／failed（库内容漂移必须显式）"
        ))
    })?;
    Ok(DeliveryRecord {
        idempotency_key: row.get(0).map_err(|err| delivery_err("读取幂等键", err))?,
        endpoint: row.get(1).map_err(|err| delivery_err("读取端点", err))?,
        event_id: row.get(2).map_err(|err| delivery_err("读取事件", err))?,
        event_seq: row
            .get(3)
            .map_err(|err| delivery_err("读取事件序号", err))?,
        status,
        attempts: row
            .get(5)
            .map_err(|err| delivery_err("读取尝试次数", err))?,
        last_status_code: row.get(6).map_err(|err| delivery_err("读取状态码", err))?,
        last_error: row
            .get(7)
            .map_err(|err| delivery_err("读取错误文本", err))?,
    })
}

fn read_record(conn: &Connection, key: &str) -> Result<DeliveryRecord, WhError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries WHERE idempotency_key = ?"
        ))
        .map_err(|err| delivery_err("准备读回", err))?;
    let mut rows = stmt
        .query(duckdb::params![key])
        .map_err(|err| delivery_err("读回", err))?;
    match rows.next().map_err(|err| delivery_err("遍历读回", err))? {
        Some(row) => row_to_record(row),
        None => Err(WhError::new(
            ErrorCode::NotFound,
            format!("webhook 投递记录 {key} 不存在"),
        )),
    }
}

/// 认领一次投递：首次认领插入新行；已失败且未达上限时递增次数。
///
/// 返回 `Ok(None)` 表示**不可认领**（已投递，或失败已达上限）——调用方应跳过该事件，
/// 不得静默重发（ADR-079 决策 2／3）。
///
/// # Errors
///
/// 端点名／事件标识非法、`max_attempts` 不在 1—5、或存储失败时返回 [`WhError`]。
pub fn claim_delivery(
    conn: &Connection,
    endpoint: &str,
    event_id: &str,
    event_seq: i64,
    max_attempts: i64,
) -> Result<Option<DeliveryRecord>, WhError> {
    if !(1..=5).contains(&max_attempts) {
        return Err(invalid(format!(
            "webhook max_attempts 必须在 1—5：收到 {max_attempts}（ADR-079 决策 1）"
        )));
    }
    let key = idempotency_key(endpoint, event_id)?;
    let inserted = conn
        .execute(
            "INSERT INTO webhook_deliveries
                 (idempotency_key, endpoint, event_id, event_seq, status, attempts)
             VALUES (?, ?, ?, ?, 'claimed', 1)
             ON CONFLICT (idempotency_key) DO NOTHING",
            duckdb::params![key, endpoint, event_id, event_seq],
        )
        .map_err(|err| delivery_err("认领", err))?;
    if inserted == 1 {
        return read_record(conn, &key).map(Some);
    }
    // 已存在：只有“失败且未达上限”才允许重新认领（次数 +1）。
    let updated = conn
        .execute(
            "UPDATE webhook_deliveries
                SET status = 'claimed', attempts = attempts + 1, updated_at = current_timestamp,
                    last_error = NULL
              WHERE idempotency_key = ? AND status = 'failed' AND attempts < ?",
            duckdb::params![key, max_attempts],
        )
        .map_err(|err| delivery_err("重新认领", err))?;
    if updated == 1 {
        return read_record(conn, &key).map(Some);
    }
    Ok(None)
}

/// 标记投递成功（终态）。
///
/// # Errors
///
/// 记录不存在时返回 `not_found`；存储失败返回 [`WhError`]。
pub fn mark_delivered(
    conn: &Connection,
    key: &str,
    status_code: Option<u16>,
) -> Result<DeliveryRecord, WhError> {
    let code = status_code.map(i64::from);
    let updated = conn
        .execute(
            "UPDATE webhook_deliveries
                SET status = 'delivered', last_status_code = ?, last_error = NULL,
                    updated_at = current_timestamp
              WHERE idempotency_key = ?",
            duckdb::params![code, key],
        )
        .map_err(|err| delivery_err("标记成功", err))?;
    if updated == 0 {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("webhook 投递记录 {key} 不存在：先认领再标记"),
        ));
    }
    read_record(conn, key)
}

/// 标记投递失败；`permanent` 为真时把 `attempts` 直接提到上限（不再重试）。
///
/// # Errors
///
/// `max_attempts` 不在 1—5、记录不存在或存储失败时返回 [`WhError`]。
pub fn mark_failed(
    conn: &Connection,
    key: &str,
    status_code: Option<u16>,
    error: &str,
    permanent: bool,
    max_attempts: i64,
) -> Result<DeliveryRecord, WhError> {
    if !(1..=5).contains(&max_attempts) {
        return Err(invalid(format!(
            "webhook max_attempts 必须在 1—5：收到 {max_attempts}（ADR-079 决策 1）"
        )));
    }
    let code = status_code.map(i64::from);
    let text = truncate_chars(error.trim(), MAX_ERROR_CHARS);
    let updated = conn
        .execute(
            "UPDATE webhook_deliveries
                SET status = 'failed',
                    attempts = CASE WHEN ? THEN ? ELSE attempts END,
                    last_status_code = ?, last_error = ?, updated_at = current_timestamp
              WHERE idempotency_key = ?",
            duckdb::params![permanent, max_attempts, code, text, key],
        )
        .map_err(|err| delivery_err("标记失败", err))?;
    if updated == 0 {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("webhook 投递记录 {key} 不存在：先认领再标记"),
        ));
    }
    read_record(conn, key)
}

/// 列出投递记录（可按状态过滤；按更新时间倒序；`limit` 夹取到
/// [`MAX_DELIVERY_ROWS`]）。
///
/// # Errors
///
/// `limit` 为 0、状态名未知或存储失败时返回 [`WhError`]。
pub fn list_deliveries(
    conn: &Connection,
    status: Option<DeliveryStatus>,
    limit: usize,
) -> Result<Vec<DeliveryRecord>, WhError> {
    if limit == 0 {
        return Err(invalid("webhook 列表 limit 必须大于 0"));
    }
    let limit_i64 = i64::try_from(limit.min(MAX_DELIVERY_ROWS))
        .map_err(|_| invalid("webhook 列表 limit 超出 i64 范围"))?;
    let mut records = Vec::new();
    match status {
        Some(status) => {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries
                      WHERE status = ? ORDER BY updated_at DESC, idempotency_key DESC LIMIT ?"
                ))
                .map_err(|err| delivery_err("准备按状态查询", err))?;
            let mut rows = stmt
                .query(duckdb::params![status.as_str(), limit_i64])
                .map_err(|err| delivery_err("按状态查询", err))?;
            while let Some(row) = rows.next().map_err(|err| delivery_err("遍历", err))? {
                records.push(row_to_record(row)?);
            }
        }
        None => {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries
                      ORDER BY updated_at DESC, idempotency_key DESC LIMIT ?"
                ))
                .map_err(|err| delivery_err("准备列表", err))?;
            let mut rows = stmt
                .query(duckdb::params![limit_i64])
                .map_err(|err| delivery_err("列表", err))?;
            while let Some(row) = rows.next().map_err(|err| delivery_err("遍历", err))? {
                records.push(row_to_record(row)?);
            }
        }
    }
    Ok(records)
}

/// 按状态统计投递记录数（固定三态，缺失按 0 计）。
///
/// # Errors
///
/// 存储失败时返回 [`WhError`]。
pub fn count_by_status(conn: &Connection) -> Result<[u64; 3], WhError> {
    let mut counts = [0u64; 3];
    let mut stmt = conn
        .prepare("SELECT status, COUNT(*) FROM webhook_deliveries GROUP BY status")
        .map_err(|err| delivery_err("准备统计", err))?;
    let mut rows = stmt.query([]).map_err(|err| delivery_err("统计", err))?;
    while let Some(row) = rows.next().map_err(|err| delivery_err("遍历统计", err))? {
        let status: String = row
            .get(0)
            .map_err(|err| delivery_err("读取统计状态", err))?;
        let count: i64 = row
            .get(1)
            .map_err(|err| delivery_err("读取统计计数", err))?;
        let index = match DeliveryStatus::parse(&status) {
            Some(DeliveryStatus::Claimed) => 0,
            Some(DeliveryStatus::Delivered) => 1,
            Some(DeliveryStatus::Failed) => 2,
            None => {
                return Err(invalid(format!(
                    "webhook 投递状态 `{status}` 未知：库内容漂移必须显式"
                )));
            }
        };
        counts[index] = u64::try_from(count).unwrap_or(0);
    }
    Ok(counts)
}

/// 一条待投递事件（`events` 表的一行；准备层投影用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEvent {
    /// 事件标识（幂等键的第二段）。
    pub id: String,
    /// 事件序号（定位与排序）。
    pub seq: i64,
    /// 所属会话。
    pub session_id: String,
    /// 事件类型稳定名。
    pub kind: String,
    /// 时间戳（库内原文，交由准备层格式化）。
    pub ts: String,
    /// 载荷文本（VARIANT 的 JSON 文本；脱敏与截断在 `whcode-app::webhooks`）。
    pub payload: String,
}

/// 某端点下“订阅类型命中且仍可投递”的事件（按 `seq` 升序，有界）。
///
/// 反连接（`NOT EXISTS`）就是去重的读取侧：已投递、已认领待结果、以及失败已达上限的
/// 事件都不会出现在计划里；**失败但未达上限**的会重新出现（重试路径），因此重复执行
/// 不会重复投递已成功的事件（ADR-079 决策 2／3）。
///
/// `max_attempts` 必须与配置一致：它是“失败是否还能重试”的唯一判据。
///
/// # Errors
///
/// `kinds` 为空或超过 [`MAX_EVENT_KINDS`]、`limit` 为 0 或超过 [`MAX_DELIVERY_ROWS`]、
/// `max_attempts` 不在 1—5、存储失败时返回 [`WhError`]。
pub fn pending_events(
    conn: &Connection,
    endpoint: &str,
    kinds: &[String],
    limit: usize,
    max_attempts: i64,
) -> Result<Vec<PendingEvent>, WhError> {
    if kinds.is_empty() {
        return Err(invalid(
            "webhook 事件类型列表为空：端点必须至少订阅一种事件",
        ));
    }
    if kinds.len() > MAX_EVENT_KINDS {
        return Err(invalid(format!(
            "webhook 事件类型超过 {MAX_EVENT_KINDS} 种上限（ADR-079 决策 1）"
        )));
    }
    if !(1..=5).contains(&max_attempts) {
        return Err(invalid(format!(
            "webhook max_attempts 必须在 1—5：收到 {max_attempts}（ADR-079 决策 1）"
        )));
    }
    if limit == 0 || limit > MAX_DELIVERY_ROWS {
        return Err(invalid(format!(
            "webhook 待投递查询 limit 必须在 1—{MAX_DELIVERY_ROWS}"
        )));
    }
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(invalid("webhook 端点名不得为空"));
    }
    let placeholders = vec!["?"; kinds.len()].join(", ");
    let sql = format!(
        "SELECT e.id, e.seq, e.session_id, e.kind, CAST(e.ts AS VARCHAR),
                CAST(CAST(e.payload_json AS JSON) AS VARCHAR)
           FROM events e
          WHERE e.kind IN ({placeholders})
            AND NOT EXISTS (
                SELECT 1 FROM webhook_deliveries d
                 WHERE d.endpoint = ? AND d.event_id = e.id
                   AND (d.status IN ('delivered', 'claimed')
                        OR (d.status = 'failed' AND d.attempts >= ?))
            )
          ORDER BY e.seq ASC LIMIT ?"
    );
    let mut binds: Vec<duckdb::types::Value> = kinds
        .iter()
        .map(|kind| duckdb::types::Value::Text(kind.clone()))
        .collect();
    binds.push(duckdb::types::Value::Text(endpoint.to_string()));
    binds.push(duckdb::types::Value::BigInt(max_attempts));
    binds.push(duckdb::types::Value::Int(
        i32::try_from(limit).unwrap_or(i32::MAX),
    ));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| delivery_err("准备待投递查询", err))?;
    let mut rows = stmt
        .query(duckdb::params_from_iter(binds.iter()))
        .map_err(|err| delivery_err("待投递查询", err))?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().map_err(|err| delivery_err("遍历待投递", err))? {
        events.push(PendingEvent {
            id: row
                .get(0)
                .map_err(|err| delivery_err("读取事件标识", err))?,
            seq: row
                .get(1)
                .map_err(|err| delivery_err("读取事件序号", err))?,
            session_id: row.get(2).map_err(|err| delivery_err("读取会话", err))?,
            kind: row
                .get(3)
                .map_err(|err| delivery_err("读取事件类型", err))?,
            ts: row.get(4).map_err(|err| delivery_err("读取时间戳", err))?,
            payload: row.get(5).map_err(|err| delivery_err("读取载荷", err))?,
        });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    /// 幂等键与首次认领：键形状稳定，认领写一行 `claimed`。
    #[test]
    fn claim_creates_one_claimed_record() {
        let conn = fixture();
        let key = idempotency_key("ci", "EV-7").expect("键");
        assert_eq!(key, "ci:EV-7");
        let record = claim_delivery(&conn, "ci", "EV-7", 7, 3)
            .expect("认领")
            .expect("首次认领必须成功");
        assert_eq!(record.idempotency_key, key);
        assert_eq!(record.status, DeliveryStatus::Claimed);
        assert_eq!(record.attempts, 1);
        assert_eq!(record.event_seq, 7);
    }

    /// 核心判据：已投递的事件再认领返回 `None`（不重复投递）。
    #[test]
    fn delivered_event_is_not_claimed_twice() {
        let conn = fixture();
        claim_delivery(&conn, "ci", "EV-7", 7, 3)
            .expect("认领")
            .expect("首次");
        mark_delivered(&conn, "ci:EV-7", Some(204)).expect("成功");
        assert!(
            claim_delivery(&conn, "ci", "EV-7", 7, 3)
                .expect("再认领")
                .is_none(),
            "已投递的事件不得再次认领（ADR-079 决策 2）"
        );
    }

    /// 失败重试有界：未达上限可重试，达上限即终态。
    #[test]
    fn failed_records_retry_until_max_attempts() {
        let conn = fixture();
        claim_delivery(&conn, "ci", "EV-7", 7, 2)
            .expect("认领")
            .expect("1");
        mark_failed(&conn, "ci:EV-7", Some(503), "上游 503", false, 2).expect("失败 1");
        let second = claim_delivery(&conn, "ci", "EV-7", 7, 2)
            .expect("重试")
            .expect("未达上限可重试");
        assert_eq!(second.attempts, 2);
        mark_failed(&conn, "ci:EV-7", Some(503), "上游 503", false, 2).expect("失败 2");
        assert!(
            claim_delivery(&conn, "ci", "EV-7", 7, 2)
                .expect("第三次")
                .is_none(),
            "达到 max_attempts 后不得再认领"
        );
    }

    /// 永久失败：一次失败即到上限，不再提供重试。
    #[test]
    fn permanent_failure_exhausts_attempts_immediately() {
        let conn = fixture();
        claim_delivery(&conn, "ci", "EV-8", 8, 5)
            .expect("认领")
            .expect("1");
        let record = mark_failed(&conn, "ci:EV-8", Some(400), "载荷被拒绝", true, 5).expect("失败");
        assert_eq!(record.attempts, 5, "永久失败直接置为上限");
        assert_eq!(record.status, DeliveryStatus::Failed);
        assert!(
            claim_delivery(&conn, "ci", "EV-8", 8, 5)
                .expect("再认领")
                .is_none()
        );
    }

    /// 边界与库内容漂移：非法键、未知状态、未认领就标记、limit=0 全部显式报错。
    #[test]
    fn invalid_inputs_and_drift_are_explicit() {
        let conn = fixture();
        assert_eq!(
            idempotency_key("", "EV-1").expect_err("空端点").code,
            ErrorCode::InvalidState
        );
        assert_eq!(
            idempotency_key("ci", &"x".repeat(MAX_EVENT_ID_CHARS + 1))
                .expect_err("超长事件标识")
                .code,
            ErrorCode::InvalidState
        );
        assert_eq!(
            claim_delivery(&conn, "ci", "EV-1", 1, 9)
                .expect_err("max_attempts 越界")
                .code,
            ErrorCode::InvalidState
        );
        assert_eq!(
            mark_delivered(&conn, "ci:EV-404", Some(200))
                .expect_err("未认领")
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            list_deliveries(&conn, None, 0).expect_err("limit=0").code,
            ErrorCode::InvalidState
        );
        conn.execute(
            "INSERT INTO webhook_deliveries
                 (idempotency_key, endpoint, event_id, event_seq, status, attempts)
             VALUES ('ci:EV-9', 'ci', 'EV-9', 9, 'unknown', 1)",
            [],
        )
        .expect("插入漂移行");
        let err = list_deliveries(&conn, None, 10).expect_err("未知状态必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("unknown"), "{}", err.message);
    }

    /// 待投递：类型命中、已认领被排除、按 seq 升序、跨端点互不影响。
    #[test]
    fn pending_events_filters_kinds_and_excludes_claimed() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status, stage)
             VALUES ('S-1', '待投递', epoch_ms(1000), epoch_ms(1000), NULL, FALSE, 'active', 'implement')",
            [],
        )
        .expect("插入会话");
        for (seq, id, kind) in [
            (1i64, "EV-1", "TurnFinished"),
            (2, "EV-2", "StreamingChunk"),
            (3, "EV-3", "TurnFinished"),
        ] {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, 'S-1', ?, CAST('{\"n\":1}' AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                duckdb::params![seq, id, kind],
            )
            .expect("插入事件");
        }
        let kinds = vec!["TurnFinished".to_string()];
        let pending = pending_events(&conn, "ci", &kinds, 10, 3).expect("待投递");
        let ids: Vec<&str> = pending.iter().map(|event| event.id.as_str()).collect();
        assert_eq!(ids, vec!["EV-1", "EV-3"], "只命中订阅类型且按 seq 升序");

        claim_delivery(&conn, "ci", "EV-1", 1, 3)
            .expect("认领")
            .expect("首次");
        let pending = pending_events(&conn, "ci", &kinds, 10, 3).expect("再次");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "EV-3", "已认领的事件不再出现在计划里");

        // 另一个端点尚未认领 EV-1，仍应看到它。
        let other = pending_events(&conn, "ops", &kinds, 10, 3).expect("另一端");
        assert_eq!(other.len(), 2, "端点之间互不影响");

        // 越界与空列表显式报错。
        assert_eq!(
            pending_events(&conn, "ci", &[], 10, 3)
                .expect_err("空类型")
                .code,
            ErrorCode::InvalidState
        );
        assert_eq!(
            pending_events(&conn, "ci", &kinds, 0, 3)
                .expect_err("limit=0")
                .code,
            ErrorCode::InvalidState
        );
    }

    /// 剩余拒绝分支：控制字符、失败标记越界与未认领、统计漂移、待投递上限。
    #[test]
    fn remaining_rejections_and_drift_are_explicit() {
        let conn = fixture();
        assert_eq!(
            idempotency_key("ci", "EV\u{7}").expect_err("控制字符").code,
            ErrorCode::InvalidState
        );
        assert_eq!(
            mark_failed(&conn, "ci:EV-1", Some(500), "网络中断", false, 0)
                .expect_err("max_attempts=0")
                .code,
            ErrorCode::InvalidState
        );
        assert_eq!(
            mark_failed(&conn, "ci:EV-404", None, "网络中断", false, 3)
                .expect_err("未认领")
                .code,
            ErrorCode::NotFound
        );

        // 错误文本按字符边界截断到 MAX_ERROR_CHARS（多字节字符不切坏）。
        claim_delivery(&conn, "ci", "EV-1", 1, 3)
            .expect("认领")
            .expect("首次");
        let long_error = "错".repeat(MAX_ERROR_CHARS + 20);
        let record = mark_failed(&conn, "ci:EV-1", Some(503), &long_error, false, 3).expect("失败");
        let text = record.last_error.expect("错误文本");
        assert_eq!(text.chars().count(), MAX_ERROR_CHARS);
        assert!(text.starts_with('错'));

        // 待投递查询的四类越界。
        let kinds = vec!["TurnFinished".to_string()];
        let too_many: Vec<String> = (0..=MAX_EVENT_KINDS)
            .map(|_| "TurnFinished".into())
            .collect();
        for (err, why) in [
            (
                pending_events(&conn, "ci", &too_many, 10, 3).expect_err("类型超限"),
                "类型超限",
            ),
            (
                pending_events(&conn, "ci", &kinds, 10, 0).expect_err("次数越界"),
                "次数越界",
            ),
            (
                pending_events(&conn, "ci", &kinds, MAX_DELIVERY_ROWS + 1, 3)
                    .expect_err("limit 超限"),
                "limit 超限",
            ),
            (
                pending_events(&conn, "  ", &kinds, 10, 3).expect_err("端点为空"),
                "端点为空",
            ),
        ] {
            assert_eq!(err.code, ErrorCode::InvalidState, "{why}: {}", err.message);
        }

        // 列表上限夹取：超过上限不报错，按上限执行。
        assert!(
            list_deliveries(&conn, None, MAX_DELIVERY_ROWS + 100)
                .expect("夹取")
                .len()
                <= MAX_DELIVERY_ROWS
        );

        // 统计遇到漂移状态必须显式报错（与列表同口径）。
        conn.execute(
            "INSERT INTO webhook_deliveries
                 (idempotency_key, endpoint, event_id, event_seq, status, attempts)
             VALUES ('ci:EV-9', 'ci', 'EV-9', 9, 'unknown', 1)",
            [],
        )
        .expect("插入漂移行");
        let err = count_by_status(&conn).expect_err("漂移状态必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("unknown"), "{}", err.message);
    }

    /// 存储失败必须显式 `IoError`（不静默零／空）：表不可用时每个入口都报错。
    #[test]
    fn storage_failure_is_explicit_io_error() {
        let conn = fixture();
        conn.execute("DROP TABLE webhook_deliveries", [])
            .expect("删表模拟存储不可用");
        let kinds = vec!["TurnFinished".to_string()];
        let checks: Vec<(WhError, &str)> = vec![
            (
                claim_delivery(&conn, "ci", "EV-1", 1, 3).expect_err("认领"),
                "认领",
            ),
            (
                mark_delivered(&conn, "ci:EV-1", Some(200)).expect_err("标记成功"),
                "标记成功",
            ),
            (
                mark_failed(&conn, "ci:EV-1", None, "网络中断", false, 3).expect_err("标记失败"),
                "标记失败",
            ),
            (list_deliveries(&conn, None, 10).expect_err("列表"), "列表"),
            (
                list_deliveries(&conn, Some(DeliveryStatus::Failed), 10).expect_err("按状态列表"),
                "按状态列表",
            ),
            (count_by_status(&conn).expect_err("统计"), "统计"),
            (
                pending_events(&conn, "ci", &kinds, 10, 3).expect_err("待投递"),
                "待投递",
            ),
        ];
        for (err, why) in checks {
            assert_eq!(err.code, ErrorCode::IoError, "{why}: {}", err.message);
            assert!(
                err.message.starts_with("webhook "),
                "{why}: {}",
                err.message
            );
        }
    }

    /// 列表过滤与统计：三态计数固定顺序。
    #[test]
    fn list_filters_and_counts_by_status() {
        let conn = fixture();
        claim_delivery(&conn, "ci", "EV-1", 1, 3)
            .expect("1")
            .expect("a");
        claim_delivery(&conn, "ci", "EV-2", 2, 3)
            .expect("2")
            .expect("b");
        mark_delivered(&conn, "ci:EV-2", Some(200)).expect("成功");
        let failed = list_deliveries(&conn, Some(DeliveryStatus::Failed), 10).expect("失败列表");
        assert!(failed.is_empty(), "尚无失败记录");
        let claimed = list_deliveries(&conn, Some(DeliveryStatus::Claimed), 10).expect("认领列表");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].event_id, "EV-1");
        let counts = count_by_status(&conn).expect("统计");
        assert_eq!(counts, [1, 1, 0]);
    }
}
