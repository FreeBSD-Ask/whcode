//! 事件写者原型（tasks.md T026；V-005、FR-502/503、ADR-002/006）。
//!
//! M0 原型职责：验证四项行为——
//! 1. **幂等键去重**：同一 `idempotency_key` 重复提交返回既有 `seq`（V-106）；
//! 2. **批次阈值**：达到行数上限或时间窗口先到者触发一次事务提交（FR-503）；
//! 3. **关键事件确认**：关键事件立即 flush，并在事务成功后返回持久化确认（§7.3-5）；
//! 4. **写入错误传播**：提交失败时返回错误且**不静默丢弃**待写批次（§7.3）。
//!
//! 边界（ADR-002）：本模块是**唯一**接受数据库写命令的组件；此处的待写缓冲是**持久化队列**
//! （ADR-006 的持久化侧），与 `whcode_app::BoundedQueue`（广播/路由侧）是不同队列，
//! 不共用实现，避免把两类可靠性语义混为一谈。
//!
//! 完整写者服务（专用写线程）已在 [`crate::service`] 交付（T034）；期望状态版本、
//! 预聚合维护与 WAL 策略仍待后续任务。

use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::{Connection, params_from_iter};
use whcode_domain::{ErrorCode, EventCriticality, EventRecord, MAX_PAYLOAD_BYTES, WhError};

/// M0 原型事件表（**刻意不与 M1 正式 `events` 表同名**，避免与 T032 的 14 表 schema 迁移冲突）。
///
/// 正式 schema 见 data-model.md §1 与 T032；本表的多余列 `idempotency_key` 是原型期去重机制，
/// 正式实现将按 §7.3 以独立去重表/期望状态版本承载。
///
/// `idempotency_key` 上的 UNIQUE 约束是去重的**权威来源**——内存索引只是快路径。
const EVENTS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS m0_events (
    seq               BIGINT PRIMARY KEY,
    id                TEXT NOT NULL,
    idempotency_key   TEXT NOT NULL UNIQUE,
    session_id        TEXT,
    task_id           TEXT,
    request_id        TEXT,
    tool_call_id      TEXT,
    kind              TEXT NOT NULL,
    payload_json      TEXT NOT NULL,
    ts_ms             BIGINT NOT NULL,
    persistence_level TEXT NOT NULL
)";

const INSERT_EVENT_COLUMNS: &str = "seq, id, idempotency_key, session_id, task_id, request_id, tool_call_id, \
     kind, payload_json, ts_ms, persistence_level";

/// 单行插入的参数个数（与 `m0_events` 列数一致）。
const COLUMNS_PER_ROW: usize = 11;

/// 单个 INSERT 语句的**行数上限**：参数总数 = 行数 × 11，控制在上限内避免
/// prepared statement 参数爆炸；分块大小是吞吐与内存的折中（可配置基线，FR-503 同源）。
const ROWS_PER_INSERT: usize = 64;

/// 批量查询已存在键时的分块大小（同样避免超长 IN 列表）。
const KEYS_PER_QUERY: usize = 256;

/// 写者配置（基线值可配置，须由 M0/M1 基准校准，FR-503、§5.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterConfig {
    /// 批次行数上限（默认 500）。
    pub batch_rows: usize,
    /// 批次时间窗口上限（毫秒，默认 50）。
    pub batch_window_ms: u64,
    /// 待写缓冲上限；满载时关键条目返回背压（不得静默丢弃）。
    pub max_pending: usize,
    /// 幂等键内存索引上限（FIFO 淘汰；权威去重仍由数据库 UNIQUE 约束保证）。
    pub idempotency_cache: usize,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            batch_rows: 500,
            batch_window_ms: 50,
            max_pending: 10_000,
            idempotency_cache: 8_192,
        }
    }
}

/// 单次提交的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// 已提交并分配了新 `seq`（持久化确认，§7.3-5）。
    Committed {
        /// 分配的事件序号。
        seq: u64,
    },
    /// 命中既有幂等键：返回既有 `seq`，未重复创建（V-106）。
    Deduplicated {
        /// 既有事件序号。
        seq: u64,
    },
    /// 已进入待写缓冲，等待阈值或窗口触发提交。
    Buffered,
}

/// 单条写入的刷新结果（内部使用）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct FlushEntry {
    idempotency_key: String,
    seq: u64,
    /// 本次刷新是否真的插入了新行（false = 命中既有键，返回既有 seq）。
    inserted: bool,
}

struct PendingEvent {
    record: EventRecord,
    idempotency_key: String,
}

/// 批量插入前的扁平化行（`flush_entries` 的中间结构）。
struct PendingRow {
    seq: u64,
    id: String,
    idempotency_key: String,
    session_id: Option<String>,
    task_id: Option<String>,
    request_id: Option<String>,
    tool_call_id: Option<String>,
    kind: &'static str,
    payload_json: String,
    ts_ms: u64,
    persistence_level: &'static str,
}

/// 一次性查出给定幂等键中**已在库**的 `key → seq` 映射。
///
/// 分块发查询（避免超长 IN 列表）；UNIQUE 索引保证键在库内至多一行。
fn load_existing_keys(
    tx: &duckdb::Transaction<'_>,
    keys: &[String],
) -> Result<HashMap<String, u64>, WhError> {
    let mut existing = HashMap::new();
    for chunk in keys.chunks(KEYS_PER_QUERY) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT idempotency_key, seq FROM m0_events WHERE idempotency_key IN ({placeholders})"
        );
        let mut stmt = tx.prepare(&sql).map_err(|err| flush_err(&err))?;
        let rows = stmt
            .query_map(params_from_iter(chunk.iter()), |row| {
                let key: String = row.get(0)?;
                let seq: i64 = row.get(1)?;
                Ok((key, seq.max(0) as u64))
            })
            .map_err(|err| flush_err(&err))?;
        for item in rows {
            let (key, seq) = item.map_err(|err| flush_err(&err))?;
            existing.insert(key, seq);
        }
    }
    Ok(existing)
}

/// 把待插入行按 [`ROWS_PER_INSERT`] 分块、以多行 VALUES 批量插入。
///
/// 纪律：全部值经参数化绑定，**不拼接字面量 SQL**（§10.1 不可信数据不得成为控制指令）。
fn insert_rows(tx: &duckdb::Transaction<'_>, rows: &[PendingRow]) -> Result<(), WhError> {
    for chunk in rows.chunks(ROWS_PER_INSERT) {
        let row_placeholders = format!("({})", ["?"; COLUMNS_PER_ROW].join(", "));
        let values = vec![row_placeholders; chunk.len()].join(", ");
        let sql = format!(
            "INSERT INTO m0_events ({INSERT_EVENT_COLUMNS}) VALUES {values} \
             ON CONFLICT (idempotency_key) DO NOTHING"
        );
        let flat = chunk.iter().flat_map(|row| {
            [
                duckdb::types::Value::from(row.seq as i64),
                duckdb::types::Value::from(row.id.clone()),
                duckdb::types::Value::from(row.idempotency_key.clone()),
                row.session_id
                    .clone()
                    .map(duckdb::types::Value::from)
                    .unwrap_or(duckdb::types::Value::Null),
                row.task_id
                    .clone()
                    .map(duckdb::types::Value::from)
                    .unwrap_or(duckdb::types::Value::Null),
                row.request_id
                    .clone()
                    .map(duckdb::types::Value::from)
                    .unwrap_or(duckdb::types::Value::Null),
                row.tool_call_id
                    .clone()
                    .map(duckdb::types::Value::from)
                    .unwrap_or(duckdb::types::Value::Null),
                duckdb::types::Value::from(row.kind.to_string()),
                duckdb::types::Value::from(row.payload_json.clone()),
                duckdb::types::Value::from(row.ts_ms as i64),
                duckdb::types::Value::from(row.persistence_level.to_string()),
            ]
        });
        tx.execute(sql.as_str(), params_from_iter(flat))
            .map_err(|err| flush_err(&err))?;
    }
    Ok(())
}

/// 事件写者原型。
pub struct EventWriter {
    conn: Connection,
    config: WriterConfig,
    pending: Vec<PendingEvent>,
    known: HashMap<String, u64>,
    known_order: VecDeque<String>,
    first_pending_ms: Option<u64>,
}

impl EventWriter {
    /// 在既有连接上建立写者（必要时创建 `m0_events` 原型表）。
    ///
    /// # Panics
    ///
    /// 配置项为 0 时 panic——与 `whcode_app::BoundedQueue` 一致：容量类错误属启动期
    /// 配置错误，必须在启动时暴露，而不是运行期静默丢事件。
    pub fn new(conn: Connection, config: WriterConfig) -> Result<Self, WhError> {
        assert!(config.batch_rows > 0, "batch_rows 必须大于 0（§5.3）");
        assert!(config.max_pending > 0, "max_pending 必须大于 0（§5.3）");
        assert!(
            config.idempotency_cache > 0,
            "idempotency_cache 必须大于 0（有界结构要求）"
        );
        conn.execute_batch(EVENTS_SCHEMA)
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("创建事件表失败：{err}")))?;
        Ok(Self {
            conn,
            config,
            pending: Vec::new(),
            known: HashMap::new(),
            known_order: VecDeque::new(),
            first_pending_ms: None,
        })
    }

    /// 生效配置。
    pub fn config(&self) -> WriterConfig {
        self.config
    }

    /// 当前待写条目数（诊断用）。
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 当前幂等键内存索引条目数（诊断用）。
    pub fn known_len(&self) -> usize {
        self.known.len()
    }

    /// 提交一条事件；时间取自系统时钟。
    ///
    /// # Errors
    ///
    /// 幂等键为空、载荷超限、待写缓冲饱和或提交失败时返回 [`WhError`]。
    pub fn submit(
        &mut self,
        record: EventRecord,
        idempotency_key: impl Into<String>,
    ) -> Result<WriteOutcome, WhError> {
        self.submit_at(record, idempotency_key, now_ms())
    }

    /// 提交一条事件，并显式给定当前时间（毫秒）；供测试控制批次窗口（避免依赖真实时钟）。
    ///
    /// # Errors
    ///
    /// 同 [`EventWriter::submit`]。
    pub fn submit_at(
        &mut self,
        record: EventRecord,
        idempotency_key: impl Into<String>,
        now_ms: u64,
    ) -> Result<WriteOutcome, WhError> {
        let idempotency_key = idempotency_key.into();
        if idempotency_key.is_empty() {
            return Err(WhError::invalid_state(
                "幂等键不得为空：空键无法去重，重复提交将产生重复状态转换（V-106）",
            ));
        }
        if !record.payload_within_limit() {
            return Err(WhError::invalid_state(format!(
                "载荷超过 {MAX_PAYLOAD_BYTES} 字节上限：必须改走内容表/引用并留下可查询标记，\
                 不得静默截断（R-05、FR-208）"
            )));
        }
        if let Some(seq) = self.known.get(&idempotency_key) {
            return Ok(WriteOutcome::Deduplicated { seq: *seq });
        }
        // 同一键已在待写批次中：不得重复入队（否则失败重试会把同一事件反复堆积，
        // 直到批次上限，且审计侧看到的是同一个键的多次入队记录）。
        if self
            .pending
            .iter()
            .any(|item| item.idempotency_key == idempotency_key)
        {
            return Ok(WriteOutcome::Buffered);
        }
        if self.pending.len() >= self.config.max_pending {
            return Err(WhError::backpressure(format!(
                "待写缓冲已满（{}）：关键条目未被接收，调用方须背压、重试或安全停止（§7.3-2）",
                self.config.max_pending
            )));
        }

        let critical = record.criticality() == EventCriticality::Critical;
        self.pending.push(PendingEvent {
            record,
            idempotency_key: idempotency_key.clone(),
        });
        if self.first_pending_ms.is_none() {
            self.first_pending_ms = Some(now_ms);
        }

        let window_elapsed = self
            .first_pending_ms
            .is_some_and(|first| now_ms.saturating_sub(first) >= self.config.batch_window_ms);
        let threshold_hit = self.pending.len() >= self.config.batch_rows;
        if !(critical || threshold_hit || window_elapsed) {
            return Ok(WriteOutcome::Buffered);
        }

        let entries = self.flush_entries()?;
        match entries
            .into_iter()
            .find(|entry| entry.idempotency_key == idempotency_key)
        {
            Some(entry) if entry.inserted => Ok(WriteOutcome::Committed { seq: entry.seq }),
            Some(entry) => Ok(WriteOutcome::Deduplicated { seq: entry.seq }),
            None => Err(WhError::internal(
                "提交后的刷新结果中缺少该幂等键：写者内部状态不一致",
            )),
        }
    }

    /// 刷新待写缓冲；返回**新增**行数。
    ///
    /// 单个事务内提交全部待写条目；失败时事务回滚且**保留**待写批次（不静默丢失），
    /// 调用方可依幂等键安全重试。
    ///
    /// # Errors
    ///
    /// 数据库提交失败时返回 `io_error`（`retryable = true`：重试不会造成副作用重复）。
    pub fn flush(&mut self) -> Result<usize, WhError> {
        let entries = self.flush_entries()?;
        Ok(entries.iter().filter(|entry| entry.inserted).count())
    }

    /// 若距首条待写事件已超过时间窗口，则刷新；返回**新增**行数（未到期返回 0）。
    ///
    /// M0 为同步原型，没有后台定时器：时间窗口的到期判定发生在 `submit_at` 与本次显式
    /// 调用上。调用方（未来的写者服务线程，M1/T034）必须周期性调用本方法，否则单条事件
    /// 可能一直滞留——本文件不得声称“50 ms 必然提交”。
    ///
    /// # Errors
    ///
    /// 同 [`EventWriter::flush`]。
    pub fn flush_if_due(&mut self, now_ms: u64) -> Result<usize, WhError> {
        let due = self
            .first_pending_ms
            .is_some_and(|first| now_ms.saturating_sub(first) >= self.config.batch_window_ms);
        if !due {
            return Ok(0);
        }
        self.flush()
    }

    fn flush_entries(&mut self) -> Result<Vec<FlushEntry>, WhError> {
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }

        // 单写者事务（ADR-002）：查询已存在键与批量插入必须在同一事务内，才能保证
        // “查询判定为不存在 → 插入必然成功”这一推理成立。
        let tx = self.conn.transaction().map_err(|err| flush_err(&err))?;

        // 步骤一：一次性查出本批次中**已在库**的幂等键及其 seq。
        // 这是去重的权威判定（UNIQUE 索引为最终保证，此处是批内快路径）。
        let keys: Vec<String> = self
            .pending
            .iter()
            .map(|item| item.idempotency_key.clone())
            .collect();
        let existing = load_existing_keys(&tx, &keys)?;

        // 步骤二：为不在库中的条目分配连续 seq，并以多行 VALUES 分块批量插入。
        // 原实现是“逐条 INSERT + 逐条回读 seq”（2N 次往返），是吞吐瓶颈；
        // 现在为 1 次批量查询 + ceil(N/分块) 次批量插入。
        let max_seq: i64 = tx
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM m0_events", [], |row| {
                row.get(0)
            })
            .map_err(|err| flush_err(&err))?;
        let mut next_seq = max_seq.max(0) as u64 + 1;

        let mut entries: Vec<FlushEntry> = Vec::with_capacity(self.pending.len());
        // 待插入行的扁平参数（每行 11 个参数，与 INSERT_EVENT_COLUMNS 对应）。
        let mut rows: Vec<PendingRow> = Vec::with_capacity(self.pending.len());
        for item in &self.pending {
            if let Some(seq) = existing.get(&item.idempotency_key) {
                entries.push(FlushEntry {
                    idempotency_key: item.idempotency_key.clone(),
                    seq: *seq,
                    inserted: false,
                });
                continue;
            }
            let payload_json = serde_json::to_string(&item.record.payload)
                .map_err(|err| WhError::internal(format!("事件载荷序列化失败：{err}")))?;
            rows.push(PendingRow {
                seq: next_seq,
                id: item.record.id.clone(),
                idempotency_key: item.idempotency_key.clone(),
                session_id: item.record.session_id.clone(),
                task_id: item.record.task_id.clone(),
                request_id: item.record.request_id.clone(),
                tool_call_id: item.record.tool_call_id.clone(),
                kind: item.record.kind.as_str(),
                payload_json,
                ts_ms: item.record.ts_ms,
                persistence_level: item.record.persistence_level.as_str(),
            });
            next_seq += 1;
        }

        insert_rows(&tx, &rows)?;

        // 插入后即可确定 seq（无并发写者，事务内无他人插入），**不需要回读**。
        for row in &rows {
            entries.push(FlushEntry {
                idempotency_key: row.idempotency_key.clone(),
                seq: row.seq,
                inserted: true,
            });
        }

        tx.commit().map_err(|err| flush_err(&err))?;

        for entry in &entries {
            self.remember(&entry.idempotency_key, entry.seq);
        }
        self.pending.clear();
        self.first_pending_ms = None;
        Ok(entries)
    }

    fn remember(&mut self, idempotency_key: &str, seq: u64) {
        if self
            .known
            .insert(idempotency_key.to_string(), seq)
            .is_none()
        {
            self.known_order.push_back(idempotency_key.to_string());
        }
        while self.known.len() > self.config.idempotency_cache {
            let Some(oldest) = self.known_order.pop_front() else {
                break;
            };
            self.known.remove(&oldest);
        }
    }
}

fn flush_err(err: &duckdb::Error) -> WhError {
    WhError::new(
        ErrorCode::IoError,
        format!("事件批次提交失败（待写批次已保留，可依幂等键重试）：{err}"),
    )
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use whcode_domain::{EventKind, PersistenceLevel};

    fn temp_db(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("whcode-writer-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let db = dir.join("writer.duckdb");
        (dir, db)
    }

    fn writer_at(db: &Path, config: WriterConfig) -> EventWriter {
        let conn = Connection::open(db).expect("打开测试库");
        EventWriter::new(conn, config).expect("建立写者")
    }

    fn event(seq: u64, kind: EventKind) -> EventRecord {
        EventRecord {
            schema_version: whcode_domain::SCHEMA_VERSION,
            seq,
            id: format!("E-{seq}"),
            kind,
            session_id: Some("S-001".to_string()),
            task_id: None,
            request_id: None,
            tool_call_id: None,
            payload: json!({"n": seq}),
            ts_ms: 1_700_000_000_000 + seq,
            persistence_level: PersistenceLevel::Redacted,
        }
    }

    fn count_events(writer: &EventWriter) -> i64 {
        writer
            .conn
            .query_row("SELECT count(*) FROM m0_events", [], |row| row.get(0))
            .expect("统计事件")
    }

    /// 库内全部 `seq`（升序）；用于断言连续集合与无空洞、无重复。
    fn seqs_in_db(writer: &EventWriter) -> Vec<i64> {
        let mut stmt = writer
            .conn
            .prepare("SELECT seq FROM m0_events ORDER BY seq")
            .expect("准备序号查询");
        let rows = stmt
            .query_map([], |row| row.get::<_, i64>(0))
            .expect("查询序号");
        rows.map(|item| item.expect("读取序号")).collect()
    }

    /// 库内 `(幂等键, seq)`（按 seq 升序）；用于断言键到 seq 的映射未被重写。
    fn key_seq_pairs(writer: &EventWriter) -> Vec<(String, u64)> {
        let mut stmt = writer
            .conn
            .prepare("SELECT idempotency_key, seq FROM m0_events ORDER BY seq")
            .expect("准备键序查询");
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .expect("查询键序");
        rows.map(|item| {
            let (key, seq) = item.expect("读取键序");
            (key, seq.max(0) as u64)
        })
        .collect()
    }

    #[test]
    fn critical_event_is_committed_and_confirmed() {
        let (dir, db) = temp_db("critical");
        let mut writer = writer_at(&db, WriterConfig::default());
        let outcome = writer
            .submit(event(1, EventKind::ToolCallStarted), "k-1")
            .expect("关键事件应提交成功");
        assert_eq!(outcome, WriteOutcome::Committed { seq: 1 });
        assert_eq!(writer.pending_len(), 0, "关键事件提交后不应留待写条目");
        assert_eq!(count_events(&writer), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_idempotency_key_returns_existing_seq() {
        let (dir, db) = temp_db("dedup");
        let mut writer = writer_at(&db, WriterConfig::default());
        writer
            .submit(event(1, EventKind::RequestCompleted), "k-dup")
            .expect("首次提交");
        let second = writer
            .submit(event(2, EventKind::RequestCompleted), "k-dup")
            .expect("重复提交");
        assert_eq!(second, WriteOutcome::Deduplicated { seq: 1 });
        assert_eq!(count_events(&writer), 1, "不得产生重复行（V-106）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_key_in_same_batch_is_deduplicated() {
        let (dir, db) = temp_db("dedup-batch");
        let mut writer = writer_at(
            &db,
            WriterConfig {
                batch_rows: 10,
                batch_window_ms: 60_000,
                ..WriterConfig::default()
            },
        );
        assert_eq!(
            writer
                .submit_at(event(1, EventKind::StreamingChunk), "k-same", 0)
                .expect("首条入缓冲"),
            WriteOutcome::Buffered
        );
        assert_eq!(
            writer
                .submit_at(event(2, EventKind::StreamingChunk), "k-same", 1)
                .expect("同键第二条入缓冲"),
            WriteOutcome::Buffered
        );
        assert_eq!(writer.flush().expect("刷新"), 1, "同键只应新增一行");
        assert_eq!(count_events(&writer), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_key_in_pending_batch_is_not_queued_twice() {
        // 失败重试时同一键会再次提交；若重复入队，待写批次会被同一事件填满。
        let (dir, db) = temp_db("dedup-pending");
        let mut writer = writer_at(
            &db,
            WriterConfig {
                batch_rows: 10,
                batch_window_ms: 60_000,
                ..WriterConfig::default()
            },
        );
        assert_eq!(
            writer
                .submit_at(event(1, EventKind::StreamingChunk), "k-same", 0)
                .expect("首条入缓冲"),
            WriteOutcome::Buffered
        );
        assert_eq!(
            writer
                .submit_at(event(1, EventKind::StreamingChunk), "k-same", 10)
                .expect("同键重试"),
            WriteOutcome::Buffered
        );
        assert_eq!(writer.pending_len(), 1, "同键不得重复入队");
        assert_eq!(writer.flush().expect("刷新"), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_if_due_commits_only_after_window() {
        // M0 无后台定时器：窗口到期由调用方显式推进（M1/T034 由写者线程轮询）。
        let (dir, db) = temp_db("due");
        let mut writer = writer_at(
            &db,
            WriterConfig {
                batch_rows: 1_000,
                batch_window_ms: 50,
                ..WriterConfig::default()
            },
        );
        writer
            .submit_at(event(1, EventKind::StreamingChunk), "k-1", 1_000)
            .expect("入缓冲");
        assert_eq!(writer.flush_if_due(1_049).expect("未到期"), 0);
        assert_eq!(writer.pending_len(), 1);
        assert_eq!(writer.flush_if_due(1_050).expect("到期刷新"), 1);
        assert_eq!(writer.pending_len(), 0);
        assert_eq!(count_events(&writer), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_row_threshold_triggers_commit() {
        let (dir, db) = temp_db("threshold");
        let config = WriterConfig {
            batch_rows: 3,
            batch_window_ms: 60_000,
            ..WriterConfig::default()
        };
        let mut writer = writer_at(&db, config);
        assert_eq!(
            writer
                .submit_at(event(1, EventKind::StreamingChunk), "k-1", 0)
                .expect("入缓冲"),
            WriteOutcome::Buffered
        );
        assert_eq!(
            writer
                .submit_at(event(2, EventKind::StreamingChunk), "k-2", 0)
                .expect("入缓冲"),
            WriteOutcome::Buffered
        );
        assert_eq!(
            writer
                .submit_at(event(3, EventKind::StreamingChunk), "k-3", 0)
                .expect("达到阈值即提交"),
            WriteOutcome::Committed { seq: 3 }
        );
        assert_eq!(writer.pending_len(), 0);
        assert_eq!(count_events(&writer), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_window_deadline_triggers_commit() {
        let (dir, db) = temp_db("window");
        let config = WriterConfig {
            batch_rows: 1_000,
            batch_window_ms: 50,
            ..WriterConfig::default()
        };
        let mut writer = writer_at(&db, config);
        assert_eq!(
            writer
                .submit_at(event(1, EventKind::StreamingChunk), "k-1", 1_000)
                .expect("入缓冲"),
            WriteOutcome::Buffered
        );
        assert_eq!(
            writer
                .submit_at(event(2, EventKind::StreamingChunk), "k-2", 1_049)
                .expect("未到窗口仍缓冲"),
            WriteOutcome::Buffered
        );
        assert_eq!(
            writer
                .submit_at(event(3, EventKind::StreamingChunk), "k-3", 1_050)
                .expect("到达窗口即提交"),
            WriteOutcome::Committed { seq: 3 }
        );
        assert_eq!(count_events(&writer), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saturated_buffer_returns_backpressure_and_keeps_entries() {
        let (dir, db) = temp_db("backpressure");
        let config = WriterConfig {
            batch_rows: 1_000,
            batch_window_ms: 60_000,
            max_pending: 2,
            ..WriterConfig::default()
        };
        let mut writer = writer_at(&db, config);
        for (index, key) in ["k-1", "k-2"].iter().enumerate() {
            assert_eq!(
                writer
                    .submit_at(event(index as u64 + 1, EventKind::StreamingChunk), *key, 0)
                    .expect("容量内应入缓冲"),
                WriteOutcome::Buffered
            );
        }
        let err = writer
            .submit_at(event(3, EventKind::StreamingChunk), "k-3", 0)
            .expect_err("满载必须返回背压");
        assert_eq!(err.code, ErrorCode::Backpressure);
        assert!(err.retryable);
        assert_eq!(writer.pending_len(), 2, "已接受条目不得被丢弃");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_failure_propagates_and_batch_is_retained() {
        let (dir, db) = temp_db("failure");
        let mut writer = writer_at(
            &db,
            WriterConfig {
                batch_rows: 1_000,
                batch_window_ms: 60_000,
                ..WriterConfig::default()
            },
        );
        writer
            .submit_at(event(1, EventKind::StreamingChunk), "k-1", 0)
            .expect("入缓冲");
        assert_eq!(writer.pending_len(), 1);

        // 制造真实写入失败：删除事件表（同一连接，无多连接干扰）。
        writer
            .conn
            .execute_batch("DROP TABLE m0_events")
            .expect("删除事件表");

        let err = writer.flush().expect_err("刷新必须失败并上报");
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(err.retryable, "重试不会造成副作用重复");
        assert_eq!(
            writer.pending_len(),
            1,
            "失败后待写批次必须保留，不得静默丢失"
        );
        assert_eq!(writer.known_len(), 0, "失败不得写入幂等索引");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_payload_is_rejected_not_truncated() {
        let (dir, db) = temp_db("oversized");
        let mut writer = writer_at(&db, WriterConfig::default());
        let mut big = event(1, EventKind::StreamingChunk);
        big.payload = json!("x".repeat(MAX_PAYLOAD_BYTES + 1));
        let err = writer.submit(big, "k-big").expect_err("超限载荷必须被拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert_eq!(writer.pending_len(), 0);
        assert_eq!(count_events(&writer), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_idempotency_key_is_rejected() {
        let (dir, db) = temp_db("empty-key");
        let mut writer = writer_at(&db, WriterConfig::default());
        let err = writer
            .submit(event(1, EventKind::ToolCallFinished), "")
            .expect_err("空幂等键必须被拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[should_panic(expected = "batch_rows 必须大于 0")]
    fn zero_batch_rows_is_rejected() {
        let (dir, db) = temp_db("zero-batch");
        let _ = writer_at(
            &db,
            WriterConfig {
                batch_rows: 0,
                ..WriterConfig::default()
            },
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_larger_than_insert_chunk_keeps_seq_contiguous() {
        // 批量插入按 ROWS_PER_INSERT（64）分块：150 行必须跨 3 条 INSERT 语句。
        // 若分块之间 seq 计数被重置或重复分配，下面的集合断言会直接抓到。
        const ROWS: usize = 150;
        const _: () = assert!(ROWS > ROWS_PER_INSERT * 2, "用例必须跨至少 3 个插入分块");
        let (dir, db) = temp_db("insert-chunk");
        let mut writer = writer_at(
            &db,
            WriterConfig {
                batch_rows: 1_000,
                batch_window_ms: 60_000,
                ..WriterConfig::default()
            },
        );
        for index in 0..ROWS {
            assert_eq!(
                writer
                    .submit_at(
                        event(index as u64 + 1, EventKind::StreamingChunk),
                        format!("k-{index}"),
                        0
                    )
                    .expect("入缓冲"),
                WriteOutcome::Buffered,
                "未达阈值与窗口时不得提前提交"
            );
        }
        assert_eq!(writer.pending_len(), ROWS, "全部条目应在同一批内");

        assert_eq!(
            writer.flush().expect("批量刷新"),
            ROWS,
            "返回的新增行数应与输入一致"
        );
        assert_eq!(count_events(&writer), ROWS as i64, "库内行数应与输入一致");
        assert_eq!(
            seqs_in_db(&writer),
            (1..=ROWS as i64).collect::<Vec<_>>(),
            "跨分块写入的 seq 必须连续、无空洞、无重复"
        );
        assert_eq!(writer.pending_len(), 0, "成功后待写批次应清空");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedup_across_flushes_returns_existing_seq() {
        // 跨批次去重：第一批已提交的键，在第二批（新写者实例、新事务、内存索引为空）
        // 重复提交时必须返回既有 seq，而不是重新分配或被忽略。
        let config = WriterConfig {
            batch_rows: 3,
            batch_window_ms: 60_000,
            ..WriterConfig::default()
        };
        let (dir, db) = temp_db("dedup-across-flush");
        let first_pairs = {
            let mut writer = writer_at(&db, config);
            for (index, key) in ["k-1", "k-2", "k-3"].iter().enumerate() {
                let outcome = writer
                    .submit_at(event(index as u64 + 1, EventKind::StreamingChunk), *key, 0)
                    .expect("首批提交");
                if index < 2 {
                    assert_eq!(outcome, WriteOutcome::Buffered, "未达阈值");
                } else {
                    assert_eq!(
                        outcome,
                        WriteOutcome::Committed { seq: 3 },
                        "达到行数阈值触发首次提交"
                    );
                }
            }
            let pairs = key_seq_pairs(&writer);
            assert_eq!(
                pairs,
                vec![
                    ("k-1".to_string(), 1),
                    ("k-2".to_string(), 2),
                    ("k-3".to_string(), 3),
                ],
                "首批 seq 应为 1..=3"
            );
            pairs
        };

        let mut second = writer_at(&db, config);
        for (index, key) in ["k-1", "k-2", "k-3"].iter().enumerate() {
            let mut record = event(index as u64 + 1, EventKind::StreamingChunk);
            if *key == "k-2" {
                // 其中一个键换了载荷：去重只认幂等键，载荷不参与判定，也不得覆盖既有行。
                record.payload = json!({"n": 999, "changed": true});
            }
            let outcome = second.submit_at(record, *key, 0).expect("跨批重复提交");
            if index < 2 {
                assert_eq!(outcome, WriteOutcome::Buffered, "新实例内存索引为空");
            } else {
                assert_eq!(
                    outcome,
                    WriteOutcome::Deduplicated { seq: 3 },
                    "第二批必须命中既有 seq，而非新分配"
                );
            }
        }
        assert_eq!(
            second.flush().expect("第二批刷新"),
            0,
            "跨批重复键不得新增行"
        );
        assert_eq!(count_events(&second), 3, "库内行数不得增加");
        assert_eq!(
            key_seq_pairs(&second),
            first_pairs,
            "既有键到 seq 的映射不得被重写"
        );
        assert_eq!(second.known.get("k-2"), Some(&2), "内存索引应记录既有 seq");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn many_keys_exceed_query_chunk_still_dedups() {
        // 去重权威在数据库 UNIQUE 约束，而不是写者内存缓存：新实例（模拟进程重启后
        // 内存索引失效）重复提交超过 KEYS_PER_QUERY 的键时必须全部命中既有行。
        // 若 IN 查询只覆盖首个分块，后续分块的键会被当作新键插入（并被 UNIQUE 抑制），
        // 导致返回的 seq 与库内既有 seq 不一致、且 flush 报告新增行数非零。
        const KEYS: usize = 300;
        const _: () = assert!(KEYS > KEYS_PER_QUERY, "用例必须跨至少 2 个 IN 查询分块");
        let config = WriterConfig {
            batch_rows: 1_000,
            batch_window_ms: 60_000,
            ..WriterConfig::default()
        };
        let (dir, db) = temp_db("dedup-many-keys");
        let expected_keys: Vec<String> = (0..KEYS).map(|index| format!("k-{index}")).collect();

        let first_pairs = {
            let mut writer = writer_at(&db, config);
            for (index, key) in expected_keys.iter().enumerate() {
                assert_eq!(
                    writer
                        .submit_at(
                            event(index as u64 + 1, EventKind::StreamingChunk),
                            key.clone(),
                            0
                        )
                        .expect("首批入缓冲"),
                    WriteOutcome::Buffered
                );
            }
            assert_eq!(writer.flush().expect("首批提交"), KEYS, "首批应全部新增");
            assert_eq!(count_events(&writer), KEYS as i64);
            let pairs = key_seq_pairs(&writer);
            assert_eq!(
                pairs.iter().map(|(_, seq)| *seq).collect::<Vec<_>>(),
                (1..=KEYS as u64).collect::<Vec<_>>(),
                "首批 seq 必须连续无空洞"
            );
            assert_eq!(
                pairs.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>(),
                expected_keys,
                "键顺序应与提交顺序一致"
            );
            pairs
        };

        let mut restarted = writer_at(&db, config);
        for (index, key) in expected_keys.iter().enumerate() {
            assert_eq!(
                restarted
                    .submit_at(
                        event(index as u64 + 5_000, EventKind::StreamingChunk),
                        key.clone(),
                        0
                    )
                    .expect("重启后重复提交"),
                WriteOutcome::Buffered,
                "新实例内存索引为空，重复键应入队等待数据库判定"
            );
        }
        assert_eq!(
            restarted.flush().expect("重复批刷新"),
            0,
            "全部命中既有键时不得报告新增行"
        );
        assert_eq!(
            count_events(&restarted),
            KEYS as i64,
            "跨 IN 分块去重后库内行数不得增加"
        );
        for (index, key) in expected_keys.iter().enumerate() {
            assert_eq!(
                restarted.known.get(key),
                Some(&(index as u64 + 1)),
                "每个键都必须返回既有 seq，而不是新分配"
            );
        }
        assert_eq!(
            key_seq_pairs(&restarted),
            first_pairs,
            "重启后的去重不得改动键到 seq 的映射"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retry_after_failed_flush_keeps_seq_unambiguous() {
        // 失败后重试：待写批次保留，重试必须避开失败前已提交行的 seq，且幂等键仍可查。
        // 制造失败的方式是“改名而非删表”：删表会连已提交行一起丢掉，那时无从验证
        // “重试 seq 不与已提交行冲突”，断言会退化为同义反复。
        let config = WriterConfig {
            batch_rows: 1_000,
            batch_window_ms: 60_000,
            ..WriterConfig::default()
        };
        let (dir, db) = temp_db("retry-after-failure");
        let mut writer = writer_at(&db, config);
        for (index, key) in ["k-1", "k-2"].iter().enumerate() {
            writer
                .submit_at(event(index as u64 + 1, EventKind::StreamingChunk), *key, 0)
                .expect("首批入缓冲");
        }
        assert_eq!(writer.flush().expect("首批提交"), 2);
        assert_eq!(seqs_in_db(&writer), vec![1, 2], "首批 seq 应为 1、2");

        for (index, key) in ["k-3", "k-4"].iter().enumerate() {
            writer
                .submit_at(event(index as u64 + 3, EventKind::StreamingChunk), *key, 1)
                .expect("第二批入缓冲");
        }
        writer
            .conn
            .execute_batch("ALTER TABLE m0_events RENAME TO m0_events_offline")
            .expect("改名事件表以制造失败");
        let err = writer.flush().expect_err("刷新必须失败并上报");
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(err.retryable, "重试不会造成副作用重复");
        assert_eq!(writer.pending_len(), 2, "失败后待写批次必须保留");
        assert_eq!(writer.known_len(), 2, "失败不得写入幂等索引");

        writer
            .conn
            .execute_batch("ALTER TABLE m0_events_offline RENAME TO m0_events")
            .expect("恢复事件表");
        assert_eq!(writer.flush().expect("重试必须成功"), 2, "重试应写入两条");
        assert_eq!(writer.pending_len(), 0, "成功后待写批次应清空");
        assert_eq!(
            seqs_in_db(&writer),
            vec![1, 2, 3, 4],
            "重试 seq 必须从 3 起、无重复、且不与失败前已提交行冲突"
        );
        assert_eq!(
            key_seq_pairs(&writer),
            vec![
                ("k-1".to_string(), 1),
                ("k-2".to_string(), 2),
                ("k-3".to_string(), 3),
                ("k-4".to_string(), 4),
            ],
            "幂等键索引必须可查到重试写入的行"
        );
        assert_eq!(writer.known.get("k-3"), Some(&3));
        assert_eq!(writer.known.get("k-4"), Some(&4));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
